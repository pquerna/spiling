// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::anchors::AnchorGap;
use super::{AnchorCandidate, AnchorWorkspace};
use crate::check_cancel;
use crate::planner::OFFSET_TOLERANCE_MM;
#[cfg(test)]
use spiling_contracts::manufacturing::WidthProfile;
use spiling_contracts::manufacturing::{
    DepositionEdges, DepositionKind, DepositionPath, ManufacturingError, PlanarPrintRecipe,
};
use std::sync::atomic::AtomicBool;

pub(crate) enum GapKind {
    Supported,
    Bridge,
    Contact,
    Demand,
}

pub(crate) fn gap_kind(gap: &AnchorGap, span: f64, width: f64, limit: f64) -> GapKind {
    let left = gap.left;
    let right = gap.right;
    // A fully unanchored tiny sampled edge is not a supported overhang. Anchors
    // must be actual positive intervals or certified endpoint-footprint overlap.
    if left || right {
        if span <= 2.0 * OFFSET_TOLERANCE_MM {
            return GapKind::Supported;
        }
        if !(left && right) && span <= width * 0.5 + 2.0 * OFFSET_TOLERANCE_MM {
            return GapKind::Supported;
        }
    }
    if left && right && span <= limit {
        if gap.direct_ends {
            GapKind::Bridge
        } else {
            GapKind::Contact
        }
    } else {
        GapKind::Demand
    }
}

pub(in crate::planner) struct Failure {
    pub(in crate::planner) path: usize,
    pub(in crate::planner) first: usize,
    pub(in crate::planner) last: usize,
    pub(in crate::planner) width_mm: f64,
    pub(in crate::planner) start: f64,
    pub(in crate::planner) end: f64,
    pub(in crate::planner) span: f64,
}
pub(in crate::planner) struct Admission {
    pub(in crate::planner) bridges: Vec<usize>,
    pub(in crate::planner) failure: Option<Failure>,
}

pub(in crate::planner) fn analyze<'a>(
    paths: &[DepositionPath],
    previous: impl Iterator<Item = &'a DepositionPath> + Clone,
    contacts: impl Iterator<Item = &'a DepositionPath> + Clone,
    recipe: &PlanarPrintRecipe,
    anchors: &mut AnchorWorkspace,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Admission, ManufacturingError> {
    let mut result = Admission {
        bridges: Vec::new(),
        failure: None,
    };
    for (path_index, path) in paths.iter().enumerate() {
        check_cancel(cancel)?;
        let mut has_bridge = false;
        for (first, last, width_mm) in StrokeRuns::for_path(path)? {
            let a = path.points_mm[first];
            let b = path.points_mm[last];
            let length = (b[0] - a[0]).hypot(b[1] - a[1]);
            let mut bridge = false;
            let mut demand = false;
            for gap in anchors.gaps(
                AnchorCandidate {
                    endpoints_mm: [a, b],
                    bead_width_mm: width_mm,
                },
                previous
                    .clone()
                    .filter(|lower| lower.physical_z_mm[1] == path.physical_z_mm[0]),
                contacts
                    .clone()
                    .filter(|lower| lower.physical_z_mm[1] <= path.physical_z_mm[0]),
                work,
                cancel,
            )? {
                let start = gap.start;
                let end = gap.end;
                let span = (end - start) * length;
                if path.kind == DepositionKind::SeparatorWall
                    && !matches!(
                        gap_kind(gap, span, width_mm, recipe.process.max_bridge_span_mm),
                        GapKind::Supported
                    )
                {
                    return Err(crate::error(
                        spiling_contracts::manufacturing::ManufacturingErrorCode::UnsupportedGeometry,
                        "UnsupportedInterface: permanent separator needs actual lower strokes; Bridge/support substitution is not authorized",
                    ));
                }
                match gap_kind(gap, span, width_mm, recipe.process.max_bridge_span_mm) {
                    GapKind::Supported => {}
                    // Preserve eligible contact provenance. Only a consumed,
                    // explicitly constructed Bridge uses bridge process here;
                    // ordinary contact-supported rows stay ordinary.
                    GapKind::Contact => bridge |= path.kind == DepositionKind::Bridge,
                    GapKind::Bridge => bridge = true,
                    GapKind::Demand => {
                        demand = true;
                        if result.failure.is_none() {
                            result.failure = Some(Failure {
                                path: path_index,
                                first,
                                last,
                                width_mm,
                                start,
                                end,
                                span,
                            });
                        }
                    }
                }
            }
            if !demand {
                has_bridge |= bridge;
            }
        }
        if has_bridge {
            result.bridges.push(path_index);
        }
    }
    Ok(result)
}

/// Maximal straight strokes, including across the seam of a closed contour.
/// Native points are never removed or moved. Only floating arithmetic roundoff
/// in the two-product cross product admits collinearity; curved edges stay distinct.
pub(crate) struct StrokeRuns<'a> {
    points: &'a [[f64; 3]],
    widths: DepositionEdges<'a>,
    closed: bool,
    seam: usize,
    consumed: usize,
    segments: usize,
}
impl<'a> StrokeRuns<'a> {
    pub(crate) fn for_path(path: &'a DepositionPath) -> Result<Self, ManufacturingError> {
        let widths = path.checked_edges()?;
        let points = &path.points_mm;
        let segments = widths.len();
        let mut result = Self {
            points,
            widths,
            segments,
            closed: segments > 1 && points.first() == points.last(),
            seam: 0,
            consumed: 0,
        };
        if result.closed {
            result.seam = (0..result.segments)
                .find(|&i| {
                    let previous = (i + result.segments - 1) % result.segments;
                    result.width(previous) != result.width(i)
                        || !straight(
                            result.points[previous],
                            result.points[i],
                            result.points[(i + 1) % result.segments],
                        )
                })
                .unwrap_or(0);
        }
        Ok(result)
    }
    fn width(&self, edge: usize) -> f64 {
        self.widths
            .edge(edge)
            .expect("stroke run stays within its validated edge view")
            .width_mm
    }
    fn vertex(&self, offset: usize) -> usize {
        if self.closed {
            (self.seam + offset) % self.segments
        } else {
            offset
        }
    }
}
impl Iterator for StrokeRuns<'_> {
    type Item = (usize, usize, f64);
    fn next(&mut self) -> Option<Self::Item> {
        if self.consumed == self.segments {
            return None;
        }
        let first = self.vertex(self.consumed);
        let width_mm = self.width(first);
        self.consumed += 1;
        let mut last = self.vertex(self.consumed);
        while self.consumed < self.segments {
            let next = self.vertex(self.consumed + 1);
            if self.width(last) != width_mm
                || !straight(self.points[first], self.points[last], self.points[next])
            {
                break;
            }
            self.consumed += 1;
            last = next;
        }
        Some((first, last, width_mm))
    }
}
fn straight(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> bool {
    let u = [b[0] - a[0], b[1] - a[1]];
    let v = [c[0] - b[0], c[1] - b[1]];
    let p = u[0] * v[1];
    let q = u[1] * v[0];
    u[0] * v[0] + u[1] * v[1] > 0.0 && (p - q).abs() <= 8.0 * f64::EPSILON * (p.abs() + q.abs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampled_closed_straight_strokes_cross_the_original_seam_without_mutation() {
        let points = [
            [21.0, 0.3, 8.4],
            [23.0, 0.3, 8.4],
            [30.7, 0.3, 8.4],
            [30.7, 9.7, 8.4],
            [11.3, 9.7, 8.4],
            [11.3, 0.3, 8.4],
            [20.0, 0.3, 8.4],
            [21.0, 0.3, 8.4],
        ];
        let path = DepositionPath {
            kind: DepositionKind::Perimeter,
            owner: Some(spiling_contracts::manufacturing::DepositionOwner::Wall),
            width: WidthProfile::Constant { width_mm: 0.6 },
            physical_z_mm: [8.0, 8.4],
            points_mm: points.to_vec(),
        };
        let runs: Vec<_> = StrokeRuns::for_path(&path).unwrap().collect();
        assert_eq!(runs, [(2, 3, 0.6), (3, 4, 0.6), (4, 5, 0.6), (5, 2, 0.6)]);
        assert_eq!(points.first(), points.last());
        let anchors = [
            DepositionPath {
                kind: spiling_contracts::manufacturing::DepositionKind::SolidFill,
                width: WidthProfile::Constant { width_mm: 0.6 },
                physical_z_mm: [7.6, 8.0],
                owner: None,
                points_mm: vec![[0.3, 0.3, 8.0], [19.7, 0.3, 8.0]],
            },
            DepositionPath {
                kind: spiling_contracts::manufacturing::DepositionKind::SolidFill,
                width: WidthProfile::Constant { width_mm: 0.6 },
                physical_z_mm: [7.6, 8.0],
                owner: None,
                points_mm: vec![[23.3, 0.3, 8.0], [42.7, 0.3, 8.0]],
            },
        ];
        let mut query = AnchorWorkspace::default();
        let gaps = query
            .gaps(
                AnchorCandidate {
                    endpoints_mm: [points[5], points[2]],
                    bead_width_mm: 0.6,
                },
                anchors.iter(),
                std::iter::empty(),
                &mut 0,
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(gaps.len(), 1);
        assert!(((gaps[0].end - gaps[0].start) * 19.4 - 3.0).abs() < 1e-12);
        assert!(gaps[0].direct_ends);
    }

    #[test]
    fn curved_strokes_are_not_flattened_by_native_sample_aggregation() {
        let points = [[0.0, 0.0, 0.4], [1.0, 0.001, 0.4], [2.0, 0.0, 0.4]];
        let path = DepositionPath {
            kind: DepositionKind::Perimeter,
            owner: Some(spiling_contracts::manufacturing::DepositionOwner::Wall),
            width: WidthProfile::Constant { width_mm: 0.6 },
            physical_z_mm: [0.0, 0.4],
            points_mm: points.to_vec(),
        };
        assert_eq!(
            StrokeRuns::for_path(&path).unwrap().collect::<Vec<_>>(),
            [(0, 1, 0.6), (1, 2, 0.6)]
        );
    }

    #[test]
    fn contact_spans_keep_the_same_limit_without_false_direct_bridge_role() {
        let classify = |start: f64, end: f64, span, direct_ends, width, limit| {
            gap_kind(
                &AnchorGap {
                    start,
                    end,
                    left: start > 1e-8,
                    right: end < 1.0 - 1e-8,
                    direct_ends,
                },
                span,
                width,
                limit,
            )
        };
        assert!(matches!(
            classify(0.23, 0.77, 5.4, false, 0.6, 6.0),
            GapKind::Contact
        ));
        assert!(matches!(
            classify(0.23, 0.77, 5.4, true, 0.6, 6.0),
            GapKind::Bridge
        ));
        assert!(matches!(
            classify(0.23, 0.77, 5.4, false, 0.6, 5.0),
            GapKind::Demand
        ));
        assert!(matches!(
            classify(0.0, 1.0, 0.001, false, 0.6, 6.0),
            GapKind::Demand
        ));
        assert!(matches!(
            classify(0.0, 0.1, 0.1, false, 0.6, 6.0),
            GapKind::Supported
        ));
    }

    #[test]
    fn endpoint_footprint_positive_area_certifies_bridge_without_dilating_interior_capsules() {
        use spiling_contracts::manufacturing::DepositionKind;
        let previous = [0.0, 2.0].map(|x| DepositionPath {
            kind: DepositionKind::SparseFill,
            width: WidthProfile::Constant { width_mm: 0.5 },
            physical_z_mm: [0.0, 0.4],
            owner: None,
            points_mm: vec![[x, -2.0, 0.4], [x, 2.0, 0.4]],
        });
        let path = DepositionPath {
            kind: DepositionKind::TopSkin,
            width: WidthProfile::Constant { width_mm: 0.5 },
            physical_z_mm: [0.4, 0.8],
            owner: Some(
                spiling_contracts::manufacturing::DepositionOwner::MandatorySkin {
                    skin: spiling_contracts::manufacturing::ProcessSkinKind::TopSkin,
                },
            ),
            points_mm: vec![[0.4, 0.0, 0.8], [1.6, 0.0, 0.8]],
        };
        let cancel = AtomicBool::new(false);
        let mut query = AnchorWorkspace::default();
        let gaps = query
            .gaps(
                AnchorCandidate {
                    endpoints_mm: [path.points_mm[0], path.points_mm[1]],
                    bead_width_mm: 0.5,
                },
                previous.iter(),
                std::iter::empty(),
                &mut 0,
                &cancel,
            )
            .unwrap();
        assert_eq!(gaps.len(), 1);
        // Neither centerline enters an original half-width .25 anchor capsule.
        // Both actual endpoint disks overlap positively at distance .4 < .5.
        assert_eq!((gaps[0].start, gaps[0].end), (0.0, 1.0));
        assert!(gaps[0].left && gaps[0].right && gaps[0].direct_ends);
        let mut recipe = crate::planner::tests::recipe(1.0);
        recipe.bead_width_mm = 0.5;
        recipe.process.max_bridge_span_mm = 1.25;
        let admitted = analyze(
            std::slice::from_ref(&path),
            previous.iter(),
            std::iter::empty(),
            &recipe,
            &mut query,
            &mut 0,
            &cancel,
        )
        .unwrap();
        assert!(admitted.failure.is_none());
        assert_eq!(admitted.bridges, [0]);
        recipe.process.max_bridge_span_mm = 1.0;
        let refused = analyze(
            std::slice::from_ref(&path),
            previous.iter(),
            std::iter::empty(),
            &recipe,
            &mut query,
            &mut 0,
            &cancel,
        )
        .unwrap();
        assert!((refused.failure.unwrap().span - 1.2).abs() < 1e-12);
        assert!(refused.bridges.is_empty());
        recipe.process.max_bridge_span_mm = 1.25;
        let contact = analyze(
            std::slice::from_ref(&path),
            previous[..1].iter(),
            previous[1..].iter(),
            &recipe,
            &mut query,
            &mut 0,
            &cancel,
        )
        .unwrap();
        assert!(contact.failure.is_none());
        assert!(contact.bridges.is_empty());
        assert_eq!(path.kind, DepositionKind::TopSkin);
    }

    #[test]
    fn endpoint_footprint_tangency_and_absence_never_certify_unanchored_strokes() {
        use spiling_contracts::manufacturing::DepositionKind;
        let previous = [0.0, 2.0].map(|x| DepositionPath {
            kind: DepositionKind::SparseFill,
            width: WidthProfile::Constant { width_mm: 0.5 },
            physical_z_mm: [0.0, 0.4],
            owner: None,
            points_mm: vec![[x, -2.0, 0.4], [x, 2.0, 0.4]],
        });
        let cancel = AtomicBool::new(false);
        let mut query = AnchorWorkspace::default();
        let mut recipe = crate::planner::tests::recipe(1.0);
        recipe.bead_width_mm = 0.5;
        recipe.process.max_bridge_span_mm = 10.0;
        for (a, b) in [(0.5, 1.5), (0.51, 1.49)] {
            let path = DepositionPath {
                kind: DepositionKind::TopSkin,
                width: WidthProfile::Constant { width_mm: 0.5 },
                physical_z_mm: [0.4, 0.8],
                owner: Some(
                    spiling_contracts::manufacturing::DepositionOwner::MandatorySkin {
                        skin: spiling_contracts::manufacturing::ProcessSkinKind::TopSkin,
                    },
                ),
                points_mm: vec![[a, 0.0, 0.8], [b, 0.0, 0.8]],
            };
            let gaps = query
                .gaps(
                    AnchorCandidate {
                        endpoints_mm: [path.points_mm[0], path.points_mm[1]],
                        bead_width_mm: 0.5,
                    },
                    previous.iter(),
                    std::iter::empty(),
                    &mut 0,
                    &cancel,
                )
                .unwrap();
            assert_eq!(gaps.len(), 1);
            assert_eq!((gaps[0].start, gaps[0].end), (0.0, 1.0));
            assert!(!gaps[0].left && !gaps[0].right && !gaps[0].direct_ends);
            let refused = analyze(
                std::slice::from_ref(&path),
                previous.iter(),
                std::iter::empty(),
                &recipe,
                &mut query,
                &mut 0,
                &cancel,
            )
            .unwrap();
            assert!(refused.failure.is_some());
            assert!(refused.bridges.is_empty());
        }
    }
}

#[cfg(test)]
mod dimension_tests {
    use super::*;

    #[test]
    fn admission_uses_actual_width_and_only_physically_adjacent_direct_strokes() {
        let cancel = AtomicBool::new(false);
        let mut recipe = crate::planner::tests::recipe(1.0);
        recipe.bead_width_mm = 0.8;
        let path = DepositionPath {
            kind: DepositionKind::SparseFill,
            owner: Some(
                spiling_contracts::manufacturing::DepositionOwner::Regional { region_index: 0 },
            ),
            width: WidthProfile::Constant { width_mm: 0.2 },
            physical_z_mm: [0.4, 0.5],
            points_mm: vec![[0.0, 0.0, 0.5], [0.4, 0.0, 0.5]],
        };
        let mut lower = DepositionPath {
            kind: DepositionKind::SparseFill,
            owner: Some(
                spiling_contracts::manufacturing::DepositionOwner::Regional { region_index: 0 },
            ),
            width: WidthProfile::Constant { width_mm: 0.2 },
            physical_z_mm: [0.3, 0.4],
            points_mm: vec![[0.0, -1.0, 0.4], [0.0, 1.0, 0.4]],
        };
        let mut anchors = AnchorWorkspace::default();
        let first = analyze(
            std::slice::from_ref(&path),
            std::iter::once(&lower),
            std::iter::empty(),
            &recipe,
            &mut anchors,
            &mut 0,
            &cancel,
        )
        .unwrap();
        assert!(
            first.failure.is_some(),
            "recipe width must not authorize a narrower stroke's overhang"
        );
        lower.width = WidthProfile::Constant { width_mm: 1.0 };
        let adjacent = analyze(
            std::slice::from_ref(&path),
            std::iter::once(&lower),
            std::iter::empty(),
            &recipe,
            &mut anchors,
            &mut 0,
            &cancel,
        )
        .unwrap();
        assert!(adjacent.failure.is_none());
        lower.physical_z_mm = [0.2, 0.3];
        lower.points_mm.iter_mut().for_each(|point| point[2] = 0.3);
        let separated = analyze(
            std::slice::from_ref(&path),
            std::iter::once(&lower),
            std::iter::empty(),
            &recipe,
            &mut anchors,
            &mut 0,
            &cancel,
        )
        .unwrap();
        assert!(
            separated.failure.is_some(),
            "a nonadjacent previous capsule is not a direct anchor"
        );
    }
}
