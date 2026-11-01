// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Test-only, one-shot indexed failures layered over the production namespace VFS.
//!
//! Safety: registration, state and the wrapped registration outlive every connection.
//! SQLite owns the aligned file allocation; its trailing storage belongs exclusively
//! to the wrapped VFS. Callbacks forward to that VFS using its original state, never
//! unwind, and do not alter global default VFS registration or syscall tables.

use crate::{Result, invalid};
use rusqlite::ffi;
use std::{
    cell::RefCell,
    ffi::{CString, c_char, c_int, c_void},
    ptr,
    sync::{Arc, Mutex},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Operation {
    Write,
    Sync,
    Truncate,
    Delete,
    Lock,
    DeleteAfter,
}

impl Operation {
    pub const ALL: [Self; 6] = [
        Self::Write,
        Self::Sync,
        Self::Truncate,
        Self::Delete,
        Self::Lock,
        Self::DeleteAfter,
    ];

    fn error(self) -> c_int {
        match self {
            Self::Write => ffi::SQLITE_IOERR_WRITE,
            Self::Sync => ffi::SQLITE_IOERR_FSYNC,
            Self::Truncate => ffi::SQLITE_IOERR_TRUNCATE,
            Self::Delete => ffi::SQLITE_IOERR_DELETE,
            Self::Lock => ffi::SQLITE_IOERR_LOCK,
            Self::DeleteAfter => ffi::SQLITE_IOERR_DIR_FSYNC,
        }
    }
}

#[derive(Default)]
struct Plan {
    counts: [usize; 6],
    target: Option<(Operation, usize)>,
    fired: bool,
}

#[derive(Default)]
struct Control(Mutex<Plan>);

impl Control {
    fn fail(&self, operation: Operation) -> bool {
        let mut plan = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        plan.counts[operation as usize] += 1;
        if !plan.fired && plan.target == Some((operation, plan.counts[operation as usize])) {
            plan.fired = true;
            true
        } else {
            false
        }
    }
}

thread_local! {
    static ACTIVE: RefCell<Option<Arc<Control>>> = const { RefCell::new(None) };
}

pub(crate) struct Scope {
    control: Arc<Control>,
    _thread: std::marker::PhantomData<std::rc::Rc<()>>,
}

impl Scope {
    pub fn new() -> Self {
        let control = Arc::new(Control::default());
        ACTIVE.with(|active| {
            assert!(active.borrow().is_none(), "nested fault VFS scope");
            *active.borrow_mut() = Some(control.clone());
        });
        Self {
            control,
            _thread: std::marker::PhantomData,
        }
    }

    pub fn arm(&self, target: Option<(Operation, usize)>) {
        assert!(target.is_none_or(|(_, index)| index > 0));
        *self.control.0.lock().unwrap() = Plan {
            target,
            ..Plan::default()
        };
    }

    pub fn count(&self, operation: Operation) -> usize {
        self.control.0.lock().unwrap().counts[operation as usize]
    }

    pub fn fired(&self) -> bool {
        self.control.0.lock().unwrap().fired
    }

    pub fn disarm(&self) {
        self.control.0.lock().unwrap().target = None;
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        self.disarm();
        ACTIVE.with(|active| *active.borrow_mut() = None);
    }
}

struct State {
    base: *mut ffi::sqlite3_vfs,
    control: Arc<Control>,
}

pub(crate) struct FaultVfs {
    registration: Box<ffi::sqlite3_vfs>,
    _state: Box<State>,
    name: CString,
}

unsafe impl Send for FaultVfs {}

impl FaultVfs {
    pub fn for_current_thread(base_name: &str) -> Result<Option<Self>> {
        let control = ACTIVE.with(|active| active.borrow().clone());
        control
            .map(|control| Self::new(base_name, control))
            .transpose()
    }

    fn new(base_name: &str, control: Arc<Control>) -> Result<Self> {
        let base_name = CString::new(base_name).map_err(|_| invalid("fault VFS base name"))?;
        let name = CString::new(format!("spiling-test-fault-{}", uuid::Uuid::new_v4())).unwrap();
        unsafe {
            let base = ffi::sqlite3_vfs_find(base_name.as_ptr());
            if base.is_null() || (*base).szOsFile < 0 {
                return Err(invalid("fault VFS base unavailable"));
            }
            let mut state = Box::new(State { base, control });
            let mut registration = Box::new(ffi::sqlite3_vfs {
                iVersion: 2,
                szOsFile: (std::mem::size_of::<File>() + (*base).szOsFile as usize) as c_int,
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
                return Err(invalid("fault VFS registration"));
            }
            Ok(Self {
                registration,
                _state: state,
                name,
            })
        }
    }

    pub fn name(&self) -> &str {
        self.name.to_str().unwrap()
    }
}

impl Drop for FaultVfs {
    fn drop(&mut self) {
        unsafe { ffi::sqlite3_vfs_unregister(&mut *self.registration) };
    }
}

#[repr(C)]
struct File {
    header: ffi::sqlite3_file,
    real: *mut ffi::sqlite3_file,
    state: *const State,
}

unsafe fn state<'state>(vfs: *mut ffi::sqlite3_vfs) -> &'state State {
    unsafe { &*((*vfs).pAppData.cast::<State>()) }
}

unsafe fn file<'file>(file: *mut ffi::sqlite3_file) -> &'file File {
    unsafe { &*file.cast::<File>() }
}

unsafe fn fail(file: *mut ffi::sqlite3_file, operation: Operation) -> bool {
    unsafe { (*self::file(file).state).control.fail(operation) }
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
        let real = file
            .cast::<u8>()
            .add(std::mem::size_of::<File>())
            .cast::<ffi::sqlite3_file>();
        ptr::write_bytes(real.cast::<u8>(), 0, (*state.base).szOsFile as usize);
        ptr::write(
            file.cast::<File>(),
            File {
                header: ffi::sqlite3_file {
                    pMethods: ptr::null(),
                },
                real,
                state,
            },
        );
        let result = (*state.base).xOpen.unwrap()(state.base, name, real, flags, output);
        if !(*real).pMethods.is_null() {
            (*file).pMethods = &METHODS;
        }
        result
    }
}

unsafe extern "C" fn delete(vfs: *mut ffi::sqlite3_vfs, name: *const c_char, sync: c_int) -> c_int {
    unsafe {
        let state = state(vfs);
        if state.control.fail(Operation::Delete) {
            Operation::Delete.error()
        } else {
            let result = (*state.base).xDelete.unwrap()(state.base, name, sync);
            if result == ffi::SQLITE_OK && state.control.fail(Operation::DeleteAfter) {
                Operation::DeleteAfter.error()
            } else {
                result
            }
        }
    }
}

unsafe extern "C" fn access(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    flags: c_int,
    output: *mut c_int,
) -> c_int {
    unsafe {
        let base = state(vfs).base;
        (*base).xAccess.unwrap()(base, name, flags, output)
    }
}

unsafe extern "C" fn full_path(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    size: c_int,
    output: *mut c_char,
) -> c_int {
    unsafe {
        let base = state(vfs).base;
        (*base).xFullPathname.unwrap()(base, name, size, output)
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

unsafe extern "C" fn close(file: *mut ffi::sqlite3_file) -> c_int {
    unsafe {
        let real = self::file(file).real;
        let result = (*(*real).pMethods).xClose.unwrap()(real);
        (*file).pMethods = ptr::null();
        result
    }
}

unsafe extern "C" fn read(
    file: *mut ffi::sqlite3_file,
    output: *mut c_void,
    size: c_int,
    offset: i64,
) -> c_int {
    unsafe {
        let real = self::file(file).real;
        (*(*real).pMethods).xRead.unwrap()(real, output, size, offset)
    }
}

unsafe extern "C" fn write(
    file: *mut ffi::sqlite3_file,
    input: *const c_void,
    size: c_int,
    offset: i64,
) -> c_int {
    unsafe {
        if fail(file, Operation::Write) {
            return Operation::Write.error();
        }
        let real = self::file(file).real;
        (*(*real).pMethods).xWrite.unwrap()(real, input, size, offset)
    }
}

unsafe extern "C" fn truncate(file: *mut ffi::sqlite3_file, size: i64) -> c_int {
    unsafe {
        if fail(file, Operation::Truncate) {
            return Operation::Truncate.error();
        }
        let real = self::file(file).real;
        (*(*real).pMethods).xTruncate.unwrap()(real, size)
    }
}

unsafe extern "C" fn sync(file: *mut ffi::sqlite3_file, flags: c_int) -> c_int {
    unsafe {
        if fail(file, Operation::Sync) {
            return Operation::Sync.error();
        }
        let real = self::file(file).real;
        (*(*real).pMethods).xSync.unwrap()(real, flags)
    }
}

unsafe extern "C" fn size(file: *mut ffi::sqlite3_file, output: *mut i64) -> c_int {
    unsafe {
        let real = self::file(file).real;
        (*(*real).pMethods).xFileSize.unwrap()(real, output)
    }
}

unsafe extern "C" fn lock(file: *mut ffi::sqlite3_file, level: c_int) -> c_int {
    unsafe {
        if fail(file, Operation::Lock) {
            return Operation::Lock.error();
        }
        let real = self::file(file).real;
        (*(*real).pMethods).xLock.unwrap()(real, level)
    }
}

unsafe extern "C" fn unlock(file: *mut ffi::sqlite3_file, level: c_int) -> c_int {
    unsafe {
        let real = self::file(file).real;
        (*(*real).pMethods).xUnlock.unwrap()(real, level)
    }
}

unsafe extern "C" fn reserved(file: *mut ffi::sqlite3_file, output: *mut c_int) -> c_int {
    unsafe {
        let real = self::file(file).real;
        (*(*real).pMethods).xCheckReservedLock.unwrap()(real, output)
    }
}

unsafe extern "C" fn control(
    file: *mut ffi::sqlite3_file,
    operation: c_int,
    argument: *mut c_void,
) -> c_int {
    unsafe {
        let real = self::file(file).real;
        (*(*real).pMethods).xFileControl.unwrap()(real, operation, argument)
    }
}

unsafe extern "C" fn sector(file: *mut ffi::sqlite3_file) -> c_int {
    unsafe {
        let real = self::file(file).real;
        (*(*real).pMethods).xSectorSize.unwrap()(real)
    }
}

unsafe extern "C" fn characteristics(file: *mut ffi::sqlite3_file) -> c_int {
    unsafe {
        let real = self::file(file).real;
        (*(*real).pMethods).xDeviceCharacteristics.unwrap()(real)
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
