// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::{
    Budget,
    element::{self, Tensor},
    mesh::{self, Mesh},
    native::NativePrism,
};
use dyn_stack::{MemBuffer, MemStack};
use faer::{
    Conj, Mat, Par, Side,
    sparse::{
        SparseColMat, Triplet,
        linalg::cholesky::{self, LltRef, SymbolicCholesky},
    },
};
use serde::{Deserialize, Serialize};
use spiling_contracts::{
    analysis::{AnalysisCase, mechanics::*},
    geometry::RigidPoseMm,
};
use std::{collections::BTreeMap, time::Instant};

pub struct PreparedOperator {
    settings: AnalysisRunSettings,
    pub(crate) subject: (
        spiling_contracts::geometry::DefinitionId,
        spiling_contracts::geometry::SourceHash,
        Vec<spiling_contracts::analysis::AnalysisFaceRestraint>,
    ),
    pub mesh: Mesh,
    pub tensors: [Tensor; 3],
    pub fixed: Vec<bool>,
    free: Vec<usize>,
    symbolic: SymbolicCholesky<usize>,
    factors: Vec<f64>,
    pub ranks: Vec<u32>,
    pub construction_ms: f64,
    pub setup_ms: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PatchSample {
    pub patch_index: u32,
    pub point_mm: [f64; 3],
    pub area_mm2: f64,
    pub traction_mpa: [f64; 3],
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ElementField {
    pub center_mm: [f64; 3],
    pub role: u32,
    pub volume_mm3: f64,
    pub stress_mpa: [f64; 6],
    pub energy_n_mm: f64,
    pub concentration: f64,
}
pub struct Solution {
    pub applied_force_n: Vec<[f64; 3]>,
    pub displacement_mm: Vec<[f64; 3]>,
    pub fields: Vec<ElementField>,
    pub patches: Vec<PatchSample>,
    pub assessment: AnalysisNumericalAssessment,
    pub energy: [f64; 3],
    pub volumes: [f64; 3],
    pub rhs_ms: f64,
    pub solve_ms: f64,
    pub assessment_ms: f64,
    pub fields_ms: f64,
}
pub fn norm(values: impl IntoIterator<Item = f64>) -> f64 {
    values
        .into_iter()
        .map(|value| value * value)
        .sum::<f64>()
        .sqrt()
}
fn cross(first: [f64; 3], second: [f64; 3]) -> [f64; 3] {
    [
        first[1] * second[2] - first[2] * second[1],
        first[2] * second[0] - first[0] * second[2],
        first[0] * second[1] - first[1] * second[0],
    ]
}
pub fn elapsed(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}

fn rank(mut matrix: [[f64; 6]; 6]) -> u32 {
    let mut count = 0;
    let scale = matrix
        .iter()
        .flatten()
        .copied()
        .map(f64::abs)
        .fold(0.0, f64::max);
    for column in 0..6 {
        let pivot = (count..6)
            .max_by(|first, second| {
                matrix[*first][column]
                    .abs()
                    .total_cmp(&matrix[*second][column].abs())
            })
            .unwrap_or(count);
        if matrix[pivot][column].abs() <= scale * 1e-10 {
            continue;
        }
        matrix.swap(count, pivot);
        let pivot_row = matrix[count];
        for row in count + 1..6 {
            let multiplier = matrix[row][column] / matrix[count][column];
            for (value, pivot) in matrix[row][column..].iter_mut().zip(&pivot_row[column..]) {
                *value -= multiplier * pivot;
            }
        }
        count += 1;
        if count == 6 {
            break;
        }
    }
    count as u32
}
fn facet_nodes(axis: usize, side: usize) -> impl Iterator<Item = usize> {
    (0..27).filter(move |index| [index % 3, (index / 3) % 3, index / 9][axis] == 2 * side)
}
pub fn prepare(
    prism: &NativePrism,
    settings: &AnalysisRunSettings,
    pose: RigidPoseMm,
    case: &AnalysisCase,
    budget: &Budget,
) -> Result<PreparedOperator, String> {
    case.validate().map_err(|error| error.to_string())?;
    if case.mode != spiling_contracts::analysis::AnalysisMode::Basic
        || case.loads.is_empty()
        || case.restraints.is_empty()
    {
        return Err("UnsupportedModel: this assumed-law macro consumer requires a complete Basic case; Enhanced does not silently fall back".into());
    }
    settings.validate().map_err(|error| error.to_string())?;
    let started = Instant::now();
    let mesh = mesh::build(prism, settings, pose, budget)?;
    let tensors = [
        settings.laws.wall_mpa,
        settings.laws.skin_mpa,
        settings.laws.effective_core_mpa,
    ]
    .map(|tensor| element::rotate(&tensor, mesh.service_to_print));
    let mut fixed = vec![false; 3 * mesh.nodes.len()];
    for restraint in &case.restraints {
        let mut found = false;
        for facet in &mesh.facets {
            if facet.face.as_ref() == Some(&restraint.face_id) {
                found = true;
                for local in facet_nodes(facet.axis, facet.side) {
                    let node = mesh.elements[facet.element].nodes[local];
                    for axis in 0..3 {
                        if restraint.fixed_translation[axis] {
                            fixed[3 * node + axis] = true;
                        }
                    }
                }
            }
        }
        if !found {
            return Err(
                "UnmappedLoad: restraint is not a complete admitted native side-face patch".into(),
            );
        }
    }
    let mut bounds = vec![[[f64::INFINITY, f64::NEG_INFINITY]; 3]; mesh.components];
    for hex in &mesh.elements {
        for &node in &hex.nodes {
            for axis in 0..3 {
                bounds[hex.component][axis][0] =
                    bounds[hex.component][axis][0].min(mesh.nodes[node][axis]);
                bounds[hex.component][axis][1] =
                    bounds[hex.component][axis][1].max(mesh.nodes[node][axis]);
            }
        }
    }
    let centers = bounds
        .iter()
        .map(|bounds| bounds.map(|[minimum, maximum]| minimum + 0.5 * (maximum - minimum)))
        .collect::<Vec<_>>();
    let scales = bounds
        .iter()
        .map(|bounds| {
            bounds
                .iter()
                .map(|[minimum, maximum]| maximum - minimum)
                .fold(0.0, f64::max)
        })
        .collect::<Vec<_>>();
    let mut grams = vec![[[0.0; 6]; 6]; mesh.components];
    let mut visited = vec![false; mesh.nodes.len()];
    for hex in &mesh.elements {
        for &node in &hex.nodes {
            if !visited[node] {
                visited[node] = true;
                for axis in 0..3 {
                    if fixed[3 * node + axis] {
                        let mut row = [0.0; 6];
                        row[axis] = 1.0;
                        for rotation in 0..3 {
                            let mut unit = [0.0; 3];
                            unit[rotation] = 1.0;
                            let centered = std::array::from_fn(|coordinate| {
                                mesh.nodes[node][coordinate] - centers[hex.component][coordinate]
                            });
                            row[3 + rotation] = cross(unit, centered)[axis] / scales[hex.component];
                        }
                        for first in 0..6 {
                            for second in 0..6 {
                                grams[hex.component][first][second] += row[first] * row[second];
                            }
                        }
                    }
                }
            }
        }
    }
    let ranks = grams.into_iter().map(rank).collect::<Vec<_>>();
    if ranks.iter().any(|value| *value != 6) {
        return Err(format!(
            "DisconnectedOrMechanism: rigid restraint ranks {ranks:?}, six required per component; no artificial pins"
        ));
    }
    let construction_ms = elapsed(started);
    let started = Instant::now();
    let free = (0..fixed.len())
        .filter(|dof| !fixed[*dof])
        .collect::<Vec<_>>();
    if free.is_empty() {
        return Err("DisconnectedOrMechanism: no free mechanical degrees of freedom".into());
    }
    if free
        .len()
        .checked_mul(free.len())
        .and_then(|count| count.checked_mul(16))
        .is_none_or(|bytes| bytes > 2 * 1024 * 1024 * 1024)
    {
        return Err(
            "ResourceLimit: conservative symbolic-factor storage admission exceeds 2 GiB".into(),
        );
    }
    let mut mapped = vec![usize::MAX; fixed.len()];
    for (index, &dof) in free.iter().enumerate() {
        mapped[dof] = index;
    }
    let mut kernels = BTreeMap::new();
    let mut entries = Vec::new();
    entries
        .try_reserve_exact(mesh.elements.len() * 3321)
        .map_err(|_| "ResourceLimit: sparse assembly reservation")?;
    for hex in &mesh.elements {
        budget.check()?;
        let key = (hex.size.map(f64::to_bits), hex.role);
        let matrix = kernels
            .entry(key)
            .or_insert_with(|| element::stiffness(hex.size, &tensors[hex.role]));
        for local_row in 0..81 {
            let row = mapped[3 * hex.nodes[local_row / 3] + local_row % 3];
            if row == usize::MAX {
                continue;
            }
            for local_column in 0..=local_row {
                let column = mapped[3 * hex.nodes[local_column / 3] + local_column % 3];
                if column == usize::MAX {
                    continue;
                }
                entries.push(Triplet::new(
                    row.max(column),
                    row.min(column),
                    matrix[81 * local_row + local_column],
                ));
            }
        }
    }
    let matrix =
        SparseColMat::<usize, f64>::try_new_from_triplets(free.len(), free.len(), &entries)
            .map_err(|error| format!("ResourceLimit: sparse assembly {error:?}"))?;
    drop(entries);
    drop(kernels);
    budget.check()?;
    let symbolic = cholesky::factorize_symbolic_cholesky(
        matrix.symbolic(),
        Side::Lower,
        Default::default(),
        Default::default(),
    )
    .map_err(|error| format!("ResourceLimit: symbolic factor {error:?}"))?;
    let scratch_req = symbolic.factorize_numeric_llt_scratch::<f64>(Par::Seq, Default::default());
    if symbolic
        .len_val()
        .saturating_mul(8)
        .saturating_add(scratch_req.size_bytes())
        > 512 * 1024 * 1024
    {
        return Err("ResourceLimit: factor and numerical scratch exceed 512 MiB".into());
    }
    let mut factors = vec![0.0; symbolic.len_val()];
    let mut scratch =
        MemBuffer::try_new(scratch_req).map_err(|_| "ResourceLimit: numeric factor scratch")?;
    let regularization = faer::linalg::cholesky::llt::factor::LltRegularization {
        dynamic_regularization_delta: 0.0,
        dynamic_regularization_epsilon: 0.0,
    };
    symbolic.factorize_numeric_llt(&mut factors,matrix.as_ref(),Side::Lower,regularization,Par::Seq,MemStack::new(&mut scratch),Default::default()).map_err(|error|format!("DisconnectedOrMechanism: original stiffness factorization refused, no regularization: {error:?}"))?;
    budget.check()?;
    Ok(PreparedOperator {
        settings: settings.clone(),
        subject: (
            case.definition_id.clone(),
            case.source_hash.clone(),
            case.restraints.clone(),
        ),
        mesh,
        tensors,
        fixed,
        free,
        symbolic,
        factors,
        ranks,
        construction_ms,
        setup_ms: elapsed(started),
    })
}

fn surface_quadrature(
    mesh: &Mesh,
    facet: &mesh::Facet,
    mut visit: impl FnMut([f64; 3], f64, [f64; 27]),
) {
    let hex = &mesh.elements[facet.element];
    let tangent = (0..3)
        .filter(|axis| *axis != facet.axis)
        .collect::<Vec<_>>();
    for (first, first_weight) in element::GAUSS {
        for (second, second_weight) in element::GAUSS {
            let mut local = [0.0; 3];
            local[facet.axis] = 2.0 * facet.side as f64 - 1.0;
            local[tangent[0]] = first;
            local[tangent[1]] = second;
            let point = std::array::from_fn(|axis| {
                hex.lower[axis] + 0.5 * (local[axis] + 1.0) * hex.size[axis]
            });
            let weight =
                hex.size[tangent[0]] * hex.size[tangent[1]] * first_weight * second_weight / 4.0;
            visit(point, weight, element::basis(local, hex.size).0);
        }
    }
}
fn solve_three(mut matrix: [[f64; 3]; 3], mut rhs: [f64; 3]) -> Result<[f64; 3], String> {
    for column in 0..3 {
        let pivot = (column..3)
            .max_by(|first, second| {
                matrix[*first][column]
                    .abs()
                    .total_cmp(&matrix[*second][column].abs())
            })
            .unwrap();
        matrix.swap(pivot, column);
        rhs.swap(pivot, column);
        if matrix[column][column].abs() < 1e-14 {
            return Err("UnmappedLoad: finite patch rotational transfer is rank deficient".into());
        }
        let pivot_row = matrix[column];
        for row in column + 1..3 {
            let scale = matrix[row][column] / matrix[column][column];
            for (value, pivot) in matrix[row][column..].iter_mut().zip(&pivot_row[column..]) {
                *value -= scale * pivot;
            }
            rhs[row] -= scale * rhs[column];
        }
    }
    let mut solution = [0.0; 3];
    for row in (0..3).rev() {
        solution[row] = (rhs[row]
            - (row + 1..3)
                .map(|column| matrix[row][column] * solution[column])
                .sum::<f64>())
            / matrix[row][row];
    }
    Ok(solution)
}

pub fn solve(
    operator: &PreparedOperator,
    case: &AnalysisCase,
    settings: &AnalysisRunSettings,
    budget: &Budget,
) -> Result<Solution, String> {
    case.validate().map_err(|error| error.to_string())?;
    if case.mode != spiling_contracts::analysis::AnalysisMode::Basic || case.loads.is_empty() {
        return Err("UnsupportedModel: macro solve requires nonempty Basic loads".into());
    }
    if settings != &operator.settings
        || (&case.definition_id, &case.source_hash, &case.restraints)
            != (
                &operator.subject.0,
                &operator.subject.1,
                &operator.subject.2,
            )
    {
        return Err("StaleOperator: construction, laws, source or restraints differ from the original operator".into());
    }
    let started = Instant::now();
    let mesh = &operator.mesh;
    let mut loads = vec![[0.0; 3]; mesh.nodes.len()];
    let mut patches = Vec::new();
    let mut requested_force = [0.0; 3];
    let mut requested_moment = [0.0; 3];
    let mut per_patch_force_error = 0.0_f64;
    let mut per_patch_moment_error = 0.0_f64;
    for (patch_index, load) in case.loads.iter().enumerate() {
        budget.check()?;
        let mut contribution = vec![[0.0; 3]; mesh.nodes.len()];
        let first_sample = patches.len();
        let facets = mesh
            .facets
            .iter()
            .filter(|facet| facet.face.as_ref() == Some(&load.face_id))
            .collect::<Vec<_>>();
        if facets.is_empty() {
            return Err(
                "UnmappedLoad: load is not a complete admitted native side-face patch".into(),
            );
        }
        let mut area = 0.0;
        let mut center = [0.0; 3];
        for facet in &facets {
            surface_quadrature(mesh, facet, |point, weight, _| {
                area += weight;
                for axis in 0..3 {
                    center[axis] += weight * point[axis];
                }
            });
        }
        for value in &mut center {
            *value /= area;
        }
        let mut inertia = [[0.0; 3]; 3];
        for facet in &facets {
            surface_quadrature(mesh, facet, |point, weight, _| {
                let offset: [f64; 3] = std::array::from_fn(|axis| point[axis] - center[axis]);
                let squared = offset.iter().map(|value| value * value).sum::<f64>();
                for row in 0..3 {
                    for column in 0..3 {
                        inertia[row][column] += weight
                            * (if row == column { squared } else { 0.0 }
                                - offset[row] * offset[column]);
                    }
                }
            });
        }
        let arm = cross(center, load.force_n);
        let couple = std::array::from_fn(|axis| load.moment_n_mm[axis] - arm[axis]);
        let rotation = solve_three(inertia, couple)?;
        for facet in &facets {
            surface_quadrature(mesh, facet, |point, weight, shape| {
                let offset = std::array::from_fn(|axis| point[axis] - center[axis]);
                let rotational = cross(rotation, offset);
                let traction =
                    std::array::from_fn(|axis| load.force_n[axis] / area + rotational[axis]);
                for (local, &value) in shape.iter().enumerate() {
                    let node = mesh.elements[facet.element].nodes[local];
                    for axis in 0..3 {
                        loads[node][axis] += weight * value * traction[axis];
                        contribution[node][axis] += weight * value * traction[axis];
                    }
                }
                patches.push(PatchSample {
                    patch_index: patch_index as u32,
                    point_mm: point,
                    area_mm2: weight,
                    traction_mpa: traction,
                });
            });
        }
        let nodal_force: [f64; 3] =
            std::array::from_fn(|axis| contribution.iter().map(|force| force[axis]).sum());
        let nodal_moment: [f64; 3] = std::array::from_fn(|axis| {
            mesh.nodes
                .iter()
                .zip(&contribution)
                .map(|(&point, &force)| cross(point, force)[axis])
                .sum()
        });
        let samples = &patches[first_sample..];
        let quadrature_force: [f64; 3] = std::array::from_fn(|axis| {
            samples
                .iter()
                .map(|sample| sample.area_mm2 * sample.traction_mpa[axis])
                .sum()
        });
        let quadrature_moment: [f64; 3] = std::array::from_fn(|axis| {
            samples
                .iter()
                .map(|sample| sample.area_mm2 * cross(sample.point_mm, sample.traction_mpa)[axis])
                .sum()
        });
        let force_error = norm((0..3).map(|axis| nodal_force[axis] - load.force_n[axis])).max(
            norm((0..3).map(|axis| quadrature_force[axis] - load.force_n[axis])),
        );
        let moment_error = norm((0..3).map(|axis| nodal_moment[axis] - load.moment_n_mm[axis]))
            .max(norm((0..3).map(|axis| {
                quadrature_moment[axis] - load.moment_n_mm[axis]
            })));
        if force_error > 1e-8 * (1.0 + norm(load.force_n))
            || moment_error > 1e-8 * (1.0 + norm(load.moment_n_mm))
        {
            return Err(format!(
                "UnmappedLoad: patch {patch_index} independent force/moment errors {force_error} N, {moment_error} N mm"
            ));
        }
        per_patch_force_error = per_patch_force_error.max(force_error);
        per_patch_moment_error = per_patch_moment_error.max(moment_error);
        for axis in 0..3 {
            requested_force[axis] += load.force_n[axis];
            requested_moment[axis] += load.moment_n_mm[axis];
        }
    }
    if case.loads.is_empty() {
        return Err("MissingInput: no applied finite-patch service wrench".into());
    }
    let nodal_force =
        std::array::from_fn::<_, 3, _>(|axis| loads.iter().map(|force| force[axis]).sum::<f64>());
    let nodal_moment = std::array::from_fn::<_, 3, _>(|axis| {
        mesh.nodes
            .iter()
            .zip(&loads)
            .map(|(&point, &force)| cross(point, force)[axis])
            .sum::<f64>()
    });
    let quadrature_force = std::array::from_fn::<_, 3, _>(|axis| {
        patches
            .iter()
            .map(|sample| sample.area_mm2 * sample.traction_mpa[axis])
            .sum::<f64>()
    });
    let quadrature_moment = std::array::from_fn::<_, 3, _>(|axis| {
        patches
            .iter()
            .map(|sample| sample.area_mm2 * cross(sample.point_mm, sample.traction_mpa)[axis])
            .sum::<f64>()
    });
    let patch_force_error_n = norm((0..3).map(|axis| nodal_force[axis] - requested_force[axis]))
        .max(per_patch_force_error)
        .max(norm(
            (0..3).map(|axis| quadrature_force[axis] - requested_force[axis]),
        ));
    let patch_moment_error_n_mm =
        norm((0..3).map(|axis| nodal_moment[axis] - requested_moment[axis]))
            .max(per_patch_moment_error)
            .max(norm((0..3).map(|axis| {
                quadrature_moment[axis] - requested_moment[axis]
            })));
    if patch_force_error_n > 1e-8 * (1.0 + norm(requested_force))
        || patch_moment_error_n_mm > 1e-8 * (1.0 + norm(requested_moment))
    {
        return Err(format!(
            "UnmappedLoad: independent nodal/quadrature wrench errors {patch_force_error_n} N, {patch_moment_error_n_mm} N mm"
        ));
    }
    let rhs_ms = elapsed(started);
    let started = Instant::now();
    budget.check()?;
    let mut rhs = Mat::from_fn(operator.free.len(), 1, |index, _| {
        loads[operator.free[index] / 3][operator.free[index] % 3]
    });
    let mut scratch =
        MemBuffer::try_new(operator.symbolic.solve_in_place_scratch::<f64>(1, Par::Seq))
            .map_err(|_| "ResourceLimit: solve scratch")?;
    LltRef::new(&operator.symbolic, &operator.factors).solve_in_place_with_conj(
        Conj::No,
        rhs.as_mut(),
        Par::Seq,
        MemStack::new(&mut scratch),
    );
    let mut displacement_mm = vec![[0.0; 3]; mesh.nodes.len()];
    for (index, &dof) in operator.free.iter().enumerate() {
        displacement_mm[dof / 3][dof % 3] = rhs[(index, 0)];
    }
    let solve_ms = elapsed(started);
    let started = Instant::now();
    budget.check()?;
    let mut internal = vec![[0.0; 3]; mesh.nodes.len()];
    let mut energy = [0.0; 3];
    let mut volumes = [0.0; 3];
    let mut fields = Vec::new();
    let mut samples = Vec::new();
    for hex in &mesh.elements {
        budget.check()?;
        let local = hex.nodes.map(|node| displacement_mm[node]);
        let volume = hex.size.iter().product::<f64>();
        let mut element_energy = 0.0;
        element::quadrature(|point, weight| {
            let (_, gradients) = element::basis(point, hex.size);
            let (strain, stress) = element::state(&gradients, &local, &operator.tensors[hex.role]);
            let density = 0.5
                * (0..6)
                    .map(|index| strain[index] * stress[index])
                    .sum::<f64>();
            element_energy += density * volume * weight;
            for local in 0..27 {
                let force = element::force(gradients[local], stress);
                for axis in 0..3 {
                    internal[hex.nodes[local]][axis] += force[axis] * volume * weight;
                }
            }
            let position: [f64; 3] = std::array::from_fn(|axis| {
                hex.lower[axis] + 0.5 * (point[axis] + 1.0) * hex.size[axis]
            });
            let fraction = if hex.role == 2 {
                settings.recipe.infill_fraction
            } else {
                1.0
            };
            samples.push((position, volume * weight * fraction, density / fraction));
        });
        energy[hex.role] += element_energy;
        volumes[hex.role] += volume;
        let (_, gradients) = element::basis([0.0; 3], hex.size);
        let (_, stress) = element::state(&gradients, &local, &operator.tensors[hex.role]);
        fields.push(ElementField {
            center_mm: std::array::from_fn(|axis| hex.lower[axis] + 0.5 * hex.size[axis]),
            role: hex.role as u32,
            volume_mm3: volume,
            stress_mpa: stress,
            energy_n_mm: element_energy,
            concentration: 0.0,
        });
    }
    let free_residual_n = norm(
        operator
            .free
            .iter()
            .map(|dof| internal[dof / 3][dof % 3] - loads[dof / 3][dof % 3]),
    );
    let rhs_norm_n = norm(operator.free.iter().map(|dof| loads[dof / 3][dof % 3]));
    let reaction_force = std::array::from_fn::<_, 3, _>(|axis| {
        (0..mesh.nodes.len())
            .filter(|node| operator.fixed[3 * node + axis])
            .map(|node| internal[node][axis] - loads[node][axis])
            .sum::<f64>()
    });
    let reaction_moment = std::array::from_fn::<_, 3, _>(|axis| {
        (0..mesh.nodes.len())
            .map(|node| {
                let reaction = std::array::from_fn(|component| {
                    if operator.fixed[3 * node + component] {
                        internal[node][component] - loads[node][component]
                    } else {
                        0.0
                    }
                });
                cross(mesh.nodes[node], reaction)[axis]
            })
            .sum::<f64>()
    });
    let force_imbalance_n = norm((0..3).map(|axis| reaction_force[axis] + requested_force[axis]));
    let moment_imbalance_n_mm =
        norm((0..3).map(|axis| reaction_moment[axis] + requested_moment[axis]));
    let work = (0..mesh.nodes.len())
        .flat_map(|node| (0..3).map(move |axis| (node, axis)))
        .map(|(node, axis)| loads[node][axis] * displacement_mm[node][axis])
        .sum::<f64>();
    let total_energy = energy.iter().sum::<f64>();
    let work_energy_relative_error = (work - 2.0 * total_energy).abs() / work.abs().max(1e-30);
    if !total_energy.is_finite()
        || total_energy <= 0.0
        || displacement_mm
            .iter()
            .flatten()
            .any(|value| !value.is_finite())
        || free_residual_n > 1e-8 + 1e-8 * rhs_norm_n
        || force_imbalance_n > 1e-6 * (1.0 + norm(requested_force))
        || moment_imbalance_n_mm > 1e-6 * (1.0 + norm(requested_moment))
        || work_energy_relative_error > 1e-7
    {
        return Err(format!(
            "NonConverged: original FP64 assessment residual={free_residual_n}, force={force_imbalance_n}, moment={moment_imbalance_n_mm}, energy={work_energy_relative_error}"
        ));
    }
    let assessment = AnalysisNumericalAssessment {
        free_residual_n,
        rhs_norm_n,
        force_imbalance_n,
        moment_imbalance_n_mm,
        work_energy_relative_error,
        patch_force_error_n,
        patch_moment_error_n_mm,
        rigid_constraint_ranks: operator.ranks.clone(),
    };
    let assessment_ms = elapsed(started);
    let started = Instant::now();
    let radius = 3.0 * settings.recipe.bead_width_mm;
    let material_volume = volumes[0] + volumes[1] + volumes[2] * settings.recipe.infill_fraction;
    let global = total_energy / material_volume;
    for field in &mut fields {
        budget.check()?;
        let mut local_energy = 0.0;
        let mut local_volume = 0.0;
        for &(point, volume, density) in &samples {
            if (0..3)
                .map(|axis| (point[axis] - field.center_mm[axis]).powi(2))
                .sum::<f64>()
                <= radius * radius
            {
                local_volume += volume;
                local_energy += volume * density;
            }
        }
        if local_volume <= 0.0 {
            return Err(
                "UnresolvedField: concentration neighborhood has no quadrature coverage".into(),
            );
        }
        field.concentration = local_energy / local_volume / global;
    }
    Ok(Solution {
        applied_force_n: loads,
        displacement_mm,
        fields,
        patches,
        assessment,
        energy,
        volumes,
        rhs_ms,
        solve_ms,
        assessment_ms,
        fields_ms: elapsed(started),
    })
}
