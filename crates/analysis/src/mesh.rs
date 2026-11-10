// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::{Budget, native::NativePrism};
use spiling_contracts::{
    analysis::mechanics::*,
    geometry::{FaceId, RigidPoseMm},
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
pub struct Hex {
    pub midpoint_weight: f64,
    pub nodes: [usize; 27],
    pub lower: [f64; 3],
    pub size: [f64; 3],
    pub role: usize,
    pub component: usize,
}
#[derive(Clone)]
pub struct Facet {
    pub element: usize,
    pub axis: usize,
    pub side: usize,
    pub face: Option<FaceId>,
}
pub struct Mesh {
    pub geometry: spiling_contracts::analysis::fastener::AnalysisMeshGeometry,
    pub nodes: Vec<[f64; 3]>,
    pub elements: Vec<Hex>,
    pub facets: Vec<Facet>,
    pub components: usize,
    pub service_to_print: [[f64; 3]; 3],
    pub layer_axis: usize,
}
fn admit_cuts(cuts: &mut Vec<f64>) -> Result<(), String> {
    cuts.sort_by(f64::total_cmp);
    cuts.dedup();
    if cuts.len() < 2
        || cuts.iter().any(|value| !value.is_finite())
        || cuts.windows(2).any(|pair| pair[1] - pair[0] <= 1e-8)
    {
        return Err("UnresolvedModel: distinct native/construction planes are too close; no gap or thin region was merged away".into());
    }
    Ok(())
}
pub fn rotation(pose: RigidPoseMm) -> [[f64; 3]; 3] {
    let [qx, qy, qz, qw] = pose.rotation_xyzw;
    [
        [
            1.0 - 2.0 * (qy * qy + qz * qz),
            2.0 * (qx * qy - qz * qw),
            2.0 * (qx * qz + qy * qw),
        ],
        [
            2.0 * (qx * qy + qz * qw),
            1.0 - 2.0 * (qx * qx + qz * qz),
            2.0 * (qy * qz - qx * qw),
        ],
        [
            2.0 * (qx * qz - qy * qw),
            2.0 * (qy * qz + qx * qw),
            1.0 - 2.0 * (qx * qx + qy * qy),
        ],
    ]
}

pub fn build(
    prism: &NativePrism,
    settings: &AnalysisRunSettings,
    pose: RigidPoseMm,
    budget: &Budget,
) -> Result<Mesh, String> {
    let service_to_print = rotation(pose);
    if service_to_print
        .iter()
        .any(|row| row.iter().filter(|entry| entry.abs() > 1e-8).count() != 1)
    {
        return Err("UnsupportedModel: initial partition needs an axis-aligned print orientation; service loads are never rotated to fit it".into());
    }
    let layer_axis = (0..3)
        .max_by(|first, second| {
            service_to_print[2][*first]
                .abs()
                .total_cmp(&service_to_print[2][*second].abs())
        })
        .unwrap();
    if let Some(bore) = &prism.bore {
        if layer_axis != 2 {
            return Err("UnsupportedModel: the curved-hole construction currently needs print layers normal to its bore axis; loads are not rotated to make it fit".into());
        }
        return crate::curved::build(bore, settings, service_to_print, budget);
    }
    let mut cuts = [Vec::new(), Vec::new(), prism.height.to_vec()];
    for edge in &prism.edges {
        for point in edge.ends {
            cuts[0].push(point[0]);
            cuts[1].push(point[1]);
        }
    }
    let wall = settings.recipe.perimeter_count as f64 * settings.recipe.bead_width_mm;
    let skin = settings
        .recipe
        .top_skin_thickness_mm
        .max(settings.recipe.bottom_skin_thickness_mm);
    if settings.recipe.top_skin_thickness_mm != settings.recipe.bottom_skin_thickness_mm {
        return Err("UnsupportedModel: initial symmetric skin partition requires equal explicit top/bottom thickness".into());
    }
    for (axis, axis_cuts) in cuts.iter_mut().enumerate() {
        admit_cuts(axis_cuts)?;
        let original = axis_cuts.clone();
        let thickness = if axis == layer_axis { skin } else { wall };
        for value in &original {
            for sign in [-1.0, 1.0] {
                let candidate = value + sign * thickness;
                if candidate > original[0] && candidate < original[original.len() - 1] {
                    axis_cuts.push(candidate);
                }
            }
        }
        admit_cuts(axis_cuts)?;
        let original = axis_cuts.clone();
        for interval in original.windows(2) {
            let count = ((interval[1] - interval[0]) / settings.maximum_span_mm).ceil() as usize;
            if count > 512 {
                return Err("ResourceLimit: mesh axis subdivision".into());
            }
            for index in 1..count {
                axis_cuts
                    .push(interval[0] + (interval[1] - interval[0]) * index as f64 / count as f64);
            }
        }
        admit_cuts(axis_cuts)?;
        if axis_cuts.len() > 128 {
            return Err("ResourceLimit: mesh axis cuts".into());
        }
    }
    let mut cells = BTreeMap::new();
    let mut elements = Vec::new();
    let mut nodes = Vec::new();
    let mut node_map = BTreeMap::new();
    for z_index in 0..cuts[2].len() - 1 {
        for y_index in 0..cuts[1].len() - 1 {
            for x_index in 0..cuts[0].len() - 1 {
                budget.check()?;
                let cell = [x_index, y_index, z_index];
                let lower = std::array::from_fn(|axis| cuts[axis][cell[axis]]);
                let size: [f64; 3] =
                    std::array::from_fn(|axis| cuts[axis][cell[axis] + 1] - lower[axis]);
                let center: [f64; 3] = std::array::from_fn(|axis| lower[axis] + 0.5 * size[axis]);
                if !prism.contains(center) {
                    continue;
                }
                if elements.len() == MAX_MECHANICAL_ELEMENTS {
                    return Err("ResourceLimit: whole-part mesh exceeds element admission; no cells were dropped".into());
                }
                let boundary_distance = |axis: usize| {
                    if axis == 2 {
                        return (center[2] - prism.height[0]).min(prism.height[1] - center[2]);
                    }
                    prism
                        .edges
                        .iter()
                        .filter_map(|edge| {
                            let tangent = 1 - axis;
                            ((edge.ends[0][axis] - edge.ends[1][axis]).abs() < 1e-8
                                && center[tangent]
                                    > edge.ends[0][tangent].min(edge.ends[1][tangent]) - 1e-8
                                && center[tangent]
                                    < edge.ends[0][tangent].max(edge.ends[1][tangent]) + 1e-8)
                                .then_some((center[axis] - edge.ends[0][axis]).abs())
                        })
                        .min_by(f64::total_cmp)
                        .unwrap_or(f64::INFINITY)
                };
                let role = if boundary_distance(layer_axis) < skin {
                    1
                } else if (0..3).any(|axis| axis != layer_axis && boundary_distance(axis) < wall) {
                    0
                } else {
                    2
                };
                let mut element_nodes = [0; 27];
                for local_z in 0..3 {
                    for local_y in 0..3 {
                        for local_x in 0..3 {
                            let local = [local_x, local_y, local_z];
                            let key: [usize; 3] =
                                std::array::from_fn(|axis| 2 * cell[axis] + local[axis]);
                            let node = *node_map.entry(key).or_insert_with(|| {
                                nodes.push(std::array::from_fn(|axis| {
                                    lower[axis] + local[axis] as f64 * 0.5 * size[axis]
                                }));
                                nodes.len() - 1
                            });
                            element_nodes[local_x + 3 * local_y + 9 * local_z] = node;
                        }
                    }
                }
                cells.insert(cell, elements.len());
                elements.push(Hex {
                    midpoint_weight: 1.0,
                    nodes: element_nodes,
                    lower,
                    size,
                    role,
                    component: usize::MAX,
                });
            }
        }
    }
    if nodes.len() * 3 > MAX_MECHANICAL_DOFS {
        return Err("ResourceLimit: whole-part DOF admission".into());
    }
    let mut facets = Vec::new();
    let mut adjacency = vec![Vec::new(); elements.len()];
    for (cell, &element) in &cells {
        for axis in 0..3 {
            for side in 0..2 {
                let mut neighbor = *cell;
                let adjacent = if side == 0 {
                    neighbor[axis].checked_sub(1)
                } else {
                    neighbor[axis].checked_add(1)
                }
                .and_then(|index| {
                    neighbor[axis] = index;
                    cells.get(&neighbor).copied()
                });
                if let Some(other) = adjacent {
                    adjacency[element].push(other);
                    continue;
                }
                let hex = &elements[element];
                let coordinate = hex.lower[axis] + side as f64 * hex.size[axis];
                let mut owners = BTreeSet::new();
                if axis < 2 {
                    for edge in &prism.edges {
                        let tangent = 1 - axis;
                        if edge
                            .ends
                            .iter()
                            .all(|point| (point[axis] - coordinate).abs() < 1e-7)
                            && hex.lower[tangent]
                                >= edge.ends[0][tangent].min(edge.ends[1][tangent]) - 1e-7
                            && hex.lower[tangent] + hex.size[tangent]
                                <= edge.ends[0][tangent].max(edge.ends[1][tangent]) + 1e-7
                        {
                            owners.insert(edge.face.clone());
                        }
                    }
                }
                if axis < 2 && owners.len() != 1 {
                    return Err(
                        "UnmappedLoad: whole mesh boundary lacks one native span owner".into(),
                    );
                }
                facets.push(Facet {
                    element,
                    axis,
                    side,
                    face: owners.into_iter().next(),
                });
            }
        }
    }
    let mut components = 0;
    for initial in 0..elements.len() {
        if elements[initial].component == usize::MAX {
            let mut pending = vec![initial];
            elements[initial].component = components;
            while let Some(element) = pending.pop() {
                for &neighbor in &adjacency[element] {
                    if elements[neighbor].component == usize::MAX {
                        elements[neighbor].component = components;
                        pending.push(neighbor);
                    }
                }
            }
            components += 1;
        }
    }
    let mut node_components = vec![None; nodes.len()];
    for element in &elements {
        for &node in &element.nodes {
            if node_components[node].is_some_and(|component| component != element.component) {
                return Err(
                    "DisconnectedOrMechanism: zero-area component contact cannot be welded".into(),
                );
            }
            node_components[node] = Some(element.component);
        }
    }
    let mut stars = vec![Vec::new(); nodes.len()];
    for (index, element) in elements.iter().enumerate() {
        for &node in &element.nodes {
            stars[node].push(index);
        }
    }
    for star in stars {
        let mut reached = BTreeSet::from([star[0]]);
        let mut pending = vec![star[0]];
        while let Some(element) = pending.pop() {
            for &neighbor in &adjacency[element] {
                if star.contains(&neighbor) && reached.insert(neighbor) {
                    pending.push(neighbor);
                }
            }
        }
        if reached.len() != star.len() {
            return Err(
                "DisconnectedOrMechanism: local point/edge contact cannot be tied across a gap"
                    .into(),
            );
        }
    }
    Ok(Mesh {
        geometry: spiling_contracts::analysis::fastener::AnalysisMeshGeometry::AffineHex27 {},
        nodes,
        elements,
        facets,
        components,
        service_to_print,
        layer_axis,
    })
}
