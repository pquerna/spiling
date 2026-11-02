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
                        let label = format!(
                            "{} · {} {} · {state}",
                            m.nickname, m.identity.manufacturer, m.identity.model
                        );
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
        if ui::close(ui, "Back to machines") {
            self.library.hardware.machine_detail = false;
            self.library.hardware.clear_secret();
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
        ui::pane_header(
            ui,
            if original.is_some() {
                "Machine"
            } else {
                "Add printer"
            },
            "My library · private physical instance",
        );
        let mut saved = false;
        let mut discard = false;
        egui::Panel::bottom("machine-editor-actions").show(ui, |ui| {
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
                    setup.id = format!("custom.{}", uuid::Uuid::new_v4());
                    setup.label = format!("Custom {} setup", setup.model.model);
                    setup.printer.id = setup.id.clone();
                    setup.printer.revision = spiling_contracts::geometry::SourceHash::from_bytes(
                        &serde_json::to_vec(&setup.printer).expect("validated printer"),
                    )
                    .as_str()
                    .into();
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
        if discard {
            self.library.hardware.machine = None;
            self.library.hardware.machine_detail = false;
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("machine-editor-body")
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
            number(
                ui,
                "Nozzle diameter (mm)",
                &mut setup.printer.nozzle_diameter_mm,
            );
            for (axis, label) in ["Maximum X (mm)", "Maximum Y (mm)", "Maximum Z (mm)"]
                .iter()
                .enumerate()
            {
                number(ui, label, &mut setup.printer.build_envelope.max[axis]);
            }
            ui.weak(format!("Mount: {}", setup.model.mounting_interface));
            ui::help(
                ui,
                "installed-settings",
                "Installed setup",
                "These are declared process settings, not physical printer qualification. Existing projects retain their captured setup until Apply.",
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
        if draft.identity.catalog_origin.is_none() {
            ui.collapsing("Declare a custom setup", |ui| {
                ui.weak("Start with explicit software settings, then review the installed configuration.");
                egui::ComboBox::from_id_salt("custom-machine-template")
                    .selected_text(c.setups.get(self.library.hardware.setup).map_or("Choose settings", |s| s.label.as_str()))
                    .show_ui(ui, |ui| {
                        for (i,s) in c.setups.iter().enumerate() {
                            ui.selectable_value(&mut self.library.hardware.setup, i, &s.label);
                        }
                    });
                field(ui, "Mount interface", &mut self.library.hardware.mounting_interface);
                let valid = draft.identity.validate().is_ok() && !self.library.hardware.mounting_interface.trim().is_empty();
                if ui.add_enabled(valid, egui::Button::new("Use declared settings")).clicked() && let Some(template) = c.setups.get(self.library.hardware.setup) {
                    let mut setup = template.clone();
                    setup.id = format!("custom.{}", uuid::Uuid::new_v4());
                    setup.label = format!("Declared {} setup", draft.identity.model);
                    setup.model = HardwareModelDefinition {
                        id: setup.id.clone(), vendor: draft.identity.manufacturer.clone(), model: draft.identity.model.clone(),
                        source: "Operator-declared software settings; no physical qualification".into(),
                        mounting_interface: self.library.hardware.mounting_interface.trim().into(), declared_volume_mm: None,
                    };
                    setup.printer.id = setup.id.clone();
                    setup.printer.revision = spiling_contracts::geometry::SourceHash::from_bytes(&serde_json::to_vec(&setup.printer).expect("finite template")).as_str().into();
                    draft.configuration = MachineConfiguration::Configured { setup: Box::new(setup) };
                }
            });
        }
    }
}
