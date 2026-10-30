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

/// Whole-occurrence material authority, independent of mutable layer indices.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OccurrenceMaterial {
    Uniform {
        material_id: String,
    },
    LayerCycle {
        recipe: crate::optical::LayerCycle,
        material_ids: [String; 2],
    },
}

// Exactly the arithmetic endpoint identity used by layer_schedule, not a
// geometric tolerance or permission to span a physical material boundary.
fn same_cycle_boundary(a: f64, b: f64) -> bool {
    (a - b).abs() <= 4.0 * f64::EPSILON * a.abs().max(b.abs()).max(1.0)
}

const MAX_CYCLE_ORDINAL: i64 = 1_i64 << 52;

fn cycle_height(recipe: &crate::optical::LayerCycle) -> Result<f64, ManufacturingError> {
    let Some(first) = recipe.slots.first() else {
        return Err(invalid("routed cycle requires 1..64 slots"));
    };
    if recipe.slots.len() > 64
        || usize::from(recipe.phase_slot) >= recipe.slots.len()
        || !recipe.origin_z_mm.is_finite()
        || !first.height_mm.is_finite()
        || first.height_mm <= 0.0
        || recipe
            .slots
            .iter()
            .any(|slot| slot.material > 1 || slot.height_mm != first.height_mm)
    {
        return Err(invalid(
            "routed cycle requires a finite origin, valid phase and equal positive heights with material indices 0/1",
        ));
    }
    Ok(first.height_mm)
}

fn cycle_seed(
    recipe: &crate::optical::LayerCycle,
    height: f64,
    z: f64,
) -> Result<i64, ManufacturingError> {
    let relative = z - recipe.origin_z_mm;
    let quotient = relative / height;
    if !z.is_finite()
        || !relative.is_finite()
        || !quotient.is_finite()
        || quotient.abs() > MAX_CYCLE_ORDINAL as f64
    {
        return Err(invalid("routed cycle ordinal is not exactly representable"));
    }
    Ok(quotient.floor() as i64)
}

fn cycle_slot_bounds(
    recipe: &crate::optical::LayerCycle,
    height: f64,
    ordinal: i64,
) -> Result<[f64; 2], ManufacturingError> {
    if ordinal < -MAX_CYCLE_ORDINAL || ordinal >= MAX_CYCLE_ORDINAL {
        return Err(invalid(
            "routed cycle boundary ordinal exceeds exact arithmetic",
        ));
    }
    let lower = (ordinal as f64).mul_add(height, recipe.origin_z_mm);
    let upper = ((ordinal + 1) as f64).mul_add(height, recipe.origin_z_mm);
    if !lower.is_finite()
        || !upper.is_finite()
        || lower >= upper
        || same_cycle_boundary(lower, upper)
    {
        return Err(invalid("routed cycle world boundaries collapse"));
    }
    Ok([lower, upper])
}

impl OccurrenceMaterial {
    pub(crate) fn validate_for_z_bounds(&self, z: [f64; 2]) -> Result<(), ManufacturingError> {
        match self {
            Self::Uniform { material_id } => label(material_id),
            Self::LayerCycle {
                recipe,
                material_ids,
            } => {
                for id in material_ids {
                    label(id)?;
                }
                if material_ids[0] == material_ids[1] {
                    return Err(invalid("routed cycle requires distinct material bindings"));
                }
                let height = cycle_height(recipe)?;
                if !z[0].is_finite() || !z[1].is_finite() || z[0] >= z[1] {
                    return Err(invalid("routed cycle requires finite positive Z bounds"));
                }
                // Endpoints need not share a slot; the planner inserts the
                // intervening grid boundaries into its existing event schedule.
                self.next_boundary_after(z[0])?;
                let seed = cycle_seed(recipe, height, z[1])?;
                let mut owns_terminal = false;
                for ordinal in seed - 2..=seed + 2 {
                    if ordinal < -MAX_CYCLE_ORDINAL || ordinal >= MAX_CYCLE_ORDINAL {
                        continue;
                    }
                    let [lower, upper] = cycle_slot_bounds(recipe, height, ordinal)?;
                    if z[1] > lower
                        && !same_cycle_boundary(z[1], lower)
                        && (z[1] <= upper || same_cycle_boundary(z[1], upper))
                    {
                        if owns_terminal {
                            return Err(invalid("routed cycle terminal ownership is ambiguous"));
                        }
                        owns_terminal = true;
                    }
                }
                if !owns_terminal {
                    return Err(invalid(
                        "routed cycle terminal exceeds its exact slot domain",
                    ));
                }
                Ok(())
            }
        }
    }

    pub fn material_id_for_interval(
        &self,
        physical_z_mm: [f64; 2],
    ) -> Result<&str, ManufacturingError> {
        let (recipe, material_ids) = match self {
            Self::Uniform { material_id } => return Ok(material_id),
            Self::LayerCycle {
                recipe,
                material_ids,
            } => (recipe, material_ids),
        };
        let height = cycle_height(recipe)?;
        let [start, end] = physical_z_mm;
        if !start.is_finite() || !end.is_finite() || start >= end || same_cycle_boundary(start, end)
        {
            return Err(invalid("routed cycle physical interval collapses"));
        }
        let seed = cycle_seed(recipe, height, start)?;
        cycle_seed(recipe, height, end)?;
        let mut found = None;
        // Rounded subtraction/division can put floor on either side of an
        // endpoint. Recompute a fixed neighborhood in the physical world frame.
        for ordinal in seed - 2..=seed + 2 {
            if ordinal < -MAX_CYCLE_ORDINAL || ordinal >= MAX_CYCLE_ORDINAL {
                continue;
            }
            let [lower, upper] = cycle_slot_bounds(recipe, height, ordinal)?;
            if (start >= lower || same_cycle_boundary(start, lower))
                && (end <= upper || same_cycle_boundary(end, upper))
            {
                if found.replace(ordinal).is_some() {
                    return Err(invalid(
                        "routed cycle interval has ambiguous slot ownership",
                    ));
                }
            }
        }
        let ordinal = found.ok_or_else(|| invalid("routed interval crosses a cycle boundary"))?;
        let count = recipe.slots.len() as i64;
        let slot = (i64::from(recipe.phase_slot) + ordinal.rem_euclid(count)) % count;
        Ok(&material_ids[usize::from(recipe.slots[slot as usize].material)])
    }

    pub fn next_boundary_after(&self, z_mm: f64) -> Result<Option<f64>, ManufacturingError> {
        let Self::LayerCycle { recipe, .. } = self else {
            return Ok(None);
        };
        let height = cycle_height(recipe)?;
        let seed = cycle_seed(recipe, height, z_mm)?;
        let mut found = None;
        for ordinal in seed - 2..=seed + 2 {
            if ordinal < -MAX_CYCLE_ORDINAL || ordinal >= MAX_CYCLE_ORDINAL {
                continue;
            }
            let [lower, upper] = cycle_slot_bounds(recipe, height, ordinal)?;
            // A point arithmetically identical to an upper endpoint belongs to
            // the following slot, so this always advances distinguishably.
            if (z_mm >= lower || same_cycle_boundary(z_mm, lower))
                && z_mm < upper
                && !same_cycle_boundary(z_mm, upper)
            {
                if found.replace(upper).is_some() {
                    return Err(invalid(
                        "routed cycle boundary has ambiguous slot ownership",
                    ));
                }
            }
        }
        found
            .map(Some)
            .ok_or_else(|| invalid("routed cycle cannot make representable boundary progress"))
    }
}

/// Explicit exclusive build-space reservation, not an inferred source mask.
/// Compile must bind the complete native occurrence to this reservation.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OccurrenceMaterialAssignment {
    pub occurrence_id: OccurrenceId,
    pub material: OccurrenceMaterial,
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
    #[serde(deserialize_with = "assignments")]
    pub assignments: Vec<OccurrenceMaterialAssignment>,
    #[serde(deserialize_with = "construction::constructions")]
    pub surface_constructions: Vec<SurfaceColorConstruction>,
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
        if self.assignments.is_empty() || self.assignments.len() > 2 {
            return Err(invalid(
                "fixed-dual job requires one or two assigned occurrences",
            ));
        }
        if self.assignments.len() == 2
            && self.assignments[0].occurrence_id == self.assignments[1].occurrence_id
        {
            return Err(invalid("each routed occurrence must have one assignment"));
        }
        for assignment in &self.assignments {
            assignment.material.validate_for_z_bounds([
                assignment.work_envelope_mm.min[2],
                assignment.work_envelope_mm.max[2],
            ])?;
            finite_box(&assignment.work_envelope_mm)?;
            match &assignment.material {
                OccurrenceMaterial::Uniform { material_id } => {
                    self.tool_for_material(material_id)?;
                }
                OccurrenceMaterial::LayerCycle { material_ids, .. } => {
                    for material_id in material_ids {
                        self.tool_for_material(material_id)?;
                    }
                }
            }
        }
        for (index, construction) in self.surface_constructions.iter().enumerate() {
            construction.validate(self)?;
            if self.surface_constructions[..index]
                .iter()
                .any(|c| c.occurrence_id == construction.occurrence_id)
            {
                return Err(invalid("one surface arrangement owns each occurrence"));
            }
        }
        if self.surface_constructions.len() > 16 {
            return Err(invalid("too many surface constructions"));
        }
        if self.assignments.len() == 2 {
            let a = &self.assignments[0].work_envelope_mm;
            let b = &self.assignments[1].work_envelope_mm;
            if !(0..2).any(|axis| a.max[axis] < b.min[axis] || b.max[axis] < a.min[axis]) {
                return Err(invalid(
                    "routed occurrence work envelopes must be strictly separated in XY",
                ));
            }
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
        if let Some(reference) = path.material_domain {
            let construction = self
                .surface_constructions
                .get(usize::from(reference.construction_index))
                .ok_or_else(|| invalid("path refers to a missing material construction"))?;
            let assignment = &self.assignments[self.assignment_for_path(path)?];
            if assignment.occurrence_id != construction.occurrence_id {
                return Err(invalid(
                    "material domain belongs to a different source occurrence",
                ));
            }
            return self.tool_for_material(match reference.role {
                MaterialConstructionRole::Shell => construction
                    .shell
                    .material_id_for_interval(path.physical_z_mm)?,
                MaterialConstructionRole::Backing | MaterialConstructionRole::Core => {
                    &construction.white_material_id
                }
            });
        }
        let assignment = &self.assignments[self.assignment_for_path(path)?];
        if self
            .surface_constructions
            .iter()
            .any(|c| c.occurrence_id == assignment.occurrence_id)
        {
            return Err(invalid(
                "constructed model bead has no material-domain owner",
            ));
        }
        self.tool_for_material(
            assignment
                .material
                .material_id_for_interval(path.physical_z_mm)?,
        )
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
                || tool.waste_deposition_segments == 0
                || [tool.waste_filament_length_mm, tool.waste_volume_mm3]
                    .iter()
                    .any(|value| !value.is_finite() || *value <= 0.0)
                || !tool.model_filament_length_mm.is_finite()
                || !tool.model_volume_mm3.is_finite()
                || (if tool.model_deposition_segments == 0 {
                    tool.model_filament_length_mm != 0.0 || tool.model_volume_mm3 != 0.0
                } else {
                    tool.model_filament_length_mm <= 0.0 || tool.model_volume_mm3 <= 0.0
                })
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

#[cfg(test)]
mod cycle_tests {
    use super::*;
    use crate::optical::{LayerCycle, OpticalSlot};

    #[test]
    fn terminal_cycle_extent_does_not_require_a_slot_after_its_end() {
        let material = cycle(0, -4_503_599_627_370_495.0, 1.0);
        assert_eq!(material.material_id_for_interval([0.0, 1.0]).unwrap(), "b");
        assert_eq!(material.next_boundary_after(0.0).unwrap(), Some(1.0));
        material.validate_for_z_bounds([0.0, 1.0]).unwrap();
        assert!(material.validate_for_z_bounds([0.0, 1.000001]).is_err());
    }

    fn cycle(phase_slot: u16, origin_z_mm: f64, height_mm: f64) -> OccurrenceMaterial {
        OccurrenceMaterial::LayerCycle {
            recipe: LayerCycle {
                slots: vec![
                    OpticalSlot {
                        material: 0,
                        height_mm,
                    },
                    OpticalSlot {
                        material: 1,
                        height_mm,
                    },
                ],
                phase_slot,
                origin_z_mm,
            },
            material_ids: ["a".into(), "b".into()],
        }
    }

    #[test]
    fn physical_phase_is_signed_and_independent_of_native_refinement() {
        let a = cycle(0, 0.5, 0.25);
        let b = cycle(1, 0.5, 0.25);
        for interval in [[0.0, 0.1], [0.1, 0.25], [0.5, 0.6], [0.6, 0.75]] {
            assert_eq!(a.material_id_for_interval(interval).unwrap(), "a");
            assert_eq!(b.material_id_for_interval(interval).unwrap(), "b");
        }
        assert_eq!(a.material_id_for_interval([0.25, 0.5]).unwrap(), "b");
        assert_eq!(a.material_id_for_interval([-0.25, 0.0]).unwrap(), "b");
    }

    #[test]
    fn only_arithmetic_identity_may_touch_a_neighboring_slot() {
        let material = cycle(0, 0.0, 0.1);
        let alternate_endpoint = 0.1 + 0.2;
        assert_eq!(
            material
                .material_id_for_interval([0.2, alternate_endpoint])
                .unwrap(),
            "a",
        );
        assert_eq!(
            material
                .material_id_for_interval([alternate_endpoint, 0.4])
                .unwrap(),
            "b",
        );
        assert!(
            material
                .material_id_for_interval([0.2, 0.3 + 1e-12])
                .is_err()
        );
        assert!(
            material
                .material_id_for_interval([0.3 - 1e-12, 0.4])
                .is_err()
        );
    }

    #[test]
    fn crossing_refuses_even_when_neighboring_materials_match() {
        let mut material = cycle(0, 0.0, 0.25);
        let OccurrenceMaterial::LayerCycle { recipe, .. } = &mut material else {
            unreachable!()
        };
        recipe.slots[1].material = 0;
        assert!(material.material_id_for_interval([0.125, 0.375]).is_err());
    }

    #[test]
    fn boundary_walk_uses_fused_world_endpoints_and_strict_progress() {
        let material = cycle(0, 0.03, 0.1);
        for ordinal in -8..8 {
            let lower = (ordinal as f64).mul_add(0.1, 0.03);
            let upper = ((ordinal + 1) as f64).mul_add(0.1, 0.03);
            let boundary = material.next_boundary_after(lower).unwrap().unwrap();
            assert_eq!(boundary.to_bits(), upper.to_bits());
            assert!(boundary > lower && !same_cycle_boundary(boundary, lower));
            let middle = lower + (upper - lower) * 0.5;
            assert_eq!(
                material
                    .next_boundary_after(middle)
                    .unwrap()
                    .unwrap()
                    .to_bits(),
                upper.to_bits(),
            );
        }
        // The ordinary accumulated-add encoding must not emit a duplicate event.
        assert_eq!(
            cycle(0, 0.0, 0.1).next_boundary_after(0.1 + 0.2).unwrap(),
            Some(0.4),
        );
    }

    #[test]
    fn unrepresentable_ordinals_and_collapsed_world_slots_refuse() {
        // Cancellation leaves small world coordinates but cannot restore exact
        // integer slot arithmetic beyond the signed 2^52 envelope.
        let out_of_range = cycle(0, -(1_i64 << 53) as f64, 1.0);
        assert!(out_of_range.material_id_for_interval([0.0, 0.5]).is_err());
        assert!(out_of_range.next_boundary_after(0.0).is_err());
        let collapsed = cycle(0, 1e16, 0.25);
        assert!(collapsed.next_boundary_after(1e16).is_err());
        let tiny = cycle(0, 0.0, f64::EPSILON);
        assert!(tiny.next_boundary_after(0.0).is_err());
        assert!(
            cycle(0, 0.0, 1.0)
                .material_id_for_interval([0.5, 0.5_f64.next_up()])
                .is_err()
        );
    }

    #[test]
    fn cancellation_uses_world_boundaries_without_shifting_phase() {
        let material = cycle(1, -(1_i64 << 50) as f64, 1.0);
        assert_eq!(material.material_id_for_interval([0.0, 0.5]).unwrap(), "b");
        assert_eq!(material.next_boundary_after(0.0).unwrap(), Some(1.0));
        assert_eq!(material.material_id_for_interval([1.0, 1.5]).unwrap(), "a");
    }

    #[test]
    fn uniform_height_envelope_does_not_change_optical_cycle_validation() {
        let mut material = cycle(0, 0.0, 0.25);
        let OccurrenceMaterial::LayerCycle { recipe, .. } = &mut material else {
            unreachable!()
        };
        recipe.slots[1].height_mm = 0.5;
        assert!(material.validate_for_z_bounds([0.0, 1.0]).is_err());
        assert!(material.next_boundary_after(0.0).is_err());
        assert!(material.material_id_for_interval([0.0, 0.1]).is_err());
    }

    #[test]
    fn rounded_origin_relative_quotient_does_not_choose_physical_slot() {
        let origin = -(1_i64 << 50) as f64;
        let height = 0.3;
        let material = OccurrenceMaterial::LayerCycle {
            recipe: LayerCycle {
                slots: vec![
                    OpticalSlot {
                        material: 0,
                        height_mm: height,
                    },
                    OpticalSlot {
                        material: 1,
                        height_mm: height,
                    },
                ],
                phase_slot: 0,
                origin_z_mm: origin,
            },
            material_ids: ["a".into(), "b".into()],
        };
        // Subtracting the large negative origin loses small world-coordinate
        // fractions. Ownership must instead use neighboring fused endpoints.
        for ordinal in 3_752_999_689_475_410_i64..3_752_999_689_475_417 {
            let lower = (ordinal as f64).mul_add(height, origin);
            let upper = ((ordinal + 1) as f64).mul_add(height, origin);
            let expected = if ordinal.rem_euclid(2) == 0 { "a" } else { "b" };
            assert_eq!(
                material
                    .material_id_for_interval([
                        lower + (upper - lower) * 0.2,
                        lower + (upper - lower) * 0.8,
                    ])
                    .unwrap(),
                expected,
            );
            assert_eq!(
                material
                    .next_boundary_after(lower)
                    .unwrap()
                    .unwrap()
                    .to_bits(),
                upper.to_bits(),
            );
        }
    }
}

fn assignments<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<OccurrenceMaterialAssignment>, D::Error> {
    bounded_vec::<D, _, 2>(d)
}
