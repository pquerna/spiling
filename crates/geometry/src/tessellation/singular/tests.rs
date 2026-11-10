// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use solidcore::{
    Brep, Point2, Point3, Vec3,
    nurbs::{b_spline_surface::BSplineSurface, math::CPoint, math::NOW, srf_basic},
    surface::surface::p2::SurfaceKind,
    topology::{Face, Shell},
};
use spiling_contracts::geometry::GeometryErrorCode;

fn fan_source(height: f64) -> (Surface, FaceMesh, Inventory) {
    let controls = [0.0, 1.0]
        .into_iter()
        .map(|horizontal| {
            vec![
                CPoint::new(2.0, 2.0, 0.0, NOW),
                CPoint::new(2.0 + 0.5 * horizontal, 2.5, 0.0, NOW),
                CPoint::new(2.0 + horizontal, 3.0, 1.0, NOW),
            ]
        })
        .collect();
    let raw = srf_basic::Surface::new(
        controls,
        1,
        2,
        vec![0.0, 0.0, 1.0, 1.0],
        vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
    )
    .unwrap();
    let surface = Surface::new(SurfaceKind::BSpline(BSplineSurface::from_nurb(raw, true)));
    let mut brep = Brep::new();
    let shell = brep.add_shell_record(Shell::default()).unwrap();
    let face = brep.add_face_record(Face::new(shell)).unwrap();
    let mesh = FaceMesh {
        face,
        uvs: vec![Point2::new(0.0, 0.0), Point2::new(0.5, height)],
        positions: vec![
            Point3::new(2.0, 2.0, 0.0),
            Point3::new(2.0 + 0.5 * height, 2.0 + height, height * height),
        ],
        normals: vec![Vec3::new(1.0, 0.0, 0.0); 2],
        shared: vec![Some(7), None],
        triangles: Vec::new(),
        gaps: vec![0.0; 2],
        inverted_edgeuses: Vec::new(),
        unconverged: false,
        split_boundary: false,
        ear_clipped: false,
        deviation: None,
        triangles_over: 0,
        backend: None,
        backend_failed: false,
        backend_deviation: None,
    };
    let inventory = Inventory {
        junctions: vec![Junction {
            shared: 7,
            source_loop: 0,
            uv: [[0.0, 0.0], [1.0, 0.0]],
            position: [2.0, 2.0, 0.0],
            normal: [0.0, 0.0, 1.0],
        }],
        locals: vec![Some(0), None],
        carrier_error: 0.0,
    };
    (surface, mesh, inventory)
}

#[test]
fn generic_collapsed_fan_keeps_positive_uv_and_complete_carrier_transport() {
    let (surface, mesh, inventory) = fan_source(0.002);
    let original = mesh.clone();
    assert_eq!(inventory.position(&mesh, 2), inventory.position(&mesh, 3));
    assert_ne!(inventory.uv(&mesh, 2), inventory.uv(&mesh, 3));
    let error = fan(
        &surface,
        &mesh,
        &inventory,
        [2, 3, 1],
        1.0,
        &mut GeometryWorkBudget::query(),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(error > 0.0 && error <= MESH_CARRIER_TOLERANCE_MM);
    assert_eq!(mesh, original);
    let error = fan(
        &surface,
        &mesh,
        &inventory,
        [3, 2, 1],
        1.0,
        &mut GeometryWorkBudget::query(),
        &AtomicBool::new(false),
    )
    .unwrap_err();
    assert_eq!(error.code, GeometryErrorCode::InvalidGeometry);
}

#[test]
fn generic_fan_refuses_excess_whole_patch_error_and_false_retained_spoke() {
    for height in [0.002, 1.0] {
        let (surface, mut mesh, inventory) = fan_source(height);
        if height < 1.0 {
            mesh.positions[1].z += 0.1;
        }
        let error = fan(
            &surface,
            &mesh,
            &inventory,
            [2, 3, 1],
            1.0,
            &mut GeometryWorkBudget::query(),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(error.code, GeometryErrorCode::ShapeErrorExceeded);
    }
}
