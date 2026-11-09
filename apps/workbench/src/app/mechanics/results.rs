// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_contracts::analysis::{fields::AnalysisFields, mechanics::AnalysisRunSettings};

fn settings_match(input: &str, captured: &AnalysisRunSettings) -> bool {
    input.len() <= 48 * 1024
        && serde_json::from_str::<AnalysisRunSettings>(input)
            .is_ok_and(|settings| settings == *captured)
}

impl Workbench {
    pub(super) fn mechanics_results(&mut self, ui: &mut egui::Ui, ready: bool) {
        ui.separator();
        ui::group(ui, "Assumed-law numerical study");
        ui.weak("Planned, directional wall/skin/effective-core continuum; interfaces assumed tied. No witnessed road attachment, calibrated strength or Basic release qualification.");
        ui.collapsing("Explicit print recipe and assumed laws", |ui| {
            ui.label("Paste AnalysisRunSettings JSON (including law-compatible recipe). No material or load is supplied implicitly.");
            ui.add(
                egui::TextEdit::multiline(&mut self.mechanics.settings_json)
                    .desired_rows(7)
                    .desired_width(f32::INFINITY),
            );
            if self.mechanics.settings_json.len() > 48 * 1024 {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    "Settings exceed the 48 KiB native form limit; shorten the input.",
                );
            }
        });
        let selected = self
            .mechanics
            .draft
            .as_ref()
            .filter(|draft| !draft.dirty() && draft.submitted.is_none())
            .map(|draft| draft.value.clone());
        let reason = if !ready {
            Some("Wait for the current operation")
        } else if self.mechanics.has_changes() {
            Some("Apply or discard case edits before solving")
        } else if selected.is_none() {
            Some("Select an applied saved case")
        } else if self.mechanics.settings_json.trim().is_empty() {
            Some("Supply an explicit recipe and assumed laws")
        } else if self.mechanics.settings_json.len() > 48 * 1024 {
            Some("Settings exceed the form limit")
        } else {
            None
        };
        if ui::primary(ui, "Solve assumed printed part", reason) {
            let parsed = serde_json::from_str::<AnalysisRunSettings>(&self.mechanics.settings_json)
                .map_err(|error| error.to_string())
                .and_then(|settings| {
                    settings
                        .validate()
                        .map(|()| settings)
                        .map_err(|error| error.to_string())
                });
            match parsed {
                Ok(settings) => {
                    if let (Some(project), Some(hello), Some(case)) =
                        (&self.state.project, &self.state.hello, selected)
                    {
                        self.queued_project = Some(ProjectCommand::RunAnalysis {
                            session_id: hello.session_id.clone(),
                            base_revision: project.revision,
                            case_id: case.id,
                            settings: std::sync::Arc::new(settings),
                        });
                    }
                }
                Err(error) => self.local_problem = Some(error),
            }
        }
        let Some(capture) = self.state.mechanics.clone().filter(|capture| {
            self.state
                .project
                .as_ref()
                .is_some_and(|project| project.project_id == capture.project)
        }) else {
            ui.weak("No successful numerical result in this workbench session. Authoring metadata is not a solve.");
            return;
        };
        let current = !self.mechanics.has_changes()
            && settings_match(&self.mechanics.settings_json, &capture.summary.settings)
            && self.state.project.as_ref().is_some_and(|project| {
                project.revision == capture.revision
                    && project.layout_id == capture.summary.case.case.layout_id
                    && project.analysis.cases.contains(&capture.summary.case)
            })
            && self
                .mechanics
                .draft
                .as_ref()
                .is_some_and(|draft| draft.value.id == capture.summary.case.case.id);
        let summary = &capture.summary;
        if let Some(previous) = self
            .state
            .mechanics_previous
            .as_ref()
            .filter(|previous| previous.project == capture.project)
        {
            ui.collapsing("Previous construction / orientation · same service wrench", |ui| {
                ui.label(format!("Max displacement {:.5} → {:.5} mm", previous.summary.maximum_displacement_mm, summary.maximum_displacement_mm));
                ui.label(format!("Max sampled centroid macro von Mises {:.5} → {:.5} MPa", previous.summary.maximum_macro_von_mises_mpa, summary.maximum_macro_von_mises_mpa));
                ui.label(format!("Strain energy {:.6} → {:.6} N·mm", previous.summary.strain_energy_n_mm, summary.strain_energy_n_mm));
                ui.weak("Two declared assumed-law models, not a calibrated improvement or strength comparison.");
            });
        }
        ui.strong(if current {
            "Last successful captured result"
        } else {
            "Stale captured result · project, case draft or study settings differ"
        });
        ui.label(format!(
            "{} · {} elements · {} DOFs · CPU FP64",
            summary.case.case.name, summary.elements, summary.dofs
        ));
        ui.label(format!(
            "Max nodal displacement {:.5} mm · max sampled centroid macro von Mises {:.5} MPa",
            summary.maximum_displacement_mm, summary.maximum_macro_von_mises_mpa
        ));
        ui.label(format!("Original-coefficient free residual {:.3e} N · force balance {:.3e} N · moment balance {:.3e} N·mm", summary.assessment.free_residual_n, summary.assessment.force_imbalance_n, summary.assessment.moment_imbalance_n_mm));
        ui.weak("Algebraic assessment is not discretization/model accuracy. Fine-road N1, weld and physical calibration evidence remain unqualified. Native closed-slab error is separate.");
        let costs = &summary.costs;
        ui.collapsing("Model and cost evidence", |ui| {
            ui.label(format!("Native capture {:.2} ms · construction {:.2} ms · original operator setup {:.2} ms", costs.native_capture_ms, costs.construction_ms, costs.operator_setup_ms));
            ui.label(format!("RHS {:.2} ms · solve {:.2} ms · independent assessment {:.2} ms · field computation {:.2} ms · reused={}", costs.rhs_ms, costs.solve_ms, costs.assessment_ms, costs.fields_ms, costs.operator_reused));
            ui.weak("These are engine numerical phases, not case/edit timings, transfer/packing costs or cold/p50/p95 cohorts. Proposed performance targets are not achieved-product claims.");
            ui.label(format!("Disjoint region volumes [wall, skin, core] {:?} mm³; energy {:?} N·mm; native shape error {:.6} mm", summary.region_volume_mm3, summary.region_energy_n_mm, summary.native_shape_error_mm));
            ui.label(&summary.settings.laws.name);
            ui.label(&summary.settings.laws.justification);
            ui.monospace(format!("input {}", summary.input_hash.as_str()));
            ui.monospace(format!("operator {}", summary.operator_hash.as_str()));
            ui.weak("Immutable fields are retained in the local engine operation ledger, not attached to the portable .spil project yet. Reopening a project can solve from its saved case without original STEP; a workbench restart does not restore this panel automatically.");
            ui.monospace(&capture.resource.name);
        });
        let fields = match AnalysisFields::decode(&capture.bytes) {
            Ok(fields) => fields,
            Err(error) => {
                ui.label(error.to_string());
                return;
            }
        };
        let choices = [
            "Centroid macro von Mises · MPa",
            "Signed macro layer-normal · MPa",
            "Centroid displacement · mm",
            "Regional energy concentration · ×",
        ];
        egui::ComboBox::from_id_salt("mechanical-field")
            .selected_text(choices[self.mechanics.field])
            .show_ui(ui, |ui| {
                for (index, label) in choices.into_iter().enumerate() {
                    ui.selectable_value(&mut self.mechanics.field, index, label);
                }
            });
        let mut bounds = [[f64::INFINITY, f64::NEG_INFINITY]; 3];
        for index in 0..fields.node_count {
            let node = fields.node(index).unwrap();
            for (axis, range) in bounds.iter_mut().enumerate() {
                range[0] = range[0].min(node.position_mm[axis]);
                range[1] = range[1].max(node.position_mm[axis]);
            }
        }
        let slice = self
            .mechanics
            .slice_z
            .get_or_insert((bounds[2][0] + bounds[2][1]) * 0.5);
        *slice = slice.clamp(bounds[2][0], bounds[2][1]);
        ui.add(egui::Slider::new(slice, bounds[2][0]..=bounds[2][1]).text("Source Z · mm"));
        let mut cells = Vec::new();
        let mut minimum = f64::INFINITY;
        let mut maximum = f64::NEG_INFINITY;
        let layer_axis = (0..3)
            .max_by(|first, second| {
                let [qx, qy, qz, _] = summary.pose.rotation_xyzw;
                let rotation = glam::DQuat::from_xyzw(qx, qy, qz, summary.pose.rotation_xyzw[3]);
                rotation
                    .mul_vec3(glam::DVec3::AXES[*first])
                    .z
                    .abs()
                    .total_cmp(&rotation.mul_vec3(glam::DVec3::AXES[*second]).z.abs())
            })
            .unwrap();
        for index in 0..fields.element_count {
            let element = fields.element(index).unwrap();
            if (*slice - element.center_mm[2]).abs() > 0.5 * element.size_mm[2] + 1e-8 {
                continue;
            }
            let stress = element.stress_mpa;
            let value = match self.mechanics.field {
                0 => (0.5
                    * ((stress[0] - stress[1]).powi(2)
                        + (stress[1] - stress[2]).powi(2)
                        + (stress[2] - stress[0]).powi(2))
                    + 3.0 * (stress[3].powi(2) + stress[4].powi(2) + stress[5].powi(2)))
                .sqrt(),
                1 => stress[layer_axis],
                2 => fields
                    .node(element.nodes[13] as usize)
                    .unwrap()
                    .displacement_mm
                    .iter()
                    .map(|value| value * value)
                    .sum::<f64>()
                    .sqrt(),
                _ => element.concentration,
            };
            minimum = minimum.min(value);
            maximum = maximum.max(value);
            cells.push((element, value));
        }
        if cells.is_empty() {
            ui.weak("No material cells on this slice.");
            return;
        }
        ui.label(format!(
            "Actual displayed scale: {:.5} → {:.5}",
            minimum, maximum
        ));
        ui.weak(format!("Source X/Y slice; constant color per cell from centroid samples, not resolved roads or a road maximum. Concentration uses radius {:.3} mm and nominal deposited-volume normalization.",summary.concentration_radius_mm));
        let width = ui.available_width().max(80.0);
        let height = 240.0;
        let (response, painter) =
            ui.allocate_painter(egui::vec2(width, height), egui::Sense::hover());
        let scale = ((width - 8.0) as f64 / (bounds[0][1] - bounds[0][0]))
            .min((height - 8.0) as f64 / (bounds[1][1] - bounds[1][0]));
        let project = |point: [f64; 2]| {
            egui::pos2(
                response.rect.left() + 4.0 + ((point[0] - bounds[0][0]) * scale) as f32,
                response.rect.bottom() - 4.0 - ((point[1] - bounds[1][0]) * scale) as f32,
            )
        };
        for (element, value) in cells {
            let fraction = if maximum > minimum {
                ((value - minimum) / (maximum - minimum)) as f32
            } else {
                0.5
            };
            let color = egui::Color32::from_rgb(
                (35.0 + 210.0 * fraction) as u8,
                (100.0 + 60.0 * (1.0 - fraction)) as u8,
                (225.0 - 180.0 * fraction) as u8,
            );
            let lower = project([
                element.center_mm[0] - 0.5 * element.size_mm[0],
                element.center_mm[1] - 0.5 * element.size_mm[1],
            ]);
            let upper = project([
                element.center_mm[0] + 0.5 * element.size_mm[0],
                element.center_mm[1] + 0.5 * element.size_mm[1],
            ]);
            let rect = egui::Rect::from_two_pos(lower, upper);
            painter.rect_filled(rect, 0.0, color);
            if response
                .hover_pos()
                .is_some_and(|position| rect.contains(position))
            {
                response.clone().on_hover_text(format!(
                    "centroid {:?} mm\nvalue {:.6}\nregion {}\nmacro stress {:?} MPa",
                    element.center_mm,
                    value,
                    ["wall", "skin", "assumed effective core"][element.role as usize],
                    element.stress_mpa
                ));
            }
        }
        ui.separator();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numerical_settings_edits_cannot_mislabel_a_capture_as_current() {
        let input = include_str!("../../../../../fixtures/analysis/hook-assumptions.json");
        let captured: AnalysisRunSettings = serde_json::from_str(input).unwrap();
        assert!(settings_match(input, &captured));
        assert!(settings_match(
            &serde_json::to_string(&captured).unwrap(),
            &captured
        ));
        let mut changed = captured.clone();
        changed.maximum_span_mm = 6.0;
        assert!(!settings_match(input, &changed));
        assert!(!settings_match("{}", &captured));
        assert!(!settings_match(&" ".repeat(48 * 1024 + 1), &captured));
    }
}
