// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! New-job print settings: printer (shipped or My hardware) + material
//! (generic family or project filament) + quality + supports, created in one
//! step by the engine's CreateDefaultIntent and then edited like any intent.
//! The last choice is remembered only as a preselection for the next job;
//! the project's captured hardware and intent stay the authority.
use super::*;
use crate::host::LibraryAction;
use crate::host::hardware::{DefaultPrinter, HardwareAction};
use spiling_contracts::hardware::{HardwareCatalog, HardwareLibrary, HardwareSetupDefinition};
use spiling_contracts::library::LibraryFilamentSummary;
use spiling_contracts::manufacturing::*;
use spiling_contracts::project::{ProjectId, ProjectRevision};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum PrinterKey {
    Shipped { id: String },
    Machine { id: String },
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum MaterialKey {
    Generic { family: String },
    Filament { id: String },
}
/// The last one-step choice: a preselection, never project authority.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub(super) struct Remembered {
    pub printer: PrinterKey,
    pub material: MaterialKey,
    pub quality: QualityPreset,
    pub supports: bool,
}

#[derive(Default)]
pub(super) struct PrintSettingsUi {
    seeded: Option<(ProjectId, bool)>,
    printer: Option<PrinterKey>,
    material: Option<MaterialKey>,
    quality: Option<QualityPreset>,
    supports: Option<bool>,
    hardware_requested: bool,
    filaments_requested: Option<(ProjectId, ProjectRevision)>,
}

/// A configured machine's plate: the one installed on it, else its one
/// compatible plate. Several or none need the hardware sheet.
pub(super) fn machine_plate(
    hardware: &HardwareLibrary,
    machine: &str,
) -> Result<(String, String), String> {
    let m = hardware
        .machines
        .iter()
        .find(|m| m.id == machine)
        .ok_or("machine is no longer in My hardware")?;
    let setup = m.configured_setup()?;
    let compatible: Vec<_> = hardware
        .plates
        .iter()
        .filter(|p| {
            p.definition.mounting_interface == setup.model.mounting_interface
                && p.installed_machine
                    .as_deref()
                    .is_none_or(|id| id == machine)
        })
        .collect();
    let installed: Vec<_> = compatible
        .iter()
        .filter(|p| p.installed_machine.as_deref() == Some(machine))
        .collect();
    let plate = match (installed.as_slice(), compatible.as_slice()) {
        ([one], _) => **one,
        ([], [one]) => *one,
        ([], []) => return Err("No compatible plate in My hardware".into()),
        _ => return Err("Several compatible plates: choose one in Choose hardware".into()),
    };
    let face = plate
        .definition
        .faces
        .first()
        .ok_or("The plate declares no face")?;
    Ok((plate.id.clone(), face.key.clone()))
}

/// The choice a current intent was created from, where it names one.
pub(super) fn seed_from_intent(
    intent: &ManufacturingIntent,
    catalog: Option<&HardwareCatalog>,
    filaments: &[LibraryFilamentSummary],
) -> (
    Option<PrinterKey>,
    Option<MaterialKey>,
    Option<QualityPreset>,
    bool,
) {
    let printer = intent.hardware.as_ref().and_then(|h| {
        catalog
            .and_then(|c| {
                c.setups
                    .iter()
                    .find(|s| **s == h.setup && c.stock_capture(s).is_ok_and(|s| s == **h))
            })
            .map(|s| PrinterKey::Shipped { id: s.id.clone() })
    });
    let material = intent.material_profile.as_ref().map(|p| {
        let m = &p.selection.profile.material;
        filaments
            .iter()
            .find(|f| f.material.as_ref().is_some_and(|fm| fm.id == m.id))
            .map(|f| MaterialKey::Filament {
                id: f.entry.current.id.0.clone(),
            })
            .unwrap_or(MaterialKey::Generic {
                family: m.polymer_family.clone(),
            })
    });
    (
        printer,
        material,
        QualityPreset::from_recipe_id(&intent.recipe.id),
        !matches!(intent.recipe.supports, SupportPolicy::Disabled),
    )
}

fn setup_label(setup: &HardwareSetupDefinition) -> String {
    match setup.printer.capabilities.output_dialect {
        OutputDialect::BambuLabGcode3mfV1 => format!("{} (plan only)", setup.label),
        _ => setup.label.clone(),
    }
}

impl Workbench {
    fn print_settings_printer(&self) -> Option<&HardwareSetupDefinition> {
        let hardware = &self.state.library.hardware;
        match self.print_settings.printer.as_ref()? {
            PrinterKey::Shipped { id } => hardware
                .catalog
                .as_ref()?
                .setups
                .iter()
                .find(|s| &s.id == id),
            PrinterKey::Machine { id } => hardware
                .personal
                .as_ref()?
                .hardware
                .machines
                .iter()
                .find(|m| &m.id == id)?
                .configured_setup()
                .ok(),
        }
    }

    /// The one-step form. Shown in Prepare › Printer.
    pub(super) fn print_settings_ui(&mut self, ui: &mut egui::Ui, writable: bool) {
        let (Some(project), Some(_)) = (self.state.project.clone(), self.state.hello.clone())
        else {
            return;
        };
        let intent = self.state.manufacturing.intent.clone();
        let library = self.state.library.clone();
        // Load the catalog once and the project's filaments once per revision.
        let current = (project.project_id.clone(), project.revision);
        if self.library.queued.is_none() {
            if library.hardware.catalog.is_none() && !self.print_settings.hardware_requested {
                self.print_settings.hardware_requested = true;
                self.library.queued = Some(LibraryAction::Hardware(HardwareAction::Refresh));
            } else if library.project_capture.as_ref() != Some(&current)
                && self.print_settings.filaments_requested.as_ref() != Some(&current)
            {
                self.print_settings.filaments_requested = Some(current);
                self.library.queued = Some(LibraryAction::Refresh(Default::default()));
            }
        }
        if library.hardware.catalog.is_none() {
            ui::group(ui, "Print settings");
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Loading printers…");
            });
            return;
        }
        // Seed once per project (and again when settings first appear).
        let key = (project.project_id.clone(), intent.is_some());
        if self.print_settings.seeded.as_ref() != Some(&key) {
            let s = &mut self.print_settings;
            s.seeded = Some(key);
            if let Some(intent) = &intent {
                let (printer, material, quality, supports) =
                    seed_from_intent(intent, library.hardware.catalog.as_ref(), &library.project);
                s.printer = printer.or(s.printer.take());
                s.material = material.or(s.material.take());
                s.quality = quality.or(s.quality);
                s.supports = Some(supports);
            } else if let Some(r) = &self.prefs.new_job {
                s.printer.get_or_insert(r.printer.clone());
                s.material.get_or_insert(r.material.clone());
                s.quality.get_or_insert(r.quality);
                s.supports.get_or_insert(r.supports);
            }
        }
        ui::group(ui, "Print settings");
        ui.weak(if intent.is_some() {
            "Recreate the plate's settings from a printer, material and quality. Undo restores the current settings."
        } else {
            "Choose a printer, material and quality. Settings are created for you and stay editable."
        });
        // Printer.
        let catalog = library.hardware.catalog.clone();
        let personal = library.hardware.personal.clone();
        let printer_text = self
            .print_settings_printer()
            .map(|s| match &self.print_settings.printer {
                Some(PrinterKey::Machine { id }) => personal
                    .as_ref()
                    .and_then(|p| p.hardware.machines.iter().find(|m| &m.id == id))
                    .map_or_else(
                        || s.label.clone(),
                        |m| format!("{} · {}", m.nickname, s.label),
                    ),
                _ => setup_label(s),
            })
            .unwrap_or_else(|| "Choose a printer".into());
        egui::ComboBox::from_label("Printer")
            .height(480.0)
            .selected_text(printer_text)
            .show_ui(ui, |ui| {
                if let Some(catalog) = &catalog {
                    ui.weak("Shipped printers");
                    for s in catalog.setups.iter().filter(|s| {
                        matches!(s.printer.workflow, JobWorkflow::CompleteMarlin { .. })
                    }) {
                        ui.selectable_value(
                            &mut self.print_settings.printer,
                            Some(PrinterKey::Shipped { id: s.id.clone() }),
                            setup_label(s),
                        );
                    }
                } else {
                    ui.spinner();
                }
                if let Some(p) = &personal {
                    let configured: Vec<_> = p
                        .hardware
                        .machines
                        .iter()
                        .filter_map(|m| m.configured_setup().ok().map(|s| (m, s)))
                        .collect();
                    if !configured.is_empty() {
                        ui.separator();
                        ui.weak("My hardware");
                    }
                    for (m, s) in configured {
                        ui.selectable_value(
                            &mut self.print_settings.printer,
                            Some(PrinterKey::Machine { id: m.id.clone() }),
                            format!("{} · {}", m.nickname, s.label),
                        );
                    }
                }
            });
        let setup = self.print_settings_printer().cloned();
        let mut problem = None;
        if let Some(setup) = &setup {
            let e = &setup.printer.build_envelope;
            ui.weak(format!(
                "{:.2} mm nozzle · {:.0} × {:.0} × {:.0} mm",
                setup.printer.nozzle_diameter_mm,
                e.max[0] - e.min[0],
                e.max[1] - e.min[1],
                e.max[2] - e.min[2]
            ));
            if setup.printer.capabilities.output_dialect == OutputDialect::BambuLabGcode3mfV1 {
                ui.colored_label(
                    self.prefs.theme.tokens().warning,
                    "Plans and verifies; program export waits for .gcode.3mf delivery.",
                );
            }
            if let (Some(PrinterKey::Machine { id }), Some(p)) =
                (&self.print_settings.printer, &personal)
                && let Err(reason) = machine_plate(&p.hardware, id)
            {
                problem = Some(reason);
            }
        }
        if ui.small_button("Custom printer…").clicked() {
            self.choose_hardware();
        }
        // Material.
        let filaments = library.project.clone();
        let material_text = match &self.print_settings.material {
            Some(MaterialKey::Generic { family }) => format!("Generic {family}"),
            Some(MaterialKey::Filament { id }) => filaments
                .iter()
                .find(|f| &f.entry.current.id.0 == id)
                .map_or_else(|| "Filament not in this project".into(), |f| f.name.clone()),
            None => "Choose a material".into(),
        };
        egui::ComboBox::from_label("Material")
            .height(480.0)
            .selected_text(material_text)
            .show_ui(ui, |ui| {
                ui.weak("Generic");
                for family in GENERIC_FAMILIES {
                    ui.selectable_value(
                        &mut self.print_settings.material,
                        Some(MaterialKey::Generic {
                            family: family.into(),
                        }),
                        format!("Generic {family}"),
                    );
                }
                if !filaments.is_empty() {
                    ui.separator();
                    ui.weak("This project's filaments");
                }
                for f in &filaments {
                    ui.selectable_value(
                        &mut self.print_settings.material,
                        Some(MaterialKey::Filament {
                            id: f.entry.current.id.0.clone(),
                        }),
                        f.name.clone(),
                    );
                }
            });
        if ui.small_button("Choose filament…").clicked() {
            self.choose_filament();
        }
        // Quality and supports.
        let nozzle = setup.as_ref().map(|s| s.printer.nozzle_diameter_mm);
        let quality = self
            .print_settings
            .quality
            .get_or_insert(QualityPreset::Standard);
        ui.horizontal(|ui| {
            ui.label("Quality");
            for q in QualityPreset::ALL {
                let text = match nozzle {
                    Some(n) => format!("{} · {:.2} mm", q.label(), q.layer_height_mm(n)),
                    None => q.label().into(),
                };
                ui.selectable_value(quality, q, text);
            }
        });
        let supports = self.print_settings.supports.get_or_insert(true);
        ui.checkbox(supports, "Generate supports where needed");
        if *supports
            && setup
                .as_ref()
                .is_some_and(|s| !s.printer.capabilities.generated_supports)
        {
            problem = Some("This printer declares no generated supports; turn supports off".into());
        }
        // One primary action, named by its effect.
        let material = match &self.print_settings.material {
            Some(MaterialKey::Generic { family }) => Some(DefaultMaterialChoice::Generic {
                polymer_family: family.clone(),
            }),
            Some(MaterialKey::Filament { id }) => filaments
                .iter()
                .find(|f| &f.entry.current.id.0 == id)
                .map(|f| DefaultMaterialChoice::ProjectFilament {
                    reference: f.entry.current.clone(),
                    profile_id: None,
                }),
            None => None,
        };
        let printer = match &self.print_settings.printer {
            Some(PrinterKey::Shipped { id }) => Some(DefaultPrinter::Shipped(id.clone())),
            Some(PrinterKey::Machine { id }) => personal.as_ref().and_then(|p| {
                machine_plate(&p.hardware, id)
                    .ok()
                    .map(|(plate, face)| DefaultPrinter::Machine {
                        expected: p.identity.clone(),
                        machine: id.clone(),
                        plate,
                        face,
                    })
            }),
            None => None,
        };
        let drafts = self.project_drafts();
        let reason = if !writable {
            Some("Project is read-only or an operation is pending".to_string())
        } else if drafts {
            Some("Apply or discard the open settings draft first".into())
        } else if setup.is_none() {
            Some("Choose a printer".into())
        } else if material.is_none() {
            Some("Choose a material".into())
        } else {
            problem.clone()
        };
        // With settings present, Replace is offered only for a different choice.
        let unchanged = intent.as_ref().is_some_and(|intent| {
            let (p, m, q, sup) =
                seed_from_intent(intent, library.hardware.catalog.as_ref(), &library.project);
            let chosen_printer = match &self.print_settings.printer {
                Some(PrinterKey::Machine { .. }) => None,
                other => other.clone(),
            };
            p.is_some()
                && p == chosen_printer
                && m == self.print_settings.material
                && q == self.print_settings.quality
                && Some(sup) == self.print_settings.supports
        });
        if unchanged {
            ui.weak("The plate's settings were created from this choice.");
            return;
        }
        let label = if intent.is_some() {
            "Replace print settings"
        } else {
            "Create print settings"
        };
        if ui::primary(ui, label, reason.as_deref())
            && let (Some(printer), Some(material)) = (printer, material)
        {
            let request = DefaultIntentRequest {
                material,
                quality: self
                    .print_settings
                    .quality
                    .unwrap_or(QualityPreset::Standard),
                supports: if self.print_settings.supports.unwrap_or(true) {
                    SupportChoice::Automatic
                } else {
                    SupportChoice::Off
                },
            };
            self.prefs.new_job = Some(Remembered {
                printer: self.print_settings.printer.clone().expect("chosen printer"),
                material: self
                    .print_settings
                    .material
                    .clone()
                    .expect("chosen material"),
                quality: request.quality,
                supports: request.supports == SupportChoice::Automatic,
            });
            self.library.queued = Some(LibraryAction::Hardware(HardwareAction::DefaultSettings {
                project: project.project_id.clone(),
                layout_id: project.layout_id.clone(),
                base: project.revision,
                printer,
                request,
            }));
        }
    }

    /// The ordinary-job summary of a verified program: estimate, filament and
    /// the reported slowdowns and limitations.
    pub(super) fn job_summary(&self, ui: &mut egui::Ui, report: &VerificationReport) {
        ui::group(ui, "Print job");
        let tokens = self.prefs.theme.tokens();
        if let Some(seconds) = report.estimated_seconds {
            ui::property(ui, "Estimated time", duration(seconds), tokens);
        } else if report.job_time.is_none() {
            ui::property(ui, "Estimated time", "Not estimated", tokens);
        }
        let total = report.filament_length_mm
            + report.support_filament_length_mm
            + report.priming_filament_length_mm;
        ui::property(
            ui,
            "Filament",
            format!(
                "{:.2} m ({:.2} m part, {:.2} m support, {:.2} m priming)",
                total / 1000.0,
                report.filament_length_mm / 1000.0,
                report.support_filament_length_mm / 1000.0,
                report.priming_filament_length_mm / 1000.0
            ),
            tokens,
        );
        if report.flow_limited_segments > 0 {
            ui.label(format!(
                "Slowed to the volumetric flow limit on {} segments",
                report.flow_limited_segments
            ));
        }
        if report.cooling_slowed_layers > 0 {
            ui.label(format!(
                "Slowed for minimum layer time on {} layers (down to {:.0} %)",
                report.cooling_slowed_layers,
                report.minimum_cooling_scale * 100.0
            ));
        }
        egui::CollapsingHeader::new(format!("Limitations ({})", report.limitations.len()))
            .id_salt("job-limitations")
            .default_open(!report.limitations.is_empty())
            .show(ui, |ui| {
                for row in &report.limitations {
                    ui.add(egui::Label::new(row).wrap());
                }
            });
    }
}

pub(super) fn duration(seconds: f64) -> String {
    let total = seconds.round().max(0.0) as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h} h {m:02} min")
    } else if m > 0 {
        format!("{m} min {s:02} s")
    } else {
        format!("{s} s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiling_contracts::hardware::builtin_catalog;
    #[test]
    fn durations_read_like_a_slicer() {
        assert_eq!(duration(42.4), "42 s");
        assert_eq!(duration(284.5), "4 min 45 s");
        assert_eq!(duration(3600.0 * 2.0 + 61.0), "2 h 01 min");
    }
    #[test]
    fn a_created_intent_seeds_its_own_choice() {
        let catalog = builtin_catalog().unwrap();
        let setup = catalog.find_setup("mk4s").unwrap();
        let hardware = std::sync::Arc::new(catalog.stock_capture(setup).unwrap());
        let intent = spiling_manufacturing_free_intent(hardware);
        let (printer, material, quality, supports) = seed_from_intent(&intent, Some(&catalog), &[]);
        assert_eq!(
            printer,
            Some(PrinterKey::Shipped {
                id: "prusa.mk4s.04".into()
            })
        );
        assert_eq!(
            material,
            Some(MaterialKey::Generic {
                family: "PLA".into()
            })
        );
        assert_eq!(quality, Some(QualityPreset::Fine));
        assert!(!supports);
        let remembered = Remembered {
            printer: printer.unwrap(),
            material: material.unwrap(),
            quality: QualityPreset::Fine,
            supports,
        };
        let json = serde_json::to_string(&remembered).unwrap();
        assert_eq!(
            serde_json::from_str::<Remembered>(&json).unwrap(),
            remembered
        );
    }
    /// A defaults-shaped intent without depending on the manufacturing crate:
    /// the shipped Marlin-reference fixture with the MK4S capture.
    fn spiling_manufacturing_free_intent(
        hardware: std::sync::Arc<spiling_contracts::hardware::ProjectHardwareCapture>,
    ) -> ManufacturingIntent {
        let mut intent = decode_intent(include_bytes!(
            "../../../../fixtures/manufacturing/complete-sparse.intent.json"
        ))
        .unwrap();
        intent.hardware = Some(hardware);
        intent.recipe.id = QualityPreset::Fine.recipe_id();
        intent.recipe.supports = SupportPolicy::Disabled;
        let mut selection: MaterialProfileSelection = serde_json::from_slice(include_bytes!(
            "../../../../fixtures/manufacturing/material-profile-a.json"
        ))
        .unwrap();
        selection.profile.material.polymer_family = "PLA".into();
        intent.material_profile = Some(ResolvedMaterialProfile {
            resolver_version: MATERIAL_RESOLVER_VERSION,
            selection,
        });
        intent
    }
}
