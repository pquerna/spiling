// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{mechanics::*, *};

const HEADER: usize = 32;
const NODE: usize = 72;
const ELEMENT: usize = 232;
const PATCH: usize = 64;
const METADATA_LIMIT: usize = 48 * 1024;

#[derive(Debug, Clone)]
pub struct AnalysisNode {
    pub position_mm: [f64; 3],
    pub displacement_mm: [f64; 3],
    pub applied_force_n: [f64; 3],
}
#[derive(Debug, Clone)]
pub struct AnalysisElement {
    pub nodes: [u32; 27],
    pub center_mm: [f64; 3],
    pub size_mm: [f64; 3],
    pub role: u32,
    pub volume_mm3: f64,
    pub stress_mpa: [f64; 6],
    pub energy_n_mm: f64,
    pub concentration: f64,
}
#[derive(Debug, Clone)]
pub struct AnalysisPatchSample {
    pub point_mm: [f64; 3],
    pub area_mm2: f64,
    pub traction_mpa: [f64; 3],
    pub patch_index: u32,
}
fn fail(message: &str) -> ProjectError {
    invalid(message)
}
fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn read_values<const COUNT: usize>(bytes: &[u8], offset: usize) -> [f64; COUNT] {
    std::array::from_fn(|index| {
        f64::from_le_bytes(
            bytes[offset + 8 * index..offset + 8 * (index + 1)]
                .try_into()
                .unwrap(),
        )
    })
}
pub struct AnalysisFields<'a> {
    bytes: &'a [u8],
    pub summary: AnalysisRunSummary,
    pub node_count: usize,
    pub element_count: usize,
    pub patch_count: usize,
    node_offset: usize,
    element_offset: usize,
    patch_offset: usize,
}
impl<'a> AnalysisFields<'a> {
    pub fn decode(bytes: &'a [u8]) -> Result<Self, ProjectError> {
        if bytes.len() < HEADER
            || bytes.len() > MAX_MECHANICAL_FIELD_BYTES
            || &bytes[..4] != b"SPLA"
            || read_u32(bytes, 4) != 1
            || bytes[24..32] != [0; 8]
        {
            return Err(fail("invalid or oversized SPLA header"));
        }
        let metadata = read_u32(bytes, 8) as usize;
        let nodes = read_u32(bytes, 12) as usize;
        let elements = read_u32(bytes, 16) as usize;
        let patches = read_u32(bytes, 20) as usize;
        if metadata > METADATA_LIMIT
            || nodes == 0
            || nodes > MAX_MECHANICAL_DOFS / 3
            || elements == 0
            || elements > MAX_MECHANICAL_ELEMENTS
            || patches > MAX_MECHANICAL_ELEMENTS * 54 * MAX_ANALYSIS_PATCHES
        {
            return Err(fail("SPLA counts exceed admission"));
        }
        let node_offset = HEADER + metadata;
        let element_offset = node_offset + nodes * NODE;
        let patch_offset = element_offset + elements * ELEMENT;
        if patch_offset + patches * PATCH != bytes.len() {
            return Err(fail("SPLA exact byte count differs from header"));
        }
        let summary: AnalysisRunSummary = serde_json::from_slice(&bytes[HEADER..node_offset])
            .map_err(|error| fail(&format!("SPLA metadata: {error}")))?;
        summary.validate()?;
        if summary.dofs as usize != 3 * nodes || summary.elements as usize != elements {
            return Err(fail("SPLA summary counts differ"));
        }
        let result = Self {
            bytes,
            summary,
            node_count: nodes,
            element_count: elements,
            patch_count: patches,
            node_offset,
            element_offset,
            patch_offset,
        };
        for index in 0..nodes {
            let node = result.node(index).unwrap();
            if node
                .position_mm
                .into_iter()
                .chain(node.displacement_mm)
                .chain(node.applied_force_n)
                .any(|value| !value.is_finite())
            {
                return Err(fail("SPLA node has nonfinite values"));
            }
        }
        for index in 0..elements {
            let element = result.element(index).unwrap();
            if element.nodes.iter().any(|node| *node as usize >= nodes)
                || element.role > 2
                || element
                    .size_mm
                    .iter()
                    .any(|size| !size.is_finite() || *size <= 0.0)
                || element.volume_mm3 <= 0.0
                || element
                    .center_mm
                    .into_iter()
                    .chain(element.stress_mpa)
                    .chain([
                        element.volume_mm3,
                        element.energy_n_mm,
                        element.concentration,
                    ])
                    .any(|value| !value.is_finite())
                || element.energy_n_mm < 0.0
                || element.concentration < 0.0
            {
                return Err(fail("SPLA element has invalid fields/connectivity"));
            }
        }
        for index in 0..patches {
            let patch = result.patch(index).unwrap();
            if patch.patch_index as usize >= result.summary.case.case.loads.len()
                || !patch.area_mm2.is_finite()
                || patch.area_mm2 <= 0.0
                || patch
                    .point_mm
                    .into_iter()
                    .chain(patch.traction_mpa)
                    .any(|value| !value.is_finite())
                || read_u32(bytes, patch_offset + index * PATCH + 60) != 0
            {
                return Err(fail("SPLA patch has invalid quadrature"));
            }
        }
        result.validate_semantics()?;
        Ok(result)
    }
    fn validate_semantics(&self) -> Result<(), ProjectError> {
        use std::collections::{BTreeMap, BTreeSet};
        let close = |actual: f64, expected: f64| {
            actual.is_finite()
                && expected.is_finite()
                && (actual - expected).abs() <= 1e-8 * (1.0 + expected.abs())
        };
        let cross = |point: [f64; 3], force: [f64; 3]| {
            [
                point[1] * force[2] - point[2] * force[1],
                point[2] * force[0] - point[0] * force[2],
                point[0] * force[1] - point[1] * force[0],
            ]
        };
        let mut used = vec![false; self.node_count];
        let mut positions = BTreeSet::new();
        let mut nodal_force = [0.0; 3];
        let mut nodal_moment = [0.0; 3];
        let mut maximum_displacement: f64 = 0.0;
        for index in 0..self.node_count {
            let node = self.node(index).unwrap();
            let key = node
                .position_mm
                .map(|value| if value == 0.0 { 0 } else { value.to_bits() });
            if !positions.insert(key) {
                return Err(fail("SPLA has duplicate disconnected node positions"));
            }
            let moment = cross(node.position_mm, node.applied_force_n);
            for axis in 0..3 {
                nodal_force[axis] += node.applied_force_n[axis];
                nodal_moment[axis] += moment[axis];
            }
            maximum_displacement = maximum_displacement.max(
                node.displacement_mm
                    .iter()
                    .map(|value| value * value)
                    .sum::<f64>()
                    .sqrt(),
            );
        }
        let mut volumes = [0.0; 3];
        let mut energies = [0.0; 3];
        let mut maximum_mises: f64 = 0.0;
        let mut faces = BTreeMap::<[u32; 9], (usize, usize, usize)>::new();
        for index in 0..self.element_count {
            let element = self.element(index).unwrap();
            let mut distinct = BTreeSet::new();
            for (local, &node) in element.nodes.iter().enumerate() {
                if !distinct.insert(node) {
                    return Err(fail("SPLA element repeats a node"));
                }
                used[node as usize] = true;
                let local_position = [local % 3, (local / 3) % 3, local / 9];
                let position = self.node(node as usize).unwrap().position_mm;
                if (0..3).any(|axis| {
                    !close(
                        position[axis] - element.center_mm[axis],
                        (local_position[axis] as f64 - 1.0) * 0.5 * element.size_mm[axis],
                    )
                }) {
                    return Err(fail(
                        "SPLA connectivity differs from its affine Hex27 chart",
                    ));
                }
            }
            if !close(element.volume_mm3, element.size_mm.iter().product()) {
                return Err(fail("SPLA envelope volume differs from element geometry"));
            }
            for other in 0..index {
                let other = self.element(other).unwrap();
                if (0..3).all(|axis| {
                    (element.center_mm[axis] - other.center_mm[axis]).abs()
                        < 0.5 * (element.size_mm[axis] + other.size_mm[axis]) - 1e-9
                }) {
                    return Err(fail("SPLA elements overlap in positive volume"));
                }
            }
            for axis in 0..3 {
                for side in 0..2 {
                    let mut face = [0; 9];
                    let mut offset = 0;
                    for (local, &node) in element.nodes.iter().enumerate() {
                        if [local % 3, (local / 3) % 3, local / 9][axis] == 2 * side {
                            face[offset] = node;
                            offset += 1;
                        }
                    }
                    face.sort_unstable();
                    let entry = faces.entry(face).or_insert((0, index, axis));
                    entry.0 += 1;
                    if entry.0 > 2 {
                        return Err(fail("SPLA nonmanifold element face"));
                    }
                }
            }
            volumes[element.role as usize] += element.volume_mm3;
            energies[element.role as usize] += element.energy_n_mm;
            let stress = element.stress_mpa;
            let mises = (0.5
                * ((stress[0] - stress[1]).powi(2)
                    + (stress[1] - stress[2]).powi(2)
                    + (stress[2] - stress[0]).powi(2))
                + 3.0 * (stress[3].powi(2) + stress[4].powi(2) + stress[5].powi(2)))
            .sqrt();
            maximum_mises = maximum_mises.max(mises);
        }
        if used.contains(&false)
            || !(0..3).all(|role| {
                close(volumes[role], self.summary.region_volume_mm3[role])
                    && close(energies[role], self.summary.region_energy_n_mm[role])
            })
            || !close(energies.iter().sum(), self.summary.strain_energy_n_mm)
            || !close(
                volumes[0] + volumes[1] + volumes[2] * self.summary.settings.recipe.infill_fraction,
                self.summary.nominal_material_volume_mm3,
            )
            || !close(maximum_displacement, self.summary.maximum_displacement_mm)
            || !close(maximum_mises, self.summary.maximum_macro_von_mises_mpa)
        {
            return Err(fail(
                "SPLA nodal/extrema/region totals differ from retained summary",
            ));
        }
        let mut patch_force = vec![[0.0; 3]; self.summary.case.case.loads.len()];
        let mut patch_moment = patch_force.clone();
        let mut seen = BTreeSet::new();
        let mut loaded_faces = BTreeMap::<[u32; 9], (u32, usize)>::new();
        let mut assembled_force = vec![[0.0; 3]; self.node_count];
        for index in 0..self.patch_count {
            let sample = self.patch(index).unwrap();
            if !seen.insert((sample.patch_index, sample.point_mm.map(f64::to_bits))) {
                return Err(fail("SPLA repeats patch quadrature"));
            }
            let boundary = faces.iter().find(|(nodes, entry)| {
                let (count, element, axis) = **entry;
                if count != 1 {
                    return false;
                }
                let face_point = self.node(nodes[0] as usize).unwrap().position_mm;
                let element = self.element(element).unwrap();
                close(sample.point_mm[axis] - face_point[axis], 0.0)
                    && (0..3).filter(|&tangent| tangent != axis).all(|tangent| {
                        (sample.point_mm[tangent] - element.center_mm[tangent]).abs()
                            < 0.5 * element.size_mm[tangent]
                    })
            });
            let Some((nodes, &(_, element, axis))) = boundary else {
                return Err(fail(
                    "SPLA patch quadrature is not on an exterior modeled face",
                ));
            };
            let element = self.element(element).unwrap();
            let mut expected_area = 1.0;
            for tangent in (0..3).filter(|&tangent| tangent != axis) {
                let coordinate = 2.0 * (sample.point_mm[tangent] - element.center_mm[tangent])
                    / element.size_mm[tangent];
                let weight = if close(coordinate, 0.0) {
                    8.0 / 9.0
                } else if close(coordinate.abs(), (3.0_f64 / 5.0).sqrt()) {
                    5.0 / 9.0
                } else {
                    return Err(fail(
                        "SPLA patch is not full-order retained face quadrature",
                    ));
                };
                expected_area *= 0.5 * element.size_mm[tangent] * weight;
            }
            if !close(sample.area_mm2, expected_area) {
                return Err(fail(
                    "SPLA patch quadrature weight differs from modeled face",
                ));
            }
            let entry = loaded_faces
                .entry(*nodes)
                .or_insert((sample.patch_index, 0));
            if entry.0 != sample.patch_index {
                return Err(fail("SPLA assigns one modeled face to two source patches"));
            }
            entry.1 += 1;
            let force = sample
                .traction_mpa
                .map(|traction| traction * sample.area_mm2);
            let local: [f64; 3] = std::array::from_fn(|axis| {
                2.0 * (sample.point_mm[axis] - element.center_mm[axis]) / element.size_mm[axis]
            });
            let basis = local.map(|coordinate| {
                [
                    0.5 * coordinate * (coordinate - 1.0),
                    1.0 - coordinate * coordinate,
                    0.5 * coordinate * (coordinate + 1.0),
                ]
            });
            for (local, &node) in element.nodes.iter().enumerate() {
                let shape = basis[0][local % 3] * basis[1][(local / 3) % 3] * basis[2][local / 9];
                for axis in 0..3 {
                    assembled_force[node as usize][axis] += shape * force[axis];
                }
            }
            let moment = cross(sample.point_mm, force);
            for axis in 0..3 {
                patch_force[sample.patch_index as usize][axis] += force[axis];
                patch_moment[sample.patch_index as usize][axis] += moment[axis];
            }
        }
        if loaded_faces.values().any(|entry| entry.1 != 9) {
            return Err(fail("SPLA patch coverage omits complete face quadrature"));
        }
        for (index, assembled) in assembled_force.iter().enumerate() {
            let retained = self.node(index).unwrap().applied_force_n;
            if (0..3).any(|axis| !close(assembled[axis], retained[axis])) {
                return Err(fail(
                    "SPLA nodal loads differ from retained finite-face quadrature assembly",
                ));
            }
        }
        for axis in 0..3 {
            let mut total_force = 0.0;
            let mut total_moment = 0.0;
            for (index, load) in self.summary.case.case.loads.iter().enumerate() {
                if !close(patch_force[index][axis], load.force_n[axis])
                    || !close(patch_moment[index][axis], load.moment_n_mm[axis])
                {
                    return Err(fail(
                        "SPLA patch quadrature does not conserve captured source-origin wrench",
                    ));
                }
                total_force += load.force_n[axis];
                total_moment += load.moment_n_mm[axis];
            }
            if !close(nodal_force[axis], total_force) || !close(nodal_moment[axis], total_moment) {
                return Err(fail(
                    "SPLA nodal loads do not conserve captured source-origin wrench",
                ));
            }
        }
        Ok(())
    }
    pub fn node(&self, index: usize) -> Option<AnalysisNode> {
        if index >= self.node_count {
            return None;
        }
        let offset = self.node_offset + NODE * index;
        Some(AnalysisNode {
            position_mm: read_values(self.bytes, offset),
            displacement_mm: read_values(self.bytes, offset + 24),
            applied_force_n: read_values(self.bytes, offset + 48),
        })
    }
    pub fn element(&self, index: usize) -> Option<AnalysisElement> {
        if index >= self.element_count {
            return None;
        }
        let offset = self.element_offset + ELEMENT * index;
        Some(AnalysisElement {
            nodes: std::array::from_fn(|node| read_u32(self.bytes, offset + 4 * node)),
            center_mm: read_values(self.bytes, offset + 108),
            size_mm: read_values(self.bytes, offset + 132),
            role: read_u32(self.bytes, offset + 156),
            volume_mm3: read_values::<1>(self.bytes, offset + 160)[0],
            stress_mpa: read_values(self.bytes, offset + 168),
            energy_n_mm: read_values::<1>(self.bytes, offset + 216)[0],
            concentration: read_values::<1>(self.bytes, offset + 224)[0],
        })
    }
    pub fn patch(&self, index: usize) -> Option<AnalysisPatchSample> {
        if index >= self.patch_count {
            return None;
        }
        let offset = self.patch_offset + PATCH * index;
        Some(AnalysisPatchSample {
            point_mm: read_values(self.bytes, offset),
            area_mm2: read_values::<1>(self.bytes, offset + 24)[0],
            traction_mpa: read_values(self.bytes, offset + 32),
            patch_index: read_u32(self.bytes, offset + 56),
        })
    }
}
pub fn encode(
    summary: &AnalysisRunSummary,
    nodes: &[AnalysisNode],
    elements: &[AnalysisElement],
    patches: &[AnalysisPatchSample],
) -> Result<Vec<u8>, ProjectError> {
    summary.validate()?;
    let metadata = serde_json::to_vec(summary).map_err(|error| fail(&error.to_string()))?;
    if metadata.len() > METADATA_LIMIT
        || nodes.len() > MAX_MECHANICAL_DOFS / 3
        || elements.len() > MAX_MECHANICAL_ELEMENTS
        || patches.len() > MAX_MECHANICAL_ELEMENTS * 54 * MAX_ANALYSIS_PATCHES
    {
        return Err(fail("SPLA output counts exceed budget"));
    }
    let count = HEADER
        + metadata.len()
        + NODE * nodes.len()
        + ELEMENT * elements.len()
        + PATCH * patches.len();
    if count > MAX_MECHANICAL_FIELD_BYTES {
        return Err(fail("SPLA field bytes exceed budget"));
    }
    let mut bytes = Vec::with_capacity(count);
    bytes.extend_from_slice(b"SPLA");
    for value in [
        1,
        metadata.len() as u32,
        nodes.len() as u32,
        elements.len() as u32,
        patches.len() as u32,
        0,
        0,
    ] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(&metadata);
    fn values(bytes: &mut Vec<u8>, values: impl IntoIterator<Item = f64>) {
        for value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    for node in nodes {
        values(
            &mut bytes,
            node.position_mm
                .into_iter()
                .chain(node.displacement_mm)
                .chain(node.applied_force_n),
        );
    }
    for element in elements {
        for node in element.nodes {
            bytes.extend_from_slice(&node.to_le_bytes());
        }
        values(
            &mut bytes,
            element.center_mm.into_iter().chain(element.size_mm),
        );
        bytes.extend_from_slice(&element.role.to_le_bytes());
        values(
            &mut bytes,
            [element.volume_mm3]
                .into_iter()
                .chain(element.stress_mpa)
                .chain([element.energy_n_mm, element.concentration]),
        );
    }
    for patch in patches {
        values(
            &mut bytes,
            patch
                .point_mm
                .into_iter()
                .chain([patch.area_mm2])
                .chain(patch.traction_mpa),
        );
        bytes.extend_from_slice(&patch.patch_index.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
    }
    AnalysisFields::decode(&bytes)?;
    Ok(bytes)
}
