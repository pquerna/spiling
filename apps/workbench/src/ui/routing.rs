// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_contracts::manufacturing::{
    FixedDualJob, FixedDualTool, InteriorToolBinding, OccurrenceMaterial,
};

pub fn tool_reference(index: usize, tool: &FixedDualTool) -> String {
    format!("T{index} · {:.2} mm nozzle", tool.nozzle_diameter_mm)
}

/// Only edits the caller's captured intent draft. Core/engine owns adoption and validation.
pub fn routed_material_fields(
    ui: &mut egui::Ui,
    job: &mut FixedDualJob,
    writable: bool,
    selected: Option<spiling_contracts::geometry::OccurrenceId>,
) {
    super::group(ui, "Loaded tools");
    for (index, tool) in job.tools.iter().enumerate() {
        ui.push_id(("loaded-tool", &tool.id), |ui| {
            ui.label(egui::RichText::new(tool_reference(index, tool)).strong())
                .on_hover_text(&tool.id);
            ui.label(&tool.material.id);
            ui.weak(format!(
                "{} · {}",
                tool.material.revision, tool.material.polymer_family
            ));
            if let Some(capture) = &tool.material_profile {
                let selected = &capture.selection;
                ui.label(format!(
                    "{:.1} mm/s · ×{:.3} · {:.0} °C",
                    selected.print_speed_mm_s(),
                    selected.flow_multiplier(),
                    selected.nozzle_temperature_c()
                ));
                ui.collapsing("Profile details", |ui| {
                    ui.label(format!(
                        "{} · {}",
                        selected.profile.id, selected.profile.revision
                    ));
                    ui.label(format!(
                        "Flow limit · {:.2} mm³/s",
                        selected
                            .profile
                            .max_volumetric_flow_mm3_s
                            .min(tool.max_volumetric_flow_mm3_s)
                    ));
                    ui.label(format!("Hotend · {}", selected.setup.hotend_id));
                    ui.label(format!(
                        "Cooling · {}",
                        selected.setup.cooling_configuration_id
                    ));
                    ui.weak("Declared process · not physical calibration");
                });
            }
        });
    }
    super::group(ui, "Part bindings");
    let resolved: Vec<_> = job
        .assignments
        .iter()
        .map(|a| match &a.material {
            OccurrenceMaterial::Uniform { material_id } => {
                job.resolve_tool(material_id, a.tool_id.as_deref()).ok()
            }
            _ => None,
        })
        .collect();
    let selected = selected.or_else(|| {
        if job.assignments.len() == 1 {
            Some(job.assignments[0].occurrence_id)
        } else {
            None
        }
    });
    if selected.is_none() {
        ui.weak(format!(
            "{} bound parts · select a part to edit",
            job.assignments.len()
        ));
    }
    let has_construction =
        !job.surface_constructions.is_empty() || job.functional_construction.is_some();
    for (assignment, resolved) in job
        .assignments
        .iter_mut()
        .zip(resolved)
        .filter(|(a, _)| Some(a.occurrence_id) == selected)
    {
        ui.push_id(("part-tool", assignment.occurrence_id), |ui| {
            ui.label(format!("Part {}", assignment.occurrence_id.get()));
            let constructed = job
                .surface_constructions
                .iter()
                .any(|c| c.occurrence_id == assignment.occurrence_id)
                || job
                    .functional_construction
                    .as_ref()
                    .is_some_and(|c| c.occurrence_id == assignment.occurrence_id);
            if constructed || !matches!(assignment.material, OccurrenceMaterial::Uniform { .. }) {
                ui.weak("Source construction");
                return;
            }
            ui.add_enabled_ui(writable, |ui| {
                egui::ComboBox::from_id_salt("loaded-tool")
                    .selected_text(resolved.map_or_else(
                        || "Choose loaded tool".into(),
                        |i| tool_reference(i as usize, &job.tools[i as usize]),
                    ))
                    .show_ui(ui, |ui| {
                        for (index, tool) in job.tools.iter().enumerate() {
                            if ui
                                .selectable_label(
                                    resolved == Some(index as u8),
                                    tool_reference(index, tool),
                                )
                                .clicked()
                            {
                                assignment.tool_id = Some(tool.id.clone());
                                assignment.material = OccurrenceMaterial::Uniform {
                                    material_id: tool.material.id.clone(),
                                };
                            }
                        }
                    });
                if !has_construction {
                    ui.label("Interior fill");
                    egui::ComboBox::from_id_salt("interior-tool")
                        .selected_text(assignment.interior.as_ref().map_or_else(
                            || "Primary tool".into(),
                            |binding| {
                                job.tools
                                    .iter()
                                    .position(|tool| tool.id == binding.tool_id)
                                    .map_or_else(
                                        || "Choose loaded tool".into(),
                                        |index| {
                                            format!("T{index} · {:.2} mm", binding.bead_width_mm)
                                        },
                                    )
                            },
                        ))
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(assignment.interior.is_none(), "Primary tool")
                                .clicked()
                            {
                                assignment.interior = None;
                            }
                            for (index, tool) in job.tools.iter().enumerate() {
                                if resolved == Some(index as u8) {
                                    continue;
                                }
                                let OccurrenceMaterial::Uniform { material_id } =
                                    &assignment.material
                                else {
                                    continue;
                                };
                                if tool.material.id != *material_id {
                                    continue;
                                }
                                if ui
                                    .selectable_label(
                                        assignment
                                            .interior
                                            .as_ref()
                                            .is_some_and(|b| b.tool_id == tool.id),
                                        tool_reference(index, tool),
                                    )
                                    .clicked()
                                {
                                    assignment.interior = Some(InteriorToolBinding {
                                        policy_context: assignment
                                            .interior
                                            .as_ref()
                                            .and_then(|b| b.policy_context.clone()),
                                        policy_override: assignment
                                            .interior
                                            .as_ref()
                                            .and_then(|b| b.policy_override),
                                        tool_id: tool.id.clone(),
                                        bead_width_mm: tool.nozzle_diameter_mm,
                                    });
                                }
                            }
                        });
                    if let Some(binding) = &mut assignment.interior {
                        super::number(ui, "Interior width (mm)", &mut binding.bead_width_mm, 0.01);
                    }
                }
            });
        });
    }
}
