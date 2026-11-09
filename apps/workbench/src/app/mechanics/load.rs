// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use glam::{DQuat, DVec3};
use spiling_contracts::analysis::load::AnalysisFacePreparation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction {
    Into,
    Out,
    PartX,
    PartY,
    PartZ,
    PlateX,
    PlateY,
    PlateZ,
    Custom,
}
impl Direction {
    const ALL: [Self; 9] = [
        Self::Into,
        Self::Out,
        Self::PartX,
        Self::PartY,
        Self::PartZ,
        Self::PlateX,
        Self::PlateY,
        Self::PlateZ,
        Self::Custom,
    ];
    fn label(self) -> &'static str {
        match self {
            Self::Into => "Into face",
            Self::Out => "Out of face",
            Self::PartX => "Part X",
            Self::PartY => "Part Y",
            Self::PartZ => "Part Z",
            Self::PlateX => "Plate X",
            Self::PlateY => "Plate Y",
            Self::PlateZ => "Plate Z",
            Self::Custom => "Custom vector",
        }
    }
}

pub(super) struct Control {
    scene: SceneRevision,
    last_force: [f64; 3],
    last_moment: [f64; 3],
    last_footprint: spiling_contracts::analysis::load::ForceFootprint,
    couple: [f64; 3],
    magnitude: f64,
    direction: Direction,
    reversed: bool,
    custom: [f64; 3],
    pub error: Option<String>,
}
impl Control {
    fn same_footprint(
        &self,
        footprint: &spiling_contracts::analysis::load::ForceFootprint,
    ) -> bool {
        use spiling_contracts::analysis::load::ForceFootprint;
        match (&self.last_footprint, footprint) {
            (ForceFootprint::WholeFace {}, ForceFootprint::WholeFace {}) => true,
            (
                ForceFootprint::Disk {
                    center_mm: first,
                    diameter_mm: first_diameter,
                },
                ForceFootprint::Disk {
                    center_mm: second,
                    diameter_mm: second_diameter,
                },
            ) => {
                first.map(f64::to_bits) == second.map(f64::to_bits)
                    && first_diameter.to_bits() == second_diameter.to_bits()
            }
            _ => false,
        }
    }
    pub fn new(preparation: &AnalysisFacePreparation, load: &AnalysisFaceLoad) -> Self {
        let force = DVec3::from_array(load.force_n);
        let unit = force
            .try_normalize()
            .unwrap_or(-DVec3::from_array(preparation.outward_normal));
        let normal = DVec3::from_array(preparation.outward_normal);
        let (direction, reversed) = if unit.distance(-normal) < 1e-10 {
            (Direction::Into, false)
        } else if unit.distance(normal) < 1e-10 {
            (Direction::Out, false)
        } else if let Some(axis) =
            (0..3).find(|axis| unit.distance(DVec3::AXES[*axis] * unit[*axis].signum()) < 1e-10)
        {
            (
                [Direction::PartX, Direction::PartY, Direction::PartZ][axis],
                unit[axis] < 0.0,
            )
        } else {
            (Direction::Custom, false)
        };
        let center = preparation
            .contact(&load.footprint)
            .map(|contact| contact.1)
            .unwrap_or(preparation.centroid_mm);
        let couple = (DVec3::from_array(load.moment_n_mm) - DVec3::from_array(center).cross(force))
            .to_array();
        Self {
            scene: preparation.reference.scene_revision,
            last_force: load.force_n,
            last_moment: load.moment_n_mm,
            last_footprint: load.footprint.clone(),
            couple,
            magnitude: force.length(),
            direction,
            reversed,
            custom: unit.to_array(),
            error: None,
        }
    }

    fn vector(&self, preparation: &AnalysisFacePreparation) -> Result<DVec3, String> {
        let direction = match self.direction {
            Direction::Into => -DVec3::from_array(preparation.outward_normal),
            Direction::Out => DVec3::from_array(preparation.outward_normal),
            Direction::PartX => DVec3::X,
            Direction::PartY => DVec3::Y,
            Direction::PartZ => DVec3::Z,
            Direction::PlateX | Direction::PlateY | Direction::PlateZ => {
                let axis = match self.direction {
                    Direction::PlateX => DVec3::X,
                    Direction::PlateY => DVec3::Y,
                    _ => DVec3::Z,
                };
                DQuat::from_array(preparation.pose.rotation_xyzw).conjugate() * axis
            }
            Direction::Custom => DVec3::from_array(self.custom),
        };
        let direction = direction
            .try_normalize()
            .ok_or("Enter a finite, nonzero direction")?;
        if !self.magnitude.is_finite() || self.magnitude < 0.0 {
            return Err("Force magnitude must be finite and nonnegative".into());
        }
        Ok(direction * self.magnitude * if self.reversed { -1.0 } else { 1.0 })
    }

    fn apply(&mut self, preparation: &AnalysisFacePreparation, load: &mut AnalysisFaceLoad) {
        match self.vector(preparation).and_then(|force| {
            preparation
                .load_on(load.footprint.clone(), force.to_array(), self.couple)
                .map_err(|error| error.to_string())
        }) {
            Ok(value) => {
                *load = value;
                self.last_force = load.force_n;
                self.last_moment = load.moment_n_mm;
                self.last_footprint = load.footprint.clone();
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
        self.last_footprint = load.footprint.clone();
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        preparation: &AnalysisFacePreparation,
        load: &mut AnalysisFaceLoad,
    ) -> bool {
        if self.scene != preparation.reference.scene_revision
            || self.last_force != load.force_n
            || self.last_moment != load.moment_n_mm
            || !self.same_footprint(&load.footprint)
        {
            *self = Self::new(preparation, load);
        }
        let enter = ui.input(|input| input.key_pressed(egui::Key::Enter));
        let response = ui::number(ui, "Force (N)", &mut self.magnitude, 0.1);
        let mut commit = response.lost_focus() && enter;
        let mut changed = response.changed();
        let mut whole = matches!(
            load.footprint,
            spiling_contracts::analysis::load::ForceFootprint::WholeFace {}
        );
        if ui.checkbox(&mut whole, "Whole face").changed() {
            load.footprint = if whole {
                spiling_contracts::analysis::load::ForceFootprint::WholeFace {}
            } else {
                spiling_contracts::analysis::load::ForceFootprint::Disk {
                    center_mm: preparation.centroid_mm,
                    diameter_mm: preparation.width_mm.min(preparation.height_mm).min(8.0),
                }
            };
            changed = true;
        }
        if let spiling_contracts::analysis::load::ForceFootprint::Disk { diameter_mm, .. } =
            &mut load.footprint
        {
            let response = ui::number(ui, "Contact diameter (mm)", diameter_mm, 0.1);
            changed |= response.changed();
            commit |= response.lost_focus() && enter;
        }
        ui.horizontal(|ui| {
            let previous = self.direction;
            egui::ComboBox::from_id_salt("load-direction")
                .selected_text(format!(
                    "{}{}",
                    if self.reversed { "− " } else { "" },
                    self.direction.label()
                ))
                .show_ui(ui, |ui| {
                    for direction in Direction::ALL {
                        changed |= ui
                            .selectable_value(&mut self.direction, direction, direction.label())
                            .changed();
                    }
                });
            if previous != self.direction {
                self.reversed = false;
            }
            if ui
                .button("Flip")
                .on_hover_text("Reverse force direction; preserve the additional couple")
                .clicked()
            {
                match self.direction {
                    Direction::Into => self.direction = Direction::Out,
                    Direction::Out => self.direction = Direction::Into,
                    _ => self.reversed = !self.reversed,
                }
                changed = true;
            }
        });
        if self.direction == Direction::Custom {
            for (axis, component) in self.custom.iter_mut().enumerate() {
                let response = ui::number(
                    ui,
                    ["Part X direction", "Part Y direction", "Part Z direction"][axis],
                    component,
                    0.1,
                );
                changed |= response.changed();
                commit |= response.lost_focus() && enter;
            }
        }
        if let Ok((area, _)) = preparation.contact(&load.footprint) {
            ui.weak(format!("{area:.2} mm² contact"));
        }
        ui.collapsing("Additional couple (N·mm)", |ui| {
            for axis in 0..3 {
                let response = ui::number(
                    ui,
                    ["Part X couple", "Part Y couple", "Part Z couple"][axis],
                    &mut self.couple[axis],
                    0.1,
                );
                changed |= response.changed();
                commit |= response.lost_focus() && enter;
            }
        });
        if changed {
            self.apply(preparation, load);
        }
        if let Some(error) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        commit && self.error.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn magnitude_direction_and_size_preserve_an_independent_couple() {
        let contact = crate::viewport::force::test_contact();
        let preparation = &contact.preparation;
        let mut load = preparation
            .load_on(contact.load.footprint, [0.0, 10.0, 0.0], [1.0, 2.0, 3.0])
            .unwrap();
        let mut control = Control::new(preparation, &load);
        control.magnitude = 20.0;
        control.apply(preparation, &mut load);
        assert_eq!(load.force_n, [0.0, 20.0, 0.0]);
        assert!(
            DVec3::from_array(load.moment_n_mm).distance(DVec3::new(-59.0, 2.0, 203.0)) < 1e-10
        );
        control.direction = Direction::Out;
        control.apply(preparation, &mut load);
        assert_eq!(load.force_n, [0.0, -20.0, 0.0]);
        assert!(
            DVec3::from_array(load.moment_n_mm).distance(DVec3::new(61.0, 2.0, -197.0)) < 1e-10
        );
        let previous = load.clone();
        control.magnitude = f64::NAN;
        control.apply(preparation, &mut load);
        assert_eq!(load, previous);
        assert!(control.error.is_some());
    }
    #[test]
    fn plate_direction_is_converted_once_to_the_service_frame() {
        let contact = crate::viewport::force::test_contact();
        let mut preparation = (*contact.preparation).clone();
        preparation.pose.rotation_xyzw =
            DQuat::from_rotation_z(std::f64::consts::FRAC_PI_2).to_array();
        let mut load = contact.load;
        let mut control = Control::new(&preparation, &load);
        control.direction = Direction::PlateX;
        control.apply(&preparation, &mut load);
        assert!(DVec3::from_array(load.force_n).distance(DVec3::NEG_Y * 10.0) < 1e-12);
    }

    #[test]
    fn an_invalid_contact_size_does_not_rehydrate_or_change_the_additional_couple() {
        use spiling_contracts::analysis::load::ForceFootprint;
        let contact = crate::viewport::force::test_contact();
        let preparation = &contact.preparation;
        let mut load = preparation
            .load_on(
                ForceFootprint::Disk {
                    center_mm: [0.0, 0.0, 3.0],
                    diameter_mm: 2.0,
                },
                [0.0, 10.0, 0.0],
                [1.0, 2.0, 3.0],
            )
            .unwrap();
        let mut control = Control::new(preparation, &load);
        if let ForceFootprint::Disk { diameter_mm, .. } = &mut load.footprint {
            *diameter_mm = f64::NAN;
        }
        control.apply(preparation, &mut load);
        assert!(control.error.is_some());
        assert!(control.same_footprint(&load.footprint));
        let context = egui::Context::default();
        let output = context.run_ui(egui::RawInput::default(), |ui| {
            control.show(ui, preparation, &mut load);
        });
        output.drop_without_applying_deltas();
        assert!(control.error.is_some());
        assert!(DVec3::from_array(control.couple).distance(DVec3::new(1.0, 2.0, 3.0)) < 1e-10);
        if let ForceFootprint::Disk { diameter_mm, .. } = &mut load.footprint {
            *diameter_mm = 3.0;
        }
        control.apply(preparation, &mut load);
        assert!(control.error.is_none());
        let (_, center) = preparation.contact(&load.footprint).unwrap();
        let couple = DVec3::from_array(load.moment_n_mm)
            - DVec3::from_array(center).cross(DVec3::from_array(load.force_n));
        assert!(couple.distance(DVec3::new(1.0, 2.0, 3.0)) < 1e-10);
    }
}
