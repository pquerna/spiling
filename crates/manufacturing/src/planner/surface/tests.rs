// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
use super::*;
use geo::{Area, Intersects, LineString, Point, Polygon};
use spiling_contracts::{
    geometry::{OccurrenceId, SourceHash},
    surface::{ImageReliefParameters, SurfaceImageRecord, SurfaceLayerId},
};
use spiling_surface_fields::HeightImage;
use std::sync::Arc;

pub(super) fn mapping(depth: f64, margin: f64) -> ImageMapping {
    let mut bytes = b"P5\n3 3\n255\n".to_vec();
    bytes.extend([0, 0, 0, 255, 32, 170, 0, 0, 0]);
    let hash = SourceHash::from_bytes(&bytes);
    let record = SurfaceImageRecord {
        image_hash: hash.clone(),
        source_hash: hash,
        width: 3,
        height: 3,
        byte_count: bytes.len() as u64,
        source_name: "asymmetric.pgm".into(),
    };
    ImageMapping {
        image: Arc::new(HeightImage::from_canonical(record, Arc::new(bytes)).unwrap()),
        parameters: ImageReliefParameters {
            depth_mm: depth,
            u_repeats: 1,
            u_phase_fraction: 0.17,
            z_period_mm: 3.0,
            z_phase_mm: 0.0,
            edge_margin_mm: margin,
            vertical_error_mm: 0.02,
        },
        layer_id: SurfaceLayerId::new(1).unwrap(),
        occurrence_id: OccurrenceId::new(1).unwrap(),
        min_z: 0.0,
        translation_z: 0.0,
        sigma: 1.0,
    }
}
fn planar() -> PreparedFinish {
    PreparedFinish {
        shape: PreparedShape::Planar(RibField {
            face_id: FaceId::from_step_entity(1).unwrap(),
            bounds: AabbMm {
                min: [0.0, -0.81, 0.0],
                max: [10.0, 0.0, 3.0],
            },
            origin: [0.0, 0.0, 0.0],
            horizontal: [1.0, 0.0, 0.0],
            normal: [0.0, -1.0, 0.0],
            width: 10.0,
            depth: 0.8,
            pitch: 2.0,
            margin: 1.0,
            phase: 0.0,
            overlap: 0.1,
            samples: 129,
            ramp_samples: 64,
            interior_samples: 1,
            peak_mm: 0.8,
            image: Some(mapping(0.8, 1.0)),
        }),
    }
}
fn material() -> MultiPolygon<f64> {
    MultiPolygon(vec![Polygon::new(
        LineString(vec![
            Coord { x: 0.0, y: 0.0 },
            Coord { x: 10.0, y: 0.0 },
            Coord { x: 10.0, y: 5.0 },
            Coord { x: 0.0, y: 5.0 },
            Coord { x: 0.0, y: 0.0 },
        ]),
        vec![LineString(vec![
            Coord { x: 3.0, y: 2.0 },
            Coord { x: 3.0, y: 3.0 },
            Coord { x: 4.0, y: 3.0 },
            Coord { x: 4.0, y: 2.0 },
            Coord { x: 3.0, y: 2.0 },
        ])],
    )])
}

#[test]
fn native_island_ownership_is_exact_cancellable_and_never_exempts_other_ribs() {
    use geo::MapCoords;
    let cancel = AtomicBool::new(false);
    let mut field = planar();
    let native = material();
    assert!(
        field
            .owns_native_island(&native.0[0], 2.0, &mut 0, &cancel)
            .unwrap()
    );
    // Nearby/congruent and disjoint islands of the same occurrence cannot
    // inherit an image's advisory coverage exemption by bounding-box overlap.
    for delta in [0.01, 20.0] {
        let unrelated = native.0[0].map_coords(|point| Coord {
            x: point.x,
            y: point.y + delta,
        });
        assert!(
            !field
                .owns_native_island(&unrelated, 2.0, &mut 0, &cancel)
                .unwrap()
        );
    }
    assert!(
        !field
            .owns_native_island(&native.0[0], 3.1, &mut 0, &cancel)
            .unwrap()
    );
    assert_eq!(
        field
            .owns_native_island(&native.0[0], 2.0, &mut 0, &AtomicBool::new(true))
            .unwrap_err()
            .code,
        ManufacturingErrorCode::Cancelled
    );
    let mut exhausted_work = super::super::MAX_BOOLEAN_PAIRS;
    assert_eq!(
        field
            .owns_native_island(&native.0[0], 2.0, &mut exhausted_work, &cancel)
            .unwrap_err()
            .code,
        ManufacturingErrorCode::ResourceLimit
    );
    let PreparedShape::Planar(planar) = &mut field.shape else {
        unreachable!()
    };
    planar.image = None;
    assert!(
        !field
            .owns_native_island(&native.0[0], 2.0, &mut 0, &cancel)
            .unwrap()
    );
}

#[test]
fn required_mixed_face_material_keeps_all_ribs_and_only_omits_image_displacement() {
    let mut rib = planar();
    let PreparedShape::Planar(field) = &mut rib.shape else {
        unreachable!()
    };
    field.image = None;
    // The image fixture partitions its interior at pixel events. A strict rib
    // instead needs its admitted curvature sampling throughout the interior:
    // 6 mm / 384 = 1/64 mm, matching the 1 mm / 64 ramp spacing.
    field.interior_samples = 384;
    field.samples = 2 * field.ramp_samples + field.interior_samples;
    field.face_id = FaceId::from_step_entity(2).unwrap();
    field.origin = [10.0, 5.0, 0.0];
    field.horizontal = [-1.0, 0.0, 0.0];
    field.normal = [0.0, 1.0, 0.0];
    field.bounds.min = [0.0, 5.0, 0.0];
    field.bounds.max = [10.0, 5.81, 3.0];
    let fields = [planar(), rib];
    assert!(fields[0].is_image());
    assert!(!fields[1].is_image());
    let native = material();
    let cancel = AtomicBool::new(false);
    let mut work = 0;
    let required = realize_required(native.clone(), &fields, 2.0, 0, &mut work, &cancel).unwrap();
    assert!(work > 0);
    assert_eq!(required.0.len(), 1);
    assert_eq!(required.0[0].interiors(), native.0[0].interiors());
    for point in &native.0[0].exterior().0 {
        assert!(required.0[0].intersects(&Point(*point)));
    }
    // Independent geometric witnesses span every unprotected full rib lobe
    // on the opposite face of this very same image-owned native island.
    for x in [3.0, 5.0, 7.0] {
        assert!(required.0[0].intersects(&Point::new(x, 5.79)));
    }
    assert!(
        required.0[0]
            .exterior()
            .0
            .iter()
            .all(|point| point.y >= 0.0)
    );
    let requested = realize(native.clone(), &fields, 2.0, 0, &mut 0, &cancel).unwrap();
    assert!(
        requested.0[0]
            .exterior()
            .0
            .iter()
            .any(|point| point.y < -0.1)
    );
    assert!(requested.unsigned_area() > required.unsigned_area());
    assert!(required.unsigned_area() > native.unsigned_area());
    assert_eq!(
        required,
        realize_required(native.clone(), &fields, 0.5, 0, &mut 0, &cancel).unwrap()
    );
    assert_eq!(
        native,
        realize_required(native.clone(), &fields[..1], 2.0, 0, &mut 0, &cancel).unwrap()
    );
    assert_eq!(
        realize_required(
            native.clone(),
            &fields,
            2.0,
            MAX_DEPOSITION_SEGMENTS as usize,
            &mut 0,
            &cancel
        )
        .unwrap_err()
        .code,
        ManufacturingErrorCode::ResourceLimit,
    );
    let mut exhausted_work = super::super::MAX_BOOLEAN_PAIRS;
    assert_eq!(
        realize_required(
            native.clone(),
            &fields,
            2.0,
            0,
            &mut exhausted_work,
            &cancel
        )
        .unwrap_err()
        .code,
        ManufacturingErrorCode::ResourceLimit,
    );
    assert_eq!(
        realize_required(native, &fields, 2.0, 0, &mut 0, &AtomicBool::new(true))
            .unwrap_err()
            .code,
        ManufacturingErrorCode::Cancelled,
    );
}
#[test]
fn blank_rows_are_exact_native_and_bright_rows_retain_native_and_holes() {
    let fields = [planar()];
    let native = material();
    let cancel = AtomicBool::new(false);
    let blank = realize(native.clone(), &fields, 0.5, 0, &mut 0, &cancel).unwrap();
    assert_eq!(blank, native);
    assert!(fields[0].is_z_invariant(0.0, 1.0).unwrap());
    assert_eq!(fields[0].vertical_error_bound(0.0, 1.0).unwrap(), 0.0);
    assert!(!fields[0].is_z_invariant(1.0, 2.0).unwrap());
    assert!(fields[0].vertical_error_bound(1.0, 2.0).unwrap() > 0.02);
    let bright = realize(native.clone(), &fields, 2.0, 0, &mut 0, &cancel).unwrap();
    assert!(bright.unsigned_area() > native.unsigned_area());
    assert_eq!(bright.0[0].interiors(), native.0[0].interiors());
    for p in &native.0[0].exterior().0 {
        assert!(bright.0[0].intersects(&Point(*p)));
    }
    let ring = &bright.0[0].exterior().0;
    assert_eq!(ring.first(), ring.last());
    assert!(ring.contains(&Coord { x: 0.0, y: 0.0 }) && ring.contains(&Coord { x: 10.0, y: 0.0 }));
    let mut events = vec![];
    fields[0]
        .append_z_events(0.0, 3.0, &mut events, 16, &cancel)
        .unwrap();
    assert_eq!(events, vec![1.0, 2.0]);
}
#[test]
fn flipped_source_z_mapping_preserves_phase_and_reverses_events() {
    let mut field = planar();
    let PreparedShape::Planar(planar) = &mut field.shape else {
        unreachable!()
    };
    let image = planar.image.as_mut().unwrap();
    image.sigma = -1.0;
    image.translation_z = 3.0;
    assert_eq!(image.source_z(1.0), 2.0);
    assert!(!image.zero_row(1.0));
    assert!(image.zero_row(2.5));
    let mut events = vec![];
    field
        .append_z_events(0.0, 3.0, &mut events, 16, &AtomicBool::new(false))
        .unwrap();
    events.sort_by(f64::total_cmp);
    assert_eq!(events, vec![1.0, 2.0]);
}

#[test]
fn actual_first_wall_diagnostics_detect_displaced_lobes_with_identical_extrema() {
    let fields = [planar()];
    let native = material();
    let cancel = AtomicBool::new(false);
    let realized = realize(native.clone(), &fields, 2.0, 0, &mut 0, &cancel).unwrap();
    let first = region::offset(&realized, -0.2, &cancel).unwrap();
    let good = collect_resolution(
        &first, &realized, &native, &fields, 2.0, 0, 0.4, &mut 0, &cancel,
    )
    .unwrap();
    good[0].validate().unwrap();
    assert!(good[0].evaluated_witnesses > 8);
    assert!(good[0].representatives_truncated);
    assert_eq!(good[0].unavailable, 0);
    let mut shifted = planar();
    let PreparedShape::Planar(f) = &mut shifted.shape else {
        unreachable!()
    };
    f.image.as_mut().unwrap().parameters.u_phase_fraction = 0.5;
    let shifted_material = realize(native.clone(), &[shifted], 2.0, 0, &mut 0, &cancel).unwrap();
    let peak = |material: &MultiPolygon<f64>| {
        material.0[0]
            .exterior()
            .0
            .iter()
            .map(|p| -p.y)
            .fold(0.0f64, f64::max)
    };
    assert!((peak(&realized) - peak(&shifted_material)).abs() < SURFACE_TOLERANCE_MM);
    let wrong_wall = region::offset(&shifted_material, -0.2, &cancel).unwrap();
    let bad = collect_resolution(
        &wrong_wall,
        &realized,
        &native,
        &fields,
        2.0,
        0,
        0.4,
        &mut 0,
        &cancel,
    )
    .unwrap();
    bad[0].validate().unwrap();
    assert!(
        bad[0].unavailable > 0
            || bad[0].max_residual_lower_mm > good[0].max_residual_lower_mm + SURFACE_TOLERANCE_MM
    );
    let mut exhausted_work = super::super::MAX_BOOLEAN_PAIRS;
    let incomplete = collect_resolution(
        &first,
        &realized,
        &native,
        &fields,
        2.0,
        0,
        0.4,
        &mut exhausted_work,
        &cancel,
    )
    .unwrap();
    assert!(!incomplete[0].coverage_complete);
    assert_eq!(incomplete[0].evaluated_witnesses, 0);
    incomplete[0].validate().unwrap();
}
