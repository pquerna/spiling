// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
//! Checked personal-library wire records; no JSON command tunnels.
use super::required;
use crate::{geometry::SourceHash, library::*, rpc};

impl TryFrom<rpc::LibraryIdentity> for LibraryIdentity {
    type Error = String;
    fn try_from(v: rpc::LibraryIdentity) -> Result<Self, String> {
        let value = Self {
            id: LibraryId(v.id),
            revision: v.revision,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<LibraryIdentity> for rpc::LibraryIdentity {
    type Error = String;
    fn try_from(v: LibraryIdentity) -> Result<Self, String> {
        v.validate()?;
        Ok(Self {
            id: v.id.0,
            revision: v.revision,
        })
    }
}
impl TryFrom<rpc::LibraryFilamentRef> for FilamentRef {
    type Error = String;
    fn try_from(v: rpc::LibraryFilamentRef) -> Result<Self, String> {
        let value = Self {
            id: FilamentId(v.id),
            revision: SourceHash::parse(v.revision).map_err(|e| e.to_string())?,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<FilamentRef> for rpc::LibraryFilamentRef {
    type Error = String;
    fn try_from(v: FilamentRef) -> Result<Self, String> {
        v.validate()?;
        Ok(Self {
            id: v.id.0,
            revision: v.revision.as_str().into(),
        })
    }
}
impl TryFrom<rpc::LibraryDefinitionSource> for DefinitionSource {
    type Error = String;
    fn try_from(v: rpc::LibraryDefinitionSource) -> Result<Self, String> {
        use rpc::library_definition_source::Kind;
        Ok(match required(v.kind, "library definition source")? {
            Kind::Operator(()) => Self::Operator,
            Kind::Imported(v) => {
                if v.name.is_empty()
                    || v.name.len() > 256
                    || v.name.trim() != v.name
                    || v.name.chars().any(char::is_control)
                {
                    return Err("invalid imported library source name".into());
                }
                Self::Imported {
                    name: v.name,
                    sha256: SourceHash::parse(v.sha256).map_err(|e| e.to_string())?,
                }
            }
        })
    }
}
impl TryFrom<DefinitionSource> for rpc::LibraryDefinitionSource {
    type Error = String;
    fn try_from(v: DefinitionSource) -> Result<Self, String> {
        use rpc::library_definition_source::Kind;
        let value = Self {
            kind: Some(match v {
                DefinitionSource::Operator => Kind::Operator(()),
                DefinitionSource::Imported { name, sha256 } => {
                    Kind::Imported(rpc::LibraryImportedSource {
                        name,
                        sha256: sha256.as_str().into(),
                    })
                }
            }),
        };
        DefinitionSource::try_from(value.clone())?;
        Ok(value)
    }
}
impl TryFrom<rpc::LibraryAppearanceDeclaration> for DeclaredAppearance {
    type Error = String;
    fn try_from(v: rpc::LibraryAppearanceDeclaration) -> Result<Self, String> {
        Ok(Self {
            color: required(v.color, "declared color")?.try_into()?,
            source: required(v.source, "declared color source")?.try_into()?,
        })
    }
}
impl TryFrom<DeclaredAppearance> for rpc::LibraryAppearanceDeclaration {
    type Error = String;
    fn try_from(v: DeclaredAppearance) -> Result<Self, String> {
        Ok(Self {
            color: Some(v.color.try_into()?),
            source: Some(v.source.try_into()?),
        })
    }
}
impl TryFrom<rpc::LibraryFilamentDefinition> for FilamentDefinition {
    type Error = String;
    fn try_from(v: rpc::LibraryFilamentDefinition) -> Result<Self, String> {
        if v.profiles.len() > MAX_PROFILES_PER_FILAMENT {
            return Err("too many filament profiles".into());
        }
        let value = Self {
            name: v.name,
            material: v.material.map(TryInto::try_into).transpose()?,
            color_name: v.color_name,
            appearance: v.appearance.map(TryInto::try_into).transpose()?,
            profiles: v
                .profiles
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            source: required(v.source, "filament source")?.try_into()?,
        };
        value.content_hash()?;
        Ok(value)
    }
}
impl TryFrom<FilamentDefinition> for rpc::LibraryFilamentDefinition {
    type Error = String;
    fn try_from(v: FilamentDefinition) -> Result<Self, String> {
        v.content_hash()?;
        Ok(Self {
            name: v.name,
            material: v.material.map(TryInto::try_into).transpose()?,
            color_name: v.color_name,
            appearance: v.appearance.map(TryInto::try_into).transpose()?,
            profiles: v
                .profiles
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            source: Some(v.source.try_into()?),
        })
    }
}
impl TryFrom<rpc::LibraryFilamentRevision> for FilamentRevision {
    type Error = String;
    fn try_from(v: rpc::LibraryFilamentRevision) -> Result<Self, String> {
        let value = Self {
            reference: required(v.reference, "filament reference")?.try_into()?,
            definition: required(v.definition, "filament definition")?.try_into()?,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<FilamentRevision> for rpc::LibraryFilamentRevision {
    type Error = String;
    fn try_from(v: FilamentRevision) -> Result<Self, String> {
        v.validate()?;
        Ok(Self {
            reference: Some(v.reference.try_into()?),
            definition: Some(v.definition.try_into()?),
        })
    }
}
impl TryFrom<rpc::LibraryOrigin> for LibraryOrigin {
    type Error = String;
    fn try_from(v: rpc::LibraryOrigin) -> Result<Self, String> {
        validate_library_uuid(&v.library_id)?;
        Ok(Self {
            library_id: LibraryId(v.library_id),
            filament: required(v.filament, "origin filament")?.try_into()?,
        })
    }
}
impl TryFrom<LibraryOrigin> for rpc::LibraryOrigin {
    type Error = String;
    fn try_from(v: LibraryOrigin) -> Result<Self, String> {
        validate_library_uuid(&v.library_id.0)?;
        Ok(Self {
            library_id: v.library_id.0,
            filament: Some(v.filament.try_into()?),
        })
    }
}
impl TryFrom<rpc::LibraryFilamentEntry> for FilamentEntry {
    type Error = String;
    fn try_from(v: rpc::LibraryFilamentEntry) -> Result<Self, String> {
        Ok(Self {
            current: required(v.current, "current filament")?.try_into()?,
            archived: v.archived,
            origin: v.origin.map(TryInto::try_into).transpose()?,
        })
    }
}
impl TryFrom<FilamentEntry> for rpc::LibraryFilamentEntry {
    type Error = String;
    fn try_from(v: FilamentEntry) -> Result<Self, String> {
        Ok(Self {
            current: Some(v.current.try_into()?),
            archived: v.archived,
            origin: v.origin.map(TryInto::try_into).transpose()?,
        })
    }
}
impl TryFrom<rpc::LibraryFilamentSummary> for LibraryFilamentSummary {
    type Error = String;
    fn try_from(v: rpc::LibraryFilamentSummary) -> Result<Self, String> {
        let value = Self {
            entry: required(v.entry, "filament entry")?.try_into()?,
            name: v.name,
            material: v.material.map(TryInto::try_into).transpose()?,
            color_name: v.color_name,
            appearance: v.appearance.map(TryInto::try_into).transpose()?,
            profile_count: v.profile_count,
            on_hand_spools: v.on_hand_spools,
            earlier_spools: v.earlier_spools,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<LibraryFilamentSummary> for rpc::LibraryFilamentSummary {
    type Error = String;
    fn try_from(v: LibraryFilamentSummary) -> Result<Self, String> {
        v.validate()?;
        Ok(Self {
            entry: Some(v.entry.try_into()?),
            name: v.name,
            material: v.material.map(TryInto::try_into).transpose()?,
            color_name: v.color_name,
            appearance: v.appearance.map(TryInto::try_into).transpose()?,
            profile_count: v.profile_count,
            on_hand_spools: v.on_hand_spools,
            earlier_spools: v.earlier_spools,
        })
    }
}
impl TryFrom<rpc::UserLibraryStatus> for LibraryStatus {
    type Error = String;
    fn try_from(v: rpc::UserLibraryStatus) -> Result<Self, String> {
        let value = Self {
            location: required(v.location, "library location")?.try_into()?,
            identity: v.identity.map(TryInto::try_into).transpose()?,
            filament_count: v.filament_count,
            spool_count: v.spool_count,
            recipe_count: v.recipe_count,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<LibraryStatus> for rpc::UserLibraryStatus {
    type Error = String;
    fn try_from(v: LibraryStatus) -> Result<Self, String> {
        v.validate()?;
        Ok(Self {
            location: Some(v.location.try_into()?),
            identity: v.identity.map(TryInto::try_into).transpose()?,
            filament_count: v.filament_count,
            spool_count: v.spool_count,
            recipe_count: v.recipe_count,
        })
    }
}
impl TryFrom<rpc::LibraryFilamentPage> for LibraryFilamentPage {
    type Error = String;
    fn try_from(v: rpc::LibraryFilamentPage) -> Result<Self, String> {
        if v.entries.len() > MAX_LIBRARY_PAGE as usize {
            return Err("library page exceeds limit".into());
        }
        let value = Self {
            identity: v.identity.map(TryInto::try_into).transpose()?,
            entries: v
                .entries
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            total_matches: v.total_matches,
            next_offset: v.next_offset,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<LibraryFilamentPage> for rpc::LibraryFilamentPage {
    type Error = String;
    fn try_from(v: LibraryFilamentPage) -> Result<Self, String> {
        v.validate()?;
        Ok(Self {
            identity: v.identity.map(TryInto::try_into).transpose()?,
            entries: v
                .entries
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            total_matches: v.total_matches,
            next_offset: v.next_offset,
        })
    }
}
impl TryFrom<rpc::LibrarySpoolRecord> for SpoolRecord {
    type Error = String;
    fn try_from(v: rpc::LibrarySpoolRecord) -> Result<Self, String> {
        let state = match rpc::LibraryStockState::try_from(v.state) {
            Ok(rpc::LibraryStockState::LibraryStockOnHand) => StockState::OnHand,
            Ok(rpc::LibraryStockState::LibraryStockOrdered) => StockState::Ordered,
            Ok(rpc::LibraryStockState::LibraryStockEmpty) => StockState::Empty,
            Ok(rpc::LibraryStockState::LibraryStockArchived) => StockState::Archived,
            _ => return Err("unknown or unspecified stock state".into()),
        };
        let quantity = v
            .quantity
            .map(|q| -> Result<StockQuantity, String> {
                let source = match rpc::LibraryQuantitySource::try_from(q.source) {
                    Ok(rpc::LibraryQuantitySource::LibraryQuantityEntered) => {
                        QuantitySource::Entered
                    }
                    Ok(rpc::LibraryQuantitySource::LibraryQuantityWeighed) => {
                        QuantitySource::Weighed
                    }
                    _ => return Err("unknown or unspecified quantity source".into()),
                };
                Ok(StockQuantity {
                    remaining_g: q.remaining_g,
                    source,
                })
            })
            .transpose()?;
        let value = Self {
            id: SpoolId(v.id),
            filament: required(v.filament, "spool filament")?.try_into()?,
            name: v.name,
            state,
            quantity,
            lot: v.lot,
            location: v.location,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<SpoolRecord> for rpc::LibrarySpoolRecord {
    type Error = String;
    fn try_from(v: SpoolRecord) -> Result<Self, String> {
        v.validate()?;
        Ok(Self {
            id: v.id.0,
            filament: Some(v.filament.try_into()?),
            name: v.name,
            state: match v.state {
                StockState::OnHand => rpc::LibraryStockState::LibraryStockOnHand,
                StockState::Ordered => rpc::LibraryStockState::LibraryStockOrdered,
                StockState::Empty => rpc::LibraryStockState::LibraryStockEmpty,
                StockState::Archived => rpc::LibraryStockState::LibraryStockArchived,
            } as i32,
            quantity: v.quantity.map(|q| rpc::LibraryStockQuantity {
                remaining_g: q.remaining_g,
                source: match q.source {
                    QuantitySource::Entered => rpc::LibraryQuantitySource::LibraryQuantityEntered,
                    QuantitySource::Weighed => rpc::LibraryQuantitySource::LibraryQuantityWeighed,
                } as i32,
            }),
            lot: v.lot,
            location: v.location,
        })
    }
}
impl TryFrom<rpc::LibrarySpoolPage> for LibrarySpoolPage {
    type Error = String;
    fn try_from(v: rpc::LibrarySpoolPage) -> Result<Self, String> {
        if v.spools.len() > MAX_LIBRARY_PAGE as usize {
            return Err("spool page exceeds limit".into());
        }
        let value = Self {
            identity: v.identity.map(TryInto::try_into).transpose()?,
            spools: v
                .spools
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            total_matches: v.total_matches,
            next_offset: v.next_offset,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<LibrarySpoolPage> for rpc::LibrarySpoolPage {
    type Error = String;
    fn try_from(v: LibrarySpoolPage) -> Result<Self, String> {
        v.validate()?;
        Ok(Self {
            identity: v.identity.map(TryInto::try_into).transpose()?,
            spools: v
                .spools
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            total_matches: v.total_matches,
            next_offset: v.next_offset,
        })
    }
}
impl TryFrom<rpc::LibraryWriteRequest> for LibraryWrite {
    type Error = String;
    fn try_from(v: rpc::LibraryWriteRequest) -> Result<Self, String> {
        use rpc::library_write_request::Mutation as M;
        let mutation = match required(v.mutation, "library mutation")? {
            M::PutFilament(v) => LibraryMutation::PutFilament {
                id: FilamentId(v.id),
                expected: v
                    .expected
                    .map(|s| SourceHash::parse(s).map_err(|e| e.to_string()))
                    .transpose()?,
                definition: required(v.definition, "filament definition")?.try_into()?,
            },
            M::ArchiveFilament(v) => LibraryMutation::ArchiveFilament {
                filament: required(v.filament, "archived filament")?.try_into()?,
                archived: v.archived,
            },
            M::PutSpool(v) => LibraryMutation::PutSpool {
                spool: v.try_into()?,
            },
            M::RemoveSpool(id) => LibraryMutation::RemoveSpool { id: SpoolId(id) },
            M::PutRecipe(v) => LibraryMutation::PutRecipe {
                transfer: required(v.transfer, "recipe transfer")?.try_into()?,
                expected: v
                    .expected
                    .map(|s| SourceHash::parse(s).map_err(|e| e.to_string()))
                    .transpose()?,
            },
            M::RemoveRecipe(v) => {
                let reference: RecipeRef = v.try_into()?;
                LibraryMutation::RemoveRecipe {
                    id: reference.id,
                    expected: reference.revision,
                }
            }
        };
        let value = Self {
            library_id: v.library_id.map(LibraryId),
            request_id: v.request_id,
            base_revision: v.base_revision,
            mutation,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<LibraryWrite> for rpc::LibraryWriteRequest {
    type Error = String;
    fn try_from(v: LibraryWrite) -> Result<Self, String> {
        v.validate()?;
        use rpc::library_write_request::Mutation as M;
        let mutation = match v.mutation {
            LibraryMutation::PutFilament {
                id,
                expected,
                definition,
            } => M::PutFilament(rpc::LibraryPutFilament {
                id: id.0,
                expected: expected.map(|h| h.as_str().into()),
                definition: Some(definition.try_into()?),
            }),
            LibraryMutation::ArchiveFilament { filament, archived } => {
                M::ArchiveFilament(rpc::LibraryArchiveFilament {
                    filament: Some(filament.try_into()?),
                    archived,
                })
            }
            LibraryMutation::PutSpool { spool } => M::PutSpool(spool.try_into()?),
            LibraryMutation::RemoveSpool { id } => M::RemoveSpool(id.0),
            LibraryMutation::PutRecipe { transfer, expected } => {
                M::PutRecipe(rpc::LibraryPutRecipe {
                    transfer: Some(transfer.try_into()?),
                    expected: expected.map(|h| h.as_str().into()),
                })
            }
            LibraryMutation::RemoveRecipe { id, expected } => M::RemoveRecipe(
                RecipeRef {
                    id,
                    revision: expected,
                }
                .try_into()?,
            ),
        };
        Ok(Self {
            library_id: v.library_id.map(|id| id.0),
            request_id: v.request_id,
            base_revision: v.base_revision,
            mutation: Some(mutation),
        })
    }
}
impl TryFrom<rpc::LibraryWriteReceipt> for LibraryWriteReceipt {
    type Error = String;
    fn try_from(v: rpc::LibraryWriteReceipt) -> Result<Self, String> {
        let publication = match rpc::LibraryPublication::try_from(v.publication) {
            Ok(rpc::LibraryPublication::Confirmed) => LibraryPublication::Confirmed,
            Ok(rpc::LibraryPublication::Unconfirmed) => LibraryPublication::Unconfirmed,
            Ok(rpc::LibraryPublication::Unknown) => LibraryPublication::Unknown,
            _ => return Err("unknown or unspecified publication outcome".into()),
        };
        let value = Self {
            identity: required(v.identity, "library receipt identity")?.try_into()?,
            request_id: v.request_id,
            publication,
            replayed: v.replayed,
            message: v.message,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<LibraryWriteReceipt> for rpc::LibraryWriteReceipt {
    type Error = String;
    fn try_from(v: LibraryWriteReceipt) -> Result<Self, String> {
        v.validate()?;
        Ok(Self {
            identity: Some(v.identity.try_into()?),
            request_id: v.request_id,
            publication: match v.publication {
                LibraryPublication::Confirmed => rpc::LibraryPublication::Confirmed,
                LibraryPublication::Unconfirmed => rpc::LibraryPublication::Unconfirmed,
                LibraryPublication::Unknown => rpc::LibraryPublication::Unknown,
            } as i32,
            replayed: v.replayed,
            message: v.message,
        })
    }
}

impl TryFrom<rpc::LibraryListFilamentsRequest> for LibraryFilamentQuery {
    type Error = String;
    fn try_from(v: rpc::LibraryListFilamentsRequest) -> Result<Self, String> {
        let value = Self {
            expected: v.expected.map(TryInto::try_into).transpose()?,
            query: v.query,
            offset: v.offset,
            page_size: v.page_size,
            on_hand_only: v.on_hand_only,
            include_archived: v.include_archived,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<LibraryFilamentQuery> for rpc::LibraryListFilamentsRequest {
    type Error = String;
    fn try_from(v: LibraryFilamentQuery) -> Result<Self, String> {
        v.validate()?;
        Ok(Self {
            expected: v.expected.map(TryInto::try_into).transpose()?,
            query: v.query,
            offset: v.offset,
            page_size: v.page_size,
            on_hand_only: v.on_hand_only,
            include_archived: v.include_archived,
        })
    }
}
impl TryFrom<rpc::LibraryListSpoolsRequest> for LibrarySpoolQuery {
    type Error = String;
    fn try_from(v: rpc::LibraryListSpoolsRequest) -> Result<Self, String> {
        let value = Self {
            expected: v.expected.map(TryInto::try_into).transpose()?,
            filament: v.filament.map(TryInto::try_into).transpose()?,
            offset: v.offset,
            page_size: v.page_size,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<LibrarySpoolQuery> for rpc::LibraryListSpoolsRequest {
    type Error = String;
    fn try_from(v: LibrarySpoolQuery) -> Result<Self, String> {
        v.validate()?;
        Ok(Self {
            expected: v.expected.map(TryInto::try_into).transpose()?,
            filament: v.filament.map(TryInto::try_into).transpose()?,
            offset: v.offset,
            page_size: v.page_size,
        })
    }
}

impl TryFrom<rpc::MaterialCollectionRecord> for MaterialCollectionRecord {
    type Error = String;
    fn try_from(v: rpc::MaterialCollectionRecord) -> Result<Self, String> {
        let value = Self {
            hash: SourceHash::parse(v.hash).map_err(|e| e.to_string())?,
            byte_count: v.byte_count,
            filament_count: v.filament_count,
            recipe_count: v.recipe_count,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<MaterialCollectionRecord> for rpc::MaterialCollectionRecord {
    type Error = String;
    fn try_from(v: MaterialCollectionRecord) -> Result<Self, String> {
        v.validate()?;
        Ok(Self {
            hash: v.hash.as_str().into(),
            byte_count: v.byte_count,
            filament_count: v.filament_count,
            recipe_count: v.recipe_count,
        })
    }
}
