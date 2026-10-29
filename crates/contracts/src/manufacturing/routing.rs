// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Finite fixed-hotend software routing. Declarations are not hardware evidence.
use super::*;

pub const FIXED_DUAL_FIRMWARE_REVISION: &str = "e5a167aaa0a671ea6d9b9d98dc0518c88d5caed7";
pub const MAX_TOOL_SELECTIONS: u32 = 50_000;

/// The reviewed, deliberately non-extensible firmware branch contract.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FixedDualConfiguration {
    Marlin2125NativeFixedHotends,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LoadedMaterial {
    pub id: String,
    pub revision: String,
    /// Identity of the inert material declaration, not a measured color model.
    pub declaration_hash: SourceHash,
    pub polymer_family: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FixedDualTool {
    pub id: String,
    pub feeder_id: String,
    pub material: LoadedMaterial,
    /// Physical nozzle displacement from tool zero; Z is identically zero.
    pub offset_xy_mm: [f64; 2],
    pub nozzle_diameter_mm: f64,
    pub filament_diameter_mm: f64,
    pub deposition_limits: DepositionLimits,
    pub max_feed_mm_s: f64,
    pub max_volumetric_flow_mm3_s: f64,
    pub nozzle_temperature_c: f64,
    pub standby_temperature_c: f64,
    pub minimum_extrusion_temperature_c: f64,
    pub maximum_nozzle_temperature_c: f64,
    pub retract_filament_mm: f64,
    pub retract_speed_mm_s: f64,
    /// Declared pressure-establishment minimum; actual waste is stroke-derived.
    pub minimum_reprime_volume_mm3: f64,
    /// One bed-rooted, repeated line, deposited once on every actual layer.
    pub waste_line_xy_mm: [[f64; 2]; 2],
}

/// Explicit exclusive build-space reservation, not an inferred source mask.
/// Compile must bind the complete native occurrence to this reservation.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OccurrenceMaterialAssignment {
    pub occurrence_id: OccurrenceId,
    pub material_id: String,
    pub work_envelope_mm: AabbMm,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FixedDualJob {
    pub machine_id: String,
    pub configuration: FixedDualConfiguration,
    pub firmware_revision: String,
    pub configuration_hash: SourceHash,
    pub calibration_record_hash: SourceHash,
    pub homing_contract: HomingCoordinateContract,
    pub home_position_mm: [f64; 3],
    pub bed_temperature_c: f64,
    pub maximum_bed_temperature_c: f64,
    pub tools: [FixedDualTool; 2],
    pub assignments: [OccurrenceMaterialAssignment; 2],
    pub transition_lift_mm: f64,
    /// Allowed tool-zero reference positions, including negative carriage X/Y.
    pub carriage_travel_bounds_mm: AabbMm,
    /// Occupied body relative to the tool-zero tip, excluding the two tips.
    pub carriage_body_bounds_mm: AabbMm,
    pub inactive_nozzle_radius_mm: f64,
    pub park_position_mm: [f64; 3],
}

fn finite_box(bounds: &AabbMm) -> Result<(), ManufacturingError> {
    bounds.validate().map_err(|e| invalid(&e.message))?;
    if (0..3).any(|axis| {
        bounds.min[axis] >= bounds.max[axis] || !(bounds.max[axis] - bounds.min[axis]).is_finite()
    }) {
        return Err(invalid("routing requires finite positive box dimensions"));
    }
    Ok(())
}
fn temperature(value: f64, maximum: f64) -> bool {
    value.is_finite() && value >= 0.0 && value <= maximum && value.fract() == 0.0
}

impl FixedDualJob {
    pub fn validate_declaration(&self) -> Result<(), ManufacturingError> {
        label(&self.machine_id)?;
        if self.firmware_revision != FIXED_DUAL_FIRMWARE_REVISION {
            return Err(invalid(
                "fixed-dual routing requires the reviewed Marlin 2.1.2.5 source revision",
            ));
        }
        positive(&[self.transition_lift_mm, self.inactive_nozzle_radius_mm])?;
        finite_box(&self.carriage_travel_bounds_mm)?;
        finite_box(&self.carriage_body_bounds_mm)?;
        if self.carriage_body_bounds_mm.min[2] <= 0.0
            || !temperature(self.bed_temperature_c, self.maximum_bed_temperature_c)
            || !temperature(self.maximum_bed_temperature_c, 200.0)
            || self.bed_temperature_c == 0.0
        {
            return Err(invalid(
                "invalid fixed-dual body clearance or bed temperature declaration",
            ));
        }
        for point in [self.home_position_mm, self.park_position_mm] {
            if point.iter().any(|value| !value.is_finite()) {
                return Err(invalid("fixed-dual home and park must be finite"));
            }
        }
        if self.tools[0].offset_xy_mm != [0.0, 0.0]
            || self.tools[1].offset_xy_mm == [0.0, 0.0]
            || self.tools[0].id == self.tools[1].id
            || self.tools[0].feeder_id == self.tools[1].feeder_id
            || self.tools[0].material.id == self.tools[1].material.id
        {
            return Err(invalid(
                "fixed-dual routing requires distinct tools, feeders and materials, with tool zero as the offset origin",
            ));
        }
        // The first whole-occurrence route has no independently admitted bond
        // policy for different polymers. This is a declaration, not adhesion proof.
        if self.tools[0].material.polymer_family != self.tools[1].material.polymer_family {
            return Err(invalid(
                "the reference route requires a declared common polymer family",
            ));
        }
        for tool in &self.tools {
            for name in [
                &tool.id,
                &tool.feeder_id,
                &tool.material.id,
                &tool.material.revision,
                &tool.material.polymer_family,
            ] {
                label(name)?;
            }
            positive(&[
                tool.nozzle_diameter_mm,
                tool.filament_diameter_mm,
                tool.max_feed_mm_s,
                tool.max_volumetric_flow_mm3_s,
                tool.retract_filament_mm,
                tool.retract_speed_mm_s,
                tool.minimum_reprime_volume_mm3,
            ])?;
            tool.deposition_limits.validate()?;
            if tool
                .offset_xy_mm
                .iter()
                .chain(tool.waste_line_xy_mm.iter().flatten())
                .any(|value| !value.is_finite())
                || tool.waste_line_xy_mm[0] == tool.waste_line_xy_mm[1]
                || !temperature(
                    tool.minimum_extrusion_temperature_c,
                    tool.maximum_nozzle_temperature_c,
                )
                || !temperature(tool.nozzle_temperature_c, tool.maximum_nozzle_temperature_c)
                || !temperature(
                    tool.standby_temperature_c,
                    tool.maximum_nozzle_temperature_c,
                )
                || !temperature(tool.maximum_nozzle_temperature_c, 500.0)
                || tool.minimum_extrusion_temperature_c == 0.0
                || tool.nozzle_temperature_c < tool.minimum_extrusion_temperature_c
                || tool.standby_temperature_c > tool.nozzle_temperature_c
                || tool.retract_filament_mm > 10.0
                || tool.retract_speed_mm_s > tool.max_feed_mm_s
            {
                return Err(invalid(
                    "invalid fixed-dual offset, waste, thermal or pressure declaration",
                ));
            }
        }
        if self.assignments[0].occurrence_id == self.assignments[1].occurrence_id {
            return Err(invalid("each routed occurrence must have one assignment"));
        }
        for assignment in &self.assignments {
            label(&assignment.material_id)?;
            finite_box(&assignment.work_envelope_mm)?;
            self.tool_for_material(&assignment.material_id)?;
        }
        let a = &self.assignments[0].work_envelope_mm;
        let b = &self.assignments[1].work_envelope_mm;
        if !(0..2).any(|axis| a.max[axis] < b.min[axis] || b.max[axis] < a.min[axis]) {
            return Err(invalid(
                "routed occurrence work envelopes must be strictly separated in XY",
            ));
        }
        Ok(())
    }

    pub fn tool_for_material(&self, material_id: &str) -> Result<u8, ManufacturingError> {
        self.tools
            .iter()
            .position(|tool| tool.material.id == material_id)
            .map(|index| index as u8)
            .ok_or_else(|| invalid("routed material is not loaded in either declared feeder"))
    }

    /// Bind the complete projected bead to one authored exclusive reservation.
    /// The planner separately proves each reservation's native source binding.
    pub fn assignment_for_path(&self, path: &DepositionPath) -> Result<usize, ManufacturingError> {
        let mut found = None;
        for (index, assignment) in self.assignments.iter().enumerate() {
            let bounds = &assignment.work_envelope_mm;
            if path.physical_z_mm[0] < bounds.min[2] || path.physical_z_mm[1] > bounds.max[2] {
                continue;
            }
            let mut contains = true;
            for edge in path.checked_edges()? {
                for point in edge.points_mm {
                    for axis in 0..2 {
                        let coordinate =
                            program_coordinates::project_xyz6_coordinate_mm(point[axis])?;
                        let error = program_coordinates::xyz6_coordinate_error_mm(point[axis])?;
                        let radius = (edge.width_mm * 0.5 + error).next_up();
                        if (coordinate - radius).next_down() < bounds.min[axis]
                            || (coordinate + radius).next_up() > bounds.max[axis]
                        {
                            contains = false;
                        }
                    }
                }
            }
            if contains {
                if found.is_some() {
                    return Err(invalid(
                        "a complete routed bead has multiple source reservations",
                    ));
                }
                found = Some(index);
            }
        }
        found.ok_or_else(|| {
            invalid("a complete routed bead is outside every assigned source reservation")
        })
    }

    pub fn tool_for_path(&self, path: &DepositionPath) -> Result<u8, ManufacturingError> {
        let assignment = &self.assignments[self.assignment_for_path(path)?];
        self.tool_for_material(&assignment.material_id)
    }

    pub fn waste_end_mm(&self, tool: u8, z: f64) -> [f64; 3] {
        let xy = self.tools[usize::from(tool)].waste_line_xy_mm[1];
        [xy[0], xy[1], z]
    }

    pub fn validate(&self, printer: &PrinterSpecification) -> Result<(), ManufacturingError> {
        self.validate_declaration()?;
        for tool in &self.tools {
            if printer.nozzle_diameter_mm != tool.nozzle_diameter_mm
                || printer.max_feed_mm_s > tool.max_feed_mm_s
                || printer.max_volumetric_flow_mm3_s > tool.max_volumetric_flow_mm3_s
                || printer.deposition_limits.min_bead_width_mm
                    < tool.deposition_limits.min_bead_width_mm
                || printer.deposition_limits.max_bead_width_mm
                    > tool.deposition_limits.max_bead_width_mm
                || printer.deposition_limits.min_bead_height_mm
                    < tool.deposition_limits.min_bead_height_mm
                || printer.deposition_limits.max_bead_height_mm
                    > tool.deposition_limits.max_bead_height_mm
            {
                return Err(invalid(
                    "shared planning envelope exceeds a declared loaded tool envelope",
                ));
            }
        }
        for point in [self.home_position_mm, self.park_position_mm] {
            if (0..3).any(|axis| {
                point[axis] < printer.build_envelope.min[axis]
                    || point[axis] > printer.build_envelope.max[axis]
            }) {
                return Err(invalid(
                    "fixed-dual home or park exceeds the command envelope",
                ));
            }
        }
        for assignment in &self.assignments {
            // A conservative native source enclosure may extend below the bed.
            // That is metadata, not permission for a negative-Z emitted bead.
            if (0..2).any(|axis| {
                assignment.work_envelope_mm.min[axis] < printer.build_envelope.min[axis]
                    || assignment.work_envelope_mm.max[axis] > printer.build_envelope.max[axis]
            }) || assignment.work_envelope_mm.max[2] > printer.build_envelope.max[2]
            {
                return Err(invalid(
                    "routed work reservation exceeds the command envelope",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RoutedToolAccounting {
    pub tool: u8,
    pub material_id: String,
    pub model_deposition_segments: u32,
    pub model_filament_length_mm: f64,
    pub model_volume_mm3: f64,
    pub waste_deposition_segments: u32,
    pub waste_filament_length_mm: f64,
    pub waste_volume_mm3: f64,
    pub outstanding_retraction_mm: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FixedDualVerification {
    pub selection_count: u32,
    pub tools: [RoutedToolAccounting; 2],
}
impl FixedDualVerification {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        if self.selection_count == 0 || self.selection_count > MAX_TOOL_SELECTIONS {
            return Err(invalid("invalid decoded fixed-dual selection count"));
        }
        let mut segments = 0u32;
        for (index, tool) in self.tools.iter().enumerate() {
            label(&tool.material_id)?;
            if tool.tool != index as u8
                || tool.model_deposition_segments == 0
                || tool.waste_deposition_segments == 0
                || [
                    tool.model_filament_length_mm,
                    tool.model_volume_mm3,
                    tool.waste_filament_length_mm,
                    tool.waste_volume_mm3,
                ]
                .iter()
                .any(|value| !value.is_finite() || *value <= 0.0)
                || !tool.outstanding_retraction_mm.is_finite()
                || tool.outstanding_retraction_mm < 0.0
            {
                return Err(invalid("invalid decoded fixed-dual material accounting"));
            }
            segments = segments
                .checked_add(tool.model_deposition_segments)
                .and_then(|value| value.checked_add(tool.waste_deposition_segments))
                .ok_or_else(|| resource("routed segment count overflow"))?;
        }
        if segments > MAX_DEPOSITION_SEGMENTS {
            return Err(resource(
                "model and waste share the existing deposition segment budget",
            ));
        }
        Ok(())
    }
}
