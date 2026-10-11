// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Small OS boundary: anchored no-follow IO, exclusive publication and durability.
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    fs::{File, OpenOptions},
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};
#[derive(Debug)]
pub(super) struct Directory {
    file: File,
    path: PathBuf,
    owned: Arc<Mutex<BTreeMap<String, File>>>,
}
fn bad(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn component(name: &str) -> io::Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains(['/', '\\', '\0']) {
        return Err(bad("invalid project path component"));
    }
    Ok(())
}
#[cfg(test)]
thread_local! { static FAIL_NEXT_DIRECTORY_SYNC: std::cell::Cell<bool> = const { std::cell::Cell::new(false) }; }
#[cfg(test)]
pub(super) fn fail_next_directory_sync() {
    FAIL_NEXT_DIRECTORY_SYNC.with(|fail| fail.set(true));
}
impl Directory {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn open(path: &Path) -> io::Result<Self> {
        if std::fs::symlink_metadata(path)?.file_type().is_symlink() {
            return Err(bad("project directory must not be a symlink"));
        }
        let file = platform::open_directory(path)?;
        if !file.metadata()?.is_dir() {
            return Err(bad("project path is not a directory"));
        }
        Ok(Self {
            file,
            path: std::fs::canonicalize(path)?,
            owned: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }
    pub fn relocated(&self, path: PathBuf) -> io::Result<Self> {
        Ok(Self {
            file: self.file.try_clone()?,
            path,
            owned: self.owned.clone(),
        })
    }
    pub fn check_path(&self) -> io::Result<()> {
        if !self.same(&Self::open(&self.path)?)? {
            return Err(bad("project directory was substituted"));
        }
        Ok(())
    }
    pub fn same(&self, other: &Self) -> io::Result<bool> {
        platform::same_file(&self.file, &other.file)
    }
    pub fn same_entry(&self, name: &str, original: &File) -> io::Result<bool> {
        platform::same_file(&self.read(name)?, original)
    }
    pub fn child(&self, name: &str) -> io::Result<Self> {
        component(name)?;
        let file = platform::child_directory(self, name)?;
        if !file.metadata()?.is_dir() {
            return Err(bad("project entry is not a directory"));
        }
        Ok(Self {
            file,
            path: self.path.join(name),
            owned: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }
    pub fn create_child(&self, name: &str) -> io::Result<Self> {
        component(name)?;
        platform::mkdir(self, name)?;
        self.child(name)
    }
    pub fn read(&self, name: &str) -> io::Result<File> {
        component(name)?;
        let file = platform::open_file(self, name, false, false)?;
        if !file.metadata()?.is_file() {
            return Err(bad("project entry must be a regular non-symlink file"));
        }
        Ok(file)
    }
    pub fn create(&self, name: &str) -> io::Result<File> {
        component(name)?;
        let file = platform::open_file(self, name, true, true)?;
        self.owned
            .lock()
            .map_err(|_| bad("ownership map poisoned"))?
            .insert(name.to_owned(), file.try_clone()?);
        Ok(file)
    }
    pub fn lock_file(&self, name: &str) -> io::Result<File> {
        component(name)?;
        let file = match self.create(name) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                platform::open_file(self, name, true, false)?
            }
            Err(e) => return Err(e),
        };
        if !file.metadata()?.is_file() {
            return Err(bad("writer lock must be a regular non-symlink file"));
        }
        Ok(file)
    }
    pub fn exists(&self, name: &str) -> io::Result<bool> {
        component(name)?;
        platform::exists(self, OsStr::new(name))
    }
    pub fn exists_native(&self, name: &OsStr) -> io::Result<bool> {
        if Path::new(name).file_name() != Some(name) {
            return Err(bad("invalid native directory basename"));
        }
        platform::exists(self, name)
    }
    pub fn entries(&self, max: usize) -> io::Result<Vec<String>> {
        platform::entries(self, max)
    }
    pub fn replace(&self, source: &str, destination: &str) -> io::Result<()> {
        component(source)?;
        component(destination)?;
        if self.exists(destination)? {
            self.read(destination)?;
        }
        platform::rename(self, source, OsStr::new(destination), false)?;
        self.move_ownership(source, destination)
    }
    pub fn publish_file(&self, source: &str, destination: &str) -> io::Result<()> {
        component(source)?;
        component(destination)?;
        platform::publish_file(self, source, destination)?;
        self.move_ownership(source, destination)
    }
    fn move_ownership(&self, source: &str, destination: &str) -> io::Result<()> {
        let mut owned = self
            .owned
            .lock()
            .map_err(|_| bad("ownership map poisoned"))?;
        if let Some(file) = owned.remove(source) {
            owned.insert(destination.to_owned(), file);
        }
        Ok(())
    }
    pub fn forget_owned(&self, name: &str) {
        if let Ok(mut owned) = self.owned.lock() {
            owned.remove(name);
        }
    }
    pub fn publish_directory(&self, source: &str, destination: &OsStr) -> io::Result<()> {
        component(source)?;
        if Path::new(destination).file_name() != Some(destination) {
            return Err(bad("invalid native directory basename"));
        }
        platform::rename(self, source, destination, true)
    }
    pub fn sync(&self) -> io::Result<()> {
        #[cfg(test)]
        if FAIL_NEXT_DIRECTORY_SYNC.with(|fail| fail.replace(false)) {
            return Err(io::Error::other("injected directory durability failure"));
        }
        platform::sync_directory(self)
    }
    pub fn remove_owned(&self, name: &str, original: &File) -> io::Result<()> {
        if platform::same_file(&self.read(name)?, original)? {
            platform::unlink(self, name, false)?;
        }
        Ok(())
    }
    pub fn clean_owned(&self) -> io::Result<()> {
        let owned = self
            .owned
            .lock()
            .map_err(|_| bad("ownership map poisoned"))?;
        for (name, original) in owned.iter() {
            let _ = self.remove_owned(name, original);
        }
        Ok(())
    }
    pub fn remove_child_owned(&self, name: &str, original: &Self) -> io::Result<()> {
        if self.child(name)?.same(original)? {
            platform::unlink(self, name, true)?;
        }
        Ok(())
    }
}
pub(super) fn sync_file(file: &File) -> io::Result<()> {
    file.sync_all()?;
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        // macOS fsync alone need not flush the device write cache.
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_FULLFSYNC) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}
#[cfg(unix)]
mod platform {
    use super::*;
    use std::{
        ffi::{CStr, CString},
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::{
                ffi::OsStrExt,
                fs::{MetadataExt, OpenOptionsExt},
            },
        },
    };
    fn c(name: &str) -> io::Result<CString> {
        CString::new(name).map_err(|_| bad("path contains NUL"))
    }
    fn result(value: libc::c_int) -> io::Result<()> {
        if value == -1 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
    fn openat(directory: &Directory, name: &str, flags: libc::c_int) -> io::Result<File> {
        let name = c(name)?;
        let fd = unsafe {
            libc::openat(
                directory.file.as_raw_fd(),
                name.as_ptr(),
                flags | libc::O_CLOEXEC | libc::O_NOFOLLOW,
                0o600,
            )
        };
        if fd == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(unsafe { File::from_raw_fd(fd) })
    }
    pub fn open_directory(path: &Path) -> io::Result<File> {
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
    }
    pub fn child_directory(directory: &Directory, name: &str) -> io::Result<File> {
        openat(directory, name, libc::O_RDONLY | libc::O_DIRECTORY)
    }
    pub fn open_file(
        directory: &Directory,
        name: &str,
        write: bool,
        create: bool,
    ) -> io::Result<File> {
        // O_NONBLOCK prevents malicious FIFOs/devices from blocking before type validation.
        openat(
            directory,
            name,
            (if write { libc::O_RDWR } else { libc::O_RDONLY })
                | libc::O_NONBLOCK
                | if create {
                    libc::O_CREAT | libc::O_EXCL
                } else {
                    0
                },
        )
    }
    pub fn same_file(a: &File, b: &File) -> io::Result<bool> {
        let a = a.metadata()?;
        let b = b.metadata()?;
        Ok(a.dev() == b.dev() && a.ino() == b.ino())
    }
    pub fn mkdir(directory: &Directory, name: &str) -> io::Result<()> {
        result(unsafe { libc::mkdirat(directory.file.as_raw_fd(), c(name)?.as_ptr(), 0o700) })
    }
    pub fn exists(directory: &Directory, name: &OsStr) -> io::Result<bool> {
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        let name = CString::new(name.as_bytes()).map_err(|_| bad("path contains NUL"))?;
        let value = unsafe {
            libc::fstatat(
                directory.file.as_raw_fd(),
                name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if value == 0 {
            Ok(true)
        } else {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::NotFound {
                Ok(false)
            } else {
                Err(e)
            }
        }
    }
    pub fn entries(directory: &Directory, max: usize) -> io::Result<Vec<String>> {
        // Independent open file description; dup would share readdir offsets.
        let file = openat(directory, ".", libc::O_RDONLY | libc::O_DIRECTORY)?;
        use std::os::fd::IntoRawFd;
        let fd = file.into_raw_fd();
        let dir = unsafe { libc::fdopendir(fd) };
        if dir.is_null() {
            unsafe {
                libc::close(fd);
            }
            return Err(io::Error::last_os_error());
        }
        struct Close(*mut libc::DIR);
        impl Drop for Close {
            fn drop(&mut self) {
                unsafe {
                    libc::closedir(self.0);
                }
            }
        }
        let _close = Close(dir);
        let mut entries = Vec::new();
        loop {
            #[cfg(target_os = "linux")]
            unsafe {
                *libc::__errno_location() = 0;
            }
            #[cfg(target_os = "macos")]
            unsafe {
                *libc::__error() = 0;
            }
            let entry = unsafe { libc::readdir(dir) };
            if entry.is_null() {
                let error = io::Error::last_os_error();
                if error.raw_os_error().unwrap_or(0) != 0 {
                    return Err(error);
                }
                break;
            }
            let name = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }
                .to_str()
                .map_err(|_| bad("project entry name must be Unicode"))?;
            if name == "." || name == ".." {
                continue;
            }
            if entries.len() == max {
                return Err(io::Error::new(
                    io::ErrorKind::FileTooLarge,
                    "project directory entry budget exceeded",
                ));
            }
            entries.push(name.to_owned());
        }
        Ok(entries)
    }
    pub fn rename(
        directory: &Directory,
        source: &str,
        destination: &OsStr,
        exclusive: bool,
    ) -> io::Result<()> {
        let source = c(source)?;
        let destination =
            CString::new(destination.as_bytes()).map_err(|_| bad("path contains NUL"))?;
        let fd = directory.file.as_raw_fd();
        if !exclusive {
            return result(unsafe {
                libc::renameat(fd, source.as_ptr(), fd, destination.as_ptr())
            });
        }
        #[cfg(target_os = "linux")]
        {
            result(unsafe {
                libc::renameat2(
                    fd,
                    source.as_ptr(),
                    fd,
                    destination.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            })
        }
        #[cfg(target_os = "macos")]
        {
            result(unsafe {
                libc::renameatx_np(
                    fd,
                    source.as_ptr(),
                    fd,
                    destination.as_ptr(),
                    libc::RENAME_EXCL,
                )
            })
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "exclusive project publication unsupported on this OS",
            ))
        }
    }
    pub fn publish_file(directory: &Directory, source: &str, destination: &str) -> io::Result<()> {
        let fd = directory.file.as_raw_fd();
        result(unsafe { libc::linkat(fd, c(source)?.as_ptr(), fd, c(destination)?.as_ptr(), 0) })?;
        unlink(directory, source, false)
    }
    pub fn unlink(directory: &Directory, name: &str, is_dir: bool) -> io::Result<()> {
        result(unsafe {
            libc::unlinkat(
                directory.file.as_raw_fd(),
                c(name)?.as_ptr(),
                if is_dir { libc::AT_REMOVEDIR } else { 0 },
            )
        })
    }
    pub fn sync_directory(directory: &Directory) -> io::Result<()> {
        directory.file.sync_all()
    }
}
#[cfg(windows)]
mod platform {
    use super::*;
    use std::os::windows::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
    };
    use windows_sys::Win32::{Foundation::GetLastError, Storage::FileSystem::*};
    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }
    fn reject_reparse(file: File) -> io::Result<File> {
        if file.metadata()?.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err(bad("project entry must not be a reparse point"));
        }
        Ok(file)
    }
    pub fn open_directory(path: &Path) -> io::Result<File> {
        reject_reparse(
            OpenOptions::new()
                .read(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(path)?,
        )
    }
    pub fn child_directory(directory: &Directory, name: &str) -> io::Result<File> {
        directory.check_path()?;
        open_directory(&directory.path.join(name))
    }
    pub fn open_file(
        directory: &Directory,
        name: &str,
        write: bool,
        create: bool,
    ) -> io::Result<File> {
        directory.check_path()?;
        reject_reparse(
            OpenOptions::new()
                .read(true)
                .write(write)
                .create_new(create)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
                .open(directory.path.join(name))?,
        )
    }
    pub fn same_file(a: &File, b: &File) -> io::Result<bool> {
        let mut a_info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        let mut b_info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        if unsafe { GetFileInformationByHandle(a.as_raw_handle(), a_info.as_mut_ptr()) } == 0
            || unsafe { GetFileInformationByHandle(b.as_raw_handle(), b_info.as_mut_ptr()) } == 0
        {
            return Err(io::Error::last_os_error());
        }
        let a = unsafe { a_info.assume_init() };
        let b = unsafe { b_info.assume_init() };
        Ok(a.dwVolumeSerialNumber == b.dwVolumeSerialNumber
            && a.nFileIndexHigh == b.nFileIndexHigh
            && a.nFileIndexLow == b.nFileIndexLow)
    }
    pub fn mkdir(directory: &Directory, name: &str) -> io::Result<()> {
        directory.check_path()?;
        std::fs::create_dir(directory.path.join(name))
    }
    pub fn exists(directory: &Directory, name: &OsStr) -> io::Result<bool> {
        directory.check_path()?;
        match std::fs::symlink_metadata(directory.path.join(name)) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }
    pub fn entries(directory: &Directory, max: usize) -> io::Result<Vec<String>> {
        directory.check_path()?;
        let mut entries = Vec::new();
        for entry in std::fs::read_dir(&directory.path)? {
            if entries.len() == max {
                return Err(io::Error::new(
                    io::ErrorKind::FileTooLarge,
                    "project directory entry budget exceeded",
                ));
            }
            entries.push(
                entry?
                    .file_name()
                    .into_string()
                    .map_err(|_| bad("project entry name must be Unicode"))?,
            );
        }
        Ok(entries)
    }
    pub fn rename(
        directory: &Directory,
        source: &str,
        destination: &OsStr,
        exclusive: bool,
    ) -> io::Result<()> {
        directory.check_path()?;
        let flags = MOVEFILE_WRITE_THROUGH
            | if exclusive {
                0
            } else {
                MOVEFILE_REPLACE_EXISTING
            };
        if unsafe {
            MoveFileExW(
                wide(&directory.path.join(source)).as_ptr(),
                wide(&directory.path.join(destination)).as_ptr(),
                flags,
            )
        } == 0
        {
            Err(io::Error::from_raw_os_error(
                unsafe { GetLastError() } as i32
            ))
        } else {
            Ok(())
        }
    }
    pub fn publish_file(directory: &Directory, source: &str, destination: &str) -> io::Result<()> {
        rename(directory, source, OsStr::new(destination), true)
    }
    pub fn unlink(directory: &Directory, name: &str, is_dir: bool) -> io::Result<()> {
        directory.check_path()?;
        if is_dir {
            std::fs::remove_dir(directory.path.join(name))
        } else {
            std::fs::remove_file(directory.path.join(name))
        }
    }
    pub fn sync_directory(directory: &Directory) -> io::Result<()> {
        // No weak fallback: filesystems refusing directory flushing produce Io.
        directory.check_path()?;
        let file = reject_reparse(
            OpenOptions::new()
                .read(true)
                .write(true)
                .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
                .open(&directory.path)?,
        )?;
        file.sync_all()
    }
}
#[cfg(not(any(unix, windows)))]
compile_error!("spiling-core storage requires a declared desktop OS");
