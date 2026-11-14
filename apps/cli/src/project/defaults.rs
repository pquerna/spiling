// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! One-step entry: `--printer`, `--material`, `--quality`, `--supports` and
//! the `--list-printers` / `--list-materials` listings. Names resolve against
//! the engine's shipped hardware catalog, the personal library and the
//! project's captured filaments; the engine's CreateDefaultIntent creates the
//! intent. Nothing is chosen silently: every resolution is reported.
use super::{info, project_materials};
use crate::Error;
use spiling_contracts::hardware::{
    HardwareCatalog, LibraryHardware, LocalProjectHardwareBinding, ProjectHardwareCapture,
};
use spiling_contracts::manufacturing::*;
use spiling_contracts::project::{PrintLayoutId, ProjectCommand};
use spiling_engine_client::EngineClient;
use std::sync::Arc;

pub(super) fn quality(value: &str) -> Result<QualityPreset, Error> {
    QualityPreset::parse(value).ok_or_else(|| {
        format!("unknown --quality {value:?}; choose draft, standard or fine").into()
    })
}
pub(super) fn supports(value: &str) -> Result<SupportChoice, Error> {
    match value {
        "on" => Ok(SupportChoice::Automatic),
        "off" => Ok(SupportChoice::Off),
        _ => Err(format!("unknown --supports {value:?}; choose on or off").into()),
    }
}

/// A resolved printer: a shipped setup on its stock plate, or a configured
/// personal-library machine (which also yields its local binding).
pub(super) struct Printer {
    pub capture: ProjectHardwareCapture,
    pub binding: Option<(String, String, u64)>,
    pub report: serde_json::Value,
}

fn emission(capture: &ProjectHardwareCapture) -> &'static str {
    match capture.setup.printer.capabilities.output_dialect {
        OutputDialect::BambuLabGcode3mfV1 => {
            "plans and verifies; program emission refused until .gcode.3mf delivery exists"
        }
        _ => "plain G-code",
    }
}

pub(super) async fn resolve_printer(client: &EngineClient, query: &str) -> Result<Printer, Error> {
    let catalog = client.rpc().hardware_catalog().await?;
    let shipped = match catalog.find_setup(query) {
        Ok(setup) => {
            let capture = catalog.stock_capture(setup)?;
            let report = serde_json::json!({
                "source": "shipped",
                "query": query,
                "setup_id": setup.id,
                "label": setup.label,
                "plate": capture.plate.id,
                "face": capture.face,
                "output": emission(&capture),
            });
            return Ok(Printer {
                capture,
                binding: None,
                report,
            });
        }
        Err(error) => error,
    };
    // Not a shipped setup: a configured machine in My hardware by nickname or id.
    let personal = match client.rpc().library_hardware().await {
        Ok(personal) => personal,
        Err(library) => {
            return Err(format!("{shipped}; personal library unavailable: {library}").into());
        }
    };
    let q = query.trim().to_lowercase();
    let machines: Vec<_> = personal
        .hardware
        .machines
        .iter()
        .filter(|m| m.id == query || m.nickname.to_lowercase() == q)
        .collect();
    let machine = match machines.as_slice() {
        [one] => *one,
        [] => {
            return Err(format!("{shipped}; no machine in My hardware is named {query:?}").into());
        }
        _ => {
            return Err(format!(
                "several machines in My hardware are named {query:?}; use the machine id"
            )
            .into());
        }
    };
    let setup = machine.configured_setup()?;
    let plates: Vec<_> = personal
        .hardware
        .plates
        .iter()
        .filter(|p| {
            p.definition.mounting_interface == setup.model.mounting_interface
                && p.installed_machine
                    .as_deref()
                    .is_none_or(|id| id == machine.id)
        })
        .collect();
    let installed: Vec<_> = plates
        .iter()
        .filter(|p| p.installed_machine.as_deref() == Some(machine.id.as_str()))
        .collect();
    let plate = match (installed.as_slice(), plates.as_slice()) {
        ([one], _) => **one,
        ([], [one]) => *one,
        ([], []) => {
            return Err(format!(
                "machine {:?} has no compatible plate in My hardware",
                machine.nickname
            )
            .into());
        }
        _ => {
            return Err(format!(
                "machine {:?} has several compatible plates; use --hardware-local MACHINE PLATE FACE",
                machine.nickname
            )
            .into());
        }
    };
    let face = plate
        .definition
        .faces
        .first()
        .ok_or("the plate declares no face")?
        .key
        .clone();
    let capture = personal.hardware.capture(&machine.id, &plate.id, &face)?;
    let report = serde_json::json!({
        "source": "my_hardware",
        "query": query,
        "machine_id": machine.id,
        "nickname": machine.nickname,
        "setup_id": setup.id,
        "label": setup.label,
        "plate": plate.id,
        "face": face,
        "output": emission(&capture),
    });
    Ok(Printer {
        capture,
        binding: Some((machine.id.clone(), plate.id.clone(), machine.generation)),
        report,
    })
}

pub(super) fn binding(
    project: &spiling_contracts::project::ProjectInfo,
    attachment: &spiling_contracts::geometry::NativePath,
    capture: &ProjectHardwareCapture,
    (machine_id, plate_id, machine_generation): (String, String, u64),
) -> Result<LocalProjectHardwareBinding, Error> {
    Ok(LocalProjectHardwareBinding {
        layout_id: project.layout_id.clone(),
        project_id: project.project_id.as_str().into(),
        attachment: attachment.clone(),
        capture_hash: capture.hash()?,
        machine_id,
        plate_id,
        machine_generation,
    })
}

/// A project-captured filament by exact name or id, otherwise a generic
/// polymer family.
async fn material_choice(
    client: &mut EngineClient,
    name: &str,
) -> Result<(DefaultMaterialChoice, serde_json::Value), Error> {
    let rows = project_materials(client).await?;
    let matches: Vec<_> = rows
        .iter()
        .filter(|r| r.entry.current.id.0 == name || r.name.eq_ignore_ascii_case(name))
        .collect();
    match matches.as_slice() {
        [row] => Ok((
            DefaultMaterialChoice::ProjectFilament {
                reference: row.entry.current.clone(),
                profile_id: None,
            },
            serde_json::json!({"source":"project_filament","query":name,"name":row.name,"filament":row.entry.current}),
        )),
        [] => Ok((
            DefaultMaterialChoice::Generic {
                polymer_family: name.into(),
            },
            serde_json::json!({"source":"generic","query":name,
                "family": generic_polymer_family(name)}),
        )),
        _ => {
            Err(format!("several project filaments are named {name:?}; use the filament id").into())
        }
    }
}

/// Create the default intent through the engine and report what was chosen.
pub(super) async fn create(
    client: &mut EngineClient,
    layout_id: &PrintLayoutId,
    material: &str,
    quality: QualityPreset,
    supports: SupportChoice,
) -> Result<serde_json::Value, Error> {
    let (choice, material_report) = material_choice(client, material).await?;
    let base_revision = info(client).await?.revision;
    let response = client
        .manufacturing(ManufacturingCommand::CreateDefaultIntent {
            layout_id: layout_id.clone(),
            session_id: client.hello().session_id.clone(),
            base_revision,
            request: DefaultIntentRequest {
                material: choice,
                quality,
                supports,
            },
        })
        .await?;
    let ManufacturingResponse::Status {
        intent: Some(intent),
        ..
    } = response
    else {
        return Err("unexpected default intent response".into());
    };
    Ok(serde_json::json!({
        "step": "defaults",
        "material": material_report,
        "quality": quality,
        "supports": supports,
        "created": summary(&intent),
    }))
}

/// What the created intent holds: every value the defaults chose.
pub(super) fn summary(intent: &ManufacturingIntent) -> serde_json::Value {
    let selection = intent.material_profile.as_ref().map(|p| &p.selection);
    serde_json::json!({
        "printer": intent.hardware.as_ref().map(|h| &h.setup.id),
        "printer_label": intent.printer.name,
        "recipe": intent.recipe.id,
        "quality": QualityPreset::from_recipe_id(&intent.recipe.id),
        "material_profile": selection.map(|s| &s.profile.id),
        "material": selection.map(|s| format!("{} {}", s.profile.material.manufacturer, s.profile.material.product)),
        "polymer_family": selection.map(|s| &s.profile.material.polymer_family),
        "explicit_overrides": selection.map(|s| &s.overrides),
        "tool_setup": selection.map(|s| &s.setup),
        "layer_height_mm": intent.recipe.layer_height_mm,
        "first_layer_height_mm": intent.recipe.first_layer_height_mm,
        "bead_width_mm": intent.recipe.bead_width_mm,
        "perimeters": intent.recipe.perimeter_count,
        "infill_fraction": intent.recipe.infill_fraction,
        "supports": intent.recipe.supports,
        "role_speeds_mm_s": intent.recipe.role_speeds,
        "max_volumetric_flow_mm3_s": intent.effective_max_volumetric_flow_mm3_s(),
    })
}

fn printer_rows(catalog: &HardwareCatalog) -> Vec<serde_json::Value> {
    catalog
        .setups
        .iter()
        .map(|s| {
            let c = catalog.stock_capture(s).ok();
            let e = &s.printer.build_envelope;
            serde_json::json!({
                "id": s.id,
                "label": s.label,
                "vendor": s.model.vendor,
                "model": s.model.model,
                "nozzle_mm": s.printer.nozzle_diameter_mm,
                "build_volume_mm": [e.max[0]-e.min[0], e.max[1]-e.min[1], e.max[2]-e.min[2]],
                "dialect": s.printer.capabilities.output_dialect,
                "generated_supports": s.printer.capabilities.generated_supports,
                "one_step_defaults": matches!(s.printer.workflow, JobWorkflow::CompleteMarlin { .. }),
                "output": c.as_ref().map(emission),
            })
        })
        .collect()
}
fn library_rows(personal: &LibraryHardware) -> Vec<serde_json::Value> {
    personal
        .hardware
        .machines
        .iter()
        .map(|m| {
            serde_json::json!({
                "id": m.id,
                "nickname": m.nickname,
                "setup": m.configured_setup().ok().map(|s| &s.id),
                "configured": m.configured_setup().is_ok(),
            })
        })
        .collect()
}

pub(super) async fn list_printers(client: &EngineClient) -> Result<serde_json::Value, Error> {
    let catalog = client.rpc().hardware_catalog().await?;
    let personal = match client.rpc().library_hardware().await {
        Ok(p) => serde_json::json!(library_rows(&p)),
        Err(error) => serde_json::json!({"unavailable": error.to_string()}),
    };
    Ok(serde_json::json!({
        "step": "list_printers",
        "shipped": printer_rows(&catalog),
        "my_hardware": personal,
        "usage": "--printer ID|LABEL|WORDS (shipped) or a My hardware nickname",
    }))
}

pub(super) async fn list_materials(
    client: &mut EngineClient,
    in_project: bool,
) -> Result<serde_json::Value, Error> {
    let generic = generic_material_profiles()?;
    let families: Vec<_> = GENERIC_FAMILIES
        .iter()
        .map(|family| {
            let profiles: Vec<_> = generic
                .iter()
                .filter(|p| p.material.polymer_family == *family)
                .collect();
            serde_json::json!({
                "family": family,
                "nozzles_mm": profiles.iter().map(|p| p.nozzle_diameter_mm).collect::<Vec<_>>(),
                "profiles": profiles.iter().map(|p| &p.id).collect::<Vec<_>>(),
            })
        })
        .collect();
    let filaments = if in_project {
        let rows = project_materials(client).await?;
        serde_json::json!(
            rows.iter()
                .map(|r| serde_json::json!({
                    "id": r.entry.current.id, "name": r.name,
                    "family": r.material.as_ref().map(|m| &m.polymer_family),
                    "profiles": r.profile_count,
                }))
                .collect::<Vec<_>>()
        )
    } else {
        serde_json::json!([])
    };
    Ok(serde_json::json!({
        "step": "list_materials",
        "generic": families,
        "project_filaments": filaments,
        "usage": "--material FAMILY (pla, petg, abs, asa, tpu; PLA+ and similar name their family) or a project filament name/id (capture one with --library-use FILAMENT_UUID)",
    }))
}

/// Set the resolved printer as project hardware.
pub(super) async fn set_printer(
    client: &mut EngineClient,
    layout_id: &PrintLayoutId,
    capture: ProjectHardwareCapture,
) -> Result<(), Error> {
    let base_revision = info(client).await?.revision;
    client
        .project(ProjectCommand::SetHardware {
            layout_id: layout_id.clone(),
            session_id: client.hello().session_id.clone(),
            base_revision,
            capture: Some(Arc::new(capture)),
        })
        .await?;
    Ok(())
}
