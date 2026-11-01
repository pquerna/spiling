// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Direct lookup only. Values never enter library/project records or diagnostic errors.
use spiling_contracts::hardware::{CredentialBackend, CredentialRef, CredentialRole};
#[cfg(unix)]
use std::fs::File;
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CredentialError {
    #[error("credential entry is missing")]
    Missing,
    #[error("system credential store is locked or access was denied")]
    Access,
    #[error("system credential store is unavailable")]
    Unavailable,
    #[error("credential storage operation failed")]
    Storage,
    #[error("invalid credential reference, payload or private file permissions")]
    Invalid,
}
fn native_error(e: keyring::Error) -> CredentialError {
    match e {
        keyring::Error::NoEntry => CredentialError::Missing,
        keyring::Error::NoStorageAccess(_) => CredentialError::Access,
        keyring::Error::NoDefaultStore | keyring::Error::NotSupportedByStore(_) => {
            CredentialError::Unavailable
        }
        _ => CredentialError::Storage,
    }
}
/// The root is independent of the chosen/movable personal .spil file.
#[derive(Debug, Clone)]
pub struct CredentialStore {
    pub(super) root: PathBuf,
}
impl CredentialStore {
    pub fn user() -> Result<Self, CredentialError> {
        let root = directories::ProjectDirs::from("org", "Spiling", "Spiling")
            .ok_or(CredentialError::Unavailable)?
            .data_local_dir()
            .join("credentials")
            .join("hardware");
        Ok(Self { root })
    }
    pub fn at(root: PathBuf) -> Result<Self, CredentialError> {
        if !root.is_absolute() {
            return Err(CredentialError::Invalid);
        }
        Ok(Self { root })
    }
    fn entry(reference: &CredentialRef) -> Result<keyring::Entry, CredentialError> {
        let account = reference.account().map_err(|_| CredentialError::Invalid)?;
        keyring::Entry::new(CredentialRef::SERVICE, &account).map_err(native_error)
    }
    /// A new slot may choose the file backend when native initialization is unavailable.
    /// Existing slots and staged rotations never change the recorded backend.
    pub fn remember(
        &self,
        role: CredentialRole,
        secret: Zeroizing<String>,
    ) -> Result<CredentialRef, CredentialError> {
        validate_secret(&secret)?;
        let backend = match keyring::Entry::store_status() {
            Ok(()) => CredentialBackend::Keyring,
            Err(keyring::Error::NoStorageAccess(_)) => return Err(CredentialError::Access),
            Err(_) => CredentialBackend::PlaintextFile,
        };
        let reference = CredentialRef {
            slot_id: uuid::Uuid::new_v4().to_string(),
            role,
            backend,
        };
        self.set(&reference, &secret)?;
        Ok(reference)
    }
    /// Write a fresh slot on the same backend; the saved route still owns the old slot
    /// until its exact reference publication is confirmed.
    pub fn stage_rotation(
        &self,
        previous: &CredentialRef,
        secret: Zeroizing<String>,
    ) -> Result<CredentialRef, CredentialError> {
        previous.account().map_err(|_| CredentialError::Invalid)?;
        let reference = CredentialRef {
            slot_id: uuid::Uuid::new_v4().to_string(),
            ..previous.clone()
        };
        self.set(&reference, &secret)?;
        Ok(reference)
    }
    pub fn set(&self, reference: &CredentialRef, secret: &str) -> Result<(), CredentialError> {
        validate_secret(secret)?;
        reference.account().map_err(|_| CredentialError::Invalid)?;
        match reference.backend {
            CredentialBackend::Keyring => Self::entry(reference)?
                .set_password(secret)
                .map_err(native_error),
            CredentialBackend::PlaintextFile => {
                private_dir(&self.root)?;
                let dir = self.root.join(&reference.slot_id);
                private_dir(&dir)?;
                let destination = dir.join(format!("{}.secret", reference.role.token()));
                if let Ok(meta) = fs::symlink_metadata(&destination) {
                    private_file(&meta)?;
                }
                let mut temp =
                    tempfile::NamedTempFile::new_in(&dir).map_err(|_| CredentialError::Storage)?;
                temp.write_all(secret.as_bytes())
                    .and_then(|_| temp.as_file().sync_all())
                    .map_err(|_| CredentialError::Storage)?;
                temp.persist(&destination)
                    .map_err(|_| CredentialError::Storage)?;
                #[cfg(unix)]
                File::open(&dir)
                    .and_then(|f| f.sync_all())
                    .map_err(|_| CredentialError::Storage)?;
                Ok(())
            }
        }
    }
    pub fn get(&self, reference: &CredentialRef) -> Result<Zeroizing<String>, CredentialError> {
        reference.account().map_err(|_| CredentialError::Invalid)?;
        match reference.backend {
            CredentialBackend::Keyring => Self::entry(reference)?
                .get_password()
                .map(Zeroizing::new)
                .map_err(native_error),
            CredentialBackend::PlaintextFile => {
                check_dir(&self.root)?;
                let dir = self.root.join(&reference.slot_id);
                check_dir(&dir)?;
                let mut options = OpenOptions::new();
                options.read(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
                }
                let file = options
                    .open(dir.join(format!("{}.secret", reference.role.token())))
                    .map_err(io_error)?;
                private_file(&file.metadata().map_err(|_| CredentialError::Storage)?)?;
                let mut value = Zeroizing::new(String::new());
                file.take(8193)
                    .read_to_string(&mut value)
                    .map_err(|_| CredentialError::Invalid)?;
                validate_secret(&value)?;
                Ok(value)
            }
        }
    }
    pub fn delete(&self, reference: &CredentialRef) -> Result<(), CredentialError> {
        reference.account().map_err(|_| CredentialError::Invalid)?;
        match reference.backend {
            CredentialBackend::Keyring => Self::entry(reference)?
                .delete_credential()
                .map_err(native_error),
            CredentialBackend::PlaintextFile => {
                check_dir(&self.root)?;
                let dir = self.root.join(&reference.slot_id);
                check_dir(&dir)?;
                let path = dir.join(format!("{}.secret", reference.role.token()));
                private_file(&fs::symlink_metadata(&path).map_err(io_error)?)?;
                fs::remove_file(path).map_err(io_error)?;
                #[cfg(unix)]
                File::open(&dir)
                    .and_then(|f| f.sync_all())
                    .map_err(|_| CredentialError::Storage)?;
                Ok(())
            }
        }
    }
}
fn validate_secret(v: &str) -> Result<(), CredentialError> {
    if v.is_empty() || v.len() > 8192 || v.contains('\0') {
        Err(CredentialError::Invalid)
    } else {
        Ok(())
    }
}
pub(super) fn io_error(e: std::io::Error) -> CredentialError {
    if e.kind() == std::io::ErrorKind::NotFound {
        CredentialError::Missing
    } else {
        CredentialError::Storage
    }
}
pub(super) fn private_file(meta: &fs::Metadata) -> Result<(), CredentialError> {
    if !meta.is_file() || meta.file_type().is_symlink() {
        return Err(CredentialError::Invalid);
    }
    check_mode(meta)
}
fn check_mode(meta: &fs::Metadata) -> Result<(), CredentialError> {
    #[cfg(not(unix))]
    let _ = meta;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if meta.permissions().mode() & 0o077 != 0 {
            return Err(CredentialError::Invalid);
        }
    }
    Ok(())
}
pub(super) fn check_dir(path: &Path) -> Result<(), CredentialError> {
    let meta = fs::symlink_metadata(path).map_err(io_error)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(CredentialError::Invalid);
    }
    check_mode(&meta)
}
pub(super) fn private_dir(path: &Path) -> Result<(), CredentialError> {
    if !path.exists() {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|_| CredentialError::Storage)?;
        }
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => return Err(CredentialError::Storage),
        }
    }
    check_dir(path)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn direct_file_lifecycle_and_wrong_backend_do_not_fall_through() {
        let root = tempfile::tempdir().unwrap();
        let store = CredentialStore::at(root.path().join("credentials")).unwrap();
        let reference = CredentialRef {
            slot_id: uuid::Uuid::new_v4().to_string(),
            role: CredentialRole::ApiKey,
            backend: CredentialBackend::PlaintextFile,
        };
        store.set(&reference, "sentinel-key-1").unwrap();
        assert_eq!(store.get(&reference).unwrap().as_str(), "sentinel-key-1");
        let staged = store
            .stage_rotation(&reference, Zeroizing::new("new-slot-sentinel".into()))
            .unwrap();
        assert_eq!(staged.backend, reference.backend);
        assert_ne!(staged.slot_id, reference.slot_id);
        assert_eq!(store.get(&reference).unwrap().as_str(), "sentinel-key-1");
        assert_eq!(store.get(&staged).unwrap().as_str(), "new-slot-sentinel");
        store.delete(&staged).unwrap();
        store.set(&reference, "sentinel-key-2").unwrap();
        assert_eq!(store.get(&reference).unwrap().as_str(), "sentinel-key-2");
        let missing = CredentialRef {
            slot_id: uuid::Uuid::new_v4().to_string(),
            ..reference.clone()
        };
        assert_eq!(store.get(&missing).unwrap_err(), CredentialError::Missing);
        store.delete(&reference).unwrap();
        assert_eq!(store.get(&reference).unwrap_err(), CredentialError::Missing);
    }
    #[cfg(unix)]
    #[test]
    fn file_store_refuses_symlink_and_world_readable_secrets() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = tempfile::tempdir().unwrap();
        let store = CredentialStore::at(root.path().join("private")).unwrap();
        let r = CredentialRef {
            slot_id: uuid::Uuid::new_v4().to_string(),
            role: CredentialRole::Password,
            backend: CredentialBackend::PlaintextFile,
        };
        store.set(&r, "example").unwrap();
        let file = store.root.join(&r.slot_id).join("password.secret");
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(store.get(&r).unwrap_err(), CredentialError::Invalid);
        fs::remove_file(&file).unwrap();
        symlink(root.path().join("other"), &file).unwrap();
        assert_eq!(store.set(&r, "new"), Err(CredentialError::Invalid));
    }
}

/// A transient form value. Debug output is deliberately redacted.
#[derive(Clone)]
pub struct CredentialInput(Zeroizing<String>);
impl std::fmt::Debug for CredentialInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CredentialInput([redacted])")
    }
}
impl CredentialInput {
    pub fn new(value: String) -> Self {
        Self(Zeroizing::new(value))
    }
    pub fn into_secret(self) -> Zeroizing<String> {
        self.0
    }
}
