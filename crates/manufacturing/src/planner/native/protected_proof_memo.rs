use spiling_geometry::{
    GeometryError, GeometryWorkBudget, PlanarCapsuleClearancePolicy, PreparedPlanarForbidden,
    certify_planar_capsule_clearance_prepared,
};
use std::{
    hash::{DefaultHasher, Hash, Hasher},
    mem::size_of,
    sync::atomic::AtomicBool,
};

const SLOTS: usize = 1024;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key([u64; 7]);

#[derive(Clone, Copy)]
struct Receipt {
    key: Key,
    work: u64,
}

impl Receipt {
    fn debit_completed_proof(
        self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        work.charge(1, cancel)?;
        work.charge(self.work - 1, &AtomicBool::new(false))
    }
}

pub(super) struct MemoizedForbidden<'a> {
    forbidden: PreparedPlanarForbidden<'a>,
    receipts: Vec<Option<Receipt>>,
}

impl<'a> MemoizedForbidden<'a> {
    pub(super) const MEMO_BYTES: usize =
        size_of::<Vec<Option<Receipt>>>() + SLOTS * size_of::<Option<Receipt>>();

    pub(super) fn new(forbidden: PreparedPlanarForbidden<'a>, allowance: usize) -> Self {
        let receipts = Self::reserve(allowance).unwrap_or_default();
        Self {
            forbidden,
            receipts,
        }
    }

    fn reserve(allowance: usize) -> Option<Vec<Option<Receipt>>> {
        if allowance < Self::MEMO_BYTES {
            return None;
        }
        let mut receipts = Vec::new();
        receipts.try_reserve_exact(SLOTS).ok()?;
        if size_of::<Vec<Option<Receipt>>>() + receipts.capacity() * size_of::<Option<Receipt>>()
            > allowance
        {
            return None;
        }
        receipts.resize(SLOTS, None);
        Some(receipts)
    }

    pub(super) fn certify(
        &mut self,
        segment: [[f64; 2]; 2],
        policy: PlanarCapsuleClearancePolicy,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        if self.receipts.is_empty() {
            return certify_planar_capsule_clearance_prepared(
                segment,
                &self.forbidden,
                policy,
                work,
                cancel,
            )
            .map(|_| false);
        }
        let key = Key([
            segment[0][0].to_bits(),
            segment[0][1].to_bits(),
            segment[1][0].to_bits(),
            segment[1][1].to_bits(),
            policy.radius_mm.to_bits(),
            policy.required_gap_mm.to_bits(),
            policy.endpoint_error_mm.to_bits(),
        ]);
        let mut hash = DefaultHasher::new();
        key.hash(&mut hash);
        let slot = &mut self.receipts[hash.finish() as usize % SLOTS];
        if let Some(receipt) = slot
            && receipt.key == key
            && receipt.work <= work.remaining()
        {
            receipt.debit_completed_proof(work, cancel)?;
            return Ok(true);
        }

        let before = work.consumed();
        certify_planar_capsule_clearance_prepared(segment, &self.forbidden, policy, work, cancel)?;
        let after = work.consumed();
        let charged = after.saturating_sub(before);
        if charged > 1 && after != u64::MAX {
            *slot = Some(Receipt { key, work: charged });
        }
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiling_geometry::{SectionLoopMm, prepare_planar_forbidden};

    const SEGMENT: [[f64; 2]; 2] = [[2.0, 3.0], [4.0, 3.0]];
    const POLICY: PlanarCapsuleClearancePolicy = PlanarCapsuleClearancePolicy {
        radius_mm: 0.125,
        required_gap_mm: 0.25,
        endpoint_error_mm: 0.0,
    };

    fn rectangle(lower: f64, upper: f64, is_hole: bool) -> SectionLoopMm {
        SectionLoopMm {
            is_hole,
            points_mm: vec![
                [lower, lower, 0.0],
                [upper, lower, 0.0],
                [upper, upper, 0.0],
                [lower, upper, 0.0],
                [lower, lower, 0.0],
            ],
        }
    }

    fn memo<'a>(rings: &'a [SectionLoopMm], bytes: usize) -> MemoizedForbidden<'a> {
        let mut work = GeometryWorkBudget::compile();
        let cancel = AtomicBool::new(false);
        MemoizedForbidden::new(
            prepare_planar_forbidden(rings, &mut work, &cancel).unwrap(),
            bytes,
        )
    }

    #[test]
    fn protected_memo_exact_charge_or_original_partial_refusal() {
        let rings = [rectangle(0.0, 6.0, false), rectangle(1.0, 5.0, true)];
        let mut cached = memo(&rings, MemoizedForbidden::MEMO_BYTES);
        let mut original = memo(&rings, MemoizedForbidden::MEMO_BYTES - 1);
        let cancel = AtomicBool::new(false);
        let mut seed = GeometryWorkBudget::compile();
        assert!(!cached.certify(SEGMENT, POLICY, &mut seed, &cancel).unwrap());
        let exact = seed.consumed();
        assert!(exact > 1);

        let mut enough = GeometryWorkBudget::new(exact);
        assert!(
            cached
                .certify(SEGMENT, POLICY, &mut enough, &cancel)
                .unwrap()
        );
        assert_eq!(enough.consumed(), exact);

        let mut tight_cached = GeometryWorkBudget::new(exact - 1);
        let mut tight_original = GeometryWorkBudget::new(exact - 1);
        let cached_error = cached
            .certify(SEGMENT, POLICY, &mut tight_cached, &cancel)
            .unwrap_err();
        let original_error = original
            .certify(SEGMENT, POLICY, &mut tight_original, &cancel)
            .unwrap_err();
        assert_eq!(
            (cached_error.code, cached_error.message),
            (original_error.code, original_error.message)
        );
        assert_eq!(tight_cached.consumed(), tight_original.consumed());
        assert_eq!(tight_cached.consumed(), exact - 1);

        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        let mut cancelled = GeometryWorkBudget::new(exact);
        assert_eq!(
            cached
                .certify(SEGMENT, POLICY, &mut cancelled, &cancel)
                .unwrap_err()
                .code,
            spiling_contracts::geometry::GeometryErrorCode::Cancelled
        );
        assert_eq!(cancelled.consumed(), 1);
    }

    #[test]
    fn protected_memo_keeps_direction_policy_and_mask_distinct() {
        let rings = [rectangle(0.0, 6.0, false), rectangle(1.0, 5.0, true)];
        let mut cached = memo(&rings, MemoizedForbidden::MEMO_BYTES);
        let mut work = GeometryWorkBudget::compile();
        let cancel = AtomicBool::new(false);
        assert!(!cached.certify(SEGMENT, POLICY, &mut work, &cancel).unwrap());
        assert!(cached.certify(SEGMENT, POLICY, &mut work, &cancel).unwrap());
        assert!(
            !cached
                .certify([SEGMENT[1], SEGMENT[0]], POLICY, &mut work, &cancel)
                .unwrap()
        );
        for adjusted in [
            PlanarCapsuleClearancePolicy {
                radius_mm: POLICY.radius_mm.next_up(),
                ..POLICY
            },
            PlanarCapsuleClearancePolicy {
                required_gap_mm: POLICY.required_gap_mm.next_up(),
                ..POLICY
            },
            PlanarCapsuleClearancePolicy {
                endpoint_error_mm: 0.000001,
                ..POLICY
            },
        ] {
            assert!(
                !cached
                    .certify(SEGMENT, adjusted, &mut work, &cancel)
                    .unwrap()
            );
        }
        let filled = [rectangle(0.0, 6.0, false)];
        let mut not_shared = memo(&filled, MemoizedForbidden::MEMO_BYTES);
        for _ in 0..2 {
            assert_eq!(
                not_shared
                    .certify(SEGMENT, POLICY, &mut work, &cancel)
                    .unwrap_err()
                    .code,
                spiling_contracts::geometry::GeometryErrorCode::AnalysisIndeterminate
            );
        }
    }
}
