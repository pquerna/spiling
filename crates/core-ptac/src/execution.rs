// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! A scoped progress callback. Its boxed state is stable until unregistration.
//! Callers hold exclusive access to a live connection, keep it alive through Drop,
//! and never install a second execution scope on that connection. Capture owns its
//! scope before its database guard, and disables it before rollback/publication.

use rusqlite::{Connection, ffi};
use std::{
    ffi::{c_int, c_void},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

struct Budget<'a> {
    cancel: &'a AtomicBool,
    started: Instant,
    steps: u64,
    waited: Duration,
    committed_decision: bool,
}

pub(crate) struct Execution<'a> {
    handle: *mut ffi::sqlite3,
    _budget: Box<Budget<'a>>,
}

impl<'a> Execution<'a> {
    pub fn new(connection: &Connection, cancel: &'a AtomicBool) -> Self {
        let mut budget = Box::new(Budget {
            cancel,
            started: Instant::now(),
            steps: 0,
            waited: Duration::ZERO,
            committed_decision: false,
        });
        let handle = unsafe { connection.handle() };
        unsafe {
            ffi::sqlite3_busy_handler(handle, Some(busy), (&mut *budget as *mut Budget<'a>).cast());
            ffi::sqlite3_progress_handler(
                handle,
                1000,
                Some(progress),
                (&mut *budget as *mut Budget<'a>).cast(),
            );
        }
        Self {
            handle,
            _budget: budget,
        }
    }
    pub fn commit_phase(&mut self) {
        self._budget.committed_decision = true;
    }
    pub fn finish(&mut self) {
        if !self.handle.is_null() {
            unsafe {
                ffi::sqlite3_progress_handler(self.handle, 0, None, std::ptr::null_mut());
                ffi::sqlite3_busy_timeout(self.handle, 250);
            }
            self.handle = std::ptr::null_mut();
        }
    }
}

impl Drop for Execution<'_> {
    fn drop(&mut self) {
        self.finish();
    }
}

unsafe extern "C" fn progress(pointer: *mut c_void) -> c_int {
    let budget = unsafe { &mut *pointer.cast::<Budget<'_>>() };
    budget.steps += 1000;
    c_int::from(
        (!budget.committed_decision && budget.cancel.load(Ordering::Acquire))
            || budget.steps > 50_000_000
            || budget.started.elapsed() > Duration::from_secs(30),
    )
}

unsafe extern "C" fn busy(pointer: *mut c_void, _attempt: c_int) -> c_int {
    let budget = unsafe { &mut *pointer.cast::<Budget<'_>>() };
    if budget.waited >= Duration::from_millis(249)
        || (!budget.committed_decision && budget.cancel.load(Ordering::Acquire))
        || budget.started.elapsed() > Duration::from_secs(30)
    {
        return 0;
    }
    let started = Instant::now();
    std::thread::sleep(Duration::from_millis(1));
    budget.waited += started.elapsed();
    1
}
