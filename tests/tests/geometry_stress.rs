// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_contracts::geometry::*;
use spiling_e2e_tests::fixtures::*;
use spiling_e2e_tests::helpers::*;
use spiling_geometry::*;
use std::f64::consts::PI;
use std::sync::atomic::AtomicBool;

#[test]
#[ignore = "oversized geometry stress; run pnpm test:geometry-stress"]
fn oversized_plate_preserves_source_material_voids_and_resource_admission() {
    let definition = import_test_step(&fixture_bytes("perforated-plate.step")).unwrap();
    assert_source_face_identity(&definition, "perforated-plate.step");
    assert_fixture_enclosure(&definition, "perforated-plate.step");
    let bounds = precision_bounds(&definition, 0.0001).unwrap();
    for (axis, expected) in [1640.0, 840.0, 20.0].into_iter().enumerate() {
        assert_close(bounds.min[axis], 0.0, 0.0001, "plate minimum");
        assert_close(bounds.max[axis], expected, 0.0001, "plate maximum");
    }
    let section = section_at_z(&definition, 10.0).unwrap();
    assert_eq!(section.loops().len(), 513);
    assert_eq!(
        section
            .loops()
            .iter()
            .filter(|boundary| boundary.is_hole)
            .count(),
        512
    );
    let chord_error = section.achieved_chord_error_mm();
    let hole_error = 2.0 * PI * 20.0 * chord_error + PI * chord_error.powi(2);
    let perimeter = 2.0 * (1640.0 + 840.0) + 512.0 * 2.0 * PI * 20.0;
    let area_error = perimeter * chord_error + 513.0 * PI * chord_error.powi(2);
    let mut material_area = 0.0;
    for boundary in section.loops() {
        assert_eq!(boundary.points_mm.first(), boundary.points_mm.last());
        let signed_area = loop_area(&section, boundary);
        assert_eq!(signed_area < 0.0, boundary.is_hole);
        if boundary.is_hole {
            assert_close(
                signed_area.abs(),
                PI * 20.0_f64.powi(2),
                hole_error,
                "hole area",
            );
        }
        material_area += signed_area;
    }
    assert_close(
        material_area,
        1640.0 * 840.0 - 512.0 * PI * 20.0_f64.powi(2),
        area_error,
        "plate material area",
    );
    let packed_bytes = spiling_contracts::display::packed_section_size(
        section
            .loops()
            .iter()
            .map(|boundary| boundary.points_mm.len()),
        MAX_GEOMETRY_CHUNK_BYTES,
    )
    .unwrap();
    assert!(packed_bytes <= MAX_SECTION_BYTES as usize);
    let native_point_bytes: usize = section
        .loops()
        .iter()
        .map(|boundary| boundary.points_mm.capacity() * std::mem::size_of::<[f64; 3]>())
        .sum();
    assert!(section.retained_bytes() >= native_point_bytes);
    assert!(section.retained_bytes() <= MAX_PREPARED_SECTION_BYTES);
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::query();
    let prepared = prepare_section_source(
        &definition,
        MAX_PREPARED_SECTION_SOURCE_BYTES,
        &cancel,
        &mut work,
    )
    .unwrap();
    let error = prepared
        .section_with_byte_limit(section.plane(), &cancel, 1, &mut work)
        .unwrap_err();
    assert_eq!(error.code, GeometryErrorCode::ResourceLimit);
}
