// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use crate::check_cancel;
use spiling_contracts::manufacturing::{
    DepositionPath, ManufacturingError, ManufacturingErrorCode, WidthProfile,
};
use std::{mem::size_of, ops::Range, sync::atomic::AtomicBool};

const LEAF_PATHS: usize = 16;

#[derive(Clone, Copy)]
struct Bounds {
    xy: [[f64; 2]; 2],
    width: f64,
}

struct Node {
    paths: Range<usize>,
    after: usize,
    nonmodel_paths: usize,
    bounds: Option<Bounds>,
}

pub(super) struct ClearanceIndex {
    layers: Vec<LayerIndex>,
}

pub(super) struct LayerIndex {
    nodes: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PathRange {
    pub(super) paths: Range<usize>,
    pub(super) nonmodel_paths: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Candidates {
    Charge(usize),
    Visit(PathRange),
    Skip { paths: usize, nonmodel_paths: usize },
}

impl ClearanceIndex {
    pub(super) fn build<'a>(
        layers: impl ExactSizeIterator<Item = &'a [DepositionPath]>,
        byte_allowance: usize,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, ManufacturingError> {
        charge(work, 1, cancel)?;
        let mut index = Self { layers: Vec::new() };
        let Some(_) = layers
            .len()
            .checked_mul(size_of::<LayerIndex>())
            .and_then(|bytes| bytes.checked_add(size_of::<Self>()))
            .filter(|bytes| *bytes <= byte_allowance)
        else {
            return Ok(None);
        };
        if index.layers.try_reserve_exact(layers.len()).is_err() {
            return Ok(None);
        }
        let Some(mut retained) = index
            .layers
            .capacity()
            .checked_mul(size_of::<LayerIndex>())
            .and_then(|bytes| bytes.checked_add(size_of::<Self>()))
            .filter(|bytes| *bytes <= byte_allowance)
        else {
            return Ok(None);
        };
        for paths in layers {
            charge(work, 1, cancel)?;
            let Some(layer) = LayerIndex::build(paths, byte_allowance - retained, work, cancel)?
            else {
                return Ok(None);
            };
            let Some(bytes) = retained.checked_add(layer.node_bytes()) else {
                return Ok(None);
            };
            retained = bytes;
            index.layers.push(layer);
        }
        check_cancel(cancel)?;
        Ok(Some(index))
    }

    pub(super) fn layer(&self, ordinal: usize) -> Option<&LayerIndex> {
        self.layers.get(ordinal)
    }

    pub(super) fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            + self.layers.capacity() * size_of::<LayerIndex>()
            + self
                .layers
                .iter()
                .map(LayerIndex::node_bytes)
                .sum::<usize>()
    }
}

impl LayerIndex {
    fn build(
        paths: &[DepositionPath],
        byte_allowance: usize,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, ManufacturingError> {
        charge(work, 1, cancel)?;
        let leaves = paths.len().div_ceil(LEAF_PATHS);
        let count = if leaves == 0 {
            Some(0)
        } else {
            leaves.checked_mul(2).and_then(|count| count.checked_sub(1))
        };
        let Some(count) = count.filter(|count| {
            count
                .checked_mul(size_of::<Node>())
                .is_some_and(|bytes| bytes <= byte_allowance)
        }) else {
            return Ok(None);
        };
        let mut index = Self { nodes: Vec::new() };
        if index.nodes.try_reserve_exact(count).is_err() || index.node_bytes() > byte_allowance {
            return Ok(None);
        }
        if !paths.is_empty() {
            index.build_range(paths, 0..paths.len(), work, cancel)?;
        }
        check_cancel(cancel)?;
        Ok(Some(index))
    }

    fn node_bytes(&self) -> usize {
        self.nodes.capacity().saturating_mul(size_of::<Node>())
    }

    fn build_range(
        &mut self,
        paths: &[DepositionPath],
        range: Range<usize>,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<usize, ManufacturingError> {
        charge(work, 1, cancel)?;
        let ordinal = self.nodes.len();
        self.nodes.push(Node {
            paths: range.clone(),
            after: 0,
            nonmodel_paths: 0,
            bounds: None,
        });
        let (nonmodel_paths, bounds) = if range.len() <= LEAF_PATHS {
            let mut count = 0;
            let mut bounds = None;
            let mut unknown = false;
            for path in &paths[range] {
                charge(work, 1, cancel)?;
                if path.kind.is_model() {
                    continue;
                }
                count += 1;
                match path_bounds(path, work, cancel)? {
                    Some(next) => bounds = Some(combine(bounds, next)),
                    None => unknown = true,
                }
            }
            (count, if unknown { None } else { bounds })
        } else {
            let left_leaves = range.len().div_ceil(LEAF_PATHS) / 2;
            let middle = range.start + left_leaves * LEAF_PATHS;
            let left = self.build_range(paths, range.start..middle, work, cancel)?;
            let right = self.build_range(paths, middle..range.end, work, cancel)?;
            let left = &self.nodes[left];
            let right = &self.nodes[right];
            let count = left.nonmodel_paths + right.nonmodel_paths;
            let unknown = (left.nonmodel_paths != 0 && left.bounds.is_none())
                || (right.nonmodel_paths != 0 && right.bounds.is_none());
            let bounds = if unknown {
                None
            } else {
                match (left.bounds, right.bounds) {
                    (bounds, Some(right)) => Some(combine(bounds, right)),
                    (bounds, None) => bounds,
                }
            };
            (count, bounds)
        };
        let after = self.nodes.len();
        let node = &mut self.nodes[ordinal];
        node.after = after;
        node.nonmodel_paths = nonmodel_paths;
        node.bounds = bounds;
        Ok(ordinal)
    }

    pub(super) fn visit(
        &self,
        forbidden: [[f64; 2]; 2],
        gap_mm: f64,
        xyz6_slack_mm: f64,
        cancel: &AtomicBool,
        mut visit: impl FnMut(Candidates) -> Result<(), ManufacturingError>,
    ) -> Result<(), ManufacturingError> {
        check_cancel(cancel)?;
        let screen = forbidden.iter().flatten().all(|value| value.is_finite())
            && (0..2).all(|axis| forbidden[0][axis] <= forbidden[1][axis])
            && gap_mm.is_finite()
            && gap_mm >= 0.0
            && xyz6_slack_mm.is_finite()
            && xyz6_slack_mm >= 0.0;
        let mut ordinal = 0;
        while let Some(node) = self.nodes.get(ordinal) {
            check_cancel(cancel)?;
            visit(Candidates::Charge(1))?;
            check_cancel(cancel)?;
            let skip = node.nonmodel_paths == 0
                || (screen
                    && node.bounds.is_some_and(|bounds| {
                        spiling_geometry::planar_bounds_clear(
                            bounds.xy,
                            forbidden,
                            bounds.width * 0.5 + xyz6_slack_mm,
                            gap_mm,
                        )
                    }));
            if skip {
                visit(Candidates::Skip {
                    paths: node.paths.len(),
                    nonmodel_paths: node.nonmodel_paths,
                })?;
                ordinal = node.after;
            } else if node.paths.len() <= LEAF_PATHS {
                visit(Candidates::Visit(PathRange {
                    paths: node.paths.clone(),
                    nonmodel_paths: node.nonmodel_paths,
                }))?;
                ordinal += 1;
            } else {
                ordinal += 1;
            }
        }
        check_cancel(cancel)
    }
}

fn combine(bounds: Option<Bounds>, next: Bounds) -> Bounds {
    bounds.map_or(next, |bounds| Bounds {
        xy: std::array::from_fn(|side| {
            std::array::from_fn(|axis| {
                if side == 0 {
                    bounds.xy[side][axis].min(next.xy[side][axis])
                } else {
                    bounds.xy[side][axis].max(next.xy[side][axis])
                }
            })
        }),
        width: bounds.width.max(next.width),
    })
}

fn path_bounds(
    path: &DepositionPath,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<Bounds>, ManufacturingError> {
    charge(work, 1, cancel)?;
    if let WidthProfile::PerEdge { widths_mm } = &path.width {
        charge(work, widths_mm.len(), cancel)?;
    }
    let Ok(edges) = path.checked_edges() else {
        return Ok(None);
    };
    let mut xy = [[f64::INFINITY; 2], [f64::NEG_INFINITY; 2]];
    for point in &path.points_mm {
        charge(work, 1, cancel)?;
        if !point.iter().all(|coordinate| coordinate.is_finite()) {
            return Ok(None);
        }
        for axis in 0..2 {
            xy[0][axis] = xy[0][axis].min(point[axis]);
            xy[1][axis] = xy[1][axis].max(point[axis]);
        }
    }
    let mut width = 0.0_f64;
    for edge in edges {
        charge(work, 1, cancel)?;
        width = width.max(edge.width_mm);
    }
    Ok(Some(Bounds { xy, width }))
}

fn charge(work: &mut usize, amount: usize, cancel: &AtomicBool) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    *work = work.checked_add(amount).ok_or_else(|| {
        crate::error(
            ManufacturingErrorCode::ResourceLimit,
            "cumulative actual support-clearance scan work counter overflow",
        )
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiling_contracts::manufacturing::{DepositionKind, ManufacturingErrorCode, WidthProfile};
    use std::sync::atomic::Ordering;

    fn path(x: f64, kind: DepositionKind) -> DepositionPath {
        DepositionPath {
            material_domain: None,
            kind,
            owner: None,
            width: WidthProfile::Constant { width_mm: 0.5 },
            physical_z_mm: [0.0, 0.2],
            points_mm: vec![[x, 0.0, 0.2], [x + 1.0, 0.0, 0.2]],
            arcs: Vec::new(),
            exposed: false,
            connectors: Vec::new(),
        }
    }

    fn collect(layer: &LayerIndex, bounds: [[f64; 2]; 2], gap: f64, slack: f64) -> Vec<Candidates> {
        let mut result = Vec::new();
        layer
            .visit(bounds, gap, slack, &AtomicBool::new(false), |item| {
                if !matches!(item, Candidates::Charge(_)) {
                    result.push(item);
                }
                Ok(())
            })
            .unwrap();
        result
    }

    fn verify_ranges(paths: &[DepositionPath], query: [[f64; 2]; 2], gap: f64, slack: f64) {
        let cancel = AtomicBool::new(false);
        let index = ClearanceIndex::build(std::iter::once(paths), usize::MAX, &mut 0, &cancel)
            .unwrap()
            .unwrap();
        let mut cursor = 0;
        for item in collect(index.layer(0).unwrap(), query, gap, slack) {
            let (count, nonmodel, skipped) = match item {
                Candidates::Charge(_) => unreachable!(),
                Candidates::Visit(range) => {
                    assert_eq!(range.paths.start, cursor);
                    (range.paths.len(), range.nonmodel_paths, false)
                }
                Candidates::Skip {
                    paths,
                    nonmodel_paths,
                } => (paths, nonmodel_paths, true),
            };
            let slice = &paths[cursor..cursor + count];
            assert_eq!(
                nonmodel,
                slice.iter().filter(|path| !path.kind.is_model()).count()
            );
            if skipped {
                for path in slice.iter().filter(|path| !path.kind.is_model()) {
                    let bounds = path_bounds(path, &mut 0, &cancel).unwrap().unwrap();
                    assert!(spiling_geometry::planar_bounds_clear(
                        bounds.xy,
                        query,
                        bounds.width * 0.5 + slack,
                        gap,
                    ));
                }
            }
            cursor += count;
        }
        assert_eq!(cursor, paths.len());
    }

    #[test]
    fn model_only_root_charges_one_node_then_skips() {
        let paths = (0..65)
            .map(|_| path(0.0, DepositionKind::Perimeter))
            .collect::<Vec<_>>();
        let cancel = AtomicBool::new(false);
        let mut work = 0;
        let index = ClearanceIndex::build(
            std::iter::once(paths.as_slice()),
            usize::MAX,
            &mut work,
            &cancel,
        )
        .unwrap()
        .unwrap();
        assert!(work >= paths.len());
        let mut events = Vec::new();
        index
            .layer(0)
            .unwrap()
            .visit([[f64::NAN; 2]; 2], 0.2, 1e-6, &cancel, |event| {
                events.push(event);
                Ok(())
            })
            .unwrap();
        assert_eq!(
            events,
            vec![
                Candidates::Charge(1),
                Candidates::Skip {
                    paths: paths.len(),
                    nonmodel_paths: 0,
                },
            ]
        );
    }

    #[test]
    fn remote_paths_skip_as_one_ordered_range() {
        let paths = (0..65)
            .map(|ordinal| path(100.0 + ordinal as f64, DepositionKind::Support))
            .collect::<Vec<_>>();
        let index = ClearanceIndex::build(
            std::iter::once(paths.as_slice()),
            usize::MAX,
            &mut 0,
            &AtomicBool::new(false),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            collect(index.layer(0).unwrap(), [[0.0, 0.0], [1.0, 1.0]], 0.2, 1e-6),
            vec![Candidates::Skip {
                paths: 65,
                nonmodel_paths: 65,
            }]
        );
    }

    #[test]
    fn ranges_cover_original_paths_and_admit_adhesion() {
        for count in [0, 1, 15, 16, 17, 31, 32, 33, 65, 129] {
            let paths = (0..count)
                .map(|ordinal| {
                    let kind = match ordinal % 5 {
                        0 => DepositionKind::Perimeter,
                        1 => DepositionKind::Skirt,
                        2 => DepositionKind::Brim,
                        3 => DepositionKind::SupportInterface,
                        _ => DepositionKind::Support,
                    };
                    path(ordinal as f64 - 25.0, kind)
                })
                .collect::<Vec<_>>();
            for x in [-100.0, -25.0, -0.25, 0.0, 1.0, 25.0, 100.0] {
                verify_ranges(&paths, [[x, -0.1], [x + 0.1, 0.1]], 0.2, 1e-6);
            }
        }
    }

    #[test]
    fn malformed_or_unknown_paths_force_their_original_leaf() {
        for variant in 0..7 {
            let mut paths = (0..65)
                .map(|_| path(100.0, DepositionKind::Support))
                .collect::<Vec<_>>();
            match variant {
                0 => paths[20].points_mm.clear(),
                1 => paths[20].width = WidthProfile::PerEdge { widths_mm: vec![] },
                2 => paths[20].width = WidthProfile::Constant { width_mm: f64::NAN },
                3 => paths[20].points_mm[0][0] = f64::INFINITY,
                4 => paths[20].points_mm[0][1] = f64::NAN,
                5 => paths[20].points_mm[0][2] = f64::INFINITY,
                _ => paths[20].physical_z_mm = [0.0, f64::NAN],
            }
            let index = ClearanceIndex::build(
                std::iter::once(paths.as_slice()),
                usize::MAX,
                &mut 0,
                &AtomicBool::new(false),
            )
            .unwrap()
            .unwrap();
            let ranges = collect(index.layer(0).unwrap(), [[0.0, 0.0], [1.0, 1.0]], 0.2, 1e-6);
            assert!(ranges.iter().any(|item| matches!(
                item,
                Candidates::Visit(range) if range.paths.contains(&20)
            )));
            verify_ranges(&paths, [[0.0, 0.0], [1.0, 1.0]], 0.2, 1e-6);
        }
    }

    #[test]
    fn widest_later_edge_and_xyz6_slack_prevent_rejection() {
        let mut path = path(1.0, DepositionKind::Support);
        path.points_mm.push([2.0, 1.0, 0.2]);
        path.width = WidthProfile::PerEdge {
            widths_mm: vec![0.5, 2.0],
        };
        let paths = [path];
        let index = ClearanceIndex::build(
            std::iter::once(paths.as_slice()),
            usize::MAX,
            &mut 0,
            &AtomicBool::new(false),
        )
        .unwrap()
        .unwrap();
        let query = [[-1.0, -1.0], [0.0, 1.0]];
        assert!(matches!(
            collect(index.layer(0).unwrap(), query, 0.0, 1e-6).as_slice(),
            [Candidates::Visit(_)]
        ));
        for gap in [0.0, 0.2, 0.5] {
            for offset in [0.0_f64, 1e-6, 0.1] {
                let edge = 1.0 - gap - offset;
                for x in [edge.next_down(), edge, edge.next_up()] {
                    verify_ranges(&paths, [[-1.0, -1.0], [x, 1.0]], gap, offset);
                }
            }
        }
    }

    #[test]
    fn unknown_queries_and_invalid_screen_parameters_never_skip() {
        let paths = [path(100.0, DepositionKind::Support)];
        let index = ClearanceIndex::build(
            std::iter::once(paths.as_slice()),
            usize::MAX,
            &mut 0,
            &AtomicBool::new(false),
        )
        .unwrap()
        .unwrap();
        for (query, gap, slack) in [
            ([[f64::NAN, 0.0], [1.0, 1.0]], 0.2, 1e-6),
            ([[2.0, 0.0], [1.0, 1.0]], 0.2, 1e-6),
            ([[0.0, 0.0], [1.0, 1.0]], -1.0, 1e-6),
            ([[0.0, 0.0], [1.0, 1.0]], 0.2, -1.0),
            ([[0.0, 0.0], [1.0, 1.0]], f64::INFINITY, 1e-6),
        ] {
            assert!(matches!(
                collect(index.layer(0).unwrap(), query, gap, slack).as_slice(),
                [Candidates::Visit(_)]
            ));
        }
    }

    #[test]
    fn allowance_covers_headers_nodes_and_empty_layers() {
        let paths = (0..33)
            .map(|_| path(100.0, DepositionKind::Support))
            .collect::<Vec<_>>();
        let empty = &[][..];
        let layers = [empty, paths.as_slice(), empty];
        let cancel = AtomicBool::new(false);
        let index = ClearanceIndex::build(layers.into_iter(), usize::MAX, &mut 0, &cancel)
            .unwrap()
            .unwrap();
        let bytes = index.retained_bytes();
        assert!(
            ClearanceIndex::build(layers.into_iter(), bytes - 1, &mut 0, &cancel)
                .unwrap()
                .is_none()
        );
        let exact = ClearanceIndex::build(layers.into_iter(), bytes, &mut 0, &cancel)
            .unwrap()
            .unwrap();
        assert!(exact.retained_bytes() <= bytes);
        assert!(exact.layer(3).is_none());
        assert!(collect(exact.layer(0).unwrap(), [[0.0; 2]; 2], 0.2, 1e-6).is_empty());
        assert!(
            ClearanceIndex::build(layers.into_iter(), 0, &mut 0, &cancel)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn callback_observes_the_original_first_refusal() {
        fn first_refusal(paths: &[DepositionPath], indexed: bool) -> ManufacturingError {
            let cancel = AtomicBool::new(false);
            let mut cursor = 0;
            let mut inspect = |item: Candidates| {
                match item {
                    Candidates::Charge(_) => {}
                    Candidates::Skip { paths, .. } => cursor += paths,
                    Candidates::Visit(range) => {
                        assert_eq!(cursor, range.paths.start);
                        for ordinal in range.paths {
                            let path = &paths[ordinal];
                            cursor = ordinal + 1;
                            if path.kind.is_model() {
                                continue;
                            }
                            path.checked_edges()?;
                            let bounds = path_bounds(path, &mut 0, &cancel)?.unwrap();
                            if !spiling_geometry::planar_bounds_clear(
                                bounds.xy,
                                [[0.0, 0.0], [1.0, 1.0]],
                                bounds.width * 0.5 + 1e-6,
                                0.2,
                            ) {
                                return Err(ManufacturingError::new(
                                    ManufacturingErrorCode::NativeAnalysisIndeterminate,
                                    format!("caller narrow refusal at path {ordinal}"),
                                ));
                            }
                        }
                    }
                }
                Ok(())
            };
            if indexed {
                let index =
                    ClearanceIndex::build(std::iter::once(paths), usize::MAX, &mut 0, &cancel)
                        .unwrap()
                        .unwrap();
                index
                    .layer(0)
                    .unwrap()
                    .visit([[0.0, 0.0], [1.0, 1.0]], 0.2, 1e-6, &cancel, inspect)
                    .unwrap_err()
            } else {
                inspect(Candidates::Visit(PathRange {
                    paths: 0..paths.len(),
                    nonmodel_paths: paths.len(),
                }))
                .unwrap_err()
            }
        }

        for (near, malformed) in [(20, 40), (40, 20)] {
            let mut paths = (0..65)
                .map(|_| path(100.0, DepositionKind::Support))
                .collect::<Vec<_>>();
            paths[near] = path(0.0, DepositionKind::Support);
            paths[malformed].width = WidthProfile::PerEdge { widths_mm: vec![] };
            let scalar = first_refusal(&paths, false);
            let indexed = first_refusal(&paths, true);
            assert_eq!(scalar.code, indexed.code);
            assert_eq!(scalar.message, indexed.message);
        }
    }

    #[test]
    fn cancellation_between_layers_discards_partial_build() {
        let paths = [path(100.0, DepositionKind::Support)];
        let cancel = AtomicBool::new(false);
        let layers = [paths.as_slice(), paths.as_slice()];
        let layers = layers.into_iter().enumerate().map(|(ordinal, paths)| {
            if ordinal == 1 {
                cancel.store(true, Ordering::Relaxed);
            }
            paths
        });
        let error = ClearanceIndex::build(layers, usize::MAX, &mut 0, &cancel)
            .err()
            .unwrap();
        assert_eq!(error.code, ManufacturingErrorCode::Cancelled);
    }

    #[test]
    fn cancellation_and_callback_failure_stop_ordered_traversal() {
        let paths = (0..65)
            .map(|_| path(100.0, DepositionKind::Support))
            .collect::<Vec<_>>();
        let cancel = AtomicBool::new(true);
        assert_eq!(
            ClearanceIndex::build(
                std::iter::once(paths.as_slice()),
                usize::MAX,
                &mut 0,
                &cancel
            )
            .err()
            .unwrap()
            .code,
            ManufacturingErrorCode::Cancelled
        );
        cancel.store(false, Ordering::Relaxed);
        let index = ClearanceIndex::build(
            std::iter::once(paths.as_slice()),
            usize::MAX,
            &mut 0,
            &cancel,
        )
        .unwrap()
        .unwrap();
        let mut visited = Vec::new();
        let error = index
            .layer(0)
            .unwrap()
            .visit([[f64::NAN; 2]; 2], 0.2, 1e-6, &cancel, |item| {
                visited.push(item);
                cancel.store(true, Ordering::Relaxed);
                Ok(())
            })
            .unwrap_err();
        assert_eq!(error.code, ManufacturingErrorCode::Cancelled);
        assert_eq!(visited, vec![Candidates::Charge(1)]);
        cancel.store(false, Ordering::Relaxed);
        let error = index
            .layer(0)
            .unwrap()
            .visit([[f64::NAN; 2]; 2], 0.2, 1e-6, &cancel, |_| {
                Err(ManufacturingError::new(
                    ManufacturingErrorCode::NativeAnalysisIndeterminate,
                    "first caller refusal",
                ))
            })
            .unwrap_err();
        assert_eq!(error.message, "first caller refusal");
    }
}
