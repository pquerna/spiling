// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;
use sha2::{Digest, Sha256};

struct Entry {
    identity: [u8; 32],
    material: Rc<OwnedMaterial>,
}

pub(super) struct UnionMemo {
    entries: Vec<Entry>,
    limit: usize,
    units: usize,
    payload_bytes: usize,
    accounted: (usize, usize),
}

impl UnionMemo {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            entries: Vec::new(),
            limit,
            units: 0,
            payload_bytes: 0,
            accounted: (0, 0),
        }
    }

    pub(super) fn receipt(&self) -> (usize, usize) {
        (
            self.units,
            self.payload_bytes.saturating_add(vec_bytes(&self.entries)),
        )
    }

    pub(super) fn unaccounted(&self) -> (usize, usize) {
        let (units, bytes) = self.receipt();
        (units - self.accounted.0, bytes - self.accounted.1)
    }

    pub(super) fn account(&mut self, units: &mut usize, bytes: &mut usize) {
        let (extra_units, extra_bytes) = self.unaccounted();
        *units = units.saturating_add(extra_units);
        *bytes = bytes.saturating_add(extra_bytes);
        self.accounted = self.receipt();
    }

    pub(super) fn merge(
        &mut self,
        pieces: &MultiPolygon<f64>,
        retained: usize,
        owned_bytes: usize,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<(Rc<OwnedMaterial>, bool), ManufacturingError> {
        check_cancel(cancel)?;
        let mut digest = Sha256::new();
        digest.update(pieces.0.len().to_le_bytes());
        for polygon in &pieces.0 {
            check_cancel(cancel)?;
            digest.update(polygon.interiors().len().to_le_bytes());
            for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
                digest.update(ring.0.len().to_le_bytes());
                for point in &ring.0 {
                    super::super::native::charge_support_clearance(work, cancel)?;
                    digest.update(point.x.to_bits().to_le_bytes());
                    digest.update(point.y.to_bits().to_le_bytes());
                }
            }
        }
        let identity: [u8; 32] = digest.finalize().into();
        let index = match self
            .entries
            .binary_search_by_key(&identity, |entry| entry.identity)
        {
            Ok(index) => return Ok((Rc::clone(&self.entries[index].material), true)),
            Err(index) => index,
        };
        let material = OwnedMaterial::new(union_pieces(pieces));
        region::check_size(&material)?;
        check_cancel(cancel)?;
        let bytes = material.bytes.saturating_add(
            std::mem::size_of::<OwnedMaterial>() + 2 * std::mem::size_of::<usize>(),
        );
        let records = (self.entries.len() + 1)
            .max(self.entries.capacity())
            .saturating_mul(2 * std::mem::size_of::<Entry>());
        super::super::remaining_geometry_units(
            retained.saturating_add(material.units),
            owned_bytes.saturating_add(bytes),
        )?;
        let retain = self
            .payload_bytes
            .saturating_add(bytes)
            .saturating_add(records)
            <= self.limit
            && super::super::remaining_geometry_units(
                retained.saturating_add(material.units),
                owned_bytes.saturating_add(bytes).saturating_add(records),
            )
            .is_ok()
            && reserve_owned(&mut self.entries, 1, owned_bytes.saturating_add(bytes)).is_ok();
        let material = material.into_shared();
        if retain {
            self.units += material.units;
            self.payload_bytes += bytes;
            self.entries.insert(
                index,
                Entry {
                    identity,
                    material: Rc::clone(&material),
                },
            );
        }
        Ok((material, retain))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::polygon;

    #[test]
    fn memo_reuses_only_identical_operands_and_low_memory_keeps_the_same_union() {
        let cancel = AtomicBool::new(false);
        let mut pieces = MultiPolygon(vec![
            polygon![(x:0., y:0.), (x:4., y:0.), (x:4., y:4.), (x:0., y:4.)],
            polygon![(x:3., y:0.), (x:6., y:0.), (x:6., y:4.), (x:3., y:4.)],
        ]);
        let mut memo = UnionMemo::new(usize::MAX);
        let mut work = 0;
        let bound = 26.0 * super::super::super::OFFSET_TOLERANCE_MM;
        let (original, cached) = memo.merge(&pieces, 0, 0, &mut work, &cancel).unwrap();
        assert!(cached);
        assert!((original.unsigned_area() - 24.).abs() < bound);
        let (reused, _) = memo.merge(&pieces, 0, 0, &mut work, &cancel).unwrap();
        assert!(Rc::ptr_eq(&original, &reused));
        pieces.0[0].interiors_push(geo::LineString::from(vec![
            (1., 1.),
            (1., 3.),
            (2., 3.),
            (2., 1.),
            (1., 1.),
        ]));
        let (with_hole, _) = memo.merge(&pieces, 0, 0, &mut work, &cancel).unwrap();
        assert!(!Rc::ptr_eq(&original, &with_hole));
        assert!((with_hole.unsigned_area() - 22.).abs() < bound);
        let mut uncached = UnionMemo::new(0);
        let (plain, retained) = uncached.merge(&pieces, 0, 0, &mut work, &cancel).unwrap();
        assert!(!retained);
        assert_eq!(uncached.receipt(), (0, 0));
        assert!(plain.difference(&with_hole.material).unsigned_area() < bound);
        assert!(with_hole.difference(&plain.material).unsigned_area() < bound);
        let (mut units, mut bytes) = (0, 0);
        memo.account(&mut units, &mut bytes);
        assert_eq!((units, bytes), memo.receipt());
        assert_eq!(memo.unaccounted(), (0, 0));
        let failure = memo
            .merge(&pieces, 0, 0, &mut work, &AtomicBool::new(true))
            .err()
            .unwrap();
        assert_eq!(failure.code, ManufacturingErrorCode::Cancelled);
    }
}
