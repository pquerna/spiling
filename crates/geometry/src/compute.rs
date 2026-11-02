// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Shared default resource envelopes for native geometry work.
//!
//! Byte limits protect simultaneously live native storage and allocator
//! admission. Ordinary work is counted and cancellable without a unit cap;
//! callers can explicitly select deterministic exhaustion. These are not a
//! responsiveness target, a geometric tolerance or an optimization claim. A
//! refusal means the selected envelope was exhausted, not that the source is
//! unsupported or invalid.
//!
//! Work units are cumulative per operation and are charged from one guard for
//! the whole operation. Each byte limit is a simultaneous-live bound for the
//! owner named by that constant, not an additive process-RSS promise. Existing
//! borrowed-work and byte-limit entry points permit caller limits;
//! convenience entry points select monitored work and bounded bytes. Deliberate
//! refusal, overflow and cancellation tests keep their narrow selectors; only
//! ordinary positive work uses these defaults.

mod boxes;
pub(crate) use boxes::pairs as box_pairs;

/// One prepared-section pool: retained native section operands and their owned
/// query scratch. Import proofs and directional scratch keep their separate
/// owners below.
pub const MAX_PREPARED_SECTION_BYTES: usize = 512 * 1024 * 1024;
pub const MAX_PREPARED_SECTION_SOURCE_BYTES: usize = MAX_PREPARED_SECTION_BYTES - 4 * 1024 * 1024;

/// Import-time query proof retained per admitted definition.
pub const MAX_QUERY_PROOF_BYTES: usize = 256 * 1024 * 1024;

/// Combined unposed directional owner/query allocation, including retained
/// query proof rather than granting it a separate full allowance.
pub const MAX_DIRECTIONAL_SCRATCH_BYTES: usize = 256 * 1024 * 1024;

/// One display candidate's retained proof workspace. The final packed mesh is
/// charged separately against the caller's scene budget.
pub const DISPLAY_PROOF_BYTES: usize = 256 * 1024 * 1024;

pub(crate) fn sort_unstable_by<T>(
    entries: &mut [T],
    compare: impl Fn(&T, &T) -> std::cmp::Ordering,
    work: &crate::GeometryWorkBudget,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(), spiling_contracts::geometry::GeometryError> {
    fn sift<T>(
        entries: &mut [T],
        compare: &impl Fn(&T, &T) -> std::cmp::Ordering,
        mut root: usize,
        end: usize,
        work: &crate::GeometryWorkBudget,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<(), spiling_contracts::geometry::GeometryError> {
        while root < end / 2 {
            work.charge(3, cancel)?;
            let mut child = 2 * root + 1;
            if child + 1 < end && compare(&entries[child], &entries[child + 1]).is_lt() {
                child += 1;
            }
            if !compare(&entries[root], &entries[child]).is_lt() {
                break;
            }
            entries.swap(root, child);
            root = child;
        }
        Ok(())
    }
    work.charge(1, cancel)?;
    let count = entries.len();
    for root in (0..count / 2).rev() {
        sift(entries, &compare, root, count, work, cancel)?;
    }
    for end in (1..count).rev() {
        work.charge(1, cancel)?;
        entries.swap(0, end);
        sift(entries, &compare, 0, end, work, cancel)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_work_is_monitored_and_storage_remains_bounded() {
        assert_eq!(crate::GeometryWorkBudget::query().remaining(), u64::MAX);
        assert_eq!(crate::GeometryWorkBudget::compile().remaining(), u64::MAX);
        assert!(MAX_PREPARED_SECTION_SOURCE_BYTES < MAX_PREPARED_SECTION_BYTES);
    }
}
