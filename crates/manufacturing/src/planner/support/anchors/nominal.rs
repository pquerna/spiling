// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;
use std::mem::size_of;

mod grid;

pub(in crate::planner::support) struct NominalAnchorPaths<'a> {
    paths: Vec<NominalPath<'a>>,
    blocks: Vec<NominalBlock>,
    reserved_records: usize,
    /// Optional transient query index; selection only, released with the inventory.
    grid: Option<grid::EdgeGrid>,
    stats: InventoryStats,
    /// Candidate runs analyzed against this inventory, for the index heuristic.
    queries: usize,
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

impl NominalPath<'_> {
    /// The same physical-floor filter the block scan applies per block.
    fn eligible(&self, lower_z: f64) -> bool {
        let top = self.path.physical_z_mm[1];
        if self.direct {
            top == lower_z
        } else {
            top <= lower_z
        }
    }
}

/// Inventory shape of one borrowed lower set; admission against a workspace
/// is a pure function of these counts and that workspace's current capacities.
#[derive(Clone, Copy)]
pub(in crate::planner::support) struct InventoryStats {
    count: usize,
    edges: usize,
    block_count: usize,
}

impl InventoryStats {
    pub(in crate::planner::support) fn new<'a>(
        previous: impl Iterator<Item = &'a DepositionPath>,
        contacts: impl Iterator<Item = &'a DepositionPath>,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, ManufacturingError> {
        let mut count = 0usize;
        let mut edges = 0usize;
        let mut block_count = 0usize;
        let mut block_len = 0usize;
        let mut physical_owner = None;
        for (path, direct) in previous
            .map(|path| (path, true))
            .chain(contacts.map(|path| (path, false)))
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
        Ok(Some(Self {
            count,
            edges,
            block_count,
        }))
    }

    fn records(&self) -> usize {
        self.count
            .saturating_mul(size_of::<NominalPath<'_>>())
            .saturating_add(self.block_count * size_of::<NominalBlock>())
            .div_ceil(size_of::<[f64; 2]>())
    }

    fn required(&self, workspace: &AnchorWorkspace) -> usize {
        growth_capacity(workspace.intervals.capacity(), self.edges)
            .saturating_add(growth_capacity(
                workspace.uncovered.capacity(),
                self.edges + 1,
            ))
            .saturating_add(self.records())
    }

    /// Admission before allocating inventory metadata.
    pub(in crate::planner::support) fn admits(&self, workspace: &AnchorWorkspace) -> bool {
        self.required(workspace) <= workspace.record_limit
    }
}

impl<'a> NominalAnchorPaths<'a> {
    pub(in crate::planner::support) fn new(
        previous: impl Iterator<Item = &'a DepositionPath> + Clone,
        contacts: impl Iterator<Item = &'a DepositionPath> + Clone,
        workspace: &AnchorWorkspace,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, ManufacturingError> {
        let Some(stats) = InventoryStats::new(previous.clone(), contacts.clone(), cancel)? else {
            return Ok(None);
        };
        if !stats.admits(workspace) {
            return Ok(None);
        }
        let inventory = Self::build(previous, contacts, stats, cancel)?;
        Ok(inventory.admitted(workspace).then_some(inventory))
    }

    /// Admission with the actual allocated metadata, as a fresh build would see it.
    pub(in crate::planner::support) fn admitted(&self, workspace: &AnchorWorkspace) -> bool {
        self.stats.admits(workspace)
            && self
                .stats
                .required(workspace)
                .saturating_sub(self.stats.records())
                .saturating_add(self.reserved_records)
                <= workspace.record_limit
    }

    pub(in crate::planner::support) fn build(
        previous: impl Iterator<Item = &'a DepositionPath>,
        contacts: impl Iterator<Item = &'a DepositionPath>,
        stats: InventoryStats,
        cancel: &AtomicBool,
    ) -> Result<Self, ManufacturingError> {
        let mut paths = Vec::new();
        let mut blocks: Vec<NominalBlock> = Vec::new();
        paths.try_reserve_exact(stats.count).map_err(|_| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "nominal anchor path inventory allocation refused",
            )
        })?;
        blocks.try_reserve_exact(stats.block_count).map_err(|_| {
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
        Ok(Self {
            paths,
            blocks,
            reserved_records,
            grid: None,
            stats,
            queries: 0,
        })
    }

    /// Build the edge grid when the expected block scans outweigh indexing.
    /// Selection only; any decision here leaves every query result unchanged.
    pub(in crate::planner::support) fn index_for(
        &mut self,
        candidates: usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        self.queries = self.queries.saturating_add(candidates);
        if self.grid.is_some() {
            return Ok(());
        }
        let edges = self.blocks.iter().map(|block| block.edges).sum::<usize>();
        if self.queries.saturating_mul(self.blocks.len()) <= edges.saturating_mul(4) {
            return Ok(());
        }
        self.grid = grid::EdgeGrid::new(self.paths.iter().map(|path| path.path), cancel)?;
        Ok(())
    }

    #[cfg(test)]
    fn force_index(&mut self, cancel: &AtomicBool) {
        self.grid = grid::EdgeGrid::new(self.paths.iter().map(|path| path.path), cancel).unwrap();
        assert!(self.grid.is_some());
    }

    /// Ordered grid selection: z-eligible edges before each admitted edge or
    /// whole-enumerated path are charged as skipped work at the same position.
    fn grid_inputs<'s>(
        paths: &'s [NominalPath<'a>],
        grid: &'s grid::EdgeGrid,
        prefix: &'s [usize],
        lower_z: f64,
    ) -> impl Iterator<Item = AnchorInput> + 's {
        let mut hits = grid.hits.iter().copied().peekable();
        let mut whole = grid
            .whole
            .iter()
            .copied()
            .filter(move |&path| paths[path as usize].eligible(lower_z))
            .peekable();
        let mut charged = 0usize;
        let mut pending: Option<AnchorInput> = None;
        let mut current = None;
        let mut finished = false;
        std::iter::from_fn(move || {
            loop {
                if let Some(item) = pending.take() {
                    return Some(item);
                }
                if let Some(item) = current.as_mut().and_then(Iterator::next) {
                    return Some(AnchorInput::Edge(item));
                }
                current = None;
                if finished {
                    return None;
                }
                let next_hit = hits.peek().copied();
                let hit_path = next_hit.map(|edge| grid.edges[edge as usize].path);
                let take_whole = match (hit_path, whole.peek().copied()) {
                    (Some(hit), Some(path)) => path < hit,
                    (None, Some(_)) => true,
                    _ => false,
                };
                let skipped = if take_whole {
                    let path = whole.next().expect("peeked whole path") as usize;
                    let prepared = &paths[path];
                    let skipped = prefix[path] - charged;
                    charged = prefix[path + 1];
                    current = Some(nominal_edges(
                        std::iter::once(prepared.path),
                        prepared.direct,
                    ));
                    skipped
                } else if let Some(edge) = next_hit {
                    hits.next();
                    let record = grid.edges[edge as usize];
                    let path = record.path as usize;
                    let position = prefix[path] + (edge - grid.first_edge[path]) as usize;
                    let skipped = position - charged;
                    charged = position + 1;
                    pending = Some(AnchorInput::Edge((
                        Ok([
                            [record.c[0], record.c[1], 0.0],
                            [record.d[0], record.d[1], 0.0],
                        ]),
                        paths[path].direct,
                        record.width,
                    )));
                    skipped
                } else {
                    finished = true;
                    let total = prefix[paths.len()];
                    let skipped = total - charged;
                    charged = total;
                    skipped
                };
                if skipped > 0 {
                    return Some(AnchorInput::Skipped(skipped));
                }
            }
        })
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
        paths: &mut NominalAnchorPaths<'_>,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<&[AnchorGap], ManufacturingError> {
        let limit = self.record_limit;
        self.record_limit -= paths.reserved_records;
        let [a, b] = candidate.endpoints_mm;
        let radius = candidate.bead_width_mm * 0.5;
        // The grid's no-side-effect argument needs a bounded valid query; any
        // other query keeps the ordered block scan and its typed refusals.
        let indexed = paths.grid.is_some()
            && radius.is_finite()
            && radius > 0.0
            && radius <= grid::INDEXED_MAGNITUDE_MM
            && [a[0], a[1], b[0], b[1]]
                .iter()
                .all(|x| x.abs() <= grid::INDEXED_MAGNITUDE_MM)
            && (a[0] != b[0] || a[1] != b[1]);
        let result = if indexed {
            let grid = paths.grid.as_mut().expect("checked edge grid");
            let prefix = grid.prefix(lower_z, &paths.paths);
            grid.collect(candidate, lower_z, &paths.paths);
            let inputs = NominalAnchorPaths::grid_inputs(
                &paths.paths,
                grid,
                grid.prefix_at(prefix),
                lower_z,
            );
            self.gaps_inputs(candidate, inputs, work, cancel)
        } else {
            self.gaps_inputs(candidate, paths.inputs(candidate, lower_z), work, cancel)
        }
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
                    arcs: Vec::new(),
                }
            })
            .collect()
    }

    fn compare(paths: &[DepositionPath], contacts: &[DepositionPath]) {
        let cancel = AtomicBool::new(false);
        let mut scalar = AnchorWorkspace::default();
        let mut screened = AnchorWorkspace::default();
        let mut prepared =
            NominalAnchorPaths::new(paths.iter(), contacts.iter(), &screened, &cancel)
                .unwrap()
                .unwrap();
        for indexed in [false, true] {
            if indexed {
                prepared.force_index(&cancel);
            }
            compare_queries(paths, contacts, &mut prepared, &mut scalar, &mut screened);
        }
        let prepared = &mut prepared;
        assert_eq!(screened.record_limit, MAX_DEPOSITION_SEGMENTS as usize);
        assert_eq!(
            screened
                .gaps_nominal(
                    AnchorCandidate {
                        endpoints_mm: [[0.0, 0.0, 0.4], [1.0, 0.0, 0.4]],
                        bead_width_mm: 0.4
                    },
                    0.2,
                    prepared,
                    &mut 0,
                    &AtomicBool::new(true),
                )
                .unwrap_err()
                .code,
            ManufacturingErrorCode::Cancelled,
        );
        assert_eq!(screened.record_limit, MAX_DEPOSITION_SEGMENTS as usize);
    }

    fn compare_queries(
        paths: &[DepositionPath],
        contacts: &[DepositionPath],
        prepared: &mut NominalAnchorPaths<'_>,
        scalar: &mut AnchorWorkspace,
        screened: &mut AnchorWorkspace,
    ) {
        let cancel = AtomicBool::new(false);
        for lower_z in [f64::from_bits(1), 0.1, 0.2, 0.3] {
            for offset in [0.0, 9.8, 20.0, 310.0, 1000.0] {
                let candidate = AnchorCandidate {
                    endpoints_mm: [[offset - 2.0, 0.0, 0.4], [offset + 2.0, 0.0, 0.4]],
                    bead_width_mm: 0.4,
                };
                for initial in [0, usize::MAX - 3, usize::MAX] {
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
                        prepared,
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
                arcs: Vec::new(),
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
            let mut prepared =
                NominalAnchorPaths::new(paths.iter(), std::iter::empty(), &screened, &cancel)
                    .unwrap()
                    .unwrap();
            prepared.force_index(&cancel);
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
                .gaps_nominal(candidate, 0.1, &mut prepared, &mut 0, &cancel)
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
    fn edge_grid_matches_scalar_on_random_inventories() {
        let mut seed = 0x9e37_79b9_7f4a_7c15_u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        let tops = [0.2, 0.4, 0.6];
        for trial in 0..12 {
            let mut make = |count: usize| {
                (0..count)
                    .map(|_| {
                        let top = tops[(next() * 3.0) as usize % 3];
                        let points = 2 + (next() * 6.0) as usize;
                        let mut point = [next() * 60.0 - 30.0, next() * 60.0 - 30.0];
                        let mut points_mm = vec![[point[0], point[1], top]];
                        for _ in 1..points {
                            let length = if next() < 0.5 {
                                next() * 2.0
                            } else {
                                next() * 40.0
                            };
                            let angle = if next() < 0.5 {
                                (next() * 4.0).floor() * std::f64::consts::FRAC_PI_2
                            } else {
                                next() * std::f64::consts::TAU
                            };
                            point = [
                                point[0] + length * angle.cos(),
                                point[1] + length * angle.sin(),
                            ];
                            // Snap to a decimal grid so exact tangencies occur.
                            point = point.map(|x| (x * 10.0).round() / 10.0);
                            points_mm.push([point[0], point[1], top]);
                        }
                        let edges = points_mm.len() - 1;
                        DepositionPath {
                            material_domain: None,
                            kind: DepositionKind::SparseFill,
                            owner: None,
                            width: if next() < 0.5 {
                                WidthProfile::Constant { width_mm: 0.4 }
                            } else {
                                WidthProfile::PerEdge {
                                    widths_mm: (0..edges)
                                        .map(|i| 0.2 + 0.2 * (i % 3) as f64)
                                        .collect(),
                                }
                            },
                            physical_z_mm: [top - 0.2, top],
                            points_mm,
                            arcs: Vec::new(),
                        }
                    })
                    .collect::<Vec<_>>()
            };
            let mut previous = make(300);
            let contacts = make(100);
            if trial % 3 == 1 {
                // Zero-length edge and malformed width stay in ordered enumeration.
                let point = previous[17].points_mm[0];
                previous[17].points_mm.insert(0, point);
                previous[17].width = WidthProfile::Constant { width_mm: 0.4 };
            }
            if trial % 3 == 2 {
                previous[250].width = WidthProfile::Constant { width_mm: f64::NAN };
            }
            let cancel = AtomicBool::new(false);
            let mut scalar = AnchorWorkspace::default();
            let mut screened = AnchorWorkspace::default();
            let mut prepared =
                NominalAnchorPaths::new(previous.iter(), contacts.iter(), &screened, &cancel)
                    .unwrap()
                    .unwrap();
            prepared.force_index(&cancel);
            for _ in 0..400 {
                let a = [next() * 70.0 - 35.0, next() * 70.0 - 35.0, 0.0];
                let length = next() * 30.0 + 0.05;
                let angle = if next() < 0.5 {
                    (next() * 4.0).floor() * std::f64::consts::FRAC_PI_2
                } else {
                    next() * std::f64::consts::TAU
                };
                let b = [
                    ((a[0] + length * angle.cos()) * 10.0).round() / 10.0,
                    ((a[1] + length * angle.sin()) * 10.0).round() / 10.0,
                    0.0,
                ];
                let a = [
                    (a[0] * 10.0).round() / 10.0,
                    (a[1] * 10.0).round() / 10.0,
                    0.0,
                ];
                let candidate = AnchorCandidate {
                    endpoints_mm: [a, b],
                    bead_width_mm: [0.2, 0.4, 0.6][(next() * 3.0) as usize % 3],
                };
                let lower_z = tops[(next() * 3.0) as usize % 3];
                let (mut expected_work, mut actual_work) = (7usize, 7usize);
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
                let expected = scalar
                    .gaps(
                        candidate,
                        previous
                            .iter()
                            .filter(|path| path.physical_z_mm[1] == lower_z),
                        contacts
                            .iter()
                            .filter(|path| path.physical_z_mm[1] <= lower_z),
                        &mut expected_work,
                        &cancel,
                    )
                    .map(fields)
                    .map_err(|e| (e.code, e.message));
                let actual = screened
                    .gaps_nominal(candidate, lower_z, &mut prepared, &mut actual_work, &cancel)
                    .map(fields)
                    .map_err(|e| (e.code, e.message));
                assert_eq!(actual, expected, "trial {trial}");
                assert_eq!(actual_work, expected_work, "trial {trial}");
            }
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
        let mut prepared =
            NominalAnchorPaths::new(paths.iter(), std::iter::empty(), &screened, &cancel)
                .unwrap()
                .unwrap();
        let tangent = paths[31].points_mm[0][0] + (0.4 * 0.5 + 0.8 * 0.5);
        let positions = [tangent.next_down(), tangent, tangent.next_up(), f64::MAX];
        for (step, position) in positions.into_iter().cycle().take(8).enumerate() {
            if step == 4 {
                prepared.force_index(&cancel);
            }
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
                    .gaps_nominal(candidate, 0.2, &mut prepared, &mut actual_work, &cancel)
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
