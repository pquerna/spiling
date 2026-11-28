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
    match family.to_ascii_lowercase().as_str() {
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
        ams,
        ..
    }) = &job.delivery
    else {
        return Err(bad("the printer declares no Bambu package"));
    };
    if !report.verified {
        return Err(bad("the program it carries is not verified"));
    }
    let ams_report = match (ams.as_ref(), report.ams.as_ref()) {
        (Some(cfg), Some(rep)) => {
            if rep.slots.len() != cfg.slots.len() {
                return Err(bad(
                    "AMS report slot count disagrees with declared AMS slots",
                ));
            }
            Some((cfg, rep))
        }
        (None, None) => None,
        _ => return Err(bad("AMS verification presence disagrees with job delivery")),
    };
    let mut checks = Vec::new();
    let mut archive =
        zip::ZipArchive::new(package.reader()).map_err(|e| bad(format!("archive: {e}")))?;
    let names: Vec<String> = (0..archive.len())
        .map(|i| archive.name_for_index(i).map(str::to_owned))
        .collect::<Option<_>>()
        .ok_or_else(|| bad("archive index"))?;
    let mut sorted = names.clone();
    sorted.sort();
    let mut expected_members: Vec<&str> = MEMBERS.to_vec();
    if ams_report.is_some() {
        expected_members.push("Metadata/filament_sequence.json");
    }
    let mut expected: Vec<String> = expected_members.iter().map(|m| (*m).to_owned()).collect();
    expected.sort();
    if sorted != expected {
        return Err(bad(format!(
            "members {names:?} differ from the Bambu plate set"
        )));
    }
    checks.push(if ams_report.is_some() {
        "member set is exactly the Bambu single-plate AMS package".to_string()
    } else {
        "member set is exactly the Bambu single-plate package".to_string()
    });
    let mut text = BTreeMap::new();
    for name in expected_members.iter().copied() {
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
    let member_exists = |target: &str| expected_members.iter().any(|m| format!("/{m}") == target);
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

    struct ExpectedFilament {
        slot: u8,
        diameter_mm: f64,
        known: Option<(&'static str, &'static str, f64)>,
        colour: String,
        total_mm: f64,
        expected_g: f64,
        used_for_object: bool,
        used_for_support: bool,
    }
    let expected_filaments: Vec<ExpectedFilament> = if let Some((cfg, rep)) = ams_report {
        let mut out = Vec::new();
        for (slot_idx, slot) in cfg.slots.iter().enumerate() {
            let acct = &rep.slots[slot_idx];
            let total_mm = acct.total_filament_length_mm();
            if slot_idx != 0 && total_mm <= 0.0 {
                continue;
            }
            let lower = slot.polymer_family.to_ascii_lowercase();
            let known = generic_polymer_family(&lower).and_then(generic_filament);
            let colour = slot
                .display_srgb
                .or_else(|| {
                    (slot_idx == 0)
                        .then(|| {
                            intent
                                .material_profile
                                .as_ref()
                                .and_then(|p| p.selection.display_srgb)
                        })
                        .flatten()
                })
                .map_or_else(
                    || "#FFFFFF".to_string(),
                    |[r, g, b]| format!("#{r:02X}{g:02X}{b:02X}"),
                );
            let area = std::f64::consts::PI * (slot.filament_diameter_mm * 0.5).powi(2);
            let expected_g = known.map_or(0.0, |(_, _, d)| total_mm * area * d / 1000.0);
            out.push(ExpectedFilament {
                slot: slot_idx as u8,
                diameter_mm: slot.filament_diameter_mm,
                known,
                colour,
                total_mm,
                expected_g,
                used_for_object: acct.used_for_object,
                used_for_support: acct.used_for_support,
            });
        }
        out
    } else {
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
        let colour = intent
            .material_profile
            .as_ref()
            .and_then(|p| p.selection.display_srgb)
            .map_or_else(
                || "#FFFFFF".to_string(),
                |[r, g, b]| format!("#{r:02X}{g:02X}{b:02X}"),
            );
        let total_mm = report.filament_length_mm
            + report.support_filament_length_mm
            + report.priming_filament_length_mm
            + report.adhesion_filament_length_mm;
        let area = std::f64::consts::PI * (intent.recipe.filament_diameter_mm * 0.5).powi(2);
        let expected_g = known.map_or(0.0, |(_, _, d)| total_mm * area * d / 1000.0);
        vec![ExpectedFilament {
            slot: 0,
            diameter_mm: intent.recipe.filament_diameter_mm,
            known,
            colour,
            total_mm,
            expected_g,
            used_for_object: true,
            used_for_support: report.support_deposition_segments > 0,
        }]
    };
    let expected_maps = vec!["1"; expected_filaments.len()].join(" ");
    if metadata(&slice_meta, "filament_maps")? != expected_maps {
        return Err(bad(
            "slice info filament_maps disagrees with active filaments",
        ));
    }
    let filaments = elements(slice, "filament")?;
    if filaments.len() != expected_filaments.len() {
        return Err(bad(
            "slice info filament count disagrees with active decoded filaments",
        ));
    }
    let mut summed_used_g = 0.0;
    let mut total_filament_mm = 0.0;
    for (filament, exp) in filaments.iter().zip(&expected_filaments) {
        let used_m = float(filament.get("used_m").map_or("", String::as_str), "used_m")?;
        let used_g = float(filament.get("used_g").map_or("", String::as_str), "used_g")?;
        summed_used_g += used_g;
        total_filament_mm += exp.total_mm;
        let expected_id = (usize::from(exp.slot) + 1).to_string();
        if filament.get("id").map(String::as_str) != Some(expected_id.as_str())
            || filament.get("color").map(String::as_str) != Some(exp.colour.as_str())
            || filament.get("group_id").map(String::as_str) != Some("0")
            || filament.get("directly_used").map(String::as_str) != Some("true")
            || filament.get("used_for_object").map(String::as_str)
                != Some(exp.used_for_object.to_string().as_str())
            || filament.get("used_for_support").map(String::as_str)
                != Some(exp.used_for_support.to_string().as_str())
            || !close(used_m, exp.total_mm / 1000.0, 0.0051)
            || !close(used_g, exp.expected_g, 0.0051)
            || exp.known.is_some_and(|(id, kind, _)| {
                filament.get("tray_info_idx").map(String::as_str) != Some(id)
                    || filament.get("type").map(String::as_str) != Some(kind)
            })
        {
            return Err(bad(
                "slice info filament usage disagrees with the decoded program",
            ));
        }
    }
    let weight = float(&metadata(&slice_meta, "weight")?, "weight")?;
    if !close(weight, summed_used_g, 1e-6) {
        return Err(bad(
            "slice info weight disagrees with sum of declared filament weights",
        ));
    }
    let layers = plan.layers.len();
    let ranges = elements(slice, "layer_filament_list")?;
    let expected_lists: Vec<(String, String)> = if let Some((cfg, rep)) = ams_report {
        let mut grouped: BTreeMap<Vec<u8>, Vec<(usize, usize)>> = BTreeMap::new();
        let mut active = 0u8;
        for (idx, layer) in plan.layers.iter().enumerate() {
            let mut used = Vec::new();
            let has_switch = {
                let mut cur = active;
                layer.paths.iter().any(|p| {
                    let s = cfg.slot_for_kind(p.kind);
                    let changed = s != cur;
                    cur = s;
                    changed
                })
            };
            if !has_switch && rep.tower_layers > 0 && (idx as u32) < rep.tower_layers {
                used.push(active);
            }
            for path in &layer.paths {
                let slot = cfg.slot_for_kind(path.kind);
                if !used.contains(&slot) {
                    used.push(slot);
                }
                active = slot;
            }
            if used.is_empty() {
                used.push(active);
            }
            used.sort_unstable();
            let entry = grouped.entry(used).or_default();
            if let Some(last) = entry.last_mut()
                && last.1 + 1 == idx
            {
                last.1 = idx;
            } else {
                entry.push((idx, idx));
            }
        }
        grouped
            .into_iter()
            .map(|(slots, rs)| {
                (
                    slots
                        .iter()
                        .map(ToString::to_string)
                        .collect::<Vec<_>>()
                        .join(" "),
                    rs.iter()
                        .map(|(a, b)| format!("{a} {b}"))
                        .collect::<Vec<_>>()
                        .join(","),
                )
            })
            .collect()
    } else {
        vec![("0".to_string(), format!("0 {}", layers.saturating_sub(1)))]
    };
    if ranges.len() != expected_lists.len()
        || ranges.iter().zip(&expected_lists).any(|(el, (list, r))| {
            el.get("filament_list").map(String::as_str) != Some(list.as_str())
                || el.get("layer_ranges").map(String::as_str) != Some(r.as_str())
        })
    {
        return Err(bad(
            "slice info layer filament ranges disagree with the plan",
        ));
    }
    checks.push(format!(
        "slice info: model {printer_model_id}, nozzle {nozzle} mm, {prediction} s, {:.2} m across {} filament(s), {layers} layers",
        total_filament_mm / 1000.0,
        expected_filaments.len()
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
    let tower_bbox = ams_report.and_then(|(cfg, rep)| {
        cfg.prime_tower
            .as_ref()
            .filter(|_| rep.tower_layers > 0)
            .map(|t| {
                let min = t.min_xy_mm();
                let max = t.max_xy_mm();
                [min[0], min[1], max[0], max[1]]
            })
    });
    let expected_bbox = match tower_bbox {
        Some(tb) => [
            lo[0].min(tb[0]),
            lo[1].min(tb[1]),
            hi[0].max(tb[2]),
            hi[1].max(tb[3]),
        ],
        None => [lo[0], lo[1], hi[0], hi[1]],
    };
    let bbox: Option<Vec<f64>> = plate_json["bbox_all"]
        .as_array()
        .and_then(|a| a.iter().map(serde_json::Value::as_f64).collect());
    let bed = intent.printer.build_envelope;
    let plate_colours: Option<Vec<&str>> = plate_json["filament_colors"]
        .as_array()
        .and_then(|a| a.iter().map(serde_json::Value::as_str).collect());
    let plate_ids: Option<Vec<u64>> = plate_json["filament_ids"]
        .as_array()
        .and_then(|a| a.iter().map(serde_json::Value::as_u64).collect());
    let expected_first = ams_report
        .and_then(|(_, rep)| rep.sequence.first().copied())
        .unwrap_or(0);
    if plate_json["bed_type"].as_str() != Some(plate.identifier())
        || plate_json["first_extruder"].as_u64() != Some(u64::from(expected_first))
        || plate_colours.as_deref()
            != Some(
                expected_filaments
                    .iter()
                    .map(|f| f.colour.as_str())
                    .collect::<Vec<_>>()
                    .as_slice(),
            )
        || plate_ids.as_deref()
            != Some(
                expected_filaments
                    .iter()
                    .map(|f| u64::from(f.slot))
                    .collect::<Vec<_>>()
                    .as_slice(),
            )
        || !plate_json["nozzle_diameter"]
            .as_f64()
            .is_some_and(|n| close(n, nozzle, 1e-9))
        || !bbox.as_deref().is_some_and(|b| {
            b.len() == 4
                && b.iter().zip(expected_bbox).all(|(a, e)| close(*a, e, 1e-6))
                && b[0] >= bed.min[0]
                && b[1] >= bed.min[1]
                && b[2] <= bed.max[0]
                && b[3] <= bed.max[1]
        })
    {
        return Err(bad(
            "plate_1.json disagrees with the plate, nozzle, filament colours/IDs or plan bounds",
        ));
    }
    if let Some(tb) = tower_bbox {
        let has_tower_obj = plate_json["bbox_objects"].as_array().is_some_and(|objs| {
            objs.iter().any(|o| {
                o["name"].as_str() == Some("wipe_tower")
                    && o["id"].as_u64() == Some(1000)
                    && o["bbox"]
                        .as_array()
                        .and_then(|a| {
                            a.iter()
                                .map(serde_json::Value::as_f64)
                                .collect::<Option<Vec<_>>>()
                        })
                        .is_some_and(|b| {
                            b.len() == 4 && b.iter().zip(tb).all(|(a, e)| close(*a, e, 1e-6))
                        })
            })
        });
        if !has_tower_obj {
            return Err(bad(
                "plate_1.json lacks a matching wipe_tower bounding box entry",
            ));
        }
    }
    checks.push(format!(
        "plate_1.json: {} plate, plan bounds inside the bed",
        plate.identifier()
    ));

    let project: serde_json::Value =
        serde_json::from_str(&text["Metadata/project_settings.config"])
            .map_err(|e| bad(format!("project_settings.config: {e}")))?;
    let proj_colours: Option<Vec<&str>> = project["filament_colour"]
        .as_array()
        .and_then(|a| a.iter().map(serde_json::Value::as_str).collect());
    let proj_diams: Option<Vec<f64>> = project["filament_diameter"].as_array().and_then(|a| {
        a.iter()
            .map(|v| v.as_str().and_then(|s| s.parse::<f64>().ok()))
            .collect()
    });
    if project["printer_model"].as_str() != Some(printer_model.as_str())
        || proj_colours.as_deref()
            != Some(
                expected_filaments
                    .iter()
                    .map(|f| f.colour.as_str())
                    .collect::<Vec<_>>()
                    .as_slice(),
            )
        || !proj_diams.as_deref().is_some_and(|ds| {
            ds.len() == expected_filaments.len()
                && ds
                    .iter()
                    .zip(&expected_filaments)
                    .all(|(d, exp)| close(*d, exp.diameter_mm, 1e-6))
        })
        || !project["nozzle_diameter"][0]
            .as_str()
            .and_then(|n| n.parse::<f64>().ok())
            .is_some_and(|n| close(n, nozzle, 1e-9))
    {
        return Err(bad(
            "project settings name another printer, nozzle or filament set",
        ));
    }
    if ams_report.is_some()
        && (project["single_extruder_multi_material"].as_str() != Some("1")
            || project["enable_prime_tower"].as_str()
                != Some(if tower_bbox.is_some() { "1" } else { "0" }))
    {
        return Err(bad(
            "project settings AMS multi-material or prime-tower flags disagree with the plan",
        ));
    }
    checks.push(format!("project settings: {printer_model}"));

    if let Some((_, rep)) = ams_report {
        let seq_json: serde_json::Value =
            serde_json::from_str(&text["Metadata/filament_sequence.json"])
                .map_err(|e| bad(format!("filament_sequence.json: {e}")))?;
        let seq: Option<Vec<u64>> = seq_json["plate_1"]["sequence"]
            .as_array()
            .and_then(|a| a.iter().map(serde_json::Value::as_u64).collect());
        let expected_seq: Vec<u64> = rep.sequence.iter().map(|&s| u64::from(s) + 1).collect();
        let nozzles: Option<Vec<u64>> = seq_json["plate_1"]["nozzle_sequence"]
            .as_array()
            .and_then(|a| a.iter().map(serde_json::Value::as_u64).collect());
        let optimal: Option<Vec<u64>> = seq_json["plate_1"]["optimal_assignment"]
            .as_array()
            .and_then(|a| a.iter().map(serde_json::Value::as_u64).collect());
        if seq.as_deref() != Some(expected_seq.as_slice())
            || nozzles.as_deref() != Some(vec![0u64; expected_seq.len()].as_slice())
            || optimal.as_deref() != Some(vec![0u64; expected_filaments.len()].as_slice())
        {
            return Err(bad(
                "filament_sequence.json disagrees with the decoded AMS load/switch sequence",
            ));
        }
        checks.push(format!(
            "filament_sequence.json: {} sequence entries match decoded AMS switches",
            expected_seq.len()
        ));
    }

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
    let csv_floats = |key: &str| -> Result<Vec<f64>, ManufacturingError> {
        let raw = header
            .get(key)
            .ok_or_else(|| bad(format!("header lacks {key}")))?;
        raw.split(',').map(|part| float(part.trim(), key)).collect()
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
    let header_layers = field("total layer number")?;
    let header_filaments = csv_floats("total filament length [mm]")?;
    let header_diameters = csv_floats("filament_diameter")?;
    let header_ids = header
        .get("filament")
        .ok_or_else(|| bad("header lacks filament"))?
        .split(',')
        .map(|p| {
            p.trim()
                .parse::<u64>()
                .map_err(|_| bad("malformed header filament ID"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let header_z = field("max_z_height")?;
    let expected_header_ids: Vec<u64> = expected_filaments
        .iter()
        .map(|f| u64::from(f.slot) + 1)
        .collect();
    if !header.contains_key("end")
        || header_layers != layers as f64
        || header_filaments.len() != expected_filaments.len()
        || header_filaments
            .iter()
            .zip(&expected_filaments)
            .any(|(a, e)| !close(*a, e.total_mm, 0.0051))
        || header_diameters.len() != expected_filaments.len()
        || header_diameters
            .iter()
            .zip(&expected_filaments)
            .any(|(a, e)| !close(*a, e.diameter_mm, 0.0051))
        || header_ids != expected_header_ids
        || !close(header_z, max_z, 0.0051)
    {
        return Err(bad(format!(
            "program header (layers {header_layers}, filaments {header_filaments:?} mm, IDs {header_ids:?}, max Z {header_z} mm) disagrees with the plan and decoded program ({layers}, {total_filament_mm:.4}, {max_z:.4})",
        )));
    }
    if !head.lines().any(|l| l == "; EXECUTABLE_BLOCK_START")
        || !text["tail"].ends_with("\n; EXECUTABLE_BLOCK_END\n")
    {
        return Err(bad("program lacks its executable block markers"));
    }
    checks.push(format!(
        "program header: {layers} layers, {total_filament_mm:.2} mm across {} filament(s), max Z {max_z:.2} mm",
        expected_filaments.len()
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
