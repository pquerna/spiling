// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Actual object bead edges in XY. A nominal scope's support clearance is
//! its deposited bead capsules, not its section material: material the walls
//! omit as unprintable deposits nothing. This grants no anchor, material or
//! collision authority: final actual capsule proofs stay independent.

use crate::check_cancel;
use spiling_contracts::manufacturing::{DepositionPath, ManufacturingError};
use std::sync::atomic::AtomicBool;

/// One actual object bead edge in XY with its nominal half width.
#[derive(Clone, Copy, Debug)]
pub(super) struct BeadEdge {
    pub(super) a: [f64; 2],
    pub(super) b: [f64; 2],
    pub(super) half: f64,
}

/// Every object bead edge of `objects` in object order, each charged once.
pub(super) fn visit_object_beads<'a>(
    objects: impl Iterator<Item = &'a DepositionPath>,
    work: &mut usize,
    cancel: &AtomicBool,
    mut visit: impl FnMut(BeadEdge) -> Result<(), ManufacturingError>,
) -> Result<(), ManufacturingError> {
    for path in objects {
        check_cancel(cancel)?;
        if super::support_role(path.kind) || path.points_mm.len() < 2 {
            continue;
        }
        let Ok(edges) = path.checked_edges() else {
            continue;
        };
        for edge in edges {
            super::super::native::charge_support_clearance(work, cancel)?;
            let &[p, q] = edge.points_mm else {
                continue;
            };
            visit(BeadEdge {
                a: [p[0], p[1]],
                b: [q[0], q[1]],
                half: edge.width_mm * 0.5,
            })?;
        }
    }
    Ok(())
}

/// Order-sensitive digest of bead edges: the same edges in the same order,
/// bit for bit, give the same value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BeadDigest(u64);

impl Default for BeadDigest {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl BeadDigest {
    pub(super) fn add(&mut self, edge: &BeadEdge) {
        for word in [edge.a[0], edge.a[1], edge.b[0], edge.b[1], edge.half].map(f64::to_bits) {
            self.0 = (self.0 ^ word).wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiling_contracts::manufacturing::{DepositionKind, WidthProfile};

    fn path(kind: DepositionKind, points: &[[f64; 2]]) -> DepositionPath {
        DepositionPath {
            material_domain: None,
            kind,
            owner: None,
            width: WidthProfile::Constant { width_mm: 0.4 },
            physical_z_mm: [0.0, 0.2],
            points_mm: points.iter().map(|&[x, y]| [x, y, 0.2]).collect(),
            arcs: Vec::new(),
            exposed: false,
            connectors: Vec::new(),
        }
    }

    fn digest(paths: &[DepositionPath]) -> (Vec<[f64; 2]>, BeadDigest) {
        let cancel = AtomicBool::new(false);
        let (mut starts, mut digest, mut work) = (Vec::new(), BeadDigest::default(), 0);
        visit_object_beads(paths.iter(), &mut work, &cancel, |bead| {
            assert_eq!(bead.half, 0.2);
            starts.push(bead.a);
            digest.add(&bead);
            Ok(())
        })
        .unwrap();
        (starts, digest)
    }

    #[test]
    fn object_beads_skip_support_and_digest_their_exact_edges() {
        let wall = path(
            DepositionKind::Perimeter,
            &[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0]],
        );
        let support = path(DepositionKind::Support, &[[5.0, 5.0], [6.0, 5.0]]);
        let (starts, before) = digest(&[wall.clone(), support]);
        assert_eq!(starts, vec![[0.0, 0.0], [1.0, 0.0]]);
        let mut moved = wall;
        moved.points_mm[2][1] = 1.0f64.next_up();
        assert_ne!(digest(&[moved]).1, before);
    }
}
