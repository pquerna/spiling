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
    #[serde(deserialize_with = "required_option")]
    pub material_profile: Option<ResolvedMaterialProfile>,
    /// Physical nozzle displacement from tool zero; Z is identically zero.
    pub offset_xy_mm: [f64; 2],
    pub nozzle_diameter_mm: f64,
    pub filament_diameter_mm: f64,
    pub waste_bead_width_mm: f64,
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

impl FixedDualTool {
    pub fn process<'a>(&'a self, recipe: &'a PlanarPrintRecipe) -> &'a ProcessPolicy {
        self.material_profile
            .as_ref()
            .map_or(&recipe.process, |p| p.selection.process())
    }
    pub fn print_speed_mm_s(&self, recipe: &PlanarPrintRecipe) -> f64 {
        self.material_profile
            .as_ref()
            .map_or(recipe.print_speed_mm_s, |p| p.selection.print_speed_mm_s())
    }
    pub fn flow_multiplier(&self, recipe: &PlanarPrintRecipe) -> f64 {
        self.material_profile
            .as_ref()
            .map_or(recipe.flow_multiplier, |p| p.selection.flow_multiplier())
    }
    pub fn effective_max_volumetric_flow_mm3_s(&self, printer: &PrinterSpecification) -> f64 {
        self.material_profile
            .as_ref()
            .map_or(self.max_volumetric_flow_mm3_s, |p| {
                self.max_volumetric_flow_mm3_s
                    .min(p.selection.profile.max_volumetric_flow_mm3_s)
            })
            .min(printer.max_volumetric_flow_mm3_s)
    }
    pub fn admits_deposition(&self, width: f64, height: f64) -> bool {
        self.deposition_limits.admits(width, height)
            && self
                .material_profile
                .as_ref()
                .is_none_or(|p| p.selection.profile.deposition_limits.admits(width, height))
    }
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
    if !(-MAX_CYCLE_ORDINAL..MAX_CYCLE_ORDINAL).contains(&ordinal) {
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
                    if !(-MAX_CYCLE_ORDINAL..MAX_CYCLE_ORDINAL).contains(&ordinal) {
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
            if !(-MAX_CYCLE_ORDINAL..MAX_CYCLE_ORDINAL).contains(&ordinal) {
                continue;
            }
            let [lower, upper] = cycle_slot_bounds(recipe, height, ordinal)?;
            if (start >= lower || same_cycle_boundary(start, lower))
                && (end <= upper || same_cycle_boundary(end, upper))
                && found.replace(ordinal).is_some()
            {
                return Err(invalid(
                    "routed cycle interval has ambiguous slot ownership",
                ));
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
            if !(-MAX_CYCLE_ORDINAL..MAX_CYCLE_ORDINAL).contains(&ordinal) {
                continue;
            }
            let [lower, upper] = cycle_slot_bounds(recipe, height, ordinal)?;
            // A point arithmetically identical to an upper endpoint belongs to
            // the following slot, so this always advances distinguishably.
            if (z_mm >= lower || same_cycle_boundary(z_mm, lower))
                && z_mm < upper
                && !same_cycle_boundary(z_mm, upper)
                && found.replace(upper).is_some()
            {
                return Err(invalid(
                    "routed cycle boundary has ambiguous slot ownership",
                ));
            }
        }
        found
            .map(Some)
            .ok_or_else(|| invalid("routed cycle cannot make representable boundary progress"))
    }
}

/// Eligible interior routing is separate from product identity and external skins.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InteriorToolBinding {
    /// Core-stamped semantic context of a deliberately created/re-enabled pin.
    #[serde(deserialize_with = "required_option")]
    pub policy_context: Option<SourceHash>,
    #[serde(deserialize_with = "required_option")]
    pub policy_override: Option<InteriorToolOverride>,
    pub tool_id: String,
    pub bead_width_mm: f64,
}

/// Explicit exclusive build-space reservation, not an inferred source mask.
/// Compile must bind the complete native occurrence to this reservation.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OccurrenceMaterialAssignment {
    pub occurrence_id: OccurrenceId,
    /// Explicit loaded tool for a uniform assignment. Null requires unambiguous
    /// loaded material identity; material-domain/cycle routing owns its selection.
    #[serde(deserialize_with = "required_option")]
    pub tool_id: Option<String>,
    #[serde(deserialize_with = "required_option")]
    pub interior: Option<InteriorToolBinding>,
    pub material: OccurrenceMaterial,
    pub work_envelope_mm: AabbMm,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FixedDualJob {
    #[serde(deserialize_with = "required_option")]
    pub interior_goal: Option<InteriorSelectionGoal>,
    #[serde(deserialize_with = "required_option")]
    pub timing_model: Option<DeclaredJobTimeModel>,
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
    #[serde(deserialize_with = "required_option")]
    pub functional_construction: Option<FunctionalMaterialConstruction>,
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
    pub fn has_bound_interior(&self) -> bool {
        self.assignments.len() == 1
            && self.assignments[0].interior.is_some()
            && self.surface_constructions.is_empty()
            && self.functional_construction.is_none()
    }
    pub fn validate_declaration(&self) -> Result<(), ManufacturingError> {
        if let Some(goal) = &self.interior_goal {
            goal.validate()?;
        }
        if let Some(model) = &self.timing_model {
            model.validate()?;
        }
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
        {
            return Err(invalid(
                "fixed-dual routing requires distinct tools and feeders, with tool zero as the offset origin",
            ));
        }
        if self.tools[0].material.id == self.tools[1].material.id
            && self.tools[0].material != self.tools[1].material
        {
            return Err(invalid(
                "the same material identity has conflicting loaded declarations",
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
                tool.waste_bead_width_mm,
                tool.max_feed_mm_s,
                tool.max_volumetric_flow_mm3_s,
                tool.retract_filament_mm,
                tool.retract_speed_mm_s,
                tool.minimum_reprime_volume_mm3,
            ])?;
            tool.deposition_limits.validate()?;
            if let Some(profile) = &tool.material_profile {
                profile.validate()?;
            }
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
                    self.resolve_tool(material_id, assignment.tool_id.as_deref())?;
                }
                OccurrenceMaterial::LayerCycle { material_ids, .. } => {
                    if assignment.tool_id.is_some() {
                        return Err(invalid("a material cycle cannot pin one uniform tool"));
                    }
                    for material_id in material_ids {
                        self.tool_for_material(material_id)?;
                    }
                }
            }
        }
        for assignment in &self.assignments {
            if let Some(interior) = &assignment.interior {
                if self.assignments.len() != 1
                    || !self.surface_constructions.is_empty()
                    || self.functional_construction.is_some()
                {
                    return Err(invalid(
                        "mixed-width interior currently requires one uniform native source without material construction",
                    ));
                }
                let OccurrenceMaterial::Uniform { material_id } = &assignment.material else {
                    return Err(invalid(
                        "mixed-width interior requires a uniform formulation",
                    ));
                };
                let primary = self.resolve_tool(material_id, assignment.tool_id.as_deref())?;
                let bulk = self.resolve_tool(material_id, Some(&interior.tool_id))?;
                positive(&[interior.bead_width_mm])?;
                if primary == bulk {
                    return Err(invalid(
                        "interior tool must be distinct from the captured primary tool",
                    ));
                }
            }
        }
        for assignment in &self.assignments {
            if assignment.tool_id.is_some()
                && (self
                    .surface_constructions
                    .iter()
                    .any(|c| c.occurrence_id == assignment.occurrence_id)
                    || self
                        .functional_construction
                        .as_ref()
                        .is_some_and(|c| c.occurrence_id == assignment.occurrence_id))
            {
                return Err(invalid(
                    "material constructions retain their own tool bindings",
                ));
            }
        }
        for (index, construction) in self.surface_constructions.iter().enumerate() {
            construction.validate(self)?;
            if self.surface_constructions[..index].iter().any(|c| {
                c.occurrence_id == construction.occurrence_id
                    && (c.definition_id != construction.definition_id
                        || c.source_hash != construction.source_hash
                        || c.white_material_id != construction.white_material_id)
            }) {
                return Err(invalid(
                    "regional constructions must share immutable source and white base authority",
                ));
            }
        }
        if self.surface_constructions.len() > 16 {
            return Err(invalid("too many surface constructions"));
        }
        if let Some(construction) = &self.functional_construction {
            construction.validate(self)?;
        }
        let uniform_without_construction = self.functional_construction.is_none()
            && self.surface_constructions.is_empty()
            && self.assignments.iter().all(|assignment| {
                matches!(&assignment.material, OccurrenceMaterial::Uniform { .. })
            });
        if self.tools[0].material.polymer_family != self.tools[1].material.polymer_family
            && !uniform_without_construction
            && !matches!(
                &self.functional_construction,
                Some(FunctionalMaterialConstruction {
                    interface_policy: FunctionalInterfacePolicy::MechanicalCapture,
                    ..
                })
            )
        {
            return Err(invalid(
                "the reference route requires a declared common polymer family unless all occurrences are uniform without construction or a complete functional construction explicitly requests mechanical capture",
            ));
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
        self.resolve_tool(material_id, None)
    }

    /// Resolve a captured explicit tool, or require exactly one material owner.
    /// Never choose the first loaded copy of a formulation.
    pub fn resolve_tool(
        &self,
        material_id: &str,
        tool_id: Option<&str>,
    ) -> Result<u8, ManufacturingError> {
        if let Some(id) = tool_id {
            label(id)?;
        }
        let mut found = None;
        for (index, tool) in self.tools.iter().enumerate() {
            if tool.material.id == material_id
                && tool_id.is_none_or(|id| tool.id == id)
                && found.replace(index as u8).is_some()
            {
                return Err(invalid(
                    "material is loaded in multiple tools; an explicit tool binding is required",
                ));
            }
        }
        found.ok_or_else(|| invalid("the selected tool does not contain the captured material"))
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
                    for (axis, &value) in point.iter().take(2).enumerate() {
                        let coordinate = program_coordinates::project_xyz6_coordinate_mm(value)?;
                        let error = program_coordinates::xyz6_coordinate_error_mm(value)?;
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
        if let Some(construction) = &self.functional_construction {
            construction.validate(self)?;
            let reference = path
                .material_domain
                .ok_or_else(|| invalid("functional bead has no material-domain owner"))?;
            if reference.construction_index != 0
                || self.assignments[self.assignment_for_path(path)?].occurrence_id
                    != construction.occurrence_id
            {
                return Err(invalid(
                    "functional bead has a different construction or occurrence owner",
                ));
            }
            return self.tool_for_material(match reference.role {
                MaterialConstructionRole::FunctionalRegion => &construction.material_id,
                MaterialConstructionRole::FunctionalBase => &construction.base_material_id,
                _ => {
                    return Err(invalid(
                        "optical material role cannot own a functional bead",
                    ));
                }
            });
        }
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
                MaterialConstructionRole::FunctionalRegion
                | MaterialConstructionRole::FunctionalBase => {
                    return Err(invalid(
                        "functional material role has no functional construction",
                    ));
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
        if let Some(interior) = assignment.active_interior()
            && matches!(path.owner, Some(DepositionOwner::Regional { .. }))
        {
            if path.width.constant_width_mm() != Some(interior.bead_width_mm) {
                return Err(invalid(
                    "interior paths must be generated at their captured tool width",
                ));
            }
            return self.resolve_tool(
                assignment
                    .material
                    .material_id_for_interval(path.physical_z_mm)?,
                Some(&interior.tool_id),
            );
        }
        self.resolve_tool(
            assignment
                .material
                .material_id_for_interval(path.physical_z_mm)?,
            assignment.tool_id.as_deref(),
        )
    }

    pub fn waste_end_mm(&self, tool: u8, z: f64) -> [f64; 3] {
        let xy = self.tools[usize::from(tool)].waste_line_xy_mm[1];
        [xy[0], xy[1], z]
    }

    pub fn validate(&self, printer: &PrinterSpecification) -> Result<(), ManufacturingError> {
        self.validate_declaration()?;
        let mut cooling = None;
        for tool in &self.tools {
            if let Some(capture) = &tool.material_profile {
                let selected = &capture.selection;
                let profile = &selected.profile;
                if profile.printer_id != printer.id
                    || profile.printer_revision != printer.revision
                    || profile.nozzle_diameter_mm != tool.nozzle_diameter_mm
                    || profile.material.id != tool.material.id
                    || profile.material.revision != tool.material.revision
                    || profile.material.polymer_family != tool.material.polymer_family
                    || profile.material.filament_diameter_mm != tool.filament_diameter_mm
                    || selected.nozzle_temperature_c() != tool.nozzle_temperature_c
                    || selected.bed_temperature_c() != self.bed_temperature_c
                {
                    return Err(invalid(
                        "tool profile disagrees with captured material, printer, nozzle or thermal targets",
                    ));
                }
                if !matches!(selected.process().retraction, RetractionPolicy::Disabled) {
                    return Err(invalid(
                        "tool profile body retraction is unsupported; captured tool selections own feeder recovery",
                    ));
                }
                if cooling.is_some_and(|id| id != &profile.cooling_configuration_id) {
                    return Err(invalid(
                        "fixed-dual tool profiles require one declared shared cooling configuration",
                    ));
                }
                cooling = Some(&profile.cooling_configuration_id);
            }
        }
        if printer.nozzle_diameter_mm != self.tools[0].nozzle_diameter_mm {
            return Err(invalid(
                "shared wall planning nozzle must match captured tool zero",
            ));
        }
        for tool in &self.tools {
            if printer.max_feed_mm_s > tool.max_feed_mm_s {
                return Err(invalid(
                    "shared travel envelope exceeds a captured tool feed limit",
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

fn assignments<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<OccurrenceMaterialAssignment>, D::Error> {
    bounded_vec::<D, _, 2>(d)
}

#[cfg(test)]
mod uniform_family_tests {
    use super::*;

    const UNIFORM: &str = include_str!("../../../../fixtures/manufacturing/fixed-dual.intent.json");
    const OPTICAL: &str =
        include_str!("../../../../fixtures/manufacturing/surface-color-box.intent.json");
    const CYCLE: &str =
        include_str!("../../../../fixtures/manufacturing/fixed-dual-cycle.intent.json");

    fn captured(json: &str) -> (PrinterSpecification, FixedDualJob) {
        let intent: ManufacturingIntent = serde_json::from_str(json).unwrap();
        let JobWorkflow::FixedDualMarlin { job } = &intent.printer.workflow else {
            panic!("expected fixed-dual fixture");
        };
        let job = job.clone();
        (intent.printer, job)
    }

    #[test]
    fn identical_formulation_on_two_tools_requires_exact_uniform_bindings() {
        let (printer, mut job) = captured(UNIFORM);
        job.tools[1].material = job.tools[0].material.clone();
        for (index, assignment) in job.assignments.iter_mut().enumerate() {
            assignment.material = OccurrenceMaterial::Uniform {
                material_id: job.tools[0].material.id.clone(),
            };
            assignment.tool_id = Some(job.tools[index].id.clone());
        }
        job.validate(&printer).unwrap();
        assert_eq!(job.tool_for_path(&bead(&job.assignments[0])).unwrap(), 0);
        assert_eq!(job.tool_for_path(&bead(&job.assignments[1])).unwrap(), 1);
        assert!(job.tool_for_material(&job.tools[0].material.id).is_err());
        let mut invalid = job.clone();
        invalid.assignments[1].tool_id = None;
        assert!(invalid.validate(&printer).is_err());
        let mut invalid = job.clone();
        invalid.assignments[1].tool_id = Some("missing".into());
        assert!(invalid.validate(&printer).is_err());
        let mut invalid = job.clone();
        invalid.tools[1].material.revision = "different".into();
        assert!(invalid.validate(&printer).is_err());
        let mut invalid = job;
        invalid.assignments[1].material = OccurrenceMaterial::Uniform {
            material_id: "foreign".into(),
        };
        assert!(invalid.validate(&printer).is_err());
    }

    #[test]
    fn uniform_tool_binding_is_strict_persisted_content() {
        let mut json: serde_json::Value = serde_json::from_str(UNIFORM).unwrap();
        json["printer"]["workflow"]["job"]["assignments"][0]
            .as_object_mut()
            .unwrap()
            .remove("tool_id");
        assert!(serde_json::from_value::<ManufacturingIntent>(json).is_err());
    }

    fn uniform(mixed: bool, bases: &[usize]) -> (PrinterSpecification, FixedDualJob) {
        let (printer, mut job) = captured(UNIFORM);
        job.assignments.truncate(bases.len());
        for (assignment, &base) in job.assignments.iter_mut().zip(bases) {
            assignment.material = OccurrenceMaterial::Uniform {
                material_id: job.tools[base].material.id.clone(),
            };
        }
        if mixed {
            job.tools[1].material.polymer_family = "TPU".into();
        }
        (printer, job)
    }

    fn functional(job: &FixedDualJob, base: usize) -> FunctionalMaterialConstruction {
        let (_, optical) = captured(OPTICAL);
        let source = &optical.surface_constructions[0];
        FunctionalMaterialConstruction {
            face_patch: None,
            interface_policy: FunctionalInterfacePolicy::MechanicalCapture,
            occurrence_id: job.assignments[0].occurrence_id,
            definition_id: source.definition_id.clone(),
            source_hash: source.source_hash.clone(),
            face_id: source.faces[0].face_id.clone(),
            material_id: job.tools[1 - base].material.id.clone(),
            base_material_id: job.tools[base].material.id.clone(),
            minimum_pure_depth_mm: 2.0,
            join: PlanarTKeyJoin {
                reach_into_region_mm: 2.0,
                reach_into_base_mm: 3.0,
                pitch_mm: 10.0,
                stem_width_mm: 2.0,
                head_width_mm: 6.0,
                head_depth_mm: 1.0,
                edge_margin_mm: 1.0,
                end_collar_height_mm: 1.0,
                collar_head_width_mm: 4.0,
            },
        }
    }

    fn bead(assignment: &OccurrenceMaterialAssignment) -> DepositionPath {
        let bounds = &assignment.work_envelope_mm;
        let x = (bounds.min[0] + bounds.max[0]) * 0.5;
        let y = (bounds.min[1] + bounds.max[1]) * 0.5;
        DepositionPath {
            material_domain: None,
            kind: DepositionKind::SolidFill,
            owner: None,
            width: WidthProfile::Constant { width_mm: 0.4 },
            physical_z_mm: [0.0, 0.2],
            points_mm: vec![[x - 1.0, y, 0.2], [x + 1.0, y, 0.2]],
        }
    }

    #[test]
    fn uniform_one_or_two_occurrences_admit_either_loaded_family() {
        for mixed in [false, true] {
            // A single occurrence leaves the other loaded tool unused by model
            // beads; both declarations must still satisfy the complete route.
            for bases in [&[0][..], &[1], &[0, 0], &[0, 1], &[1, 0], &[1, 1]] {
                let (printer, job) = uniform(mixed, bases);
                job.validate_declaration().unwrap();
                job.validate(&printer).unwrap();
                for (index, assignment) in job.assignments.iter().enumerate() {
                    let path = bead(assignment);
                    assert_eq!(job.assignment_for_path(&path).unwrap(), index);
                    assert_eq!(job.tool_for_path(&path).unwrap(), bases[index] as u8);
                }
            }
        }
    }

    #[test]
    fn family_contact_requires_complete_mechanical_capture_and_removal_restores_uniform() {
        for base in [0, 1] {
            for mixed in [false, true] {
                for policy in [
                    FunctionalInterfacePolicy::DeclaredSameFamily,
                    FunctionalInterfacePolicy::MechanicalCapture,
                ] {
                    let (printer, mut job) = uniform(mixed, &[base]);
                    let mut construction = functional(&job, base);
                    construction.interface_policy = policy;
                    job.functional_construction = Some(construction);
                    assert_eq!(
                        job.validate(&printer).is_ok(),
                        !mixed || policy == FunctionalInterfacePolicy::MechanicalCapture,
                        "base {base}, mixed {mixed}, policy {policy:?}",
                    );
                    job.functional_construction = None;
                    job.validate(&printer).unwrap();
                    assert_eq!(
                        job.tool_for_path(&bead(&job.assignments[0])).unwrap(),
                        base as u8,
                    );
                }
            }
            let (_, mut job) = uniform(true, &[base]);
            job.functional_construction = Some(functional(&job, base));
            job.validate_declaration().unwrap();
            for change in 0..5 {
                let mut bad = job.clone();
                let construction = bad.functional_construction.as_mut().unwrap();
                match change {
                    0 => construction.join.end_collar_height_mm = 0.0,
                    1 => construction.material_id = "unloaded-region".into(),
                    2 => construction.base_material_id = "unloaded-base".into(),
                    3 => construction.occurrence_id = OccurrenceId::new(99).unwrap(),
                    _ => construction.join.collar_head_width_mm = construction.join.head_width_mm,
                }
                assert!(
                    bad.validate_declaration().is_err(),
                    "capture change {change}"
                );
            }
        }
    }

    #[test]
    fn every_occurrence_must_be_uniform_for_the_no_construction_exception() {
        let (_, cycle) = captured(CYCLE);
        for count in [1, 2] {
            for cycle_index in 0..count {
                for mixed in [false, true] {
                    let (printer, mut job) = uniform(mixed, &[0, 0][..count]);
                    job.assignments[cycle_index].material =
                        cycle.assignments[cycle_index].material.clone();
                    assert_eq!(job.validate(&printer).is_ok(), !mixed);
                    // A single-material cycle still has physical boundaries;
                    // it is not the uniform declaration required by this gate.
                    let OccurrenceMaterial::LayerCycle { recipe, .. } =
                        &mut job.assignments[cycle_index].material
                    else {
                        unreachable!()
                    };
                    for slot in &mut recipe.slots {
                        slot.material = 0;
                    }
                    assert_eq!(job.validate(&printer).is_ok(), !mixed);
                }
            }
        }
        for mixed in [false, true] {
            for change in 0..5 {
                let (_, mut job) = uniform(mixed, &[0]);
                job.assignments[0].material = cycle.assignments[0].material.clone();
                let OccurrenceMaterial::LayerCycle {
                    recipe,
                    material_ids,
                } = &mut job.assignments[0].material
                else {
                    unreachable!()
                };
                match change {
                    0 => recipe.slots.clear(),
                    1 => recipe.slots[0].height_mm = 0.0,
                    2 => recipe.slots[0].material = 2,
                    3 => recipe.phase_slot = 64,
                    _ => material_ids[1] = "unloaded-cycle-material".into(),
                }
                assert!(job.validate_declaration().is_err(), "cycle change {change}");
            }
        }
    }

    #[test]
    fn optical_construction_keeps_the_common_family_gate_for_uniform_and_cycle_shells() {
        let (_, cycle) = captured(CYCLE);
        for mixed in [false, true] {
            for cycle_shell in [false, true] {
                let (printer, mut job) = captured(OPTICAL);
                if mixed {
                    job.tools[1].material.polymer_family = "TPU".into();
                }
                if cycle_shell {
                    job.surface_constructions[0].shell = cycle.assignments[0].material.clone();
                }
                assert_eq!(job.validate(&printer).is_ok(), !mixed);
                job.functional_construction = Some(functional(&job, 1));
                assert!(job.validate_declaration().is_err());
            }
        }
    }

    #[test]
    fn uniform_mixed_reservations_require_strict_xy_separation_even_across_z() {
        for axis in [0, 1] {
            for gap in [-1.0, 0.0, 0.001] {
                let (printer, mut job) = uniform(true, &[0, 1]);
                let first = AabbMm::new([20.0, 20.0, 0.0], [40.0, 40.0, 10.0]).unwrap();
                let mut second = first;
                second.min[axis] += 20.0 + gap;
                second.max[axis] += 20.0 + gap;
                // Vertical separation cannot substitute for exclusive XY.
                second.min[2] = 20.0;
                second.max[2] = 30.0;
                job.assignments[0].work_envelope_mm = first;
                job.assignments[1].work_envelope_mm = second;
                assert_eq!(job.validate(&printer).is_ok(), gap > 0.0);
            }
        }
    }

    #[test]
    fn uniform_mixed_assignments_keep_count_identity_binding_and_finite_bounds_checks() {
        let (_, job) = uniform(true, &[0, 1]);
        for change in 0..6 {
            let mut bad = job.clone();
            match change {
                0 => bad.assignments.clear(),
                1 => bad.assignments.push(bad.assignments[0].clone()),
                2 => bad.assignments[1].occurrence_id = bad.assignments[0].occurrence_id,
                3 => {
                    bad.assignments[0].material = OccurrenceMaterial::Uniform {
                        material_id: "unloaded-uniform-material".into(),
                    }
                }
                4 => bad.assignments[0].work_envelope_mm.max[0] = f64::INFINITY,
                _ => {
                    bad.assignments[0].work_envelope_mm.max[2] =
                        bad.assignments[0].work_envelope_mm.min[2]
                }
            }
            assert!(
                bad.validate_declaration().is_err(),
                "assignment change {change}"
            );
        }
    }

    #[test]
    fn every_loaded_tool_must_admit_its_actual_waste_even_without_model_work() {
        for base in [0, 1] {
            let (mut printer, job) = uniform(true, &[base]);
            printer.workflow = JobWorkflow::FixedDualMarlin { job };
            let mut intent = decode_intent(include_bytes!(
                "../../../../fixtures/manufacturing/fixed-dual.intent.json"
            ))
            .unwrap();
            intent.printer = printer;
            intent.validate().unwrap();
            for index in 0..2 {
                for case in 0..5 {
                    let mut bad = intent.clone();
                    let height = bad.select_layer_height().unwrap().height_mm;
                    let JobWorkflow::FixedDualMarlin { job } = &mut bad.printer.workflow else {
                        unreachable!()
                    };
                    let tool = &mut job.tools[index];
                    match case {
                        0 => {
                            tool.deposition_limits.min_bead_width_mm =
                                tool.waste_bead_width_mm + 0.01
                        }
                        1 => {
                            tool.deposition_limits.max_bead_width_mm =
                                tool.waste_bead_width_mm - 0.01
                        }
                        2 => tool.deposition_limits.min_bead_height_mm = height + 0.01,
                        3 => tool.deposition_limits.max_bead_height_mm = height - 0.01,
                        _ => tool.max_volumetric_flow_mm3_s = 0.001,
                    }
                    assert!(
                        bad.validate().is_err(),
                        "base{base}, tool{index}, actual process{case}"
                    );
                }
            }
        }
    }

    #[test]
    fn unused_foreign_tool_keeps_thermal_waste_and_shared_travel_checks() {
        for base in [0, 1] {
            let (printer, job) = uniform(true, &[base]);
            job.validate(&printer).unwrap();
            for tool_index in [0, 1] {
                for change in 0..8 {
                    let mut bad = job.clone();
                    let tool = &mut bad.tools[tool_index];
                    match change {
                        0 => tool.nozzle_temperature_c = tool.minimum_extrusion_temperature_c - 1.0,
                        1 => tool.standby_temperature_c = tool.nozzle_temperature_c + 1.0,
                        2 => tool.waste_line_xy_mm[1] = tool.waste_line_xy_mm[0],
                        3 => tool.minimum_reprime_volume_mm3 = 0.0,
                        4 => tool.retract_filament_mm = 11.0,
                        5 => tool.nozzle_diameter_mm = f64::NAN,
                        6 => tool.max_feed_mm_s = printer.max_feed_mm_s - 1.0,
                        _ => tool.max_volumetric_flow_mm3_s = 0.0,
                    }
                    assert!(
                        bad.validate(&printer).is_err(),
                        "base {base}, tool {tool_index}, change {change}"
                    );
                }
            }
            for change in 0..8 {
                let mut bad = job.clone();
                match change {
                    0 => bad.tools[1].offset_xy_mm = [0.0, 0.0],
                    1 => bad.tools[1].feeder_id = bad.tools[0].feeder_id.clone(),
                    2 => bad.tools[1].material.id = bad.tools[0].material.id.clone(),
                    3 => bad.bed_temperature_c = bad.maximum_bed_temperature_c + 1.0,
                    4 => bad.carriage_body_bounds_mm.min[2] = 0.0,
                    5 => bad.home_position_mm[0] = printer.build_envelope.max[0] + 1.0,
                    6 => bad.park_position_mm[2] = printer.build_envelope.max[2] + 1.0,
                    _ => {
                        bad.assignments[0].work_envelope_mm.max[0] =
                            printer.build_envelope.max[0] + 1.0
                    }
                }
                assert!(bad.validate(&printer).is_err(), "job change {change}");
            }
        }
    }

    #[test]
    fn uniform_mixed_paths_still_require_complete_projected_beads_in_one_reservation() {
        let (_, job) = uniform(true, &[0, 1]);
        for (index, assignment) in job.assignments.iter().enumerate() {
            let path = bead(assignment);
            assert_eq!(job.tool_for_path(&path).unwrap(), index as u8);
            let mut outside = path.clone();
            // The centerline is inside, but half the bead extends outside.
            outside.points_mm[0][0] = assignment.work_envelope_mm.min[0] + 0.1;
            assert!(job.assignment_for_path(&outside).is_err());
            let mut per_edge = path.clone();
            per_edge.points_mm.push([
                assignment.work_envelope_mm.max[0] - 0.1,
                path.points_mm[1][1],
                0.2,
            ]);
            per_edge.width = WidthProfile::PerEdge {
                widths_mm: vec![0.1, 0.4],
            };
            assert!(job.assignment_for_path(&per_edge).is_err());
            let mut outside_z = path.clone();
            outside_z.physical_z_mm = [
                assignment.work_envelope_mm.max[2],
                assignment.work_envelope_mm.max[2] + 0.2,
            ];
            assert!(job.assignment_for_path(&outside_z).is_err());
            let mut unowned = path;
            unowned.material_domain = Some(MaterialDomainRef {
                construction_index: 0,
                role: MaterialConstructionRole::FunctionalRegion,
            });
            assert!(job.tool_for_path(&unowned).is_err());
        }
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

/// Borrowed captured process values. This selects data; it does not emit or verify commands.
pub struct CapturedDepositionProcess<'a> {
    pub policy: &'a ProcessPolicy,
    pub speed_mm_s: f64,
    pub flow_multiplier: f64,
    pub max_volumetric_flow_mm3_s: f64,
}
impl ManufacturingIntent {
    pub fn process_for_tool(
        &self,
        tool: Option<u8>,
    ) -> Result<CapturedDepositionProcess<'_>, ManufacturingError> {
        if let Some(index) = tool {
            let JobWorkflow::FixedDualMarlin { job } = &self.printer.workflow else {
                return Err(invalid("explicit tool process requires a routed job"));
            };
            let tool = job
                .tools
                .get(usize::from(index))
                .ok_or_else(|| invalid("missing captured tool"))?;
            Ok(CapturedDepositionProcess {
                policy: tool.process(&self.recipe),
                speed_mm_s: tool.print_speed_mm_s(&self.recipe),
                flow_multiplier: tool.flow_multiplier(&self.recipe),
                max_volumetric_flow_mm3_s: tool.effective_max_volumetric_flow_mm3_s(&self.printer),
            })
        } else {
            Ok(CapturedDepositionProcess {
                policy: &self.recipe.process,
                speed_mm_s: self.recipe.print_speed_mm_s,
                flow_multiplier: self.recipe.flow_multiplier,
                max_volumetric_flow_mm3_s: self.effective_max_volumetric_flow_mm3_s(),
            })
        }
    }
    pub fn process_for_path(
        &self,
        path: &DepositionPath,
    ) -> Result<CapturedDepositionProcess<'_>, ManufacturingError> {
        self.process_for_tool(match &self.printer.workflow {
            JobWorkflow::FixedDualMarlin { job } => Some(job.tool_for_path(path)?),
            _ => None,
        })
    }
}

#[cfg(test)]
mod mixed_width_tests {
    use super::*;

    #[test]
    fn interior_binding_is_consumed_by_role_and_exact_generated_width() {
        let intent = decode_intent(include_bytes!(
            "../../../../fixtures/manufacturing/mixed-width-interior.intent.json"
        ))
        .unwrap();
        let JobWorkflow::FixedDualMarlin { job } = &intent.printer.workflow else {
            unreachable!()
        };
        let mut path = DepositionPath {
            material_domain: None,
            kind: DepositionKind::SolidFill,
            owner: Some(DepositionOwner::Regional { region_index: 0 }),
            width: WidthProfile::Constant { width_mm: 0.6 },
            physical_z_mm: [0.0, 0.2],
            points_mm: vec![[21.0, 21.0, 0.2], [22.0, 21.0, 0.2]],
        };
        assert_eq!(job.tool_for_path(&path).unwrap(), 1);
        path.width = WidthProfile::Constant { width_mm: 0.45 };
        assert!(
            job.tool_for_path(&path).is_err(),
            "relabeling fine paths cannot implement mixed width"
        );
        path.owner = Some(DepositionOwner::Wall);
        path.kind = DepositionKind::Perimeter;
        assert_eq!(job.tool_for_path(&path).unwrap(), 0);
        for case in 0..5 {
            let mut bad = intent.clone();
            let JobWorkflow::FixedDualMarlin { job } = &mut bad.printer.workflow else {
                unreachable!()
            };
            match case {
                0 => {
                    job.assignments[0].interior.as_mut().unwrap().tool_id = job.tools[0].id.clone()
                }
                1 => job.assignments[0].interior.as_mut().unwrap().bead_width_mm = 0.4,
                2 => job.tools[1].waste_bead_width_mm = 0.3,
                3 => job.assignments[0].interior.as_mut().unwrap().bead_width_mm = 1.0,
                _ => job.assignments[0].tool_id = None,
            }
            assert!(bad.validate().is_err(), "case{case}");
        }
    }
}
