// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Anchored namespace admission. Database identities are inspected using metadata only
//! while SQLite is live: closing an independently opened main-file descriptor can drop
//! SQLite's POSIX locks. Only the separate lease and directories retain OS handles.
use crate::{
    APPLICATION_ID, Error, MAX_DATABASE_BYTES, MAX_JOURNAL_BYTES, Result, SCHEMA_VERSION, invalid,
};
use std::{
    collections::BTreeMap,
    fs::{self, File, Metadata, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, OnceLock, Weak},
};

#[cfg(unix)]
use std::os::{
    fd::{AsRawFd, FromRawFd},
    unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
    },
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Identity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl Identity {
    pub fn of(metadata: &Metadata) -> Self {
        Self {
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct Parent {
    path: PathBuf,
    chain: Vec<(PathBuf, File)>,
}

impl Parent {
    pub fn open(path: &Path) -> Result<Arc<Self>> {
        #[cfg(not(unix))]
        {
            let _ = path;
            return Err(Error::Unsupported(
                "guarded PTAC filesystem access is not qualified on this platform".into(),
            ));
        }
        #[cfg(unix)]
        {
            let absolute = if path.is_absolute() {
                path.to_path_buf()
            } else {
                std::env::current_dir()?.join(path)
            };
            let mut normalized = PathBuf::from("/");
            for component in absolute.components() {
                match component {
                    Component::Normal(name) => normalized.push(name),
                    Component::ParentDir => {
                        normalized.pop();
                    }
                    Component::RootDir | Component::CurDir => (),
                    Component::Prefix(_) => return Err(invalid("invalid native path prefix")),
                }
            }
            let root = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open("/")?;
            let mut chain = vec![(PathBuf::from("/"), root)];
            let mut traversed = PathBuf::from("/");
            for component in normalized.components() {
                let Component::Normal(name) = component else {
                    continue;
                };
                let name =
                    std::ffi::CString::new(name.as_bytes()).map_err(|_| invalid("NUL in path"))?;
                let descriptor = unsafe {
                    libc::openat(
                        chain.last().expect("root ancestor").1.as_raw_fd(),
                        name.as_ptr(),
                        libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                    )
                };
                if descriptor < 0 {
                    return Err(std::io::Error::last_os_error().into());
                }
                let directory = unsafe { File::from_raw_fd(descriptor) };
                traversed.push(component.as_os_str());
                chain.push((traversed.clone(), directory));
            }
            Ok(Arc::new(Self {
                path: normalized,
                chain,
            }))
        }
    }
    pub fn check(&self) -> Result<()> {
        for (path, handle) in &self.chain {
            let metadata = fs::symlink_metadata(path)?;
            if !metadata.is_dir()
                || metadata.file_type().is_symlink()
                || Identity::of(&metadata) != Identity::of(&handle.metadata()?)
            {
                return Err(invalid("attached path ancestor was substituted"));
            }
        }
        Ok(())
    }
    pub fn sync(&self) -> Result<()> {
        self.check()?;
        self.chain
            .last()
            .ok_or_else(|| invalid("missing parent handle"))?
            .1
            .sync_all()?;
        Ok(())
    }
    pub fn lease(&self, name: &std::ffi::OsStr) -> Result<File> {
        self.check()?;
        let path = self.path.join(name);
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).truncate(false);
        #[cfg(unix)]
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let file = options.open(&path)?;
        regular(&file.metadata()?, 4096)?;
        if Identity::of(&fs::symlink_metadata(&path)?) != Identity::of(&file.metadata()?) {
            return Err(invalid("writer lease was substituted"));
        }
        fs2::FileExt::try_lock_exclusive(&file).map_err(|error| {
            if error.kind() == std::io::ErrorKind::WouldBlock {
                Error::Locked
            } else {
                error.into()
            }
        })?;
        Ok(file)
    }
}

fn regular(metadata: &Metadata, maximum: u64) -> Result<()> {
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(invalid("expected regular non-symlink file"));
    }
    #[cfg(unix)]
    if metadata.nlink() != 1 {
        return Err(invalid("hard-linked PTAC file/sidecar is forbidden"));
    }
    if metadata.len() > maximum {
        return Err(crate::limit("physical file/sidecar bytes"));
    }
    Ok(())
}

#[derive(Debug)]
pub(crate) struct Location {
    pub parent: Arc<Parent>,
    pub path: PathBuf,
    pub journal: PathBuf,
    identity: Mutex<Option<Identity>>,
    journal_identity: Mutex<Option<Identity>>,
    pub readers: std::sync::atomic::AtomicUsize,
}

static ATTACHMENTS: OnceLock<Mutex<BTreeMap<PathBuf, Weak<Location>>>> = OnceLock::new();

impl Location {
    pub fn matches_path(&self, path: &Path) -> Result<bool> {
        let Some(name) = path.file_name() else {
            return Ok(false);
        };
        let parent = Parent::open(
            path.parent()
                .filter(|path| !path.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?;
        let same = parent.path.join(name) == self.path;
        if same {
            self.check(false)?;
        }
        Ok(same)
    }
    pub fn attach(path: &Path, creating: bool) -> Result<Arc<Self>> {
        let name = path
            .file_name()
            .ok_or_else(|| invalid("project path needs a filename"))?;
        let parent = Parent::open(
            path.parent()
                .filter(|path| !path.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?;
        let path = parent.path.join(name);
        let mut registry = ATTACHMENTS
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| invalid("attachment registry poisoned"))?;
        registry.retain(|_, value| value.strong_count() != 0);
        if let Some(existing) = registry.get(&path).and_then(Weak::upgrade) {
            if creating {
                return Err(Error::Conflict);
            }
            existing.check(false)?;
            return Ok(existing);
        }
        let journal = companion(&path, "-journal");
        let location = Arc::new(Self {
            parent,
            path: path.clone(),
            journal,
            identity: Mutex::new(None),
            journal_identity: Mutex::new(None),
            readers: 0.into(),
        });
        if creating {
            if fs::symlink_metadata(&path).is_ok() {
                return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists).into());
            }
        } else {
            location.check(false)?;
            let mut options = OpenOptions::new();
            options.read(true);
            #[cfg(unix)]
            options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
            let mut file = options.open(&path)?;
            let mut header = [0u8; 100];
            file.read_exact(&mut header)?;
            if &header[..16] != b"SQLite format 3\0"
                || u16::from_be_bytes(header[16..18].try_into().unwrap()) != 4096
                || header[18] != 1
                || header[19] != 1
                || u32::from_be_bytes(header[60..64].try_into().unwrap()) != SCHEMA_VERSION
                || u32::from_be_bytes(header[68..72].try_into().unwrap()) != APPLICATION_ID
            {
                return Err(Error::Unsupported(
                    "not the current Spiling PTAC container".into(),
                ));
            }
            if Some(Identity::of(&file.metadata()?))
                != *location
                    .identity
                    .lock()
                    .map_err(|_| invalid("identity poisoned"))?
            {
                return Err(invalid("file substituted during preflight"));
            }
        }
        for suffix in ["-wal", "-shm"] {
            if fs::symlink_metadata(companion(&path, suffix)).is_ok() {
                return Err(Error::Unsupported("WAL/SHM sidecars are forbidden".into()));
            }
        }
        location.check_journal(true)?;
        registry.insert(path, Arc::downgrade(&location));
        Ok(location)
    }
    pub fn check(&self, missing: bool) -> Result<()> {
        self.parent.check()?;
        match fs::symlink_metadata(&self.path) {
            Ok(metadata) => {
                regular(&metadata, MAX_DATABASE_BYTES)?;
                let mut identity = self
                    .identity
                    .lock()
                    .map_err(|_| invalid("identity poisoned"))?;
                let actual = Identity::of(&metadata);
                if identity
                    .as_ref()
                    .is_some_and(|expected| expected != &actual)
                {
                    return Err(invalid("attached database was substituted"));
                }
                *identity = Some(actual);
                Ok(())
            }
            Err(error) if missing && error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
    pub fn check_journal(&self, missing: bool) -> Result<()> {
        self.parent.check()?;
        match fs::symlink_metadata(&self.journal) {
            Ok(metadata) => {
                regular(&metadata, MAX_JOURNAL_BYTES)?;
                let mut identity = self
                    .journal_identity
                    .lock()
                    .map_err(|_| invalid("journal identity poisoned"))?;
                let actual = Identity::of(&metadata);
                if identity
                    .as_ref()
                    .is_some_and(|expected| expected != &actual)
                {
                    return Err(invalid("journal was substituted"));
                }
                *identity = Some(actual);
                Ok(())
            }
            Err(error) if missing && error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
    pub fn journal_deleted(&self) {
        if let Ok(mut identity) = self.journal_identity.lock() {
            *identity = None;
        }
    }
    pub fn journal_absent(&self) -> bool {
        self.parent.check().is_ok()
            && fs::symlink_metadata(&self.journal)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
    }
    pub fn writer_lease(&self) -> Result<File> {
        let name = self
            .path
            .file_name()
            .ok_or_else(|| invalid("missing filename"))?;
        let mut lease = name.to_os_string();
        lease.push(".lock");
        self.parent.lease(&lease)
    }
    pub fn check_lease(&self, lease: &File) -> Result<()> {
        let metadata = fs::symlink_metadata(companion(&self.path, ".lock"))?;
        regular(&metadata, 4096)?;
        if Identity::of(&metadata) != Identity::of(&lease.metadata()?) {
            return Err(invalid("writer lease was substituted"));
        }
        Ok(())
    }
    pub fn admit_journal(&self) -> Result<()> {
        self.check_journal(true)?;
        let mut options = OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
        let mut file = match options.open(&self.journal) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let metadata = file.metadata()?;
        regular(&metadata, MAX_JOURNAL_BYTES)?;
        if Some(Identity::of(&metadata))
            != *self
                .journal_identity
                .lock()
                .map_err(|_| invalid("journal identity poisoned"))?
        {
            return Err(invalid("journal substituted during admission"));
        }
        if metadata.len() >= 16 {
            file.seek(SeekFrom::End(-8))?;
            let mut magic = [0u8; 8];
            file.read_exact(&mut magic)?;
            if magic == [0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7] {
                return Err(invalid("super-journal transactions are forbidden"));
            }
        }
        Ok(())
    }
    pub fn absent_wal(&self) -> Result<()> {
        self.parent.check()?;
        for suffix in ["-wal", "-shm"] {
            match fs::symlink_metadata(companion(&self.path, suffix)) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
                Err(error) => return Err(error.into()),
                Ok(_) => return Err(invalid("WAL/SHM sidecars are forbidden")),
            }
        }
        self.admit_journal()
    }
    pub fn main_identity(&self) -> Option<Identity> {
        self.identity.lock().ok()?.clone()
    }
}

pub(crate) fn companion(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

pub(crate) fn path_bytes(path: &Path) -> Result<Vec<u8>> {
    #[cfg(unix)]
    {
        Ok(path.as_os_str().as_bytes().to_vec())
    }
    #[cfg(not(unix))]
    {
        path.to_str()
            .map(|path| path.as_bytes().to_vec())
            .ok_or_else(|| invalid("unsupported native SQLite path encoding"))
    }
}

pub(crate) struct Stage {
    pub location: Arc<Location>,
    directory: OwnedDirectory,
    _scratch: Scratch,
}

struct OwnedDirectory {
    path: PathBuf,
    identity: Identity,
}

impl OwnedDirectory {
    fn unchanged(&self) -> bool {
        fs::symlink_metadata(&self.path)
            .is_ok_and(|metadata| metadata.is_dir() && Identity::of(&metadata) == self.identity)
    }
}

impl Drop for OwnedDirectory {
    fn drop(&mut self) {
        if self.unchanged() {
            let _ = fs::remove_dir(&self.path);
        }
    }
}

impl Stage {
    pub fn new(parent: &Arc<Parent>) -> Result<Self> {
        parent.check()?;
        let directory = parent
            .path
            .join(format!(".spiling-stage-{}", uuid::Uuid::new_v4()));
        let scratch = Scratch::reserve(parent, &directory)?;
        fs::create_dir(&directory)?;
        let identity = Identity::of(&fs::symlink_metadata(&directory)?);
        let directory = OwnedDirectory {
            path: directory,
            identity,
        };
        let location = Location::attach(&directory.path.join("project.spil"), true)?;
        Ok(Self {
            location,
            directory,
            _scratch: scratch,
        })
    }
    pub fn seal(&self) -> Result<()> {
        self.location.check(false)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&self.location.path)?;
        file.sync_all()?;
        #[cfg(target_os = "macos")]
        if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_FULLFSYNC) } < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        drop(file);
        if fs::symlink_metadata(&self.location.journal).is_ok() {
            return Err(invalid("closed staging database retained a journal"));
        }
        self.location.parent.sync()
    }
    pub fn publish(&self, target: &Path, parent: &Parent) -> Result<()> {
        self.location.check(false)?;
        parent.check()?;
        #[cfg(unix)]
        {
            let source = std::ffi::CString::new(
                self.location
                    .path
                    .file_name()
                    .ok_or_else(|| invalid("missing stage filename"))?
                    .as_bytes(),
            )
            .map_err(|_| invalid("NUL in stage path"))?;
            let destination = std::ffi::CString::new(
                target
                    .file_name()
                    .ok_or_else(|| invalid("missing destination filename"))?
                    .as_bytes(),
            )
            .map_err(|_| invalid("NUL in destination path"))?;
            let source_fd = self.location.parent.chain.last().unwrap().1.as_raw_fd();
            let destination_fd = parent.chain.last().unwrap().1.as_raw_fd();
            #[cfg(target_os = "linux")]
            let result = unsafe {
                libc::renameat2(
                    source_fd,
                    source.as_ptr(),
                    destination_fd,
                    destination.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            };
            #[cfg(target_os = "macos")]
            let result = unsafe {
                libc::renameatx_np(
                    source_fd,
                    source.as_ptr(),
                    destination_fd,
                    destination.as_ptr(),
                    libc::RENAME_EXCL,
                )
            };
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            return Err(Error::Unsupported(
                "atomic exclusive publication is unsupported".into(),
            ));
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            if result < 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            Ok(())
        }
        #[cfg(not(unix))]
        {
            let _ = (target, parent);
            Err(Error::Unsupported(
                "atomic PTAC publication is unsupported".into(),
            ))
        }
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        if self.directory.unchanged() {
            if let Ok(identity) = self.location.journal_identity.lock()
                && let Some(identity) = identity.as_ref()
                && fs::symlink_metadata(&self.location.journal)
                    .is_ok_and(|metadata| Identity::of(&metadata) == *identity)
            {
                let _ = fs::remove_file(&self.location.journal);
            }
            if let Some(identity) = self.location.main_identity()
                && fs::symlink_metadata(&self.location.path)
                    .is_ok_and(|metadata| Identity::of(&metadata) == identity)
            {
                let _ = fs::remove_file(&self.location.path);
            }
        }
    }
}

pub(crate) struct Spool {
    pub file: File,
    path: PathBuf,
    identity: Identity,
    parent: Arc<Parent>,
    scratch: Scratch,
}

impl Spool {
    pub fn new(parent: &Arc<Parent>) -> Result<Self> {
        parent.check()?;
        let path = parent
            .path
            .join(format!(".spiling-chunks-{}", uuid::Uuid::new_v4()));
        let scratch = Scratch::reserve(parent, &path)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)?;
        let identity = Identity::of(&file.metadata()?);
        Ok(Self {
            file,
            path,
            identity,
            parent: parent.clone(),
            scratch,
        })
    }
    pub fn append(&mut self, bytes: &[u8]) -> Result<()> {
        self.scratch.charge(bytes.len())?;
        self.file.write_all(bytes)?;
        Ok(())
    }
}

static SCRATCH: OnceLock<Mutex<BTreeMap<PathBuf, (PathBuf, u64)>>> = OnceLock::new();

pub(crate) fn scratch_usage(parent: &Parent) -> Result<(u64, u64)> {
    let active = SCRATCH
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| invalid("scratch registry poisoned"))?;
    Ok(active
        .values()
        .filter(|(owner, _)| *owner == parent.path)
        .fold((0, 0), |(bytes, count), (_, size)| {
            (bytes + size, count + 1)
        }))
}

struct Scratch {
    path: PathBuf,
}

impl Scratch {
    fn reserve(parent: &Parent, path: &Path) -> Result<Self> {
        let group = if parent
            .path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with(".spiling-stage-"))
        {
            parent
                .path
                .parent()
                .ok_or_else(|| invalid("scratch parent"))?
                .to_path_buf()
        } else {
            parent.path.clone()
        };
        let mut active = SCRATCH
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| invalid("scratch registry poisoned"))?;
        for (index, entry) in fs::read_dir(&group)?.enumerate() {
            if index >= 4096 {
                return Err(crate::limit("scratch directory admission entries"));
            }
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if (name.starts_with(".spiling-stage-") || name.starts_with(".spiling-chunks-"))
                && !active.contains_key(&entry.path())
            {
                return Err(crate::limit(
                    "unowned scratch entry; inspect interrupted work before saving",
                ));
            }
        }
        if active.values().filter(|(owner, _)| *owner == group).count() >= 8 {
            return Err(crate::limit("eight aggregate owned scratch entries"));
        }
        active.insert(path.to_path_buf(), (group, 0));
        Ok(Self {
            path: path.to_path_buf(),
        })
    }
    fn charge(&mut self, bytes: usize) -> Result<()> {
        let mut active = SCRATCH
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| invalid("scratch registry poisoned"))?;
        let group = &active
            .get(&self.path)
            .ok_or_else(|| invalid("missing scratch owner"))?
            .0;
        let total = active
            .values()
            .filter(|(owner, _)| owner == group)
            .try_fold(bytes as u64, |total, (_, size)| total.checked_add(*size))
            .ok_or_else(|| crate::limit("scratch byte arithmetic"))?;
        if total > 192 * 1024 * 1024 {
            return Err(crate::limit("192 MiB aggregate scratch bytes"));
        }
        active
            .get_mut(&self.path)
            .ok_or_else(|| invalid("missing scratch owner"))?
            .1 += bytes as u64;
        Ok(())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        if let Ok(mut active) = SCRATCH.get_or_init(Default::default).lock() {
            active.remove(&self.path);
        }
    }
}

impl Drop for Spool {
    fn drop(&mut self) {
        if self.parent.check().is_ok()
            && fs::symlink_metadata(&self.path)
                .is_ok_and(|metadata| Identity::of(&metadata) == self.identity)
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}
