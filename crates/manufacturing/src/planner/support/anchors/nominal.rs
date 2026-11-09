// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;
use std::mem::size_of;

pub(in crate::planner::support) struct NominalAnchorPaths<'a> {
    paths: Vec<NominalPath<'a>>,
    blocks: Vec<NominalBlock>,
    reserved_records: usize,
}

const BLOCK_PATHS: usize = 16;

struct NominalBlock {
    start: usize,
    end: usize,
    bounds: [[f64; 2]; 2],
    radius: f64,
    edges: usize,
    top_z: f64,
    direct: bool,
}

struct NominalPath<'a> {
    path: &'a DepositionPath,
    direct: bool,
    bounds: [[f64; 2]; 2],
    radius: f64,
    edges: usize,
}

impl<'a> NominalAnchorPaths<'a> {
    pub(in crate::planner::support) fn new(
        previous: impl Iterator<Item = &'a DepositionPath> + Clone,
        contacts: impl Iterator<Item = &'a DepositionPath> + Clone,
        workspace: &AnchorWorkspace,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, ManufacturingError> {
        let mut count = 0usize;
        let mut edges = 0usize;
        let mut block_count = 0usize;
        let mut block_len = 0usize;
        let mut physical_owner = None;
        for (path, direct) in previous
            .clone()
            .map(|path| (path, true))
            .chain(contacts.clone().map(|path| (path, false)))
        {
            check_cancel(cancel)?;
            if block_len == BLOCK_PATHS || physical_owner != Some((direct, path.physical_z_mm[1])) {
                block_count += 1;
                block_len = 0;
                physical_owner = Some((direct, path.physical_z_mm[1]));
            }
            block_len += 1;
            count = count.saturating_add(1);
            edges = edges.saturating_add(path.points_mm.len().saturating_sub(1));
            if count > MAX_DEPOSITION_SEGMENTS as usize || edges > MAX_DEPOSITION_SEGMENTS as usize
            {
                return Ok(None);
            }
        }
        if count < 8 {
            return Ok(None);
        }
        let records = count
            .saturating_mul(size_of::<NominalPath<'a>>())
            .saturating_add(block_count * size_of::<NominalBlock>())
            .div_ceil(size_of::<[f64; 2]>());
        let required = growth_capacity(workspace.intervals.capacity(), edges)
            .saturating_add(growth_capacity(workspace.uncovered.capacity(), edges + 1))
            .saturating_add(records);
        if required > workspace.record_limit {
            return Ok(None);
        }
        let mut paths = Vec::new();
        let mut blocks: Vec<NominalBlock> = Vec::new();
        paths.try_reserve_exact(count).map_err(|_| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "nominal anchor path inventory allocation refused",
            )
        })?;
        blocks.try_reserve_exact(block_count).map_err(|_| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "nominal anchor block inventory allocation refused",
            )
        })?;
        let reserved_records = paths
            .capacity()
            .saturating_mul(size_of::<NominalPath<'a>>())
            .saturating_add(blocks.capacity() * size_of::<NominalBlock>())
            .div_ceil(size_of::<[f64; 2]>());
        if required
            .saturating_sub(records)
            .saturating_add(reserved_records)
            > workspace.record_limit
        {
            return Ok(None);
        }
        for (path, direct) in previous
            .map(|path| (path, true))
            .chain(contacts.map(|path| (path, false)))
        {
            check_cancel(cancel)?;
            let mut bounds = [[f64::INFINITY; 2], [f64::NEG_INFINITY; 2]];
            let mut valid = true;
            for point in &path.points_mm {
                check_cancel(cancel)?;
                for axis in 0..2 {
                    valid &= point[axis].is_finite();
                    bounds[0][axis] = bounds[0][axis].min(point[axis]);
                    bounds[1][axis] = bounds[1][axis].max(point[axis]);
                }
            }
            let mut radius = 0.0_f64;
            match path.checked_edges() {
                Ok(edges) => {
                    for edge in edges {
                        check_cancel(cancel)?;
                        let edge_radius = edge.width_mm * 0.5;
                        valid &= edge_radius.is_finite() && edge_radius > 0.0;
                        radius = radius.max(edge_radius);
                    }
                }
                Err(_) => valid = false,
            }
            let radius = if valid { radius } else { f64::INFINITY };
            let edges = path.points_mm.len().saturating_sub(1);
            if let Some(block) = blocks.last_mut()
                && block.end - block.start < BLOCK_PATHS
                && block.direct == direct
                && block.top_z == path.physical_z_mm[1]
            {
                block.end += 1;
                block.edges += edges;
                block.radius = block.radius.max(radius);
                for (axis, (&minimum, &maximum)) in bounds[0].iter().zip(&bounds[1]).enumerate() {
                    block.bounds[0][axis] = block.bounds[0][axis].min(minimum);
                    block.bounds[1][axis] = block.bounds[1][axis].max(maximum);
                }
            } else {
                blocks.push(NominalBlock {
                    start: paths.len(),
                    end: paths.len() + 1,
                    bounds,
                    radius,
                    edges,
                    top_z: path.physical_z_mm[1],
                    direct,
                });
            }
            paths.push(NominalPath {
                path,
                direct,
                bounds,
                radius,
                edges,
            });
        }
        Ok(Some(Self {
            paths,
            blocks,
            reserved_records,
        }))
    }

    fn inputs(
        &self,
        candidate: AnchorCandidate,
        lower_z: f64,
    ) -> impl Iterator<Item = AnchorInput> + '_ {
        self.blocks
            .iter()
            .filter(move |block| {
                if block.direct {
                    block.top_z == lower_z
                } else {
                    block.top_z <= lower_z
                }
            })
            .flat_map(move |block| {
                let rejected = rejects(candidate, block.bounds, block.radius);
                rejected
                    .then_some(AnchorInput::Skipped(block.edges))
                    .into_iter()
                    .chain(
                        (!rejected)
                            .then_some(&self.paths[block.start..block.end])
                            .into_iter()
                            .flatten()
                            .flat_map(move |prepared| {
                                let rejected = rejects(candidate, prepared.bounds, prepared.radius);
                                rejected
                                    .then_some(AnchorInput::Skipped(prepared.edges))
                                    .into_iter()
                                    .chain(
                                        nominal_edges(
                                            (!rejected).then_some(prepared.path).into_iter(),
                                            prepared.direct,
                                        )
                                        .map(AnchorInput::Edge),
                                    )
                            }),
                    )
            })
    }
}

fn rejects(candidate: AnchorCandidate, bounds: [[f64; 2]; 2], radius: f64) -> bool {
    let [start, end] = candidate.endpoints_mm;
    let reach = candidate.bead_width_mm * 0.5 + radius;
    (0..2).any(|axis| {
        start[axis].min(end[axis]) > bounds[1][axis] + reach
            || start[axis].max(end[axis]) < bounds[0][axis] - reach
    })
}

fn growth_capacity(mut capacity: usize, needed: usize) -> usize {
    while capacity < needed {
        capacity = capacity.saturating_mul(2).max(4);
    }
    capacity
}

impl AnchorWorkspace {
    pub(in crate::planner::support) fn gaps_nominal(
        &mut self,
        candidate: AnchorCandidate,
        lower_z: f64,
        paths: &NominalAnchorPaths<'_>,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<&[AnchorGap], ManufacturingError> {
        let limit = self.record_limit;
        self.record_limit -= paths.reserved_records;
        let result = self
            .gaps_inputs(candidate, paths.inputs(candidate, lower_z), work, cancel)
            .map(|_| ());
        self.record_limit = limit;
        result?;
        Ok(&self.uncovered)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiling_contracts::manufacturing::{DepositionKind, WidthProfile};

    fn paths() -> Vec<DepositionPath> {
        (0..32)
            .map(|index| {
                let offset = index as f64 * 10.0 + 0.000000123;
                DepositionPath {
                    material_domain: None,
                    kind: DepositionKind::Perimeter,
                    owner: None,
                    width: WidthProfile::PerEdge {
                        widths_mm: vec![0.4, 0.8],
                    },
                    physical_z_mm: [0.0, if index % 3 == 0 { 0.1 } else { 0.2 }],
                    points_mm: vec![[offset, -1.0, 0.2], [offset, 0.0, 0.2], [offset, 1.0, 0.2]],
                }
            })
            .collect()
    }

    fn compare(paths: &[DepositionPath], contacts: &[DepositionPath]) {
        let cancel = AtomicBool::new(false);
        let mut scalar = AnchorWorkspace::default();
        let mut screened = AnchorWorkspace::default();
        let prepared = NominalAnchorPaths::new(paths.iter(), contacts.iter(), &screened, &cancel)
            .unwrap()
            .unwrap();
        for lower_z in [f64::from_bits(1), 0.1, 0.2, 0.3] {
            for offset in [0.0, 9.8, 20.0, 310.0, 1000.0] {
                let candidate = AnchorCandidate {
                    endpoints_mm: [[offset - 2.0, 0.0, 0.4], [offset + 2.0, 0.0, 0.4]],
                    bead_width_mm: 0.4,
                };
                for initial in [0, MAX_ANCHOR_PAIR_WORK - 3, MAX_ANCHOR_PAIR_WORK] {
                    let mut expected_work = initial;
                    let mut actual_work = initial;
                    let expected = scalar.gaps(
                        candidate,
                        paths.iter().filter(|path| path.physical_z_mm[1] == lower_z),
                        contacts
                            .iter()
                            .filter(|path| path.physical_z_mm[1] <= lower_z),
                        &mut expected_work,
                        &cancel,
                    );
                    let actual = screened.gaps_nominal(
                        candidate,
                        lower_z,
                        &prepared,
                        &mut actual_work,
                        &cancel,
                    );
                    assert_eq!(actual_work, expected_work);
                    match (expected, actual) {
                        (Ok(expected), Ok(actual)) => {
                            let fields = |gaps: &[AnchorGap]| {
                                gaps.iter()
                                    .map(|gap| {
                                        (
                                            gap.start.to_bits(),
                                            gap.end.to_bits(),
                                            gap.left,
                                            gap.right,
                                            gap.direct_ends,
                                        )
                                    })
                                    .collect::<Vec<_>>()
                            };
                            assert_eq!(fields(actual), fields(expected));
                        }
                        (Err(expected), Err(actual)) => {
                            assert_eq!(actual.code, expected.code);
                            assert_eq!(actual.message, expected.message);
                        }
                        result => panic!("nominal screening mismatch: {result:?}"),
                    }
                    assert_eq!(screened.record_limit, MAX_DEPOSITION_SEGMENTS as usize);
                }
            }
        }
        assert_eq!(screened.record_limit, MAX_DEPOSITION_SEGMENTS as usize);
        assert_eq!(
            screened
                .gaps_nominal(
                    AnchorCandidate {
                        endpoints_mm: [[0.0, 0.0, 0.4], [1.0, 0.0, 0.4]],
                        bead_width_mm: 0.4
                    },
                    0.2,
                    &prepared,
                    &mut 0,
                    &AtomicBool::new(true),
                )
                .unwrap_err()
                .code,
            ManufacturingErrorCode::Cancelled,
        );
        assert_eq!(screened.record_limit, MAX_DEPOSITION_SEGMENTS as usize);
    }

    #[test]
    fn nominal_screening_preserves_raw_coordinates_contacts_and_work() {
        let mut paths = paths();
        compare(&paths, &paths);
        compare(&paths, &[]);
        for path in &mut paths {
            path.physical_z_mm = [0.0, 0.2];
        }
        compare(&paths, &paths);
        compare(&paths, &[]);
        let prepared = NominalAnchorPaths::new(
            paths.iter(),
            std::iter::empty(),
            &AnchorWorkspace::default(),
            &AtomicBool::new(false),
        )
        .unwrap()
        .unwrap();
        let rejected = prepared
            .inputs(
                AnchorCandidate {
                    endpoints_mm: [[10_000.0, 0.0, 0.4], [10_001.0, 0.0, 0.4]],
                    bead_width_mm: 0.4,
                },
                0.2,
            )
            .map(|input| match input {
                AnchorInput::Skipped(edges) => edges,
                AnchorInput::Edge(_) => panic!("distant block enumerated an edge"),
            })
            .collect::<Vec<_>>();
        assert_eq!(rejected, [32, 32]);
    }

    #[test]
    fn nominal_screening_never_hides_malformed_distant_paths() {
        for invalid in 0..7 {
            let mut paths = paths();
            match invalid {
                0 => paths[31].points_mm[1][0] = f64::NAN,
                1 => {
                    paths[31].width = WidthProfile::PerEdge {
                        widths_mm: vec![0.4],
                    }
                }
                2 => {
                    paths[31].width = WidthProfile::Constant {
                        width_mm: f64::from_bits(1),
                    }
                }
                3 => {
                    paths[31].physical_z_mm = [0.0, f64::from_bits(1)];
                    paths[31].width = WidthProfile::PerEdge {
                        widths_mm: vec![f64::from_bits(1), 0.8],
                    };
                    assert!(paths[31].checked_edges().is_ok());
                }
                4 => paths[31].points_mm[0][1] = f64::INFINITY,
                5 => paths[31].width = WidthProfile::Constant { width_mm: f64::NAN },
                _ => paths[31].points_mm.clear(),
            }
            compare(&paths, &[]);
            compare(&paths[..31], &paths);
            for path in &mut paths[..31] {
                path.physical_z_mm = [0.0, 0.2];
            }
            compare(&paths, &[]);
            compare(&paths[..31], &paths);
        }
    }

    #[test]
    fn nominal_inventory_declines_before_workspace_pressure() {
        let paths = paths();
        let cancel = AtomicBool::new(false);
        let mut workspace = AnchorWorkspace::default();
        for budget in [0, 1, 8, 64, 128] {
            workspace.set_budget(budget);
            assert!(
                NominalAnchorPaths::new(paths.iter(), std::iter::empty(), &workspace, &cancel)
                    .unwrap()
                    .is_none()
            );
            assert_eq!(workspace.record_limit, budget);
            assert_eq!(workspace.retained_bytes(), 0);
        }
    }

    #[test]
    fn nominal_inventory_admits_actual_retained_growth_and_worst_gaps() {
        let cancel = AtomicBool::new(false);
        let paths = (0..1000)
            .map(|index| DepositionPath {
                material_domain: None,
                kind: DepositionKind::Perimeter,
                owner: None,
                width: WidthProfile::Constant { width_mm: 0.2 },
                physical_z_mm: [0.0, 0.1],
                points_mm: vec![
                    [index as f64 * 2.0, -0.1, 0.1],
                    [index as f64 * 2.0, 0.1, 0.1],
                ],
            })
            .collect::<Vec<_>>();
        let metadata = (paths.len() * size_of::<NominalPath<'_>>()
            + paths.len().div_ceil(BLOCK_PATHS) * size_of::<NominalBlock>())
        .div_ceil(size_of::<[f64; 2]>());
        let candidate = AnchorCandidate {
            endpoints_mm: [[-1.0, 0.0, 0.2], [2001.0, 0.0, 0.2]],
            bead_width_mm: 0.2,
        };
        for (interval_capacity, gap_capacity) in [(900, 0), (0, 900), (900, 900)] {
            let mut screened = AnchorWorkspace::default();
            screened
                .intervals
                .try_reserve_exact(interval_capacity)
                .unwrap();
            screened.uncovered.try_reserve_exact(gap_capacity).unwrap();
            screened.set_budget(6048);
            assert!(
                NominalAnchorPaths::new(paths.iter(), std::iter::empty(), &screened, &cancel)
                    .unwrap()
                    .is_none()
            );
            let budget = metadata
                + growth_capacity(screened.intervals.capacity(), 1000)
                + growth_capacity(screened.uncovered.capacity(), 1001);
            screened.set_budget(budget);
            let prepared =
                NominalAnchorPaths::new(paths.iter(), std::iter::empty(), &screened, &cancel)
                    .unwrap()
                    .unwrap();
            let mut scalar = AnchorWorkspace::default();
            scalar
                .intervals
                .try_reserve_exact(interval_capacity)
                .unwrap();
            scalar.uncovered.try_reserve_exact(gap_capacity).unwrap();
            scalar.set_budget(6048);
            let expected = scalar
                .gaps(candidate, paths.iter(), std::iter::empty(), &mut 0, &cancel)
                .unwrap();
            let actual = screened
                .gaps_nominal(candidate, 0.1, &prepared, &mut 0, &cancel)
                .unwrap();
            assert_eq!(actual.len(), 1001);
            assert_eq!(actual.len(), expected.len());
            for (actual, expected) in actual.iter().zip(expected) {
                assert_eq!(
                    (actual.start.to_bits(), actual.end.to_bits()),
                    (expected.start.to_bits(), expected.end.to_bits())
                );
            }
            assert_eq!(screened.record_limit, budget);
        }
    }

    #[test]
    fn nominal_block_tangency_matches_adjacent_scalar_queries() {
        let mut paths = paths();
        for path in &mut paths {
            path.physical_z_mm = [0.0, 0.2];
        }
        let cancel = AtomicBool::new(false);
        let mut scalar = AnchorWorkspace::default();
        let mut screened = AnchorWorkspace::default();
        let prepared =
            NominalAnchorPaths::new(paths.iter(), std::iter::empty(), &screened, &cancel)
                .unwrap()
                .unwrap();
        let tangent = paths[31].points_mm[0][0] + (0.4 * 0.5 + 0.8 * 0.5);
        for position in [tangent.next_down(), tangent, tangent.next_up(), f64::MAX] {
            let candidate = AnchorCandidate {
                endpoints_mm: [[position, -0.25, 0.4], [position, 0.25, 0.4]],
                bead_width_mm: 0.4,
            };
            let mut expected_work = 0;
            let mut actual_work = 0;
            let fields = |gaps: &[AnchorGap]| {
                gaps.iter()
                    .map(|gap| {
                        (
                            gap.start.to_bits(),
                            gap.end.to_bits(),
                            gap.left,
                            gap.right,
                            gap.direct_ends,
                        )
                    })
                    .collect::<Vec<_>>()
            };
            let expected = fields(
                scalar
                    .gaps(
                        candidate,
                        paths.iter(),
                        std::iter::empty(),
                        &mut expected_work,
                        &cancel,
                    )
                    .unwrap(),
            );
            let actual = fields(
                screened
                    .gaps_nominal(candidate, 0.2, &prepared, &mut actual_work, &cancel)
                    .unwrap(),
            );
            assert_eq!(actual, expected);
            assert_eq!(actual_work, expected_work);
        }
    }

    #[test]
    fn nominal_blocks_preserve_z_provenance_and_bounded_order() {
        let mut paths = paths();
        for path in &mut paths {
            path.physical_z_mm = [0.0, 0.2];
        }
        paths[0].physical_z_mm = [-0.1, -0.0];
        paths[1].physical_z_mm = [-0.1, 0.0];
        paths[2].physical_z_mm[1] = f64::NAN;
        let prepared = NominalAnchorPaths::new(
            paths.iter(),
            paths.iter(),
            &AnchorWorkspace::default(),
            &AtomicBool::new(false),
        )
        .unwrap()
        .unwrap();
        assert_eq!(prepared.blocks.len(), 8);
        assert_eq!(prepared.blocks[0].end, 2);
        let mut cursor = 0;
        for block in &prepared.blocks {
            assert_eq!(block.start, cursor);
            assert!((1..=BLOCK_PATHS).contains(&(block.end - block.start)));
            for path in &prepared.paths[block.start..block.end] {
                assert_eq!(path.direct, block.direct);
                assert!(
                    path.path.physical_z_mm[1] == block.top_z
                        || block.top_z.is_nan() && block.end - block.start == 1
                );
            }
            assert_eq!(
                block.edges,
                prepared.paths[block.start..block.end]
                    .iter()
                    .map(|path| path.edges)
                    .sum::<usize>()
            );
            cursor = block.end;
        }
        assert_eq!(cursor, paths.len() * 2);
    }
}
