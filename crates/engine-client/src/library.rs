// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Checked personal-library access over the supervised engine connection.
use crate::{ClientError, EngineRpc, native::rpc_error};
use prost::Message;
use spiling_contracts::{MAX_CONTROL_BYTES, geometry::NativePath, library::*, rpc};
fn invalid(message: String) -> ClientError {
    ClientError::InvalidRequest(message)
}
fn checked<T: Message>(message: T) -> Result<T, ClientError> {
    if message.encoded_len() > MAX_CONTROL_BYTES as usize {
        return Err(invalid("library control exceeds limit".into()));
    }
    Ok(message)
}
fn page(
    expected: Option<&LibraryIdentity>,
    actual: Option<&LibraryIdentity>,
    offset: u32,
    count: usize,
    total: u32,
    next: Option<u32>,
    limit: usize,
) -> Result<(), ClientError> {
    let end = offset
        .checked_add(count as u32)
        .ok_or_else(|| ClientError::Protocol("library page overflow".into()))?;
    if expected.is_some_and(|id| Some(id) != actual)
        || count > limit
        || end > total
        || next != (end < total).then_some(end)
        || (end < total && count == 0)
    {
        return Err(ClientError::Protocol(
            "library page disagrees with captured query".into(),
        ));
    }
    Ok(())
}
impl EngineRpc {
    fn libraries(&self) -> rpc::libraries_client::LibrariesClient<tonic::transport::Channel> {
        rpc::libraries_client::LibrariesClient::new(self.channel.clone())
            .max_encoding_message_size(MAX_CONTROL_BYTES as usize)
            .max_decoding_message_size(MAX_CONTROL_BYTES as usize)
    }
    pub async fn hardware_catalog(
        &self,
    ) -> Result<spiling_contracts::hardware::HardwareCatalog, ClientError> {
        self.libraries()
            .get_hardware_catalog(self.request(()))
            .await
            .map_err(rpc_error)?
            .into_inner()
            .try_into()
            .map_err(ClientError::Protocol)
    }
    pub async fn library_hardware(
        &self,
    ) -> Result<spiling_contracts::hardware::LibraryHardware, ClientError> {
        self.libraries()
            .get_hardware(self.request(()))
            .await
            .map_err(rpc_error)?
            .into_inner()
            .try_into()
            .map_err(ClientError::Protocol)
    }
    pub async fn library_status(&self) -> Result<LibraryStatus, ClientError> {
        self.libraries()
            .get_status(self.request(()))
            .await
            .map_err(rpc_error)?
            .into_inner()
            .try_into()
            .map_err(ClientError::Protocol)
    }
    pub async fn library_set_location(
        &self,
        location: NativePath,
    ) -> Result<LibraryStatus, ClientError> {
        let location_wire = location.clone().try_into().map_err(invalid)?;
        let status: LibraryStatus = self
            .libraries()
            .set_location(self.request(checked(rpc::LibraryLocationRequest {
                location: Some(location_wire),
            })?))
            .await
            .map_err(rpc_error)?
            .into_inner()
            .try_into()
            .map_err(ClientError::Protocol)?;
        if status.location != location {
            return Err(ClientError::Protocol(
                "library locator reply mismatch".into(),
            ));
        }
        Ok(status)
    }
    pub async fn library_filaments(
        &self,
        query: &LibraryFilamentQuery,
    ) -> Result<LibraryFilamentPage, ClientError> {
        let wire = query.clone().try_into().map_err(invalid)?;
        let result: LibraryFilamentPage = self
            .libraries()
            .list_filaments(self.request(checked(wire)?))
            .await
            .map_err(rpc_error)?
            .into_inner()
            .try_into()
            .map_err(ClientError::Protocol)?;
        page(
            query.expected.as_ref(),
            result.identity.as_ref(),
            query.offset,
            result.entries.len(),
            result.total_matches,
            result.next_offset,
            query.limit(),
        )?;
        if result.entries.iter().any(|e| {
            (!query.include_archived && e.entry.archived)
                || (query.on_hand_only && e.on_hand_spools == 0)
        }) {
            return Err(ClientError::Protocol("library filter mismatch".into()));
        }
        Ok(result)
    }
    pub async fn library_filament(
        &self,
        library: &LibraryId,
        reference: &FilamentRef,
    ) -> Result<FilamentRevision, ClientError> {
        validate_library_uuid(&library.0).map_err(invalid)?;
        let result: FilamentRevision = self
            .libraries()
            .get_filament(self.request(checked(rpc::LibraryGetFilamentRequest {
                library_id: library.0.clone(),
                reference: Some(reference.clone().try_into().map_err(invalid)?),
            })?))
            .await
            .map_err(rpc_error)?
            .into_inner()
            .try_into()
            .map_err(ClientError::Protocol)?;
        if &result.reference != reference {
            return Err(ClientError::Protocol(
                "library revision reply mismatch".into(),
            ));
        }
        Ok(result)
    }
    pub async fn library_spools(
        &self,
        query: &LibrarySpoolQuery,
    ) -> Result<LibrarySpoolPage, ClientError> {
        let wire = query.clone().try_into().map_err(invalid)?;
        let result: LibrarySpoolPage = self
            .libraries()
            .list_spools(self.request(checked(wire)?))
            .await
            .map_err(rpc_error)?
            .into_inner()
            .try_into()
            .map_err(ClientError::Protocol)?;
        page(
            query.expected.as_ref(),
            result.identity.as_ref(),
            query.offset,
            result.spools.len(),
            result.total_matches,
            result.next_offset,
            query.limit(),
        )?;
        if result.spools.iter().any(|s| {
            query
                .filament
                .as_ref()
                .is_some_and(|f| f.id != s.filament.id)
        }) {
            return Err(ClientError::Protocol(
                "library stock filter mismatch".into(),
            ));
        }
        Ok(result)
    }
    pub async fn library_recipes(
        &self,
        query: &LibraryRecipeQuery,
    ) -> Result<LibraryRecipePage, ClientError> {
        let wire = query.clone().try_into().map_err(invalid)?;
        let result: LibraryRecipePage = self
            .libraries()
            .list_recipes(self.request(checked(wire)?))
            .await
            .map_err(rpc_error)?
            .into_inner()
            .try_into()
            .map_err(ClientError::Protocol)?;
        page(
            query.expected.as_ref(),
            result.identity.as_ref(),
            query.offset,
            result.entries.len(),
            result.total_matches,
            result.next_offset,
            MAX_LIBRARY_PAGE as usize,
        )?;
        Ok(result)
    }
    pub async fn library_recipe(
        &self,
        library: &LibraryId,
        reference: &RecipeRef,
    ) -> Result<RecipeTransfer, ClientError> {
        validate_library_uuid(&library.0).map_err(invalid)?;
        let result: RecipeTransfer = self
            .libraries()
            .get_recipe(self.request(checked(rpc::LibraryGetRecipeRequest {
                library_id: library.0.clone(),
                reference: Some(reference.clone().try_into().map_err(invalid)?),
            })?))
            .await
            .map_err(rpc_error)?
            .into_inner()
            .try_into()
            .map_err(ClientError::Protocol)?;
        if result.recipe.reference() != *reference {
            return Err(ClientError::Protocol(
                "recipe reply differs from exact request".into(),
            ));
        }
        Ok(result)
    }
    pub async fn library_write(
        &self,
        request: &LibraryWrite,
    ) -> Result<LibraryWriteReceipt, ClientError> {
        let wire = request.clone().try_into().map_err(invalid)?;
        let result: LibraryWriteReceipt = self
            .libraries()
            .write(self.request(checked(wire)?))
            .await
            .map_err(rpc_error)?
            .into_inner()
            .try_into()
            .map_err(ClientError::Protocol)?;
        if result.request_id != request.request_id
            || request
                .library_id
                .as_ref()
                .is_some_and(|id| id != &result.identity.id)
            || (!result.replayed && result.identity.revision != request.base_revision + 1)
        {
            return Err(ClientError::Protocol(
                "library mutation receipt mismatch".into(),
            ));
        }
        Ok(result)
    }
}
