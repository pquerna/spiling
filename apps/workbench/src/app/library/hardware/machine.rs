// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::*;
impl Workbench {
    pub(super) fn hardware_machines(
        &mut self,
        ui: &mut egui::Ui,
        p: &LibraryHardware,
        catalog: Option<&HardwareCatalog>,
        ready: bool,
    ) {
        if !self.library.hardware.machine_detail {
            ui.horizontal_wrapped(|ui| {
                if self.library.hardware.machine.is_some() {
                    if ui::primary(ui, "Resume machine draft", None) {
                        self.library.hardware.machine_detail = true;
                    }
                    if ui.button("Discard machine draft").clicked() {
                        self.library.hardware.machine = None;
                        self.library.hardware.mounting_interface.clear();
                    }
                } else if ui::primary(
                    ui,
                    "Add printer",
                    (!ready).then_some("Wait for the current operation"),
                ) {
                    self.library
                        .discovery
                        .open(spiling_contracts::catalog::CatalogKind::Printer);
                }
            });
            ui.weak("Private inventory · applying a setup to a project is a separate action");
            egui::ScrollArea::vertical()
                .id_salt("owned-machine-list")
                .show(ui, |ui| {
                    for m in &p.hardware.machines {
                        let state = m.configured_setup().map_or("Setup needed".to_owned(), |s| {
                            format!("{} mm nozzle", s.printer.nozzle_diameter_mm)
                        });
                        let identity = if m.identity.model.starts_with(&m.identity.manufacturer) {
                            m.identity.model.clone()
                        } else {
                            format!("{} {}", m.identity.manufacturer, m.identity.model)
                        };
                        let label = if m.nickname == identity {
                            format!("{} · {state}", m.nickname)
                        } else {
                            format!("{} · {identity} · {state}", m.nickname)
                        };
                        if ui
                            .add_enabled(
                                ready && self.library.hardware.machine.is_none(),
                                egui::Button::new(label).wrap(),
                            )
                            .clicked()
                        {
                            self.library.hardware.clear_secret();
                            self.library.hardware.machine = Some(m.clone());
                            self.library.hardware.machine_detail = true;
                        }
                    }
                });
            return;
        }
        let Some(mut draft) = self.library.hardware.machine.clone() else {
            self.library.hardware.machine_detail = false;
            return;
        };
        let original = p.hardware.machines.iter().find(|m| m.id == draft.id);
        if self.library.hardware.connections && original.is_some() {
            if ui::close(ui, "Back to machine") {
                self.library.hardware.connections = false;
                self.library.hardware.clear_secret();
            }
            egui::ScrollArea::vertical()
                .show(ui, |ui| self.hardware_connections(ui, p, &draft.id, ready));
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.strong(if original.is_some() {
                "Machine"
            } else {
                "Add printer"
            });
            if ui::close(ui, "Back to machines") {
                self.library.hardware.machine_detail = false;
                self.library.hardware.clear_secret();
            }
        });
        ui.weak("My library · private physical instance");
        let mut saved = false;
        let mut discard = false;
        egui::Panel::bottom("machine-editor-actions")
            .exact_size(76.0)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    let validation = draft.validate().err();
                    let reason = if !ready {
                        Some("Wait for the current library operation")
                    } else if original == Some(&draft) {
                        Some("No unapplied changes")
                    } else {
                        validation.as_deref()
                    };
                    if ui::primary(
                        ui,
                        if original.is_some() {
                            "Save machine"
                        } else {
                            "Add printer"
                        },
                        reason,
                    ) {
                        if let MachineConfiguration::Configured { setup } = &mut draft.configuration
                            && original.is_some_and(|old| {
                                old.configuration
                                    != MachineConfiguration::Configured {
                                        setup: setup.clone(),
                                    }
                            })
                        {
                            // A changed setup is a new custom identity; its printer is
                            // re-derived from the edited facts. Material profiles are
                            // keyed by compatibility, so they survive this re-identity.
                            setup.id = format!("custom.{}", uuid::Uuid::new_v4());
                            setup.label = format!("Custom {} setup", setup.model.model);
                            setup.rederive().expect("validated machine facts");
                        }
                        let mut h = p.hardware.clone();
                        if let Some(old) = h.machines.iter_mut().find(|m| m.id == draft.id) {
                            *old = draft.clone();
                        } else {
                            h.machines.push(draft.clone());
                        }
                        self.library.hardware.submitted_machine = Some(draft.clone());
                        self.save_hardware(p, h);
                        saved = true;
                    }
                    if ui.button("Discard machine draft").clicked() {
                        discard = true;
                    }
                });
            });
        if discard {
            self.library.hardware.mounting_interface.clear();
            self.library.hardware.machine = None;
            self.library.hardware.machine_detail = false;
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("machine-editor-body")
            .min_scrolled_height(0.0)
            .show(ui, |ui| {
                ui.push_id(draft.id.clone(), |ui| {
                    ui.add_enabled_ui(ready, |ui| {
                        self.machine_identity_fields(ui, &mut draft, original.is_none());
                        self.machine_configuration_fields(ui, &mut draft, catalog);
                        if original.is_some() {
                            if ui.button("Connections…").clicked() {
                                self.library.hardware.connections = true;
                            }
                            let has_credentials = p.hardware.routes.iter().any(|r| r.machine_id == draft.id && r.credential.is_some());
                            ui.collapsing("Remove machine", |ui| {
                                if has_credentials {
                                    ui.weak("Forget connection credentials before removing this machine.");
                                }
                                if ui.add_enabled(!has_credentials, egui::Button::new("Remove from My Hardware")).clicked() {
                                    let mut h = p.hardware.clone();
                                    h.machines.retain(|m| m.id != draft.id);
                                    h.routes.retain(|r| r.machine_id != draft.id);
                                    h.bindings.retain(|b| b.machine_id != draft.id);
                                    for plate in &mut h.plates {
                                        if plate.installed_machine.as_deref() == Some(&draft.id) { plate.installed_machine = None; }
                                    }
                                    for accessory in &mut h.accessories {
                                        if accessory.installed_machine.as_deref() == Some(&draft.id) { accessory.installed_machine = None; }
                                    }
                                    self.save_hardware(p, h);
                                    discard = true;
                                }
                            });
                        }
                    });
                });
            });
        if discard {
            self.library.hardware.mounting_interface.clear();
            self.library.hardware.machine = None;
            self.library.hardware.machine_detail = false;
        } else {
            self.library.hardware.machine = Some(draft);
        }
        if saved {
            ui.ctx().request_repaint();
        }
    }
    fn machine_identity_fields(
        &mut self,
        ui: &mut egui::Ui,
        draft: &mut OwnedMachine,
        is_new: bool,
    ) {
        field(ui, "Name", &mut draft.nickname);
        let can_edit = is_new
            && draft.identity.catalog_origin.is_none()
            && matches!(draft.configuration, MachineConfiguration::Unconfigured);
        ui.add_enabled_ui(can_edit, |ui| {
            field(ui, "Manufacturer", &mut draft.identity.manufacturer);
            field(ui, "Model", &mut draft.identity.model);
        });
        if let Some(origin) = &draft.identity.catalog_origin {
            ui.collapsing("Catalog source", |ui| {
                ui.label(&origin.origin.url);
                ui.label(format!("{} · {}", origin.origin.id, origin.origin.revision));
                if let spiling_contracts::catalog::CatalogItem::Printer { volume_mm, .. } =
                    &origin.item
                {
                    ui.label(volume_mm.map_or("Build size unknown".into(), |v| {
                        format!("{} × {} × {} mm · declared", v[0], v[1], v[2])
                    }));
                }
            });
        }
        if let Some(origin) = &draft.identity.catalog_origin {
            let lookup = self
                .state
                .library
                .catalog_lookup
                .as_ref()
                .filter(|s| s.key == origin.key())
                .map(|s| &s.result);
            match ui::catalog_review(ui, origin, lookup, true) {
                Some(ui::CatalogReviewAction::Check) => {
                    self.library.queued = Some(LibraryAction::LookupCatalog(origin.key()))
                }
                Some(ui::CatalogReviewAction::Review(row)) => {
                    draft.identity.catalog_origin = Some(row);
                }
                None => {}
            }
        }
        ui.collapsing("Private details", |ui| {
            let mut serial = draft.serial.clone().unwrap_or_default();
            let mut location = draft.location.clone().unwrap_or_default();
            field(ui, "Serial", &mut serial);
            field(ui, "Location", &mut location);
            draft.serial = optional(&serial);
            draft.location = optional(&location);
        });
    }
    fn machine_configuration_fields(
        &mut self,
        ui: &mut egui::Ui,
        draft: &mut OwnedMachine,
        catalog: Option<&HardwareCatalog>,
    ) {
        if let MachineConfiguration::Configured { setup } = &mut draft.configuration {
            ui::group(ui, "Installed setup");
            ui.label(&setup.label);
            if let Some(machine) = &mut setup.machine {
                machine_fact_fields(ui, machine);
                if let Err(reason) = setup.rederive() {
                    ui.colored_label(ui.visuals().warn_fg_color, reason);
                }
            } else {
                ui.weak(format!(
                    "Declared software reference · {} mm nozzle · not editable; choose a machine-description setup to edit facts",
                    setup.printer.nozzle_diameter_mm
                ));
            }
            ui.weak(format!("Mount: {}", setup.model.mounting_interface));
            ui::help(
                ui,
                "installed-settings",
                "Installed setup",
                "Setups are derived from machine facts: prime line, park, envelope and deposition limits follow the bed and nozzle. Each fact records its source. These are declarations, not physical printer qualification. Existing projects retain their captured setup until Apply.",
            );
            return;
        }
        ui::group(ui, "Setup needed");
        ui.label("Save this printer now; configure it before use in a project.");
        let Some(c) = catalog else {
            return;
        };
        for setup in c.setups.iter().filter(|s| draft.identity.matches_setup(s)) {
            if ui.button(format!("Use {}", setup.label)).clicked() {
                draft.configuration = MachineConfiguration::Configured {
                    setup: Box::new(setup.clone()),
                };
                return;
            }
        }
        if !c.setups.iter().any(|s| draft.identity.matches_setup(s)) {
            ui.weak("No admitted setup for this model in this build.");
        }
        {
            ui.collapsing("Declare a custom setup", |ui| {
                ui.weak("Start from a machine description, then edit the facts that differ.");
                egui::ComboBox::from_id_salt("custom-machine-template")
                    .selected_text(
                        c.setups
                            .get(self.library.hardware.setup)
                            .map_or("Choose settings", |s| s.label.as_str()),
                    )
                    .show_ui(ui, |ui| {
                        for (i, s) in c
                            .setups
                            .iter()
                            .enumerate()
                            .filter(|(_, s)| s.machine.is_some())
                        {
                            ui.selectable_value(&mut self.library.hardware.setup, i, &s.label);
                        }
                    });
                field(
                    ui,
                    "Mount interface",
                    &mut self.library.hardware.mounting_interface,
                );
                let valid = draft.identity.validate().is_ok()
                    && !self.library.hardware.mounting_interface.trim().is_empty();
                if ui
                    .add_enabled(valid, egui::Button::new("Use declared settings"))
                    .clicked()
                    && let Some(template) = c
                        .setups
                        .get(self.library.hardware.setup)
                        .filter(|s| s.machine.is_some())
                {
                    let mut setup = template.clone();
                    setup.id = format!("custom.{}", uuid::Uuid::new_v4());
                    setup.label = format!("Declared {} setup", draft.identity.model);
                    setup.model = HardwareModelDefinition {
                        id: setup.id.clone(),
                        vendor: draft.identity.manufacturer.clone(),
                        model: draft.identity.model.clone(),
                        source: "Operator-declared software settings; no physical qualification"
                            .into(),
                        mounting_interface: self.library.hardware.mounting_interface.trim().into(),
                        declared_volume_mm: None,
                    };
                    if setup.machine.is_some() && setup.rederive().is_ok() {
                        draft.configuration = MachineConfiguration::Configured {
                            setup: Box::new(setup),
                        };
                    }
                }
            });
        }
    }
}

/// Edit sourced machine facts; an edited value becomes operator-declared.
fn machine_fact_fields(ui: &mut egui::Ui, machine: &mut MachineDescription) {
    ui.weak(format!(
        "{:?} firmware · {:?} · {:?} levelling · from {}",
        machine.firmware,
        machine.kinematics,
        machine.levelling,
        machine.lineage.join(" → ")
    ));
    egui::ComboBox::from_label("Bed levelling")
        .selected_text(format!("{:?}", machine.levelling))
        .show_ui(ui, |ui| {
            for (value, label) in [
                (BedLevelling::Manual, "Manual · no firmware mesh"),
                (BedLevelling::ProbeMesh, "Probe a mesh every print"),
                (BedLevelling::StoredMesh, "Restore the stored mesh"),
            ] {
                ui.selectable_value(&mut machine.levelling, value, label);
            }
        });
    ui.collapsing("Start and end G-code blocks", |ui| {
        ui.weak("Operator or printer macros are emitted verbatim. Declare what each block does; the verifier checks the program around it and reports the block, without interpreting it. Placeholders: {bed_temperature_c}, {nozzle_temperature_c}.");
        if machine.firmware == FirmwareFamily::Klipper
            && ui.button("Use Klipper START_PRINT / END_PRINT macros").clicked()
        {
            machine.start_block = Some(LifecycleBlock::klipper_start_print());
            machine.end_block = Some(LifecycleBlock::klipper_end_print());
        }
        block_editor(ui, "Start block", &mut machine.start_block, true);
        block_editor(ui, "End block", &mut machine.end_block, false);
    });
    egui::Grid::new("machine-facts")
        .num_columns(2)
        .show(ui, |ui| {
            for key in MachineLimit::ALL {
                let mut value = machine.limit(key);
                number(ui, key.label(), &mut value);
                machine.declare(key, value);
                ui.weak(match machine.source(key) {
                    Some(FactSource::Published { reference }) => format!("published · {reference}"),
                    Some(FactSource::FamilyDefault { family }) => format!("default · {family}"),
                    Some(FactSource::OperatorDeclared) | None => "declared by you".into(),
                });
                ui.end_row();
            }
        });
}

use spiling_contracts::manufacturing::{
    BlockEffects, ExtrusionEffect, LevellingEffect, LifecycleBlock, ModeEffect, PositionEffect,
    ThermalEffect,
};

/// Edit one optional opaque block and its declared effects.
fn block_editor(ui: &mut egui::Ui, title: &str, block: &mut Option<LifecycleBlock>, start: bool) {
    ui.push_id(title, |ui| {
        let mut enabled = block.is_some();
        ui.checkbox(&mut enabled, title);
        if !enabled {
            *block = None;
            return;
        }
        let b = block.get_or_insert_with(|| LifecycleBlock {
            label: title.into(),
            lines: vec!["M117 Printing".into()],
            effects: BlockEffects::NONE,
        });
        field(ui, "Label", &mut b.label);
        let mut text = b.lines.join("\n");
        ui.add(
            egui::TextEdit::multiline(&mut text)
                .code_editor()
                .desired_rows(3),
        );
        b.lines = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(Into::into)
            .collect();
        let e = &mut b.effects;
        if start {
            ui.checkbox(&mut e.homes, "Homes all axes");
        }
        let mut lost = e.position == PositionEffect::Unknown;
        ui.checkbox(
            &mut lost,
            "Moves the toolhead (position unknown afterwards)",
        );
        e.position = if lost || e.homes || e.motors_off {
            PositionEffect::Unknown
        } else {
            PositionEffect::Preserved
        };
        let mut modes = e.modes == ModeEffect::Unknown;
        ui.checkbox(&mut modes, "May change units/positioning modes");
        e.modes = if modes {
            ModeEffect::Unknown
        } else {
            ModeEffect::Preserved
        };
        egui::ComboBox::from_label("Extrusion axis afterwards")
            .selected_text(format!("{:?}", e.extrusion))
            .show_ui(ui, |ui| {
                for v in [
                    ExtrusionEffect::Preserved,
                    ExtrusionEffect::Reset,
                    ExtrusionEffect::Unknown,
                ] {
                    ui.selectable_value(&mut e.extrusion, v, format!("{v:?}"));
                }
            });
        egui::ComboBox::from_label("Bed levelling afterwards")
            .selected_text(format!("{:?}", e.levelling))
            .show_ui(ui, |ui| {
                for v in [
                    LevellingEffect::Unchanged,
                    LevellingEffect::Active,
                    LevellingEffect::Off,
                ] {
                    ui.selectable_value(&mut e.levelling, v, format!("{v:?}"));
                }
            });
        for (label, value) in [("Bed heater", &mut e.bed), ("Nozzle heater", &mut e.nozzle)] {
            egui::ComboBox::from_label(label)
                .selected_text(format!("{value:?}"))
                .show_ui(ui, |ui| {
                    for v in [
                        ThermalEffect::Unchanged,
                        ThermalEffect::Target,
                        ThermalEffect::Wait,
                        ThermalEffect::Off,
                    ] {
                        ui.selectable_value(value, v, format!("{v:?}"));
                    }
                });
        }
        if !start {
            ui.checkbox(&mut e.fan_off, "Turns the fan off");
            ui.checkbox(&mut e.motors_off, "Releases the motors");
        }
        // Dialect checks (line length, effects) surface through re-derivation.
    });
}
