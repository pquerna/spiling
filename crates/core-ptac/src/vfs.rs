// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Trusted namespace wrapper, not a page or journal implementation.
//!
//! Safety: SQLite owns each aligned GuardFile allocation and invokes callbacks only
//! between xOpen/xClose. Its trailing real-file allocation is aligned and sized for
//! the wrapped standard VFS. Registration and State have stable boxed addresses;
//! Database drops its connection before registration. Callbacks never unwind into C.
//! The standard VFS owns all real descriptors and locking. No extra main-file FD is
//! opened here. No global default VFS or system-call configuration is changed.
use crate::{
    MAX_DATABASE_BYTES, MAX_JOURNAL_BYTES, Result, invalid,
    storage_os::{Location, path_bytes},
};
use rusqlite::ffi;
use std::{
    ffi::{CStr, CString, c_char, c_int, c_void},
    ptr,
    sync::Arc,
};

struct State {
    base: *mut ffi::sqlite3_vfs,
    location: Arc<Location>,
    main: Vec<u8>,
    journal: Vec<u8>,
    writable: bool,
    creating: bool,
}

pub(crate) struct Guard {
    registration: Box<ffi::sqlite3_vfs>,
    _state: Box<State>,
    name: CString,
}

unsafe impl Send for Guard {}

impl Guard {
    pub fn new(location: Arc<Location>, writable: bool, creating: bool) -> Result<Self> {
        unsafe {
            let base = ffi::sqlite3_vfs_find(ptr::null());
            if base.is_null() || (*base).iVersion < 1 {
                return Err(invalid("standard SQLite VFS unavailable"));
            }
            let name = CString::new(format!("spiling-{}", uuid::Uuid::new_v4()))
                .map_err(|_| invalid("VFS name"))?;
            let mut state = Box::new(State {
                base,
                main: path_bytes(&location.path)?,
                journal: path_bytes(&location.journal)?,
                location,
                writable,
                creating,
            });
            let mut registration = Box::new(ffi::sqlite3_vfs {
                iVersion: 2,
                szOsFile: (std::mem::size_of::<GuardFile>() + (*base).szOsFile as usize) as c_int,
                mxPathname: (*base).mxPathname,
                pNext: ptr::null_mut(),
                zName: name.as_ptr(),
                pAppData: (&mut *state as *mut State).cast(),
                xOpen: Some(open),
                xDelete: Some(delete),
                xAccess: Some(access),
                xFullPathname: Some(full_path),
                xDlOpen: None,
                xDlError: None,
                xDlSym: None,
                xDlClose: None,
                xRandomness: Some(randomness),
                xSleep: Some(sleep),
                xCurrentTime: Some(current_time),
                xGetLastError: Some(last_error),
                xCurrentTimeInt64: Some(current_time64),
                xSetSystemCall: None,
                xGetSystemCall: None,
                xNextSystemCall: None,
            });
            if ffi::sqlite3_vfs_register(&mut *registration, 0) != ffi::SQLITE_OK {
                return Err(invalid("could not register guarded SQLite VFS"));
            }
            Ok(Self {
                registration,
                _state: state,
                name,
            })
        }
    }
    pub fn name(&self) -> &str {
        self.name.to_str().expect("ASCII VFS name")
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        unsafe {
            ffi::sqlite3_vfs_unregister(&mut *self.registration);
        }
    }
}

unsafe fn state<'a>(vfs: *mut ffi::sqlite3_vfs) -> &'a State {
    unsafe { &*((*vfs).pAppData.cast::<State>()) }
}

unsafe fn allowed(state: &State, name: *const c_char, missing: bool) -> Option<bool> {
    if name.is_null() {
        return None;
    }
    let name = unsafe { CStr::from_ptr(name) }.to_bytes();
    if name == state.main && state.location.check(missing).is_ok() {
        Some(false)
    } else if name == state.journal && state.location.check_journal(missing).is_ok() {
        Some(true)
    } else {
        None
    }
}

#[repr(C)]
struct GuardFile {
    header: ffi::sqlite3_file,
    real: *mut ffi::sqlite3_file,
    maximum: i64,
}

unsafe extern "C" fn open(
    vfs: *mut ffi::sqlite3_vfs,
    name: ffi::sqlite3_filename,
    file: *mut ffi::sqlite3_file,
    flags: c_int,
    output: *mut c_int,
) -> c_int {
    unsafe {
        let state = state(vfs);
        let journal = match allowed(
            state,
            name,
            state.creating || flags & ffi::SQLITE_OPEN_CREATE != 0,
        ) {
            Some(journal) => journal,
            None => return ffi::SQLITE_CANTOPEN,
        };
        let kind = flags
            & (ffi::SQLITE_OPEN_MAIN_DB
                | ffi::SQLITE_OPEN_MAIN_JOURNAL
                | ffi::SQLITE_OPEN_TEMP_DB
                | ffi::SQLITE_OPEN_TRANSIENT_DB
                | ffi::SQLITE_OPEN_TEMP_JOURNAL
                | ffi::SQLITE_OPEN_SUBJOURNAL
                | ffi::SQLITE_OPEN_SUPER_JOURNAL
                | ffi::SQLITE_OPEN_WAL);
        if (journal && kind != ffi::SQLITE_OPEN_MAIN_JOURNAL)
            || (!journal && kind != ffi::SQLITE_OPEN_MAIN_DB)
            || (!state.writable
                && flags
                    & (ffi::SQLITE_OPEN_READWRITE
                        | ffi::SQLITE_OPEN_CREATE
                        | ffi::SQLITE_OPEN_DELETEONCLOSE)
                    != 0)
            || (!journal && flags & ffi::SQLITE_OPEN_CREATE != 0 && !state.creating)
        {
            return ffi::SQLITE_CANTOPEN;
        }
        if journal && state.location.admit_journal().is_err() {
            return ffi::SQLITE_CANTOPEN;
        }
        let real = file
            .cast::<u8>()
            .add(std::mem::size_of::<GuardFile>())
            .cast::<ffi::sqlite3_file>();
        let Some(callback) = (*state.base).xOpen else {
            return ffi::SQLITE_CANTOPEN;
        };
        let result = callback(
            state.base,
            name,
            real,
            flags | ffi::SQLITE_OPEN_NOFOLLOW,
            output,
        );
        if result != ffi::SQLITE_OK {
            if !(*real).pMethods.is_null() {
                if let Some(close) = (*(*real).pMethods).xClose {
                    close(real);
                }
                (*real).pMethods = ptr::null();
            }
            return result;
        }
        if (*real).pMethods.is_null() {
            return ffi::SQLITE_CANTOPEN;
        }
        let methods = &*(*real).pMethods;
        if methods.xClose.is_none()
            || methods.xRead.is_none()
            || methods.xWrite.is_none()
            || methods.xTruncate.is_none()
            || methods.xSync.is_none()
            || methods.xFileSize.is_none()
            || methods.xLock.is_none()
            || methods.xUnlock.is_none()
            || methods.xCheckReservedLock.is_none()
            || methods.xFileControl.is_none()
            || methods.xSectorSize.is_none()
            || methods.xDeviceCharacteristics.is_none()
        {
            if let Some(close) = methods.xClose {
                close(real);
            }
            return ffi::SQLITE_CANTOPEN;
        }
        let admitted = if journal {
            state.location.check_journal(false)
        } else {
            state.location.check(false)
        };
        if admitted.is_err() {
            if let Some(close) = (*(*real).pMethods).xClose {
                close(real);
            }
            return ffi::SQLITE_CANTOPEN;
        }
        ptr::write(
            file.cast::<GuardFile>(),
            GuardFile {
                header: ffi::sqlite3_file { pMethods: &METHODS },
                real,
                maximum: if journal {
                    MAX_JOURNAL_BYTES as i64
                } else {
                    MAX_DATABASE_BYTES as i64
                },
            },
        );
        ffi::SQLITE_OK
    }
}

unsafe extern "C" fn delete(vfs: *mut ffi::sqlite3_vfs, name: *const c_char, sync: c_int) -> c_int {
    unsafe {
        let state = state(vfs);
        if !state.writable || allowed(state, name, true) != Some(true) {
            return ffi::SQLITE_IOERR_DELETE;
        }
        let Some(callback) = (*state.base).xDelete else {
            return ffi::SQLITE_IOERR_DELETE;
        };
        let result = callback(state.base, name, sync);
        if result == ffi::SQLITE_OK || state.location.journal_absent() {
            state.location.journal_deleted();
        }
        result
    }
}

unsafe extern "C" fn access(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    flags: c_int,
    output: *mut c_int,
) -> c_int {
    unsafe {
        let state = state(vfs);
        if allowed(state, name, true).is_none() {
            if !name.is_null() {
                let name = CStr::from_ptr(name).to_bytes();
                if name.strip_suffix(b"-wal") == Some(state.main.as_slice())
                    && state.location.absent_wal().is_ok()
                {
                    *output = 0;
                    return ffi::SQLITE_OK;
                }
            }
            return ffi::SQLITE_IOERR_ACCESS;
        }
        match (*state.base).xAccess {
            Some(callback) => callback(state.base, name, flags, output),
            None => ffi::SQLITE_IOERR_ACCESS,
        }
    }
}

unsafe extern "C" fn full_path(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    maximum: c_int,
    output: *mut c_char,
) -> c_int {
    unsafe {
        let state = state(vfs);
        if name.is_null()
            || CStr::from_ptr(name).to_bytes() != state.main
            || maximum <= state.main.len() as c_int
        {
            return ffi::SQLITE_CANTOPEN;
        }
        ptr::copy_nonoverlapping(name, output, state.main.len() + 1);
        ffi::SQLITE_OK
    }
}

unsafe extern "C" fn randomness(
    vfs: *mut ffi::sqlite3_vfs,
    size: c_int,
    output: *mut c_char,
) -> c_int {
    unsafe {
        let base = state(vfs).base;
        (*base)
            .xRandomness
            .map_or(0, |callback| callback(base, size, output))
    }
}
unsafe extern "C" fn sleep(vfs: *mut ffi::sqlite3_vfs, micros: c_int) -> c_int {
    unsafe {
        let base = state(vfs).base;
        (*base).xSleep.map_or(0, |callback| callback(base, micros))
    }
}
unsafe extern "C" fn current_time(vfs: *mut ffi::sqlite3_vfs, output: *mut f64) -> c_int {
    unsafe {
        let base = state(vfs).base;
        (*base)
            .xCurrentTime
            .map_or(ffi::SQLITE_IOERR, |callback| callback(base, output))
    }
}
unsafe extern "C" fn current_time64(vfs: *mut ffi::sqlite3_vfs, output: *mut i64) -> c_int {
    unsafe {
        let base = state(vfs).base;
        (*base)
            .xCurrentTimeInt64
            .map_or(ffi::SQLITE_IOERR, |callback| callback(base, output))
    }
}
unsafe extern "C" fn last_error(
    vfs: *mut ffi::sqlite3_vfs,
    size: c_int,
    output: *mut c_char,
) -> c_int {
    unsafe {
        let base = state(vfs).base;
        (*base)
            .xGetLastError
            .map_or(0, |callback| callback(base, size, output))
    }
}

unsafe fn real(file: *mut ffi::sqlite3_file) -> *mut ffi::sqlite3_file {
    unsafe { (*file.cast::<GuardFile>()).real }
}
unsafe extern "C" fn close(file: *mut ffi::sqlite3_file) -> c_int {
    unsafe {
        let actual = real(file);
        let result = (*(*actual).pMethods).xClose.unwrap()(actual);
        (*file).pMethods = ptr::null();
        result
    }
}
unsafe extern "C" fn read(
    file: *mut ffi::sqlite3_file,
    data: *mut c_void,
    size: c_int,
    offset: i64,
) -> c_int {
    unsafe {
        let actual = real(file);
        (*(*actual).pMethods).xRead.unwrap()(actual, data, size, offset)
    }
}
unsafe extern "C" fn write(
    file: *mut ffi::sqlite3_file,
    data: *const c_void,
    size: c_int,
    offset: i64,
) -> c_int {
    unsafe {
        if size < 0
            || offset < 0
            || offset
                .checked_add(size as i64)
                .is_none_or(|end| end > (*file.cast::<GuardFile>()).maximum)
        {
            return ffi::SQLITE_FULL;
        }
        let actual = real(file);
        (*(*actual).pMethods).xWrite.unwrap()(actual, data, size, offset)
    }
}
unsafe extern "C" fn truncate(file: *mut ffi::sqlite3_file, size: i64) -> c_int {
    unsafe {
        if size < 0 || size > (*file.cast::<GuardFile>()).maximum {
            return ffi::SQLITE_FULL;
        }
        let actual = real(file);
        (*(*actual).pMethods).xTruncate.unwrap()(actual, size)
    }
}
unsafe extern "C" fn sync(file: *mut ffi::sqlite3_file, flags: c_int) -> c_int {
    unsafe {
        let actual = real(file);
        (*(*actual).pMethods).xSync.unwrap()(actual, flags)
    }
}
unsafe extern "C" fn size(file: *mut ffi::sqlite3_file, output: *mut i64) -> c_int {
    unsafe {
        let actual = real(file);
        (*(*actual).pMethods).xFileSize.unwrap()(actual, output)
    }
}
unsafe extern "C" fn lock(file: *mut ffi::sqlite3_file, level: c_int) -> c_int {
    unsafe {
        let actual = real(file);
        (*(*actual).pMethods).xLock.unwrap()(actual, level)
    }
}
unsafe extern "C" fn unlock(file: *mut ffi::sqlite3_file, level: c_int) -> c_int {
    unsafe {
        let actual = real(file);
        (*(*actual).pMethods).xUnlock.unwrap()(actual, level)
    }
}
unsafe extern "C" fn reserved(file: *mut ffi::sqlite3_file, output: *mut c_int) -> c_int {
    unsafe {
        let actual = real(file);
        (*(*actual).pMethods).xCheckReservedLock.unwrap()(actual, output)
    }
}
unsafe extern "C" fn control(
    file: *mut ffi::sqlite3_file,
    operation: c_int,
    argument: *mut c_void,
) -> c_int {
    unsafe {
        let actual = real(file);
        (*(*actual).pMethods).xFileControl.unwrap()(actual, operation, argument)
    }
}
unsafe extern "C" fn sector(file: *mut ffi::sqlite3_file) -> c_int {
    unsafe {
        let actual = real(file);
        (*(*actual).pMethods).xSectorSize.unwrap()(actual)
    }
}
unsafe extern "C" fn characteristics(file: *mut ffi::sqlite3_file) -> c_int {
    unsafe {
        let actual = real(file);
        (*(*actual).pMethods).xDeviceCharacteristics.unwrap()(actual)
    }
}

static METHODS: ffi::sqlite3_io_methods = ffi::sqlite3_io_methods {
    iVersion: 1,
    xClose: Some(close),
    xRead: Some(read),
    xWrite: Some(write),
    xTruncate: Some(truncate),
    xSync: Some(sync),
    xFileSize: Some(size),
    xLock: Some(lock),
    xUnlock: Some(unlock),
    xCheckReservedLock: Some(reserved),
    xFileControl: Some(control),
    xSectorSize: Some(sector),
    xDeviceCharacteristics: Some(characteristics),
    xShmMap: None,
    xShmLock: None,
    xShmBarrier: None,
    xShmUnmap: None,
    xFetch: None,
    xUnfetch: None,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fault_vfs::{FaultVfs, Operation, Scope};

    #[test]
    fn journal_unlinked_before_directory_sync_error_does_not_poison_namespace() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("late-delete.spil");
        let location = Location::attach(&path, true).unwrap();
        let scope = Scope::new();
        let base_name = unsafe {
            CStr::from_ptr((*ffi::sqlite3_vfs_find(ptr::null())).zName)
                .to_str()
                .unwrap()
        };
        let fault = FaultVfs::for_current_thread(base_name).unwrap().unwrap();
        let mut guard = Guard::new(location.clone(), true, true).unwrap();
        let fault_name = CString::new(fault.name()).unwrap();
        guard._state.base = unsafe { ffi::sqlite3_vfs_find(fault_name.as_ptr()) };
        guard.registration.szOsFile = unsafe {
            (std::mem::size_of::<GuardFile>() + (*guard._state.base).szOsFile as usize) as c_int
        };
        let connection = rusqlite::Connection::open_with_flags_and_vfs(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_CREATE,
            guard.name(),
        )
        .unwrap();
        connection
            .execute_batch("PRAGMA journal_mode=DELETE; PRAGMA synchronous=EXTRA; CREATE TABLE value(body TEXT); INSERT INTO value VALUES('old');")
            .unwrap();
        scope.arm(Some((Operation::DeleteAfter, 1)));
        assert!(
            connection
                .execute_batch("BEGIN IMMEDIATE; UPDATE value SET body='new'; COMMIT;")
                .is_err()
        );
        assert!(scope.fired());
        assert!(location.journal_absent());
        assert_eq!(
            connection
                .query_row("SELECT body FROM value", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "new"
        );
        scope.disarm();
        connection
            .execute_batch("BEGIN IMMEDIATE; UPDATE value SET body='next'; COMMIT;")
            .unwrap();
        location.check_journal(true).unwrap();
        drop(connection);
        drop(guard);
        drop(fault);
    }
}
