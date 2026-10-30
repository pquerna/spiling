// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_contracts::{
    manufacturing::*,
    project::{ProjectId, ProjectRevision},
};

#[derive(Default)]
pub(super) struct MaterialUi {
    identity: Option<(ProjectId, SessionId)>,
    ignored_nonce: Option<u64>,
    draft: Option<MaterialDraft>,
}
struct MaterialDraft {
    revision: ProjectRevision,
    nonce: u64,
    original: MaterialProfileSelection,
    value: MaterialProfileSelection,
}
impl MaterialUi {
    fn sync(
        &mut self,
        project: &ProjectId,
        session: &SessionId,
        revision: ProjectRevision,
        loaded: Option<&crate::manufacture::LoadedMaterial>,
        captured: Option<&MaterialProfileSelection>,
    ) {
        let identity = (project.clone(), session.clone());
        if self.identity.as_ref() != Some(&identity) {
            *self = Self {
                identity: Some(identity),
                ..Self::default()
            };
        }
        let loaded = loaded.filter(|l| {
            &l.project == project && &l.session == session && self.ignored_nonce != Some(l.nonce)
        });
        let (nonce, base, source) = if let Some(l) = loaded {
            (l.nonce, l.revision, Some(&l.selection))
        } else {
            (0, revision, captured)
        };
        let reset = self.draft.as_ref().is_none_or(|d| {
            d.nonce != nonce
                || (d.revision != revision
                    && ((nonce == 0 && d.value == d.original) || captured == Some(&d.value)))
        });
        if reset {
            self.draft = source.map(|s| MaterialDraft {
                revision: base,
                nonce,
                original: s.clone(),
                value: s.clone(),
            });
        }
    }
}
impl Workbench {
    pub(super) fn material_ui(&mut self, ui: &mut egui::Ui, writable: bool) {
        let (Some(project), Some(hello)) = (self.state.project.clone(), self.state.hello.clone())
        else {
            return;
        };
        let m = self.state.manufacturing.clone();
        let captured = m
            .intent
            .as_ref()
            .and_then(|i| i.material_profile.as_ref())
            .map(|p| &p.selection);
        self.material.sync(
            &project.project_id,
            &hello.session_id,
            project.revision,
            m.loaded_material.as_deref(),
            captured,
        );
        ui.heading("Material and setup");
        self.button(ui, Action::LoadMaterial);
        let pending_intent =
            m.loaded.is_some() || self.manufacturing.has_intent_draft(&project.project_id);
        if pending_intent {
            ui.weak("Apply or reload the full intent draft before applying a material profile.");
        }
        let Some(d) = &mut self.material.draft else {
            ui.weak("Select an explicit profile JSON. No material or setup is inferred.");
            return;
        };
        if let Some(l) = &m.loaded_material
            && l.nonce == d.nonce
        {
            ui.weak(format!("Selected for review: {}", l.label));
        }
        let p = &d.value.profile;
        ui.label(format!(
            "{} · {}",
            p.material.manufacturer, p.material.product
        ));
        ui.weak(format!(
            "Profile {} / {} · material {} / {}",
            p.id, p.revision, p.material.id, p.material.revision
        ));
        ui.label(match p.source.kind {
            MaterialProfileSourceKind::SyntheticTest => {
                "Synthetic test declaration · not calibrated"
            }
            MaterialProfileSourceKind::ImportedRecommendation => {
                "Imported recommendation · not calibrated"
            }
            MaterialProfileSourceKind::OperatorDeclaration => {
                "Operator declaration · not calibrated"
            }
        });
        ui.collapsing("Captured setup and provenance", |ui| {
            ui.label(format!(
                "Printer {} / {} · nozzle {:.3} mm",
                p.printer_id, p.printer_revision, p.nozzle_diameter_mm
            ));
            ui.label(format!(
                "Hotend {} · {} · cooling {}",
                d.value.setup.hotend_id,
                d.value.setup.nozzle_material,
                d.value.setup.cooling_configuration_id
            ));
            ui.label(format!(
                "Abrasion-resistant nozzle: {} · material requires: {}",
                d.value.setup.abrasion_resistant_nozzle,
                p.material.requires_abrasion_resistant_nozzle
            ));
            ui.label(format!(
                "{} · filament {:.3} mm",
                p.material.polymer_family, p.material.filament_diameter_mm
            ));
            if let Some(filler) = &p.material.filler {
                let name = match filler.kind {
                    MaterialFillerKind::CarbonFiber => "Carbon fiber",
                    MaterialFillerKind::GlassFiber => "Glass fiber",
                };
                let fraction = filler.mass_fraction.map_or_else(
                    || "mass fraction undeclared".into(),
                    |f| format!("{:.1}% mass", f * 100.0),
                );
                ui.label(format!("Filler: {name} · {fraction}"));
            } else {
                ui.label("Filler: none declared");
            }
            ui.label(format!(
                "Source {} / {}",
                p.source.source_id, p.source.source_revision
            ));
            ui.label(format!("Source hash {}", p.source.source_hash.as_str()));
            ui.label(format!(
                "Declared material flow ceiling {:.3} mm³/s",
                p.max_volumetric_flow_mm3_s
            ));
            ui.label(format!(
                "Declared bead width {:.4}…{:.4} mm; height {:.4}…{:.4} mm",
                p.deposition_limits.min_bead_width_mm,
                p.deposition_limits.max_bead_width_mm,
                p.deposition_limits.min_bead_height_mm,
                p.deposition_limits.max_bead_height_mm
            ));
        });
        let conflict = d.revision != project.revision;
        if conflict {
            ui.colored_label(self.prefs.theme.tokens().warning, "Project changed while editing this material draft. Reload the captured profile before applying.");
        }
        ui.add_enabled_ui(writable && !conflict, |ui| {
            ui.weak("Enable an override to replace its profile value. Clear it to inherit again.");
            scalar(ui, "Nozzle", "°C", p.nozzle_temperature_c, &mut d.value.overrides.nozzle_temperature_c, 1.0);
            scalar(ui, "Bed", "°C", p.bed_temperature_c, &mut d.value.overrides.bed_temperature_c, 1.0);
            scalar(ui, "Speed", "mm/s", p.print_speed_mm_s, &mut d.value.overrides.print_speed_mm_s, 0.1);
            scalar(ui, "Flow", "×", p.flow_multiplier, &mut d.value.overrides.flow_multiplier, 0.01);
            let mut process = d.value.overrides.process.is_some();
            if ui.checkbox(&mut process, "Override cooling, bridges and retraction").changed() {
                d.value.overrides.process = process.then(|| p.process.clone());
            }
            if let Some(policy) = &mut d.value.overrides.process {
                ui.push_id("material-process-override", |ui| super::policies::process_fields(ui, policy));
            } else {
                ui.weak(format!("Profile policy: later fan {} PWM · bridge span {:.3} mm · bridge speed ×{:.3}, flow ×{:.3}", p.process.fan_after_first_layers, p.process.max_bridge_span_mm, p.process.bridge_speed_factor, p.process.bridge_flow_factor));
            }
        });
        let validity = d.value.validate();
        if let Err(error) = &validity {
            ui.colored_label(self.prefs.theme.tokens().warning, error.to_string());
        }
        if ui
            .add_enabled(
                writable
                    && !conflict
                    && validity.is_ok()
                    && !pending_intent
                    && captured != Some(&d.value),
                egui::Button::new("Apply material profile"),
            )
            .clicked()
        {
            self.queued_manufacturing = Some(ManufacturingCommand::ApplyMaterialProfile {
                session_id: hello.session_id.clone(),
                base_revision: d.revision,
                selection: d.value.clone(),
            });
        }
        if ui.button("Reload captured profile").clicked() {
            self.material.ignored_nonce = m.loaded_material.as_ref().map(|l| l.nonce);
            self.material.draft = None;
        }
        if let Some(intent) = &m.intent
            && let Some(profile) = &intent.material_profile
        {
            let selected = &profile.selection;
            ui.collapsing("Applied engine values", |ui| {
                ui.label(format!("{} / {} · project revision {}", selected.profile.id, selected.profile.revision, project.revision.0));
                if let JobWorkflow::CompleteMarlin { job } = &intent.printer.workflow {
                    ui.label(format!("Nozzle {:.0} °C · bed {:.0} °C", job.nozzle_temperature_c, job.bed_temperature_c));
                }
                ui.label(format!("Speed {:.3} mm/s · flow ×{:.4}", intent.recipe.print_speed_mm_s, intent.recipe.flow_multiplier));
                ui.label(format!("Effective flow ceiling {:.3} mm³/s · machine {:.3} · material {:.3}", intent.effective_max_volumetric_flow_mm3_s(), intent.printer.max_volumetric_flow_mm3_s, selected.profile.max_volumetric_flow_mm3_s));
                let p = &intent.recipe.process;
                ui.label(format!("First layer: speed ×{:.3} · flow ×{:.3} · fan {} PWM", p.first_layer_speed_factor, p.first_layer_flow_factor, p.fan_first_layers));
                ui.label(format!("Later layers: fan {} PWM", p.fan_after_first_layers));
                ui.label(format!("Bridges: maximum {:.3} mm · speed ×{:.3} · flow ×{:.3} · fan {} PWM", p.max_bridge_span_mm, p.bridge_speed_factor, p.bridge_flow_factor, p.bridge_fan_pwm));
                match p.retraction {
                    RetractionPolicy::Disabled => { ui.label("Retraction disabled"); }
                    RetractionPolicy::Enabled { distance_mm, speed_mm_s, minimum_travel_mm, z_hop_mm } => {
                        ui.label(format!("Retraction: {:.3} mm at {:.3} mm/s · minimum travel {:.3} mm · Z hop {:.3} mm", distance_mm, speed_mm_s, minimum_travel_mm, z_hop_mm));
                    }
                }
                ui.weak("These are applied engine values. Draft overrides take effect only after Apply succeeds. Declarations and replay are not physical calibration.");
            });
        }
    }
}
fn scalar(
    ui: &mut egui::Ui,
    label: &str,
    unit: &str,
    profile: f64,
    value: &mut Option<f64>,
    speed: f64,
) {
    ui.push_id(label, |ui| {
        ui.horizontal_wrapped(|ui| {
            let mut enabled = value.is_some();
            let response = ui
                .checkbox(&mut enabled, format!("{label} ({unit})"))
                .on_hover_text("Enable an explicit override; clear to inherit the profile value.");
            if response.changed() {
                *value = enabled.then_some(profile);
            }
            if let Some(value) = value {
                ui.add(
                    egui::DragValue::new(value)
                        .speed(speed)
                        .suffix(format!(" {unit}")),
                )
                .labelled_by(response.id)
                .on_hover_text(format!(
                    "Profile value: {profile:.3} {unit}. Clear the override to inherit it."
                ));
            } else {
                ui.weak(format!("{profile} · profile"));
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn selection() -> MaterialProfileSelection {
        serde_json::from_slice(include_bytes!(
            "../../../../fixtures/manufacturing/material-profile-a.json"
        ))
        .unwrap()
    }
    fn ids() -> (ProjectId, SessionId) {
        (ProjectId::new(), SessionId::new())
    }
    #[test]
    fn rejected_and_stale_drafts_preserve_overrides_until_reload() {
        let (project, session) = ids();
        let captured = selection();
        let mut state = MaterialUi::default();
        state.sync(
            &project,
            &session,
            ProjectRevision(2),
            None,
            Some(&captured),
        );
        state
            .draft
            .as_mut()
            .unwrap()
            .value
            .overrides
            .print_speed_mm_s = Some(28.0);
        // A rejected request leaves the authoritative revision and capture unchanged.
        state.sync(
            &project,
            &session,
            ProjectRevision(2),
            None,
            Some(&captured),
        );
        assert_eq!(
            state
                .draft
                .as_ref()
                .unwrap()
                .value
                .overrides
                .print_speed_mm_s,
            Some(28.0)
        );
        // An unrelated project transaction cannot rebase an edited draft implicitly.
        state.sync(
            &project,
            &session,
            ProjectRevision(3),
            None,
            Some(&captured),
        );
        assert_eq!(state.draft.as_ref().unwrap().revision, ProjectRevision(2));
        assert_eq!(
            state
                .draft
                .as_ref()
                .unwrap()
                .value
                .overrides
                .print_speed_mm_s,
            Some(28.0)
        );
        state.draft = None;
        state.sync(
            &project,
            &session,
            ProjectRevision(3),
            None,
            Some(&captured),
        );
        assert_eq!(state.draft.as_ref().unwrap().value, captured);
        assert_eq!(state.draft.as_ref().unwrap().revision, ProjectRevision(3));
    }
    #[test]
    fn applied_receipt_and_new_session_refresh_clean_drafts() {
        let (project, session) = ids();
        let mut captured = selection();
        let mut state = MaterialUi::default();
        state.sync(
            &project,
            &session,
            ProjectRevision(2),
            None,
            Some(&captured),
        );
        captured.overrides.flow_multiplier = Some(0.9);
        state.draft.as_mut().unwrap().value = captured.clone();
        state.sync(
            &project,
            &session,
            ProjectRevision(3),
            None,
            Some(&captured),
        );
        assert_eq!(state.draft.as_ref().unwrap().original, captured);
        assert_eq!(state.draft.as_ref().unwrap().revision, ProjectRevision(3));
        state
            .draft
            .as_mut()
            .unwrap()
            .value
            .overrides
            .flow_multiplier = Some(0.8);
        state.sync(
            &project,
            &SessionId::new(),
            ProjectRevision(3),
            None,
            Some(&captured),
        );
        assert_eq!(state.draft.as_ref().unwrap().value, captured);
    }
    #[test]
    fn loaded_selection_retains_its_revision_and_can_be_explicitly_discarded() {
        let (project, session) = ids();
        let captured = selection();
        let mut selected = captured.clone();
        selected.overrides.bed_temperature_c = Some(65.0);
        let loaded = crate::manufacture::LoadedMaterial {
            nonce: 7,
            label: "Selected.json".into(),
            project: project.clone(),
            session: session.clone(),
            revision: ProjectRevision(2),
            selection: selected.clone(),
        };
        let mut state = MaterialUi::default();
        state.sync(
            &project,
            &session,
            ProjectRevision(3),
            Some(&loaded),
            Some(&captured),
        );
        assert_eq!(state.draft.as_ref().unwrap().revision, ProjectRevision(2));
        assert_eq!(state.draft.as_ref().unwrap().value, selected);
        state.ignored_nonce = Some(7);
        state.draft = None;
        state.sync(
            &project,
            &session,
            ProjectRevision(3),
            Some(&loaded),
            Some(&captured),
        );
        assert_eq!(state.draft.as_ref().unwrap().value, captured);
    }
}
