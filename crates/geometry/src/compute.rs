// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Shared default resource envelopes for native geometry work.
//!
//! These limits protect availability: they bound cumulative counted work,
//! simultaneously live native storage and allocator admission. They are not a
//! responsiveness target, a geometric tolerance or an optimization claim. A
//! refusal means the selected envelope was exhausted, not that the source is
//! unsupported or invalid.
//!
//! Work units are cumulative per operation and are charged from one guard for
//! the whole operation. Each byte limit is a simultaneous-live bound for the
//! owner named by that constant, not an additive process-RSS promise. Callers
//! may always narrow an allowance with an explicit smaller value. Deliberate
//! refusal, overflow and cancellation tests keep their narrow selectors; only
//! ordinary positive work uses these defaults.

/// Cumulative work for one explicit interactive geometry measurement or
/// section query, including its native preparation.
pub const QUERY_WORK_UNITS: u64 = 1_000_000_000;

/// Cumulative work for one complete manufacturing compile. A compile retains
/// one guard across preflight, height analysis, composition, sections, offsets
/// and support obligations instead of resetting a query allowance per stage.
pub const COMPILE_WORK_UNITS: u64 = 8_000_000_000;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_budgets_use_the_shared_envelopes() {
        assert_eq!(
            crate::GeometryWorkBudget::query().remaining(),
            QUERY_WORK_UNITS
        );
        assert_eq!(
            crate::GeometryWorkBudget::compile().remaining(),
            COMPILE_WORK_UNITS
        );
        assert!(COMPILE_WORK_UNITS > QUERY_WORK_UNITS);
        assert!(MAX_PREPARED_SECTION_SOURCE_BYTES < MAX_PREPARED_SECTION_BYTES);
    }
}
