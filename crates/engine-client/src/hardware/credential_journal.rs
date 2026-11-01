// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Durable local credential-reference publication. No secret values or automatic replay.
use super::credentials::{CredentialError, CredentialStore, io_error, private_dir, private_file};
use serde::{Deserialize, Serialize};
use spiling_contracts::{geometry::NativePath, hardware::CredentialRef, library::*};
#[cfg(unix)]
use std::fs::File;
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::Path,
};
const MAX_PENDING: usize = 32;
const MAX_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PendingCredential {
    pub location: NativePath,
    pub request: LibraryWrite,
    pub delete_after: Option<CredentialRef>,
    pub created: Option<CredentialRef>,
}
impl PendingCredential {
    fn validate(&self) -> Result<(), CredentialError> {
        self.location
            .validate()
            .map_err(|_| CredentialError::Invalid)?;
        self.request
            .validate()
            .map_err(|_| CredentialError::Invalid)?;
        if self.request.library_id.is_none()
            || !matches!(
                self.request.mutation,
                LibraryMutation::ReplaceHardware { .. }
            )
        {
            return Err(CredentialError::Invalid);
        }
        for r in [&self.delete_after, &self.created].into_iter().flatten() {
            r.account().map_err(|_| CredentialError::Invalid)?;
        }
        Ok(())
    }
}
impl CredentialStore {
    /// Persist before sending the request. Existing request IDs must retain exact operands.
    pub fn save_pending(&self, pending: &PendingCredential) -> Result<(), CredentialError> {
        pending.validate()?;
        let bytes = serde_json::to_vec(pending).map_err(|_| CredentialError::Invalid)?;
        if bytes.len() > MAX_BYTES {
            return Err(CredentialError::Invalid);
        }
        private_dir(&self.root)?;
        let dir = self.root.join("pending");
        private_dir(&dir)?;
        let path = dir.join(format!("{}.json", pending.request.request_id));
        if path.try_exists().map_err(io_error)? {
            return if read(&path)? == *pending {
                Ok(())
            } else {
                Err(CredentialError::Invalid)
            };
        }
        if fs::read_dir(&dir)
            .map_err(io_error)?
            .take(MAX_PENDING + 1)
            .count()
            >= MAX_PENDING
        {
            return Err(CredentialError::Storage);
        }
        let mut temp = tempfile::NamedTempFile::new_in(&dir).map_err(io_error)?;
        temp.write_all(&bytes)
            .and_then(|_| temp.as_file().sync_all())
            .map_err(io_error)?;
        match temp.persist_noclobber(&path) {
            Ok(_) => sync(&dir),
            Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {
                if read(&path)? == *pending {
                    Ok(())
                } else {
                    Err(CredentialError::Invalid)
                }
            }
            Err(_) => Err(CredentialError::Storage),
        }
    }
    /// Recover exact pending operands for this attachment; observation never resubmits.
    pub fn pending(
        &self,
        location: &NativePath,
        library: &LibraryId,
    ) -> Result<Option<PendingCredential>, CredentialError> {
        if !self.root.try_exists().map_err(io_error)? {
            return Ok(None);
        }
        super::credentials::check_dir(&self.root)?;
        let dir = self.root.join("pending");
        if !dir.try_exists().map_err(io_error)? {
            return Ok(None);
        }
        super::credentials::check_dir(&dir)?;
        let mut paths = Vec::new();
        for (index, entry) in fs::read_dir(&dir)
            .map_err(io_error)?
            .take(MAX_PENDING + 1)
            .enumerate()
        {
            if index == MAX_PENDING {
                return Err(CredentialError::Storage);
            }
            let path = entry.map_err(io_error)?.path();
            if path
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|s| s.starts_with(".tmp"))
            {
                continue;
            }
            paths.push(path);
        }
        if paths.len() > MAX_PENDING {
            return Err(CredentialError::Storage);
        }
        paths.sort();
        for path in paths {
            let record = read(&path)?;
            if record.location == *location && record.request.library_id.as_ref() == Some(library) {
                return Ok(Some(record));
            }
        }
        Ok(None)
    }
    pub fn remove_pending(&self, pending: &PendingCredential) -> Result<(), CredentialError> {
        pending.validate()?;
        let dir = self.root.join("pending");
        super::credentials::check_dir(&self.root)?;
        super::credentials::check_dir(&dir)?;
        let path = dir.join(format!("{}.json", pending.request.request_id));
        if !path.try_exists().map_err(io_error)? {
            return Ok(());
        }
        if read(&path)? != *pending {
            return Err(CredentialError::Invalid);
        }
        fs::remove_file(&path).map_err(io_error)?;
        sync(&dir)
    }
}
fn sync(dir: &Path) -> Result<(), CredentialError> {
    #[cfg(unix)]
    File::open(dir)
        .and_then(|f| f.sync_all())
        .map_err(io_error)?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}
fn read(path: &Path) -> Result<PendingCredential, CredentialError> {
    private_file(&fs::symlink_metadata(path).map_err(io_error)?)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    let file = options.open(path).map_err(io_error)?;
    private_file(&file.metadata().map_err(io_error)?)?;
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() > MAX_BYTES {
        return Err(CredentialError::Invalid);
    }
    let record: PendingCredential =
        serde_json::from_slice(&bytes).map_err(|_| CredentialError::Invalid)?;
    record.validate()?;
    if path.file_name().and_then(|s| s.to_str())
        != Some(format!("{}.json", record.request.request_id).as_str())
    {
        return Err(CredentialError::Invalid);
    }
    Ok(record)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn restart_recovers_exact_original_request_without_secret_or_automatic_replay() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("credentials");
        let store = CredentialStore::at(path.clone()).unwrap();
        let library = LibraryId(uuid::Uuid::new_v4().to_string());
        let location =
            NativePath::from_os_str(root.path().join("My library.spil").as_os_str()).unwrap();
        let record = PendingCredential {
            location: location.clone(),
            request: LibraryWrite {
                library_id: Some(library.clone()),
                base_revision: 2,
                request_id: uuid::Uuid::new_v4().to_string(),
                mutation: LibraryMutation::ReplaceHardware {
                    hardware: Default::default(),
                },
            },
            delete_after: None,
            created: None,
        };
        store.save_pending(&record).unwrap();
        store.save_pending(&record).unwrap();
        let mut altered = record.clone();
        altered.request.base_revision += 1;
        assert_eq!(
            store.save_pending(&altered).unwrap_err(),
            CredentialError::Invalid
        );
        drop(store);
        let restarted = CredentialStore::at(path).unwrap();
        assert_eq!(
            restarted.pending(&location, &library).unwrap(),
            Some(record.clone())
        );
        let other = NativePath::from_os_str(root.path().join("another.spil").as_os_str()).unwrap();
        assert!(restarted.pending(&other, &library).unwrap().is_none());
        restarted.remove_pending(&record).unwrap();
        restarted.remove_pending(&record).unwrap();
        assert!(restarted.pending(&location, &library).unwrap().is_none());
    }
}
