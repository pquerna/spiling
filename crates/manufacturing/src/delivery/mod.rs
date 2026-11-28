// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Firmware delivery packages around a verified program. The Bambu Lab
//! `.gcode.3mf` writer follows the package structure observed in Bambu
//! Studio 02.09.01.70 A1/A1 mini exports (member names, OPC relationships,
//! metadata keys the printer and Bambu Studio read); the contents are this
//! producer's own. Packages are deterministic: fixed member order, fixed
//! timestamps and compression. The verifier re-reads every package
//! independently (`verifier::package`) before it is published.

pub(crate) mod md5;
mod thumbnail;

use crate::{check_cancel, error};
use spiling_chunks::{ChunkSource, ChunkWriter};
use spiling_contracts::manufacturing::*;
use std::io::{Read, Write};
use std::sync::atomic::AtomicBool;

/// One plate part as a posed triangle mesh in printer coordinates, carried in
/// the package's 3MF model for slicer viewers.
pub struct PackageObject {
    pub name: String,
    pub vertices: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
}

/// The package and what it carries.
pub struct EncodedDeliveryPackage {
    pub info: DeliveryPackageInfo,
    pub bytes: ChunkSource,
}

/// Bambu's generic filament identity and density for a polymer family, as
/// Bambu Studio 02.09.01.70's `Generic <family>` filament profiles declare
/// them (`filament_id`, `filament_density`, `filament_type`).
pub(crate) fn bambu_filament(family: &str) -> Option<(&'static str, &'static str, f64)> {
    Some(match family {
        "pla" => ("GFL99", "PLA", 1.24),
        "petg" => ("GFG99", "PETG", 1.27),
        "abs" => ("GFB99", "ABS", 1.04),
        "asa" => ("GFB98", "ASA", 1.04),
        "tpu" => ("GFU99", "TPU", 1.24),
        _ => return None,
    })
}
/// Fallback filament colour shown by the printer when the material declares none.
pub(crate) const BAMBU_UNDECLARED_COLOUR: &str = "#FFFFFF";

fn format_srgb_hex(rgb: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2])
}

/// The thumbnails a package carries, by member name and pixel size.
pub(crate) const BAMBU_THUMBNAILS: [(&str, u32); 3] = [
    ("Metadata/plate_1.png", 512),
    ("Metadata/plate_1_small.png", 128),
    ("Metadata/plate_no_light_1.png", 512),
];

/// Package facts derived from the bundle and its fresh verification report.
pub(crate) struct PlateFacts {
    pub printer_model_id: String,
    pub printer_model: String,
    pub bed_type: &'static str,
    pub nozzle_mm: f64,
    pub filament_diameter_mm: f64,
    pub filament_type: String,
    pub filament_colour: String,
    pub filament_id: Option<&'static str>,
    pub density: Option<f64>,
    pub filament_mm: f64,
    pub prediction_s: u64,
    pub layers: usize,
    pub layer_height_mm: f64,
    pub bbox: [f64; 4],
    pub support_used: bool,
    pub nozzle_c: f64,
    pub bed_c: f64,
    pub first_layer_nozzle_c: f64,
    pub first_layer_bed_c: f64,
}

impl PlateFacts {
    pub(crate) fn new(
        bundle: &ManufacturingBundle,
        report: &VerificationReport,
    ) -> Result<Self, ManufacturingError> {
        let intent = &bundle.provenance.intent;
        let JobWorkflow::CompleteMarlin { job } = &intent.printer.workflow else {
            return Err(unsupported());
        };
        let Some(DeliveryPackage::BambuGcode3mf {
            printer_model_id,
            printer_model,
            plate,
            ..
        }) = &job.delivery
        else {
            return Err(unsupported());
        };
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
            .and_then(bambu_filament);
        let filament_type = match (&known, &family) {
            (Some((_, kind, _)), _) => (*kind).to_string(),
            (None, Some(f)) => f.to_ascii_uppercase(),
            (None, None) => "UNSPECIFIED".to_string(),
        };
        let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
        for p in bundle
            .plan
            .layers
            .iter()
            .flat_map(|l| &l.paths)
            .flat_map(|p| &p.points_mm)
        {
            lo = [lo[0].min(p[0]), lo[1].min(p[1])];
            hi = [hi[0].max(p[0]), hi[1].max(p[1])];
        }
        if lo[0] > hi[0] {
            return Err(error(
                ManufacturingErrorCode::InvalidSpecification,
                "a delivery package needs deposited paths",
            ));
        }
        let filament_colour = intent
            .material_profile
            .as_ref()
            .and_then(|p| p.selection.display_srgb)
            .map_or_else(|| BAMBU_UNDECLARED_COLOUR.to_string(), format_srgb_hex);
        Ok(Self {
            printer_model_id: printer_model_id.clone(),
            printer_model: printer_model.clone(),
            bed_type: plate.identifier(),
            nozzle_mm: intent.printer.nozzle_diameter_mm,
            filament_diameter_mm: intent.recipe.filament_diameter_mm,
            filament_type,
            filament_colour,
            filament_id: known.map(|(id, _, _)| id),
            density: known.map(|(_, _, d)| d),
            filament_mm: report.filament_length_mm
                + report.support_filament_length_mm
                + report.priming_filament_length_mm
                + report.adhesion_filament_length_mm,
            prediction_s: report
                .estimated_seconds
                .map_or(0, |s| s.max(0.0).round() as u64),
            layers: bundle.plan.layers.len(),
            layer_height_mm: intent.recipe.layer_height_mm,
            bbox: [lo[0], lo[1], hi[0], hi[1]],
            support_used: report.support_deposition_segments > 0,
            nozzle_c: job.nozzle_temperature_c,
            bed_c: job.bed_temperature_c,
            first_layer_nozzle_c: job.first_layer_nozzle_temperature_c,
            first_layer_bed_c: job.first_layer_bed_temperature_c,
        })
    }
    pub(crate) fn weight_g(&self) -> f64 {
        let area = std::f64::consts::PI * (self.filament_diameter_mm * 0.5).powi(2);
        self.density
            .map_or(0.0, |d| self.filament_mm * area * d / 1000.0)
    }
}

fn unsupported() -> ManufacturingError {
    error(
        ManufacturingErrorCode::UnsupportedCapability,
        "this printer takes a plain program, not a delivery package",
    )
}

/// Whether the printer's firmware takes a delivery package.
pub fn package_format_for_workflow(workflow: &JobWorkflow) -> Option<DeliveryPackageFormat> {
    match workflow {
        JobWorkflow::CompleteMarlin { job } => job
            .delivery
            .as_ref()
            .map(|DeliveryPackage::BambuGcode3mf { .. }| DeliveryPackageFormat::BambuGcode3mfV1),
        _ => None,
    }
}

pub fn package_format(intent: &ManufacturingIntent) -> Option<DeliveryPackageFormat> {
    package_format_for_workflow(&intent.printer.workflow)
}

fn r2(v: f64) -> String {
    format!("{v:.2}")
}
fn number(v: f64) -> String {
    let text = format!("{v:.6}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

pub(crate) const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
 <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
 <Default Extension="model" ContentType="application/vnd.ms-package.3dmanufacturing-3dmodel+xml"/>
 <Default Extension="png" ContentType="image/png"/>
 <Default Extension="gcode" ContentType="text/x.gcode"/>
 <Default Extension="config" ContentType="text/xml"/>
 <Default Extension="json" ContentType="application/json"/>
 <Default Extension="md5" ContentType="text/plain"/>
</Types>
"#;

pub(crate) const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
 <Relationship Target="/3D/3dmodel.model" Id="rel-1" Type="http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel"/>
 <Relationship Target="/Metadata/plate_1.png" Id="rel-2" Type="http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail"/>
 <Relationship Target="/Metadata/plate_1.png" Id="rel-4" Type="http://schemas.bambulab.com/package/2021/cover-thumbnail-middle"/>
 <Relationship Target="/Metadata/plate_1_small.png" Id="rel-5" Type="http://schemas.bambulab.com/package/2021/cover-thumbnail-small"/>
</Relationships>
"#;

pub(crate) const MODEL_SETTINGS_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
 <Relationship Target="/Metadata/plate_1.gcode" Id="rel-1" Type="http://schemas.bambulab.com/package/2021/gcode"/>
</Relationships>
"#;

/// The 3MF core model: each plate part's posed display mesh as one object
/// with an identity build item. The plan, not these meshes, is the program's
/// authority; they let slicer viewers show what the plate holds.
fn model(objects: &[PackageObject]) -> String {
    let mut out = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xml:lang="en-US" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" xmlns:BambuStudio="http://schemas.bambulab.com/package/2021">
 <metadata name="Application">Spiling-{}</metadata>
 <metadata name="BambuStudio:3mfVersion">1</metadata>
 <metadata name="Title">Spiling plate</metadata>
 <resources>
"#,
        crate::EMITTER_VERSION
    );
    use std::fmt::Write as _;
    for (i, object) in objects.iter().enumerate() {
        let name: String = object
            .name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || " ._-".contains(c) {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let _ = writeln!(
            out,
            r#"  <object id="{}" name="{name}" type="model">"#,
            i + 1
        );
        out.push_str("   <mesh>\n    <vertices>\n");
        for v in &object.vertices {
            let _ = writeln!(
                out,
                r#"     <vertex x="{}" y="{}" z="{}"/>"#,
                number(f64::from(v[0])),
                number(f64::from(v[1])),
                number(f64::from(v[2]))
            );
        }
        out.push_str("    </vertices>\n    <triangles>\n");
        for t in &object.triangles {
            let _ = writeln!(
                out,
                r#"     <triangle v1="{}" v2="{}" v3="{}"/>"#,
                t[0], t[1], t[2]
            );
        }
        out.push_str("    </triangles>\n   </mesh>\n  </object>\n");
    }
    out.push_str(" </resources>\n <build>\n");
    for i in 0..objects.len() {
        let _ = writeln!(out, r#"  <item objectid="{}"/>"#, i + 1);
    }
    out.push_str(" </build>\n</model>\n");
    out
}

fn model_settings() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<config>
  <plate>
    <metadata key="plater_id" value="1"/>
    <metadata key="plater_name" value=""/>
    <metadata key="locked" value="false"/>
    <metadata key="gcode_file" value="Metadata/plate_1.gcode"/>
    <metadata key="thumbnail_file" value="Metadata/plate_1.png"/>
    <metadata key="thumbnail_no_light_file" value="Metadata/plate_no_light_1.png"/>
  </plate>
</config>
"#
}

fn slice_info(f: &PlateFacts) -> String {
    let tray = f.filament_id.unwrap_or("");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<config>
  <header>
    <header_item key="X-BBL-Client-Type" value="slicer"/>
    <header_item key="X-BBL-Client-Version" value="Spiling-{emitter}"/>
  </header>
  <plate>
    <metadata key="index" value="1"/>
    <metadata key="extruder_type" value="0"/>
    <metadata key="nozzle_volume_type" value="0"/>
    <metadata key="printer_model_id" value="{model}"/>
    <metadata key="nozzle_diameters" value="{nozzle}"/>
    <metadata key="timelapse_type" value="0"/>
    <metadata key="prediction" value="{prediction}"/>
    <metadata key="weight" value="{weight}"/>
    <metadata key="pause_count" value="0"/>
    <metadata key="outside" value="false"/>
    <metadata key="support_used" value="{support}"/>
    <metadata key="label_object_enabled" value="false"/>
    <metadata key="filament_maps" value="1"/>
    <filament id="1" tray_info_idx="{tray}" type="{kind}" color="{colour}" used_m="{used_m}" used_g="{weight}" group_id="0" nozzle_diameter="{nozzle_mm:.2}" volume_type="Standard" used_for_object="true" used_for_support="{support}" directly_used="true"/>
    <nozzle id="0" extruder_id="1" nozzle_diameter="{nozzle}" volume_type="Standard"/>
    <layer_filament_lists>
      <layer_filament_list filament_list="0" layer_ranges="0 {last}" />
    </layer_filament_lists>
  </plate>
</config>
"#,
        emitter = crate::EMITTER_VERSION,
        model = f.printer_model_id,
        nozzle = number(f.nozzle_mm),
        nozzle_mm = f.nozzle_mm,
        prediction = f.prediction_s,
        weight = r2(f.weight_g()),
        support = f.support_used,
        kind = f.filament_type,
        colour = f.filament_colour,
        used_m = r2(f.filament_mm / 1000.0),
        last = f.layers.saturating_sub(1),
    )
}

fn plate_json(f: &PlateFacts) -> String {
    let b = f.bbox.map(number);
    let area = (f.bbox[2] - f.bbox[0]) * (f.bbox[3] - f.bbox[1]);
    format!(
        r#"{{"bbox_all":[{b0},{b1},{b2},{b3}],"bbox_objects":[{{"area":{area},"bbox":[{b0},{b1},{b2},{b3}],"id":1,"layer_height":{lh},"name":"Spiling plate"}}],"bed_type":"{bed}","filament_colors":["{colour}"],"filament_ids":[0],"first_extruder":0,"is_seq_print":false,"nozzle_diameter":{nozzle},"version":2}}"#,
        b0 = b[0],
        b1 = b[1],
        b2 = b[2],
        b3 = b[3],
        area = number(area),
        lh = number(f.layer_height_mm),
        bed = f.bed_type,
        colour = f.filament_colour,
        nozzle = number(f.nozzle_mm),
    )
}

/// The project settings Bambu Studio loads with the plate: printer identity,
/// nozzle, plate, filament and temperatures, as strings in its keys.
fn project_settings(f: &PlateFacts) -> String {
    let settings = serde_json::json!({
        "from": "project",
        "name": "project_settings",
        "version": "02.00.00.00",
        "printer_model": f.printer_model,
        "printer_settings_id": format!("Spiling {}", f.printer_model),
        "print_settings_id": "Spiling",
        "filament_settings_id": [format!("Spiling {}", f.filament_type)],
        "nozzle_diameter": [number(f.nozzle_mm)],
        "curr_bed_type": bed_label(f.bed_type),
        "filament_type": [f.filament_type.clone()],
        "filament_colour": [f.filament_colour.clone()],
        "filament_diameter": [number(f.filament_diameter_mm)],
        "filament_density": [f.density.map_or("0".to_string(), number)],
        "filament_ids": [f.filament_id.unwrap_or("")],
        "nozzle_temperature": [number(f.nozzle_c)],
        "nozzle_temperature_initial_layer": [number(f.first_layer_nozzle_c)],
        "hot_plate_temp": [number(f.bed_c)],
        "hot_plate_temp_initial_layer": [number(f.first_layer_bed_c)],
        "textured_plate_temp": [number(f.bed_c)],
        "textured_plate_temp_initial_layer": [number(f.first_layer_bed_c)],
        "layer_height": number(f.layer_height_mm),
        "enable_support": if f.support_used { "1" } else { "0" },
    });
    serde_json::to_string_pretty(&settings).expect("JSON value") + "\n"
}
fn bed_label(identifier: &str) -> &'static str {
    match identifier {
        "cool_plate" => "Cool Plate",
        "eng_plate" => "Engineering Plate",
        "hot_plate" => "High Temp Plate",
        _ => "Textured PEI Plate",
    }
}

/// Write the Bambu package for `program`, already independently verified as
/// `report`.
pub fn write_bambu_package(
    bundle: &ManufacturingBundle,
    program: &ChunkSource,
    report: &VerificationReport,
    objects: &[PackageObject],
    cancel: &AtomicBool,
) -> Result<(ChunkSource, Vec<String>, String), ManufacturingError> {
    for object in objects {
        let n = object.vertices.len();
        if object.triangles.is_empty()
            || object.vertices.iter().flatten().any(|v| !v.is_finite())
            || object.triangles.iter().flatten().any(|&i| i as usize >= n)
        {
            return Err(error(
                ManufacturingErrorCode::InvalidSpecification,
                "package object mesh is empty, nonfinite or misindexed",
            ));
        }
    }
    check_cancel(cancel)?;
    let facts = PlateFacts::new(bundle, report)?;
    let io = |e: std::io::Error| {
        if spiling_chunks::is_limit(&e) {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "delivery package exceeds its representation width",
            )
        } else {
            error(ManufacturingErrorCode::ResourceLimit, e.to_string())
        }
    };
    let zip_error =
        |e: zip::result::ZipError| error(ManufacturingErrorCode::ResourceLimit, e.to_string());
    let mut digest = md5::Md5::default();
    let mut reader = program.reader();
    let mut block = vec![0u8; 1 << 16];
    loop {
        let n = reader.read(&mut block).map_err(io)?;
        if n == 0 {
            break;
        }
        digest.update(&block[..n]);
    }
    let md5_hex = md5::upper_hex(&digest.finish());
    check_cancel(cancel)?;
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .compression_level(Some(6))
        .last_modified_time(zip::DateTime::default());
    let mut zip = zip::ZipWriter::new_stream(ChunkWriter::new(MAX_DELIVERY_PACKAGE_BYTES));
    let mut members: Vec<String> = Vec::new();
    let mut add = |zip: &mut zip::ZipWriter<zip::write::StreamWriter<ChunkWriter>>,
                   name: &str,
                   bytes: &mut dyn Read|
     -> Result<(), ManufacturingError> {
        zip.start_file(name, options).map_err(zip_error)?;
        loop {
            let n = bytes.read(&mut block).map_err(io)?;
            if n == 0 {
                break;
            }
            zip.write_all(&block[..n]).map_err(io)?;
            check_cancel(cancel)?;
        }
        members.push(name.to_string());
        Ok(())
    };
    add(
        &mut zip,
        "[Content_Types].xml",
        &mut CONTENT_TYPES.as_bytes(),
    )?;
    add(&mut zip, "_rels/.rels", &mut ROOT_RELS.as_bytes())?;
    add(&mut zip, "3D/3dmodel.model", &mut model(objects).as_bytes())?;
    // The plate program streams through without an in-memory copy.
    add(&mut zip, BAMBU_PLATE_GCODE, &mut program.reader())?;
    add(
        &mut zip,
        "Metadata/plate_1.gcode.md5",
        &mut md5_hex.as_bytes(),
    )?;
    add(
        &mut zip,
        "Metadata/plate_1.json",
        &mut plate_json(&facts).as_bytes(),
    )?;
    add(
        &mut zip,
        "Metadata/model_settings.config",
        &mut model_settings().as_bytes(),
    )?;
    add(
        &mut zip,
        "Metadata/_rels/model_settings.config.rels",
        &mut MODEL_SETTINGS_RELS.as_bytes(),
    )?;
    add(
        &mut zip,
        "Metadata/slice_info.config",
        &mut slice_info(&facts).as_bytes(),
    )?;
    add(
        &mut zip,
        "Metadata/project_settings.config",
        &mut project_settings(&facts).as_bytes(),
    )?;
    for (name, size) in BAMBU_THUMBNAILS {
        let png = thumbnail::render(&bundle.plan, size, cancel)?;
        add(&mut zip, name, &mut png.as_slice())?;
    }
    drop(add);
    let writer = zip.finish().map_err(zip_error)?.into_inner();
    let bytes = writer.finish().map_err(io)?;
    check_cancel(cancel)?;
    Ok((bytes, members, md5_hex))
}
