// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Independent borrowed stored-path semantic cursor. No realization/emitter imports.
use spiling_contracts::manufacturing::*;

#[derive(Clone, Copy)]
pub(super) enum Action {
    Fan(u8),
    Move {
        target: [f64; 3],
        source: Option<[f64; 3]>,
        layer: u16,
        role: DepositionKind,
    },
    Recovery {
        retract: bool,
        layer: u16,
        role: DepositionKind,
    },
}
pub(super) struct Expected<'a> {
    plan: &'a NormalizedPrintPlan,
    recipe: &'a PlanarPrintRecipe,
    layer: usize,
    path: usize,
    prefix: [Option<Action>; 8],
    prefix_index: usize,
    edge: usize,
    position: Option<[f64; 3]>,
    fan: Option<u8>,
    loaded: bool,
}
impl<'a> Expected<'a> {
    pub(super) fn new(
        plan: &'a NormalizedPrintPlan,
        recipe: &'a PlanarPrintRecipe,
        position: Option<[f64; 3]>,
    ) -> Self {
        Self {
            plan,
            recipe,
            layer: 0,
            path: 0,
            prefix: [None; 8],
            prefix_index: 0,
            edge: 0,
            position,
            fan: None,
            loaded: false,
        }
    }
    fn load(&mut self) {
        let p = &self.plan.layers[self.layer].paths[self.path];
        let target = p.points_mm[0];
        let layer = self.layer as u16;
        let role = p.kind;
        let travel = |target| {
            Some(Action::Move {
                target,
                source: None,
                layer,
                role,
            })
        };
        self.prefix = [None; 8];
        self.prefix_index = 0;
        self.edge = 1;
        let pwm = if role == DepositionKind::Bridge {
            self.recipe.process.bridge_fan_pwm
        } else if self.layer == 0 {
            self.recipe.process.fan_first_layers
        } else {
            self.recipe.process.fan_after_first_layers
        };
        let mut n = 0;
        if self.fan != Some(pwm) {
            self.prefix[n] = Some(Action::Fan(pwm));
            n += 1;
            self.fan = Some(pwm);
        }
        let retract = match (&self.recipe.process.retraction, self.position) {
            (
                RetractionPolicy::Enabled {
                    minimum_travel_mm, ..
                },
                Some(last),
            ) => super::distance(last, target) >= *minimum_travel_mm,
            _ => false,
        };
        if retract {
            self.prefix[n] = Some(Action::Recovery {
                retract: true,
                layer,
                role,
            });
            n += 1;
            let RetractionPolicy::Enabled { z_hop_mm, .. } = self.recipe.process.retraction else {
                unreachable!()
            };
            if z_hop_mm > 0.0 {
                let last = self.position.unwrap();
                let z = last[2].max(target[2]) + z_hop_mm;
                self.prefix[n] = travel([last[0], last[1], z]);
                n += 1;
                self.prefix[n] = travel([target[0], target[1], z]);
                n += 1;
            }
        }
        self.prefix[n] = travel(target);
        n += 1;
        if retract {
            self.prefix[n] = Some(Action::Recovery {
                retract: false,
                layer,
                role,
            });
        }
        self.loaded = true;
    }
}
impl Iterator for Expected<'_> {
    type Item = Action;
    fn next(&mut self) -> Option<Action> {
        loop {
            if self.layer == self.plan.layers.len() {
                return None;
            }
            if self.path == self.plan.layers[self.layer].paths.len() {
                self.layer += 1;
                self.path = 0;
                continue;
            }
            if !self.loaded {
                self.load();
            }
            while self.prefix_index < self.prefix.len() {
                let value = self.prefix[self.prefix_index];
                self.prefix_index += 1;
                if value.is_some() {
                    return value;
                }
            }
            let path = &self.plan.layers[self.layer].paths[self.path];
            if self.edge < path.points_mm.len() {
                let source = path.points_mm[self.edge - 1];
                let target = path.points_mm[self.edge];
                self.edge += 1;
                self.position = Some(target);
                return Some(Action::Move {
                    target,
                    source: Some(source),
                    layer: self.layer as u16,
                    role: path.kind,
                });
            }
            self.path += 1;
            self.loaded = false;
        }
    }
}
