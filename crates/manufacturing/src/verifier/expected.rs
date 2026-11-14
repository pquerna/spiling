// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Independent borrowed stored-path semantic cursor. No realization/emitter imports.
use spiling_contracts::manufacturing::*;
use std::cmp::Ordering;
use std::sync::atomic::AtomicBool;

/// Check authored seam semantics independently of path/program correspondence.
/// The prior endpoint is projected because accepted byte replay must reach that
/// exact command-space point. No path is reordered or copied here.
pub(super) fn validate_wall_seams(
    plan: &NormalizedPrintPlan,
    recipe: &PlanarPrintRecipe,
    mut previous: Option<[f64; 3]>,
    routing: Option<&FixedDualRouter>,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let zero = |value: f64| if value == 0.0 { 0.0 } else { value };
    let score = |point: [f64; 3], history: &[[f64; 2]]| match recipe.seam_policy {
        SeamPolicy::Nearest => 0.0,
        SeamPolicy::Distributed => {
            let mut clearance = f64::INFINITY;
            for prior in history {
                clearance = clearance.min((prior[0] - point[0]).hypot(prior[1] - point[1]));
            }
            clearance
        }
        SeamPolicy::PreferredBuildDirection { xy } => zero(point[0] * xy[0] + point[1] * xy[1]),
    };
    let lex_xy = |a: [f64; 3], b: [f64; 3]| {
        zero(a[0])
            .total_cmp(&zero(b[0]))
            .then_with(|| zero(a[1]).total_cmp(&zero(b[1])))
    };
    let mut previous_group = None;
    let mut wall_starts = [[[0.0; 2]; 8]; 2];
    let mut wall_counts = [0usize; 2];
    let mut wall_cursor = [0usize; 2];
    for layer in &plan.layers {
        for path in &layer.paths {
            let tool = routing.map_or(Ok(0), |job| job.tool_for_path(path))?;
            if let Some(job) = routing
                && previous_group != Some((layer.index, tool))
            {
                let endpoint = job.waste_end_mm(tool, layer.z_mm);
                previous = Some(if recipe.seam_policy == SeamPolicy::Distributed {
                    super::project_xyz6_point_mm(endpoint)?
                } else {
                    endpoint
                });
                previous_group = Some((layer.index, tool));
            }
            super::cancelled(cancel)?;
            let points = &path.points_mm;
            if path.owner == Some(DepositionOwner::Wall) && points.first() == points.last() {
                let start = super::project_xyz6_point_mm(points[0])?;
                let history = &wall_starts[tool as usize][..wall_counts[tool as usize]];
                let start_score = score(start, history);
                for &raw in &points[1..points.len() - 1] {
                    super::cancelled(cancel)?;
                    let candidate = super::project_xyz6_point_mm(raw)?;
                    let precedes_start = match score(candidate, history).total_cmp(&start_score) {
                        Ordering::Greater => true,
                        Ordering::Less => false,
                        Ordering::Equal => {
                            let proximity = previous.map_or(Ordering::Equal, |from| {
                                super::distance(from, candidate)
                                    .total_cmp(&super::distance(from, start))
                            });
                            proximity
                                .then_with(|| lex_xy(candidate, start))
                                .then_with(|| lex_xy(raw, points[0]))
                                .is_lt()
                        }
                    };
                    if precedes_start {
                        return Err(super::failed(
                            "closed wall start violates authored seam policy",
                        ));
                    }
                }
                if recipe.seam_policy == SeamPolicy::Distributed {
                    let tool = tool as usize;
                    wall_starts[tool][wall_cursor[tool]] = [start[0], start[1]];
                    wall_cursor[tool] = (wall_cursor[tool] + 1) % 8;
                    wall_counts[tool] = (wall_counts[tool] + 1).min(8);
                }
                // Identical projected/raw XY ties have the same physical seam;
                // their pre-rotation cyclic indices are not retained by the plan.
                previous = Some(start);
            } else {
                previous = points
                    .last()
                    .copied()
                    .map(super::project_xyz6_point_mm)
                    .transpose()?;
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
pub(super) struct Association {
    pub width: f64,
    pub physical_z: [f64; 2],
    pub owner: Option<DepositionOwner>,
}

#[derive(Clone, Copy)]
pub(super) enum Action {
    BeginTool {
        layer: u32,
        tool: u8,
    },
    Fan(u8),
    Move {
        target: [f64; 3],
        tool: Option<u8>,
        source: Option<[f64; 3]>,
        layer: u32,
        role: DepositionKind,
        association: Option<Association>,
        closes_wall: bool,
        /// The planned arc run this edge starts, if any.
        arc: Option<ArcRun>,
    },
    Recovery {
        retract: bool,
        layer: u32,
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
    edges: Option<DepositionEdges<'a>>,
    position: Option<[f64; 3]>,
    fan: Option<u8>,
    loaded: bool,
    routing: Option<&'a FixedDualRouter<'a>>,
    previous_group: Option<(u32, u8)>,
    groups_started: u8,
    safe_z: f64,
}
impl<'a> Expected<'a> {
    pub(super) fn new(
        plan: &'a NormalizedPrintPlan,
        recipe: &'a PlanarPrintRecipe,
        position: Option<[f64; 3]>,
        routing: Option<&'a FixedDualRouter<'a>>,
    ) -> Self {
        Self {
            plan,
            recipe,
            layer: 0,
            path: 0,
            prefix: [None; 8],
            prefix_index: 0,
            edge: 0,
            edges: None,
            position,
            fan: None,
            loaded: false,
            routing,
            previous_group: None,
            groups_started: 0,
            safe_z: routing.map_or(0.0, |job| {
                plan.layers.last().unwrap().z_mm + job.transition_lift_mm
            }),
        }
    }
    fn load(&mut self) -> Result<(), ManufacturingError> {
        let p = &self.plan.layers[self.layer].paths[self.path];
        let target = p.points_mm[0];
        let path_tool = self.routing.map(|job| job.tool_for_path(p)).transpose()?;
        let layer = self.layer as u32;
        let role = p.kind;
        let travel = |target| {
            Some(Action::Move {
                target,
                source: None,
                layer,
                role,
                tool: path_tool,
                association: None,
                closes_wall: false,
                arc: None,
            })
        };
        self.prefix = [None; 8];
        self.prefix_index = 0;
        self.edge = 1;
        self.edges = Some(p.checked_edges()?);
        let policy = self
            .routing
            .zip(path_tool)
            .map_or(&self.recipe.process, |(job, tool)| {
                job.tools[usize::from(tool)].process(self.recipe)
            });
        let pwm = if role == DepositionKind::Bridge {
            policy.bridge_fan_pwm
        } else if self.layer == 0 {
            policy.fan_first_layers
        } else {
            policy.fan_after_first_layers
        };
        let mut n = 0;
        if let Some(job) = self.routing {
            let tool = job.tool_for_path(p)?;
            if self.previous_group != Some((layer, tool)) {
                self.prefix[n] = Some(Action::BeginTool { layer, tool });
                n += 1;
                self.position = Some(job.waste_end_mm(tool, p.physical_z_mm[1]));
                self.previous_group = Some((layer, tool));
            }
        }
        if self.fan != Some(pwm) {
            self.prefix[n] = Some(Action::Fan(pwm));
            n += 1;
            self.fan = Some(pwm);
        }
        if self.routing.is_some() {
            let previous = self
                .position
                .ok_or_else(|| super::failed("routed path entry lacks a known position"))?;
            self.prefix[n] = travel([previous[0], previous[1], self.safe_z]);
            self.prefix[n + 1] = travel([target[0], target[1], self.safe_z]);
            self.prefix[n + 2] = travel(target);
            self.loaded = true;
            return Ok(());
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
        Ok(())
    }
}
impl Iterator for Expected<'_> {
    type Item = Result<Action, ManufacturingError>;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.layer == self.plan.layers.len() {
                return None;
            }
            if let Some(job) = self.routing
                && !self.loaded
                && self.groups_started < 2
            {
                let next = self.plan.layers[self.layer]
                    .paths
                    .get(self.path)
                    .map(|p| job.tool_for_path(p))
                    .transpose();
                let next = match next {
                    Ok(value) => value,
                    Err(error) => {
                        self.layer = self.plan.layers.len();
                        return Some(Err(error));
                    }
                };
                let order = self.plan.routing_tool_orders[self.layer];
                if self.groups_started == 0
                    || next.is_none_or(|tool| tool == order[self.groups_started as usize])
                {
                    let tool = order[self.groups_started as usize];
                    self.groups_started += 1;
                    self.position = Some(job.waste_end_mm(tool, self.plan.layers[self.layer].z_mm));
                    self.previous_group = Some((self.layer as u32, tool));
                    return Some(Ok(Action::BeginTool {
                        layer: self.layer as u32,
                        tool,
                    }));
                }
            }
            if self.path == self.plan.layers[self.layer].paths.len() {
                self.layer += 1;
                self.path = 0;
                self.groups_started = 0;
                continue;
            }
            if !self.loaded
                && let Err(error) = self.load()
            {
                self.layer = self.plan.layers.len();
                return Some(Err(error));
            }
            while self.prefix_index < self.prefix.len() {
                let value = self.prefix[self.prefix_index];
                self.prefix_index += 1;
                if value.is_some() {
                    return value.map(Ok);
                }
            }
            let path = &self.plan.layers[self.layer].paths[self.path];
            if let Some(edge) = self.edges.as_mut().and_then(Iterator::next) {
                let source = edge.points_mm[0];
                let target = edge.points_mm[1];
                let owner = match path.owner {
                    Some(DepositionOwner::RegionalSkinBridge {
                        region_index,
                        skin,
                        skin_first,
                    }) => Some(if skin_first == (self.edge == 1) {
                        DepositionOwner::MandatorySkin { skin }
                    } else {
                        DepositionOwner::Regional { region_index }
                    }),
                    owner => owner,
                };
                self.edge += 1;
                self.position = Some(target);
                return Some(Ok(Action::Move {
                    target,
                    tool: self.routing.map(|_| self.previous_group.unwrap().1),
                    source: Some(source),
                    layer: self.layer as u32,
                    role: path.kind,
                    association: Some(Association {
                        width: edge.width_mm,
                        physical_z: path.physical_z_mm,
                        owner,
                    }),
                    closes_wall: path.owner == Some(DepositionOwner::Wall)
                        && path.points_mm.first() == path.points_mm.last()
                        && self.edge == path.points_mm.len(),
                    arc: path
                        .arcs
                        .iter()
                        .find(|run| run.first as usize + 2 == self.edge)
                        .copied(),
                }));
            }
            self.path += 1;
            self.loaded = false;
        }
    }
}
