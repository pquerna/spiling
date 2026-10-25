// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::*;
use crate::{PlaneFrameMm, SectionLoopMm, from_step_bytes_with_cancel};
use spiling_contracts::geometry::MAX_SECTION_BYTES;
use std::{f64::consts::PI, path::Path};
fn fixture(name: &str) -> Definition {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/geometry")
        .join(name);
    let bytes = std::fs::read(path).expect("frozen original geometry fixture");
    from_step_bytes_with_cancel(&bytes, &AtomicBool::new(false))
        .expect("admitted original native fixture")
}
fn area(boundary: &SectionLoopMm, frame: PlaneFrameMm) -> f64 {
    let xy = |point: [f64; 3]| -> [f64; 2] {
        let relative: [f64; 3] = std::array::from_fn(|i| point[i] - frame.origin_mm[i]);
        [
            relative.iter().zip(frame.x_axis).map(|(a, b)| a * b).sum(),
            relative.iter().zip(frame.y_axis).map(|(a, b)| a * b).sum(),
        ]
    };
    boundary
        .points_mm
        .windows(2)
        .map(|edge| {
            let a = xy(edge[0]);
            let b = xy(edge[1]);
            a[0] * b[1] - a[1] * b[0]
        })
        .sum::<f64>()
        * 0.5
}
fn query(definition: &Definition, z: f64, normal: [f64; 3]) -> NativeSection {
    let mut work = GeometryWorkBudget::new(20_000_000);
    section(
        definition,
        PlaneMm {
            origin_mm: [0.0, 0.0, z],
            normal,
        },
        &AtomicBool::new(false),
        MAX_SECTION_BYTES,
        &mut work,
    )
    .unwrap_or_else(|error| panic!("z={z} normal={normal:?}: {error:?}"))
}
fn assert_provenance(definition: &Definition, result: &NativeSection) {
    assert!(result.is_native_intersection());
    assert_eq!(result.definition_id(), definition.id());
    assert!(result.achieved_chord_error_mm() <= SECTION_SAMPLING_TOLERANCE_MM);
    assert!(result.achieved_plane_error_mm() <= SECTION_PLANE_TOLERANCE_MM);
    for (i, boundary) in result.loops().iter().enumerate() {
        assert!(boundary.points_mm.len() >= 4);
        assert_eq!(boundary.points_mm.first(), boundary.points_mm.last());
        assert_eq!(area(boundary, result.frame()) < 0.0, boundary.is_hole);
        let spans = result
            .spans(i)
            .expect("native exact spans accompany polygons");
        assert!(!spans.is_empty());
        let owners = result.loop_source_face_ordinals(i).unwrap();
        for span in spans {
            assert_eq!(
                definition.face_ordinal(span.face),
                Some(span.source_face_ordinal)
            );
            assert!(owners.contains(&span.source_face_ordinal));
            assert!(span.trim_parameters.iter().all(|r| r[0] <= r[1]));
        }
        for j in 0..spans.len() {
            assert_eq!(
                spans[j].endpoints[1],
                spans[(j + 1) % spans.len()].endpoints[0]
            );
        }
    }
}
#[test]
fn box_cylinder_and_void_sections_preserve_native_area_orientation_and_lineage() {
    for (name, expected_area, loop_count, perimeter) in [
        ("box-mm.step", 200.0, 1, 60.0),
        ("cylinder.step", 25.0 * PI, 1, 10.0 * PI),
        ("through-hole.step", 400.0 - 9.0 * PI, 2, 80.0 + 6.0 * PI),
        (
            "independent-exporters/occt-cylinder-mm.step",
            25.0 * PI,
            1,
            10.0 * PI,
        ),
        (
            "independent-exporters/occt-through-hole-mm.step",
            400.0 - 9.0 * PI,
            2,
            80.0 + 6.0 * PI,
        ),
    ] {
        let definition = fixture(name);
        for normal in [[0.0, 0.0, 1.0], [0.0, 0.0, -1.0], [0.05, 0.05, 1.0]] {
            let result = query(&definition, 4.0, normal);
            assert_eq!(result.loops().len(), loop_count, "{name}");
            assert_provenance(&definition, &result);
            let measured: f64 = result.loops().iter().map(|l| area(l, result.frame())).sum();
            let scale = normal.iter().map(|v| v * v).sum::<f64>().sqrt();
            let allowance = perimeter * scale * SECTION_SAMPLING_TOLERANCE_MM
                + PI * SECTION_SAMPLING_TOLERANCE_MM.powi(2)
                + 1e-6;
            assert!(
                (measured - expected_area * scale).abs() <= allowance,
                "{name}: {measured}"
            );
        }
    }
}
#[test]
fn unchanged_independent_fitting_section_preserves_taper_and_bore() {
    let definition = fixture("independent-exporters/occt-tapered-fitting-mm.step");
    let result = query(&definition, 6.0, [0.0, 0.0, 1.0]);
    assert_provenance(&definition, &result);
    assert_eq!(result.loops().len(), 2);
    assert_eq!(result.loops().iter().filter(|l| l.is_hole).count(), 1);
    let measured: f64 = result.loops().iter().map(|l| area(l, result.frame())).sum();
    let allowance = 14.0 * PI * SECTION_SAMPLING_TOLERANCE_MM
        + 2.0 * PI * SECTION_SAMPLING_TOLERANCE_MM.powi(2)
        + 1e-7;
    assert!((measured - 21.0 * PI).abs() <= allowance, "{measured}");
}

#[test]
fn unchanged_bracket_periodic_bore_section_preserves_void_and_original_face() {
    let definition = fixture("independent-exporters/occt-ribbed-bracket-mm.step");
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(50_000_000);
    let prepared = prepare_section_source(
        &definition,
        MAX_PREPARED_SECTION_SOURCE_BYTES,
        &cancel,
        &mut work,
    )
    .expect("unchanged original bracket prepares with the standard section limits");
    let height = 0.9499998642846288;
    let result = prepared
        .section_with_byte_limit(
            PlaneMm {
                origin_mm: [0.0, 0.0, height],
                normal: [0.0, 0.0, 1.0],
            },
            &cancel,
            spiling_contracts::geometry::MAX_SECTION_BYTES,
            &mut work,
        )
        .expect("the original periodic bore must close at the actual refused operator plane");
    assert_canonical(&result);
    let bore_indices: Vec<_> = (0..result.loops.len())
        .filter(|&index| {
            result
                .loop_source_face_ordinals(index)
                .is_some_and(|owners| owners.contains(&31))
        })
        .collect();
    assert_eq!(
        bore_indices.len(),
        1,
        "retain the complete original bore cycle"
    );
    let bore_index = bore_indices[0];
    assert_eq!(result.loop_source_face_ordinals(bore_index).unwrap(), &[31]);
    let bore = &result.loops[bore_index];
    assert!(
        bore.is_hole,
        "the inward original cylinder bounds void, not material"
    );
    let radius = 1.3;
    for point in &bore.points_mm {
        assert!(
            ((point[0] - 24.0).hypot(point[1] - 11.0) - radius).abs() <= SECTION_PLANE_TOLERANCE_MM
        );
        assert!((point[2] - height).abs() <= SECTION_PLANE_TOLERANCE_MM);
    }
    let allowance = 2.0 * PI * radius * SECTION_SAMPLING_TOLERANCE_MM
        + PI * SECTION_SAMPLING_TOLERANCE_MM.powi(2);
    assert!(
        (signed_area(bore, &result.frame) + PI * radius * radius).abs() <= allowance,
        "the complete radius-1.3 void retains the declared chord-area enclosure"
    );
    // Independent XY ray parity checks the actual consumer polygons, not just
    // the hole label or the copied original-face inventory.
    let contains = |boundary: &SectionLoopMm, point: [f64; 2]| {
        boundary.points_mm.windows(2).fold(false, |inside, edge| {
            let [a, b] = [edge[0], edge[1]];
            inside
                ^ ((a[1] > point[1]) != (b[1] > point[1])
                    && point[0] < a[0] + (point[1] - a[1]) * (b[0] - a[0]) / (b[1] - a[1]))
        })
    };
    assert!(contains(bore, [24.0, 11.0]));
    let is_material = |point| {
        result
            .loops
            .iter()
            .fold(false, |inside, boundary| inside ^ contains(boundary, point))
    };
    assert!(
        !is_material([24.0, 11.0]),
        "the through-bore center stays void"
    );
    assert!(
        is_material([25.4, 11.0]),
        "the adjacent bracket wall stays material"
    );
    for index in 0..result.loops.len() {
        let owners = result.loop_source_face_ordinals(index).unwrap();
        assert!(!owners.is_empty());
        assert!(owners.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(owners.iter().all(|owner| *owner < definition.faces().len()));
    }
}

#[test]
fn unchanged_fitting_near_cap_section_preserves_original_back_root_and_material() {
    let definition = fixture("independent-exporters/occt-tapered-fitting-mm.step");
    let height = 0.0999999912280684;
    let result = section(
        &definition,
        PlaneMm {
            // Exact source-frame plane of the centered operator occurrence.
            origin_mm: [-110.0, -110.0, height],
            normal: [0.0, 0.0, 1.0],
        },
        &AtomicBool::new(false),
    )
    .expect("back-only original endpoint seeds its unchanged negative covering chart");
    assert_canonical(&result);
    assert_eq!(result.loops.len(), 2);
    assert!(!result.loops[0].is_hole);
    assert!(result.loops[1].is_hole);
    let outer_radius = 6.0 - height / 6.0;
    let expected = PI * (outer_radius * outer_radius - 4.0);
    let area: f64 = result
        .loops
        .iter()
        .map(|boundary| signed_area(boundary, &result.frame))
        .sum();
    let error = 2.0 * PI * (outer_radius + 2.0) * SECTION_SAMPLING_TOLERANCE_MM
        + 2.0 * PI * SECTION_SAMPLING_TOLERANCE_MM.powi(2)
        + 1e-7;
    assert!(
        (area - expected).abs() <= error,
        "area={area}, expected={expected}"
    );
    assert!((0..result.loops.len()).all(|index| {
        result
            .loop_source_face_ordinals(index)
            .is_some_and(|owners| !owners.is_empty())
    }));
}
#[test]
fn unchanged_housing_near_floor_preserves_trimmed_material_under_plane_reversal() {
    let definition = fixture("independent-exporters/occt-rounded-housing-mm.step");
    for height in [0.0125_f64, 0.4249999535616466] {
        let radius = (4.0 - (2.0 - height).powi(2)).sqrt();
        let expected = 280.0 + 68.0 * radius + PI * radius * radius;
        let perimeter = 68.0 + 2.0 * PI * radius;
        for normal_z in [1.0, -1.0] {
            let result = query(&definition, height, [0.0, 0.0, normal_z]);
            assert_provenance(&definition, &result);
            assert_eq!(result.loops().len(), 1);
            assert!(!result.loops()[0].is_hole);
            assert!(
                (area(&result.loops()[0], result.frame()) - expected).abs()
                    <= perimeter * SECTION_SAMPLING_TOLERANCE_MM
                        + PI * SECTION_SAMPLING_TOLERANCE_MM.powi(2)
                        + 1e-7
            );
        }
    }
}
#[test]
fn original_bracket_relevant_cuts_have_closed_native_span_incidence() {
    let definition = fixture("independent-exporters/occt-ribbed-bracket-mm.step");
    let bounds = definition.bounds_mm();
    for fraction in [0.123, 0.537, 0.913] {
        let height = bounds.min[2] + fraction * (bounds.max[2] - bounds.min[2]);
        let result = query(&definition, height, [0.0, 0.0, 1.0]);
        assert!(!result.loops().is_empty());
        assert_provenance(&definition, &result);
    }
}
#[test]
fn critical_plane_refuses_instead_of_fabricating_an_empty_section() {
    let definition = fixture("box-mm.step");
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(20_000_000);
    let error = section(
        &definition,
        PlaneMm {
            origin_mm: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
        },
        &cancel,
        MAX_SECTION_BYTES,
        &mut work,
    )
    .unwrap_err();
    assert_eq!(error.code, GeometryErrorCode::AnalysisIndeterminate);
}
#[test]
fn prepared_queries_keep_shared_admission_and_cancellation() {
    let definition = fixture("box-mm.step");
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(20_000_000);
    let prepared = prepare_section_source(
        &definition,
        MAX_PREPARED_SECTION_SOURCE_BYTES,
        &cancel,
        &mut work,
    )
    .unwrap();
    let plane = PlaneMm {
        origin_mm: [0.0, 0.0, 4.0],
        normal: [0.0, 0.0, 1.0],
    };
    assert_eq!(
        prepared
            .section_with_byte_limit(plane, &cancel, 1, &mut work)
            .unwrap_err()
            .code,
        GeometryErrorCode::ResourceLimit
    );
    cancel.store(true, std::sync::atomic::Ordering::Release);
    assert_eq!(
        prepared
            .section_with_byte_limit(plane, &cancel, MAX_SECTION_BYTES, &mut work)
            .unwrap_err()
            .code,
        GeometryErrorCode::Cancelled
    );
}

#[test]
fn plane_covector_family_preserves_regular_box_cut_and_refuses_vertex_contact() {
    use crate::parametric::Interval;
    let definition = fixture("box-mm.step");
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(20_000_000);
    let normal = [
        Interval::point(0.0),
        Interval::point(0.0),
        Interval::point(1.0),
    ];
    certify_plane_functional_family(
        &definition,
        normal,
        Interval::new(-4.000000001, -3.999999999).unwrap(),
        MAX_PREPARED_SECTION_BYTES,
        &mut work,
        &cancel,
    )
    .expect("entire coefficient family is regular");
    let error = certify_plane_functional_family(
        &definition,
        normal,
        Interval::new(-0.0001, 0.0001).unwrap(),
        MAX_PREPARED_SECTION_BYTES,
        &mut work,
        &cancel,
    )
    .unwrap_err();
    assert_eq!(error.code, GeometryErrorCode::AnalysisIndeterminate);
}
