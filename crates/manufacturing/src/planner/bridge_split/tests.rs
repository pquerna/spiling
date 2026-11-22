// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;

const WIDTH: f64 = 0.45;

fn stroke(
    kind: DepositionKind,
    owner: Option<DepositionOwner>,
    z: [f64; 2],
    x: [f64; 2],
) -> DepositionPath {
    DepositionPath {
        material_domain: None,
        kind,
        owner,
        width: WidthProfile::Constant { width_mm: WIDTH },
        physical_z_mm: z,
        points_mm: vec![[x[0], 5.0, z[1]], [x[1], 5.0, z[1]]],
        arcs: Vec::new(),
        exposed: false,
        connectors: Vec::new(),
    }
}

fn layer(index: u32, paths: Vec<DepositionPath>) -> PrintLayer {
    PrintLayer {
        index,
        hatch_course: index,
        lower_z_mm: f64::from(index) * 0.2,
        z_mm: f64::from(index + 1) * 0.2,
        height_mm: 0.2,
        section_z_mm: f64::from(index) * 0.2 + 0.1,
        paths,
        skin_command_fits: Vec::new(),
        region_coverage: crate::planner::empty_coverage(1.0),
    }
}

const TOP: Option<DepositionOwner> = Some(DepositionOwner::MandatorySkin {
    skin: ProcessSkinKind::TopSkin,
});

/// A 30 mm top row over lower strokes with the given uncovered X intervals.
fn split(gaps: &[[f64; 2]], span_limit: f64) -> Vec<DepositionPath> {
    let mut lower = Vec::new();
    let mut x = 0.0;
    for gap in gaps.iter().chain(std::iter::once(&[30.0, 30.0])) {
        if gap[0] > x {
            lower.push(stroke(
                DepositionKind::SolidFill,
                None,
                [0.0, 0.2],
                [x, gap[0]],
            ));
        }
        x = gap[1];
    }
    let row = stroke(DepositionKind::Bridge, TOP, [0.2, 0.4], [0.0, 30.0]);
    let mut layers = vec![layer(0, lower), layer(1, vec![row])];
    let mut recipe = crate::planner::tests::recipe(1.0);
    recipe.process.max_bridge_span_mm = span_limit;
    recipe.top_skin_axis = RectilinearAxisPolicy::X;
    let mut work = 0;
    let keys = hatch_alternation_keys(&layers);
    split_bridged_rows(
        &mut layers,
        &keys,
        &recipe,
        &mut work,
        &AtomicBool::new(false),
    )
    .unwrap();
    layers.pop().unwrap().paths
}

fn summary(paths: &[DepositionPath]) -> Vec<(DepositionKind, f64, f64)> {
    paths
        .iter()
        .map(|p| (p.kind, p.points_mm[0][0], p.points_mm[1][0]))
        .collect()
}

#[test]
fn only_directly_supported_exact_fitted_skin_takes_back_its_authored_role() {
    let fitted = stroke(DepositionKind::Bridge, TOP, [0.2, 0.4], [41.226, 41.248]);
    let lower = stroke(DepositionKind::SolidFill, None, [0.0, 0.2], [40., 42.]);
    let mut recipe = crate::planner::tests::recipe(0.15);
    recipe.top_skin_axis = RectilinearAxisPolicy::X;
    for (lower, bound, expected) in [
        (Some(lower.clone()), true, DepositionKind::TopSkin),
        (Some(lower), false, DepositionKind::Bridge),
        (None, true, DepositionKind::Bridge),
    ] {
        let mut upper = layer(1, vec![fitted.clone()]);
        if bound {
            upper.skin_command_fits.push(SkinCommandFitDomain {
                region_index: 0,
                region: ProcessRegionCoverage {
                    material_domain: None,
                    functional_solid_zone: None,
                    fraction: 0.15,
                    area_mm2: 1.,
                    contributions: Vec::new(),
                },
                skin: ProcessSkinKind::TopSkin,
                regional: Vec::new(),
                mandatory: Vec::new(),
                source_owner: Vec::new(),
                fits: vec![SkinCommandFit {
                    required_mm: [[41.23, 5., 0.4], [41.24, 5., 0.4]],
                    fitted_mm: [fitted.points_mm[0], fitted.points_mm[1]],
                }],
            });
        }
        let path = &upper.paths[0];
        let restored = supported_fitted_skin(
            &upper,
            path,
            &layer(0, lower.into_iter().collect()),
            &recipe,
            1,
            &mut AnchorWorkspace::default(),
            &mut 0,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(restored.unwrap_or(path.kind), expected);
    }
}

#[test]
fn only_the_bridged_span_and_its_anchors_stay_bridge() {
    let paths = split(&[[10.0, 14.0]], 10.0);
    let pieces = summary(&paths);
    assert_eq!(
        pieces.iter().map(|p| p.0).collect::<Vec<_>>(),
        [
            DepositionKind::TopSkin,
            DepositionKind::Bridge,
            DepositionKind::TopSkin
        ]
    );
    // Pieces meet end to end and keep the whole row.
    assert_eq!(pieces[0].1, 0.0);
    assert_eq!(pieces[0].2, pieces[1].1);
    assert_eq!(pieces[1].2, pieces[2].1);
    assert_eq!(pieces[2].2, 30.0);
    // The Bridge covers the gap left by the lower capsules plus an anchor.
    let anchor = BRIDGE_ANCHOR_WIDTHS * WIDTH;
    let gap = [10.0 + WIDTH / 2.0, 14.0 - WIDTH / 2.0];
    assert!((pieces[1].1 - (gap[0] - anchor)).abs() < 1e-6, "{pieces:?}");
    assert!((pieces[1].2 - (gap[1] + anchor)).abs() < 1e-6, "{pieces:?}");
    assert!(paths.iter().all(|p| p.owner == TOP && !p.exposed));
}

#[test]
fn nearby_gaps_merge_and_short_remainders_join_the_bridge() {
    // Two gaps 1 mm apart: the supported 1 mm between them is shorter than
    // two anchors, so one Bridge piece crosses both.
    let pieces = summary(&split(&[[10.0, 12.0], [13.0, 15.0]], 10.0));
    assert_eq!(
        pieces.iter().map(|p| p.0).collect::<Vec<_>>(),
        [
            DepositionKind::TopSkin,
            DepositionKind::Bridge,
            DepositionKind::TopSkin
        ]
    );
    // A gap that ends within one anchor of the row end leaves no skin there.
    let pieces = summary(&split(&[[27.0, 29.6]], 10.0));
    assert_eq!(
        pieces.iter().map(|p| p.0).collect::<Vec<_>>(),
        [DepositionKind::TopSkin, DepositionKind::Bridge]
    );
    assert_eq!(pieces[1].2, 30.0);
}

#[test]
fn rows_with_unbridgeable_gaps_or_no_supported_remainder_stay_whole() {
    // Over the span limit: a support demand, not this split's decision.
    let pieces = summary(&split(&[[10.0, 14.0]], 2.0));
    assert_eq!(pieces, [(DepositionKind::Bridge, 0.0, 30.0)]);
    // Gaps so close to both ends that no remainder reaches an anchor.
    let pieces = summary(&split(&[[0.5, 14.0], [15.0, 29.5]], 20.0));
    assert_eq!(pieces, [(DepositionKind::Bridge, 0.0, 30.0)]);
}

#[test]
fn rows_across_the_authored_axis_stay_whole() {
    let row = stroke(DepositionKind::Bridge, TOP, [0.2, 0.4], [0.0, 30.0]);
    let mut recipe = crate::planner::tests::recipe(1.0);
    recipe.top_skin_axis = RectilinearAxisPolicy::Y;
    assert!(!on_authored_axis(&row, DepositionKind::TopSkin, &recipe, 1).unwrap());
    recipe.top_skin_axis = RectilinearAxisPolicy::X;
    assert!(on_authored_axis(&row, DepositionKind::TopSkin, &recipe, 1).unwrap());
    // Solid and bottom-skin pieces follow the ordinary hatch axis.
    recipe.rectilinear_axis = RectilinearAxisPolicy::AlternatingXY;
    assert!(on_authored_axis(&row, DepositionKind::SolidFill, &recipe, 0).unwrap());
    assert!(!on_authored_axis(&row, DepositionKind::SolidFill, &recipe, 1).unwrap());
}

#[test]
fn sparse_rows_and_walls_are_not_split() {
    let row = stroke(
        DepositionKind::Bridge,
        Some(DepositionOwner::Wall),
        [0.2, 0.4],
        [0.0, 30.0],
    );
    assert_eq!(supported_role(&layer(1, Vec::new()), &row), None);
    let regional = Some(DepositionOwner::Regional { region_index: 0 });
    let row = stroke(DepositionKind::Bridge, regional, [0.2, 0.4], [0.0, 30.0]);
    assert_eq!(supported_role(&layer(1, Vec::new()), &row), None);
}
