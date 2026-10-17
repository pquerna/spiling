// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Deterministic original corpus recipe. Run without --check to stage bytes;
//! --check requires exact regenerated drift equality and never modifies files.
mod corpus;
mod parity;
use parity::{circle, extruded, original, rectangle};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    error::Error,
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

use spiling_contracts::geometry::{DisplayProfile, PlaneMm};
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn projected_bounds_report(bounds: &spiling_geometry::ProjectedBounds) -> Value {
    json!({"minimum":bounds.minimum,"maximum":bounds.maximum,
        "achieved_uncertainty_mm":bounds.achieved_uncertainty_mm})
}
fn directional_bounds_report(bounds: &[spiling_geometry::ProjectedBounds]) -> Value {
    Value::Array(bounds.iter().map(projected_bounds_report).collect())
}
fn check_section_oracle(
    section: &spiling_geometry::NativeSection,
    expectation: &Value,
    name: &str,
) -> Result<(), Box<dyn Error>> {
    let expected = if let Some(area) = expectation["section_area_mm2"].as_f64() {
        area
    } else {
        match expectation["section_area_expression"].as_str() {
            Some("16*pi") => 16. * std::f64::consts::PI,
            Some("25*pi") => 25. * std::f64::consts::PI,
            Some("40*pi") => 40. * std::f64::consts::PI,
            Some("48*pi") => 48. * std::f64::consts::PI,
            Some("12*pi") => 12. * std::f64::consts::PI,
            Some("221-4*pi") => 221. - 4. * std::f64::consts::PI,
            Some("400-9*pi") => 400. - 9. * std::f64::consts::PI,
            None => return Ok(()),
            Some(other) => {
                return Err(format!("unknown independent section oracle: {other}").into());
            }
        }
    };
    let mut area = 0.;
    let mut perimeter = 0.;
    for boundary in section.loops() {
        for pair in boundary.points_mm.windows(2) {
            area += 0.5 * (pair[0][0] * pair[1][1] - pair[1][0] * pair[0][1]);
            perimeter += (0..3)
                .map(|axis| (pair[1][axis] - pair[0][axis]).powi(2))
                .sum::<f64>()
                .sqrt();
        }
    }
    let tolerance = section.sampling_tolerance_mm();
    let error = 2. * perimeter * tolerance
        + std::f64::consts::TAU * tolerance * tolerance * section.loops().len() as f64
        + 1e-8;
    if (area - expected).abs() > error {
        return Err(format!("{name} native section area {area} disagrees with analytic oracle {expected} (bound {error})").into());
    }
    Ok(())
}
fn license() -> Value {
    json!({"SPDX-FileCopyrightText":"2026 Spiling contributors","SPDX-License-Identifier":"OSL-3.0","origin":"Original Spiling recipes plus unchanged independent-exporter originals; per-entry source_origin identifies the actual producer"})
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
    let mut notices = BTreeMap::<String, Vec<u8>>::new();
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
    let fitting_source = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/geometry/independent-exporters/occt-tapered-fitting-mm.step"),
    )?;
    let implicit_sources = [
        (
            "native-implicit-plane-box.step",
            corpus::implicit_intersections(&mm, "native-implicit-plane-box.step"),
            json!({"physical_bounds_mm":{"min":[0,0,0],"max":[20,10,8]},
                "face_count":6,"section_z_mm":4,"section_loop_count":1,"section_area_mm2":200}),
        ),
        (
            "native-implicit-plane-box-inch.step",
            corpus::implicit_intersections(&inch, "native-implicit-plane-box-inch.step"),
            json!({"physical_bounds_mm":{"min":[0,0,0],"max":[20,10,8]},
                "face_count":6,"source_unit":"inch","section_z_mm":4,"section_loop_count":1,"section_area_mm2":200}),
        ),
        (
            "native-implicit-plane-cylinder.step",
            corpus::implicit_intersections(&cylinder, "native-implicit-plane-cylinder.step"),
            json!({"physical_bounds_mm":{"min":[-5,-5,0],"max":[5,5,8]},
                "radius_mm":5,"section_z_mm":4,"section_loop_count":1,"section_area_expression":"25*pi"}),
        ),
        (
            "native-implicit-cone-cylinder-fitting.step",
            corpus::implicit_intersections(
                &fitting_source,
                "native-implicit-cone-cylinder-fitting.step",
            ),
            json!({"physical_bounds_mm":{"min":[-6,-6,0],"max":[6,6,12]},
                "baseline":"independent-exporters/occt-tapered-fitting-mm.step",
                "section_z_mm":6,"section_loop_count":2,"section_area_mm2":21.*std::f64::consts::PI}),
        ),
    ];
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
        entries.push(json!({"path":name,"sha256":hash(bytes.as_bytes()),"byte_count":bytes.len(),"expected":"accepted","source_origin":"same-kernel-original-roundtrip","recipe":"public analytic planar-wire extrusion","expectation":expectation}));
        files.insert(name.into(), bytes.into_bytes());
    }
    for (name, bytes, expectation) in implicit_sources {
        let definition = spiling_geometry::import_step(bytes.as_bytes(), &cancel)
            .map_err(|error| format!("original implicit {name} admission: {error:?}"))?;
        let mesh =
            spiling_geometry::tessellate(&definition, DisplayProfile::MeshTrimmedMm005V2, &cancel)?;
        let mesh_triangle_count = mesh.triangle_count;
        drop(mesh);
        let mut work = spiling_geometry::GeometryWorkBudget::new(50_000_000);
        let directional_bounds = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]]
            .map(|direction| {
                spiling_geometry::projected_bounds(
                    &definition,
                    direction,
                    spiling_geometry::BoundsPolicy {
                        maximum_endpoint_uncertainty_mm: 1e-6,
                    },
                    &mut work,
                    &cancel,
                )
            })
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        for (axis, bounds) in directional_bounds.iter().enumerate() {
            let minimum = expectation["physical_bounds_mm"]["min"][axis]
                .as_f64()
                .ok_or("implicit minimum oracle")?;
            let maximum = expectation["physical_bounds_mm"]["max"][axis]
                .as_f64()
                .ok_or("implicit maximum oracle")?;
            if !(bounds.minimum[0] <= minimum
                && minimum <= bounds.minimum[1]
                && bounds.maximum[0] <= maximum
                && maximum <= bounds.maximum[1])
            {
                return Err(format!("original implicit {name} directional oracle failed").into());
            }
        }
        let sample = expectation["section_z_mm"]
            .as_f64()
            .ok_or("implicit section height oracle")?;
        let source = spiling_geometry::prepare_material_source(
            &definition,
            spiling_geometry::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &cancel,
            &mut work,
        )?;
        let bounds_bytes = directional_bounds
            .capacity()
            .checked_mul(std::mem::size_of::<spiling_geometry::ProjectedBounds>())
            .ok_or("implicit retained bounds capacity overflow")?;
        let query_bytes = spiling_geometry::MAX_PREPARED_SECTION_BYTES
            .checked_sub(source.retained_bytes())
            .and_then(|bytes| bytes.checked_sub(bounds_bytes))
            .ok_or("implicit prepared source and bound results exceed native query pool")?;
        let slab = source.analyze_slab(
            spiling_geometry::SlabRequest {
                frame: spiling_contracts::geometry::PlaneFrameMm {
                    origin_mm: [0.; 3],
                    x_axis: [1., 0., 0.],
                    y_axis: [0., 1., 0.],
                    z_axis: [0., 0., 1.],
                },
                height_interval_mm: [sample - 0.01, sample + 0.01],
                sample_height_mm: sample,
                native_shape_error_mm: 0.1,
            },
            query_bytes,
            &mut work,
            &cancel,
        )?;
        let section = slab.section();
        if section.loops().len() as u64
            != expectation["section_loop_count"]
                .as_u64()
                .ok_or("implicit loop oracle")?
        {
            return Err(
                format!("original implicit {name} disagrees with analytic region count").into(),
            );
        }
        check_section_oracle(section, &expectation, name)?;
        entries.push(json!({"path":name,"sha256":hash(bytes.as_bytes()),"byte_count":bytes.len(),
            "expected":"accepted","source_origin":if expectation["baseline"].is_string() {
                "controlled-implicit-source-from-independent-cad-export"
            } else {"same-kernel-controlled-implicit-source"},
            "recipe":"original INTERSECTION_CURVE carrier arrangement; independent analytic solid oracle",
            "expectation":expectation,"observed":{"faces":definition.faces().len(),
                "mesh_triangles":mesh_triangle_count,"native_section_loops":section.loops().len(),
                "directional_bounds":directional_bounds_report(&directional_bounds)}}));
        if expectation["baseline"].is_string() {
            notices.insert(name.into(),std::fs::read(PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/geometry/independent-exporters/occt-tapered-fitting-mm.step.license"))?);
        }
        files.insert(name.into(), bytes.into_bytes());
    }
    for (name, solid, expectation) in parity::recipes() {
        for shell in solid.boundaries() {
            shell
                .check_solid_boundary()
                .map_err(|e| format!("original {name} native topology: {e:?}"))?;
        }
        let bytes = original(name, solid);
        let definition = spiling_geometry::import_step(bytes.as_bytes(), &cancel)
            .map_err(|e| format!("original {name} admission: {e:?}"))?;
        let mesh =
            spiling_geometry::tessellate(&definition, DisplayProfile::MeshTrimmedMm005V2, &cancel)
                .map_err(|e| format!("original {name} certified display: {e:?}"))?;
        let mut work = spiling_geometry::GeometryWorkBudget::new(1_000_000);
        let directional_bounds = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
            .map(|direction| {
                spiling_geometry::projected_bounds(
                    &definition,
                    direction,
                    spiling_geometry::BoundsPolicy {
                        maximum_endpoint_uncertainty_mm: 1e-6,
                    },
                    &mut work,
                    &cancel,
                )
            })
            .into_iter()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("original {name} certified directional bounds: {e:?}"))?;
        let height = expectation["section_z_mm"]
            .as_f64()
            .ok_or("missing native section recipe height")?;
        let native = spiling_geometry::section(
            &definition,
            PlaneMm {
                origin_mm: [0.0, 0.0, height],
                normal: [0.0, 0.0, 1.0],
            },
            &cancel,
        )
        .map_err(|e| format!("original {name} native section: {e:?}"))?;
        if native.loops().is_empty() {
            return Err(format!("original {name} interior native section lost material").into());
        }
        if let Some(count) = expectation["section_loop_count"].as_u64() {
            if native.loops().len() as u64 != count {
                return Err(
                    format!("original {name} native loop count disagrees with recipe").into(),
                );
            }
        }
        check_section_oracle(&native, &expectation, name)?;
        entries.push(json!({"path":name,"sha256":hash(bytes.as_bytes()),"byte_count":bytes.len(),
            "expected":"accepted","source_origin":"same-kernel-original-roundtrip",
            "recipe":"original closed native parity construction","expectation":expectation,
            "observed":{"faces":definition.faces().len(),"mesh_triangles":mesh.triangle_count,
                "native_section_loops":native.loops().len(),"directional_bounds":directional_bounds_report(&directional_bounds)}}));
        files.insert(name.into(), bytes.into_bytes());
    }
    // These are frozen genuine OCCT bytes, never re-exported by this producer.
    let independent_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/geometry/independent-exporters");
    let independent_manifest: Value =
        serde_json::from_slice(&std::fs::read(independent_root.join("manifest.json"))?)?;
    for (name, expectation) in parity::independent_recipes() {
        let basename = Path::new(name)
            .file_name()
            .ok_or("independent fixture basename missing")?;
        let bytes = std::fs::read(independent_root.join(basename))?;
        let record = independent_manifest["fixtures"]
            .as_array()
            .ok_or("independent fixture manifest missing")?
            .iter()
            .find(|record| record["path"].as_str() == basename.to_str())
            .ok_or("independent fixture original hash unavailable")?;
        if record["sha256"].as_str() != Some(hash(&bytes).as_str())
            || record["bytes"].as_u64() != Some(bytes.len() as u64)
        {
            return Err(format!("frozen independent source identity changed: {name}").into());
        }
        let definition = spiling_geometry::import_step(&bytes, &cancel)?;
        if expectation["face_count"]
            .as_u64()
            .is_some_and(|count| definition.faces().len() as u64 != count)
        {
            return Err(format!("independent source face oracle failed: {name}").into());
        }
        let mesh =
            spiling_geometry::tessellate(&definition, DisplayProfile::MeshTrimmedMm005V2, &cancel)?;
        let mut work = spiling_geometry::GeometryWorkBudget::new(1_000_000);
        let directional_bounds = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]]
            .map(|direction| {
                spiling_geometry::projected_bounds(
                    &definition,
                    direction,
                    spiling_geometry::BoundsPolicy {
                        maximum_endpoint_uncertainty_mm: 1e-6,
                    },
                    &mut work,
                    &cancel,
                )
            })
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        for (axis, bounds) in directional_bounds.iter().enumerate() {
            let minimum = expectation["physical_bounds_mm"]["min"][axis]
                .as_f64()
                .ok_or("missing independent minimum")?;
            let maximum = expectation["physical_bounds_mm"]["max"][axis]
                .as_f64()
                .ok_or("missing independent maximum")?;
            if !(bounds.minimum[0] <= minimum
                && minimum <= bounds.minimum[1]
                && bounds.maximum[0] <= maximum
                && maximum <= bounds.maximum[1])
            {
                return Err(format!("independent directional bound oracle failed: {name}").into());
            }
        }
        let height = expectation["section_z_mm"]
            .as_f64()
            .ok_or("missing independent section height")?;
        let section = spiling_geometry::section(
            &definition,
            PlaneMm {
                origin_mm: [0., 0., height],
                normal: [0., 0., 1.],
            },
            &cancel,
        )?;
        if section.loops().is_empty()
            || expectation["section_loop_count"]
                .as_u64()
                .is_some_and(|count| section.loops().len() as u64 != count)
        {
            return Err(
                format!("independent native section topology oracle failed: {name}").into(),
            );
        }
        check_section_oracle(&section, &expectation, name)?;
        entries.push(json!({"path":name,"sha256":hash(&bytes),"byte_count":bytes.len(),
            "expected":"accepted","source_origin":"independent-cad-exporter",
            "exporter":independent_manifest["exporter"],"expectation":expectation,
            "analytic_oracle":record.get("analytic_oracle"),
            "observed":{"faces":definition.faces().len(),"mesh_triangles":mesh.triangle_count,
                "native_section_loops":section.loops().len(),"directional_bounds":directional_bounds_report(&directional_bounds)}}));
        notices.insert(
            name.into(),
            std::fs::read(
                independent_root.join(format!("{}.license", basename.to_string_lossy())),
            )?,
        );
        files.insert(name.into(), bytes);
    }
    let wrong_master = corpus::wrong_selected_master(
        &std::fs::read_to_string(independent_root.join("occt-tapered-fitting-mm.step"))?,
        "native-source-wrong-selected-master.step",
    );
    let failure = spiling_geometry::import_step(wrong_master.as_bytes(), &cancel)
        .err()
        .ok_or("wrong selected master borrowed an alternate carrier receipt")?;
    if failure.code
        != spiling_contracts::geometry::GeometryErrorCode::ParameterCorrespondenceUnavailable
    {
        return Err(format!("wrong selected master returned wrong refusal: {failure:?}").into());
    }
    entries.push(json!({"path":"native-source-wrong-selected-master.step",
        "sha256":hash(wrong_master.as_bytes()),"byte_count":wrong_master.len(),
        "source_origin":"controlled-adversary-of-independent-cad-export",
        "baseline":"independent-exporters/occt-tapered-fitting-mm.step",
        "expected_error":"parameter_correspondence_unavailable",
        "mutation":"preferred PCURVE on bottom plane while alternate top carrier remains valid"}));
    notices.insert(
        "native-source-wrong-selected-master.step".into(),
        std::fs::read(independent_root.join("occt-tapered-fitting-mm.step.license"))?,
    );
    files.insert(
        "native-source-wrong-selected-master.step".into(),
        wrong_master.into_bytes(),
    );
    for adversary in corpus::adversaries(&mm, &inch) {
        let result = spiling_geometry::import_step(adversary.bytes.as_bytes(), &cancel);
        if adversary.expected == "accepted" {
            let definition = result
                .map_err(|e| format!("equivalent {} source import: {e:?}", adversary.name))?;
            let mesh = spiling_geometry::tessellate(
                &definition,
                DisplayProfile::MeshTrimmedMm005V2,
                &cancel,
            )?;
            let mut work = spiling_geometry::GeometryWorkBudget::new(1_000_000);
            let bounds = spiling_geometry::projected_bounds(
                &definition,
                [0.0, 0.0, 1.0],
                spiling_geometry::BoundsPolicy {
                    maximum_endpoint_uncertainty_mm: 1e-6,
                },
                &mut work,
                &cancel,
            )?;
            let native = spiling_geometry::section(
                &definition,
                PlaneMm {
                    origin_mm: [0.0, 0.0, 4.0],
                    normal: [0.0, 0.0, 1.0],
                },
                &cancel,
            )?;
            if native.loops().len() != 1 {
                return Err("equivalent bounded NURBS box lost native section material".into());
            }
            entries.push(json!({"path":adversary.name,"sha256":hash(adversary.bytes.as_bytes()),
                "byte_count":adversary.bytes.len(),"expected":"accepted","mutation":adversary.mutation,
                "baseline":"box-mm.step","observed":{"triangles":mesh.triangle_count,"z_bounds":projected_bounds_report(&bounds),
                    "native_section_loops":native.loops().len()}}));
        } else {
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
        }
        files.insert(adversary.name.into(), adversary.bytes.into_bytes());
    }
    let wrong_line = corpus::wrong_spatial_line(&mm, "native-source-wrong-spatial-line.step");
    let failure = spiling_geometry::import_step(wrong_line.as_bytes(), &cancel)
        .err()
        .ok_or("original spatial LINE was silently replaced by its BREP vertex chord")?;
    if failure.code != spiling_contracts::geometry::GeometryErrorCode::InvalidGeometry {
        return Err(
            format!("wrong original spatial LINE returned wrong refusal: {failure:?}").into(),
        );
    }
    entries.push(json!({"path":"native-source-wrong-spatial-line.step",
        "sha256":hash(wrong_line.as_bytes()),"byte_count":wrong_line.len(),
        "source_origin":"controlled-adversary-of-original-source",
        "baseline":"box-mm.step","expected_error":"invalid_geometry",
        "mutation":"original spatial LINE carrier displaced 123 mm; original BREP vertices unchanged"}));
    notices.insert(
        "native-source-wrong-spatial-line.step".into(),
        ASSET_NOTICE.to_vec(),
    );
    files.insert(
        "native-source-wrong-spatial-line.step".into(),
        wrong_line.into_bytes(),
    );
    let conditioning = parity::conditioning_source();
    let error = spiling_geometry::import_step(conditioning.as_bytes(), &cancel)
        .err()
        .ok_or("denominator-zero source was admitted")?;
    if error.code != spiling_contracts::geometry::GeometryErrorCode::ConditioningFailure {
        return Err(format!("denominator-zero source returned wrong refusal: {error:?}").into());
    }
    entries.push(
        json!({"path":"native-parity-denominator-zero.step","sha256":hash(conditioning.as_bytes()),
        "byte_count":conditioning.len(),"expected_error":"conditioning_failure",
        "recipe":"genuine rational surface with complete-span zero denominator"}),
    );
    files.insert(
        "native-parity-denominator-zero.step".into(),
        conditioning.into_bytes(),
    );
    let aliasing = parity::aliasing_source();
    let definition = spiling_geometry::import_step(aliasing.as_bytes(), &cancel)?;
    let mesh_outcome = match spiling_geometry::tessellate(
        &definition,
        DisplayProfile::MeshTrimmedMm005V2,
        &cancel,
    ) {
        Ok(mesh) => json!({"evidence":"certified","triangles":mesh.triangle_count}),
        Err(error)
            if matches!(
                error.code,
                spiling_contracts::geometry::GeometryErrorCode::ShapeErrorExceeded
                    | spiling_contracts::geometry::GeometryErrorCode::ConditioningFailure
                    | spiling_contracts::geometry::GeometryErrorCode::AnalysisIndeterminate
                    | spiling_contracts::geometry::GeometryErrorCode::ResourceLimit
            ) =>
        {
            json!({"evidence":"refused","diagnostic":error})
        }
        Err(error) => {
            return Err(format!("aliasing source unexpected display failure: {error:?}").into());
        }
    };
    entries.push(
        json!({"path":"native-parity-aliasing-extrusion.step","sha256":hash(aliasing.as_bytes()),
        "byte_count":aliasing.len(),"recipe":"degree-18 eighth-grid-zero polynomial extrusion",
        "display":mesh_outcome}),
    );
    files.insert(
        "native-parity-aliasing-extrusion.step".into(),
        aliasing.into_bytes(),
    );
    let kernel_revision = include_str!("../../../Cargo.toml")
        .lines()
        .find(|line| line.starts_with("monstertruck-modeling = "))
        .and_then(|line| line.split_once("rev = \""))
        .and_then(|(_, suffix)| suffix.split_once('"').map(|(revision, _)| revision))
        .ok_or("workspace native kernel revision declaration is unavailable")?;
    let manifest = json!({"schema_version":2,"profile":spiling_contracts::geometry::IMPORT_PROFILE,"kernel_revision":kernel_revision,"license":license(),"generation":{"timestamp":"2026-01-01T00:00:00","command":"cargo run --locked -p spiling-geometry --example generate_corpus --","drift_command":"cargo run --locked -p spiling-geometry --example generate_corpus -- --check","plate_status":"32x16 source geometry frozen; measured packed workload evidence is separate from engine transfer acceptance"},"fixtures":entries});
    for (name, bytes) in &files {
        write_or_check(&root.join(name), bytes, check)?;
        write_or_check(
            &root.join(format!("{name}.license")),
            notices.get(name).map_or(ASSET_NOTICE, Vec::as_slice),
            check,
        )?;
    }
    write_or_check(&root.join("manifest.json"), &pretty(&manifest), check)?;
    for name in [
        "manifest.json",
        "recipe.json",
        "scenes/two-parts.scene.json",
        "scenes/repeated-128.scene.json",
        "scenes/large-origin.scene.json",
        "scenes/realistic-native.scene.json",
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
