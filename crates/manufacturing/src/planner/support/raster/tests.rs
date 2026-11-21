// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::super::grid::{CellFrame, SUPPORT_CELL_MM};
use super::*;
use geo::{LineString, Polygon};
use spiling_contracts::manufacturing::program_coordinates::project_xyz6_coordinate_mm;
use std::sync::atomic::AtomicUsize;

static ADMIT: fn(usize) -> Result<(), ManufacturingError> = |_| Ok(());

fn raster(cancel: &AtomicBool) -> Raster<'_> {
    Raster {
        frame: CellFrame::new([[-2.0, -2.0], [40.0, 40.0]], SUPPORT_CELL_MM).unwrap(),
        cancel,
        admit: &ADMIT,
        work: AtomicUsize::new(0),
    }
}

fn rectangle(x: f64, y: f64, width: f64, height: f64) -> MultiPolygon<f64> {
    MultiPolygon(vec![Polygon::new(
        LineString::from(vec![
            (x, y),
            (x + width, y),
            (x + width, y + height),
            (x, y + height),
            (x, y),
        ]),
        vec![],
    )])
}

fn cells(raster: &Raster<'_>, region: &MultiPolygon<f64>) -> Bits {
    raster.inward(&region.0).unwrap()
}

fn hatch_layer(
    recipe: &PlanarPrintRecipe,
    axis: region::HatchAxis,
    z: f64,
    height: f64,
) -> region::HatchLayer<'_> {
    region::HatchLayer {
        roof: None,
        width_mm: recipe.bead_width_mm,
        z,
        lower_z: z - height,
        height,
        axis,
        top_axis: axis,
        recipe,
    }
}

fn stage(
    raster: &Raster<'_>,
    support: &Bits,
    interface: &Bits,
    density: f64,
    layer: &region::HatchLayer<'_>,
) -> (Vec<DepositionPath>, WallReceipt) {
    paths(
        raster,
        support,
        interface,
        density,
        layer,
        0,
        0,
        0,
        &mut 0,
        0,
        raster.cancel,
    )
    .unwrap()
}

fn support_path(points: &[[f64; 2]], bottom: f64, top: f64, shift: f64) -> DepositionPath {
    DepositionPath {
        material_domain: None,
        kind: DepositionKind::Support,
        width: WidthProfile::Constant { width_mm: 0.6 },
        physical_z_mm: [bottom, top],
        owner: None,
        points_mm: points
            .iter()
            .map(|point| [point[0] + shift, point[1] + shift, top])
            .collect(),
        arcs: Vec::new(),
        exposed: false,
        connectors: Vec::new(),
    }
}

#[test]
fn unrooted_islands_cannot_erase_an_independent_deposited_dependency_chain() {
    let recipe = crate::planner::tests::recipe(1.0);
    let cancel = AtomicBool::new(false);
    for shift in [0.0, 0.37] {
        let row = |start: [f64; 2], end: [f64; 2], bottom, top| {
            support_path(&[start, end], bottom, top, shift)
        };
        let lower = [0.0, 2.0, 4.0, 6.0].map(|offset| row([0.0, offset], [10.0, offset], 0.0, 0.2));
        let rooted = row([5.0, 0.0], [5.0, 6.0], 0.2, 0.4);
        let floating = support_path(
            &[
                [20.0, 0.0],
                [21.0, 0.0],
                [21.0, 1.0],
                [20.0, 1.0],
                [20.0, 0.0],
            ],
            0.2,
            0.4,
            shift,
        );
        let overextended = row([7.0, 0.0], [7.0, 8.0], 0.2, 0.4);
        let mut candidate = vec![floating, rooted.clone(), overextended];
        let mut anchors = AnchorWorkspace::default();
        let mut work = 0;
        let omitted = retain_anchored(
            &mut candidate,
            1,
            &mut strokes::LowerStrokes::new(lower.iter(), std::iter::empty()),
            &recipe,
            0,
            0,
            &mut anchors,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert_eq!(candidate, [rooted]);
        assert!((omitted.omitted_mm - 4.0).abs() <= crate::planner::OFFSET_TOLERANCE_MM);
        let upper = [row([5.0, 1.0], [5.0, 5.0], 0.4, 0.6)];
        assert!(
            scan(
                &upper,
                candidate.iter(),
                std::iter::empty(),
                &recipe,
                &mut anchors,
                &mut work,
                &cancel,
                true
            )
            .unwrap()
            .is_none()
        );
        let cancelled = retain_anchored(
            &mut candidate,
            0,
            &mut strokes::LowerStrokes::new(lower.iter(), std::iter::empty()),
            &recipe,
            0,
            0,
            &mut anchors,
            &mut work,
            &AtomicBool::new(true),
        )
        .unwrap_err();
        assert_eq!(cancelled.code, ManufacturingErrorCode::Cancelled);
    }
}

#[test]
fn a_partially_rooted_closed_wall_keeps_roads_on_both_sides_of_its_seam() {
    let recipe = crate::planner::tests::recipe(1.0);
    let cancel = AtomicBool::new(false);
    for shift in [0.0, 0.37] {
        let lower = support_path(&[[0.0, 0.0], [10.0, 0.0]], 0.0, 0.2, shift);
        let mut candidate = vec![support_path(
            &[
                [5.0, 0.0],
                [10.0, 0.0],
                [10.0, 6.0],
                [0.0, 6.0],
                [0.0, 0.0],
                [5.0, 0.0],
            ],
            0.2,
            0.4,
            shift,
        )];
        let mut anchors = AnchorWorkspace::default();
        let mut work = 0;
        let omitted = retain_anchored(
            &mut candidate,
            1,
            &mut strokes::LowerStrokes::new(std::iter::once(&lower), std::iter::empty()),
            &recipe,
            0,
            0,
            &mut anchors,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!((omitted.omitted_mm - 22.0).abs() <= crate::planner::OFFSET_TOLERANCE_MM);
        let retained_length: f64 = candidate
            .iter()
            .flat_map(|path| path.points_mm.windows(2))
            .map(|edge| (edge[1][0] - edge[0][0]).hypot(edge[1][1] - edge[0][1]))
            .sum();
        assert!((retained_length - 10.0).abs() <= crate::planner::OFFSET_TOLERANCE_MM);
        let upper = support_path(&[[1.0, 0.0], [9.0, 0.0]], 0.4, 0.6, shift);
        assert!(
            scan(
                std::slice::from_ref(&upper),
                candidate.iter(),
                std::iter::empty(),
                &recipe,
                &mut anchors,
                &mut work,
                &cancel,
                true
            )
            .unwrap()
            .is_none()
        );
    }
}

#[test]
fn every_staged_bead_stays_inside_the_kept_support_cells() {
    let recipe = crate::planner::tests::recipe(1.0);
    let cancel = AtomicBool::new(false);
    let raster = raster(&cancel);
    let half = recipe.bead_width_mm * 0.5;
    // An L-shaped region with a hole: reentrant corners and a hole wall.
    let mut region = rectangle(0.13, 0.27, 12.0, 4.0);
    region.0.extend(rectangle(0.13, 4.27, 4.0, 8.0).0);
    let support = raster
        .difference(
            &cells(&raster, &region),
            &raster
                .outward(&rectangle(6.0, 1.5, 2.0, 1.0).0, 0.0)
                .unwrap(),
        )
        .unwrap();
    for axis in [region::HatchAxis::X, region::HatchAxis::Y] {
        let (paths, receipt) = stage(
            &raster,
            &support,
            &Bits::default(),
            0.3,
            &hatch_layer(&recipe, axis, 0.4, 0.2),
        );
        assert!(receipt.paths > 0);
        assert!(paths.len() > receipt.paths);
        // Each projected centreline point's closed bead disk meets no cell
        // outside the kept set.
        let outside = |x: f64, y: f64| {
            let mut escaped = raster.capsules([([x, y], [x, y], half)]).unwrap();
            escaped.subtract(&support);
            !escaped.is_empty()
        };
        for path in &paths {
            for point in &path.points_mm {
                let x = project_xyz6_coordinate_mm(point[0]).unwrap();
                let y = project_xyz6_coordinate_mm(point[1]).unwrap();
                assert!(!outside(x, y), "bead at ({x}, {y}) leaves the kept cells");
            }
        }
        // Rows follow the preferred axis only.
        let fixed = usize::from(axis == region::HatchAxis::X);
        for path in &paths[receipt.paths..] {
            assert_eq!(path.points_mm.len(), 2);
            assert_eq!(path.points_mm[0][fixed], path.points_mm[1][fixed]);
        }
    }
}

#[test]
fn support_rows_keep_their_physical_lattice_across_clipping_and_thinner_layers() {
    let recipe = crate::planner::tests::recipe(1.0);
    let cancel = AtomicBool::new(false);
    let raster = raster(&cancel);
    for axis in [region::HatchAxis::X, region::HatchAxis::Y] {
        for density in [0.3, 1.0] {
            let generate = |domain: &MultiPolygon<f64>, height| {
                let mut paths = Vec::new();
                let layer = region::HatchLayer {
                    width_mm: 0.6,
                    ..hatch_layer(&recipe, axis, 0.8, height)
                };
                hatch(
                    &raster,
                    &cells(&raster, domain),
                    DepositionKind::Support,
                    density,
                    &layer,
                    &mut paths,
                    &mut 0,
                    &cancel,
                )
                .unwrap();
                paths
            };
            let original = generate(&rectangle(0.0, 0.0, 20.0, 20.0), 0.4);
            let thinner = generate(&rectangle(1.13, 0.27, 17.2, 18.0), 0.2);
            assert!(!thinner.is_empty());
            let perpendicular = usize::from(axis == region::HatchAxis::X);
            for path in thinner {
                assert_eq!(path.physical_z_mm, [0.8 - 0.2, 0.8]);
                let coordinate = path.points_mm[0][perpendicular];
                assert!(
                    original
                        .iter()
                        .any(|reference| reference.points_mm[0][perpendicular] == coordinate)
                );
            }
        }
    }
}

#[test]
fn optional_role_fragments_reach_command_fitting_and_report_omission() {
    let centers = rectangle(0.0, 0.0, 2.0, 2.0);
    let ring = WallRing {
        z: 1.0,
        lower_z: 0.0,
        bead_width_mm: 1.0,
        first_path: 0,
    };
    let mut paths = Vec::new();
    let mut segments = 0;
    let mut points = 0;
    wall_piece(
        [
            Coord { x: 0.5, y: 0.5 },
            Coord {
                x: 0.5
                    + spiling_contracts::manufacturing::program_coordinates::XYZ6_QUANTUM_MM * 0.25,
                y: 0.5,
            },
        ],
        DepositionKind::SupportInterface,
        &ring,
        &mut paths,
        &mut segments,
        &mut points,
        crate::working_set::units(),
        0,
    )
    .unwrap();
    let omitted = commands::fit(
        &centers,
        &mut paths,
        &mut segments,
        0,
        0,
        &mut 0,
        0,
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(paths.is_empty());
    assert!(omitted > 0.0);
}

#[test]
fn unpartitioned_support_contours_fit_subcommand_original_edges() {
    let centers = MultiPolygon(vec![Polygon::new(
        LineString::from(vec![
            (0.0, 0.0),
            (0.0, 2.0),
            (2.0, 2.0),
            (2.0, 0.0),
            (0.000001, 0.0),
            (0.0, 0.0),
        ]),
        Vec::new(),
    )]);
    for kind in [DepositionKind::Support, DepositionKind::SupportInterface] {
        let mut paths = Vec::new();
        let mut segments = 0;
        let cancel = AtomicBool::new(false);
        region::support_contour_paths(
            &centers,
            1.0,
            0.8,
            0.4,
            kind,
            &mut paths,
            &mut segments,
            &cancel,
        )
        .unwrap();
        let omitted = commands::fit(
            &centers,
            &mut paths,
            &mut segments,
            0,
            0,
            &mut 0,
            0,
            &cancel,
        )
        .unwrap();
        assert_eq!(omitted, 0.0);
        assert!(!paths.is_empty());
        for path in paths {
            assert_eq!(path.kind, kind);
            assert_eq!(path.points_mm.first(), path.points_mm.last());
            for edge in path.points_mm.windows(2) {
                assert!(
                    commands::command_length(
                        Coord {
                            x: edge[0][0],
                            y: edge[0][1]
                        },
                        Coord {
                            x: edge[1][0],
                            y: edge[1][1]
                        },
                    )
                    .unwrap()
                );
            }
        }
    }
}

#[test]
fn support_domain_retention_is_not_a_command_count() {
    let recipe = crate::planner::tests::recipe(1.0);
    let cancel = AtomicBool::new(false);
    let raster = raster(&cancel);
    let support = cells(&raster, &rectangle(0.0, 0.0, 2.0, 2.0));
    let layer = hatch_layer(&recipe, region::HatchAxis::X, 0.4, 0.2);
    let attempt = |retained, owned| {
        paths(
            &raster,
            &support,
            &Bits::default(),
            0.3,
            &layer,
            0,
            retained,
            owned,
            &mut 0,
            0,
            &cancel,
        )
    };
    // The former 2M command cap is not a retention limit.
    assert!(attempt(2_000_001, 0).is_ok());
    assert!(attempt(0, crate::working_set::bytes()).is_err());
}

#[test]
fn sub_bead_interface_transition_preserves_actual_union_wall_anchors() {
    let cancel = AtomicBool::new(false);
    let raster = raster(&cancel);
    let mut work = 0;
    let mut recipe = crate::planner::tests::recipe(1.0);
    recipe.layer_height_mm = 0.575;
    let support = cells(&raster, &rectangle(0.0, 0.0, 20.0, 5.0));
    let lower_interface = raster
        .intersection(
            &support,
            &cells(&raster, &rectangle(-1.0, -1.0, 22.0, 1.525)),
        )
        .unwrap();
    let upper_interface = raster
        .intersection(
            &support,
            &cells(&raster, &rectangle(-1.0, -1.0, 22.0, 2.675)),
        )
        .unwrap();
    // The lower interface strip is narrower than a bead: it holds no
    // centreline of its own, yet the union's wall and rows survive.
    assert!(
        raster
            .erode(&lower_interface, recipe.bead_width_mm * 0.5)
            .unwrap()
            .is_empty()
    );
    let (previous, _) = stage(
        &raster,
        &support,
        &lower_interface,
        1.0,
        &hatch_layer(&recipe, region::HatchAxis::Y, 0.575, 0.575),
    );
    let (current, _) = stage(
        &raster,
        &support,
        &upper_interface,
        1.0,
        &hatch_layer(&recipe, region::HatchAxis::X, 1.15, 0.575),
    );
    assert!(
        previous
            .iter()
            .any(|path| path.kind == DepositionKind::SupportInterface)
    );
    assert!(
        previous
            .iter()
            .any(|path| path.kind == DepositionKind::Support)
    );
    let mut anchors = AnchorWorkspace::default();
    anchors.set_budget(candidate_budget(&current, 0, 0).unwrap());
    assert!(
        failure(
            &current,
            previous.iter(),
            &recipe,
            &mut anchors,
            &mut work,
            &cancel
        )
        .unwrap()
        .is_none()
    );
    // Process ownership is not an anchor certificate: the identical raster
    // still fails when no actual support was deposited underneath it.
    let first = failure(
        &current,
        std::iter::empty(),
        &recipe,
        &mut anchors,
        &mut work,
        &cancel,
    )
    .unwrap()
    .unwrap();
    // The span a degraded layer reports is the worst of every stroke, not
    // the first one found.
    let worst = worst_failure(
        &current,
        std::iter::empty(),
        &recipe,
        &mut anchors,
        &mut work,
        &cancel,
    )
    .unwrap()
    .unwrap();
    assert!(worst.span >= first.span);
    let longest = current
        .iter()
        .flat_map(|path| path.points_mm.windows(2))
        .map(|edge| (edge[1][0] - edge[0][0]).hypot(edge[1][1] - edge[0][1]))
        .fold(0.0_f64, f64::max);
    assert!(worst.span >= longest - 1e-9);
    assert!(
        worst_failure(
            &current,
            previous.iter(),
            &recipe,
            &mut anchors,
            &mut work,
            &cancel
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn role_partition_preserves_each_physical_exterior_and_hole_wall_once() {
    let cancel = AtomicBool::new(false);
    let raster = raster(&cancel);
    let mut centers = rectangle(0.0, 0.0, 20.0, 5.0);
    let hole = rectangle(1.0, 2.0, 2.0, 2.0).0.remove(0);
    centers.0[0].interiors_push(hole.exterior().clone());
    let interface = cells(&raster, &rectangle(4.0, -1.0, 8.0, 2.0));
    let mut paths = Vec::new();
    let mut segments = 0;
    let recipe = crate::planner::tests::recipe(1.0);
    walls(
        &raster,
        &centers,
        &interface,
        &cells(&raster, &centers),
        &hatch_layer(&recipe, region::HatchAxis::X, 0.6, 0.6),
        &mut paths,
        &mut segments,
        0,
        0,
        &cancel,
    )
    .unwrap();
    let length: f64 = paths
        .iter()
        .flat_map(|p| p.points_mm.windows(2))
        .map(|e| (e[1][0] - e[0][0]).hypot(e[1][1] - e[0][1]))
        .sum();
    assert!((length - 58.0).abs() < 1e-9);
    assert_eq!(segments, 10);
    for ring in std::iter::once(centers.0[0].exterior()).chain(centers.0[0].interiors()) {
        for vertex in &ring.0 {
            assert!(
                paths
                    .iter()
                    .flat_map(|p| &p.points_mm)
                    .any(|p| p[0] == vertex.x && p[1] == vertex.y)
            );
        }
    }
    let interface_paths: Vec<_> = paths
        .iter()
        .filter(|p| p.kind == DepositionKind::SupportInterface)
        .collect();
    assert_eq!(interface_paths.len(), 1);
    assert!(
        interface_paths
            .iter()
            .flat_map(|p| p.points_mm.windows(2))
            .all(|e| e[0][1] == 0.0 && e[1][1] == 0.0)
    );
}

#[test]
fn support_raster_staging_preserves_segment_and_cancellation_admission() {
    let cancel = AtomicBool::new(false);
    let raster = raster(&cancel);
    let support = cells(&raster, &rectangle(0.0, 0.0, 20.0, 5.0));
    let recipe = crate::planner::tests::recipe(1.0);
    let layer = hatch_layer(&recipe, region::HatchAxis::X, 0.4, 0.4);
    let attempt = |existing, cancel: &AtomicBool| {
        paths(
            &raster,
            &support,
            &Bits::default(),
            1.0,
            &layer,
            existing,
            0,
            0,
            &mut 0,
            0,
            cancel,
        )
    };
    assert_eq!(
        attempt(crate::working_set::units(), &cancel)
            .unwrap_err()
            .code,
        ManufacturingErrorCode::ResourceLimit
    );
    assert!(attempt(0, &AtomicBool::new(true)).is_err());
}
