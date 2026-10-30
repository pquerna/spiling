// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Source-bound physical material construction, independent of deposition roles.
use super::*;

pub const MAX_SURFACE_COLOR_FACES: usize = 16;
// The Cartesian product is at most61,440 points BEFORE decoding or allocation.
// A separate cumulative65,536-point check also covers programmatic producers.
pub const MAX_MATERIAL_DOMAINS: usize = 24;
pub const MAX_MATERIAL_DOMAIN_POLYGONS: usize = 2;
pub const MAX_MATERIAL_DOMAIN_HOLES: usize = 4;
pub const MAX_MATERIAL_DOMAIN_RING_POINTS: usize = 256;

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SurfaceColorFace {
    pub face_id: FaceId,
    pub skin_depth_mm: f64,
    pub backing_depth_mm: f64,
    #[serde(deserialize_with = "required_option")]
    pub mask: Option<SurfaceColorMask>,
}

/// A hard finite window in the certified face's source-local horizontal frame.
/// Height is the complete finite native face; transparency inherits white base.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SurfaceColorMask {
    pub min_u_mm: f64,
    pub max_u_mm: f64,
}

/// Inspectable source frame, not an independent native material certificate.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SurfaceColorFrame {
    pub construction_index: u16,
    pub face_id: FaceId,
    pub origin_mm: [f64; 3],
    pub horizontal_direction: [f64; 3],
    pub outward_normal: [f64; 3],
    pub width_mm: f64,
    pub source_z_mm: [f64; 2],
    pub selected_u_mm: [f64; 2],
}

pub(super) fn frames<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<SurfaceColorFrame>, D::Error> {
    bounded_vec::<D, _, MAX_SURFACE_COLOR_FACES>(d)
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SurfaceColorConstruction {
    pub occurrence_id: OccurrenceId,
    pub definition_id: DefinitionId,
    pub source_hash: SourceHash,
    #[serde(deserialize_with = "faces")]
    pub faces: Vec<SurfaceColorFace>,
    pub shell: OccurrenceMaterial,
    pub white_material_id: String,
    #[serde(deserialize_with = "required_option")]
    pub optical_origin: Option<OpticalConstructionOrigin>,
}

fn faces<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<SurfaceColorFace>, D::Error> {
    bounded_vec::<D, _, MAX_SURFACE_COLOR_FACES>(d)
}

impl SurfaceColorConstruction {
    pub fn validate(&self, job: &FixedDualJob) -> Result<(), ManufacturingError> {
        if self.faces.is_empty() || self.faces.len() > MAX_SURFACE_COLOR_FACES {
            return Err(invalid("surface construction requires 1..16 native faces"));
        }
        let assignment = job
            .assignments
            .iter()
            .find(|a| a.occurrence_id == self.occurrence_id)
            .ok_or_else(|| invalid("surface construction occurrence has no work reservation"))?;
        self.shell.validate_for_z_bounds([
            assignment.work_envelope_mm.min[2],
            assignment.work_envelope_mm.max[2],
        ])?;
        match &self.shell {
            OccurrenceMaterial::Uniform { material_id } => {
                job.tool_for_material(material_id)?;
            }
            OccurrenceMaterial::LayerCycle { material_ids, .. } => {
                for id in material_ids {
                    job.tool_for_material(id)?;
                }
            }
        }
        job.tool_for_material(&self.white_material_id)?;
        if !matches!(&assignment.material, OccurrenceMaterial::Uniform { material_id } if material_id == &self.white_material_id)
        {
            return Err(invalid(
                "surface construction requires the occurrence's uniform base material to be its declared white core/backing",
            ));
        }
        if let Some(origin) = &self.optical_origin {
            origin.validate(job, &self.shell)?;
        }
        for (index, face) in self.faces.iter().enumerate() {
            if face.mask.as_ref().is_some_and(|m| {
                !m.min_u_mm.is_finite()
                    || !m.max_u_mm.is_finite()
                    || m.min_u_mm < 0.0
                    || m.min_u_mm >= m.max_u_mm
            }) {
                return Err(invalid(
                    "surface color mask requires a finite positive source-face U window",
                ));
            }
            if !face.skin_depth_mm.is_finite()
                || face.skin_depth_mm <= 0.0
                || !face.backing_depth_mm.is_finite()
                || face.backing_depth_mm <= 0.0
                || !(face.skin_depth_mm + face.backing_depth_mm).is_finite()
                || self.faces[..index]
                    .iter()
                    .any(|other| other.face_id == face.face_id)
            {
                return Err(invalid(
                    "surface construction requires unique faces and positive finite skin/backing depths",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MaterialConstructionRole {
    Shell,
    Backing,
    Core,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MaterialDomainRef {
    /// Shell selects its regional request. Shared white backing/core use index0
    /// and cover the union of all admitted constructions on the same native part.
    pub construction_index: u16,
    pub role: MaterialConstructionRole,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaterialDomainPolygon {
    #[serde(deserialize_with = "ring")]
    pub exterior_mm: Vec<[f64; 2]>,
    #[serde(deserialize_with = "holes")]
    pub holes_mm: Vec<Vec<[f64; 2]>>,
}
fn ring<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<[f64; 2]>, D::Error> {
    bounded_vec::<D, _, MAX_MATERIAL_DOMAIN_RING_POINTS>(d)
}
fn holes<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Vec<[f64; 2]>>, D::Error> {
    #[derive(Deserialize)]
    struct Ring(#[serde(deserialize_with = "ring")] Vec<[f64; 2]>);
    Ok(bounded_vec::<D, Ring, MAX_MATERIAL_DOMAIN_HOLES>(d)?
        .into_iter()
        .map(|r| r.0)
        .collect())
}

/// A finite native-derived XY domain over an explicit physical interval. Native
/// certificates remain compiler authority; fresh replay checks actual full beads.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ResolvedMaterialDomain {
    pub reference: MaterialDomainRef,
    pub physical_z_mm: [f64; 2],
    #[serde(deserialize_with = "polygons")]
    pub polygons: Vec<MaterialDomainPolygon>,
}
impl ResolvedMaterialDomain {
    pub fn validate(&self) -> Result<usize, ManufacturingError> {
        if !self.physical_z_mm.iter().all(|v| v.is_finite())
            || self.physical_z_mm[0] >= self.physical_z_mm[1]
            || self.polygons.is_empty()
            || self.polygons.len() > MAX_MATERIAL_DOMAIN_POLYGONS
        {
            return Err(invalid(
                "invalid resolved material domain interval or polygons",
            ));
        }
        let mut points = 0usize;
        for polygon in &self.polygons {
            if polygon.holes_mm.len() > MAX_MATERIAL_DOMAIN_HOLES {
                return Err(resource("material-domain hole limit exceeded"));
            }
            for ring in std::iter::once(&polygon.exterior_mm).chain(&polygon.holes_mm) {
                if ring.len() < 4
                    || ring.len() > MAX_MATERIAL_DOMAIN_RING_POINTS
                    || ring.first() != ring.last()
                    || ring.iter().flatten().any(|v| !v.is_finite())
                {
                    return Err(invalid("material-domain ring must be finite and closed"));
                }
                points = points
                    .checked_add(ring.len())
                    .ok_or_else(|| resource("material-domain point overflow"))?;
            }
        }
        Ok(points)
    }
}

pub(super) fn validate_material_domains(
    plan: &NormalizedPrintPlan,
    intent: &ManufacturingIntent,
) -> Result<(), ManufacturingError> {
    let job = match &intent.printer.workflow {
        JobWorkflow::FixedDualMarlin { job } => Some(job),
        _ => None,
    };
    if plan.material_frames.len() > MAX_SURFACE_COLOR_FACES
        || plan.material_frames.len()
            != job.map_or(0, |j| {
                j.surface_constructions.iter().map(|c| c.faces.len()).sum()
            })
    {
        return Err(invalid(
            "surface frame table does not bind the requested faces",
        ));
    }
    for (index, frame) in plan.material_frames.iter().enumerate() {
        let face = job
            .and_then(|j| {
                j.surface_constructions
                    .get(usize::from(frame.construction_index))
            })
            .and_then(|c| c.faces.iter().find(|f| f.face_id == frame.face_id))
            .ok_or_else(|| invalid("surface frame has no captured native-face request"))?;
        let requested = face
            .mask
            .as_ref()
            .map_or([0.0, frame.width_mm], |m| [m.min_u_mm, m.max_u_mm]);
        if frame
            .origin_mm
            .iter()
            .chain(&frame.horizontal_direction)
            .chain(&frame.outward_normal)
            .chain(&frame.source_z_mm)
            .chain(&frame.selected_u_mm)
            .any(|v| !v.is_finite())
            || !frame.width_mm.is_finite()
            || frame.width_mm <= 0.0
            || frame.selected_u_mm != requested
            || frame.selected_u_mm[0] < 0.0
            || frame.selected_u_mm[0] >= frame.selected_u_mm[1]
            || frame.selected_u_mm[1] > frame.width_mm
            || frame.source_z_mm[0] >= frame.source_z_mm[1]
            || frame.horizontal_direction[2] != 0.0
            || frame.outward_normal[2] != 0.0
            || plan.material_frames[..index].iter().any(|f| {
                f.construction_index == frame.construction_index && f.face_id == frame.face_id
            })
        {
            return Err(invalid("invalid source-bound surface frame or finite mask"));
        }
    }
    if plan.material_domains.is_empty() {
        if job.is_some_and(|j| !j.surface_constructions.is_empty()) {
            return Err(invalid("surface construction has no resolved domains"));
        }
    } else if job.is_none_or(|j| j.surface_constructions.is_empty()) {
        return Err(invalid(
            "material domains require an explicit source construction",
        ));
    }
    if plan.material_domains.len() > MAX_MATERIAL_DOMAINS {
        return Err(resource("material-domain count exceeded"));
    }
    let mut points = 0usize;
    for (index, domain) in plan.material_domains.iter().enumerate() {
        points += domain.validate()?;
        if points > 65_536 {
            return Err(resource("material-domain point budget exceeded"));
        }
        if job.is_none_or(|j| {
            usize::from(domain.reference.construction_index) >= j.surface_constructions.len()
        }) {
            return Err(invalid("material domain references an absent construction"));
        }
        if domain.reference.construction_index != 0
            && domain.reference.role != MaterialConstructionRole::Shell
        {
            return Err(invalid(
                "shared white backing/core must have one canonical arrangement owner",
            ));
        }
        if plan.material_domains[..index].iter().any(|other| {
            other.reference == domain.reference
                && other.physical_z_mm[0] < domain.physical_z_mm[1]
                && domain.physical_z_mm[0] < other.physical_z_mm[1]
        }) {
            return Err(invalid("material domain has overlapping physical owners"));
        }
    }
    for path in plan.layers.iter().flat_map(|layer| &layer.paths) {
        if let Some(reference) = path.material_domain {
            if plan
                .material_domains
                .iter()
                .filter(|d| {
                    d.reference == reference
                        && d.physical_z_mm[0] <= path.physical_z_mm[0]
                        && d.physical_z_mm[1] >= path.physical_z_mm[1]
                })
                .count()
                != 1
            {
                return Err(invalid(
                    "complete bead does not bind exactly one material domain",
                ));
            }
        } else if job.is_none() && !plan.material_domains.is_empty() {
            return Err(invalid("unrouted material domain"));
        }
    }
    for layer in &plan.layers {
        for construction_index in 0..job.map_or(0, |j| j.surface_constructions.len()) {
            for role in [
                MaterialConstructionRole::Shell,
                MaterialConstructionRole::Backing,
                MaterialConstructionRole::Core,
            ] {
                if construction_index != 0 && role != MaterialConstructionRole::Shell {
                    continue;
                }
                if plan
                    .material_domains
                    .iter()
                    .filter(|d| {
                        d.reference
                            == (MaterialDomainRef {
                                construction_index: construction_index as u16,
                                role,
                            })
                            && d.physical_z_mm[0] <= layer.lower_z_mm
                            && d.physical_z_mm[1] >= layer.z_mm
                    })
                    .count()
                    != 1
                {
                    return Err(invalid(
                        "surface construction lacks a complete shell/backing/core partition",
                    ));
                }
            }
        }
        for region in &layer.region_coverage.process_regions {
            if let Some(reference) = region.material_domain
                && (!plan.material_domains.iter().any(|d| {
                    d.reference == reference
                        && d.physical_z_mm[0] <= layer.lower_z_mm
                        && d.physical_z_mm[1] >= layer.z_mm
                }) || (reference.role != MaterialConstructionRole::Core
                    && region.fraction != 1.0))
            {
                return Err(invalid(
                    "solid surface construction has invalid material-domain coverage",
                ));
            }
        }
        for path in &layer.paths {
            if let Some(DepositionOwner::Regional { region_index }) = path.owner
                && let Some(reference) = layer
                    .region_coverage
                    .process_regions
                    .get(usize::from(region_index))
                    .and_then(|r| r.material_domain)
                && path.material_domain != Some(reference)
            {
                return Err(invalid(
                    "regional bead disagrees with its constructed material owner",
                ));
            }
        }
    }
    Ok(())
}
fn polygons<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<MaterialDomainPolygon>, D::Error> {
    bounded_vec::<D, _, MAX_MATERIAL_DOMAIN_POLYGONS>(d)
}
pub(super) fn material_domains<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<ResolvedMaterialDomain>, D::Error> {
    bounded_vec::<D, _, MAX_MATERIAL_DOMAINS>(d)
}
pub(super) fn constructions<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<SurfaceColorConstruction>, D::Error> {
    bounded_vec::<D, _, 16>(d)
}

/// Explicit evidence-to-loaded-material binding; no key/index inference.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OpticalMaterialBinding {
    pub evidence_key: String,
    pub evidence_hash: SourceHash,
    pub loaded_material_id: String,
    pub loaded_declaration_hash: SourceHash,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ApplyOpticalConstruction {
    pub construction_index: u16,
    pub report_hash: SourceHash,
    pub selection: crate::optical::OpticalSelection,
    pub bindings: [OpticalMaterialBinding; 2],
}
impl ApplyOpticalConstruction {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        if self.construction_index >= 16 {
            return Err(invalid(
                "optical construction index exceeds the bounded request table",
            ));
        }
        for binding in &self.bindings {
            for value in [&binding.evidence_key, &binding.loaded_material_id] {
                if value.is_empty()
                    || value.len() > 64
                    || value.trim() != value
                    || value.chars().any(char::is_control)
                {
                    return Err(invalid(
                        "optical application requires bounded explicit evidence/material keys",
                    ));
                }
            }
        }
        if self.bindings[0].loaded_material_id == self.bindings[1].loaded_material_id {
            return Err(invalid(
                "optical recipe materials require two distinct loaded materials",
            ));
        }
        Ok(())
    }
}

/// Immutable evidence closure captured by Core when an accepted study is applied.
/// This is historical authoring provenance, not backing optics or print authority.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AppliedOpticalMaterial {
    pub evidence: crate::appearance::PaletteRow,
    pub evidence_hash: SourceHash,
    pub loaded_material: LoadedMaterial,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OpticalConstructionOrigin {
    pub report: crate::optical::OpticalReportRecord,
    pub palette_hash: SourceHash,
    pub selection: crate::optical::OpticalSelection,
    pub model: crate::optical::OpticalModel,
    pub observation: crate::optical::OpticalObservation,
    pub target: crate::optical::OpticalTarget,
    pub requested_patch: crate::optical::OpticalPatch,
    pub recipe: crate::optical::LayerCycle,
    pub materials: [AppliedOpticalMaterial; 2],
    pub applied_pose: RigidPoseMm,
}
impl OpticalConstructionOrigin {
    pub fn validate_capture(&self) -> Result<(), ManufacturingError> {
        let convert = |e: crate::project::ProjectError| invalid(&e.message);
        self.report.validate().map_err(convert)?;
        self.requested_patch.validate().map_err(convert)?;
        self.recipe
            .validate(2, &self.requested_patch)
            .map_err(convert)?;
        self.applied_pose
            .validate()
            .map_err(|e| invalid(&e.message))?;
        self.target.color.validate().map_err(convert)?;
        if !self.report.complete
            || !self.target.delta_e00_tolerance.is_finite()
            || self.target.delta_e00_tolerance < 0.0
            || self
                .target
                .lightness_tolerance
                .is_some_and(|v| !v.is_finite() || v < 0.0)
            || matches!(self.selection, crate::optical::OpticalSelection::Candidate { index } if index >= crate::optical::MAX_OPTICAL_CANDIDATES as u32)
            || self.materials[0].evidence.key == self.materials[1].evidence.key
        {
            return Err(invalid(
                "optical construction origin requires a complete bounded retained selection",
            ));
        }
        for material in &self.materials {
            material.evidence.validate().map_err(convert)?;
            if material.evidence.evidence_hash().map_err(convert)? != material.evidence_hash {
                return Err(invalid(
                    "optical construction evidence content hash changed",
                ));
            }
            for value in [
                &material.loaded_material.id,
                &material.loaded_material.revision,
                &material.loaded_material.polymer_family,
            ] {
                if value.is_empty()
                    || value.len() > 64
                    || value.trim() != value
                    || value.chars().any(char::is_control)
                {
                    return Err(invalid("invalid captured loaded-material declaration"));
                }
            }
        }
        Ok(())
    }
    pub fn validate(
        &self,
        job: &FixedDualJob,
        shell: &OccurrenceMaterial,
    ) -> Result<(), ManufacturingError> {
        self.validate_capture()?;
        let mut ids = Vec::new();
        for material in &self.materials {
            if job.tools[usize::from(job.tool_for_material(&material.loaded_material.id)?)].material
                != material.loaded_material
            {
                return Err(invalid(
                    "optical construction evidence or loaded-material identity changed",
                ));
            }
            ids.push(material.loaded_material.id.clone());
        }
        if ids[0] == ids[1]
            || !matches!(shell, OccurrenceMaterial::LayerCycle { recipe, material_ids } if recipe == &self.recipe && material_ids.as_slice() == ids.as_slice())
        {
            return Err(invalid(
                "applied optical origin disagrees with the executable shell recipe; clear origin before independent recipe edits",
            ));
        }
        Ok(())
    }
}
