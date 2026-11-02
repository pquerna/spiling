// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Shared-library hardware views. Drafts remain transient until an explicit native action.
use super::*;
use crate::host::hardware::HardwareAction;
use spiling_contracts::hardware::*;
#[derive(Default)]
pub(super) struct HardwareUi {
    pub active: bool,
    tab: u8,
    query: String,
    names: [String; 3],
    submitted_add: Option<(usize, String)>,
    machine_detail: bool,
    plate_detail: bool,
    connections: bool,
    setup: usize,
    plate_model: usize,
    machine: Option<OwnedMachine>,
    plate: Option<OwnedPlate>,
    accessory: Option<OwnedAccessory>,
    chosen_machine: String,
    chosen_plate: String,
    face: String,
    endpoint: String,
    identity: String,
    route_family: usize,
    secret: zeroize::Zeroizing<String>,
    credential_route: Option<String>,
}
impl HardwareUi {
    pub(super) fn has_changes(&self, state: &State) -> bool {
        let current = state.hardware.personal.as_ref().map(|p| &p.hardware);
        self.machine
            .as_ref()
            .is_some_and(|d| current.is_none_or(|h| !h.machines.contains(d)))
            || self
                .plate
                .as_ref()
                .is_some_and(|d| current.is_none_or(|h| !h.plates.contains(d)))
            || self
                .accessory
                .as_ref()
                .is_some_and(|d| current.is_none_or(|h| !h.accessories.contains(d)))
            || self.names.iter().any(|n| !n.is_empty())
            || !self.endpoint.is_empty()
            || !self.identity.is_empty()
            || !self.secret.is_empty()
    }
    pub(super) fn confirmed_add(&mut self) {
        if let Some((index, value)) = self.submitted_add.take()
            && self.names[index] == value
        {
            self.names[index].clear();
        }
    }

    pub(super) fn clear_secret(&mut self) {
        self.secret = zeroize::Zeroizing::default();
        self.credential_route = None;
    }
}
fn field(ui: &mut egui::Ui, label: &str, value: &mut String) {
    ui::text(ui, label, value);
}
fn number(ui: &mut egui::Ui, label: &str, value: &mut f64) {
    ui::number(ui, label, value, 0.1);
}
fn machine_picker(
    ui: &mut egui::Ui,
    id: &str,
    value: &mut Option<String>,
    hardware: &HardwareLibrary,
) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(
            value
                .as_ref()
                .and_then(|id| hardware.machines.iter().find(|m| &m.id == id))
                .map_or("Not installed", |m| m.nickname.as_str()),
        )
        .show_ui(ui, |ui| {
            ui.selectable_value(value, None, "Not installed");
            for m in &hardware.machines {
                ui.selectable_value(value, Some(m.id.clone()), &m.nickname);
            }
        });
}
impl Workbench {
    fn queue_hardware(&mut self, action: HardwareAction) {
        self.library.queued = Some(LibraryAction::Hardware(action));
    }
    fn save_hardware(&mut self, personal: &LibraryHardware, hardware: HardwareLibrary) {
        if let Err(e) = hardware.validate() {
            self.local_problem = Some(e);
            return;
        }
        if hardware.machines.len() > personal.hardware.machines.len() {
            self.library.hardware.submitted_add = Some((0, self.library.hardware.names[0].clone()));
        } else if hardware.plates.len() > personal.hardware.plates.len() {
            self.library.hardware.submitted_add = Some((1, self.library.hardware.names[1].clone()));
        }
        let request = LibraryWrite {
            library_id: personal.identity.as_ref().map(|i| i.id.clone()),
            base_revision: personal.identity.as_ref().map_or(0, |i| i.revision),
            request_id: uuid::Uuid::new_v4().to_string(),
            mutation: LibraryMutation::ReplaceHardware { hardware },
        };
        self.library.pending = Some(request.clone());
        self.library.queued = Some(LibraryAction::Write(request));
    }
    pub(super) fn hardware_content(
        &mut self,
        ui: &mut egui::Ui,
        state: &State,
        ready: bool,
        writable: bool,
    ) {
        ui.set_max_width(850.0);
        let personal = state.hardware.personal.clone();
        let catalog = state.hardware.catalog.clone();
        if !self.library.personal {
            self.project_hardware(ui, state, ready, writable);
            return;
        }
        ui.horizontal(|ui| {
            for (index, name) in ["Machines", "Build plates", "Accessories", "Catalog"]
                .iter()
                .enumerate()
            {
                ui.selectable_value(&mut self.library.hardware.tab, index as u8, *name);
            }
        });
        if let Some(error) = &state.hardware.error {
            ui.label(format!("Local hardware unavailable: {error}"));
        }
        let Some(personal) = personal else {
            ui.label("Load My Hardware to see machines, plates and accessories.");
            if ui
                .add_enabled(ready, egui::Button::new("Load My Hardware"))
                .clicked()
            {
                self.queue_hardware(HardwareAction::Refresh);
            }
            return;
        };
        let can_write =
            ready && self.library.pending.is_none() && state.hardware.credential_pending.is_none();
        if let Some(request) = &state.hardware.credential_pending {
            ui.label("A credential reference save is unresolved.");
            ui.weak(format!("Request {}", request.request.request_id));
            if ui
                .add_enabled(ready, egui::Button::new("Resolve credential save"))
                .clicked()
            {
                self.queue_hardware(HardwareAction::ResolveCredential);
            }
        }
        ui.add_enabled_ui(can_write, |ui| match self.library.hardware.tab {
            0 => self.hardware_machines(ui, &personal, catalog.as_ref(), can_write),
            1 => self.hardware_plates(ui, &personal, catalog.as_ref(), can_write),
            2 => self.hardware_accessories(ui, &personal, can_write),
            _ => {
                field(ui, "Search catalog", &mut self.library.hardware.query);
                if let Some(c) = catalog {
                    for m in c.models.iter().filter(|m| {
                        format!("{} {}", m.vendor, m.model)
                            .to_lowercase()
                            .contains(&self.library.hardware.query.to_lowercase())
                    }) {
                        ui.group(|ui| {
                            ui.strong(format!("{} {}", m.vendor, m.model));
                            ui.label(&m.source);
                            if let Some(v) = m.declared_volume_mm {
                                ui.label(format!(
                                    "Declared volume: {:.0} × {:.0} × {:.0} mm",
                                    v.max[0] - v.min[0],
                                    v.max[1] - v.min[1],
                                    v.max[2] - v.min[2]
                                ));
                            }
                            for s in c.setups.iter().filter(|s| s.model.id == m.id) {
                                ui.label(format!(
                                    "{} · {} mm nozzle",
                                    s.label, s.printer.nozzle_diameter_mm
                                ));
                            }
                            if !c.setups.iter().any(|s| s.model.id == m.id) {
                                ui.weak("Catalog identity only · no admitted setup in this build");
                            }
                        });
                    }
                }
            }
        });
    }
    fn hardware_machines(
        &mut self,
        ui: &mut egui::Ui,
        p: &LibraryHardware,
        catalog: Option<&HardwareCatalog>,
        ready: bool,
    ) {
        if !self.library.hardware.machine_detail {
            if self.library.hardware.machine.is_some() {
                ui.label(
                    "Machine draft retained. Resume or discard it before choosing another machine.",
                );
                if ui.button("Resume machine").clicked() {
                    self.library.hardware.machine_detail = true;
                }
            }
            ui.weak("Machine names, locations, serials and connections stay on this computer.");
            for m in &p.hardware.machines {
                ui.push_id(&m.id, |ui| {
                    if ui
                        .add_enabled(
                            !self.library.hardware.machine.as_ref().is_some_and(|d| {
                                p.hardware
                                    .machines
                                    .iter()
                                    .find(|m| m.id == d.id)
                                    .is_none_or(|m| m != d)
                            }) && self.library.hardware.endpoint.is_empty()
                                && self.library.hardware.identity.is_empty(),
                            egui::Button::selectable(
                                self.library
                                    .hardware
                                    .machine
                                    .as_ref()
                                    .is_some_and(|d| d.id == m.id),
                                format!(
                                    "{} · {} · {} mm",
                                    m.nickname,
                                    m.setup.model.model,
                                    m.setup.printer.nozzle_diameter_mm
                                ),
                            ),
                        )
                        .clicked()
                    {
                        self.library.hardware.secret = zeroize::Zeroizing::default();
                        self.library.hardware.credential_route = None;
                        self.library.hardware.machine = Some(m.clone());
                        self.library.hardware.machine_detail = true;
                    }
                });
            }
            egui::CollapsingHeader::new("Add machine")
                .default_open(p.hardware.machines.is_empty())
                .show(ui, |ui| {
                    field(ui, "Machine name", &mut self.library.hardware.names[0]);
                    if let Some(c) = catalog {
                        egui::ComboBox::from_id_salt("hardware-add-setup")
                            .selected_text(
                                c.setups
                                    .get(self.library.hardware.setup)
                                    .map_or("Current project setup", |s| s.label.as_str()),
                            )
                            .show_ui(ui, |ui| {
                                for (i, s) in c.setups.iter().enumerate() {
                                    ui.selectable_value(
                                        &mut self.library.hardware.setup,
                                        i,
                                        &s.label,
                                    );
                                }
                                if self.state.manufacturing.intent.is_some() {
                                    ui.selectable_value(
                                        &mut self.library.hardware.setup,
                                        c.setups.len(),
                                        "Current project setup (custom copy)",
                                    );
                                }
                            });
                        if ui::primary(
                            ui,
                            "Add machine",
                            if !ready {
                                Some("Wait for pending library operations")
                            } else if self.library.hardware.names[0].trim().is_empty() {
                                Some("Enter a name")
                            } else {
                                None
                            },
                        ) {
                            let setup =
                                c.setups
                                    .get(self.library.hardware.setup)
                                    .cloned()
                                    .or_else(|| {
                                        self.state.manufacturing.intent.as_ref().map(|intent| {
                                            let model = HardwareModelDefinition {
                                                id: format!("custom.{}", uuid::Uuid::new_v4()),
                                                vendor: "Custom".into(),
                                                model: "Custom Cartesian machine".into(),
                                                source: "Operator-declared public setup".into(),
                                                mounting_interface: "custom.cartesian".into(),
                                                declared_volume_mm: Some(
                                                    intent.printer.build_envelope,
                                                ),
                                            };
                                            HardwareSetupDefinition {
                                                id: format!("custom.{}", uuid::Uuid::new_v4()),
                                                label: "Custom project setup".into(),
                                                model,
                                                printer: intent.printer.clone(),
                                            }
                                        })
                                    });
                            if let Some(setup) = setup {
                                let mut h = p.hardware.clone();
                                h.machines.push(OwnedMachine {
                                    id: uuid::Uuid::new_v4().to_string(),
                                    nickname: self.library.hardware.names[0].trim().into(),
                                    serial: None,
                                    location: None,
                                    setup,
                                    generation: 1,
                                });
                                self.save_hardware(p, h);
                            }
                        }
                    }
                });
            return;
        }
        if ui::close(ui, "Back to machines") {
            self.library.hardware.machine_detail = false;
            self.library.hardware.clear_secret();
        }
        if let Some(mut draft) = self.library.hardware.machine.clone() {
            let Some(original) = p.hardware.machines.iter().find(|m| m.id == draft.id) else {
                return;
            };
            if self.library.hardware.connections {
                if ui::close(ui, "Back to machine") {
                    self.library.hardware.connections = false;
                    self.library.hardware.clear_secret();
                }
                self.hardware_connections(ui, p, &draft.id, ready);
                return;
            }
            ui::pane_header(ui, "Machine", "My library · private inventory");
            ui.push_id(draft.id.clone(), |ui| {
                if draft != *original { ui.label("Unapplied machine changes"); }
                if ui.button("Discard machine draft").clicked() { draft = original.clone(); }
                field(ui, "Name", &mut draft.nickname);
                let mut serial = draft.serial.clone().unwrap_or_default();
                let mut location = draft.location.clone().unwrap_or_default();
                field(ui, "Serial (private)", &mut serial);
                field(ui, "Location (private)", &mut location);
                draft.serial = (!serial.is_empty()).then_some(serial);
                draft.location = (!location.is_empty()).then_some(location);
                ui.collapsing("Installed setup", |ui| {
                    ui.label(format!("{} · {}", draft.setup.model.vendor, draft.setup.model.model));
                    ui.weak("Changes create a custom setup. Existing projects retain their saved setup until Apply.");
                    number(ui, "Nozzle diameter (mm)", &mut draft.setup.printer.nozzle_diameter_mm);
                    for (axis, label) in ["Maximum X (mm)", "Maximum Y (mm)", "Maximum Z (mm)"].iter().enumerate() {
                        number(ui, label, &mut draft.setup.printer.build_envelope.max[axis]);
                    }
                    ui.label(format!("Mount: {}", draft.setup.model.mounting_interface));
                });
                if draft != *original && ui::primary(ui, "Save machine", (!ready).then_some("Wait for the current library operation")) {
                    if draft.setup != original.setup {
                        let Some(generation) = original.generation.checked_add(1) else {
                            self.local_problem = Some("Machine installation generation exhausted".into());
                            return;
                        };
                        draft.generation = generation;
                        draft.setup.id = format!("custom.{}", uuid::Uuid::new_v4());
                        draft.setup.label = format!("Custom {} setup", draft.setup.model.model);
                        draft.setup.printer.id = draft.setup.id.clone();
                        if let Err(reason) = draft.setup.validate() {
                            self.local_problem = Some(reason);
                            return;
                        }
                        let bytes = serde_json::to_vec(&draft.setup.printer).expect("validated finite printer");
                        draft.setup.printer.revision = spiling_contracts::geometry::SourceHash::from_bytes(&bytes).as_str().into();
                    }
                    let mut h = p.hardware.clone();
                    *h.machines.iter_mut().find(|m| m.id == draft.id).expect("retained machine") = draft.clone();
                    self.save_hardware(p, h);
                }
                let has_credentials = p.hardware.routes.iter().any(|r| r.machine_id == draft.id && r.credential.is_some());
                if has_credentials { ui.weak("Forget connection credentials before removing this machine."); }
                if ui.add_enabled(ready && !has_credentials, egui::Button::new("Remove machine")).clicked() {
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
                    self.library.hardware.machine = None;
                }
                if ui.button("Connections…").clicked() { self.library.hardware.connections = true; }
            });
            if self.library.hardware.machine.is_some() {
                self.library.hardware.machine = Some(draft);
            }
        }
    }
    fn hardware_plates(
        &mut self,
        ui: &mut egui::Ui,
        p: &LibraryHardware,
        catalog: Option<&HardwareCatalog>,
        ready: bool,
    ) {
        if !self.library.hardware.plate_detail {
            if self.library.hardware.plate.is_some() {
                ui.label(
                    "Plate draft retained. Resume or discard it before choosing another plate.",
                );
                if ui.button("Resume plate").clicked() {
                    self.library.hardware.plate_detail = true;
                }
            }
            ui.weak(
                "One sheet is one inventory item; its faces can have different planning limits.",
            );
            for plate in &p.hardware.plates {
                if ui
                    .add_enabled(
                        !self.library.hardware.plate.as_ref().is_some_and(|d| {
                            p.hardware
                                .plates
                                .iter()
                                .find(|p| p.id == d.id)
                                .is_none_or(|p| p != d)
                        }),
                        egui::Button::selectable(
                            self.library
                                .hardware
                                .plate
                                .as_ref()
                                .is_some_and(|d| d.id == plate.id),
                            format!("{} · {}", plate.nickname, plate.definition.label),
                        ),
                    )
                    .clicked()
                {
                    self.library.hardware.plate = Some(plate.clone());
                    self.library.hardware.plate_detail = true;
                }
            }
            egui::CollapsingHeader::new("Add build plate")
                .default_open(p.hardware.plates.is_empty())
                .show(ui, |ui| {
                    field(ui, "Plate name", &mut self.library.hardware.names[1]);
                    if let Some(c) = catalog {
                        egui::ComboBox::from_id_salt("hardware-add-plate")
                            .selected_text(
                                c.plates
                                    .get(self.library.hardware.plate_model)
                                    .map_or("Choose plate", |p| p.label.as_str()),
                            )
                            .show_ui(ui, |ui| {
                                for (i, p) in c.plates.iter().enumerate() {
                                    ui.selectable_value(
                                        &mut self.library.hardware.plate_model,
                                        i,
                                        &p.label,
                                    );
                                }
                            });
                        if ui::primary(
                            ui,
                            "Add plate",
                            if !ready {
                                Some("Wait for pending library operations")
                            } else if self.library.hardware.names[1].trim().is_empty() {
                                Some("Enter a name")
                            } else {
                                None
                            },
                        ) && let Some(definition) =
                            c.plates.get(self.library.hardware.plate_model)
                        {
                            let mut h = p.hardware.clone();
                            h.plates.push(OwnedPlate {
                                id: uuid::Uuid::new_v4().to_string(),
                                nickname: self.library.hardware.names[1].trim().into(),
                                definition: definition.clone(),
                                installed_machine: None,
                                notes: String::new(),
                            });
                            self.save_hardware(p, h);
                        }
                    }
                });
            return;
        }
        if ui::close(ui, "Back to plates") {
            self.library.hardware.plate_detail = false;
        }
        if let Some(mut draft) = self.library.hardware.plate.clone() {
            ui.separator();
            ui.push_id(draft.id.clone(), |ui| {
                ui::pane_header(ui, "Build plate", "My library · one physical sheet");
                if ui.button("Discard plate draft").clicked()
                    && let Some(original) = p.hardware.plates.iter().find(|p| p.id == draft.id)
                {
                    draft = original.clone();
                }
                field(ui, "Name", &mut draft.nickname);
                field(ui, "Notes (private)", &mut draft.notes);
                machine_picker(
                    ui,
                    "plate-installed",
                    &mut draft.installed_machine,
                    &p.hardware,
                );
                ui.collapsing("Plate definition and faces", |ui| {
                    field(ui, "Portable plate label", &mut draft.definition.label);
                    field(
                        ui,
                        "Mount interface",
                        &mut draft.definition.mounting_interface,
                    );
                    number(ui, "Thickness (mm)", &mut draft.definition.thickness_mm);
                    number(
                        ui,
                        "Installed top surface Z (mm)",
                        &mut draft.definition.surface_z_mm,
                    );
                    for face in &mut draft.definition.faces {
                        ui.push_id(&face.key, |ui| {
                            ui.strong(&face.label);
                            number(
                                ui,
                                "Maximum bed temperature (°C)",
                                &mut face.max_bed_celsius,
                            );
                            for (axis, name) in ["X", "Y"].iter().enumerate() {
                                number(
                                    ui,
                                    &format!("Usable {name} minimum (mm)"),
                                    &mut face.usable_mm.min[axis],
                                );
                                number(
                                    ui,
                                    &format!("Usable {name} maximum (mm)"),
                                    &mut face.usable_mm.max[axis],
                                );
                            }
                            for (i, b) in face.keepouts_mm.iter_mut().enumerate() {
                                ui.push_id(i, |ui| {
                                    ui.label(format!("Keepout {}", i + 1));
                                    for axis in 0..3 {
                                        number(
                                            ui,
                                            &format!("Axis {axis} minimum"),
                                            &mut b.min[axis],
                                        );
                                        number(
                                            ui,
                                            &format!("Axis {axis} maximum"),
                                            &mut b.max[axis],
                                        );
                                    }
                                });
                            }
                            if ui.button("Add rectangular keepout").clicked() {
                                face.keepouts_mm.push(AabbMm {
                                    min: [100., 100., 0.],
                                    max: [110., 110., 10.],
                                });
                            }
                            if !face.keepouts_mm.is_empty()
                                && ui.button("Remove last keepout").clicked()
                            {
                                face.keepouts_mm.pop();
                            }
                        });
                    }
                });
                if ui::primary(
                    ui,
                    "Save plate",
                    (!ready).then_some("Wait for the current library operation"),
                ) {
                    let mut h = p.hardware.clone();
                    if let Some(old) = h.plates.iter_mut().find(|v| v.id == draft.id) {
                        if old.definition != draft.definition {
                            draft.definition.id = format!("custom.{}", uuid::Uuid::new_v4());
                            draft.definition.source =
                                "Operator-declared plate based on a retained model".into();
                        }
                        *old = draft.clone();
                    }
                    self.save_hardware(p, h);
                }
                if ui
                    .add_enabled(ready, egui::Button::new("Remove plate"))
                    .clicked()
                {
                    let mut h = p.hardware.clone();
                    h.plates.retain(|v| v.id != draft.id);
                    h.bindings.retain(|b| b.plate_id != draft.id);
                    self.save_hardware(p, h);
                    self.library.hardware.plate = None;
                }
            });
            if self.library.hardware.plate.is_some() {
                self.library.hardware.plate = Some(draft);
            }
        }
    }
    fn hardware_accessories(&mut self, ui: &mut egui::Ui, p: &LibraryHardware, ready: bool) {
        let editing = self.library.hardware.accessory.is_some();
        if editing && ui.button("Discard accessory draft").clicked() {
            self.library.hardware.accessory = None;
        }
        ui.add_enabled_ui(!editing, |ui| {
            for a in &p.hardware.accessories {
                if ui
                    .selectable_label(
                        self.library
                            .hardware
                            .accessory
                            .as_ref()
                            .is_some_and(|d| d.id == a.id),
                        &a.nickname,
                    )
                    .clicked()
                {
                    self.library.hardware.accessory = Some(a.clone());
                }
            }
            ui.horizontal(|ui| {
                if ui.button("Add nozzle").clicked() {
                    self.library.hardware.accessory = Some(OwnedAccessory {
                        id: uuid::Uuid::new_v4().to_string(),
                        nickname: "Spare nozzle".into(),
                        definition: AccessoryDefinition {
                            id: format!("custom.{}", uuid::Uuid::new_v4()),
                            label: "0.4 mm nozzle".into(),
                            mounting_interface: "spiling.reference.220".into(),
                            effect: AccessoryEffect::Nozzle {
                                diameter_mm: 0.4,
                                material: "Hardened steel".into(),
                            },
                        },
                        installed_machine: None,
                    });
                }
                if ui.button("Add fixture").clicked() {
                    self.library.hardware.accessory = Some(OwnedAccessory {
                        id: uuid::Uuid::new_v4().to_string(),
                        nickname: "Bed fixture".into(),
                        definition: AccessoryDefinition {
                            id: format!("custom.{}", uuid::Uuid::new_v4()),
                            label: "Rectangular fixture".into(),
                            mounting_interface: "spiling.reference.220".into(),
                            effect: AccessoryEffect::Fixture {
                                bounds_mm: AabbMm {
                                    min: [100., 100., 0.],
                                    max: [110., 110., 10.],
                                },
                            },
                        },
                        installed_machine: None,
                    });
                }
            });
        });
        if let Some(mut d) = self.library.hardware.accessory.clone() {
            ui.separator();
            ui.push_id(d.id.clone(), |ui| {
                field(ui, "Accessory name (private)", &mut d.nickname);
                field(ui, "Portable label", &mut d.definition.label);
                field(ui, "Mount interface", &mut d.definition.mounting_interface);
                machine_picker(
                    ui,
                    "accessory-machine",
                    &mut d.installed_machine,
                    &p.hardware,
                );
                match &mut d.definition.effect {
                    AccessoryEffect::Nozzle {
                        diameter_mm,
                        material,
                    } => {
                        number(ui, "Diameter (mm)", diameter_mm);
                        field(ui, "Nozzle material", material);
                    }
                    AccessoryEffect::Fixture { bounds_mm } => {
                        for axis in 0..3 {
                            number(
                                ui,
                                &format!("Axis {axis} minimum (mm)"),
                                &mut bounds_mm.min[axis],
                            );
                            number(
                                ui,
                                &format!("Axis {axis} maximum (mm)"),
                                &mut bounds_mm.max[axis],
                            );
                        }
                    }
                }
                if ui::primary(
                    ui,
                    "Save accessory",
                    (!ready).then_some("Wait for pending library operations"),
                ) {
                    let mut h = p.hardware.clone();
                    if let Some(old) = h.accessories.iter_mut().find(|a| a.id == d.id) {
                        if old.definition != d.definition {
                            d.definition.id = format!("custom.{}", uuid::Uuid::new_v4());
                        }
                        *old = d.clone();
                    } else {
                        h.accessories.push(d.clone());
                    }
                    self.save_hardware(p, h);
                }
                if ui
                    .add_enabled(ready, egui::Button::new("Remove accessory"))
                    .clicked()
                {
                    let mut h = p.hardware.clone();
                    h.accessories.retain(|a| a.id != d.id);
                    self.save_hardware(p, h);
                    self.library.hardware.accessory = None;
                }
            });
            if self.library.hardware.accessory.is_some() {
                self.library.hardware.accessory = Some(d);
            }
        }
    }
    fn hardware_connections(
        &mut self,
        ui: &mut egui::Ui,
        p: &LibraryHardware,
        machine: &str,
        ready: bool,
    ) {
        ui::pane_header(
            ui,
            "Connections",
            "Private configuration · no machine communication",
        );
        ui.vertical(|ui| {
            ui.weak("Routes stay on this installation. Selecting a route never sends a program.");
            for route in p.hardware.routes.iter().filter(|r| r.machine_id == machine) {
                ui.push_id(&route.id, |ui| {
                    ui.label(format!("{} · {}", connection_label(route.family), route.endpoint));
                    ui.weak(format!("Expected identity: {}", route.expected_identity));
                    if route.family == ConnectionFamily::File {
                        ui.label("Use Export in Prepare to save the freshly verified program and reports. This records a local save only.");
                    } else {
                        ui.label("Configuration only: this protocol has no supported probe, upload or start implementation in this build.");
                    }
                    if let Some(c) = &route.credential {
                        ui.label(match c.backend {
                            CredentialBackend::Keyring => "Credential in system keyring",
                            CredentialBackend::PlaintextFile => "Credential in local plaintext file",
                        });
                        if ui.add_enabled(ready, egui::Button::new("Forget credential")).clicked()
                            && let Some(expected) = &p.identity
                        {
                            self.queue_hardware(HardwareAction::ForgetCredential {
                                route: route.id.clone(), expected: expected.clone(),
                            });
                        }
                    }
                    if route.family != ConnectionFamily::File && ui.add_enabled(ready, egui::Button::new("Set / rotate credential")).clicked() {
                        self.library.hardware.secret = zeroize::Zeroizing::default();
                        self.library.hardware.credential_route = Some(route.id.clone());
                    }
                    if self.library.hardware.credential_route.as_deref() == Some(route.id.as_str()) {
                        let l = ui.label("Credential");
                        ui.add(egui::TextEdit::singleline(&mut *self.library.hardware.secret).password(true).char_limit(8192))
                            .labelled_by(l.id);
                        ui.weak("On headless or unsupported systems, new credentials use a private plaintext file. An inaccessible existing keyring entry never falls back.");
                        if ui::primary(ui, "Remember credential", (!ready || self.library.hardware.secret.is_empty()).then_some("Enter a credential and wait for any pending save"))
                            && let Some(expected) = &p.identity
                        {
                            let secret = spiling_engine_client::hardware::credentials::CredentialInput::new(std::mem::take(&mut *self.library.hardware.secret));
                            self.library.hardware.credential_route = None;
                            self.queue_hardware(HardwareAction::RememberCredential {
                                route: route.id.clone(), expected: expected.clone(),
                                role: if route.family == ConnectionFamily::BambuDeveloperLan { CredentialRole::LanAccessCode } else { CredentialRole::ApiKey },
                                secret,
                            });
                        }
                    }
                    if ui.add_enabled(ready && route.credential.is_none(), egui::Button::new("Remove connection")).clicked() {
                        let mut h = p.hardware.clone();
                        h.routes.retain(|r| r.id != route.id);
                        self.save_hardware(p, h);
                    }
                });
            }
            ui.separator();
            if ui.button("Discard connection draft").clicked() { self.library.hardware.endpoint.clear(); self.library.hardware.identity.clear(); self.library.hardware.clear_secret(); }
            let families = [ConnectionFamily::File, ConnectionFamily::BambuConnect,
                ConnectionFamily::BambuDeveloperLan, ConnectionFamily::Moonraker,
                ConnectionFamily::OctoPrint, ConnectionFamily::PrusaLink];
            self.library.hardware.route_family = self.library.hardware.route_family.min(families.len() - 1);
            egui::ComboBox::from_id_salt("connection-family")
                .selected_text(connection_label(families[self.library.hardware.route_family]))
                .show_ui(ui, |ui| {
                    for (i, family) in families.iter().enumerate() {
                        ui.selectable_value(&mut self.library.hardware.route_family, i, connection_label(*family));
                    }
                });
            let family = families[self.library.hardware.route_family];
            if family != ConnectionFamily::File {
                ui.label("Save a future connection configuration. This build cannot communicate using this protocol.");
                field(ui, "Endpoint (without credentials)", &mut self.library.hardware.endpoint);
                field(ui, "Expected device identity", &mut self.library.hardware.identity);
            }
            if self.library.hardware.credential_route.is_none() && ui::primary(ui, "Save connection", (!ready).then_some("Wait for the current operation")) {
                let mut h = p.hardware.clone();
                h.routes.push(EndpointRoute {
                    id: uuid::Uuid::new_v4().to_string(), machine_id: machine.into(), family,
                    endpoint: if family == ConnectionFamily::File { "local-file".into() } else { self.library.hardware.endpoint.trim().into() },
                    expected_identity: if family == ConnectionFamily::File { machine.into() } else { self.library.hardware.identity.trim().into() },
                    generation: 1, credential: None,
                });
                self.save_hardware(p, h);
            }
        });
    }
    fn project_hardware(&mut self, ui: &mut egui::Ui, state: &State, ready: bool, writable: bool) {
        let Some(project) = self.state.project.clone() else {
            ui.label("Open a project to capture its hardware setup.");
            return;
        };
        ui.strong("Project hardware");
        if let Some(c) = &project.hardware {
            ui.label(format!(
                "{} {} · {}",
                c.setup.model.vendor, c.setup.model.model, c.setup.label
            ));
            ui.label(format!(
                "{} / {} · {} mm nozzle",
                c.plate.label, c.face, c.setup.printer.nozzle_diameter_mm
            ));
            ui.weak("This complete setup travels with the project. Changes in My Hardware are applied explicitly.");
            if let Some(local) = state
                .hardware
                .local_selection
                .as_ref()
                .filter(|l| l.project == project.project_id)
                && let Some(p) = &state.hardware.personal
                && let Some(m) = p
                    .hardware
                    .machines
                    .iter()
                    .find(|m| m.id == local.machine_id)
            {
                ui.label(if local.matches {
                    format!("Using {} on this computer", m.nickname)
                } else {
                    format!(
                        "Saved setup differs from {}; review before Apply",
                        m.nickname
                    )
                });
            }
        } else {
            ui.label("No project hardware capture selected.");
        }
        let Some(p) = &state.hardware.personal else {
            if ui
                .add_enabled(ready, egui::Button::new("Load local hardware"))
                .clicked()
            {
                self.queue_hardware(HardwareAction::Refresh);
            }
            return;
        };
        if let Some(capture) = &project.hardware
            && ui
                .add_enabled(
                    ready && self.library.pending.is_none(),
                    egui::Button::new("Copy captured setup and plate to My Hardware"),
                )
                .clicked()
        {
            let mut h = p.hardware.clone();
            let machine = uuid::Uuid::new_v4().to_string();
            h.machines.push(OwnedMachine {
                id: machine.clone(),
                nickname: capture.setup.model.model.clone(),
                serial: None,
                location: None,
                setup: capture.setup.clone(),
                generation: 1,
            });
            h.plates.push(OwnedPlate {
                id: uuid::Uuid::new_v4().to_string(),
                nickname: capture.plate.label.clone(),
                definition: capture.plate.clone(),
                installed_machine: Some(machine.clone()),
                notes: String::new(),
            });
            for definition in &capture.accessories {
                h.accessories.push(OwnedAccessory {
                    id: uuid::Uuid::new_v4().to_string(),
                    nickname: definition.label.clone(),
                    definition: definition.clone(),
                    installed_machine: Some(machine.clone()),
                });
            }
            self.save_hardware(p, h);
        }
        egui::ComboBox::from_id_salt("project-local-machine")
            .selected_text(
                p.hardware
                    .machines
                    .iter()
                    .find(|m| m.id == self.library.hardware.chosen_machine)
                    .map_or("Choose local machine", |m| m.nickname.as_str()),
            )
            .show_ui(ui, |ui| {
                for m in &p.hardware.machines {
                    ui.selectable_value(
                        &mut self.library.hardware.chosen_machine,
                        m.id.clone(),
                        &m.nickname,
                    );
                }
            });
        egui::ComboBox::from_id_salt("project-local-plate")
            .selected_text(
                p.hardware
                    .plates
                    .iter()
                    .find(|p| p.id == self.library.hardware.chosen_plate)
                    .map_or("Choose build plate", |p| p.nickname.as_str()),
            )
            .show_ui(ui, |ui| {
                for plate in &p.hardware.plates {
                    ui.selectable_value(
                        &mut self.library.hardware.chosen_plate,
                        plate.id.clone(),
                        &plate.nickname,
                    );
                }
            });
        if let Some(plate) = p
            .hardware
            .plates
            .iter()
            .find(|p| p.id == self.library.hardware.chosen_plate)
        {
            egui::ComboBox::from_id_salt("project-plate-face")
                .selected_text(if self.library.hardware.face.is_empty() {
                    "Choose face"
                } else {
                    &self.library.hardware.face
                })
                .show_ui(ui, |ui| {
                    for face in &plate.definition.faces {
                        ui.selectable_value(
                            &mut self.library.hardware.face,
                            face.key.clone(),
                            &face.label,
                        );
                    }
                });
        }
        let candidate = (!self.library.hardware.chosen_machine.is_empty()
            && !self.library.hardware.chosen_plate.is_empty()
            && !self.library.hardware.face.is_empty())
        .then(|| {
            p.hardware.capture(
                &self.library.hardware.chosen_machine,
                &self.library.hardware.chosen_plate,
                &self.library.hardware.face,
            )
        });
        if let Some(candidate) = &candidate {
            match candidate {
                Err(reason) => {
                    ui.label(format!("Cannot use this selection: {reason}"));
                }
                Ok(c) => {
                    ui.label(format!(
                        "{} · {:.2} mm nozzle · {} / {}",
                        c.setup.label, c.setup.printer.nozzle_diameter_mm, c.plate.label, c.face
                    ));
                    let volume = c.setup.printer.build_envelope;
                    ui.weak(format!(
                        "Build bounds {:?} to {:?} mm · bed limit {} °C",
                        volume.min,
                        volume.max,
                        c.selected_face()
                            .expect("checked selection")
                            .max_bed_celsius
                    ));
                    if project.hardware.as_deref().is_some_and(|old| old != c) {
                        ui.label("Apply replaces the project's planning setup and invalidates its compiled program. Undo restores the prior setup.");
                    }
                }
            }
        }
        let reason = if !ready || !writable || self.library.pending.is_some() {
            Some("Open a writable project and wait for pending operations")
        } else if !candidate.as_ref().is_some_and(Result::is_ok) {
            Some("Choose a compatible machine, plate and face")
        } else {
            None
        };
        if ui::primary(ui, "Apply setup", reason) {
            self.queue_hardware(HardwareAction::Select {
                project: project.project_id.clone(),
                base: project.revision,
                expected: p.identity.clone(),
                machine: self.library.hardware.chosen_machine.clone(),
                plate: self.library.hardware.chosen_plate.clone(),
                face: self.library.hardware.face.clone(),
            });
        }
        if ui
            .add_enabled(
                writable && project.hardware.is_some(),
                egui::Button::new("Clear project hardware capture"),
            )
            .clicked()
            && let (Some(host), Some(hello)) = (&self.host, &self.state.hello)
        {
            match host.project(spiling_contracts::project::ProjectCommand::SetHardware {
                session_id: hello.session_id.clone(),
                base_revision: project.revision,
                capture: None,
            }) {
                Ok(()) => self.awaiting_serial = Some(self.state.serial),
                Err(e) => self.local_problem = Some(e),
            }
        }
    }
}

impl Workbench {
    pub(in crate::app) fn hardware_summary(&mut self, ui: &mut egui::Ui) {
        if let Some(p) = &self.state.project
            && let Some(c) = &p.hardware
        {
            ui.label(format!(
                "Target: {} {} · {} / {}",
                c.setup.model.vendor, c.setup.model.model, c.plate.label, c.face
            ));
        }
        if ui.button("Choose hardware").clicked() {
            self.library.open = true;
            self.library.personal = false;
            self.library.hardware.active = true;
            self.library.refresh();
        }
    }
}

fn connection_label(family: ConnectionFamily) -> &'static str {
    match family {
        ConnectionFamily::File => "File / media export",
        ConnectionFamily::BambuConnect => "Bambu Connect (not supported)",
        ConnectionFamily::BambuDeveloperLan => "Bambu developer LAN (not supported)",
        ConnectionFamily::Moonraker => "Moonraker (not supported)",
        ConnectionFamily::OctoPrint => "OctoPrint (not supported)",
        ConnectionFamily::PrusaLink => "PrusaLink (not supported)",
        ConnectionFamily::DevelopmentSimulator => "Development simulator (not production)",
    }
}
