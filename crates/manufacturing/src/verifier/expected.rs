// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Independent borrowed stored-path semantic cursor. No realization/emitter imports.
use spiling_contracts::manufacturing::*;
use std::cmp::Ordering;
use std::sync::atomic::AtomicBool;

/// Build-XY seam directions for Distributed walls: the verifier's own copy of
/// the authored rule, exact small integers in compass order.
const COMPASS: [[f64; 2]; 16] = [
    [1.0, 0.0],
    [2.0, 1.0],
    [1.0, 1.0],
    [1.0, 2.0],
    [0.0, 1.0],
    [-1.0, 2.0],
    [-1.0, 1.0],
    [-2.0, 1.0],
    [-1.0, 0.0],
    [-2.0, -1.0],
    [-1.0, -1.0],
    [-1.0, -2.0],
    [0.0, -1.0],
    [1.0, -2.0],
    [1.0, -1.0],
    [2.0, -1.0],
];

/// Check authored seam semantics independently of path/program correspondence.
/// The prior endpoint is projected because accepted byte replay must reach that
/// exact command-space point. No path is reordered or copied here. Proximity
/// ties follow the actual previous path end through every layer (or a fixed-
/// dual group's waste end). A Distributed wall maximizes its dot product with
/// compass entry `(7 * layer + identity) mod 16`, `identity` mixing its least
/// projected XY; no history crosses walls or layers.
pub(super) fn validate_wall_seams(
    plan: &NormalizedPrintPlan,
    recipe: &PlanarPrintRecipe,
    mut previous: Option<[f64; 3]>,
    routing: Option<&FixedDualRouter>,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let zero = |value: f64| if value == 0.0 { 0.0 } else { value };
    let lex_xy = |a: [f64; 3], b: [f64; 3]| {
        zero(a[0])
            .total_cmp(&zero(b[0]))
            .then_with(|| zero(a[1]).total_cmp(&zero(b[1])))
    };
    let mut previous_group = None;
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
                let vertices = &points[..points.len() - 1];
                let direction = match recipe.seam_policy {
                    SeamPolicy::Nearest => None,
                    SeamPolicy::PreferredBuildDirection { xy } => Some(xy),
                    SeamPolicy::Distributed => {
                        let mut least = super::project_xyz6_point_mm(vertices[0])?;
                        for &raw in vertices {
                            let p = super::project_xyz6_point_mm(raw)?;
                            if lex_xy(p, least).is_lt() {
                                least = p;
                            }
                        }
                        let key = |v: f64| (v * 1e6).round() as i64 as u64;
                        let identity = (key(least[0]) ^ key(least[1]).rotate_left(32))
                            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                            >> 60;
                        Some(COMPASS[((7 * u64::from(layer.index) + identity) % 16) as usize])
                    }
                };
                let score =
                    |p: [f64; 3]| direction.map_or(0.0, |xy| zero(p[0] * xy[0] + p[1] * xy[1]));
                let start = super::project_xyz6_point_mm(points[0])?;
                let start_score = score(start);
                for &raw in &vertices[1..] {
                    super::cancelled(cancel)?;
                    let candidate = super::project_xyz6_point_mm(raw)?;
                    let precedes_start = match score(candidate).total_cmp(&start_score) {
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
    pub path: usize,
    pub width: f64,
    pub physical_z: [f64; 2],
    pub owner: Option<DepositionOwner>,
    pub exposed: bool,
}

#[derive(Clone, Copy)]
pub(super) enum Action {
    BeginTool {
        layer: u32,
        tool: u8,
    },
    Fan(u8),
    /// Single-extruder later-layer nozzle/bed targets (no wait).
    NozzleTarget(f64),
    BedTarget(f64),
    /// Single-nozzle Bambu AMS support frame on a non-switch layer <= last change.
    AmsTowerFrame {
        layer: u32,
        slot: u8,
    },
    /// Single-nozzle Bambu AMS mid-print switch + flush + optional tower prime.
    AmsSwitch {
        from: u8,
        to: u8,
        layer: u32,
    },
    Move {
        target: [f64; 3],
        tool: Option<u8>,
        source: Option<[f64; 3]>,
        layer: u32,
        role: DepositionKind,
        speed_role: DepositionKind,
        association: Option<Association>,
        closes_wall: bool,
        /// The planned arc run this edge starts, if any.
        arc: Option<ArcRun>,
        /// A short interior edge joining two straight rows of one continuous
        /// skin or solid-fill path; exempt from the row-axis rule only.
        connector: bool,
    },
    Recovery {
        retract: bool,
        layer: u32,
        /// The role the speed rule and fan follow ([`speeds::speed_role`]).
        speed_role: DepositionKind,
    },
}
pub(super) struct Expected<'a> {
    plan: &'a NormalizedPrintPlan,
    intent: &'a ManufacturingIntent,
    recipe: &'a PlanarPrintRecipe,
    /// The current layer's declared cooling decision, recomputed here.
    cooling: Option<(usize, speeds::LayerCooling)>,
    /// The later-layer temperature targets have been expected.
    later_nozzle_done: bool,
    later_bed_done: bool,
    bambu_ams: Option<&'a BambuAms>,
    last_ams_change: Option<u32>,
    ams_active_slot: u8,
    ams_frame_checked_layer: Option<usize>,
    layer: usize,
    path: usize,
    prefix: [Option<Action>; 12],
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
        intent: &'a ManufacturingIntent,
        position: Option<[f64; 3]>,
        routing: Option<&'a FixedDualRouter<'a>>,
    ) -> Self {
        let bambu_ams = match &intent.printer.workflow {
            JobWorkflow::CompleteMarlin { job } => job.bambu_ams(),
            _ => None,
        };
        let last_ams_change = bambu_ams.and_then(|ams| super::ams::last_change_layer(plan, ams));
        Self {
            plan,
            intent,
            recipe: &intent.recipe,
            cooling: None,
            later_nozzle_done: false,
            later_bed_done: false,
            bambu_ams,
            last_ams_change,
            ams_active_slot: 0,
            ams_frame_checked_layer: None,
            layer: 0,
            path: 0,
            prefix: [None; 12],
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
        let speed_role =
            speeds::speed_role(self.recipe, &self.plan.layers[self.layer], role, p.owner);
        let travel = |target| {
            Some(Action::Move {
                target,
                source: None,
                layer,
                role,
                speed_role,
                tool: path_tool,
                association: None,
                closes_wall: false,
                arc: None,
                connector: false,
            })
        };
        self.prefix = [None; 12];
        self.prefix_index = 0;
        self.edge = 1;
        self.edges = Some(p.checked_edges()?);
        if self.cooling.is_none_or(|(layer, _)| layer != self.layer) {
            let cooling = speeds::layer_cooling(self.intent, &self.plan.layers[self.layer], layer)?;
            self.cooling = Some((self.layer, cooling));
        }
        let short = self.cooling.is_some_and(|(_, c)| c.short);
        let policy = self
            .routing
            .zip(path_tool)
            .map_or(&self.recipe.process, |(job, tool)| {
                job.tools[usize::from(tool)].process(self.recipe)
            });
        let pwm = speeds::path_fan_pwm(policy, speed_role, layer, short);
        let mut n = 0;
        if let Some(ams) = self.bambu_ams {
            let needed = ams.slot_for_kind(p.kind);
            if needed != self.ams_active_slot {
                self.prefix[n] = Some(Action::AmsSwitch {
                    from: self.ams_active_slot,
                    to: needed,
                    layer,
                });
                n += 1;
                self.ams_active_slot = needed;
                self.fan = Some(0);
                self.position = None;
                if self.layer > 0 {
                    self.later_nozzle_done = true;
                }
            }
        }
        // The second layer's first path is preceded by the later-layer
        // targets when they differ from the first-layer ones.
        if let JobWorkflow::CompleteMarlin { job } = &self.intent.printer.workflow
            && self.layer > 0
        {
            if !self.later_nozzle_done {
                self.later_nozzle_done = true;
                let (nozzle_c, first_nozzle_c) = match self.bambu_ams {
                    Some(ams) => {
                        let s = &ams.slots[usize::from(self.ams_active_slot)];
                        (s.nozzle_temperature_c, s.first_layer_nozzle_temperature_c)
                    }
                    None => (
                        job.nozzle_temperature_c,
                        job.first_layer_nozzle_temperature_c,
                    ),
                };
                if nozzle_c != first_nozzle_c {
                    self.prefix[n] = Some(Action::NozzleTarget(nozzle_c));
                    n += 1;
                }
            }
            if !self.later_bed_done {
                self.later_bed_done = true;
                if job.bed_temperature_c != job.first_layer_bed_temperature_c {
                    self.prefix[n] = Some(Action::BedTarget(job.bed_temperature_c));
                    n += 1;
                }
            }
        }
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
                speed_role,
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
                speed_role,
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
            if let Some(ams) = self.bambu_ams
                && self.ams_frame_checked_layer != Some(self.layer)
            {
                self.ams_frame_checked_layer = Some(self.layer);
                let layer = &self.plan.layers[self.layer];
                if ams.prime_tower.is_some()
                    && self.last_ams_change.is_some_and(|lc| layer.index <= lc)
                    && !super::ams::layer_has_switch(layer, ams, self.ams_active_slot)
                {
                    self.position = None;
                    return Some(Ok(Action::AmsTowerFrame {
                        layer: layer.index,
                        slot: self.ams_active_slot,
                    }));
                }
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
                let connector = path.is_connector_edge(self.edge - 2);
                return Some(Ok(Action::Move {
                    connector,
                    target,
                    tool: self.routing.map(|_| self.previous_group.unwrap().1),
                    source: Some(source),
                    layer: self.layer as u32,
                    role: path.kind,
                    speed_role: speeds::speed_role(
                        self.recipe,
                        &self.plan.layers[self.layer],
                        path.kind,
                        path.owner,
                    ),
                    association: Some(Association {
                        path: self.path,
                        width: edge.width_mm,
                        physical_z: path.physical_z_mm,
                        owner,
                        exposed: path.exposed,
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
