// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use solidcore::{
    curve::curve::Curve,
    nurbs::{
        b_spline_curve::BSplineCurve,
        b_spline_surface::BSplineSurface,
        crv_basic,
        math::{CPoint, NOW, NOZ},
        srf_basic,
    },
    surface::surface::{Surface, p2::SurfaceKind},
};

fn bezier_knots(degree: usize) -> Vec<f64> {
    std::iter::repeat_n(0.0, degree + 1)
        .chain(std::iter::repeat_n(1.0, degree + 1))
        .collect()
}

fn native_surface(knots: [Vec<f64>; 2], points: Vec<Vec<CPoint>>) -> Surface {
    let degree = [
        knots[0].len() - points.len() - 1,
        knots[1].len() - points[0].len() - 1,
    ];
    let [u, v] = knots;
    let coefficients = srf_basic::Surface::new(points, degree[0], degree[1], u, v).unwrap();
    Surface::new(SurfaceKind::BSpline(BSplineSurface::from_nurb(
        coefficients,
        true,
    )))
}

fn native_curve(knots: Vec<f64>, points: Vec<CPoint>, dimension: u64) -> Curve {
    let degree = knots.len() - points.len() - 1;
    BSplineCurve::from_nurb(
        crv_basic::Curve::new(points, degree, knots).unwrap(),
        dimension,
        true,
    )
    .into()
}

#[test]
fn reference_centroid_is_invariant_to_native_boundary_sampling_density() {
    let frame = PlaneFrameMm {
        origin_mm: [0.0; 3],
        x_axis: [1.0, 0.0, 0.0],
        y_axis: [0.0, 1.0, 0.0],
        z_axis: [0.0, 0.0, 1.0],
    };
    let sparse = [
        [-4.0, -3.0, 2.0],
        [4.0, -3.0, 2.0],
        [4.0, 3.0, 2.0],
        [-4.0, 3.0, 2.0],
        [-4.0, -3.0, 2.0],
    ];
    let mut dense = sparse[..3].to_vec();
    dense.extend((-4..4).rev().map(|x| [f64::from(x), 3.0, 2.0]));
    dense.push(sparse[0]);
    let mut work = GeometryWorkBudget::new(1000);
    for points in [sparse.as_slice(), dense.as_slice()] {
        let center = reference_centroid(points, frame, &mut work, &AtomicBool::new(false)).unwrap();
        assert!(center[0].abs() < 1.0e-12 && center[1].abs() < 1.0e-12);
        assert!((center[2] - 2.0).abs() < 1.0e-12);
    }
}

#[test]
fn shared_projection_origin_preserves_one_ulp_boundary_orientation() {
    let frame = PlaneFrameMm {
        origin_mm: [12.0, 9.0, 8.1],
        x_axis: [1.0, 0.0, 0.0],
        y_axis: [0.0, 1.0, 0.0],
        z_axis: [0.0, 0.0, 1.0],
    };
    let start = [2.0, 18.0, 8.1];
    let end = [f64::from_bits(2.0_f64.to_bits() - 1), 18.0, 8.1];
    let a = project(start.map(Interval::point), frame);
    let delta = projected_displacement(start, end, frame);
    let signed = a[0].mul(delta[1]).sub(a[1].mul(delta[0]));
    let edge_length = length([delta[0], delta[1], Interval::point(0.0)]).unwrap();
    assert!(signed.lo > 0.0);
    assert!(
        Interval::point(signed.lo)
            .div(Interval::point(edge_length.hi))
            .unwrap()
            .lo
            > 8.0
    );
}

#[test]
fn material_height_flow_uses_the_actual_oblique_dual_and_strict_submersion() {
    let height = [0.0, 0.0, 2.0].map(Interval::point);
    let vertical = [0.25, 0.0, 0.5].map(Interval::point);
    let face = material_face_flow_rate([1.0, 0.0, 0.0].map(Interval::point), height, vertical)
        .unwrap()
        .unwrap();
    assert!(face >= 0.25 && face < 0.250000000001);
    let edge = material_edge_flow_rate([0.0, 0.0, 4.0].map(Interval::point), height, vertical)
        .unwrap()
        .unwrap();
    assert!(edge >= 0.25 && edge < 0.250000000001);
    assert!(
        material_face_flow_rate(height, height, vertical)
            .unwrap()
            .is_none()
    );
    assert!(
        material_edge_flow_rate([1.0, 0.0, 0.0].map(Interval::point), height, vertical)
            .unwrap()
            .is_none()
    );
    // A genuinely almost-critical face has a large rate, never a clamped
    // horizontal normal, zero source correction, or exact invariance.
    let steep = material_face_flow_rate(
        [1.0e-12, 0.0, 1.0].map(Interval::point),
        [0.0, 0.0, 1.0].map(Interval::point),
        [0.0, 0.0, 1.0].map(Interval::point),
    )
    .unwrap()
    .unwrap();
    assert!(steep >= 1.0e12);
}

fn annular_metric_candidate() -> SamplePrism {
    let request = horizontal_request([-0.1, 0.1], 0.0);
    let dual = [
        [1.0, 0.0, 0.0].map(Interval::point),
        [0.0, 1.0, 0.0].map(Interval::point),
        [0.0, 0.0, 1.0].map(Interval::point),
    ];
    let vertices = [
        [-10.0, -10.0],
        [10.0, -10.0],
        [10.0, 10.0],
        [-10.0, 10.0],
        [-3.0, 0.0],
        [0.0, 3.0],
        [3.0, 0.0],
        [0.0, -3.0],
    ]
    .into_iter()
    .map(|p| p.map(Interval::point))
    .collect::<Vec<_>>();
    let index = SamplePrism::ray_index(
        &vertices,
        4,
        MAX_SAMPLE_BYTES,
        &mut GeometryWorkBudget::new(100_000),
        &AtomicBool::new(false),
    )
    .unwrap();
    SamplePrism {
        vertices,
        ray_index: Some(index),
        outer_count: 4,
        center: [0.0; 3],
        frame: request.frame,
        dual,
        height_dual: dual,
        height_vertical_norm: 1.0,
        core_radius: Some(6.0),
        convex_disk: false,
        inner_radius_upper: 3.0,
        range: request.height_interval_mm,
        source_height: HeightFunctional::from_frame(request.frame),
        approximation: 0.0,
        vertical_return: 0.1,
    }
}

#[test]
fn convex_box_distance_allows_different_actual_boundary_witnesses() {
    let mut prism = annular_metric_candidate();
    prism.ray_index = None;
    prism.core_radius = None;
    prism.vertices = [[4.0, 0.0], [0.0, 4.0], [-4.0, 0.0], [0.0, -4.0]]
        .into_iter()
        .map(|p| p.map(Interval::point))
        .collect();
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(100_000);
    prism.convex_disk =
        SamplePrism::convex_radial_polygon(&prism.vertices, &mut work, &cancel).unwrap();
    let bound = prism
        .boundary_distance(
            [[3.8, 4.01], [-0.2, 0.2], [-0.02, 0.02]].map(|[lo, hi]| Interval { lo, hi }),
            0.2,
            &mut work,
            &cancel,
        )
        .unwrap();
    // The two exterior corners use different sides of the actual diamond.
    let actual_exterior_distance = (4.01 + 0.2 - 4.0) / 2.0_f64.sqrt();
    assert!(bound >= actual_exterior_distance && bound < 0.15, "{bound}");
}

#[test]
fn convex_box_distance_keeps_cap_bound_when_all_corners_are_on_boundary() {
    let mut prism = annular_metric_candidate();
    prism.ray_index = None;
    prism.core_radius = None;
    prism.vertices.truncate(prism.outer_count);
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(100_000);
    prism.convex_disk =
        SamplePrism::convex_radial_polygon(&prism.vertices, &mut work, &cancel).unwrap();
    let bound = prism
        .boundary_distance(
            [[-10.0, 10.0], [-10.0, 10.0], [-0.1, 0.1]].map(|[lo, hi]| Interval { lo, hi }),
            0.15,
            &mut work,
            &cancel,
        )
        .unwrap();
    // Every corner has zero boundary distance, but the center is 0.1 mm
    // from either cap. Filled-set convexity alone would miss this maximum.
    assert!(bound >= 0.1 && bound < 0.101, "{bound}");
}

#[test]
fn convex_radial_predicate_rejects_twice_wound_positive_turn_cycle() {
    let vertices = [[4.0, 0.0], [0.0, 4.0], [-4.0, 0.0], [0.0, -4.0]];
    let twice = vertices
        .iter()
        .cycle()
        .take(8)
        .map(|p| p.map(Interval::point))
        .collect::<Vec<_>>();
    assert!(
        !SamplePrism::convex_radial_polygon(
            &twice,
            &mut GeometryWorkBudget::new(100),
            &AtomicBool::new(false),
        )
        .unwrap()
    );
}

#[test]
fn convex_disk_metric_never_assigns_cap_witnesses_to_exterior_angles() {
    let mut prism = annular_metric_candidate();
    prism.ray_index = None;
    prism.core_radius = None;
    prism.outer_count = 8;
    prism.vertices = (0..prism.outer_count)
        .map(|index| {
            let angle = std::f64::consts::TAU * index as f64 / prism.outer_count as f64;
            [angle.cos(), angle.sin()].map(Interval::point)
        })
        .collect();
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(100_000);
    prism.convex_disk =
        SamplePrism::convex_radial_polygon(&prism.vertices, &mut work, &cancel).unwrap();
    for index in 0..64 {
        // Includes the angular wrap, original vertices and the reference
        // ray: ambiguous closed comparisons must retain the full scan.
        let angle = std::f64::consts::TAU * index as f64 / 64.0;
        let exterior = prism
            .boundary_distance(
                [1.5 * angle.cos(), 1.5 * angle.sin(), 0.0].map(Interval::point),
                0.15,
                &mut work,
                &cancel,
            )
            .unwrap();
        // The entire polygon lies in the unit disk, independently of
        // which finite polygon side supplies the returned upper bound.
        assert!(
            exterior.is_finite() && exterior >= 0.49,
            "{index}: {exterior}"
        );
        let interior = prism
            .boundary_distance(
                [0.25 * angle.cos(), 0.25 * angle.sin(), 0.0].map(Interval::point),
                0.15,
                &mut work,
                &cancel,
            )
            .unwrap();
        assert!(interior >= 0.1 && interior < 0.101, "{index}: {interior}");
    }
}

#[test]
fn disk_side_witness_covers_complete_segment_and_clamps_endpoints() {
    let mut prism = annular_metric_candidate();
    prism.ray_index = None;
    prism.vertices.truncate(prism.outer_count);
    prism.core_radius = None;
    let mut work = GeometryWorkBudget::new(100_000);
    let cancel = AtomicBool::new(false);
    let along_side = prism
        .boundary_distance(
            [
                Interval::point(10.0),
                Interval { lo: -9.0, hi: 9.0 },
                Interval::point(0.0),
            ],
            0.01,
            &mut work,
            &cancel,
        )
        .unwrap();
    assert!(
        along_side <= 0.01,
        "the entire box is on one actual side: {along_side}"
    );
    let over_endpoint = prism
        .boundary_distance(
            [
                Interval::point(10.0),
                Interval { lo: 9.9, hi: 10.1 },
                Interval::point(0.0),
            ],
            0.2,
            &mut work,
            &cancel,
        )
        .unwrap();
    assert!(over_endpoint >= 0.09999 && over_endpoint < 0.10001);
}

#[test]
fn disk_side_witness_preserves_closed_height_and_sampling_reserves() {
    let mut prism = annular_metric_candidate();
    prism.ray_index = None;
    prism.vertices.truncate(prism.outer_count);
    prism.core_radius = None;
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(100_000);
    let near_cut = prism
        .boundary_distance(
            [10.0, -10.0, 0.100001].map(Interval::point),
            0.01,
            &mut work,
            &cancel,
        )
        .unwrap();
    assert!(near_cut >= 0.999e-6 && near_cut < 1.1e-6);
    let outside = prism
        .boundary_distance(
            [10.0, -10.0, 0.3].map(Interval::point),
            0.01,
            &mut work,
            &cancel,
        )
        .unwrap();
    assert!(outside >= 0.19999 && outside < 0.20001);
    prism.approximation = 0.004;
    let reserved = prism
        .boundary_distance(
            [10.0, 0.0, 0.0].map(Interval::point),
            0.01,
            &mut work,
            &cancel,
        )
        .unwrap();
    assert!(reserved >= 0.012 && reserved < 0.012001);
}

#[test]
fn annular_ray_edge_functional_contracts_at_square_corner_and_closed_cut() {
    let prism = annular_metric_candidate();
    let mut work = GeometryWorkBudget::new(100_000);
    let cancel = AtomicBool::new(false);
    let distance = prism
        .boundary_distance(
            [10.0, -10.0, 0.100001].map(Interval::point),
            0.0,
            &mut work,
            &cancel,
        )
        .unwrap();
    assert!(
        distance < 1.1e-6,
        "same-ray edge witness must contract: {distance}"
    );
    let outside = prism
        .boundary_distance(
            [10.0, -10.0, 0.3].map(Interval::point),
            0.0,
            &mut work,
            &cancel,
        )
        .unwrap();
    assert!(
        outside >= 0.19999 && outside < 0.20001,
        "the side witness must account the actual closed-cut height: {outside}"
    );
    let unresolved = prism
        .boundary_distance(
            [
                Interval { lo: -2.0, hi: 20.0 },
                Interval {
                    lo: -20.0,
                    hi: -2.0,
                },
                Interval::point(0.0),
            ],
            0.0,
            &mut work,
            &cancel,
        )
        .unwrap();
    assert!(
        unresolved.is_finite() && unresolved > 1.0,
        "an unresolved denominator retains the complete broad fallback"
    );
}

#[test]
fn prism_closed_cut_witness_uses_the_oblique_height_dual() {
    let mut prism = annular_metric_candidate();
    prism.frame.x_axis = [1.0, 0.0, 0.25];
    prism.dual[2] = [-0.25, 0.0, 1.0].map(Interval::point);
    prism.height_dual = prism.dual;
    prism.height_vertical_norm = length(prism.height_dual[2]).unwrap().hi;
    let z = 0.100001;
    let distance = prism
        .boundary_distance(
            [-10.0 - 0.25 * z, -10.0, z].map(Interval::point),
            0.0,
            &mut GeometryWorkBudget::new(100_000),
            &AtomicBool::new(false),
        )
        .unwrap();
    assert!(
        distance > 1.02e-6 && distance < 1.1e-6,
        "a normal-only cap shift is not inside this oblique corner: {distance}"
    );
}

#[test]
fn annular_cap_witness_uses_actual_hole_not_its_enclosing_disk() {
    let prism = annular_metric_candidate();
    let cap_z = prism.range[0];
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(100_000);
    // Outside the actual diamond hole, but inside its enclosing disk.
    let cap = prism
        .boundary_distance(
            [2.0, 2.0, cap_z].map(Interval::point),
            0.0,
            &mut work,
            &cancel,
        )
        .unwrap();
    assert!(
        cap < 1e-10,
        "actual closed cap must remain available: {cap}"
    );
    let hole = prism
        .boundary_distance(
            [1.0, 1.0, cap_z].map(Interval::point),
            0.0,
            &mut work,
            &cancel,
        )
        .unwrap();
    assert!(
        hole > 0.1,
        "a hole point must not acquire a cap witness: {hole}"
    );
}

fn tilted_frame() -> PlaneFrameMm {
    PlaneFrameMm {
        origin_mm: [0.0; 3],
        x_axis: [0.8, 0.0, -0.6],
        y_axis: [0.0, 1.0, 0.0],
        z_axis: [0.6, 0.0, 0.8],
    }
}
fn source(name: &str) -> Definition {
    crate::analysis::test_source::recipe(name)
}
#[test]
fn native_rational_and_polynomial_caps_share_the_critical_complex() {
    let cancel = AtomicBool::new(false);
    for name in [
        "native-parity-bspline-box.step",
        "native-parity-nurbs-box.step",
    ] {
        let definition = source(name);
        let mut work = GeometryWorkBudget::new(30_000_000);
        let prepared = crate::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        // The bottom-right edge changes source face incidence at height 12.
        // Both encodings take the same oriented patch/trim construction.
        let result = prepared
            .analyze_slab(
                SlabRequest {
                    frame: tilted_frame(),
                    height_interval_mm: [11.98, 12.02],
                    sample_height_mm: 12.001,
                    native_shape_error_mm: 0.1,
                },
                crate::MAX_PREPARED_SECTION_BYTES.saturating_sub(prepared.retained_bytes()),
                &mut work,
                &cancel,
            )
            .unwrap();
        assert_eq!(result.section.loops.len(), 1);
        assert!(
            result
                .critical
                .as_ref()
                .is_some_and(CriticalSlabCertificate::has_native_convex_section_topology_proof)
        );
        assert!(
            !result.event_neighborhoods().is_empty(),
            "physical corner strata remain in the critical cover"
        );
        assert!(result.boundary_motion_bound_mm <= 0.1);
        assert!(
            result
                .occupied
                .as_ref()
                .is_some_and(|s| !s.is_native_intersection())
        );
    }
}
#[test]
fn imported_rational_torus_has_generic_terminal_metric_and_cycle_incidence() {
    let cancel = AtomicBool::new(false);
    let definition = source("native-parity-nurbs-torus.step");
    let mut work = GeometryWorkBudget::new(100_000_000);
    let prepared = crate::prepare_solid(
        &definition,
        crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
        &mut work,
        &cancel,
    )
    .unwrap();
    let frame = PlaneFrameMm {
        origin_mm: [0.0; 3],
        x_axis: [1.0, 0.0, 0.0],
        y_axis: [0.0, 1.0, 0.0],
        z_axis: [0.0, 0.0, 1.0],
    };
    let request = SlabRequest {
        frame,
        height_interval_mm: [1.999, 2.001],
        sample_height_mm: 1.99901,
        native_shape_error_mm: 0.05,
    };
    let result = prepared
        .analyze_slab(
            request,
            crate::MAX_PREPARED_SECTION_BYTES.saturating_sub(prepared.retained_bytes()),
            &mut work,
            &cancel,
        )
        .unwrap();
    assert_eq!(result.section.loops.len(), 2);
    assert_eq!(result.critical_material_component_count(), Some(1));
    assert_eq!(result.critical_bounded_void_count(), Some(0));
    assert_eq!(result.critical_tunnel_cycle_count(), Some(1));
    assert!(
        !result
            .critical
            .as_ref()
            .unwrap()
            .has_native_convex_section_topology_proof(),
        "fiberwise convex toroidal meridians are not a globally convex original source"
    );
    assert!(!result.event_neighborhoods().is_empty());
    assert!(result.boundary_motion_bound_mm <= request.native_shape_error_mm);
    assert_eq!(result.request().sample_height_mm, request.sample_height_mm);
    assert!(
        result
            .occupied
            .as_ref()
            .is_some_and(|s| !s.is_native_intersection())
    );
}

#[test]
fn original_circle_callbacks_keep_radian_gauge_separate_from_native_interval() {
    use crate::parametric::CurveEvaluator;
    use solidcraft_step::geom::{Curve as OriginalCurve, PreferredRep};
    let cancel = AtomicBool::new(false);
    let definition = crate::from_step_bytes_with_cancel(
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/geometry/independent-exporters/occt-cylinder-mm.step"
        )),
        &cancel,
    )
    .unwrap();
    let mut work = GeometryWorkBudget::new(30_000_000);
    let prepared =
        crate::prepare_solid(&definition, MAX_SCRATCH_BYTES, &mut work, &cancel).unwrap();
    let source = prepared.source_view().unwrap();
    let remaining = MAX_SCRATCH_BYTES - prepared.retained_bytes();
    let mut circles = 0;
    for face in 0..definition.face_ids().len() {
        let domain = source
            .face_domain(face, remaining, &mut work, &cancel)
            .unwrap();
        for index in 0..domain.coedges().len() {
            let proof = domain.original_coedge_correspondence(index).unwrap();
            let binding = *proof
                .binding()
                .expect("sealed selected original occurrence");
            let native = definition.brep().edge(binding.edge).unwrap();
            assert_eq!(
                binding.native_interval,
                [native.interval.min(), native.interval.max()]
            );
            let spatial = proof.spatial_range();
            domain.with_original_spatial_curve(index, remaining, &mut work, &cancel, |curve, work, cancel| {
                let mut original = curve.original_curve().expect("original callback operand, not a native carrier");
                for _ in 0..16 {
                    match original {
                        OriginalCurve::Trimmed { basis, .. } => original = basis,
                        OriginalCurve::SurfaceCurve { curve_3d, master: PreferredRep::Curve3d, .. } =>
                            original = curve_3d,
                        _ => break,
                    }
                }
                let OriginalCurve::Circle { position, radius } = original else { return Ok(()); };
                circles += 1;
                assert_ne!(spatial, binding.native_interval,
                    "this actual circular occurrence must exercise independent source/native gauges");
                let mut t = spatial[0] * 0.5 + spatial[1] * 0.5;
                if let Some(native_t) = binding.native_interval.into_iter()
                    .find(|&value| value < spatial[0] || value > spatial[1])
                {
                    let refusal = crate::parametric::curve_enclosure(curve, (native_t, native_t), work, cancel)
                        .unwrap_err();
                    assert_eq!(refusal.code, GeometryErrorCode::ParameterCorrespondenceUnavailable,
                        "a native parameter cannot enlarge the selected original radian restriction");
                } else {
                    t = spatial.into_iter().find(|&value|
                        value < binding.native_interval[0] || value > binding.native_interval[1]).unwrap();
                }
                let jet = crate::parametric::curve_enclosure(curve, (t, t), work, cancel)?;
                let (sine, cosine) = t.sin_cos();
                let y = position.y();
                let point: [f64; 3] = std::array::from_fn(|axis|
                    position.origin[axis] + radius * (position.ref_direction[axis] * cosine + y[axis] * sine));
                let tangent: [f64; 3] = std::array::from_fn(|axis|
                    radius * (-position.ref_direction[axis] * sine + y[axis] * cosine));
                for axis in 0..3 {
                    assert!(jet.position[axis].contains(point[axis]));
                    assert!(jet.derivative[axis].contains(tangent[axis]));
                }
                let direction = [1.0, -0.5, 0.25];
                let height = HeightFunctional { direction: direction.map(Interval::point), offset: Interval::point(0.0) };
                assert!(height.height(jet.position).contains((0..3).map(|axis| direction[axis] * point[axis]).sum()));
                assert!(height.dot(jet.derivative).contains((0..3).map(|axis| direction[axis] * tangent[axis]).sum()));
                Ok(())
            }).unwrap();
        }
    }
    assert!(
        circles > 0,
        "all actual selected circular uses are exercised"
    );
}
#[test]
fn source_normal_matching_uses_independent_uv_and_native_edge_parameters() {
    let cancel = AtomicBool::new(false);
    let definition = source("native-parity-nurbs-torus-independent-parameters.step");
    let mut work = GeometryWorkBudget::new(10_000_000);
    let prepared =
        crate::prepare_solid(&definition, MAX_SCRATCH_BYTES / 2, &mut work, &cancel).unwrap();
    let source = prepared.source_view().unwrap();
    let remaining = MAX_SCRATCH_BYTES - prepared.retained_bytes();
    let domain = source
        .face_domain(0, remaining, &mut work, &cancel)
        .unwrap();
    for index in 0..domain.coedges().len() {
        let correspondence = domain.original_coedge_correspondence(index).unwrap();
        // Source pcurves retain [0,2]/[2,4]; spatial source leaders
        // retain [10,14]/[14,18]. No numeric parameter is shared.
        assert!(correspondence.spatial_range()[0] >= 10.0);
        assert!(
            correspondence
                .uv_range()
                .into_iter()
                .all(|value| value <= 4.0)
        );
        domain
            .with_original_spatial_curve(
                index,
                remaining,
                &mut work,
                &cancel,
                |prepared, work, cancel| {
                    let [lo, hi] = crate::parametric::curve_parameter_spans(
                        prepared,
                        correspondence.spatial_range(),
                        work,
                        cancel,
                    )?
                    .next()
                    .unwrap();
                    let t = lo + (hi - lo) * 0.37;
                    let child = crate::parametric::curve_enclosure(
                        prepared,
                        (t, t + (hi - lo) * 0.005),
                        work,
                        cancel,
                    )
                    .unwrap();
                    let n = coedge_normal_enclosure(domain, index, child.position, work, cancel)
                        .unwrap();
                    assert!(length(n).unwrap().lo > 0.0);
                    let p = child.position.map(Interval::mid);
                    let rho = p[0].hypot(p[1]);
                    let outward = [p[0] * (rho - 6.0) / rho, p[1] * (rho - 6.0) / rho, p[2]];
                    assert!(dot(n, outward.map(Interval::point)).lo > 0.0);
                    Ok(())
                },
            )
            .unwrap();
        let complete = coedge_normal_enclosure(
            &domain,
            index,
            [Interval {
                lo: -10.0,
                hi: 10.0,
            }; 3],
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(
            complete
                .into_iter()
                .any(|axis| axis.lo < 0.0 && axis.hi > 0.0),
            "all position-compatible portions of the source half-cycle must survive"
        );
        let disjoint = coedge_normal_enclosure(
            &domain,
            index,
            [Interval {
                lo: 100.0,
                hi: 101.0,
            }; 3],
            &mut work,
            &cancel,
        )
        .unwrap_err();
        assert_eq!(
            disjoint.code,
            GeometryErrorCode::ParameterCorrespondenceUnavailable
        );
    }
}
#[test]
fn imported_exact_tensor_proves_internal_and_periodic_projective_g1_identities() {
    let cancel = AtomicBool::new(false);
    let definition = source("native-parity-nurbs-torus.step");
    let mut work = GeometryWorkBudget::new(10_000_000);
    let prepared =
        crate::prepare_solid(&definition, MAX_SCRATCH_BYTES, &mut work, &cancel).unwrap();
    let source = prepared.source_view().unwrap();
    let domain = source
        .face_domain(
            0,
            MAX_SCRATCH_BYTES - prepared.retained_bytes(),
            &mut work,
            &cancel,
        )
        .unwrap();
    let surface = domain.surface_evaluator();
    let mut original = surface
        .original_surface()
        .expect("selected original tensor coefficients");
    while let solidcraft_step::geom::Surface::RectangularTrimmed { basis, .. } = original {
        original = basis;
    }
    let solidcraft_step::geom::Surface::BSpline(tensor) = original else {
        panic!("the original rational torus retains its actual B-spline coefficient net");
    };
    assert!(
        tensor.weights.is_some(),
        "the selected original tensor retains its rational weights"
    );
    let source_domain = domain.uv_domain();
    for axis in 0..2 {
        assert!(periodic_regular_join(surface, axis, source_domain, &mut work, &cancel).unwrap());
    }
    let mut joins = 0;
    assert!(
        critical_face_gluing(surface, source_domain, &mut work, &cancel).unwrap(),
        "the complete exact rational torus is admitted by coefficient identities, not a knot-count shortcut"
    );
    for interface in surface_crease_interfaces(surface, source_domain, &mut work, &cancel).unwrap()
    {
        assert!(interface_regular_join(surface, interface, &mut work, &cancel).unwrap());
        let fixed_axis = usize::from(interface[1][0] == interface[1][1]);
        let mut uncertain_band = interface;
        uncertain_band[fixed_axis] = [down(interface[fixed_axis][0]), up(interface[fixed_axis][1])];
        assert!(
            !interface_regular_join(surface, uncertain_band, &mut work, &cancel).unwrap(),
            "a reflected knot enclosure is not either nominal endpoint"
        );
        joins += 1;
    }
    // Three internal knot lines in each axis, each split into four
    // one-sided spans by the other axis's original knot partition.
    assert_eq!(joins, 24);
}
#[test]
fn original_one_degree_tilted_cylinder_retains_its_terminal_slab() {
    let cancel = AtomicBool::new(false);
    let definition = crate::from_step_bytes_with_cancel(
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/geometry/independent-exporters/occt-cylinder-mm.step"
        )),
        &cancel,
    )
    .unwrap();
    let angle = std::f64::consts::PI / 180.0;
    let (sine, cosine) = angle.sin_cos();
    let frame = PlaneFrameMm {
        origin_mm: [0.0; 3],
        x_axis: [cosine, 0.0, -sine],
        y_axis: [0.0, 1.0, 0.0],
        z_axis: [sine, 0.0, cosine],
    };
    let terminal = 8.0 * cosine + 5.0 * sine;
    let mut work = GeometryWorkBudget::new(100_000_000);
    let prepared = crate::prepare_solid(
        &definition,
        crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
        &mut work,
        &cancel,
    )
    .unwrap();
    let request = SlabRequest {
        frame,
        height_interval_mm: [terminal - 0.001, terminal + 0.001],
        sample_height_mm: terminal - 0.00099,
        native_shape_error_mm: 0.02,
    };
    let result = prepared
        .analyze_slab(
            request,
            crate::MAX_PREPARED_SECTION_BYTES.saturating_sub(prepared.retained_bytes()),
            &mut work,
            &cancel,
        )
        .unwrap();
    assert_eq!(result.critical_material_component_count(), Some(1));
    assert_eq!(result.critical_tunnel_cycle_count(), Some(0));
    assert!(
        !result
            .critical
            .as_ref()
            .unwrap()
            .has_native_convex_section_topology_proof(),
        "bounded material transport is not an exact whole-source convexity theorem"
    );
    assert!(result.boundary_motion_bound_mm <= 0.02);
    assert!(!result.event_neighborhoods().is_empty());
    assert_eq!(result.request().sample_height_mm, request.sample_height_mm);
}
#[test]
fn trimmed_uv_pose_and_sense_preserve_only_exact_chart_linear_identities() {
    use solidcore::{
        Point3, Vec3,
        geom::{axis2_placement::Axis2Placement, extent1d::Extent1d},
        nurbs::b_spline_curve_n_lib::TrimMode,
    };
    let kernel = solidcore::Kernel::default();
    let mut curve = native_curve(
        bezier_knots(1),
        vec![
            CPoint::new(0.0, 1.0, NOZ, NOW),
            CPoint::new(0.0, 2.0, NOZ, NOW),
        ],
        2,
    );
    let mut interval = Extent1d::from_min_max(0.2, 0.8);
    curve
        .trim(&kernel, &mut interval, TrimMode::Silent)
        .unwrap();
    curve
        .transform(
            &kernel,
            &Axis2Placement::new(
                Point3::new(4.0, -3.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(-1.0, 0.0, 0.0),
            ),
            None,
        )
        .unwrap();
    curve.reverse_parameterization(&kernel, &interval).unwrap();
    let coefficients = [0.0, 1.0, 3.0].map(ExactCoefficient::point);
    let mut work = GeometryWorkBudget::new(10_000);
    let cancel = AtomicBool::new(false);
    assert!(chart_linear_identity(&curve, &coefficients, &mut work, &cancel).unwrap());
    let not_identical = [0.0, 1.0, 3.0 + f64::EPSILON * 2.0].map(ExactCoefficient::point);
    assert!(!chart_linear_identity(&curve, &not_identical, &mut work, &cancel).unwrap());
}
#[test]
fn diagonal_stationary_cover_retains_a_whole_critical_line_and_excludes_nonroots() {
    // Equivalent native encodings of z=(2u-1)^2 retain the complete
    // stationary line u=1/2 even though the full Hessian is singular.
    let surface = native_surface(
        [bezier_knots(2), bezier_knots(1)],
        [0.0, 0.5, 1.0]
            .into_iter()
            .zip([1.0, -1.0, 1.0])
            .map(|(x, z)| {
                [-1000.0, 1000.0]
                    .into_iter()
                    .map(|y| CPoint::new(x, y, z, NOW))
                    .collect()
            })
            .collect(),
    );
    let height = HeightFunctional {
        direction: [0.0, 0.0, 1.0].map(Interval::point),
        offset: Interval::point(0.0),
    };
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(100_000);
    let uv = [[0.0, 0.6], [0.0, 1.0]];
    let jet = surface_enclosure(&surface, uv, &mut work, &cancel).unwrap();
    let root = necessary_stationary_domain(&surface, uv, &jet, height, &mut work, &cancel)
        .unwrap()
        .expect("the whole original critical line remains");
    assert!(root[0][0] <= 0.5 && 0.5 <= root[0][1]);
    assert!(root[0][0] >= uv[0][0] && root[0][1] <= uv[0][1]);
    assert_eq!(
        root[1], uv[1],
        "a necessary cover never drops part of the critical line"
    );
    // Tensor interval dependency may prevent contraction; that is not
    // evidence of exclusion. The direct profile supplies a sharp diagonal.
    use solidcore::{
        Point2, Point3, Vec3, geom::extent2d::Extent2d, surface::surf_of_extrusion::SurfOfExtrusion,
    };
    let kernel = solidcore::Kernel::default();
    let profile = native_curve(
        bezier_knots(2),
        vec![
            CPoint::new(0.0, -1000.0, 1.0, NOW),
            CPoint::new(0.5, -1000.0, -1.0, NOW),
            CPoint::new(1.0, -1000.0, 1.0, NOW),
        ],
        3,
    );
    let mut extrusion = SurfOfExtrusion::new(
        &kernel,
        profile,
        Point3::new(0.0, -1000.0, 1.0),
        Vec3::new(0.0, 2000.0, 0.0),
        Extent2d::from_points(Point2::new(0.0, 0.0), Point2::new(1.0, 1.0)),
        false,
    )
    .unwrap();
    extrusion.make_nurb(&kernel).unwrap();
    let surface = Surface::new(SurfaceKind::SurfOfExtrusion(Box::new(extrusion)));
    let jet = surface_enclosure(&surface, uv, &mut work, &cancel).unwrap();
    assert!(!height.dot(jet.duu).contains_zero());
    let root = necessary_stationary_domain(&surface, uv, &jet, height, &mut work, &cancel)
        .unwrap()
        .expect("the same original critical line remains");
    assert!(root[0][0] <= 0.5 && 0.5 <= root[0][1] && root[0][1] - root[0][0] < 1e-8);
    assert_eq!(root[1], uv[1], "constant-height generator stays complete");
    let critical_height = height.height(
        surface_enclosure(&surface, root, &mut work, &cancel)
            .unwrap()
            .position,
    );
    assert!(critical_height.contains_zero() && critical_height.width() < 1e-8);
    let nonroot = [[0.0, 0.2], [0.0, 1.0]];
    let jet = surface_enclosure(&surface, nonroot, &mut work, &cancel).unwrap();
    assert!(
        necessary_stationary_domain(&surface, nonroot, &jet, height, &mut work, &cancel,)
            .unwrap()
            .is_none()
    );
}
#[test]
fn mixed_stationary_cover_retains_true_saddles_and_original_pole_corners() {
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(100_000);
    let height = HeightFunctional {
        direction: [0.0, 0.0, 1.0].map(Interval::point),
        offset: Interval::point(0.0),
    };
    // h=8+(u-1/2)(v-1/2) has a genuine interior saddle, with
    // zero diagonal Hessians. Its nonsingular mixed entry supplies
    // necessary contraction, while Krawczyk separately proves existence.
    let saddle = native_surface(
        [bezier_knots(1), bezier_knots(1)],
        vec![
            vec![
                CPoint::new(0.0, 0.0, 8.25, NOW),
                CPoint::new(0.0, 1.0, 7.75, NOW),
            ],
            vec![
                CPoint::new(1.0, 0.0, 7.75, NOW),
                CPoint::new(1.0, 1.0, 8.25, NOW),
            ],
        ],
    );
    let uv = [[0.4, 0.6]; 2];
    let jet = surface_enclosure(&saddle, uv, &mut work, &cancel).unwrap();
    let root = necessary_stationary_domain(&saddle, uv, &jet, height, &mut work, &cancel)
        .unwrap()
        .expect("the true saddle remains covered");
    assert!(
        root.iter()
            .all(|axis| { axis[0] <= 0.5 && 0.5 <= axis[1] && axis[1] - axis[0] < 1e-10 })
    );
    let proved = proved_stationary_height(&saddle, uv, &jet, height, &mut work, &cancel)
        .unwrap()
        .expect("independent inclusion proves the interior saddle");
    assert!(proved.lo <= 8.0 && 8.0 <= proved.hi);
    let nonroot = [[0.6, 0.9], [0.4, 0.6]];
    let jet = surface_enclosure(&saddle, nonroot, &mut work, &cancel).unwrap();
    assert!(
        necessary_stationary_domain(&saddle, nonroot, &jet, height, &mut work, &cancel)
            .unwrap()
            .is_none()
    );

    // The SAME horizontal-axis sphere uses native longitude/latitude degrees:
    // h=8+2 cos(latitude)sin(longitude). The original polar corner is
    // (longitude,latitude)=(0,-90), not an interior approximate sample.
    use solidcore::{
        Point2, Point3, Vec3,
        geom::extent2d::Extent2d,
        surface::{
            sphere::{Sphere, SphereOptions},
            surface::StepSurface,
        },
    };
    let kernel = solidcore::Kernel::default();
    let sphere = Sphere::new(
        &kernel,
        Point3::new(0.0, 0.0, 8.0),
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(0.0, 0.0, 1.0),
        Extent2d::from_points(Point2::new(0.0, -90.0), Point2::new(360.0, 90.0)),
        2.0,
        SphereOptions::default(),
    )
    .unwrap();
    let pole = Surface::new(SurfaceKind::Step(
        StepSurface::new(Surface::new(SurfaceKind::Sphere(Box::new(sphere))), true).unwrap(),
    ));
    let uv = [[0.0, 45.0], [-90.0, -90.0 + 4e-7_f64.to_degrees()]];
    let jet = surface_enclosure(&pole, uv, &mut work, &cancel).unwrap();
    assert!(height.dot(jet.duu).contains_zero() && height.dot(jet.dvv).contains_zero());
    assert!(!height.dot(jet.duv).contains_zero());
    assert!(height.height(jet.position).width() > 1e-7);
    let root = necessary_stationary_domain(&pole, uv, &jet, height, &mut work, &cancel)
        .unwrap()
        .expect("the original pole corner remains covered");
    for (axis, endpoint) in [0.0, -90.0].into_iter().enumerate() {
        assert_eq!(root[axis][0], endpoint);
        assert!(root[axis][1] - endpoint < 1e-12 && root[axis][1] <= uv[axis][1]);
    }
    let bound = height.height(
        surface_enclosure(&pole, root, &mut work, &cancel)
            .unwrap()
            .position,
    );
    assert!(bound.lo <= 8.0 && 8.0 <= bound.hi && bound.width() < 1e-12);
}
#[test]
fn a_piecewise_hessian_cannot_exclude_a_true_source_knot_cusp() {
    // Actual native height is u²-|u|+v². Its smooth-piece Hessians are
    // 2I, yet its gradient jumps at u=0. Newton from the right derivative
    // would jump outside this box and falsely exclude the source cusp.
    let xs = [-1.0, -0.5, 0.0, 0.5, 1.0];
    let hu = [0.0, -0.5, 0.0, -0.5, 0.0];
    let ys = [-1.0, 0.0, 1.0];
    let hv = [1.0, -1.0, 1.0];
    let controls = xs
        .into_iter()
        .enumerate()
        .map(|(i, x)| {
            ys.into_iter()
                .enumerate()
                .map(|(j, y)| CPoint::new(x, y, hu[i] + hv[j], NOW))
                .collect()
        })
        .collect();
    let surface = native_surface(
        [
            vec![-1.0, -1.0, -1.0, 0.0, 0.0, 1.0, 1.0, 1.0],
            vec![-1.0, -1.0, -1.0, 1.0, 1.0, 1.0],
        ],
        controls,
    );
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(100_000);
    assert!(!critical_face_gluing(&surface, [[-1.0, 1.0]; 2], &mut work, &cancel).unwrap());
    let uv = [[-0.125, 0.125]; 2];
    let jet =
        crate::parametric::surface_piecewise_enclosure(&surface, uv, &mut work, &cancel).unwrap();
    let height = HeightFunctional {
        direction: [0.0, 0.0, 1.0].map(Interval::point),
        offset: Interval::point(0.0),
    };
    assert!(matches!(
        stationary_root(&surface, uv, &jet, height, &mut work, &cancel).unwrap(),
        RootIsolation::ConvexCriticalCover
    ));
    assert_eq!(
        necessary_stationary_domain(&surface, uv, &jet, height, &mut work, &cancel).unwrap(),
        Some(uv),
        "a discontinuous source gradient cannot justify a Newton contraction"
    );
    let monotone = HeightFunctional {
        direction: [0.0, 1.0, 0.0].map(Interval::point),
        offset: Interval::point(0.0),
    };
    assert!(
        matches!(
            stationary_root(&surface, uv, &jet, monotone, &mut work, &cancel).unwrap(),
            RootIsolation::Excluded
        ),
        "gradient-only exclusion remains valid"
    );
}
#[test]
fn original_folded_profile_cannot_gain_global_convexity_by_partitioning() {
    use solidcore::{
        Point2, Point3, Vec3, curve::crv_on_surf::CrvOnSurf, geom::extent2d::Extent2d,
        surface::surf_of_extrusion::SurfOfExtrusion,
    };
    let kernel = solidcore::Kernel::default();
    let carrier = native_surface(
        [vec![0.0, 0.0, 0.5, 1.0, 1.0], bezier_knots(1)],
        vec![
            vec![
                CPoint::new(0.0, 0.0, 0.0, NOW),
                CPoint::new(0.0, 1.0, 0.0, NOW),
            ],
            vec![
                CPoint::new(0.5, 0.25, 0.0, NOW),
                CPoint::new(0.5, 1.25, 0.0, NOW),
            ],
            vec![
                CPoint::new(1.0, 0.0, 0.0, NOW),
                CPoint::new(1.0, 1.0, 0.0, NOW),
            ],
        ],
    );
    let chart = native_curve(
        bezier_knots(1),
        vec![
            CPoint::new(0.0, 0.0, NOZ, NOW),
            CPoint::new(1.0, 0.0, NOZ, NOW),
        ],
        2,
    );
    let nested: Curve = CrvOnSurf::new(&kernel, chart, carrier, None, 0)
        .unwrap()
        .into();
    assert!(
        !crate::parametric::curve_parameter_span_smooth(
            &nested,
            [0.0, 1.0],
            &mut GeometryWorkBudget::new(100_000),
            &AtomicBool::new(false)
        )
        .unwrap()
    );
    let rejection = SurfOfExtrusion::new(
        &kernel,
        nested,
        Point3::new(0.0, 0.0, 0.0),
        Vec3::new(0.0, 0.0, 1.0),
        Extent2d::from_points(Point2::new(0.0, 0.0), Point2::new(1.0, 1.0)),
        false,
    )
    .expect_err("pinned native kernel cannot construct a curve-on-surface generator");
    assert!(
        rejection
            .to_string()
            .contains("generator is not derived from SmBSplineCurve")
    );
    // Native extrusion accepts only B-spline-derived generators and owns its
    // original stored tensor. Keep the SAME exact fold in that reachable
    // generator as well: subdivision cannot erase its true interior C0 knot.
    let profile = native_curve(
        vec![0.0, 0.0, 0.5, 1.0, 1.0],
        vec![
            CPoint::new(0.0, 0.0, 0.0, NOW),
            CPoint::new(0.5, 0.25, 0.0, NOW),
            CPoint::new(1.0, 0.0, 0.0, NOW),
        ],
        3,
    );
    let mut extrusion = SurfOfExtrusion::new(
        &kernel,
        profile,
        Point3::new(0.0, 0.0, 0.0),
        Vec3::new(0.0, 0.0, 1.0),
        Extent2d::from_points(Point2::new(0.0, 0.0), Point2::new(1.0, 1.0)),
        false,
    )
    .unwrap();
    extrusion.make_nurb(&kernel).unwrap();
    let surface = Surface::new(SurfaceKind::SurfOfExtrusion(Box::new(extrusion)));
    assert!(
        !critical_face_gluing(
            &surface,
            [[0.0, 1.0]; 2],
            &mut GeometryWorkBudget::new(100_000),
            &AtomicBool::new(false)
        )
        .unwrap(),
        "a hidden original carrier crease is not an original topological coedge"
    );
}
#[test]
fn rounded_axis_length_is_not_an_exact_native_rotation_identity() {
    let mut work = GeometryWorkBudget::new(1000);
    let cancel = AtomicBool::new(false);
    assert!(exact_unit_vector_identity([0.0, 0.0, 1.0], &mut work, &cancel).unwrap());
    assert_eq!(0.6_f64 * 0.6 + 0.8_f64 * 0.8, 1.0);
    assert!(!exact_unit_vector_identity([0.6, 0.0, 0.8], &mut work, &cancel).unwrap());
}
#[test]
fn exact_expansion_never_rounds_a_residual_or_underflow_product_to_an_identity() {
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(100);
    let next = f64::from_bits(1.0_f64.to_bits() + 1);
    assert_eq!(ExactCoefficient::difference(next, 1.0).unwrap().sign(), 1);
    let tiny = ExactCoefficient::point(2.0_f64.powi(-600));
    assert!(tiny.product(&tiny, &mut work, &cancel).unwrap().is_none());
    let mut residual = ExactCoefficient::point(1.0);
    residual.subtract(&ExactCoefficient::point(next)).unwrap();
    assert_eq!(residual.sign(), -1);
}
#[test]
fn a_nonzero_g1_coefficient_residual_is_not_a_small_jump_certificate() {
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(10_000);
    let center = |_: usize, coordinate: usize, _: &mut GeometryWorkBudget| {
        Ok(Some(ExactCoefficient::point(if coordinate == 3 {
            1.0
        } else {
            0.0
        })))
    };
    let derivative = |other: usize, coordinate: usize, left: bool, _: &mut GeometryWorkBudget| {
        let value = if coordinate == 0 {
            1.0
        } else if !left && other == 1 && coordinate == 1 {
            f64::EPSILON
        } else {
            0.0
        };
        Ok(Some(ExactCoefficient::point(value)))
    };
    assert!(!coefficient_tangent_join(2, center, derivative, &mut work, &cancel).unwrap());
}
#[test]
fn krawczyk_proves_native_morse_root_and_excludes_no_root_patch() {
    let cancel = AtomicBool::new(false);
    let surface = native_surface(
        [bezier_knots(2), bezier_knots(2)],
        (0..3)
            .map(|u| {
                (0..3)
                    .map(|v| {
                        CPoint::new(
                            u as f64 * 0.5,
                            v as f64 * 0.5,
                            [0.25, -0.25, 0.25][u] + [0.25, -0.25, 0.25][v],
                            NOW,
                        )
                    })
                    .collect()
            })
            .collect(),
    );
    let frame = PlaneFrameMm {
        origin_mm: [0.0; 3],
        x_axis: [1.0, 0.0, 0.0],
        y_axis: [0.0, 1.0, 0.0],
        z_axis: [0.0, 0.0, 1.0],
    };
    let mut work = GeometryWorkBudget::new(1_000_000);
    let uv = [[0.1, 0.9], [0.1, 0.9]];
    let jet = surface_enclosure(&surface, uv, &mut work, &cancel).unwrap();
    assert!(matches!(
        stationary_root(
            &surface,
            uv,
            &jet,
            HeightFunctional::from_frame(frame),
            &mut work,
            &cancel
        )
        .unwrap(),
        RootIsolation::Unique(_)
    ));
    let uv = [[0.01, 0.1], [0.01, 0.1]];
    let jet = surface_enclosure(&surface, uv, &mut work, &cancel).unwrap();
    assert!(matches!(
        stationary_root(
            &surface,
            uv,
            &jet,
            HeightFunctional::from_frame(frame),
            &mut work,
            &cancel
        )
        .unwrap(),
        RootIsolation::Excluded
    ));
}
#[test]
fn flat_native_rank_zero_is_not_a_fake_isolated_root() {
    let cancel = AtomicBool::new(false);
    use solidcore::{Point2, Point3, Vec2, Vec3, geom::extent2d::Extent2d, surface::plane::Plane};
    let surface = Surface::new(SurfaceKind::Plane(Plane::new(
        Point3::new(0.0, 0.0, 0.0),
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
        Vec2::new(1.0, 1.0),
        &Extent2d::from_points(Point2::new(0.0, 0.0), Point2::new(1.0, 1.0)),
    )));
    let frame = PlaneFrameMm {
        origin_mm: [0.0; 3],
        x_axis: [1.0, 0.0, 0.0],
        y_axis: [0.0, 1.0, 0.0],
        z_axis: [0.0, 0.0, 1.0],
    };
    let mut work = GeometryWorkBudget::new(1_000_000);
    let uv = [[0.0, 1.0], [0.0, 1.0]];
    let jet = surface_enclosure(&surface, uv, &mut work, &cancel).unwrap();
    assert!(locally_convex(&jet, normal(&jet, true)));
    assert!(matches!(
        stationary_root(
            &surface,
            uv,
            &jet,
            HeightFunctional::from_frame(frame),
            &mut work,
            &cancel
        )
        .unwrap(),
        RootIsolation::ConvexCriticalCover
    ));
}
#[test]
fn enclosed_direction_is_used_in_both_gradient_and_hessian() {
    let functional = HeightFunctional {
        direction: [
            Interval {
                lo: -0.01,
                hi: 0.01,
            },
            Interval::point(0.0),
            Interval::point(1.0),
        ],
        offset: Interval::point(0.0),
    };
    assert!(
        functional
            .dot([
                Interval::point(1.0),
                Interval::point(0.0),
                Interval::point(0.0)
            ])
            .contains_zero()
    );
    let projected = functional.height([
        Interval::point(10.0),
        Interval::point(0.0),
        Interval::point(2.0),
    ]);
    assert!(projected.lo <= 1.9 && projected.hi >= 2.1);
}

fn horizontal_request(range: [f64; 2], sample: f64) -> SlabRequest {
    SlabRequest {
        frame: PlaneFrameMm {
            origin_mm: [0.0; 3],
            x_axis: [1.0, 0.0, 0.0],
            y_axis: [0.0, 1.0, 0.0],
            z_axis: [0.0, 0.0, 1.0],
        },
        height_interval_mm: range,
        sample_height_mm: sample,
        native_shape_error_mm: 0.1,
    }
}

fn horizontal_section(
    source: &crate::PreparedSolid<'_>,
    height: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> NativeSection {
    source
        .section_for_query(
            &crate::section::SectionQuery {
                plane: crate::PlaneMm {
                    origin_mm: [0.0, 0.0, height],
                    normal: [0.0, 0.0, 1.0],
                },
                maximum_chord_error_mm: spiling_contracts::geometry::SECTION_SAMPLING_TOLERANCE_MM,
                maximum_plane_error_mm: spiling_contracts::geometry::SECTION_PLANE_TOLERANCE_MM,
            },
            cancel,
            (crate::MAX_PREPARED_SECTION_BYTES - source.retained_bytes()) as u32,
            work,
        )
        .unwrap()
}

fn horizontal_critical(
    source: &crate::PreparedSolid<'_>,
    request: SlabRequest,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<CriticalSlabCertificate, GeometryError> {
    let section = horizontal_section(source, request.sample_height_mm, work, cancel);
    let height = super::super::slab::native_sample_height_band(
        source.definition(),
        request.frame,
        &section,
        work,
        cancel,
    )?;
    certify_critical_slab(
        CriticalSlabQuery {
            source: source.source_view()?,
            request: &request,
            section: &section,
            functional: HeightFunctional::from_frame(request.frame),
            actual_sample_height: height,
        },
        crate::MAX_PREPARED_SECTION_BYTES
            .checked_sub(source.retained_bytes())
            .and_then(|remaining| remaining.checked_sub(section.retained_bytes()))
            .ok_or_else(|| {
                GeometryError::new(
                    GeometryErrorCode::ResourceLimit,
                    "independent critical proof input exceeds remaining query capacity",
                )
            })?,
        work,
        cancel,
    )?
    .ok_or_else(|| {
        GeometryError::new(
            GeometryErrorCode::AnalysisIndeterminate,
            "independent critical property has no represented native disk",
        )
    })
}

#[test]
fn closed_capture_excludes_original_pocket_only_when_its_whole_height_is_disjoint() {
    let cancel = AtomicBool::new(false);
    for name in [
        "native-parity-blind-pocket.step",
        "native-parity-blind-pocket-nurbs-floor.step",
    ] {
        let definition = source(name);
        let mut work = GeometryWorkBudget::new(100_000_000);
        let prepared = crate::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        // The SAME immutable source has real void-facing walls above Z=4.
        // Below that floor its actual closed material intersection is a disk.
        let request = horizontal_request([1.99, 2.01], 2.0);
        let proof = horizontal_critical(&prepared, request, &mut work, &cancel).unwrap();
        assert_eq!(proof.definition_id(), definition.id());
        assert_eq!(
            proof.request().height_interval_mm,
            request.height_interval_mm
        );
        assert_eq!(proof.material_component_count(), 1);
        assert_eq!(proof.bounded_void_count(), 0);
        assert!(
            !proof.has_native_convex_section_topology_proof(),
            "the actual upper pocket forbids a whole-source convex claim"
        );
        let Some(SourceBoundaryScope::ClosedCapture(boundary)) = &proof.boundary_scope else {
            panic!("the original upper pocket needs explicit height-exclusion authority");
        };
        assert!(boundary.excluded_face_cells > 0);
        assert!(boundary.excluded_coedge_cells > 0);
        assert!(proof.boundary_motion_bound_mm <= request.native_shape_error_mm);
        assert!(proof.covered_cells().0 > boundary.excluded_face_cells);
        // Intersecting the pocket, or merely touching its floor at the
        // CLOSED endpoint, must not reuse those previous exclusions.
        for range in [[3.99, 4.01], [3.99, 4.0]] {
            let failure = horizontal_critical(
                &prepared,
                horizontal_request(range, 3.995),
                &mut work,
                &cancel,
            )
            .unwrap_err();
            assert_eq!(failure.code, GeometryErrorCode::AnalysisIndeterminate);
            assert!(failure.message.contains("radial degree"), "{failure:?}");
        }
    }
}

#[test]
fn actual_whole_source_convexity_is_not_replaced_by_capture_exclusions() {
    let cancel = AtomicBool::new(false);
    let definition = source("native-parity-nurbs-box.step");
    let mut work = GeometryWorkBudget::new(100_000_000);
    let prepared = crate::prepare_solid(
        &definition,
        crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
        &mut work,
        &cancel,
    )
    .unwrap();
    let proof = horizontal_critical(
        &prepared,
        horizontal_request([1.99, 2.01], 2.0),
        &mut work,
        &cancel,
    )
    .unwrap();
    assert!(
        proof.has_native_convex_section_topology_proof(),
        "all original box faces and links, not only this section, were proved"
    );
}

#[test]
fn original_halfspace_cap_degree_requires_a_strictly_interior_native_witness() {
    let cancel = AtomicBool::new(false);
    let definition = source("native-parity-blind-pocket.step");
    let mut work = GeometryWorkBudget::new(100_000_000);
    let prepared = crate::prepare_solid(
        &definition,
        crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
        &mut work,
        &cancel,
    )
    .unwrap();
    let request = horizontal_request([1.9, 2.1], 2.0);
    let section = horizontal_section(&prepared, 2.0, &mut work, &cancel);
    let sample_height = super::super::slab::native_sample_height_band(
        &definition,
        request.frame,
        &section,
        &mut work,
        &cancel,
    )
    .unwrap();
    let mut prism = SamplePrism::new(
        &section,
        &request,
        HeightFunctional::from_frame(request.frame),
        sample_height,
        &mut work,
        &cancel,
    )
    .unwrap();
    prism.closed_capture_boundary(&mut work, &cancel).unwrap();
    let height = prism
        .source_height
        .height(prism.center.map(Interval::point));
    for range in [[height.lo, height.hi + 0.1], [height.lo - 0.1, height.hi]] {
        prism.range = range;
        let failure = prism
            .closed_capture_boundary(&mut work, &cancel)
            .unwrap_err();
        assert_eq!(failure.code, GeometryErrorCode::AnalysisIndeterminate);
    }
}

#[test]
fn closed_caps_enclose_the_whole_native_core_not_only_its_center() {
    let cancel = AtomicBool::new(false);
    let definition = source("native-parity-nurbs-torus.step");
    let mut work = GeometryWorkBudget::new(100_000_000);
    let prepared = crate::prepare_solid(
        &definition,
        crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
        &mut work,
        &cancel,
    )
    .unwrap();
    let request = horizontal_request([-0.01, 0.01], 0.0);
    let section = horizontal_section(&prepared, 0.0, &mut work, &cancel);
    let band = super::super::slab::native_sample_height_band(
        &definition,
        request.frame,
        &section,
        &mut work,
        &cancel,
    )
    .unwrap();
    let mut prism = SamplePrism::new(
        &section,
        &request,
        HeightFunctional::from_frame(request.frame),
        band,
        &mut work,
        &cancel,
    )
    .unwrap();
    assert!(prism.core_radius.unwrap() > 1.0);
    prism.closed_capture_boundary(&mut work, &cancel).unwrap();
    // Tilt the ACTUAL cut covector relative to the captured native core.
    // The center remains interior, but part of this same core leaves it.
    prism.source_height.direction = [0.1, 0.0, 1.0].map(Interval::point);
    let center_height = prism
        .source_height
        .height(prism.center.map(Interval::point));
    prism.range = [center_height.lo - 0.01, center_height.hi + 0.01];
    let failure = prism
        .closed_capture_boundary(&mut work, &cancel)
        .unwrap_err();
    assert_eq!(failure.code, GeometryErrorCode::AnalysisIndeterminate);
    assert!(failure.message.contains("strictly interior"));
}

#[test]
fn complete_radial_material_link_preserves_a_genuine_nonconvex_star() {
    let solid = crate::analysis::test_source::polygon_prism(
        &[
            [0.0, 0.0],
            [20.0, 0.0],
            [20.0, 20.0],
            [12.0, 20.0],
            [10.0, 18.0],
            [8.0, 20.0],
            [0.0, 20.0],
        ],
        4.0,
    );
    let definition = crate::analysis::test_source::original_definition(solid);
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(100_000_000);
    let prepared = crate::prepare_solid(
        &definition,
        crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
        &mut work,
        &cancel,
    )
    .unwrap();
    let proof = horizontal_critical(
        &prepared,
        horizontal_request([1.99, 2.01], 2.0),
        &mut work,
        &cancel,
    )
    .unwrap();
    assert_eq!(proof.material_component_count(), 1);
    assert_eq!(proof.bounded_void_count(), 0);
    assert_eq!(proof.tunnel_cycle_count(), 0);
    assert!(!proof.has_native_convex_section_topology_proof());
    assert!(proof.boundary_motion_bound_mm <= 0.1);
    let section = horizontal_section(&prepared, 2.0, &mut work, &cancel);
    assert_eq!(section.loops.len(), 1);
    let points = &section.loops[0].points_mm;
    let area = points
        .windows(2)
        .map(|pair| pair[0][0] * pair[1][1] - pair[1][0] * pair[0][1])
        .sum::<f64>()
        .abs()
        * 0.5;
    assert!(
        (area - 396.0).abs() <= 0.01,
        "the actual V notch must remain: {area}"
    );
}

#[test]
fn original_housing_layer_one_has_a_complete_paired_material_link() {
    let cancel = AtomicBool::new(false);
    let definition = crate::from_step_bytes_with_cancel(
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step"
        )),
        &cancel,
    )
    .unwrap();
    let mut work = GeometryWorkBudget::new(100_000_000);
    let prepared = crate::prepare_solid(
        &definition,
        crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
        &mut work,
        &cancel,
    )
    .unwrap();
    let proof = horizontal_critical(
        &prepared,
        horizontal_request(
            [0.09999992857142809, 0.1499998928571424],
            0.12499991071428523,
        ),
        &mut work,
        &cancel,
    )
    .unwrap();
    assert_eq!(proof.material_component_count(), 1);
    assert_eq!(proof.bounded_void_count(), 0);
    assert_eq!(proof.tunnel_cycle_count(), 0);
    assert!(
        !proof.has_native_convex_section_topology_proof(),
        "the unchanged upper blind pocket is not a globally convex source"
    );
    assert!(proof.boundary_motion_bound_mm <= 0.1);
    assert!(proof.occupied.is_some());
    assert_eq!(
        proof.material_realization_identity(),
        crate::kernel::MaterialRealizationIdentity::new(&definition)
    );
}
