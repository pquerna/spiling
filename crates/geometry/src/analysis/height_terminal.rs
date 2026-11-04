// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{GeometryWorkBudget, HeightCapture, HeightFunctional, ProjectedBounds};
use crate::GeometryError;
use solidcore::EdgeId;
use spiling_contracts::geometry::{DefinitionId, GeometryErrorCode, RigidPoseMm};
use std::sync::atomic::AtomicBool;

#[derive(Debug, Clone, Copy)]
pub(super) struct CurveRoot {
    pub event_index: usize,
    pub edge: EdgeId,
    pub original_range: [f64; 2],
    pub uniqueness_range: [f64; 2],
    pub second_sign: i8,
    pub root_exists: bool,
}

#[derive(Debug)]
pub(super) struct TerminalRoot {
    pub edge: EdgeId,
    pub original_range: [f64; 2],
    pub uniqueness_range: [f64; 2],
    pub height_bracket_mm: [f64; 2],
    pub event_brackets_mm: Vec<[f64; 2]>,
}

impl TerminalRoot {
    pub(super) fn owns_event(
        &self,
        bracket: [f64; 2],
        work: &GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        work.charge(
            self.event_brackets_mm.len().max(1).ilog2() as u64 + 2,
            cancel,
        )?;
        Ok(self
            .event_brackets_mm
            .binary_search_by(|old| {
                old[0]
                    .total_cmp(&bracket[0])
                    .then(old[1].total_cmp(&bracket[1]))
            })
            .is_ok())
    }
}

#[derive(Debug)]
pub(super) struct GlobalTerminalScope {
    pub definition_id: DefinitionId,
    pub material_realization: crate::kernel::MaterialRealizationIdentity,
    pub pose: RigidPoseMm,
    pub frame: crate::PlaneFrameMm,
    pub functional: HeightFunctional,
    pub height_interval_mm: [f64; 2],
    pub policy: [f64; 2],
    pub support_policy_mm: f64,
    pub support: ProjectedBounds,
}
impl GlobalTerminalScope {
    pub(super) fn matches(&self, capture: &HeightCapture) -> bool {
        self.definition_id == capture.definition_id
            && self.material_realization == capture.material_realization
            && Some(self.pose) == capture.pose
            && self.frame == capture.frame
            && self.functional == capture.functional
            && self.height_interval_mm == capture.height_interval_mm
            && self.policy
                == [
                    capture.policy.maximum_height_uncertainty_mm,
                    capture.policy.maximum_shape_error_mm,
                ]
            && self.support_policy_mm == self.policy[0].min(self.policy[1])
            && self.height_interval_mm[0] <= self.support.minimum[0]
            && self.height_interval_mm[1] >= self.support.maximum[1]
    }
}

/// A role proposal, never slab or corner authority. Resolve it before the raw
/// census loses event indices through exact-band deduplication. A shared height
/// band cannot identify a root; only the same original edge/restriction and a
/// connected strictly convex parameter neighborhood can own these records.
pub(super) fn lower_terminal_candidate(
    events: &[[f64; 2]],
    roots: &[CurveRoot],
    maximum_uncertainty: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<TerminalRoot>, GeometryError> {
    work.charge(1, cancel)?;
    if !maximum_uncertainty.is_finite() || maximum_uncertainty <= 0.0 {
        return Ok(None);
    }
    let mut seed_index = None;
    for (index, band) in events.iter().enumerate() {
        work.charge(1, cancel)?;
        if !band.iter().all(|value| value.is_finite()) || band[0] > band[1] {
            return Ok(None);
        }
        if seed_index.is_none_or(|old: usize| band[0] < events[old][0]) {
            seed_index = Some(index);
        }
    }
    let Some(seed_index) = seed_index else {
        return Ok(None);
    };
    let mut seed = None;
    let mut previous = None;
    for root in roots {
        work.charge(1, cancel)?;
        if previous.is_some_and(|old| old >= root.event_index)
            || root.event_index >= events.len()
            || !root
                .original_range
                .iter()
                .chain(root.uniqueness_range.iter())
                .all(|v| v.is_finite())
            || root.original_range[0] >= root.original_range[1]
            || root.uniqueness_range[0] < root.original_range[0]
            || root.uniqueness_range[1] > root.original_range[1]
            || root.uniqueness_range[0] >= root.uniqueness_range[1]
        {
            return Ok(None);
        }
        previous = Some(root.event_index);
        if root.event_index == seed_index {
            seed = Some(root);
        }
    }
    let Some(seed) = seed.filter(|root| root.second_sign == 1) else {
        return Ok(None);
    };
    let same_source = |root: &CurveRoot| {
        root.edge == seed.edge
            && root.original_range == seed.original_range
            && root.second_sign == 1
    };
    let mut range = seed.uniqueness_range;
    loop {
        let previous = range;
        for root in roots {
            work.charge(1, cancel)?;
            if same_source(root)
                && root.uniqueness_range[0] <= range[1]
                && range[0] <= root.uniqueness_range[1]
            {
                range[0] = range[0].min(root.uniqueness_range[0]);
                range[1] = range[1].max(root.uniqueness_range[1]);
            }
        }
        if range == previous {
            break;
        }
    }
    let member = |root: &CurveRoot| {
        same_source(root)
            && root.uniqueness_range[0] >= range[0]
            && root.uniqueness_range[1] <= range[1]
    };
    let mut exists = false;
    let mut height = [f64::INFINITY, f64::NEG_INFINITY];
    let mut count = 0usize;
    for root in roots {
        work.charge(1, cancel)?;
        if member(root) {
            let band = events[root.event_index];
            height[0] = height[0].min(band[0]);
            height[1] = height[1].max(band[1]);
            exists |= root.root_exists;
            count += 1;
        }
    }
    if !exists || height[0] > height[1] || height[1] - height[0] > maximum_uncertainty {
        return Ok(None);
    }
    let mut root_index = 0;
    for (event_index, band) in events.iter().enumerate() {
        work.charge(1, cancel)?;
        let owns = roots
            .get(root_index)
            .is_some_and(|root| root.event_index == event_index && member(root));
        if roots
            .get(root_index)
            .is_some_and(|root| root.event_index == event_index)
        {
            root_index += 1;
        }
        if !owns && band[0] <= height[1] {
            return Ok(None);
        }
    }
    let resource = || {
        GeometryError::new(
            GeometryErrorCode::ResourceLimit,
            "native terminal root provenance allocation refused",
        )
    };
    let mut bands = Vec::new();
    bands.try_reserve_exact(count).map_err(|_| resource())?;
    if bands.capacity() > events.len() {
        return Err(resource());
    }
    for root in roots {
        work.charge(1, cancel)?;
        if member(root) {
            bands.push(events[root.event_index]);
        }
    }
    crate::compute::sort_unstable_by(
        &mut bands,
        |left, right| {
            left[0]
                .total_cmp(&right[0])
                .then(left[1].total_cmp(&right[1]))
        },
        work,
        cancel,
    )?;
    work.charge(bands.len() as u64, cancel)?;
    bands.dedup();
    Ok(Some(TerminalRoot {
        edge: seed.edge,
        original_range: seed.original_range,
        uniqueness_range: range,
        height_bracket_mm: height,
        event_brackets_mm: bands,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge_pair() -> [EdgeId; 2] {
        let definition = crate::from_step_bytes_with_cancel(
            include_bytes!("../../../../fixtures/geometry/cylinder.step"),
            &AtomicBool::new(false),
        )
        .unwrap();
        let edges = definition.brep().edge_list();
        [edges[0], edges[1]]
    }

    fn root(event_index: usize, edge: EdgeId, range: [f64; 2]) -> CurveRoot {
        CurveRoot {
            event_index,
            edge,
            original_range: [0.0, 1.0],
            uniqueness_range: range,
            second_sign: 1,
            root_exists: true,
        }
    }

    #[test]
    fn terminal_identity_joins_only_same_edge_unique_parameter_root() {
        let [edge, other] = edge_pair();
        let events = [[0.0, 0.01], [0.002, 0.008], [0.005, 0.015], [1.0, 1.01]];
        let roots = [
            root(0, edge, [0.2, 0.4]),
            root(1, edge, [0.3, 0.5]),
            root(2, edge, [0.4, 0.6]),
            root(3, other, [0.2, 0.4]),
        ];
        let candidates = lower_terminal_candidate(
            &events,
            &roots,
            0.02,
            &mut GeometryWorkBudget::query(),
            &AtomicBool::new(false),
        )
        .unwrap();
        let lower = candidates.as_ref().unwrap();
        assert_eq!(lower.uniqueness_range, [0.2, 0.6]);
        assert_eq!(lower.event_brackets_mm.len(), 3);
        assert!(!lower.event_brackets_mm.contains(&events[3]));
    }

    #[test]
    fn terminal_identity_refuses_other_close_roots_unknown_events_and_missing_existence() {
        let [edge, other] = edge_pair();
        let events = [[0.0, 0.01], [0.005, 0.015]];
        for roots in [
            vec![root(0, edge, [0.2, 0.4]), root(1, other, [0.2, 0.4])],
            vec![root(0, edge, [0.2, 0.4]), root(1, edge, [0.6, 0.8])],
            vec![root(0, edge, [0.2, 0.4])],
        ] {
            let result = lower_terminal_candidate(
                &events,
                &roots,
                0.02,
                &mut GeometryWorkBudget::query(),
                &AtomicBool::new(false),
            )
            .unwrap();
            assert!(result.is_none());
        }
        let mut roots = [root(0, edge, [0.2, 0.4]), root(1, edge, [0.3, 0.5])];
        for root in &mut roots {
            root.root_exists = false;
        }
        assert!(
            lower_terminal_candidate(
                &events,
                &roots,
                0.02,
                &mut GeometryWorkBudget::query(),
                &AtomicBool::new(false)
            )
            .unwrap()
            .is_none()
        );
        roots[0].root_exists = true;
        roots[1].original_range = [0.0, 2.0];
        assert!(
            lower_terminal_candidate(
                &events,
                &roots,
                0.02,
                &mut GeometryWorkBudget::query(),
                &AtomicBool::new(false)
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn terminal_identity_preserves_distinct_neighbors_and_checks_every_raw_record() {
        let [edge, _] = edge_pair();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::query();
        let events = [[0.0, 1e-8], [2e-8, 3e-8]];
        let roots = [root(0, edge, [0.2, 0.3]), root(1, edge, [0.31, 0.4])];
        let lower = lower_terminal_candidate(&events, &roots, 1e-6, &mut work, &cancel)
            .unwrap()
            .unwrap();
        assert!(lower.owns_event(events[0], &work, &cancel).unwrap());
        assert!(!lower.owns_event(events[1], &work, &cancel).unwrap());
        assert_eq!(lower.uniqueness_range, roots[0].uniqueness_range);
        assert!(
            lower_terminal_candidate(
                &[events[0], events[0]],
                &roots[..1],
                1e-6,
                &mut work,
                &cancel,
            )
            .unwrap()
            .is_none()
        );
        for malformed in [
            vec![roots[1], roots[0]],
            vec![roots[0], roots[0]],
            vec![CurveRoot {
                event_index: events.len(),
                ..roots[0]
            }],
            vec![
                CurveRoot {
                    second_sign: 0,
                    ..roots[0]
                },
                roots[1],
            ],
            vec![
                CurveRoot {
                    root_exists: false,
                    ..roots[0]
                },
                roots[1],
            ],
        ] {
            assert!(
                lower_terminal_candidate(&events, &malformed, 1e-6, &mut work, &cancel,)
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn terminal_identity_preserves_budget_and_cancellation() {
        let [edge, _] = edge_pair();
        let events = [[0.0, 0.01]];
        let roots = [root(0, edge, [0.2, 0.4])];
        for (mut work, cancel, expected) in [
            (
                GeometryWorkBudget::new(0),
                AtomicBool::new(false),
                GeometryErrorCode::ResourceLimit,
            ),
            (
                GeometryWorkBudget::new(100),
                AtomicBool::new(true),
                GeometryErrorCode::Cancelled,
            ),
        ] {
            assert_eq!(
                lower_terminal_candidate(&events, &roots, 0.02, &mut work, &cancel)
                    .unwrap_err()
                    .code,
                expected
            );
        }
    }
}
