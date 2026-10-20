// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;

/// Exact classified permanent construction extent, including protected holes.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DiaphragmPolygonMm {
    #[serde(deserialize_with = "diaphragm_ring")]
    pub outer: Vec<[f64; 2]>,
    #[serde(deserialize_with = "diaphragm_holes")]
    pub holes: Vec<Vec<[f64; 2]>>,
}
fn diaphragm_ring<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<[f64; 2]>, D::Error> {
    bounded_vec::<D, _, { MAX_DEPOSITION_SEGMENTS as usize }>(d)
}
fn diaphragm_holes<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<Vec<[f64; 2]>>, D::Error> {
    #[derive(Deserialize)]
    struct Ring(#[serde(deserialize_with = "diaphragm_ring")] Vec<[f64; 2]>);
    bounded_vec::<D, Ring, MAX_PROCESS_REGIONS>(d).map(|rings| rings.into_iter().map(|r| r.0).collect())
}
/// Explicit plate ownership is separate from the authored sparse values. Paths
/// use ordinary SolidFill/Bridge process and the identified Regional wire owner.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProcessDiaphragmCoverage {
    pub occurrence_id: OccurrenceId,
    pub definition_id: DefinitionId,
    pub provenance: crate::process::ProcessProvenance,
    pub source_plane_mm: f64,
    pub requested_thickness_mm: f64,
    pub physical_z_mm: [f64; 2],
    pub neighbors: [crate::process::ResolvedProcessValue; 2],
    pub region_index: u16,
    pub area_mm2: f64,
    #[serde(deserialize_with = "diaphragm_extent")]
    pub extent: Vec<DiaphragmPolygonMm>,
}
fn diaphragm_extent<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<DiaphragmPolygonMm>, D::Error> {
    bounded_vec::<D, _, MAX_PROCESS_REGIONS>(d)
}
pub(super) fn process_diaphragms<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<ProcessDiaphragmCoverage>, D::Error> {
    bounded_vec::<D, _, MAX_PROCESS_REGIONS>(d)
}
impl ProcessDiaphragmCoverage {
    pub fn allocated_bytes(&self) -> usize {
        self.definition_id.allocated_bytes()
            + self.extent.capacity() * std::mem::size_of::<DiaphragmPolygonMm>()
            + self.extent.iter().map(|p| {
                p.outer.capacity() * std::mem::size_of::<[f64; 2]>()
                    + p.holes.capacity() * std::mem::size_of::<Vec<[f64; 2]>>()
                    + p.holes.iter().map(|r| r.capacity() * std::mem::size_of::<[f64; 2]>()).sum::<usize>()
            }).sum::<usize>()
    }
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        positive(&[self.requested_thickness_mm, self.area_mm2])?;
        if !self.source_plane_mm.is_finite()
            || !self.physical_z_mm.iter().all(|z| z.is_finite())
            || self.physical_z_mm[0] >= self.physical_z_mm[1]
            || self.extent.is_empty() || self.extent.len() > MAX_PROCESS_REGIONS
        {
            return Err(invalid("invalid permanent diaphragm extent or physical slab"));
        }
        for value in self.neighbors {
            if !value.fraction.is_finite() || value.fraction <= 0.0 || value.fraction > 1.0 {
                return Err(invalid("invalid authored diaphragm neighbor"));
            }
        }
        let mut points = 0usize;
        for polygon in &self.extent {
            for ring in std::iter::once(&polygon.outer).chain(&polygon.holes) {
                points = points.checked_add(ring.len()).ok_or_else(|| resource("diaphragm point overflow"))?;
                if points > MAX_DEPOSITION_SEGMENTS as usize {
                    return Err(resource("diaphragm extent point limit exceeded"));
                }
                if ring.len() < 4 || ring.first() != ring.last()
                    || ring.iter().flatten().any(|v| !v.is_finite())
                {
                    return Err(invalid("diaphragm extent requires finite closed polygon rings"));
                }
            }
        }
        Ok(())
    }
}

pub(super) fn validate_layers(layers: &[PrintLayer]) -> Result<(), ManufacturingError> {
    for (index, layer) in layers.iter().enumerate() {
        for plate in &layer.region_coverage.process_diaphragms {
            plate.validate()?;
            let [lower, upper] = plate.physical_z_mm;
            if layer.lower_z_mm < lower || layer.z_mm > upper {
                return Err(invalid("diaphragm coverage escapes its requested world slab"));
            }
            let scale = lower.abs().max(upper.abs()).max(1.0);
            if ((upper - lower) - plate.requested_thickness_mm).abs() > 4.0 * f64::EPSILON * scale {
                return Err(invalid("diaphragm physical thickness differs from its explicit permission"));
            }
            let first = layers.partition_point(|candidate| candidate.z_mm <= lower);
            let after = layers.partition_point(|candidate| candidate.lower_z_mm < upper);
            if first == 0 || after >= layers.len() || first >= after
                || layers[first].lower_z_mm != lower || layers[after - 1].z_mm != upper
            {
                return Err(invalid("permanent diaphragm must occupy complete actual intervals with lower and upper structure"));
            }
            if index == first {
            for covered in &layers[first..after] {
                let mut matching = covered.region_coverage.process_diaphragms.iter().filter(|candidate|
                    candidate.occurrence_id == plate.occurrence_id && candidate.provenance == plate.provenance);
                let candidate = matching.next().ok_or_else(|| invalid("permanent diaphragm has an unowned physical interval"))?;
                if matching.next().is_some() || candidate.physical_z_mm != plate.physical_z_mm
                    || candidate.extent != plate.extent || candidate.neighbors != plate.neighbors
                    || candidate.requested_thickness_mm != plate.requested_thickness_mm
                    || candidate.source_plane_mm != plate.source_plane_mm
                {
                    return Err(invalid("permanent diaphragm changes extent or construction across its slab"));
                }
            }
            } else if !layers[first].region_coverage.process_diaphragms.iter().any(|root|
                root.occurrence_id == plate.occurrence_id && root.provenance == plate.provenance
                    && root.physical_z_mm == plate.physical_z_mm) {
                return Err(invalid("permanent diaphragm lacks its first physical construction interval"));
            }
            let mut paths = layer.paths.iter().filter(|path|
                path.owner == Some(DepositionOwner::Regional { region_index: plate.region_index }));
            let mut count = 0usize;
            for path in &mut paths {
                count += 1;
                if !matches!(path.kind, DepositionKind::SolidFill | DepositionKind::Bridge)
                    || path.physical_z_mm != [layer.lower_z_mm, layer.z_mm]
                {
                    return Err(invalid("permanent diaphragm owner lacks fine solid/Bridge deposition"));
                }
            }
            if count == 0 {
                return Err(invalid("permanent diaphragm ownership has no actual deposition"));
            }
            if index == first {
            for (neighbor, expected) in [(&layers[first - 1], plate.neighbors[0]), (&layers[after], plate.neighbors[1])] {
                if !neighbor.region_coverage.process_regions.iter().any(|region|
                    region.fraction == expected.fraction && region.contributions.iter().any(|source|
                        source.occurrence_id == plate.occurrence_id && source.definition_id == plate.definition_id
                            && source.provenance == expected.provenance))
                {
                    return Err(invalid("permanent diaphragm omits its actual authored lower/upper regional structure"));
                }
            }
            }
            if index < first || index >= after {
                return Err(invalid("invalid diaphragm layer association"));
            }
        }
    }
    Ok(())
}

pub(super) fn validate_binding(plate: &ProcessDiaphragmCoverage, process: &ProcessDocument, pose: &RigidPoseMm, global_fraction: f64) -> Result<(), ManufacturingError> {
    let bounds = process.layers.iter().find_map(|layer| {
        let scoped = match &layer.scope {
            crate::surface::SurfaceScope::Definition { definition_id } => definition_id == &plate.definition_id,
            crate::surface::SurfaceScope::Occurrence { occurrence_id, expected_definition_id } =>
                *occurrence_id == plate.occurrence_id && expected_definition_id == &plate.definition_id,
        };
        if !layer.enabled || !scoped || layer.layer_id != plate.provenance.layer_id { return None; }
        layer.operations.iter().find_map(|operation| {
            let crate::process::ProcessSelector::SourceBoxMm { min, max } = operation.selector else { return None; };
            ((plate.source_plane_mm == min[2] || plate.source_plane_mm == max[2])
                && operation.operation_id == plate.provenance.operation_id
                && matches!(operation.action, crate::process::ProcessAction::Replace { .. })
                && operation.interface == Some(crate::process::ProcessInterfacePermission::HorizontalDiaphragm {
                    thickness_mm: plate.requested_thickness_mm,
                })).then_some((min, max))
        })
    }).ok_or_else(|| invalid("permanent diaphragm lacks captured source-plane/thickness permission"))?;
    // This checks semantic correspondence to the captured, already admitted
    // exact upright pose; it supplies no native geometry or metric authority.
    let [x, y, z, w] = pose.rotation_xyzw;
    let direction = 1.0 - 2.0 * (x * x + y * y);
    if 2.0 * (x * z - w * y) != 0.0 || 2.0 * (y * z + w * x) != 0.0
        || 2.0 * (x * z + w * y) != 0.0 || 2.0 * (y * z - w * x) != 0.0
        || direction.abs() != 1.0
    {
        return Err(invalid("permanent diaphragm does not bind an exact upright captured pose"));
    }
    let ends = [plate.source_plane_mm - plate.requested_thickness_mm * 0.5,
        plate.source_plane_mm + plate.requested_thickness_mm * 0.5]
        .map(|height| direction * height + pose.translation_mm[2]);
    if plate.physical_z_mm != [ends[0].min(ends[1]), ends[0].max(ends[1])] {
        return Err(invalid("permanent diaphragm world slab differs from its source-plane/pose permission"));
    }
    let [min, max] = [bounds.0, bounds.1];
    let mut neighbors = [plate.source_plane_mm.next_down(), plate.source_plane_mm.next_up()].map(|height|
        process.resolve_at(&plate.definition_id, plate.occurrence_id, global_fraction,
            [min[0] * 0.5 + max[0] * 0.5, min[1] * 0.5 + max[1] * 0.5, height]));
    if direction < 0.0 { neighbors.reverse(); }
    if neighbors != plate.neighbors || !neighbors.iter().any(|value| value.provenance == Some(plate.provenance)) {
        return Err(invalid("permanent diaphragm does not bind the current resolved source winner and neighbors"));
    }
    Ok(())
}
