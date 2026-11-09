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
        Ok(result)
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
