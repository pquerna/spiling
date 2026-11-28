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
    Some(match family.to_ascii_lowercase().as_str() {
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

/// Per-filament facts in a Bambu sliced-plate package.
pub(crate) struct PlateFilamentFact {
    /// 0-based slot/extruder index (`0` for single-spool).
    pub slot: u8,
    pub filament_diameter_mm: f64,
    pub filament_type: String,
    pub filament_colour: String,
    pub filament_id: Option<&'static str>,
    pub density: Option<f64>,
    pub filament_mm: f64,
    pub nozzle_c: f64,
    pub first_layer_nozzle_c: f64,
    pub used_for_object: bool,
    pub used_for_support: bool,
}

impl PlateFilamentFact {
    pub(crate) fn weight_g(&self) -> f64 {
        let area = std::f64::consts::PI * (self.filament_diameter_mm * 0.5).powi(2);
        self.density
            .map_or(0.0, |d| self.filament_mm * area * d / 1000.0)
    }
}

/// Grouped `(sorted 0-based slot IDs, contiguous layer ranges)` for `slice_info.config`.
type LayerFilamentLists = Vec<(Vec<u8>, Vec<(usize, usize)>)>;

/// Package facts derived from the bundle and its fresh verification report.
pub(crate) struct PlateFacts {
    pub printer_model_id: String,
    pub printer_model: String,
    pub bed_type: &'static str,
    pub nozzle_mm: f64,
    pub filaments: Vec<PlateFilamentFact>,
    /// True when single-nozzle AMS is declared on the job.
    pub ams_enabled: bool,
    /// Ordered 0-based slot load/switch sequence (present when AMS is active).
    pub ams_sequence: Vec<u8>,
    /// Per-layer filament lists grouped into contiguous `[first, last]` layer ranges.
    pub layer_filament_lists: LayerFilamentLists,
    /// Prime tower `[min_x, min_y, max_x, max_y]` when printed on the plate.
    pub prime_tower_bbox: Option<[f64; 4]>,
    pub prediction_s: u64,
    pub layer_height_mm: f64,
    pub part_bbox: [f64; 4],
    pub bbox: [f64; 4],
    pub support_used: bool,
    pub bed_c: f64,
    pub first_layer_bed_c: f64,
}

fn filament_identity(family: Option<&str>) -> (String, Option<&'static str>, Option<f64>) {
    let lower = family.map(str::to_ascii_lowercase);
    let known = lower
        .as_deref()
        .and_then(generic_polymer_family)
        .and_then(bambu_filament);
    let filament_type = match (&known, &lower) {
        (Some((_, kind, _)), _) => (*kind).to_string(),
        (None, Some(f)) => f.to_ascii_uppercase(),
        (None, None) => "UNSPECIFIED".to_string(),
    };
    (
        filament_type,
        known.map(|(id, _, _)| id),
        known.map(|(_, _, d)| d),
    )
}

fn layer_filament_lists(
    plan: &NormalizedPrintPlan,
    ams: Option<&BambuAms>,
    tower_layers: u32,
) -> LayerFilamentLists {
    let Some(ams) = ams else {
        return vec![(vec![0], vec![(0, plan.layers.len().saturating_sub(1))])];
    };
    let mut grouped: std::collections::BTreeMap<Vec<u8>, Vec<(usize, usize)>> =
        std::collections::BTreeMap::new();
    let mut active = 0u8;
    for (idx, layer) in plan.layers.iter().enumerate() {
        let mut used = Vec::new();
        let has_switch = {
            let mut cur = active;
            layer.paths.iter().any(|p| {
                let s = ams.slot_for_kind(p.kind);
                let changed = s != cur;
                cur = s;
                changed
            })
        };
        if !has_switch && tower_layers > 0 && (idx as u32) < tower_layers {
            used.push(active);
        }
        for path in &layer.paths {
            let slot = ams.slot_for_kind(path.kind);
            if !used.contains(&slot) {
                used.push(slot);
            }
            active = slot;
        }
        if used.is_empty() {
            used.push(active);
        }
        used.sort_unstable();
        let ranges = grouped.entry(used).or_default();
        if let Some(last) = ranges.last_mut()
            && last.1 + 1 == idx
        {
            last.1 = idx;
        } else {
            ranges.push((idx, idx));
        }
    }
    grouped.into_iter().collect()
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
            ams,
            ..
        }) = &job.delivery
        else {
            return Err(unsupported());
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
        let part_bbox = [lo[0], lo[1], hi[0], hi[1]];
        let (filaments, ams_enabled, ams_sequence, prime_tower_bbox, tower_layers) =
            if let Some(ams) = ams {
                let ams_report = report.ams.as_ref().ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::VerificationFailed,
                        "Bambu AMS delivery package requires verified AMS accounting",
                    )
                })?;
                let mut facts = Vec::new();
                for (slot_idx, slot) in ams.slots.iter().enumerate() {
                    let Some(acct) = ams_report.slots.get(slot_idx) else {
                        return Err(error(
                            ManufacturingErrorCode::VerificationFailed,
                            "AMS verification slot count disagrees with declared slots",
                        ));
                    };
                    let total_mm = acct.total_filament_length_mm();
                    if slot_idx != 0 && total_mm <= 0.0 {
                        continue;
                    }
                    let (filament_type, filament_id, density) =
                        filament_identity(Some(&slot.polymer_family));
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
                        .map_or_else(|| BAMBU_UNDECLARED_COLOUR.to_string(), format_srgb_hex);
                    facts.push(PlateFilamentFact {
                        slot: slot_idx as u8,
                        filament_diameter_mm: slot.filament_diameter_mm,
                        filament_type,
                        filament_colour: colour,
                        filament_id,
                        density,
                        filament_mm: total_mm,
                        nozzle_c: slot.nozzle_temperature_c,
                        first_layer_nozzle_c: slot.first_layer_nozzle_temperature_c,
                        used_for_object: acct.used_for_object,
                        used_for_support: acct.used_for_support,
                    });
                }
                let tower_bbox = ams
                    .prime_tower
                    .as_ref()
                    .filter(|_| ams_report.tower_layers > 0)
                    .map(|t| {
                        let min = t.min_xy_mm();
                        let max = t.max_xy_mm();
                        [min[0], min[1], max[0], max[1]]
                    });
                (
                    facts,
                    true,
                    ams_report.sequence.clone(),
                    tower_bbox,
                    ams_report.tower_layers,
                )
            } else {
                let family = intent
                    .material_profile
                    .as_ref()
                    .map(|p| p.selection.profile.material.polymer_family.as_str());
                let (filament_type, filament_id, density) = filament_identity(family);
                let filament_colour = intent
                    .material_profile
                    .as_ref()
                    .and_then(|p| p.selection.display_srgb)
                    .map_or_else(|| BAMBU_UNDECLARED_COLOUR.to_string(), format_srgb_hex);
                let filament_mm = report.filament_length_mm
                    + report.support_filament_length_mm
                    + report.priming_filament_length_mm
                    + report.adhesion_filament_length_mm;
                (
                    vec![PlateFilamentFact {
                        slot: 0,
                        filament_diameter_mm: intent.recipe.filament_diameter_mm,
                        filament_type,
                        filament_colour,
                        filament_id,
                        density,
                        filament_mm,
                        nozzle_c: job.nozzle_temperature_c,
                        first_layer_nozzle_c: job.first_layer_nozzle_temperature_c,
                        used_for_object: true,
                        used_for_support: report.support_deposition_segments > 0,
                    }],
                    false,
                    Vec::new(),
                    None,
                    0,
                )
            };
        let bbox = match prime_tower_bbox {
            Some(tb) => [
                part_bbox[0].min(tb[0]),
                part_bbox[1].min(tb[1]),
                part_bbox[2].max(tb[2]),
                part_bbox[3].max(tb[3]),
            ],
            None => part_bbox,
        };
        let lists = layer_filament_lists(&bundle.plan, ams.as_ref(), tower_layers);
        Ok(Self {
            printer_model_id: printer_model_id.clone(),
            printer_model: printer_model.clone(),
            bed_type: plate.identifier(),
            nozzle_mm: intent.printer.nozzle_diameter_mm,
            filaments,
            ams_enabled,
            ams_sequence,
            layer_filament_lists: lists,
            prime_tower_bbox,
            prediction_s: report
                .estimated_seconds
                .map_or(0, |s| s.max(0.0).round() as u64),
            layer_height_mm: intent.recipe.layer_height_mm,
            part_bbox,
            bbox,
            support_used: report.support_deposition_segments > 0,
            bed_c: job.bed_temperature_c,
            first_layer_bed_c: job.first_layer_bed_temperature_c,
        })
    }

    pub(crate) fn total_weight_g(&self) -> f64 {
        self.filaments
            .iter()
            .map(|f| (f.weight_g() * 100.0).round() / 100.0)
            .sum()
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
    use std::fmt::Write as _;
    let filament_maps = vec!["1"; f.filaments.len()].join(" ");
    let mut out = format!(
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
    <metadata key="filament_maps" value="{filament_maps}"/>
"#,
        emitter = crate::EMITTER_VERSION,
        model = f.printer_model_id,
        nozzle = number(f.nozzle_mm),
        prediction = f.prediction_s,
        weight = r2(f.total_weight_g()),
        support = f.support_used,
    );
    for fil in &f.filaments {
        let tray = fil.filament_id.unwrap_or("");
        let _ = writeln!(
            out,
            r#"    <filament id="{id}" tray_info_idx="{tray}" type="{kind}" color="{colour}" used_m="{used_m}" used_g="{used_g}" group_id="0" nozzle_diameter="{nozzle_mm:.2}" volume_type="Standard" used_for_object="{obj}" used_for_support="{sup}" directly_used="true"/>"#,
            id = usize::from(fil.slot) + 1,
            kind = fil.filament_type,
            colour = fil.filament_colour,
            used_m = r2(fil.filament_mm / 1000.0),
            used_g = r2(fil.weight_g()),
            nozzle_mm = f.nozzle_mm,
            obj = fil.used_for_object,
            sup = fil.used_for_support,
        );
    }
    let _ = writeln!(
        out,
        r#"    <nozzle id="0" extruder_id="1" nozzle_diameter="{nozzle}" volume_type="Standard"/>"#,
        nozzle = number(f.nozzle_mm),
    );
    out.push_str("    <layer_filament_lists>\n");
    for (slots, ranges) in &f.layer_filament_lists {
        let list: Vec<String> = slots.iter().map(|s| s.to_string()).collect();
        let range_text: Vec<String> = ranges.iter().map(|(a, b)| format!("{a} {b}")).collect();
        let _ = writeln!(
            out,
            r#"      <layer_filament_list filament_list="{}" layer_ranges="{}" />"#,
            list.join(" "),
            range_text.join(","),
        );
    }
    out.push_str("    </layer_filament_lists>\n  </plate>\n</config>\n");
    out
}

fn plate_json(f: &PlateFacts) -> String {
    let b = f.bbox.map(number);
    let pb = f.part_bbox.map(number);
    let part_area = (f.part_bbox[2] - f.part_bbox[0]) * (f.part_bbox[3] - f.part_bbox[1]);
    let mut objects = format!(
        r#"{{"area":{area},"bbox":[{p0},{p1},{p2},{p3}],"id":1,"layer_height":{lh},"name":"Spiling plate"}}"#,
        area = number(part_area),
        p0 = pb[0],
        p1 = pb[1],
        p2 = pb[2],
        p3 = pb[3],
        lh = number(f.layer_height_mm),
    );
    if let Some(tb) = f.prime_tower_bbox {
        let t = tb.map(number);
        let tower_area = (tb[2] - tb[0]) * (tb[3] - tb[1]);
        objects.push_str(&format!(
            r#",{{"area":{area},"bbox":[{t0},{t1},{t2},{t3}],"id":1000,"layer_height":0,"name":"wipe_tower"}}"#,
            area = number(tower_area),
            t0 = t[0],
            t1 = t[1],
            t2 = t[2],
            t3 = t[3],
        ));
    }
    let colours: Vec<String> = f
        .filaments
        .iter()
        .map(|fil| format!(r#""{}""#, fil.filament_colour))
        .collect();
    let ids: Vec<String> = f.filaments.iter().map(|fil| fil.slot.to_string()).collect();
    let first_extruder = f.ams_sequence.first().copied().unwrap_or(0);
    format!(
        r#"{{"bbox_all":[{b0},{b1},{b2},{b3}],"bbox_objects":[{objects}],"bed_type":"{bed}","filament_colors":[{colours}],"filament_ids":[{ids}],"first_extruder":{first_extruder},"is_seq_print":false,"nozzle_diameter":{nozzle},"version":2}}"#,
        b0 = b[0],
        b1 = b[1],
        b2 = b[2],
        b3 = b[3],
        bed = f.bed_type,
        colours = colours.join(","),
        ids = ids.join(","),
        nozzle = number(f.nozzle_mm),
    )
}

fn filament_sequence_json(f: &PlateFacts) -> String {
    let seq: Vec<u32> = f.ams_sequence.iter().map(|&s| u32::from(s) + 1).collect();
    let nozzle_seq = vec![0u32; seq.len()];
    let optimal = vec![0u32; f.filaments.len()];
    serde_json::json!({
        "plate_1": {
            "nozzle_sequence": nozzle_seq,
            "optimal_assignment": optimal,
            "sequence": seq,
        }
    })
    .to_string()
}

/// The project settings Bambu Studio loads with the plate: printer identity,
/// nozzle, plate, filament and temperatures, as strings in its keys.
fn project_settings(f: &PlateFacts) -> String {
    let count = f.filaments.len();
    let mut settings = serde_json::json!({
        "from": "project",
        "name": "project_settings",
        "version": "02.00.00.00",
        "printer_model": f.printer_model,
        "printer_settings_id": format!("Spiling {}", f.printer_model),
        "print_settings_id": "Spiling",
        "filament_settings_id": f.filaments.iter().map(|fil| format!("Spiling {}", fil.filament_type)).collect::<Vec<_>>(),
        "nozzle_diameter": [number(f.nozzle_mm)],
        "curr_bed_type": bed_label(f.bed_type),
        "filament_type": f.filaments.iter().map(|fil| fil.filament_type.clone()).collect::<Vec<_>>(),
        "filament_colour": f.filaments.iter().map(|fil| fil.filament_colour.clone()).collect::<Vec<_>>(),
        "filament_diameter": f.filaments.iter().map(|fil| number(fil.filament_diameter_mm)).collect::<Vec<_>>(),
        "filament_density": f.filaments.iter().map(|fil| fil.density.map_or("0".to_string(), number)).collect::<Vec<_>>(),
        "filament_ids": f.filaments.iter().map(|fil| fil.filament_id.unwrap_or("")).collect::<Vec<_>>(),
        "nozzle_temperature": f.filaments.iter().map(|fil| number(fil.nozzle_c)).collect::<Vec<_>>(),
        "nozzle_temperature_initial_layer": f.filaments.iter().map(|fil| number(fil.first_layer_nozzle_c)).collect::<Vec<_>>(),
        "hot_plate_temp": vec![number(f.bed_c); count],
        "hot_plate_temp_initial_layer": vec![number(f.first_layer_bed_c); count],
        "textured_plate_temp": vec![number(f.bed_c); count],
        "textured_plate_temp_initial_layer": vec![number(f.first_layer_bed_c); count],
        "layer_height": number(f.layer_height_mm),
        "enable_support": if f.support_used { "1" } else { "0" },
    });
    if f.ams_enabled
        && let Some(obj) = settings.as_object_mut()
    {
        obj.insert("single_extruder_multi_material".into(), "1".into());
        obj.insert(
            "enable_prime_tower".into(),
            if f.prime_tower_bbox.is_some() {
                "1"
            } else {
                "0"
            }
            .into(),
        );
    }
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
    if facts.ams_enabled {
        add(
            &mut zip,
            "Metadata/filament_sequence.json",
            &mut filament_sequence_json(&facts).as_bytes(),
        )?;
    }
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
