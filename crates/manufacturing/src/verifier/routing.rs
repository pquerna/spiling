// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Independent fixed-hotend command cursor. No producer/realization imports.
use super::*;

#[derive(Clone, Copy)]
enum Op {
    Bare(Command),
    Scalar(Command, usize, f64, Option<u8>),
    Offset(u8),
    Select(u8),
    Home,
    Epoch,
    Retract,
    Recover,
    Move([f64; 3], bool), // endpoint, final lifecycle motion
    Waste,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stage {
    Setup,
    Groups,
    Finish,
    Done,
}

pub(super) struct RouteState<'a> {
    job: &'a FixedDualJob,
    plan: &'a NormalizedPrintPlan,
    recipe: &'a PlanarPrintRecipe,
    ops: [Option<Op>; 24],
    count: usize,
    index: usize,
    stage: Stage,
    active: u8,
    debt_ticks: [i64; 2],
    ready: [bool; 2],
    group: u32,
    layer: u16,
    safe_z: f64,
    accounting: [RoutedToolAccounting; 2],
    pub(super) expected_e: f64,
    pub(super) travel_segments: u32,
    pub(super) retraction_count: u32,
    pub(super) retracted_filament_mm: f64,
    pub(super) selection_count: u32,
    pub(super) max_position_error_mm: f64,
    pub(super) max_extrusion_error_mm: f64,
}

fn ticks(value: f64) -> Result<i64, ManufacturingError> {
    let scaled = (value * 100_000_000.0).round();
    if !scaled.is_finite() || scaled.abs() > (1_u64 << 52) as f64 {
        return Err(failed(
            "routed E8 coordinate exceeds exact feeder-accounting range",
        ));
    }
    Ok(scaled as i64)
}

impl<'a> RouteState<'a> {
    pub(super) fn new(
        job: &'a FixedDualJob,
        plan: &'a NormalizedPrintPlan,
        recipe: &'a PlanarPrintRecipe,
    ) -> Self {
        let account = |index: usize| RoutedToolAccounting {
            tool: index as u8,
            material_id: job.tools[index].material.id.clone(),
            model_deposition_segments: 0,
            model_filament_length_mm: 0.0,
            model_volume_mm3: 0.0,
            waste_deposition_segments: 0,
            waste_filament_length_mm: 0.0,
            waste_volume_mm3: 0.0,
            outstanding_retraction_mm: 0.0,
        };
        let mut state = Self {
            job,
            plan,
            recipe,
            ops: [None; 24],
            count: 0,
            index: 0,
            stage: Stage::Setup,
            active: 0,
            debt_ticks: [0; 2],
            ready: [false; 2],
            group: 0,
            layer: 0,
            safe_z: plan.layers.last().unwrap().z_mm + job.transition_lift_mm,
            accounting: [account(0), account(1)],
            expected_e: 0.0,
            travel_segments: 0,
            retraction_count: 0,
            retracted_filament_mm: 0.0,
            selection_count: 0,
            max_position_error_mm: 0.0,
            max_extrusion_error_mm: 0.0,
        };
        state.push(Op::Bare(Command::Celsius));
        state.push(Op::Scalar(Command::VolumetricOff, 7, 0.0, None));
        state.push(Op::Scalar(Command::FeedScale, 5, 100.0, None));
        state.push(Op::Scalar(Command::FlowScale, 5, 100.0, Some(0)));
        state.push(Op::Scalar(Command::FlowScale, 5, 100.0, Some(1)));
        state.push(Op::Bare(Command::FanOff));
        state.push(Op::Offset(0));
        state.push(Op::Offset(1));
        state.push(Op::Select(0));
        state.push(Op::Home);
        state.push(Op::Scalar(Command::LevelingOff, 5, 0.0, None));
        state.push(Op::Epoch);
        state.push(Op::Scalar(Command::WaitBed, 6, job.bed_temperature_c, None));
        for tool in 0..2 {
            state.push(Op::Scalar(
                Command::NozzleOff,
                5,
                job.tools[tool].standby_temperature_c,
                Some(tool as u8),
            ));
        }
        state
    }
    fn push(&mut self, op: Op) {
        // Fixed-size lifecycle cursor, never a motion buffer.
        self.ops[self.count] = Some(op);
        self.count += 1;
    }
    fn clear(&mut self) {
        self.count = 0;
        self.index = 0;
        self.ops.fill(None);
    }
    pub(super) fn pending(&self) -> bool {
        self.index < self.count
    }
    pub(super) fn done(&self) -> bool {
        self.stage == Stage::Done
    }
    pub(super) fn filament_area(&self) -> f64 {
        std::f64::consts::PI
            * (self.job.tools[self.active as usize].filament_diameter_mm * 0.5).powi(2)
    }
    pub(super) fn begin_group(
        &mut self,
        layer: u16,
        tool: u8,
        motion: &MotionState,
        expected_e: f64,
    ) -> Result<(), ManufacturingError> {
        if self.stage != Stage::Groups
            || self.pending()
            || u32::from(layer) * 2 + u32::from(tool) != self.group
        {
            return Err(failed(
                "routed model groups must be tool zero then one on every actual layer",
            ));
        }
        self.clear();
        self.layer = layer;
        self.expected_e = expected_e;
        let last = motion
            .position
            .ok_or_else(|| failed("routed transition has no actual position"))?;
        let waste = self.job.tools[tool as usize].waste_line_xy_mm[0];
        let start =
            project_xyz6_point_mm([waste[0], waste[1], self.plan.layers[layer as usize].z_mm])?;
        if self.group != 0 {
            self.push(Op::Bare(Command::Barrier));
            self.push(Op::Retract);
            self.push(Op::Move([last[0], last[1], self.safe_z], false));
            let outgoing = self.job.tools[self.active as usize].offset_xy_mm;
            let incoming = self.job.tools[tool as usize].offset_xy_mm;
            self.push(Op::Move(
                [
                    start[0] + outgoing[0] - incoming[0],
                    start[1] + outgoing[1] - incoming[1],
                    self.safe_z,
                ],
                false,
            ));
            self.push(Op::Bare(Command::Barrier));
            self.push(Op::Scalar(
                Command::NozzleOff,
                5,
                self.job.tools[self.active as usize].standby_temperature_c,
                Some(self.active),
            ));
            self.push(Op::Select(tool));
            self.push(Op::Epoch);
        } else if tool != 0 || self.active != 0 {
            return Err(failed("first routed group must use selected tool zero"));
        }
        self.push(Op::Scalar(
            Command::WaitNozzle,
            6,
            self.job.tools[tool as usize].nozzle_temperature_c,
            Some(tool),
        ));
        if self.debt_ticks[tool as usize] != 0 {
            self.push(Op::Recover);
        }
        if self.group == 0 {
            self.push(Op::Move([last[0], last[1], self.safe_z], false));
            self.push(Op::Move([start[0], start[1], self.safe_z], false));
        }
        self.push(Op::Move(start, false));
        self.push(Op::Waste);
        self.group += 1;
        Ok(())
    }
    pub(super) fn begin_finish(
        &mut self,
        motion: &MotionState,
        expected_e: f64,
    ) -> Result<(), ManufacturingError> {
        if self.stage != Stage::Groups
            || self.pending()
            || self.group != self.plan.layers.len() as u32 * 2
        {
            return Err(failed("shutdown before complete routed model groups"));
        }
        let last = motion
            .position
            .ok_or_else(|| failed("routed park has no actual position"))?;
        self.clear();
        self.stage = Stage::Finish;
        self.expected_e = expected_e;
        self.push(Op::Bare(Command::Barrier));
        self.push(Op::Retract);
        self.push(Op::Move(
            [last[0], last[1], self.job.park_position_mm[2]],
            true,
        ));
        self.push(Op::Move(self.job.park_position_mm, true));
        self.push(Op::Bare(Command::Barrier));
        self.push(Op::Scalar(Command::NozzleOff, 5, 0.0, Some(0)));
        self.push(Op::Scalar(Command::NozzleOff, 5, 0.0, Some(1)));
        self.push(Op::Scalar(Command::BedOff, 5, 0.0, None));
        self.push(Op::Bare(Command::FanOff));
        self.push(Op::Bare(Command::MotorsOff));
        Ok(())
    }
    pub(super) fn check_motion(
        &self,
        from: [f64; 3],
        to: [f64; 3],
        intent: &ManufacturingIntent,
    ) -> Result<(), ManufacturingError> {
        let active_offset = self.job.tools[self.active as usize].offset_xy_mm;
        let inactive_offset = self.job.tools[1 - self.active as usize].offset_xy_mm;
        for point in [from, to] {
            for axis in 0..3 {
                if !point[axis].is_finite()
                    || point[axis] < intent.printer.build_envelope.min[axis]
                    || point[axis] > intent.printer.build_envelope.max[axis]
                {
                    return Err(failed("actual routed tip exceeds the command envelope"));
                }
                let carriage = point[axis] - if axis < 2 { active_offset[axis] } else { 0.0 };
                if carriage < self.job.carriage_travel_bounds_mm.min[axis]
                    || carriage > self.job.carriage_travel_bounds_mm.max[axis]
                {
                    return Err(failed(
                        "actual routed carriage exceeds declared travel bounds",
                    ));
                }
            }
        }
        // Whole-segment AABBs are conservative supersets. This deliberately
        // refuses a crossing it cannot prove clear; it never samples a sweep.
        let mut idle = [[0.0; 3]; 2];
        let mut body = [[0.0; 3]; 2];
        for axis in 0..3 {
            let low = from[axis].min(to[axis]);
            let high = from[axis].max(to[axis]);
            let active = if axis < 2 { active_offset[axis] } else { 0.0 };
            let inactive = if axis < 2 { inactive_offset[axis] } else { 0.0 };
            let pad = if axis < 2 {
                self.job.inactive_nozzle_radius_mm
            } else {
                0.0
            };
            idle[0][axis] = (low + inactive - active - pad).next_down();
            idle[1][axis] = (high + inactive - active + pad).next_up();
            body[0][axis] = (low - active + self.job.carriage_body_bounds_mm.min[axis]).next_down();
            body[1][axis] = (high - active + self.job.carriage_body_bounds_mm.max[axis]).next_up();
        }
        let top = self.plan.layers[self.layer as usize].z_mm;
        let intersects = |a: [[f64; 3]; 2], b: [[f64; 3]; 2]| {
            (0..3).all(|axis| a[0][axis] <= b[1][axis] && b[0][axis] <= a[1][axis])
        };
        for assignment in &self.job.assignments {
            let bounds = &assignment.work_envelope_mm;
            let occupied = [
                bounds.min,
                [bounds.max[0], bounds.max[1], bounds.max[2].min(top)],
            ];
            if intersects(idle, occupied) || intersects(body, occupied) {
                return Err(failed(
                    "actual coplanar inactive nozzle or carriage body intersects a reserved model prefix",
                ));
            }
        }
        let radius = self.recipe.bead_width_mm * 0.5;
        for tool in &self.job.tools {
            let [a, b] = tool.waste_line_xy_mm;
            let occupied = [
                [
                    a[0].min(b[0]) - radius,
                    a[1].min(b[1]) - radius,
                    intent.printer.build_envelope.min[2],
                ],
                [a[0].max(b[0]) + radius, a[1].max(b[1]) + radius, top],
            ];
            if intersects(idle, occupied) || intersects(body, occupied) {
                return Err(failed(
                    "actual inactive nozzle or carriage body intersects a reserved waste prefix",
                ));
            }
        }
        Ok(())
    }
    pub(super) fn consume(
        &mut self,
        decoded: &Decoded,
        motion: &mut MotionState,
        intent: &ManufacturingIntent,
        line: u32,
        mut replay: Option<&mut ReplaySink>,
    ) -> Result<(), ManufacturingError> {
        let op =
            self.ops[self.index].ok_or_else(|| failed("missing routed command expectation"))?;
        let tool = self.active as usize;
        match op {
            Op::Bare(command) => {
                if decoded.command != command {
                    return Err(failed("missing or reordered routed state/barrier"));
                }
            }
            Op::Scalar(command, field, value, addressed) => {
                if decoded.command != command
                    || decoded.fields[field] != Some(value)
                    || decoded.fields[8] != addressed.map(f64::from)
                {
                    return Err(failed(
                        "mutated routed thermal/modal command or addressed tool",
                    ));
                }
                if let Some(target) = addressed {
                    if command == Command::WaitNozzle {
                        self.ready[target as usize] = true;
                    }
                    if command == Command::NozzleOff {
                        self.ready[target as usize] = false;
                    }
                }
            }
            Op::Offset(target) => {
                let offset = self.job.tools[target as usize].offset_xy_mm;
                if decoded.command != Command::HotendOffset
                    || decoded.fields[8] != Some(f64::from(target))
                    || decoded.fields[0] != Some(offset[0])
                    || decoded.fields[1] != Some(offset[1])
                    || decoded.fields[2] != Some(0.0)
                {
                    return Err(failed(
                        "missing or mutated complete coplanar hotend offset reset",
                    ));
                }
            }
            Op::Select(target) => {
                if decoded.command != Command::SelectTool(target) {
                    return Err(failed("wrong or non-S1 routed tool selection"));
                }
                if let Some(point) = &mut motion.position {
                    for axis in 0..2 {
                        point[axis] += self.job.tools[target as usize].offset_xy_mm[axis]
                            - self.job.tools[tool].offset_xy_mm[axis];
                    }
                }
                self.active = target;
            }
            Op::Home => {
                if decoded.command != Command::Home || self.active != 0 {
                    return Err(failed("routed native home must select tool zero"));
                }
                motion.position = Some(project_xyz6_point_mm(self.job.home_position_mm)?);
            }
            Op::Epoch => {
                if decoded.command != Command::ResetExtrusion {
                    return Err(failed("selection must establish an explicit E0 epoch"));
                }
                motion.extrusion = 0.0;
                self.expected_e = 0.0;
                self.selection_count += 1;
                if self.selection_count > MAX_TOOL_SELECTIONS {
                    return Err(failed("tool epoch budget exceeded"));
                }
                if let Some(sink) = replay.as_deref_mut() {
                    sink.push_epoch(
                        motion
                            .position
                            .ok_or_else(|| failed("tool epoch lacks known position"))?,
                        self.active,
                        line,
                        self.layer,
                    )?;
                }
            }
            Op::Retract | Op::Recover => {
                let retract = matches!(op, Op::Retract);
                let policy = &self.job.tools[tool];
                if decoded.command != Command::Recovery
                    || motion.position.is_none()
                    || (retract && self.debt_ticks[tool] != 0)
                    || (!retract && self.debt_ticks[tool] <= 0)
                    || !self.ready[tool]
                {
                    return Err(failed(
                        "routed pressure operation has wrong selection, debt or readiness",
                    ));
                }
                let next = decoded.fields[3].unwrap();
                let feed = decoded.fields[4].unwrap();
                let wanted = if retract {
                    self.expected_e - policy.retract_filament_mm
                } else {
                    self.debt_ticks[tool] as f64 / 100_000_000.0
                };
                if (next - wanted).abs() > E_HALF_STEP + arithmetic_slack(wanted)
                    || (feed - policy.retract_speed_mm_s * 60.0).abs()
                        > F_HALF_STEP + arithmetic_slack(feed)
                {
                    return Err(failed(
                        "routed pressure length or speed differs from explicit policy",
                    ));
                }
                if retract {
                    let debt = ticks(motion.extrusion)? - ticks(next)?;
                    if debt <= 0 {
                        return Err(failed(
                            "routed retraction must physically withdraw feeder length",
                        ));
                    }
                    self.debt_ticks[tool] = debt;
                    self.retraction_count += 1;
                    self.retracted_filament_mm += debt as f64 / 100_000_000.0;
                    self.expected_e -= policy.retract_filament_mm;
                } else {
                    if ticks(next)? - ticks(motion.extrusion)? != self.debt_ticks[tool] {
                        return Err(failed("G92 does not discharge exact feeder recovery debt"));
                    }
                    self.debt_ticks[tool] = 0;
                    self.expected_e = wanted;
                }
                self.max_extrusion_error_mm =
                    self.max_extrusion_error_mm.max((next - wanted).abs());
                motion.extrusion = next;
                if let Some(sink) = replay.as_deref_mut() {
                    sink.push(
                        motion.position.unwrap(),
                        next,
                        feed,
                        line,
                        self.layer,
                        if retract { 11 } else { 12 },
                    )?;
                }
            }
            Op::Move(target, lifecycle) => {
                if decoded.command != Command::Travel {
                    return Err(failed("missing ordered lifted routed travel"));
                }
                let actual = [
                    decoded.fields[0].unwrap(),
                    decoded.fields[1].unwrap(),
                    decoded.fields[2].unwrap(),
                ];
                let feed = decoded.fields[4].unwrap();
                if actual != project_xyz6_point_mm(target)?
                    || (feed - self.recipe.travel_speed_mm_s * 60.0).abs()
                        > F_HALF_STEP + arithmetic_slack(feed)
                {
                    return Err(failed("mutated routed travel endpoint or feed"));
                }
                self.check_motion(
                    motion
                        .position
                        .ok_or_else(|| failed("travel before native home"))?,
                    actual,
                    intent,
                )?;
                self.max_position_error_mm =
                    self.max_position_error_mm.max(distance(actual, target));
                if lifecycle {
                } else {
                    self.travel_segments += 1;
                }
                if let Some(sink) = replay.as_deref_mut() {
                    sink.push(
                        actual,
                        motion.extrusion,
                        feed,
                        line,
                        if lifecycle { u16::MAX } else { self.layer },
                        if lifecycle { 6 } else { 0 },
                    )?;
                }
                motion.position = Some(actual);
            }
            Op::Waste => {
                if decoded.command != Command::Deposit
                    || !self.ready[tool]
                    || self.debt_ticks[tool] != 0
                {
                    return Err(failed(
                        "waste before selected-tool wait or exact debt recovery",
                    ));
                }
                let layer = &self.plan.layers[self.layer as usize];
                let xy = self.job.tools[tool].waste_line_xy_mm[1];
                let wanted = [xy[0], xy[1], layer.z_mm];
                let actual = [
                    decoded.fields[0].unwrap(),
                    decoded.fields[1].unwrap(),
                    decoded.fields[2].unwrap(),
                ];
                let from = motion
                    .position
                    .ok_or_else(|| failed("waste before positioning"))?;
                if actual != project_xyz6_point_mm(wanted)? {
                    return Err(failed("mutated waste stroke endpoint"));
                }
                self.check_motion(from, actual, intent)?;
                for point in [from, actual] {
                    for axis in 0..2 {
                        let radius = (self.recipe.bead_width_mm * 0.5
                            + program_coordinates::xyz6_coordinate_error_mm(point[axis])?)
                        .next_up();
                        if (point[axis] - radius).next_down()
                            < intent.printer.build_envelope.min[axis]
                            || (point[axis] + radius).next_up()
                                > intent.printer.build_envelope.max[axis]
                        {
                            return Err(failed(
                                "actual full waste bead extends beyond the declared bed footprint",
                            ));
                        }
                    }
                }
                let length = distance(from, actual);
                let height = layer.z_mm - layer.lower_z_mm;
                let speed = self.recipe.print_speed_mm_s
                    * if self.layer == 0 {
                        self.recipe.process.first_layer_speed_factor
                    } else {
                        1.0
                    };
                let flow = self.recipe.flow_multiplier
                    * if self.layer == 0 {
                        self.recipe.process.first_layer_flow_factor
                    } else {
                        1.0
                    };
                let area = ((self.recipe.bead_width_mm - height) * height
                    + std::f64::consts::PI * (height * 0.5).powi(2))
                    * flow;
                let volume = length * area;
                let feed = decoded.fields[4].unwrap();
                let next = decoded.fields[3].unwrap();
                self.expected_e += volume / self.filament_area();
                let delta = next - motion.extrusion;
                if length < program_coordinates::MIN_DEPOSITION_COMMAND_MM
                    || delta <= 0.0
                    || !volume.is_finite()
                    || volume < self.job.tools[tool].minimum_reprime_volume_mm3
                    || (next - self.expected_e).abs()
                        > E_HALF_STEP + arithmetic_slack(self.expected_e)
                    || (feed - speed * 60.0).abs() > F_HALF_STEP + arithmetic_slack(feed)
                    || feed / 60.0 > intent.printer.max_feed_mm_s
                    || area * speed > intent.effective_max_volumetric_flow_mm3_s()
                {
                    return Err(failed(
                        "waste quantity, E8 volume, flow or speed differs from declared supported strip",
                    ));
                }
                let filament_area = self.filament_area();
                let account = &mut self.accounting[tool];
                account.waste_deposition_segments += 1;
                account.waste_filament_length_mm += delta;
                account.waste_volume_mm3 += delta * filament_area;
                if let Some(sink) = replay.as_deref_mut() {
                    sink.push_associated(
                        actual,
                        next,
                        feed,
                        line,
                        self.layer,
                        15,
                        Some(expected::Association {
                            width: self.recipe.bead_width_mm,
                            physical_z: [layer.lower_z_mm, layer.z_mm],
                            owner: None,
                        }),
                    )?;
                }
                self.max_extrusion_error_mm = self
                    .max_extrusion_error_mm
                    .max((next - self.expected_e).abs());
                self.max_position_error_mm =
                    self.max_position_error_mm.max(distance(actual, wanted));
                motion.position = Some(actual);
                motion.extrusion = next;
            }
        }
        self.index += 1;
        if self.index == self.count {
            self.stage = if self.stage == Stage::Finish {
                Stage::Done
            } else {
                Stage::Groups
            };
        }
        Ok(())
    }
    pub(super) fn record_model(
        &mut self,
        path_tool: u8,
        delta: f64,
    ) -> Result<(), ManufacturingError> {
        if path_tool != self.active
            || !self.ready[self.active as usize]
            || self.debt_ticks[self.active as usize] != 0
        {
            return Err(failed(
                "actual selected feeder does not own the expected source material",
            ));
        }
        let area = self.filament_area();
        let account = &mut self.accounting[self.active as usize];
        account.model_deposition_segments += 1;
        account.model_filament_length_mm += delta;
        account.model_volume_mm3 += delta * area;
        Ok(())
    }
    pub(super) fn report(&mut self) -> Result<FixedDualVerification, ManufacturingError> {
        if !self.done() || self.group != self.plan.layers.len() as u32 * 2 {
            return Err(failed("incomplete routed lifecycle"));
        }
        for tool in 0..2 {
            if self.debt_ticks[tool] <= 0 {
                return Err(failed(
                    "routed shutdown must leave both feeders explicitly retracted",
                ));
            }
            self.accounting[tool].outstanding_retraction_mm =
                self.debt_ticks[tool] as f64 / 100_000_000.0;
        }
        let report = FixedDualVerification {
            selection_count: self.selection_count,
            tools: self.accounting.clone(),
        };
        report.validate()?;
        Ok(report)
    }
}
