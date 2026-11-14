// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Host-selected live working-set admission for one manufacturing operation.
//!
//! The planner and verifier count their simultaneously live geometry, caches
//! and staging buffers and refuse with `ResourceLimit` before they would grow
//! past this allowance. The allowance is selected by the host from the
//! machine's actual memory headroom ([ADR009]); it is not a part-size rule.
//! Counts that grow with the job (layers, height events, segments, travel
//! and process events) are admitted against this allowance through
//! [`records`]; they have no fixed ceiling. Only binary representation
//! widths (u32 counts, bundle and program byte formats) stay in contracts.
//!
//! The allowance is installed once per public entry point (compile,
//! preflight, verification) and read by the counting sites through
//! [`bytes`] and [`units`], so one value governs the whole operation without
//! a per-site limit. Work handed to other threads must re-enter the
//! captured allowance with [`scope`]. Outside an operation scope no host
//! admission is selected: only arithmetic and allocation failure remain.
//!
//! [ADR009]: ../../../docs/adr/009-resource-envelope-policy.md
use spiling_contracts::manufacturing::{ManufacturingError, ManufacturingErrorCode};
use std::cell::Cell;

/// Bytes of simultaneously live manufacturing working set one operation may
/// admit. Geometry retention converts to coordinate-equivalent units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkingSet {
    bytes: usize,
}

impl WorkingSet {
    /// No host admission: the largest allocation Rust can express.
    pub const UNMEASURED: Self = Self {
        bytes: isize::MAX as usize,
    };

    /// A positive host-selected allowance.
    pub fn new(bytes: usize) -> Result<Self, ManufacturingError> {
        if bytes == 0 {
            return Err(ManufacturingError::new(
                ManufacturingErrorCode::ResourceLimit,
                "no manufacturing working-set memory is available",
            ));
        }
        Ok(Self {
            bytes: bytes.min(Self::UNMEASURED.bytes),
        })
    }

    pub fn bytes(self) -> usize {
        self.bytes
    }
}

thread_local! {
    static CURRENT: Cell<WorkingSet> = const { Cell::new(WorkingSet::UNMEASURED) };
}

/// The allowance governing the current operation.
pub(crate) fn current() -> WorkingSet {
    CURRENT.with(Cell::get)
}

/// Run `operation` under `working_set`, restoring the enclosing allowance
/// afterwards, including on unwind.
pub fn scope<R>(working_set: WorkingSet, operation: impl FnOnce() -> R) -> R {
    struct Restore(WorkingSet);
    impl Drop for Restore {
        fn drop(&mut self) {
            CURRENT.with(|current| current.set(self.0));
        }
    }
    let _restore = Restore(CURRENT.with(|current| current.replace(working_set)));
    operation()
}

/// Admitted live bytes.
pub(crate) fn bytes() -> usize {
    current().bytes
}

/// Admitted live bytes, for comparisons against `u64` totals.
pub(crate) fn bytes_u64() -> u64 {
    current().bytes as u64
}

/// Admitted coordinate-equivalent geometry units (including opaque proof
/// bytes), never the independent published-command ceilings.
pub(crate) fn units() -> usize {
    current().bytes / std::mem::size_of::<geo::Coord<f64>>()
}

/// Records of `T` the operation's allowance could hold at once: the
/// byte-derived bound for a count that grows with the job (layers, height
/// events, segments), never a fixed cap.
pub(crate) fn records<T>() -> usize {
    current().bytes / std::mem::size_of::<T>().max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_selects_and_restores_the_operation_allowance() {
        assert_eq!(current(), WorkingSet::UNMEASURED);
        let narrow = WorkingSet::new(1024).unwrap();
        scope(narrow, || {
            assert_eq!(bytes(), 1024);
            assert_eq!(units(), 1024 / 16);
            assert_eq!(records::<f64>(), 1024 / 8);
            let inner = WorkingSet::new(64).unwrap();
            scope(inner, || assert_eq!(bytes(), 64));
            assert_eq!(bytes(), 1024);
        });
        assert_eq!(current(), WorkingSet::UNMEASURED);
        assert!(WorkingSet::new(0).is_err());
        assert_eq!(WorkingSet::new(usize::MAX).unwrap(), WorkingSet::UNMEASURED);
    }
}
