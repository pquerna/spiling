// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::*;
use spiling_model_import::{Affine, Mesh, threemf::*};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};
fn append(a: &mut Mesh, b: Mesh) {
    let offset = a.vertices.len() as u32;
    a.vertices.extend(b.vertices);
    a.triangles
        .extend(b.triangles.iter().map(|t| t.map(|v| v + offset)));
}
fn area(c: &SurfaceComplex, z: f64) -> f64 {
    TolerantModel::from_complex(c, TolerantPolicy::default())
        .unwrap()
        .section(z)
        .unwrap()
        .loops
        .iter()
        .map(|r| r.area)
        .sum()
}
#[test]
fn repaired_boundary_and_debris_have_locations_and_keep_material() {
    let cancel = AtomicBool::new(false);
    let mut mesh = tests::cube(0., 10.);
    mesh.vertices.push([1000.; 3]);
    mesh.triangles.push([8, 8, 8]);
    mesh.vertices.push([0., 0., 0.00001]);
    mesh.triangles[0][0] = 9;
    let (c, report) = normalize::build(mesh, 1., Affine::IDENTITY, &cancel).unwrap();
    assert!((area(&c, 5.) - 100.).abs() < 0.001);
    assert!(c.bounds().unwrap().max[0] < 11.);
    for code in [
        ImportFindingCode::BoundaryWeld,
        ImportFindingCode::DegenerateFacets,
    ] {
        let finding = report.iter().find(|f| f.code == code).unwrap();
        assert_eq!(finding.count, 1);
        assert!(finding.bounds_mm.is_some());
        finding.validate().unwrap();
    }
    assert!(c.error_mm >= 0.00001);
    let mut open = tests::cube(0., 10.);
    open.triangles.drain(..2);
    assert!(SurfaceComplex::normalize(open, 1., Affine::IDENTITY, &cancel).is_err());
}
#[test]
fn overlapping_and_coincident_components_compose_without_losing_material() {
    let cancel = AtomicBool::new(false);
    let mut input = tests::cube(0., 10.);
    append(&mut input, tests::cube(5., 15.));
    let c = SurfaceComplex::normalize(input, 1., Affine::IDENTITY, &cancel).unwrap();
    for (z, expected) in [(2., 100.), (7., 175.), (12., 100.)] {
        assert!((area(&c, z) - expected).abs() < 0.001);
    }
    let mut input = tests::cube(0., 10.);
    append(&mut input, tests::cube(0., 10.));
    let c = SurfaceComplex::normalize(input, 1., Affine::IDENTITY, &cancel).unwrap();
    assert!((area(&c, 5.) - 100.).abs() < 0.001);
}
#[test]
fn nested_island_is_not_erased_with_the_surrounding_void() {
    let cancel = AtomicBool::new(false);
    let mut input = tests::cube(0., 10.);
    append(&mut input, tests::cube(2., 8.));
    append(&mut input, tests::cube(4., 6.));
    let c = SurfaceComplex::normalize(input, 1., Affine::IDENTITY, &cancel).unwrap();
    assert!((area(&c, 5.) - 68.).abs() < 0.001);
}
#[test]
fn negative_composition_stays_inside_its_build_object_and_reuses_instances() {
    let cancel = AtomicBool::new(false);
    let key = |id| ObjectKey {
        part: "/3D/root.model".into(),
        id,
    };
    let instance = |id| Instance {
        object: key(id),
        transform: Affine::IDENTITY,
        printable: true,
    };
    let package = Package {
        root: "/3D/root.model".into(),
        models: BTreeMap::from([(
            "/3D/root.model".into(),
            Model {
                scale_to_mm: 1.,
                objects: BTreeMap::new(),
                build: vec![instance(1), instance(2), instance(1)],
            },
        )]),
        omitted: BTreeSet::new(),
        plates: BTreeMap::new(),
        roles: BTreeMap::new(),
    };
    let leaf = |id, build_index, role, mesh, translation| {
        let mut transform = Affine::IDENTITY;
        transform.0[3] = translation;
        Leaf {
            object: key(id),
            mesh: Arc::new(mesh),
            volume_index: 0,
            transform_mm: transform,
            source_scale_to_mm: 1.,
            role,
            name: "part".into(),
            build_index,
        }
    };
    let mut cut = tests::cube(2., 8.);
    for p in &mut cut.vertices {
        p[2] = if p[2] < 5. { -1. } else { 11. };
    }
    let leaves = vec![
        leaf(1, 0, Role::Model, tests::cube(0., 10.), [0.; 3]),
        leaf(3, 0, Role::Negative, cut.clone(), [0.; 3]),
        leaf(2, 1, Role::Model, tests::cube(0., 10.), [0.; 3]),
        leaf(1, 2, Role::Model, tests::cube(0., 10.), [20., 0., 0.]),
        leaf(3, 2, Role::Negative, cut, [20., 0., 0.]),
    ];
    let result = composition::negative_objects(&package, leaves, &[], 1, &cancel).unwrap();
    assert_eq!(result.len(), 2);
    assert!((area(&result[0].complex, 5.) - 64.).abs() < 0.001);
    assert!((area(&result[1].complex, 5.) - 100.).abs() < 0.001);
    assert_eq!(result[0].placements.len(), 2);
    let restored = SurfaceComplex::decode(&result[0].complex.encode().unwrap()).unwrap();
    assert!((area(&restored, 5.) - 64.).abs() < 0.001);
}

#[test]
fn a_component_in_a_concave_bbox_void_is_added_not_misclassified_as_a_cavity() {
    let cancel = AtomicBool::new(false);
    let mut corner = tests::cube(4., 11.);
    for p in &mut corner.vertices {
        p[2] = if p[2] < 5. { -1. } else { 11. };
    }
    let mut input = csg::compose(tests::cube(0., 10.), corner, true, &cancel).unwrap();
    append(&mut input, tests::cube(5., 8.));
    let c = SurfaceComplex::normalize(input, 1., Affine::IDENTITY, &cancel).unwrap();
    assert!((area(&c, 6.) - 73.).abs() < 0.001);
}

#[test]
fn located_controls_follow_the_inverse_occurrence_frame() {
    let angle = std::f64::consts::FRAC_PI_2;
    let pose = RigidPoseMm {
        rotation_xyzw: [0., 0., (angle / 2.).sin(), (angle / 2.).cos()],
        translation_mm: [20., 30., 0.],
    };
    let transform = Affine([[0., 1., 0.], [-1., 0., 0.], [0., 0., 1.], [20., 30., 0.]]);
    let relative = composition::relative_transform(transform, pose);
    let p = relative.apply([2., 4., 6.]);
    for (actual, expected) in p.into_iter().zip([2., 4., 6.]) {
        assert!((actual - expected).abs() < 1e-10);
    }
}

#[test]
fn cancelled_import_never_publishes_a_part() {
    let cancel = AtomicBool::new(true);
    let error =
        SurfaceComplex::normalize(tests::cube(0., 10.), 1., Affine::IDENTITY, &cancel).unwrap_err();
    assert_eq!(error.code, GeometryErrorCode::Cancelled);
}

#[test]
fn boundary_join_that_collapses_a_face_reports_unresolved_material() {
    // The slit has uniquely paired boundary vertices. Closing it makes every
    // edge two-sided, but collapses the skinny face to a line.
    let mesh = Mesh {
        vertices: vec![
            [0., 0., 0.],
            [10., 0., 0.],
            [0., 10., 0.],
            [0., 0., 10.],
            [5., 0., 0.],
            [5., 0.00001, 0.],
        ],
        triangles: vec![
            [0, 2, 4],
            [4, 2, 1],
            [0, 1, 3],
            [1, 2, 3],
            [0, 3, 2],
            [0, 5, 1],
        ],
    };
    let error =
        SurfaceComplex::normalize(mesh, 1., Affine::IDENTITY, &AtomicBool::new(false)).unwrap_err();
    assert_eq!(error.code, GeometryErrorCode::UnsupportedGeometry);
}

#[test]
fn rejected_mesh_reports_actual_open_edges_and_never_adopts_a_source() {
    let cancel = AtomicBool::new(false);
    let mut mesh = tests::cube(0., 10.);
    mesh.triangles.drain(..2);
    let failure = normalize::build_report(mesh, 1., Affine::IDENTITY, &cancel).unwrap_err();
    let diagnostic = failure.error.import_diagnostic.as_ref().unwrap();
    assert_eq!(diagnostic.kind, ImportFailureKind::OpenBoundary);
    let bounds = diagnostic.bounds_mm.unwrap();
    assert!(bounds.max.iter().zip(bounds.min).any(|(hi, lo)| *hi == lo)); // A local boundary plane, not a whole-part box.
    failure.mesh.as_ref().unwrap().validate().unwrap();
    failure.error.validate().unwrap();
    let mut invalid = tests::cube(0., 10.);
    invalid.vertices[0][0] = f64::NAN;
    let failure = normalize::build_report(invalid, 1., Affine::IDENTITY, &cancel).unwrap_err();
    assert!(failure.mesh.is_none());
    assert!(failure.error.import_diagnostic.is_none());
}
