// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Exclusive output transaction. Cleanup is handle-relative, never recursive.
use std::{
    fs::File,
    io::{self, Write},
    path::{Component, Path},
};

#[derive(Debug, thiserror::Error)]
pub enum ExportError {
    #[error("export inspection: {0}")]
    InvalidInspection(String),
    #[error("export I/O: {0}")]
    Io(#[from] io::Error),
    #[error("export JSON: {0}")]
    Json(#[from] serde_json::Error),
}

pub struct OutputDirectory {
    owned: platform::OwnedDirectory,
    committed: bool,
}
impl OutputDirectory {
    pub fn create(path: &Path) -> io::Result<Self> {
        Ok(Self {
            owned: platform::OwnedDirectory::create(path)?,
            committed: false,
        })
    }
    pub fn write(&mut self, name: &str, bytes: &[u8]) -> io::Result<()> {
        let mut components = Path::new(name).components();
        if !matches!(components.next(), Some(Component::Normal(_))) || components.next().is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "output filename must be a single normal component",
            ));
        }
        self.owned.write(name, bytes)
    }
    /// Export only a client-validated bundle after fresh engine-side replay.
    pub fn write_manufacturing_bundle(
        &mut self,
        bundle: &spiling_contracts::manufacturing::ManufacturingBundle,
        verification: &spiling_contracts::manufacturing::VerificationReport,
        inspection: &spiling_contracts::manufacturing::ManufacturingInspectionProvenance,
    ) -> Result<(), ExportError> {
        inspection
            .validate()
            .map_err(|e| ExportError::InvalidInspection(e.to_string()))?;
        let mut expected =
            spiling_contracts::manufacturing::ManufacturingInspectionProvenance::from(
                &bundle.provenance,
            );
        expected.surface_color = inspection.surface_color.clone();
        if &expected != inspection || !verification.verified {
            return Err(ExportError::InvalidInspection(
                "fresh inspection does not bind captured bundle".into(),
            ));
        }
        #[derive(serde::Serialize)]
        struct ExportProvenance<'a> {
            #[serde(flatten)]
            captured: &'a spiling_contracts::manufacturing::ManufacturingProvenance,
            inspection: &'a spiling_contracts::manufacturing::ManufacturingInspectionProvenance,
        }
        self.write("plan.json", &serde_json::to_vec_pretty(&bundle.plan)?)?;
        self.write("program.gcode", bundle.program.as_bytes())?;
        self.write(
            "verification.json",
            &serde_json::to_vec_pretty(verification)?,
        )?;
        self.write(
            "provenance.json",
            &serde_json::to_vec_pretty(&ExportProvenance {
                captured: &bundle.provenance,
                inspection,
            })?,
        )?;
        Ok(())
    }
    /// Call only after validated output and its completion evidence are written.
    pub fn commit(&mut self) {
        self.committed = true;
    }
}
impl Drop for OutputDirectory {
    fn drop(&mut self) {
        if !self.committed {
            self.owned.cleanup();
        }
    }
}

/// A previously nonexistent file, retained by handle until the caller commits.
/// Rollback never removes a replacement inode or follows a substituted symlink.
pub struct OutputFile {
    owned: platform::OwnedFile,
    committed: bool,
}
impl OutputFile {
    pub fn create(path: &Path) -> io::Result<Self> {
        Ok(Self {
            owned: platform::OwnedFile::create(path)?,
            committed: false,
        })
    }
    pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.owned.write(bytes)
    }
    pub fn commit(&mut self) {
        self.committed = true;
    }
}
impl Drop for OutputFile {
    fn drop(&mut self) {
        if !self.committed {
            self.owned.cleanup();
        }
    }
}

#[cfg(unix)]
mod platform {
    use super::*;
    use rustix::fs::{AtFlags, Mode, OFlags, fstat, mkdirat, open, openat, statat, unlinkat};
    use std::ffi::OsString;

    struct CreatedFile {
        name: String,
        handle: File,
    }
    pub struct OwnedDirectory {
        parent: File,
        name: OsString,
        directory: File,
        files: Vec<CreatedFile>,
    }
    fn same(a: &rustix::fs::Stat, b: &rustix::fs::Stat) -> bool {
        a.st_dev == b.st_dev && a.st_ino == b.st_ino
    }
    pub struct OwnedFile {
        parent: File,
        name: OsString,
        handle: File,
    }
    impl OwnedFile {
        pub fn create(path: &Path) -> io::Result<Self> {
            let name = path
                .file_name()
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "output requires a new file name",
                    )
                })?
                .to_owned();
            let parent = File::from(open(
                path.parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
            )?);
            let handle = File::from(openat(
                &parent,
                &name,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_bits_truncate(0o600),
            )?);
            Ok(Self {
                parent,
                name,
                handle,
            })
        }
        pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
            let current = statat(&self.parent, &self.name, AtFlags::SYMLINK_NOFOLLOW)?;
            if !same(&current, &fstat(&self.handle)?) {
                return Err(io::Error::other("output file was replaced or moved"));
            }
            self.handle.write_all(bytes)?;
            self.handle.flush()
        }
        pub fn cleanup(&mut self) {
            if let (Ok(current), Ok(owned)) = (
                statat(&self.parent, &self.name, AtFlags::SYMLINK_NOFOLLOW),
                fstat(&self.handle),
            ) && same(&current, &owned)
            {
                let _ = unlinkat(&self.parent, &self.name, AtFlags::empty());
            }
        }
    }
    impl OwnedDirectory {
        pub fn create(path: &Path) -> io::Result<Self> {
            let name = path
                .file_name()
                .ok_or_else(|| {
                    io::Error::new(
                        io::ErrorKind::InvalidInput,
                        "output requires a new directory name",
                    )
                })?
                .to_owned();
            let parent_path = path
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
                .unwrap_or(Path::new("."));
            let parent = File::from(open(
                parent_path,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
                Mode::empty(),
            )?);
            mkdirat(&parent, &name, Mode::from_bits_truncate(0o700))?;
            // NOFOLLOW excludes symlink substitution before the new directory is opened.
            let directory = match openat(
                &parent,
                &name,
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
            ) {
                Ok(fd) => File::from(fd),
                Err(error) => {
                    // Do not touch an unowned substitute if opening the created directory fails.
                    return Err(error.into());
                }
            };
            Ok(Self {
                parent,
                name,
                directory,
                files: Vec::new(),
            })
        }
        pub fn write(&mut self, name: &str, bytes: &[u8]) -> io::Result<()> {
            let current = statat(&self.parent, &self.name, AtFlags::SYMLINK_NOFOLLOW)?;
            if !same(&current, &fstat(&self.directory)?) {
                return Err(io::Error::other("output directory was replaced or moved"));
            }
            let file = File::from(openat(
                &self.directory,
                name,
                OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::from_bits_truncate(0o600),
            )?);
            // Own the inode before the first write, including partial-write failures.
            self.files.push(CreatedFile {
                name: name.to_owned(),
                handle: file,
            });
            let file = &mut self.files.last_mut().unwrap().handle;
            file.write_all(bytes)?;
            file.flush()
        }
        pub fn cleanup(&mut self) {
            for file in self.files.drain(..).rev() {
                if let (Ok(current), Ok(owned)) = (
                    statat(
                        &self.directory,
                        file.name.as_str(),
                        AtFlags::SYMLINK_NOFOLLOW,
                    ),
                    fstat(&file.handle),
                ) && same(&current, &owned)
                {
                    let _ = unlinkat(&self.directory, file.name.as_str(), AtFlags::empty());
                }
            }
            if let (Ok(current), Ok(owned)) = (
                statat(&self.parent, &self.name, AtFlags::SYMLINK_NOFOLLOW),
                fstat(&self.directory),
            ) && same(&current, &owned)
            {
                let _ = unlinkat(&self.parent, &self.name, AtFlags::REMOVEDIR);
            }
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::{
        fs::OpenOptions,
        os::windows::{fs::OpenOptionsExt, io::AsRawHandle},
        path::PathBuf,
    };
    use windows_sys::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{
            DELETE, FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS,
            FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE, FileDispositionInfo,
            SetFileInformationByHandle,
        },
    };

    pub struct OwnedFile {
        handle: File,
        _ancestors: Vec<File>,
    }
    impl OwnedFile {
        pub fn create(path: &Path) -> io::Result<Self> {
            let name = path.file_name().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "output requires a new file name",
                )
            })?;
            let parent = path
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new("."))
                .canonicalize()?;
            let ancestors = parent
                .ancestors()
                .map(|p| hold_directory(p, false))
                .collect::<io::Result<Vec<_>>>()?;
            let handle = OpenOptions::new()
                .write(true)
                .create_new(true)
                .access_mode(0x40000000 | DELETE)
                .share_mode(FILE_SHARE_READ)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(parent.join(name))?;
            Ok(Self {
                handle,
                _ancestors: ancestors,
            })
        }
        pub fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
            self.handle.write_all(bytes)?;
            self.handle.flush()
        }
        pub fn cleanup(&mut self) {
            delete_on_handle(&self.handle);
        }
    }
    pub struct OwnedDirectory {
        path: PathBuf,
        directory: File,
        files: Vec<File>,
        _ancestors: Vec<File>,
    }
    fn hold_directory(path: &Path, deletable: bool) -> io::Result<File> {
        let access = 0x80000000 | if deletable { DELETE } else { 0 };
        let file = OpenOptions::new()
            .read(true)
            .access_mode(access)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        if !file.metadata()?.is_dir() || file.metadata()?.file_type().is_symlink() {
            return Err(io::Error::other(
                "output directory cannot be a reparse point",
            ));
        }
        Ok(file)
    }
    fn delete_on_handle(file: &File) {
        let info = FILE_DISPOSITION_INFO { DeleteFile: 1 };
        // SAFETY: the handle remains live; the ABI-sized disposition structure is
        // borrowed only during the call. Deletion targets this handle, not a path.
        unsafe {
            SetFileInformationByHandle(
                file.as_raw_handle() as HANDLE,
                FileDispositionInfo,
                &info as *const _ as *const _,
                std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
            );
        }
    }
    impl OwnedDirectory {
        pub fn create(path: &Path) -> io::Result<Self> {
            let name = path.file_name().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "output requires a new directory name",
                )
            })?;
            let parent = path
                .parent()
                .filter(|path| !path.as_os_str().is_empty())
                .unwrap_or(Path::new("."))
                .canonicalize()?;
            // Denying delete sharing on the canonical ancestry prevents directory
            // renames/substitutions while path-based Windows opens are in progress.
            let mut ancestors = Vec::new();
            for ancestor in parent.ancestors() {
                ancestors.push(hold_directory(ancestor, false)?);
            }
            let path = parent.join(name);
            std::fs::create_dir(&path)?;
            let directory = hold_directory(&path, true)?;
            Ok(Self {
                path,
                directory,
                files: Vec::new(),
                _ancestors: ancestors,
            })
        }
        pub fn write(&mut self, name: &str, bytes: &[u8]) -> io::Result<()> {
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .access_mode(0x40000000 | DELETE)
                .share_mode(FILE_SHARE_READ)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(self.path.join(name))?;
            self.files.push(file);
            let file = self.files.last_mut().unwrap();
            file.write_all(bytes)?;
            file.flush()
        }
        pub fn cleanup(&mut self) {
            for file in self.files.drain(..).rev() {
                delete_on_handle(&file);
            }
            delete_on_handle(&self.directory);
        }
    }
}

#[cfg(not(any(unix, windows)))]
compile_error!("geometry output requires a Unix or Windows native filesystem");

#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "spiling-cli-output-{}",
            spiling_contracts::geometry::SessionId::new().as_str()
        ));
        std::fs::create_dir(&path).unwrap();
        path
    }
    #[test]
    fn exclusive_output_rolls_back_only_its_created_files() {
        let root = root();
        let path = root.join("result");
        {
            let mut output = OutputDirectory::create(&path).unwrap();
            output.write("mesh-000001-000000.splm", b"payload").unwrap();
            assert!(
                output
                    .write("mesh-000001-000000.splm", b"replacement")
                    .is_err()
            );
            assert!(output.write("../escape", b"bad").is_err());
            std::fs::write(path.join("user-owned"), b"keep").unwrap();
        }
        assert!(!path.join("mesh-000001-000000.splm").exists());
        assert_eq!(std::fs::read(path.join("user-owned")).unwrap(), b"keep");
        assert!(OutputDirectory::create(&path).is_err());
        std::fs::remove_file(path.join("user-owned")).unwrap();
        std::fs::remove_dir(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
    #[test]
    fn completed_output_persists_and_empty_failed_output_is_removed() {
        let root = root();
        let path = root.join("complete");
        {
            let mut output = OutputDirectory::create(&path).unwrap();
            output.write("manifest.json", b"{}").unwrap();
            output.commit();
        }
        assert_eq!(std::fs::read(path.join("manifest.json")).unwrap(), b"{}");
        let failed = root.join("failed");
        drop(OutputDirectory::create(&failed).unwrap());
        assert!(!failed.exists());
        std::fs::remove_file(path.join("manifest.json")).unwrap();
        std::fs::remove_dir(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn replaced_directory_symlink_is_never_followed_on_write_or_cleanup() {
        use std::os::unix::fs::symlink;
        let root = root();
        let path = root.join("result");
        let moved = root.join("moved");
        let victim = root.join("victim");
        std::fs::create_dir(&victim).unwrap();
        std::fs::write(victim.join("mesh.splm"), b"keep").unwrap();
        let mut output = OutputDirectory::create(&path).unwrap();
        output.write("mesh.splm", b"ours").unwrap();
        std::fs::rename(&path, &moved).unwrap();
        symlink(&victim, &path).unwrap();
        assert!(output.write("second.splm", b"bad").is_err());
        drop(output);
        assert_eq!(std::fs::read(victim.join("mesh.splm")).unwrap(), b"keep");
        assert!(!moved.join("mesh.splm").exists());
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(victim.join("mesh.splm")).unwrap();
        std::fs::remove_dir(victim).unwrap();
        std::fs::remove_dir(moved).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn replaced_payload_is_not_deleted() {
        let root = root();
        let path = root.join("result");
        let mut output = OutputDirectory::create(&path).unwrap();
        output.write("mesh.splm", b"ours").unwrap();
        std::fs::remove_file(path.join("mesh.splm")).unwrap();
        std::fs::write(path.join("mesh.splm"), b"keep").unwrap();
        drop(output);
        assert_eq!(std::fs::read(path.join("mesh.splm")).unwrap(), b"keep");
        std::fs::remove_file(path.join("mesh.splm")).unwrap();
        std::fs::remove_dir(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
    #[test]
    fn replay_file_is_exclusive_and_rolls_back_uncommitted_bytes() {
        let root = root();
        let path = root.join("replay.splr");
        let mut output = OutputFile::create(&path).unwrap();
        output.write(b"checked replay").unwrap();
        assert!(OutputFile::create(&path).is_err());
        drop(output);
        assert!(!path.exists());
        let mut output = OutputFile::create(&path).unwrap();
        output.write(b"checked replay").unwrap();
        output.commit();
        drop(output);
        assert_eq!(std::fs::read(&path).unwrap(), b"checked replay");
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn replay_file_replacement_is_never_written_or_deleted() {
        use std::os::unix::fs::symlink;
        let root = root();
        let path = root.join("replay.splr");
        let victim = root.join("operator.splr");
        std::fs::write(&victim, b"keep").unwrap();
        let mut output = OutputFile::create(&path).unwrap();
        output.write(b"ours").unwrap();
        std::fs::remove_file(&path).unwrap();
        symlink(&victim, &path).unwrap();
        assert!(output.write(b"bad").is_err());
        drop(output);
        assert_eq!(std::fs::read(&victim).unwrap(), b"keep");
        assert!(path.symlink_metadata().unwrap().file_type().is_symlink());
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(victim).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
