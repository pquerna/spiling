// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Independent re-reading of a Bambu Lab `.gcode.3mf` delivery package. The
//! reader opens the archive itself, requires exactly the expected members,
//! byte-compares the plate program with the verified program, recomputes its
//! MD5 and checks every metadata value the printer or Bambu Studio reads
//! against the plan, the captured intent and the fresh replay report. It
//! never calls the writer's helpers or formats. A consistent package is not
//! a firmware acceptance or printability claim.

use super::*;
use crate::delivery::md5::{Md5, upper_hex};
use quick_xml::{Reader, events::Event};
use spiling_chunks::ChunkSource;
use std::collections::BTreeMap;
use std::io::Read;

/// Members a Bambu sliced-plate package must contain, and nothing else.
const MEMBERS: [&str; 13] = [
    "[Content_Types].xml",
    "_rels/.rels",
    "3D/3dmodel.model",
    "Metadata/plate_1.gcode",
    "Metadata/plate_1.gcode.md5",
    "Metadata/plate_1.json",
    "Metadata/model_settings.config",
    "Metadata/_rels/model_settings.config.rels",
    "Metadata/slice_info.config",
    "Metadata/project_settings.config",
    "Metadata/plate_1.png",
    "Metadata/plate_1_small.png",
    "Metadata/plate_no_light_1.png",
];
const THUMBNAILS: [(&str, u32); 3] = [
    ("Metadata/plate_1.png", 512),
    ("Metadata/plate_1_small.png", 128),
    ("Metadata/plate_no_light_1.png", 512),
];

fn bad(message: impl AsRef<str>) -> ManufacturingError {
    failed(format!("delivery package: {}", message.as_ref()))
}

/// Every element in `xml` named `tag`, as attribute maps.
fn elements(xml: &str, tag: &str) -> Result<Vec<BTreeMap<String, String>>, ManufacturingError> {
    let mut reader = Reader::from_str(xml);
    let mut found = Vec::new();
    let mut depth = 0usize;
    loop {
        let event = reader.read_event().map_err(|e| bad(format!("XML: {e}")))?;
        let start = match &event {
            Event::Start(s) => {
                depth += 1;
                Some(s)
            }
            Event::Empty(s) => Some(s),
            Event::End(_) => {
                depth = depth.checked_sub(1).ok_or_else(|| bad("unbalanced XML"))?;
                None
            }
            Event::Eof => break,
            _ => None,
        };
        if let Some(start) = start
            && start.name().as_ref() == tag.as_bytes()
        {
            let mut attributes = BTreeMap::new();
            for a in start.attributes() {
                let a = a.map_err(|e| bad(format!("XML attribute: {e}")))?;
                let key = std::str::from_utf8(a.key.as_ref())
                    .map_err(|_| bad("XML name"))?
                    .to_owned();
                let value = a
                    .normalized_value(quick_xml::XmlVersion::Implicit1_0)
                    .map_err(|e| bad(format!("XML value: {e}")))?
                    .into_owned();
                if attributes.insert(key, value).is_some() {
                    return Err(bad("duplicate XML attribute"));
                }
            }
            found.push(attributes);
        }
    }
    if depth != 0 {
        return Err(bad("unterminated XML"));
    }
    Ok(found)
}
fn metadata(items: &[BTreeMap<String, String>], key: &str) -> Result<String, ManufacturingError> {
    let mut values = items
        .iter()
        .filter(|m| m.get("key").map(String::as_str) == Some(key))
        .filter_map(|m| m.get("value"));
    match (values.next(), values.next()) {
        (Some(v), None) => Ok(v.clone()),
        _ => Err(bad(format!("metadata {key} must appear exactly once"))),
    }
}
fn float(text: &str, what: &str) -> Result<f64, ManufacturingError> {
    text.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| bad(format!("{what} is not a finite number")))
}
fn close(a: f64, b: f64, tolerance: f64) -> bool {
    (a - b).abs() <= tolerance + b.abs() * 1e-9
}

/// Bambu generic filament identity and density per family, from Bambu
/// Studio 02.09.01.70's `Generic <family>` filament profiles (kept here
/// separately from the writer's table).
fn generic_filament(family: &str) -> Option<(&'static str, &'static str, f64)> {
    match family {
        "pla" => Some(("GFL99", "PLA", 1.24)),
        "petg" => Some(("GFG99", "PETG", 1.27)),
        "abs" => Some(("GFB99", "ABS", 1.04)),
        "asa" => Some(("GFB98", "ASA", 1.04)),
        "tpu" => Some(("GFU99", "TPU", 1.24)),
        _ => None,
    }
}

/// Re-read `package` against the verified `program` and its fresh `report`.
pub fn verify_bambu_package(
    package: &ChunkSource,
    program: &ChunkSource,
    bundle: &ManufacturingBundle,
    bundle_hash: &SourceHash,
    report: &VerificationReport,
    objects: usize,
    cancel: &AtomicBool,
) -> Result<DeliveryPackageInfo, ManufacturingError> {
    let intent = &bundle.provenance.intent;
    let plan = &bundle.plan;
    let JobWorkflow::CompleteMarlin { job } = &intent.printer.workflow else {
        return Err(bad("only a complete single-extruder job is packaged"));
    };
    let Some(DeliveryPackage::BambuGcode3mf {
        printer_model_id,
        printer_model,
        plate,
        ..
    }) = &job.delivery
    else {
        return Err(bad("the printer declares no Bambu package"));
    };
    if !report.verified {
        return Err(bad("the program it carries is not verified"));
    }
    let mut checks = Vec::new();
    let mut archive =
        zip::ZipArchive::new(package.reader()).map_err(|e| bad(format!("archive: {e}")))?;
    let names: Vec<String> = (0..archive.len())
        .map(|i| archive.name_for_index(i).map(str::to_owned))
        .collect::<Option<_>>()
        .ok_or_else(|| bad("archive index"))?;
    let mut sorted = names.clone();
    sorted.sort();
    let mut expected: Vec<String> = MEMBERS.iter().map(|m| (*m).to_owned()).collect();
    expected.sort();
    if sorted != expected {
        return Err(bad(format!(
            "members {names:?} differ from the Bambu plate set"
        )));
    }
    checks.push("member set is exactly the Bambu single-plate package".to_string());
    let mut text = BTreeMap::new();
    for name in MEMBERS {
        cancelled(cancel)?;
        let mut file = archive
            .by_name(name)
            .map_err(|e| bad(format!("{name}: {e}")))?;
        if file.is_dir() || file.encrypted() || file.enclosed_name().is_none() {
            return Err(bad(format!("{name} is not a plain enclosed file")));
        }
        if name == "Metadata/plate_1.gcode" {
            // Byte-compare with the verified program while hashing it.
            if file.size() != program.len() {
                return Err(bad(
                    "plate program length differs from the verified program",
                ));
            }
            let mut digest = Md5::default();
            let mut expected = program.reader();
            let (mut a, mut b) = (vec![0u8; 1 << 16], vec![0u8; 1 << 16]);
            let mut head = Vec::new();
            let mut tail = Vec::new();
            loop {
                let n = file.read(&mut a).map_err(|e| bad(format!("{name}: {e}")))?;
                if n == 0 {
                    break;
                }
                expected
                    .read_exact(&mut b[..n])
                    .map_err(|_| bad("plate program is longer than the verified program"))?;
                if a[..n] != b[..n] {
                    return Err(bad("plate program bytes differ from the verified program"));
                }
                digest.update(&a[..n]);
                if head.len() < 4096 {
                    head.extend_from_slice(&a[..n.min(4096 - head.len())]);
                }
                tail.extend_from_slice(&a[..n]);
                if tail.len() > 256 {
                    tail.drain(..tail.len() - 256);
                }
                cancelled(cancel)?;
            }
            text.insert("md5", upper_hex(&digest.finish()));
            text.insert("head", String::from_utf8_lossy(&head).into_owned());
            text.insert("tail", String::from_utf8_lossy(&tail).into_owned());
            continue;
        }
        // A member is bounded only by the package's representation width.
        if file.size() > MAX_DELIVERY_PACKAGE_BYTES {
            return Err(bad(format!("{name} exceeds the package representation")));
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|e| bad(format!("{name}: {e}")))?;
        if let Some((_, size)) = THUMBNAILS.iter().find(|(n, _)| *n == name) {
            let decoder = png::Decoder::new(std::io::Cursor::new(&bytes));
            let reader = decoder
                .read_info()
                .map_err(|e| bad(format!("{name}: PNG {e}")))?;
            let info = reader.info();
            if info.width != *size || info.height != *size {
                return Err(bad(format!("{name} is not {size}x{size}")));
            }
            continue;
        }
        let string = String::from_utf8(bytes).map_err(|_| bad(format!("{name} is not UTF-8")))?;
        text.insert(name, string);
    }
    checks.push("every member decompresses with a matching CRC-32".to_string());
    checks.push("plate program is byte-identical to the verified program".to_string());
    let md5 = text["md5"].clone();
    if text["Metadata/plate_1.gcode.md5"] != md5 {
        return Err(bad("plate_1.gcode.md5 differs from the program's MD5"));
    }
    checks.push("plate_1.gcode.md5 equals the recomputed program MD5".to_string());
    checks.push("thumbnails decode as PNG at 512/128/512 px".to_string());

    // OPC structure.
    let types = elements(&text["[Content_Types].xml"], "Default")?;
    for extension in ["rels", "model", "png", "gcode"] {
        if !types
            .iter()
            .any(|t| t.get("Extension").map(String::as_str) == Some(extension))
        {
            return Err(bad(format!("content types lack .{extension}")));
        }
    }
    let member_exists = |target: &str| MEMBERS.iter().any(|m| format!("/{m}") == target);
    let root = elements(&text["_rels/.rels"], "Relationship")?;
    let model_rel = "http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel";
    if !root.iter().any(|r| {
        r.get("Type").map(String::as_str) == Some(model_rel)
            && r.get("Target").map(String::as_str) == Some("/3D/3dmodel.model")
    }) || root
        .iter()
        .any(|r| !r.get("Target").is_some_and(|t| member_exists(t)))
    {
        return Err(bad("root relationships do not resolve to package members"));
    }
    let gcode_rels = elements(
        &text["Metadata/_rels/model_settings.config.rels"],
        "Relationship",
    )?;
    if gcode_rels.len() != 1
        || gcode_rels[0].get("Target").map(String::as_str) != Some("/Metadata/plate_1.gcode")
    {
        return Err(bad(
            "model settings relationship must name the plate program",
        ));
    }
    let model = &text["3D/3dmodel.model"];
    if elements(model, "model")?.len() != 1 {
        return Err(bad("3D model part must have one model root"));
    }
    // Each carried part lies on the bed inside the envelope; every triangle
    // indexes its own object's vertices; every object has one build item.
    let envelope = intent.printer.build_envelope;
    let mut reader = Reader::from_str(model);
    let (mut object_count, mut vertex_count, mut items) = (0usize, 0usize, 0usize);
    loop {
        match reader
            .read_event()
            .map_err(|e| bad(format!("model XML: {e}")))?
        {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) => {
                let get = |key: &[u8]| {
                    e.attributes()
                        .flatten()
                        .find(|a| a.key.as_ref() == key)
                        .and_then(|a| String::from_utf8(a.value.to_vec()).ok())
                };
                match e.name().as_ref() {
                    b"object" => {
                        object_count += 1;
                        vertex_count = 0;
                        if get(b"id") != Some(object_count.to_string()) {
                            return Err(bad("model objects must be numbered from 1"));
                        }
                    }
                    b"vertex" => {
                        vertex_count += 1;
                        for (axis, key) in [b"x", b"y", b"z"].iter().enumerate() {
                            let v = float(&get(*key).unwrap_or_default(), "vertex")?;
                            let slack = 0.5;
                            if v < envelope.min[axis] - slack || v > envelope.max[axis] + slack {
                                return Err(bad("model mesh leaves the build envelope"));
                            }
                        }
                    }
                    b"triangle" => {
                        for key in [b"v1", b"v2", b"v3"] {
                            let index: usize = get(key)
                                .and_then(|v| v.parse().ok())
                                .ok_or_else(|| bad("malformed triangle"))?;
                            if index >= vertex_count {
                                return Err(bad("triangle indexes outside its object"));
                            }
                        }
                    }
                    b"item" => {
                        items += 1;
                        if get(b"objectid") != Some(items.to_string())
                            || get(b"transform").is_some()
                        {
                            return Err(bad("each object needs one identity build item"));
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    if object_count != objects || items != objects {
        return Err(bad("model part does not carry the plate's parts"));
    }
    checks.push("content types and relationships resolve to members".to_string());
    checks.push(format!(
        "3D model carries {objects} posed part mesh(es) inside the build envelope"
    ));

    // Plate and slice metadata against plan, intent and report.
    let plate_meta = elements(&text["Metadata/model_settings.config"], "metadata")?;
    if metadata(&plate_meta, "plater_id")? != "1"
        || metadata(&plate_meta, "gcode_file")? != "Metadata/plate_1.gcode"
        || !member_exists(&format!("/{}", metadata(&plate_meta, "thumbnail_file")?))
    {
        return Err(bad(
            "model settings must bind plate 1 to its program and thumbnail",
        ));
    }
    let slice = &text["Metadata/slice_info.config"];
    let slice_meta = elements(slice, "metadata")?;
    let nozzle = intent.printer.nozzle_diameter_mm;
    let filament_mm = report.filament_length_mm
        + report.support_filament_length_mm
        + report.priming_filament_length_mm
        + report.adhesion_filament_length_mm;
    let prediction = report
        .estimated_seconds
        .map_or(0, |s| s.max(0.0).round() as u64);
    if metadata(&slice_meta, "index")? != "1"
        || &metadata(&slice_meta, "printer_model_id")? != printer_model_id
        || !close(
            float(&metadata(&slice_meta, "nozzle_diameters")?, "nozzle")?,
            nozzle,
            1e-9,
        )
        || metadata(&slice_meta, "prediction")? != prediction.to_string()
        || metadata(&slice_meta, "support_used")?
            != (report.support_deposition_segments > 0).to_string()
    {
        return Err(bad("slice info disagrees with the printer, report or plan"));
    }
    let family = intent.material_profile.as_ref().map(|p| {
        p.selection
            .profile
            .material
            .polymer_family
            .to_ascii_lowercase()
    });
    let known = family
        .as_deref()
        .and_then(generic_polymer_family)
        .and_then(generic_filament);
    let filaments = elements(slice, "filament")?;
    let [filament] = filaments.as_slice() else {
        return Err(bad("slice info must declare exactly one filament"));
    };
    let used_m = float(filament.get("used_m").map_or("", String::as_str), "used_m")?;
    let used_g = float(filament.get("used_g").map_or("", String::as_str), "used_g")?;
    let weight = float(&metadata(&slice_meta, "weight")?, "weight")?;
    let expected_colour = intent
        .material_profile
        .as_ref()
        .and_then(|p| p.selection.display_srgb)
        .map_or_else(
            || "#FFFFFF".to_string(),
            |[r, g, b]| format!("#{r:02X}{g:02X}{b:02X}"),
        );
    let area = std::f64::consts::PI * (intent.recipe.filament_diameter_mm * 0.5).powi(2);
    let expected_g = known.map_or(0.0, |(_, _, d)| filament_mm * area * d / 1000.0);
    if filament.get("id").map(String::as_str) != Some("1")
        || filament.get("color").map(String::as_str) != Some(expected_colour.as_str())
        || !close(used_m, filament_mm / 1000.0, 0.0051)
        || !close(used_g, expected_g, 0.0051)
        || !close(weight, used_g, 0.0)
        || known.is_some_and(|(id, kind, _)| {
            filament.get("tray_info_idx").map(String::as_str) != Some(id)
                || filament.get("type").map(String::as_str) != Some(kind)
        })
    {
        return Err(bad(
            "slice info filament usage disagrees with the decoded program",
        ));
    }
    let layers = plan.layers.len();
    let ranges = elements(slice, "layer_filament_list")?;
    if ranges.len() != 1
        || ranges[0].get("layer_ranges").map(String::as_str)
            != Some(format!("0 {}", layers.saturating_sub(1)).as_str())
    {
        return Err(bad("slice info layer range disagrees with the plan"));
    }
    checks.push(format!(
        "slice info: model {printer_model_id}, nozzle {nozzle} mm, {prediction} s, {:.2} m filament, {layers} layers",
        filament_mm / 1000.0
    ));
    let plate_json: serde_json::Value = serde_json::from_str(&text["Metadata/plate_1.json"])
        .map_err(|e| bad(format!("plate_1.json: {e}")))?;
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in plan
        .layers
        .iter()
        .flat_map(|l| &l.paths)
        .flat_map(|p| &p.points_mm)
    {
        lo = [lo[0].min(p[0]), lo[1].min(p[1])];
        hi = [hi[0].max(p[0]), hi[1].max(p[1])];
    }
    let bbox: Option<Vec<f64>> = plate_json["bbox_all"]
        .as_array()
        .and_then(|a| a.iter().map(serde_json::Value::as_f64).collect());
    let bed = intent.printer.build_envelope;
    if plate_json["bed_type"].as_str() != Some(plate.identifier())
        || plate_json["filament_colors"][0].as_str() != Some(expected_colour.as_str())
        || !plate_json["nozzle_diameter"]
            .as_f64()
            .is_some_and(|n| close(n, nozzle, 1e-9))
        || !bbox.as_deref().is_some_and(|b| {
            b.len() == 4
                && b.iter()
                    .zip([lo[0], lo[1], hi[0], hi[1]])
                    .all(|(a, e)| close(*a, e, 1e-6))
                && b[0] >= bed.min[0]
                && b[1] >= bed.min[1]
                && b[2] <= bed.max[0]
                && b[3] <= bed.max[1]
        })
    {
        return Err(bad(
            "plate_1.json disagrees with the plate, nozzle, filament colour or plan bounds",
        ));
    }
    checks.push(format!(
        "plate_1.json: {} plate, plan bounds inside the bed",
        plate.identifier()
    ));
    let project: serde_json::Value =
        serde_json::from_str(&text["Metadata/project_settings.config"])
            .map_err(|e| bad(format!("project_settings.config: {e}")))?;
    if project["printer_model"].as_str() != Some(printer_model.as_str())
        || project["filament_colour"][0].as_str() != Some(expected_colour.as_str())
        || !project["nozzle_diameter"][0]
            .as_str()
            .and_then(|n| n.parse::<f64>().ok())
            .is_some_and(|n| close(n, nozzle, 1e-9))
    {
        return Err(bad(
            "project settings name another printer, nozzle or filament colour",
        ));
    }
    checks.push(format!("project settings: {printer_model}"));

    // The program header block the printer reads.
    let head = &text["head"];
    let mut lines = head.lines();
    if lines.next() != Some("; HEADER_BLOCK_START") {
        return Err(bad("program does not open with its header block"));
    }
    let mut header = BTreeMap::new();
    for line in lines.by_ref() {
        if line == "; HEADER_BLOCK_END" {
            header.insert("end", String::new());
            break;
        }
        if let Some((key, value)) = line.trim_start_matches("; ").split_once(':') {
            header.insert(key.trim(), value.trim().to_string());
        }
    }
    let field = |key: &str| {
        header
            .get(key)
            .ok_or_else(|| bad(format!("header lacks {key}")))
            .and_then(|v| float(v, key))
    };
    // The highest deposited Z: path points and the prime line (a top layer
    // may carry no path).
    let max_z = plan
        .layers
        .iter()
        .flat_map(|l| &l.paths)
        .flat_map(|p| &p.points_mm)
        .map(|p| p[2])
        .fold(job.prime_start_mm[2].max(job.prime_end_mm[2]), f64::max);
    let (header_layers, header_filament, header_z) = (
        field("total layer number")?,
        field("total filament length [mm]")?,
        field("max_z_height")?,
    );
    if !header.contains_key("end")
        || header_layers != layers as f64
        || !close(header_filament, filament_mm, 0.0051)
        || !close(header_z, max_z, 0.0051)
    {
        return Err(bad(format!(
            "program header (layers {header_layers}, filament {header_filament} mm, max Z {header_z} mm) disagrees with the plan and decoded program ({layers}, {filament_mm:.4}, {max_z:.4})",
        )));
    }
    if !head.lines().any(|l| l == "; EXECUTABLE_BLOCK_START")
        || !text["tail"].ends_with("\n; EXECUTABLE_BLOCK_END\n")
    {
        return Err(bad("program lacks its executable block markers"));
    }
    checks.push(format!(
        "program header: {layers} layers, {filament_mm:.2} mm filament, max Z {max_z:.2} mm"
    ));
    let info = DeliveryPackageInfo {
        format: DeliveryPackageFormat::BambuGcode3mfV1,
        bundle_hash: bundle_hash.clone(),
        program_hash: bundle.program.hash.clone(),
        program_md5: md5,
        members: names,
        checks,
    };
    Ok(info)
}
