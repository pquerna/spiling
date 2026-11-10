// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::analysis::{attachment, chart, fastener::*};
use std::collections::{BTreeMap, BTreeSet};

fn close(actual: f64, expected: f64) -> bool {
    actual.is_finite()
        && expected.is_finite()
        && (actual - expected).abs() <= 1e-8 * (1.0 + expected.abs())
}
fn magnitude(vector: [f64; 3]) -> f64 {
    chart::dot(vector, vector).sqrt()
}
fn face_key(element: &AnalysisElement, axis: usize, side: usize) -> [u32; 9] {
    let mut key = [0; 9];
    for (index, node) in chart::facet_nodes(axis, side).enumerate() {
        key[index] = element.nodes[node];
    }
    key.sort_unstable();
    key
}
fn source_face(
    profile: &AnalysisBoreProfile,
    positions: &[[f64; 3]; 27],
    axis: usize,
    side: usize,
) -> Result<Option<FaceId>, ProjectError> {
    let points = chart::facet_nodes(axis, side)
        .map(|node| positions[node])
        .collect::<Vec<_>>();
    for end in 0..2 {
        let height = profile.hole.center_mm[2] + end as f64 * profile.hole.depth_mm;
        if points.iter().all(|point| (point[2] - height).abs() <= 1e-7) {
            return Ok(Some(profile.hole.cap_faces[end].clone()));
        }
    }
    for normal in 0..2 {
        for end in 0..2 {
            if points
                .iter()
                .all(|point| (point[normal] - profile.outer_xy_mm[normal][end]).abs() <= 1e-7)
            {
                return Ok(Some(profile.outer_faces[2 * normal + end].clone()));
            }
        }
    }
    if axis != 1 || side != 0 {
        return Err(fail(
            "SPLA exterior facet is not on the declared native plate boundary",
        ));
    }
    Ok(None)
}
fn solve_six(mut matrix: [[f64; 6]; 6], mut rhs: [f64; 6]) -> Result<[f64; 6], ProjectError> {
    let scale = matrix
        .iter()
        .flatten()
        .map(|value| value.abs())
        .fold(0.0, f64::max);
    for column in 0..6 {
        let pivot = (column..6)
            .max_by(|first, second| {
                matrix[*first][column]
                    .abs()
                    .total_cmp(&matrix[*second][column].abs())
            })
            .unwrap();
        if !matrix[pivot][column].is_finite() || matrix[pivot][column].abs() <= 1e-12 * scale {
            return Err(fail("SPLA fastener reaction projection is singular"));
        }
        matrix.swap(column, pivot);
        rhs.swap(column, pivot);
        let divisor = matrix[column][column];
        for entry in &mut matrix[column][column..] {
            *entry /= divisor;
        }
        rhs[column] /= divisor;
        let pivot_row = matrix[column];
        for row in 0..6 {
            if row == column {
                continue;
            }
            let factor = matrix[row][column];
            for (entry, pivot) in matrix[row][column..].iter_mut().zip(&pivot_row[column..]) {
                *entry -= factor * pivot;
            }
            rhs[row] -= factor * rhs[column];
        }
    }
    Ok(rhs)
}

impl AnalysisFields<'_> {
    pub(super) fn validate_semantics(&self) -> Result<(), ProjectError> {
        let nodes = (0..self.node_count)
            .map(|index| self.node(index).unwrap())
            .collect::<Vec<_>>();
        let elements = (0..self.element_count)
            .map(|index| self.element(index).unwrap())
            .collect::<Vec<_>>();
        let positions = elements
            .iter()
            .map(|element| element.nodes.map(|node| nodes[node as usize].position_mm))
            .collect::<Vec<_>>();
        let displacements = nodes
            .iter()
            .map(|node| node.displacement_mm)
            .collect::<Vec<_>>();
        let profile = match &self.summary.mesh_geometry {
            AnalysisMeshGeometry::RationalBoredPlate { profile } => Some(profile),
            AnalysisMeshGeometry::AffineHex27 {} => None,
        };
        let rotation = chart::service_rotation(self.summary.pose);
        let tensors = [
            self.summary.settings.laws.wall_mpa,
            self.summary.settings.laws.skin_mpa,
            self.summary.settings.laws.effective_core_mpa,
        ];
        let layer_axis = (0..3)
            .max_by(|first, second| {
                rotation[2][*first]
                    .abs()
                    .total_cmp(&rotation[2][*second].abs())
            })
            .unwrap();
        let mut unique_positions = BTreeSet::new();
        let mut external_work = 0.0;
        let mut applied_force = [0.0; 3];
        let mut applied_moment = [0.0; 3];
        let mut maximum_displacement: f64 = 0.0;
        for node in &nodes {
            if !unique_positions.insert(
                node.position_mm
                    .map(|value| if value == 0.0 { 0 } else { value.to_bits() }),
            ) {
                return Err(fail("SPLA has duplicate disconnected node positions"));
            }
            external_work += chart::dot(node.applied_force_n, node.displacement_mm);
            maximum_displacement = maximum_displacement.max(magnitude(node.displacement_mm));
            let moment = chart::cross(node.position_mm, node.applied_force_n);
            for axis in 0..3 {
                applied_force[axis] += node.applied_force_n[axis];
                applied_moment[axis] += moment[axis];
            }
        }
        let work_error = (external_work - 2.0 * self.summary.strain_energy_n_mm).abs()
            / external_work.abs().max(1e-30);
        if external_work <= 0.0
            || !external_work.is_finite()
            || work_error > 1e-7
            || !close(
                work_error,
                self.summary.assessment.work_energy_relative_error,
            )
        {
            return Err(fail(
                "SPLA displacement work differs from retained energy/assessment",
            ));
        }
        let mut used = vec![false; nodes.len()];
        let mut faces = BTreeMap::<[u32; 9], Vec<(usize, usize, usize)>>::new();
        let mut internal = vec![[0.0; 3]; nodes.len()];
        let mut volumes = [0.0; 3];
        let mut energies = [0.0; 3];
        let mut maximum_mises: f64 = 0.0;
        let mut maximum_layer: f64 = 0.0;
        let mut density_samples = Vec::new();
        for (index, element) in elements.iter().enumerate() {
            let mut distinct = BTreeSet::new();
            for &node in &element.nodes {
                if !distinct.insert(node) {
                    return Err(fail("SPLA element repeats a node"));
                }
                used[node as usize] = true;
            }
            if let Some(profile) = profile {
                chart::validate_extruded_cell(&positions[index], element.midpoint_weight)
                    .map_err(|error| fail(&error))?;
                if element.midpoint_weight >= 1.0 {
                    return Err(fail("SPLA curved element has no rational angular chart"));
                }
                for point in &positions[index] {
                    if (0..2).any(|axis| {
                        point[axis] < profile.outer_xy_mm[axis][0] - 1e-7
                            || point[axis] > profile.outer_xy_mm[axis][1] + 1e-7
                    }) || point[2] < profile.hole.center_mm[2] - 1e-7
                        || point[2] > profile.hole.center_mm[2] + profile.hole.depth_mm + 1e-7
                        || (point[0] - profile.hole.center_mm[0])
                            .hypot(point[1] - profile.hole.center_mm[1])
                            < profile.hole.radius_mm - 1e-7
                    {
                        return Err(fail(
                            "SPLA curved node lies outside the native plate material",
                        ));
                    }
                }
                for axis in 0..3 {
                    let minimum = positions[index]
                        .iter()
                        .map(|point| point[axis])
                        .fold(f64::INFINITY, f64::min);
                    let maximum = positions[index]
                        .iter()
                        .map(|point| point[axis])
                        .fold(f64::NEG_INFINITY, f64::max);
                    if !close(maximum - minimum, element.size_mm[axis]) {
                        return Err(fail("SPLA curved envelope differs from its stored nodes"));
                    }
                }
            } else {
                if element.midpoint_weight != 1.0 {
                    return Err(fail("SPLA affine chart has a rational weight"));
                }
                for (local, &point) in positions[index].iter().enumerate() {
                    let coordinate = [local % 3, (local / 3) % 3, local / 9];
                    if (0..3).any(|axis| {
                        !close(
                            point[axis] - element.center_mm[axis],
                            (coordinate[axis] as f64 - 1.0) * 0.5 * element.size_mm[axis],
                        )
                    }) {
                        return Err(fail(
                            "SPLA connectivity differs from its affine Hex27 chart",
                        ));
                    }
                }
                for other in &elements[..index] {
                    if (0..3).all(|axis| {
                        (element.center_mm[axis] - other.center_mm[axis]).abs()
                            < 0.5 * (element.size_mm[axis] + other.size_mm[axis]) - 1e-9
                    }) {
                        return Err(fail("SPLA elements overlap in positive volume"));
                    }
                }
            }
            for axis in 0..3 {
                for side in 0..2 {
                    let entry = faces.entry(face_key(element, axis, side)).or_default();
                    entry.push((index, axis, side));
                    if entry.len() > 2 {
                        return Err(fail("SPLA nonmanifold element face"));
                    }
                }
            }
            let local = element
                .nodes
                .map(|node| nodes[node as usize].displacement_mm);
            let mut volume = 0.0;
            let mut energy = 0.0;
            let mut mapping_error = None;
            chart::quadrature(element.midpoint_weight, |point, weight| {
                let mapped = match chart::mapped(&positions[index], element.midpoint_weight, point)
                {
                    Ok(mapped) => mapped,
                    Err(reason) => {
                        mapping_error = Some(reason);
                        return;
                    }
                };
                let physical_weight = weight * mapped.determinant;
                let strain = chart::strain(&mapped.gradients, &local);
                let stress =
                    chart::service_stress(&tensors[element.role as usize], rotation, strain);
                let density = 0.5 * (0..6).map(|axis| strain[axis] * stress[axis]).sum::<f64>();
                volume += physical_weight;
                energy += density * physical_weight;
                for node in 0..27 {
                    let force = chart::internal_force(mapped.gradients[node], stress);
                    for axis in 0..3 {
                        internal[element.nodes[node] as usize][axis] +=
                            physical_weight * force[axis];
                    }
                }
                let fraction = if element.role == 2 {
                    self.summary.settings.recipe.infill_fraction
                } else {
                    1.0
                };
                density_samples.push((
                    mapped.position,
                    physical_weight * fraction,
                    density / fraction,
                ));
            });
            if let Some(reason) = mapping_error {
                return Err(fail(reason));
            }
            let center = chart::mapped(&positions[index], element.midpoint_weight, [0.0; 3])
                .map_err(fail)?;
            let stress = chart::service_stress(
                &tensors[element.role as usize],
                rotation,
                chart::strain(&center.gradients, &local),
            );
            if (0..3).any(|axis| !close(center.position[axis] - element.center_mm[axis], 0.0))
                || (0..6).any(|axis| !close(stress[axis], element.stress_mpa[axis]))
                || !close(volume, element.volume_mm3)
                || !close(energy, element.energy_n_mm)
            {
                return Err(fail(
                    "SPLA original-coefficient element stress/energy/volume differs from retained fields",
                ));
            }
            volumes[element.role as usize] += volume;
            energies[element.role as usize] += energy;
            let stress = element.stress_mpa;
            maximum_mises = maximum_mises.max(
                (0.5 * ((stress[0] - stress[1]).powi(2)
                    + (stress[1] - stress[2]).powi(2)
                    + (stress[2] - stress[0]).powi(2))
                    + 3.0 * (stress[3].powi(2) + stress[4].powi(2) + stress[5].powi(2)))
                .sqrt(),
            );
            maximum_layer = maximum_layer.max(stress[layer_axis].abs());
        }
        let mut adjacency = vec![Vec::new(); elements.len()];
        let mut exterior = Vec::new();
        let mut exterior_sources = BTreeMap::new();
        for (key, uses) in &faces {
            if uses.len() == 2 {
                adjacency[uses[0].0].push(uses[1].0);
                adjacency[uses[1].0].push(uses[0].0);
            } else {
                let (element, axis, side) = uses[0];
                if let Some(profile) = profile {
                    let owner = source_face(profile, &positions[element], axis, side)?;
                    if owner.is_none() {
                        let weight = elements[element].midpoint_weight;
                        let values: [[_; 3]; 3] = std::array::from_fn(|angular| {
                            let point = positions[element][angular];
                            [
                                point[0] - profile.hole.center_mm[0],
                                point[1] - profile.hole.center_mm[1],
                                0.0,
                            ]
                        });
                        let coefficients: [[f64; 3]; 2] = std::array::from_fn(|axis| {
                            [
                                weight * values[1][axis],
                                0.5 * (values[2][axis] - values[0][axis]),
                                0.5 * (values[2][axis] + values[0][axis])
                                    - weight * values[1][axis],
                            ]
                        });
                        let denominator = [weight, 0.0, 1.0 - weight];
                        let mut residual = [0.0; 5];
                        for first in 0..3 {
                            for second in 0..3 {
                                residual[first + second] += coefficients[0][first]
                                    * coefficients[0][second]
                                    + coefficients[1][first] * coefficients[1][second]
                                    - profile.hole.radius_mm.powi(2)
                                        * denominator[first]
                                        * denominator[second];
                            }
                        }
                        if residual
                            .iter()
                            .any(|value| value.abs() > 1e-8 * profile.hole.radius_mm)
                        {
                            return Err(fail(
                                "SPLA rational bore edge does not retain the complete native circle",
                            ));
                        }
                    }
                    exterior_sources.insert(*key, owner);
                }
                exterior.push((element, axis, side, *key));
            }
        }
        let mut visited = vec![false; elements.len()];
        let mut components = 0;
        for initial in 0..elements.len() {
            if !visited[initial] {
                components += 1;
                let mut pending = vec![initial];
                visited[initial] = true;
                while let Some(element) = pending.pop() {
                    for &neighbor in &adjacency[element] {
                        if !visited[neighbor] {
                            visited[neighbor] = true;
                            pending.push(neighbor);
                        }
                    }
                }
            }
        }
        let nominal =
            volumes[0] + volumes[1] + volumes[2] * self.summary.settings.recipe.infill_fraction;
        if used.contains(&false)
            || components != self.summary.components
            || (0..3).any(|role| {
                !close(volumes[role], self.summary.region_volume_mm3[role])
                    || !close(energies[role], self.summary.region_energy_n_mm[role])
            })
            || !close(energies.iter().sum(), self.summary.strain_energy_n_mm)
            || !close(nominal, self.summary.nominal_material_volume_mm3)
            || !close(maximum_displacement, self.summary.maximum_displacement_mm)
            || !close(maximum_mises, self.summary.maximum_macro_von_mises_mpa)
            || !close(maximum_layer, self.summary.maximum_macro_layer_normal_mpa)
        {
            return Err(fail(
                "SPLA nodal/extrema/region totals differ from retained summary",
            ));
        }
        if let Some(profile) = profile
            && !close(volumes.iter().sum(), profile.native_volume_mm3())
        {
            return Err(fail(
                "SPLA whole curved mesh does not conserve the native plate-minus-bore volume",
            ));
        }
        let global = self.summary.strain_energy_n_mm / nominal;
        let radius = 3.0 * self.summary.settings.recipe.bead_width_mm;
        if !close(radius, self.summary.concentration_radius_mm) {
            return Err(fail(
                "SPLA concentration neighborhood differs from the captured construction",
            ));
        }
        for element in &elements {
            let mut local_volume = 0.0;
            let mut local_energy = 0.0;
            for &(point, weight, density) in &density_samples {
                if (0..3)
                    .map(|axis| (point[axis] - element.center_mm[axis]).powi(2))
                    .sum::<f64>()
                    <= radius * radius
                {
                    local_volume += weight;
                    local_energy += weight * density;
                }
            }
            if local_volume <= 0.0
                || !close(local_energy / local_volume / global, element.concentration)
            {
                return Err(fail(
                    "SPLA concentration differs from independently reconstructed regional energy",
                ));
            }
        }
        let mut expected_loads =
            BTreeMap::<(usize, usize, usize, usize), Vec<([f64; 3], f64)>>::new();
        if profile.is_some() {
            for (load_index, load) in self.summary.case.case.loads.iter().enumerate() {
                for &(element, axis, side, key) in &exterior {
                    if exterior_sources.get(&key).and_then(Option::as_ref) != Some(&load.face_id) {
                        continue;
                    }
                    let (normal, center, size) =
                        chart::rectangle_facet(&positions[element], axis, side).map_err(fail)?;
                    expected_loads.insert(
                        (load_index, element, axis, side),
                        load::surface_samples(&load.footprint, center, size, normal, 1),
                    );
                }
            }
        }
        let mut seen = BTreeSet::new();
        let mut seen_loads = BTreeSet::new();
        let mut assembled = vec![[0.0; 3]; nodes.len()];
        let mut patch_force = vec![[0.0; 3]; self.summary.case.case.loads.len()];
        let mut patch_moment = patch_force.clone();
        let mut reactions = vec![Vec::new(); self.summary.case.case.fasteners.len()];
        for index in 0..self.patch_count {
            let sample = self.patch(index).unwrap();
            if !seen.insert((
                sample.patch_index,
                sample.element_index,
                sample.point_mm.map(f64::to_bits),
            )) {
                return Err(fail("SPLA repeats boundary quadrature"));
            }
            let patch = sample.patch_index as usize;
            let element = sample.element_index as usize;
            if patch >= self.summary.case.case.loads.len() {
                reactions[patch - self.summary.case.case.loads.len()].push(sample);
                continue;
            }
            let local = chart::locate(
                &positions[element],
                elements[element].midpoint_weight,
                sample.point_mm,
            )
            .map_err(fail)?;
            let boundary = exterior.iter().find(|&&(owner, axis, side, _)| {
                owner == element && (local[axis] - (2 * side) as f64 + 1.0).abs() < 1e-7
            });
            let Some(&(_, axis, side, _)) = boundary else {
                return Err(fail(
                    "SPLA load quadrature is not on an exterior modeled face",
                ));
            };
            let key = (patch, element, axis, side);
            if profile.is_none() {
                let (normal, center, size) =
                    chart::rectangle_facet(&positions[element], axis, side).map_err(fail)?;
                expected_loads.entry(key).or_insert_with(|| {
                    load::surface_samples(
                        &self.summary.case.case.loads[patch].footprint,
                        center,
                        size,
                        normal,
                        1,
                    )
                });
            }
            let expected = expected_loads
                .get(&key)
                .ok_or_else(|| fail("SPLA load face differs from the captured native owner"))?;
            let Some(ordinal) = expected.iter().position(|(point, weight)| {
                (0..3).all(|axis| close(point[axis] - sample.point_mm[axis], 0.0))
                    && close(*weight, sample.area_mm2)
            }) else {
                return Err(fail(
                    "SPLA load sample or weight differs from canonical physical-footprint quadrature",
                ));
            };
            if !seen_loads.insert((key, ordinal)) {
                return Err(fail("SPLA repeats a canonical physical load sample"));
            }
            let values = chart::reference(local, elements[element].midpoint_weight).0;
            let force = sample.traction_mpa.map(|value| value * sample.area_mm2);
            for node in 0..27 {
                for axis in 0..3 {
                    assembled[elements[element].nodes[node] as usize][axis] +=
                        values[node] * force[axis];
                }
            }
            let moment = chart::cross(sample.point_mm, force);
            for axis in 0..3 {
                patch_force[patch][axis] += force[axis];
                patch_moment[patch][axis] += moment[axis];
            }
        }
        if seen_loads.len() != expected_loads.values().map(Vec::len).sum::<usize>() {
            return Err(fail(
                "SPLA finite load footprint omits complete modeled-face quadrature",
            ));
        }
        if assembled
            .iter()
            .zip(&nodes)
            .any(|(force, node)| (0..3).any(|axis| !close(force[axis], node.applied_force_n[axis])))
        {
            return Err(fail(
                "SPLA nodal loads differ from retained finite-face quadrature assembly",
            ));
        }
        let mut requested_force = [0.0; 3];
        let mut requested_moment = [0.0; 3];
        for (index, load) in self.summary.case.case.loads.iter().enumerate() {
            for axis in 0..3 {
                if !close(patch_force[index][axis], load.force_n[axis])
                    || !close(patch_moment[index][axis], load.moment_n_mm[axis])
                {
                    return Err(fail(
                        "SPLA load quadrature does not conserve the captured source-origin wrench",
                    ));
                }
                requested_force[axis] += load.force_n[axis];
                requested_moment[axis] += load.moment_n_mm[axis];
            }
        }
        if (0..3).any(|axis| {
            !close(applied_force[axis], requested_force[axis])
                || !close(applied_moment[axis], requested_moment[axis])
        }) {
            return Err(fail(
                "SPLA nodal loads do not conserve the captured source-origin wrench",
            ));
        }
        let facets = exterior
            .iter()
            .map(|&(element, axis, side, _)| attachment::AttachmentFacet {
                element_index: element as u32,
                axis,
                side,
                nodes: elements[element].nodes,
                positions: positions[element],
                midpoint_weight: elements[element].midpoint_weight,
            })
            .collect::<Vec<_>>();
        let mut connector_forces = vec![[0.0; 3]; nodes.len()];
        for (index, fastener) in self.summary.case.case.fasteners.iter().enumerate() {
            let expected = attachment::samples(fastener, &facets)?;
            let exact: BTreeMap<_, _> = expected
                .iter()
                .enumerate()
                .map(|(ordinal, candidate)| {
                    (
                        (
                            candidate.element_index,
                            candidate.point_mm.map(f64::to_bits),
                            candidate.area_mm2.to_bits(),
                        ),
                        ordinal,
                    )
                })
                .collect();
            let coupling = attachment::coupling(fastener, &expected, nodes.len())?;
            let samples = &reactions[index];
            if expected.len() != samples.len() {
                return Err(fail(
                    "SPLA fastener contact omits canonical finite-region samples",
                ));
            }
            let mut seen = BTreeSet::new();
            let mut projection = [[0.0; 6]; 6];
            let mut rhs = [0.0; 6];
            let mut force = [0.0; 3];
            let mut moment = [0.0; 3];
            for sample in samples {
                let identity = (
                    sample.element_index,
                    sample.point_mm.map(f64::to_bits),
                    sample.area_mm2.to_bits(),
                );
                let ordinal = exact.get(&identity).copied().or_else(|| {
                    expected
                        .iter()
                        .enumerate()
                        .filter(|(_, candidate)| {
                            candidate.element_index == sample.element_index
                                && (0..3).all(|axis| {
                                    close(candidate.point_mm[axis] - sample.point_mm[axis], 0.0)
                                })
                                && close(candidate.area_mm2, sample.area_mm2)
                        })
                        .min_by(|(_, first), (_, second)| {
                            let distance = |candidate: &attachment::AttachmentSample| {
                                (0..3)
                                    .map(|axis| {
                                        (candidate.point_mm[axis] - sample.point_mm[axis]).powi(2)
                                    })
                                    .sum::<f64>()
                            };
                            distance(first).total_cmp(&distance(second)).then_with(|| {
                                (first.area_mm2 - sample.area_mm2)
                                    .abs()
                                    .total_cmp(&(second.area_mm2 - sample.area_mm2).abs())
                            })
                        })
                        .map(|(ordinal, _)| ordinal)
                });
                let Some(ordinal) = ordinal else {
                    return Err(fail(
                        "SPLA fastener sample does not belong to its native washer/bore/engagement footprint",
                    ));
                };
                if !seen.insert(ordinal) {
                    return Err(fail("SPLA repeats canonical fastener contact quadrature"));
                }
                let candidate = &expected[ordinal];
                let basis: [[f64; 3]; 6] = std::array::from_fn(|motion| {
                    let mut multiplier = [0.0; 6];
                    multiplier[motion] = 1.0;
                    coupling.traction(multiplier, sample.point_mm)
                });
                for row in 0..6 {
                    rhs[row] += sample.area_mm2 * chart::dot(basis[row], sample.traction_mpa);
                    for column in 0..6 {
                        projection[row][column] +=
                            sample.area_mm2 * chart::dot(basis[row], basis[column]);
                    }
                }
                let nodal = sample.traction_mpa.map(|value| value * sample.area_mm2);
                for node in 0..27 {
                    for axis in 0..3 {
                        connector_forces[candidate.nodes[node] as usize][axis] +=
                            candidate.values[node] * nodal[axis];
                    }
                }
                let arm = chart::cross(sample.point_mm, nodal);
                for axis in 0..3 {
                    force[axis] += nodal[axis];
                    moment[axis] += arm[axis];
                }
            }
            let multipliers = solve_six(projection, rhs)?;
            let motions = coupling.motions(&displacements);
            let constrained = fastener.constrained_motions();
            let maximum_error = (0..6)
                .filter(|motion| constrained[*motion])
                .map(|motion| motions[motion].abs())
                .fold(0.0, f64::max);
            if maximum_error > 1e-7
                || (0..6).any(|motion| {
                    !constrained[motion]
                        && multipliers[motion].abs() > 1e-7 * (1.0 + magnitude(force))
                })
            {
                return Err(fail(
                    "SPLA ideal connector restricts the wrong motions or violates its constraints",
                ));
            }
            for sample in samples {
                let traction = coupling.traction(multipliers, sample.point_mm);
                if (0..3).any(|axis| !close(traction[axis], sample.traction_mpa[axis])) {
                    return Err(fail(
                        "SPLA fastener traction is not the declared distributing connector law",
                    ));
                }
            }
            let response = &self.summary.fastener_responses[index];
            if !close(response.area_mm2, coupling.area_mm2)
                || !close(response.maximum_constraint_error_mm, maximum_error)
                || (0..3).any(|axis| {
                    !close(force[axis], response.reaction_force_n[axis])
                        || !close(moment[axis], response.reaction_moment_n_mm[axis])
                })
            {
                return Err(fail(
                    "SPLA fastener reaction or constraint assessment differs from retained quadrature",
                ));
            }
        }
        if profile.is_some() {
            let mut fixed = vec![[false; 3]; nodes.len()];
            for restraint in &self.summary.case.case.restraints {
                let mut found = false;
                for &(element, axis, side, key) in &exterior {
                    if exterior_sources.get(&key).and_then(Option::as_ref)
                        == Some(&restraint.face_id)
                    {
                        found = true;
                        for local in chart::facet_nodes(axis, side) {
                            for (held, requested) in fixed[elements[element].nodes[local] as usize]
                                .iter_mut()
                                .zip(restraint.fixed_translation)
                            {
                                *held |= requested;
                            }
                        }
                    }
                }
                if !found {
                    return Err(fail("SPLA restraint has no complete native boundary owner"));
                }
            }
            let mut residual = 0.0;
            let mut rhs_norm = 0.0;
            let mut reaction = [0.0; 3];
            let mut reaction_moment = [0.0; 3];
            for (index, node) in nodes.iter().enumerate() {
                let mut nodal_reaction = connector_forces[index];
                for axis in 0..3 {
                    let remaining = internal[index][axis]
                        - node.applied_force_n[axis]
                        - connector_forces[index][axis];
                    if fixed[index][axis] {
                        if node.displacement_mm[axis].abs() > 1e-7 {
                            return Err(fail("SPLA fixed support is displaced"));
                        }
                        nodal_reaction[axis] += remaining;
                    } else {
                        residual += remaining * remaining;
                        rhs_norm += node.applied_force_n[axis].powi(2);
                    }
                }
                let moment = chart::cross(node.position_mm, nodal_reaction);
                for axis in 0..3 {
                    reaction[axis] += nodal_reaction[axis];
                    reaction_moment[axis] += moment[axis];
                }
            }
            let residual = residual.sqrt();
            let force_error = magnitude(std::array::from_fn(|axis| {
                reaction[axis] + requested_force[axis]
            }));
            let moment_error = magnitude(std::array::from_fn(|axis| {
                reaction_moment[axis] + requested_moment[axis]
            }));
            if residual > 1e-8 + 1e-8 * rhs_norm.sqrt()
                || force_error > 1e-6 * (1.0 + magnitude(requested_force))
                || moment_error > 1e-6 * (1.0 + magnitude(requested_moment))
                || !close(residual, self.summary.assessment.free_residual_n)
                || !close(force_error, self.summary.assessment.force_imbalance_n)
                || !close(moment_error, self.summary.assessment.moment_imbalance_n_mm)
            {
                return Err(fail(
                    "SPLA original mechanical operator, reactions or assessment do not agree",
                ));
            }
        }
        Ok(())
    }
}
