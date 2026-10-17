// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Original native operator regressions; never an import repair path.
mod parity;
use monstertruck_io::step::{
    load::step_geometry::{Curve3D, ElementarySurface, Surface, SweepSurface, re_exports::*},
    save::{CompleteStepDisplay, StepHeaderDescriptor, StepModel},
};
use monstertruck_modeling::builder;
use monstertruck_topology::{Face, Solid, Wire};
use sha2::{Digest, Sha256};
use std::{error::Error, path::PathBuf};

fn prism(points_xz: &[(f64, f64)], depth: f64) -> Solid<Point3, Curve3D, Surface> {
    // XZ counterclockwise faces point toward -Y; align the face with +Y extrusion.
    let vertices = builder::vertices(points_xz.iter().rev().map(|&(x, z)| (x, 0.0, z)));
    let wire: Wire<Point3, Curve3D> = (0..vertices.len())
        .map(|i| builder::line(&vertices[i], &vertices[(i + 1) % vertices.len()]))
        .collect();
    let face: Face<Point3, Curve3D, Surface> =
        builder::try_attach_plane(vec![wire]).expect("original planar wire");
    let solid = builder::extrude(&face, depth * Vector3::unit_y());
    let solid = solid
        .try_mapped(
            |p| Some(*p),
            |c| Some(c.clone()),
            |surface| {
                if let Surface::SweepSurface(SweepSurface::ExtrusionSurface(extrusion)) = surface {
                    let Curve3D::Line(line) = extrusion.entity_curve().as_ref() else {
                        return None;
                    };
                    Some(Surface::ElementarySurface(ElementarySurface::Plane(
                        Plane::new(line.0, line.1, line.0 + extrusion.extruding_vector()),
                    )))
                } else {
                    Some(surface.clone())
                }
            },
        )
        .expect("exact original planar extrusion carriers");
    for shell in solid.boundaries() {
        shell
            .check_solid_boundary()
            .expect("original closed manifold");
    }
    solid
}

fn circular_tunnel(center_x: f64, center_z: f64, radius: f64) -> Solid<Point3, Curve3D, Surface> {
    let solid = parity::extruded(
        vec![
            parity::rectangle(20.0, 7.85),
            parity::circle(center_x, center_z, radius).inverse(),
        ],
        10.0,
    );
    // Exact positive-orientation coordinate permutation: the original XY
    // profile becomes XZ, and the analytic bore runs through +Y. No fitted
    // carriers, Boolean subtraction or rounded rotation is involved.
    let frame = Matrix4::from_cols(
        Vector4::new(-1.0, 0.0, 0.0, 0.0),
        Vector4::new(0.0, 0.0, 1.0, 0.0),
        Vector4::new(0.0, 1.0, 0.0, 0.0),
        Vector4::new(20.0, 0.0, 0.0, 1.0),
    );
    solid
        .try_mapped(
            |point| Some(frame.transform_point(*point)),
            |curve| Some(curve.transformed(frame)),
            |surface| Some(surface.transformed(frame)),
        )
        .expect("exact original cross-axis cylindrical tunnel")
}

fn main() -> std::result::Result<(), Box<dyn Error>> {
    let check = std::env::args().skip(1).any(|arg| arg == "--check");
    let root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/manufacturing/native");
    if !check {
        std::fs::create_dir_all(&root)?;
    }
    let fixtures: [(&str, &[(f64, f64)], Option<[f64; 3]>); 4] = [
        (
            "nonintegral-top",
            &[(0.0, 0.0), (20.0, 0.0), (20.0, 7.85), (0.0, 7.85)],
            None,
        ),
        (
            "non-grid-shelf",
            &[
                (0.0, 0.0),
                (20.0, 0.0),
                (20.0, 3.15),
                (12.0, 3.15),
                (12.0, 7.85),
                (0.0, 7.85),
            ],
            None,
        ),
        (
            "inward-planar-ramp",
            &[(0.0, 0.0), (20.0, 0.0), (16.0, 7.85), (0.0, 7.85)],
            None,
        ),
        (
            "off-grid-circular-tunnel",
            &[(0.0, 0.0), (20.0, 0.0), (20.0, 7.85), (0.0, 7.85)],
            Some([10.0, 1.37, 0.24]),
        ),
    ];
    let mut manifest = Vec::new();
    let notice = b"SPDX-FileCopyrightText: 2026 Spiling contributors\nSPDX-License-Identifier: OSL-3.0\nOriginal Spiling native planar operator fixture; no external CAD source.\n";
    for (name, points, tunnel) in fixtures {
        let solid = match tunnel {
            Some([x, z, radius]) => circular_tunnel(x, z, radius),
            None => prism(points, 10.0),
        };
        let compressed = solid.compress();
        let header = StepHeaderDescriptor {
            file_name: format!("{name}.step"),
            time_stamp: "2026-01-01T00:00:00".into(),
            authors: vec!["Spiling contributors".into()],
            organization: vec!["Spiling".into()],
            organization_system: "Spiling original native operator corpus".into(),
            authorization: "OSL-3.0".into(),
        };
        let bytes = format!(
            "{}",
            CompleteStepDisplay::new(StepModel::from(&compressed), header)
        )
        .into_bytes();
        let path = root.join(format!("{name}.step"));
        output(&path, &bytes, check)?;
        let fixture_notice = if tunnel.is_some() {
            b"SPDX-FileCopyrightText: 2026 Spiling contributors\nSPDX-License-Identifier: OSL-3.0\nOriginal Spiling native cylindrical-tunnel operator fixture; no external CAD source.\n".as_slice()
        } else {
            notice.as_slice()
        };
        output(&root.join(format!("{name}.step.license")), fixture_notice, check)?;
        let hash = format!("{:x}", Sha256::digest(&bytes));
        println!("{name}.step {hash}");
        let mut entry = serde_json::json!({"file":format!("{name}.step"), "sha256":hash, "depth_mm":10.0, "native_xz_vertices_mm":points});
        if let Some(circle) = tunnel {
            entry["native_y_tunnel_circle_xz_mm"] = serde_json::json!(circle);
        }
        manifest.push(entry);
    }
    let mut bytes = serde_json::to_vec_pretty(&serde_json::json!({
        "schema_version":1, "software_only":true, "SPDX-License-Identifier":"OSL-3.0",
        "source_recipe":"crates/geometry/examples/generate_operator_fixtures.rs", "fixtures":manifest,
    }))?;
    bytes.push(b'\n');
    output(&root.join("manifest.json"), &bytes, check)?;
    output(&root.join("manifest.json.license"), notice, check)?;
    Ok(())
}

fn output(
    path: &std::path::Path,
    bytes: &[u8],
    check: bool,
) -> std::result::Result<(), Box<dyn Error>> {
    if check {
        if std::fs::read(path)? != bytes {
            return Err(format!("native fixture drift: {}", path.display()).into());
        }
    } else {
        std::fs::write(path, bytes)?;
    }
    Ok(())
}
