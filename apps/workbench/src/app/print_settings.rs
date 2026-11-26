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
    /// The form differs from the plate's settings (or none exist yet).
    pub(super) changed: bool,
    /// Printed extent of the reviewed artifact.
    printed_size: Option<(spiling_contracts::geometry::SourceHash, Option<[f64; 3]>)>,
}

/// A configured machine's default plate and face (the shared contracts rule).
pub(super) fn machine_plate(
    hardware: &HardwareLibrary,
    machine: &str,
) -> Result<(String, String), String> {
    hardware.default_plate_face(machine)
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
        OutputDialect::BambuLabGcode3mfV1 => format!("{} (.gcode.3mf)", setup.label),
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
                _ => s.label.clone(),
            })
            .unwrap_or_else(|| "Choose…".into());
        let filaments = library.project.clone();
        let material_text = match &self.print_settings.material {
            Some(MaterialKey::Generic { family }) => format!("Generic {family}"),
            Some(MaterialKey::Filament { id }) => filaments
                .iter()
                .find(|f| &f.entry.current.id.0 == id)
                .map_or_else(|| "Filament not in this project".into(), |f| f.name.clone()),
            None => "Choose…".into(),
        };
        let mut problem = None;
        let tokens = ui::Tokens::for_ui(ui);
        egui::Grid::new("print-settings")
            .num_columns(2)
            .spacing([12.0, 8.0])
            .min_col_width(64.0)
            .show(ui, |ui| {
                ui.label(egui::RichText::new("Printer").color(tokens.muted));
                egui::ComboBox::from_id_salt("printer")
                    .height(480.0)
                    .width(ui.available_width().min(260.0))
                    .selected_text(printer_text)
                    .show_ui(ui, |ui| {
                        if let Some(catalog) = &catalog {
                            for s in catalog
                                .setups
                                .iter()
                                .filter(|s| s.offers_default_settings())
                            {
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
                ui.end_row();
                if let Some(setup) = self.print_settings_printer() {
                    let e = &setup.printer.build_envelope;
                    ui.label("");
                    ui.label(
                        egui::RichText::new(format!(
                            "{:.0} × {:.0} × {:.0} mm · {:.2} mm nozzle{}",
                            e.max[0] - e.min[0],
                            e.max[1] - e.min[1],
                            e.max[2] - e.min[2],
                            setup.printer.nozzle_diameter_mm,
                            if setup.printer.capabilities.output_dialect
                                == OutputDialect::BambuLabGcode3mfV1
                            {
                                " · .gcode.3mf"
                            } else {
                                ""
                            }
                        ))
                        .small()
                        .color(tokens.muted),
                    );
                    ui.end_row();
                }
                ui.label(egui::RichText::new("Material").color(tokens.muted));
                egui::ComboBox::from_id_salt("material")
                    .height(480.0)
                    .width(ui.available_width().min(260.0))
                    .selected_text(material_text)
                    .show_ui(ui, |ui| {
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
                            ui.weak("This project");
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
                ui.end_row();
                let nozzle = self
                    .print_settings_printer()
                    .map(|s| s.printer.nozzle_diameter_mm);
                let quality = self
                    .print_settings
                    .quality
                    .get_or_insert(QualityPreset::Standard);
                ui.label(egui::RichText::new("Layer").color(tokens.muted));
                ui.horizontal(|ui| {
                    for q in QualityPreset::ALL {
                        let text = match nozzle {
                            Some(n) => format!("{:.2} mm", q.layer_height_mm(n)),
                            None => q.label().into(),
                        };
                        ui.selectable_value(quality, q, text)
                            .on_hover_text(q.label());
                    }
                });
                ui.end_row();
                ui.label(egui::RichText::new("Supports").color(tokens.muted));
                // The shared default (as the CLI): Auto where the printer
                // generates supports, else Off; Auto is not offered there.
                let capable = self
                    .print_settings_printer()
                    .is_none_or(|s| s.printer.capabilities.generated_supports);
                let default_auto = self.print_settings_printer().is_none_or(|s| {
                    SupportChoice::default_for(&s.printer) == SupportChoice::Automatic
                });
                let supports = self.print_settings.supports.get_or_insert(default_auto);
                if !capable {
                    *supports = false;
                }
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(capable, |ui| {
                        ui.selectable_value(supports, true, "Auto")
                            .on_disabled_hover_text("This printer has no generated supports");
                    });
                    ui.selectable_value(supports, false, "Off");
                });
                ui.end_row();
            });
        let setup = self.print_settings_printer().cloned();
        if setup.is_some()
            && let (Some(PrinterKey::Machine { id }), Some(p)) =
                (&self.print_settings.printer, &personal)
            && let Err(reason) = machine_plate(&p.hardware, id)
        {
            problem = Some(reason);
        }
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
        let reason = if !writable {
            Some("Read-only or busy".to_string())
        } else if let Some(reason) = self.slice_draft_reason() {
            Some(reason.into())
        } else if setup.is_none() {
            Some("Choose a printer".into())
        } else if material.is_none() {
            Some("Choose a material".into())
        } else {
            problem.clone()
        };
        // Settings created from this exact choice need no new request; the
        // pane's Slice action then slices them.
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
        self.print_settings.changed = !unchanged;
        ui.add_space(8.0);
        if !unchanged {
            let slicing = self.slicing();
            if ui::primary(
                ui,
                if slicing { "Slicing…" } else { "Slice plate" },
                if slicing { Some("") } else { reason.as_deref() },
            ) && let (Some(printer), Some(material)) = (printer, material)
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
                self.library.queued =
                    Some(LibraryAction::Hardware(HardwareAction::DefaultSettings {
                        project: project.project_id.clone(),
                        layout_id: project.layout_id.clone(),
                        base: project.revision,
                        printer,
                        request,
                    }));
                self.slice = Some(super::slice::SliceRun::after_settings(self.state.serial));
            }
        }
        ui.add_space(4.0);
        egui::CollapsingHeader::new("More")
            .id_salt("print-settings-more")
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Custom printer…").clicked() {
                        self.choose_hardware();
                    }
                    if ui.button("Filament…").clicked() {
                        self.choose_filament();
                    }
                });
                self.hardware_summary(ui);
                ui.horizontal_wrapped(|ui| {
                    self.button(ui, Action::LoadIntent);
                    self.button(ui, Action::Library);
                });
            });
    }

    /// The few numbers a person decides on: time, filament, layers and size.
    pub(super) fn job_headline(
        &mut self,
        ui: &mut egui::Ui,
        report: &VerificationReport,
        plan: &NormalizedPrintPlan,
        artifact: &spiling_contracts::geometry::SourceHash,
    ) {
        let tokens = ui::Tokens::for_ui(ui);
        // One scan per artifact; a large plate is not rescanned every frame.
        let size = match &self.print_settings.printed_size {
            Some((k, size)) if k == artifact => *size,
            _ => {
                let size = printed_size(plan);
                self.print_settings.printed_size = Some((artifact.clone(), size));
                size
            }
        };
        let time = report
            .estimated_seconds
            .map_or_else(|| "Unavailable".into(), duration);
        ui.weak("Estimated time");
        ui.label(egui::RichText::new(time).size(24.0).strong());
        let filament = report.filament_length_mm
            + report.support_filament_length_mm
            + report.priming_filament_length_mm
            + report.adhesion_filament_length_mm;
        let goal = &report.layer_height;
        let layers = if goal.realized_max_mm - goal.realized_min_mm > 0.0005 {
            format!(
                "{} · {:.2}–{:.2} mm",
                plan.layers.len(),
                goal.realized_min_mm,
                goal.realized_max_mm
            )
        } else {
            format!("{} × {:.2} mm", plan.layers.len(), goal.selected_mm)
        };
        egui::Grid::new("job-headline")
            .num_columns(2)
            .spacing([16.0, 4.0])
            .show(ui, |ui| {
                let row = |ui: &mut egui::Ui, k: &str, v: String| {
                    ui.label(egui::RichText::new(k).color(tokens.muted));
                    ui.label(v);
                    ui.end_row();
                };
                row(ui, "Filament", format!("{:.2} m", filament / 1000.0));
                if report.support_filament_length_mm > 0.0 {
                    row(
                        ui,
                        "Support",
                        format!("{:.2} m", report.support_filament_length_mm / 1000.0),
                    );
                }
                row(ui, "Layers", layers);
                if let Some(size) = size {
                    row(
                        ui,
                        "Size",
                        format!("{:.1} × {:.1} × {:.1} mm", size[0], size[1], size[2]),
                    );
                }
            });
    }

    /// The ordinary-job summary of a verified program: estimate, filament and
    /// the reported slowdowns and limitations.
    pub(super) fn job_summary(&self, ui: &mut egui::Ui, report: &VerificationReport) {
        ui::group(ui, "Print job");
        let tokens = ui::Tokens::for_ui(ui);
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
            .show(ui, |ui| {
                for row in &report.limitations {
                    ui.add(egui::Label::new(row).wrap());
                }
            });
    }
}

/// Extent of the printed part itself: model deposition, without skirt, brim
/// or support.
fn printed_size(plan: &NormalizedPrintPlan) -> Option<[f64; 3]> {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for layer in &plan.layers {
        for path in layer
            .paths
            .iter()
            .filter(|p| !p.kind.is_adhesion() && !p.kind.is_support())
        {
            let r = match &path.width {
                WidthProfile::Constant { width_mm } => *width_mm,
                WidthProfile::PerEdge { widths_mm } => {
                    widths_mm.iter().copied().fold(0.0, f64::max)
                }
            } * 0.5;
            for p in &path.points_mm {
                for a in 0..2 {
                    lo[a] = lo[a].min(p[a] - r);
                    hi[a] = hi[a].max(p[a] + r);
                }
            }
            lo[2] = lo[2].min(path.physical_z_mm[0]);
            hi[2] = hi[2].max(path.physical_z_mm[1]);
        }
    }
    (hi[0] > lo[0]).then(|| [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]])
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
