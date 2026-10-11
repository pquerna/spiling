// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Deterministic original corpus recipe. Run without --check to stage bytes;
//! --check requires exact regenerated drift equality and never modifies files.
mod corpus;
use monstertruck_io::step::{
    load::step_geometry::{
        Conic3D, Curve3D, ElementarySurface, Surface, SweepSurface, re_exports::*,
    },
    save::{CompleteStepDisplay, StepHeaderDescriptor, StepModel},
};
use monstertruck_modeling::builder;
use monstertruck_topology::{Face, Solid, Wire};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    error::Error,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

type StepSolid = Solid<Point3, Curve3D, Surface>;
type StepWire = Wire<Point3, Curve3D>;
fn rectangle(width: f64, depth: f64) -> StepWire {
    let vertices = builder::vertices([
        (0., 0., 0.),
        (width, 0., 0.),
        (width, depth, 0.),
        (0., depth, 0.),
    ]);
    (0..4)
        .map(|i| builder::line::<Curve3D>(&vertices[i], &vertices[(i + 1) % 4]))
        .collect()
}
fn circle(x: f64, y: f64, radius: f64) -> StepWire {
    builder::revolve(
        &builder::vertex((x + radius, y, 0.)),
        Point3::new(x, y, 0.),
        Vector3::unit_z(),
        builder::SweepAngle::Closed,
        2,
    )
}
fn analytic(surface: &Surface) -> Option<Surface> {
    let Surface::SweepSurface(SweepSurface::ExtrusionSurface(extrusion)) = surface else {
        return Some(surface.clone());
    };
    let vector = extrusion.extruding_vector();
    match extrusion.entity_curve() {
        Curve3D::Line(line) => Some(Surface::ElementarySurface(ElementarySurface::Plane(
            Plane::new(line.0, line.1, line.0 + vector),
        ))),
        Curve3D::Conic(Conic3D::Ellipse(c)) => {
            let center = c.transform().transform_point(Point3::origin());
            let u = c.transform().transform_vector(Vector3::unit_x());
            let v = c.transform().transform_vector(Vector3::unit_y());
            if (u.magnitude() - v.magnitude()).abs() > 1e-10
                || u.dot(v).abs() > 1e-10
                || u.dot(vector).abs() > 1e-10
                || v.dot(vector).abs() > 1e-10
            {
                return None;
            }
            let point = extrusion.subs(0.37, 0.73);
            let axis = vector.normalize();
            let delta = point - center;
            let radial = delta - delta.dot(axis) * axis;
            let expected_outward = extrusion.normal(0.37, 0.73).dot(radial) > 0.0;
            let line = Line(center + u, center + u + vector);
            let mut processor =
                Processor::new(RevolutionSurface::by_revolution(line, center, axis));
            let p = processor.subs(0.37, 0.73);
            let d = p - center;
            let outward = processor.normal(0.37, 0.73).dot(d - d.dot(axis) * axis) > 0.0;
            if outward != expected_outward {
                processor.invert();
            }
            Some(Surface::ElementarySurface(
                ElementarySurface::CylindricalSurface(processor),
            ))
        }
        _ => None,
    }
}
fn extruded(wires: Vec<StepWire>, thickness: f64) -> StepSolid {
    let face: Face<Point3, Curve3D, Surface> =
        builder::try_attach_plane(wires).expect("original planar loops");
    let solid: StepSolid = builder::extrude(&face, thickness * Vector3::unit_z());
    // Deliberate source-construction normalization: exact conic extrusion is a cylinder,
    // not a spline. This runs before writing, never as an importer repair heuristic.
    let solid = solid
        .try_mapped(|p| Some(*p), |c| Some(c.clone()), analytic)
        .expect("original analytic extrusion carriers");
    for shell in solid.boundaries() {
        shell
            .check_solid_boundary()
            .expect("original closed manifold recipe");
    }
    solid
}
fn original(name: &str, solid: StepSolid) -> String {
    let compressed = solid.compress();
    let header = StepHeaderDescriptor {
        file_name: name.into(),
        time_stamp: "2026-01-01T00:00:00".into(),
        authors: vec!["Spiling contributors".into()],
        organization: vec!["Spiling".into()],
        organization_system: "Spiling original analytic corpus".into(),
        authorization: "OSL-3.0".into(),
    };
    format!(
        "{}",
        CompleteStepDisplay::new(StepModel::from(&compressed), header)
    )
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn license() -> Value {
    json!({"SPDX-FileCopyrightText":"2026 Spiling contributors","SPDX-License-Identifier":"OSL-3.0","origin":"Original Spiling public native builder recipe; no external CAD source"})
}
const ASSET_NOTICE: &[u8] = b"SPDX-FileCopyrightText: 2026 Spiling contributors\nSPDX-License-Identifier: OSL-3.0\nLicensed under the Open Software License version 3.0\n\nOriginal Spiling analytic corpus and evaluation recipes; no external CAD source.\n";
fn write_or_check(
    path: &Path,
    bytes: &[u8],
    check: bool,
) -> std::result::Result<(), Box<dyn Error>> {
    if check {
        if std::fs::read(path)? != bytes {
            return Err(format!("deterministic corpus drift: {}", path.display()).into());
        }
    } else {
        if path
            .extension()
            .is_some_and(|extension| extension == "step")
            && path.exists()
        {
            if std::fs::read(path)? != bytes {
                return Err(format!(
                    "frozen STEP source changed: {}; introduce a new fixture name/version",
                    path.display()
                )
                .into());
            }
            return Ok(());
        }
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(path, bytes)?;
    }
    Ok(())
}
fn pretty(value: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(value).unwrap();
    bytes.push(b'\n');
    bytes
}
fn main() -> std::result::Result<(), Box<dyn Error>> {
    let mut check = false;
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/geometry");
    let mut arguments = std::env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--check" {
            check = true;
        } else if argument == "--out" {
            root = PathBuf::from(arguments.next().ok_or("--out requires directory")?);
        } else {
            return Err("usage: generate_corpus [--check] [--out DIRECTORY]".into());
        }
    }
    let mut files = BTreeMap::<String, Vec<u8>>::new();
    let mut entries = Vec::new();
    let mm = original("box-mm.step", extruded(vec![rectangle(20., 10.)], 8.));
    let inch_exchange = corpus::inches(&mm);
    let inch = corpus::serialize(&inch_exchange);
    let cylinder = original("cylinder.step", extruded(vec![circle(0., 0., 5.)], 8.));
    let hole = original(
        "through-hole.step",
        extruded(
            vec![rectangle(20., 20.), circle(10., 10., 3.).inverse()],
            8.,
        ),
    );
    let columns = 32;
    let rows = 16;
    let width = (columns - 1) as f64 * 50. + 90.;
    let depth = (rows - 1) as f64 * 50. + 90.;
    let mut wires = vec![rectangle(width, depth)];
    for row in 0..rows {
        for col in 0..columns {
            wires.push(circle(45. + col as f64 * 50., 45. + row as f64 * 50., 20.).inverse());
        }
    }
    let plate = original("perforated-plate.step", extruded(wires, 20.));
    let cancel = AtomicBool::new(false);
    for (name, bytes, expectation) in [
        (
            "box-mm.step",
            mm.clone(),
            json!({"physical_bounds_mm":{"min":[0,0,0],"max":[20,10,8]},"face_count":6,"section_z_mm":4,"section_area_mm2":200}),
        ),
        (
            "box-inch.step",
            inch.clone(),
            json!({"physical_bounds_mm":{"min":[0,0,0],"max":[20,10,8]},"face_count":6,"source_unit":"inch","section_z_mm":4,"section_area_mm2":200}),
        ),
        (
            "cylinder.step",
            cylinder,
            json!({"physical_bounds_mm":{"min":[-5,-5,0],"max":[5,5,8]},"radius_mm":5,"section_z_mm":4,"section_area_expression":"25*pi"}),
        ),
        (
            "through-hole.step",
            hole,
            json!({"physical_bounds_mm":{"min":[0,0,0],"max":[20,20,8]},"hole_radius_mm":3,"section_z_mm":4,"section_loop_count":2,"section_area_expression":"400-9*pi"}),
        ),
        (
            "perforated-plate.step",
            plate,
            json!({"columns":columns,"rows":rows,"radius_mm":20,"pitch_mm":50,"thickness_mm":20,"exterior_margin_mm":25,"physical_bounds_mm":{"min":[0,0,0],"max":[width,depth,20]},"packed_mesh_bytes":{"min_exclusive":4194304,"max_inclusive":67108864},"source_geometry_frozen":true}),
        ),
    ] {
        // Prevent committing source bytes that the actual public adapter does not admit.
        spiling_geometry::import_step(bytes.as_bytes(), &cancel)
            .map_err(|e| format!("original {name} fails admission: {e:?}"))?;
        entries.push(json!({"path":name,"sha256":hash(bytes.as_bytes()),"byte_count":bytes.len(),"expected":"accepted","recipe":"public analytic planar-wire extrusion","expectation":expectation}));
        files.insert(name.into(), bytes.into_bytes());
    }
    for adversary in corpus::adversaries(&mm, &inch) {
        let result = spiling_geometry::import_step(adversary.bytes.as_bytes(), &cancel);
        let err = result
            .err()
            .ok_or_else(|| format!("adversary {} was accepted", adversary.name))?;
        let actual = serde_json::to_value(err.code)?;
        if actual != adversary.expected {
            return Err(format!(
                "adversary {} expected {} but got {:?}",
                adversary.name, adversary.expected, err
            )
            .into());
        }
        entries.push(json!({"path":adversary.name,"sha256":hash(adversary.bytes.as_bytes()),"byte_count":adversary.bytes.len(),"expected_error":adversary.expected,"mutation":adversary.mutation,"baseline":if adversary.name.contains("conversion") || adversary.name == "recursive-units.step" { "box-inch.step" } else { "box-mm.step" }}));
        files.insert(adversary.name.into(), adversary.bytes.into_bytes());
    }
    let manifest = json!({"schema_version":1,"profile":"step-planar-cylindrical-v1","kernel_revision":"d87b4d9ced1f3baf31aa771ac0e7c663efb1c001","license":license(),"generation":{"timestamp":"2026-01-01T00:00:00","command":"cargo run --locked -p spiling-geometry --example generate_corpus --","drift_command":"cargo run --locked -p spiling-geometry --example generate_corpus -- --check","plate_status":"32x16 source geometry frozen; measured packed workload evidence is separate from engine transfer acceptance"},"fixtures":entries});
    for (name, bytes) in &files {
        write_or_check(&root.join(name), bytes, check)?;
        write_or_check(&root.join(format!("{name}.license")), ASSET_NOTICE, check)?;
    }
    write_or_check(&root.join("manifest.json"), &pretty(&manifest), check)?;
    for name in [
        "manifest.json",
        "recipe.json",
        "scenes/two-parts.scene.json",
        "scenes/repeated-128.scene.json",
        "scenes/large-origin.scene.json",
    ] {
        write_or_check(&root.join(format!("{name}.license")), ASSET_NOTICE, check)?;
    }
    println!(
        "{} frozen original corpus files {}",
        files.len(),
        if check {
            "match deterministic recipe"
        } else {
            "validated with computed source SHA-256 and canonical license sidecars"
        }
    );
    Ok(())
}
