// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Bounded per-user services; project authoring remains in the native project service.
use prost::Message;
use spiling_contracts::{
    geometry::{JobError, NativePath},
    library::*,
    native, rpc,
};
use spiling_core::library::LibraryStore;
use std::{
    path::PathBuf,
    sync::{
        Arc, RwLock,
        atomic::{AtomicBool, Ordering},
    },
};
use tokio::sync::Semaphore;
use tonic::{Request, Response, Status};

#[derive(Clone)]
pub(crate) struct LibraryService {
    location: Arc<RwLock<Option<PathBuf>>>,
    default_location: Option<PathBuf>,
    work: Arc<Semaphore>,
    catalog: Arc<std::sync::OnceLock<Result<crate::catalog::CatalogIndex, String>>>,
}
impl LibraryService {
    pub fn new(configured: Option<PathBuf>) -> Self {
        let default_location = directories::ProjectDirs::from("org", "Spiling", "Spiling")
            .map(|dirs| dirs.data_local_dir().join("My library.spil"));
        Self {
            location: Arc::new(RwLock::new(configured.or_else(|| default_location.clone()))),
            default_location,
            work: Arc::new(Semaphore::new(1)),
            catalog: Arc::new(std::sync::OnceLock::new()),
        }
    }
    pub(crate) fn location(&self) -> Arc<RwLock<Option<PathBuf>>> {
        self.location.clone()
    }
    fn store(&self) -> Result<LibraryStore, Status> {
        let path = self
            .location
            .read()
            .map_err(|_| Status::internal("library location lock failed"))?
            .clone()
            .ok_or_else(|| {
                failure(LibraryError::new(
                    LibraryErrorCode::Io,
                    "Cannot locate the per-user data directory; choose a library location",
                ))
            })?;
        Ok(LibraryStore::new(path))
    }
    async fn run<T: Send + 'static>(
        &self,
        task: impl FnOnce(&AtomicBool) -> Result<T, LibraryError> + Send + 'static,
    ) -> Result<T, Status> {
        let permit = self.work.clone().try_acquire_owned().map_err(|_| {
            failure(LibraryError::new(
                LibraryErrorCode::Locked,
                "Another library request is active; keep the draft and refresh",
            ))
        })?;
        struct Cancel(Arc<AtomicBool>);
        impl Drop for Cancel {
            fn drop(&mut self) {
                self.0.store(true, Ordering::Release);
            }
        }
        let cancellation = Cancel(Arc::new(AtomicBool::new(false)));
        let cancel = cancellation.0.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            task(&cancel)
        })
        .await
        .map_err(|_| Status::internal("library worker failed"))?
        .map_err(failure)
    }
}
fn failure(error: LibraryError) -> Status {
    let status = native::domain_status(&JobError::Library { error });
    Status::with_details(
        tonic::Code::from_i32(status.code),
        status.message.clone(),
        status.encode_to_vec().into(),
    )
}
fn bad(message: String) -> Status {
    failure(LibraryError::new(LibraryErrorCode::InvalidData, message))
}
fn reply<T, W>(value: T) -> Result<Response<W>, Status>
where
    W: TryFrom<T, Error = String> + Message,
{
    let value = W::try_from(value).map_err(Status::internal)?;
    if value.encoded_len() > spiling_contracts::MAX_CONTROL_BYTES as usize {
        return Err(Status::resource_exhausted(
            "library reply exceeds control budget",
        ));
    }
    Ok(Response::new(value))
}
#[tonic::async_trait]
impl rpc::libraries_server::Libraries for LibraryService {
    async fn get_catalog_record(
        &self,
        request: Request<rpc::CatalogKey>,
    ) -> Result<Response<rpc::CatalogRecord>, Status> {
        let key: spiling_contracts::catalog::CatalogKey =
            request.into_inner().try_into().map_err(bad)?;
        let index = self.catalog.clone();
        reply(
            self.run(move |_| {
                let index = index
                    .get_or_init(crate::catalog::CatalogIndex::bundled)
                    .as_ref()
                    .map_err(|e| LibraryError::new(LibraryErrorCode::InvalidData, e.clone()))?;
                index
                    .get(&key)
                    .map_err(|e| LibraryError::new(LibraryErrorCode::NotFound, e))
            })
            .await?,
        )
    }
    async fn search_catalog(
        &self,
        request: Request<rpc::CatalogQuery>,
    ) -> Result<Response<rpc::CatalogPage>, Status> {
        let query: spiling_contracts::catalog::CatalogQuery =
            request.into_inner().try_into().map_err(bad)?;
        let index = self.catalog.clone();
        reply(
            self.run(move |cancel| {
                let index = index
                    .get_or_init(crate::catalog::CatalogIndex::bundled)
                    .as_ref()
                    .map_err(|e| LibraryError::new(LibraryErrorCode::InvalidData, e.clone()))?;
                index
                    .search(&query, cancel)
                    .map_err(|e| LibraryError::new(LibraryErrorCode::InvalidData, e))
            })
            .await?,
        )
    }

    async fn get_hardware_catalog(
        &self,
        _: Request<()>,
    ) -> Result<Response<rpc::HardwareCatalog>, Status> {
        reply(spiling_contracts::hardware::builtin_catalog().map_err(Status::internal)?)
    }
    async fn get_hardware(&self, _: Request<()>) -> Result<Response<rpc::LibraryHardware>, Status> {
        let store = self.store()?;
        reply(self.run(move |cancel| store.hardware(cancel)).await?)
    }
    async fn get_status(&self, _: Request<()>) -> Result<Response<rpc::UserLibraryStatus>, Status> {
        let store = self.store()?;
        reply(self.run(move |cancel| store.status(cancel)).await?)
    }
    async fn set_location(
        &self,
        request: Request<rpc::LibraryLocationRequest>,
    ) -> Result<Response<rpc::UserLibraryStatus>, Status> {
        let path: NativePath = native::required(request.into_inner().location, "library location")
            .and_then(TryInto::try_into)
            .map_err(bad)?;
        let path = PathBuf::from(path.to_os_string().map_err(|e| bad(e.to_string()))?);
        if !path.is_absolute() {
            return Err(bad("library location must be absolute".into()));
        }
        let location = self.location.clone();
        reply(
            self.run(move |cancel| {
                let store = LibraryStore::new(path.clone());
                let status = store.status(cancel)?;
                *location.write().map_err(|_| {
                    LibraryError::new(LibraryErrorCode::Io, "library location lock failed")
                })? = Some(path);
                Ok(status)
            })
            .await?,
        )
    }
    async fn list_filaments(
        &self,
        request: Request<rpc::LibraryListFilamentsRequest>,
    ) -> Result<Response<rpc::LibraryFilamentPage>, Status> {
        let query: LibraryFilamentQuery = request.into_inner().try_into().map_err(bad)?;
        let store = self.store()?;
        reply(
            self.run(move |cancel| store.list_filaments(&query, cancel))
                .await?,
        )
    }
    async fn get_filament(
        &self,
        request: Request<rpc::LibraryGetFilamentRequest>,
    ) -> Result<Response<rpc::LibraryFilamentRevision>, Status> {
        let request = request.into_inner();
        validate_library_uuid(&request.library_id).map_err(bad)?;
        let reference: FilamentRef = native::required(request.reference, "filament reference")
            .and_then(TryInto::try_into)
            .map_err(bad)?;
        let library = LibraryId(request.library_id);
        let store = self.store()?;
        reply(
            self.run(move |cancel| store.get_filament(&library, &reference, cancel))
                .await?,
        )
    }
    async fn list_spools(
        &self,
        request: Request<rpc::LibraryListSpoolsRequest>,
    ) -> Result<Response<rpc::LibrarySpoolPage>, Status> {
        let query: LibrarySpoolQuery = request.into_inner().try_into().map_err(bad)?;
        let store = self.store()?;
        reply(
            self.run(move |cancel| store.list_spools(&query, cancel))
                .await?,
        )
    }
    async fn list_recipes(
        &self,
        request: Request<rpc::LibraryListRecipesRequest>,
    ) -> Result<Response<rpc::LibraryRecipePage>, Status> {
        let query: LibraryRecipeQuery = request.into_inner().try_into().map_err(bad)?;
        let store = self.store()?;
        reply(
            self.run(move |cancel| store.list_recipes(&query, cancel))
                .await?,
        )
    }
    async fn get_recipe(
        &self,
        request: Request<rpc::LibraryGetRecipeRequest>,
    ) -> Result<Response<rpc::LibraryRecipeTransfer>, Status> {
        let request = request.into_inner();
        validate_library_uuid(&request.library_id).map_err(bad)?;
        let reference: RecipeRef = native::required(request.reference, "recipe reference")
            .and_then(TryInto::try_into)
            .map_err(bad)?;
        let library = LibraryId(request.library_id);
        let store = self.store()?;
        reply(
            self.run(move |cancel| store.recipe(&library, &reference, cancel))
                .await?,
        )
    }
    async fn write(
        &self,
        request: Request<rpc::LibraryWriteRequest>,
    ) -> Result<Response<rpc::LibraryWriteReceipt>, Status> {
        let request: LibraryWrite = request.into_inner().try_into().map_err(bad)?;
        let store = self.store()?;
        let allow_create_parent = self.default_location.as_deref() == Some(store.path());
        reply(
            self.run(move |cancel| {
                if allow_create_parent {
                    let parent = store.path().parent().ok_or_else(|| {
                        LibraryError::new(LibraryErrorCode::Io, "library location has no parent")
                    })?;
                    std::fs::create_dir_all(parent)
                        .map_err(|e| LibraryError::new(LibraryErrorCode::Io, e.to_string()))?;
                }
                let commit = store.write(&request, cancel)?;
                Ok(commit.receipt(request.request_id))
            })
            .await?,
        )
    }
}
