// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Inert functional material requirements; native geometry and replay own proof.
use super::*;

pub const MAX_FUNCTIONAL_KEYS: usize = 16;

/// Authored interface intent, never adhesion or physical-retention qualification.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FunctionalInterfacePolicy {
    DeclaredSameFamily,
    /// Nominal headed-key/end-collar geometry; native production and independent
    /// replay must still establish their obligations. No physical retention credit.
    MechanicalCapture,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FunctionalMaterialConstruction {
    pub interface_policy: FunctionalInterfacePolicy,
    pub occurrence_id: OccurrenceId,
    pub definition_id: DefinitionId,
    pub source_hash: SourceHash,
    pub face_id: FaceId,
    pub material_id: String,
    pub base_material_id: String,
    pub minimum_pure_depth_mm: f64,
    pub join: PlanarTKeyJoin,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PlanarTKeyJoin {
    pub reach_into_region_mm: f64,
    pub reach_into_base_mm: f64,
    pub pitch_mm: f64,
    pub stem_width_mm: f64,
    pub head_width_mm: f64,
    pub head_depth_mm: f64,
    pub edge_margin_mm: f64,
    pub end_collar_height_mm: f64,
    pub collar_head_width_mm: f64,
}

impl FunctionalMaterialConstruction {
    pub fn validate_dimensions(&self) -> Result<(), ManufacturingError> {
        label(&self.material_id)?;
        label(&self.base_material_id)?;
        if self.material_id == self.base_material_id {
            return Err(invalid(
                "functional region and base require distinct materials",
            ));
        }
        self.join.validate()?;
        positive(&[self.minimum_pure_depth_mm])?;
        if !(self.minimum_pure_depth_mm
            + self.join.reach_into_region_mm
            + self.join.reach_into_base_mm)
            .is_finite()
        {
            return Err(invalid("functional construction depth overflow"));
        }
        Ok(())
    }

    pub fn validate(&self, job: &FixedDualJob) -> Result<(), ManufacturingError> {
        self.validate_dimensions()?;
        if job.assignments.len() != 1
            || !job.surface_constructions.is_empty()
            || job.assignments[0].occurrence_id != self.occurrence_id
            || !matches!(&job.assignments[0].material, OccurrenceMaterial::Uniform { material_id } if material_id == &self.base_material_id)
            || self.material_id == self.base_material_id
        {
            return Err(invalid(
                "functional construction requires one uniformly assigned base, a distinct loaded region material and no optical construction",
            ));
        }
        job.tool_for_material(&self.material_id)?;
        job.tool_for_material(&self.base_material_id)?;
        Ok(())
    }
}

impl PlanarTKeyJoin {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        let join = self;
        positive(&[
            join.reach_into_region_mm,
            join.reach_into_base_mm,
            join.pitch_mm,
            join.stem_width_mm,
            join.head_width_mm,
            join.head_depth_mm,
            join.edge_margin_mm,
            join.end_collar_height_mm,
            join.collar_head_width_mm,
        ])?;
        let transition = join.reach_into_region_mm + join.reach_into_base_mm;
        if !transition.is_finite()
            || join.stem_width_mm >= join.collar_head_width_mm
            || join.collar_head_width_mm >= join.head_width_mm
            || join.head_width_mm >= join.pitch_mm
            || join.head_depth_mm >= join.reach_into_base_mm
        {
            return Err(invalid(
                "T-keys require stem < collar head < middle head < pitch and a head wholly beyond the nominal interface",
            ));
        }
        Ok(())
    }
}

/// Inspectable source-local face and world-Z key placement, never a certificate
/// of pure depth, retention, material contact or physical printability.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FunctionalMaterialFrame {
    pub occurrence_id: OccurrenceId,
    pub definition_id: DefinitionId,
    pub source_hash: SourceHash,
    pub face_id: FaceId,
    pub origin_mm: [f64; 3],
    pub world_origin_mm: [f64; 3],
    pub stock_bounds_mm: AabbMm,
    pub horizontal_direction: [f64; 3],
    pub outward_normal: [f64; 3],
    pub width_mm: f64,
    pub source_z_mm: [f64; 2],
    /// Closed world interval where the wider middle heads apply.
    pub key_middle_z_mm: [f64; 2],
    #[serde(deserialize_with = "key_centers")]
    pub key_centers_u_mm: Vec<f64>,
}

fn key_centers<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<f64>, D::Error> {
    bounded_vec::<D, _, MAX_FUNCTIONAL_KEYS>(d)
}

impl FunctionalMaterialFrame {
    pub fn validate_values(&self) -> Result<(), ManufacturingError> {
        self.stock_bounds_mm
            .validate()
            .map_err(|error| invalid(&error.message))?;
        let norm = |v: [f64; 3]| v.iter().map(|x| x * x).sum::<f64>();
        let dot = self
            .horizontal_direction
            .iter()
            .zip(self.outward_normal)
            .map(|(a, b)| a * b)
            .sum::<f64>();
        if self
            .origin_mm
            .iter()
            .chain(&self.world_origin_mm)
            .chain(&self.horizontal_direction)
            .chain(&self.outward_normal)
            .chain(&self.source_z_mm)
            .chain(&self.key_middle_z_mm)
            .chain(&self.key_centers_u_mm)
            .any(|v| !v.is_finite())
            || !self.width_mm.is_finite()
            || self.width_mm <= 0.0
            || !(self.source_z_mm[1] - self.source_z_mm[0]).is_finite()
            || self.source_z_mm[0] >= self.source_z_mm[1]
            || !(self.key_middle_z_mm[1] - self.key_middle_z_mm[0]).is_finite()
            || self.key_middle_z_mm[0] >= self.key_middle_z_mm[1]
            || self.horizontal_direction[2] != 0.0
            || self.outward_normal[2] != 0.0
            || norm(self.horizontal_direction) != 1.0
            || norm(self.outward_normal) != 1.0
            || dot != 0.0
            || !self.horizontal_direction[..2]
                .iter()
                .any(|value| value.abs() == 1.0)
            || !self.outward_normal[..2]
                .iter()
                .any(|value| value.abs() == 1.0)
            || (0..3).any(|axis| self.stock_bounds_mm.min[axis] >= self.stock_bounds_mm.max[axis])
            || self.key_centers_u_mm.is_empty()
            || self.key_centers_u_mm.len() > MAX_FUNCTIONAL_KEYS
            || !self.key_centers_u_mm.contains(&(self.width_mm * 0.5))
            || self
                .key_centers_u_mm
                .iter()
                .any(|u| *u <= 0.0 || *u >= self.width_mm)
            || self.key_centers_u_mm.windows(2).any(|u| u[0] >= u[1])
        {
            return Err(invalid(
                "invalid finite functional frame or bounded centered keys",
            ));
        }
        let depth_axis = (0..2)
            .find(|&axis| self.outward_normal[axis].abs() == 1.0)
            .unwrap();
        let horizontal_axis = 1 - depth_axis;
        let bounds = &self.stock_bounds_mm;
        let depth_coordinate = if self.outward_normal[depth_axis] > 0.0 {
            bounds.max[depth_axis]
        } else {
            bounds.min[depth_axis]
        };
        let horizontal_coordinate = if self.horizontal_direction[horizontal_axis] > 0.0 {
            bounds.min[horizontal_axis]
        } else {
            bounds.max[horizontal_axis]
        };
        if self.world_origin_mm[depth_axis] != depth_coordinate
            || self.world_origin_mm[horizontal_axis] != horizontal_coordinate
            || self.world_origin_mm[2] != bounds.min[2]
            || self.width_mm != bounds.max[horizontal_axis] - bounds.min[horizontal_axis]
        {
            return Err(invalid(
                "functional frame does not preserve the literal stock face and exterior corners",
            ));
        }
        Ok(())
    }

    pub fn validate(
        &self,
        construction: &FunctionalMaterialConstruction,
        job: &FixedDualJob,
    ) -> Result<(), ManufacturingError> {
        self.validate_values()?;
        construction.validate(job)?;
        let join = &construction.join;
        let bounds = &job.assignments[0].work_envelope_mm;
        if self.occurrence_id != construction.occurrence_id
            || self.definition_id != construction.definition_id
            || self.source_hash != construction.source_hash
            || self.face_id != construction.face_id
            || self.source_z_mm[1] - self.source_z_mm[0] <= 2.0 * join.end_collar_height_mm
            || self.key_middle_z_mm[0] < bounds.min[2] + join.end_collar_height_mm
            || self.key_middle_z_mm[1] > bounds.max[2] - join.end_collar_height_mm
            || self.key_centers_u_mm.iter().any(|u| {
                *u - join.head_width_mm * 0.5 < join.edge_margin_mm
                    || self.width_mm - *u - join.head_width_mm * 0.5 < join.edge_margin_mm
            })
            || self
                .key_centers_u_mm
                .windows(2)
                .any(|u| u[1] - u[0] < join.pitch_mm)
        {
            return Err(invalid(
                "invalid source-bound functional frame or bounded centered key placement",
            ));
        }
        Ok(())
    }

    pub fn validate_source_pose(
        &self,
        construction: &FunctionalMaterialConstruction,
        pose: &RigidPoseMm,
    ) -> Result<(), ManufacturingError> {
        self.validate_values()?;
        construction.validate_dimensions()?;
        pose.validate().map_err(|e| invalid(&e.message))?;
        let world_origin: [f64; 3] =
            std::array::from_fn(|axis| self.origin_mm[axis] + pose.translation_mm[axis]);
        let middle = [
            self.source_z_mm[0] + pose.translation_mm[2] + construction.join.end_collar_height_mm,
            self.source_z_mm[1] + pose.translation_mm[2] - construction.join.end_collar_height_mm,
        ];
        if pose.rotation_xyzw != [0.0, 0.0, 0.0, 1.0]
            || world_origin.iter().chain(&middle).any(|v| !v.is_finite())
            || self.world_origin_mm != world_origin
            || self.key_middle_z_mm != middle
            || self.stock_bounds_mm.min[2] != self.source_z_mm[0] + pose.translation_mm[2]
            || self.stock_bounds_mm.max[2] != self.source_z_mm[1] + pose.translation_mm[2]
        {
            return Err(invalid(
                "functional world frame does not bind the original translation-only source pose and collars",
            ));
        }
        Ok(())
    }
}

pub(super) fn validate_material_domains(
    plan: &NormalizedPrintPlan,
    construction: &FunctionalMaterialConstruction,
    job: &FixedDualJob,
) -> Result<(), ManufacturingError> {
    let frame = plan
        .functional_material_frame
        .as_ref()
        .ok_or_else(|| invalid("functional construction has no source frame"))?;
    frame.validate(construction, job)?;
    if !plan.material_frames.is_empty() || !job.surface_constructions.is_empty() {
        return Err(invalid(
            "functional construction cannot carry optical frames or requests",
        ));
    }
    if plan.material_domains.is_empty() || plan.material_domains.len() > MAX_MATERIAL_DOMAINS {
        return Err(resource(
            "functional material-domain count must be within 1..24",
        ));
    }
    let roles = [
        MaterialConstructionRole::FunctionalRegion,
        MaterialConstructionRole::FunctionalBase,
    ];
    let mut points = 0usize;
    for (index, domain) in plan.material_domains.iter().enumerate() {
        points = points
            .checked_add(domain.validate()?)
            .ok_or_else(|| resource("functional material-domain point overflow"))?;
        if points > 65_536 {
            return Err(resource("functional material-domain point budget exceeded"));
        }
        if domain.reference.construction_index != 0
            || !roles.contains(&domain.reference.role)
            || !plan.material_domains.iter().any(|other| {
                other.reference.construction_index == 0
                    && other.reference.role != domain.reference.role
                    && roles.contains(&other.reference.role)
                    && other.physical_z_mm == domain.physical_z_mm
            })
            || frame
                .key_middle_z_mm
                .iter()
                .any(|z| domain.physical_z_mm[0] < *z && *z < domain.physical_z_mm[1])
            || plan.material_domains[..index].iter().any(|other| {
                other.reference == domain.reference
                    && other.physical_z_mm[0] < domain.physical_z_mm[1]
                    && domain.physical_z_mm[0] < other.physical_z_mm[1]
            })
            || !plan.layers.iter().any(|layer| {
                domain.physical_z_mm[0] <= layer.lower_z_mm && layer.z_mm <= domain.physical_z_mm[1]
            })
        {
            return Err(invalid(
                "functional domain has an invalid role, phase or aliased interval",
            ));
        }
    }
    for layer in &plan.layers {
        for role in roles {
            if plan
                .material_domains
                .iter()
                .filter(|d| {
                    d.reference
                        == (MaterialDomainRef {
                            construction_index: 0,
                            role,
                        })
                        && d.physical_z_mm[0] <= layer.lower_z_mm
                        && layer.z_mm <= d.physical_z_mm[1]
                })
                .count()
                != 1
            {
                return Err(invalid(
                    "functional layer lacks complete region/base material intervals",
                ));
            }
        }
        for region in &layer.region_coverage.process_regions {
            let reference = region
                .material_domain
                .ok_or_else(|| invalid("functional process region has no material owner"))?;
            if !plan.material_domains.iter().any(|d| {
                d.reference == reference
                    && d.physical_z_mm[0] <= layer.lower_z_mm
                    && layer.z_mm <= d.physical_z_mm[1]
            }) || (reference.role == MaterialConstructionRole::FunctionalRegion
                && region.fraction != 1.0)
                || region.contributions.iter().any(|source| {
                    source.occurrence_id != construction.occurrence_id
                        || source.definition_id != construction.definition_id
                })
            {
                return Err(invalid(
                    "functional process region disagrees with its material domain",
                ));
            }
        }
        for path in &layer.paths {
            job.tool_for_path(path)?;
            if plan
                .material_domains
                .iter()
                .filter(|d| {
                    Some(d.reference) == path.material_domain
                        && d.physical_z_mm[0] <= path.physical_z_mm[0]
                        && path.physical_z_mm[1] <= d.physical_z_mm[1]
                })
                .count()
                != 1
            {
                return Err(invalid(
                    "functional bead does not bind exactly one complete material interval",
                ));
            }
            if let Some(DepositionOwner::Regional { region_index }) = path.owner {
                let region = layer
                    .region_coverage
                    .process_regions
                    .get(usize::from(region_index))
                    .ok_or_else(|| invalid("functional bead has an absent regional owner"))?;
                if region.material_domain != path.material_domain {
                    return Err(invalid(
                        "functional regional bead disagrees with its material owner",
                    ));
                }
            }
        }
    }
    Ok(())
}
