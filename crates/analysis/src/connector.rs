// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Exact distributing constraints. Augmentation only makes the factor invertible;
//! the Schur projection removes its contribution from the final equilibrium.
use crate::{Budget, mesh::Mesh};
use spiling_contracts::analysis::{
    AnalysisCase,
    attachment::{self, AttachmentFacet, AttachmentSample, Coupling},
    fastener::*,
};

pub(crate) struct Connection {
    pub coupling: Coupling,
    pub samples: Vec<AttachmentSample>,
    pub constrained: [bool; 6],
}
pub(crate) struct Row {
    pub fastener: usize,
    pub motion: usize,
    pub values: Vec<(usize, f64)>,
}
pub(crate) struct Connectors {
    pub connections: Vec<Connection>,
    pub rows: Vec<Row>,
    /// Columns A^-1 C^T in the free ordering, not physical flexibility.
    pub inverse_columns: Vec<Vec<f64>>,
    pub schur: Vec<Vec<f64>>,
    pub alpha: f64,
}
impl Connectors {
    pub fn prepare(mesh: &Mesh, case: &AnalysisCase, budget: &Budget) -> Result<Self, String> {
        let mut result = Self {
            connections: Vec::new(),
            rows: Vec::new(),
            inverse_columns: Vec::new(),
            schur: Vec::new(),
            alpha: 0.0,
        };
        if case.fasteners.is_empty() {
            return Ok(result);
        }
        let AnalysisMeshGeometry::RationalBoredPlate { profile } = &mesh.geometry else {
            return Err("UnsupportedModel: distributing fasteners require an admitted native circular bored plate".into());
        };
        let facets = mesh
            .facets
            .iter()
            .map(|facet| {
                let hex = &mesh.elements[facet.element];
                AttachmentFacet {
                    element_index: facet.element as u32,
                    axis: facet.axis,
                    side: facet.side,
                    nodes: hex.nodes.map(|node| node as u32),
                    positions: hex.nodes.map(|node| mesh.nodes[node]),
                    midpoint_weight: hex.midpoint_weight,
                }
            })
            .collect::<Vec<_>>();
        for (index, fastener) in case.fasteners.iter().enumerate() {
            budget.check()?;
            if fastener.hole != profile.hole {
                return Err(
                    "StaleOperator: fastener does not match the complete admitted native hole"
                        .into(),
                );
            }
            if let FastenerAttachment::Clamp {
                washer_diameter_mm, ..
            } = fastener.attachment
            {
                let radius = washer_diameter_mm * 0.5;
                if (0..2).any(|axis| {
                    fastener.hole.center_mm[axis] - radius < profile.outer_xy_mm[axis][0]
                        || fastener.hole.center_mm[axis] + radius > profile.outer_xy_mm[axis][1]
                }) {
                    return Err("UnsupportedModel: complete washer annulus must fit the native plate; reduce washer diameter".into());
                }
            }
            // The mesh builder already certifies every whole cell. Recheck the
            // contacted cells here rather than deriving authority from samples.
            let samples = attachment::samples(fastener, &facets).map_err(|e| e.to_string())?;
            let mut last = None;
            for sample in &samples {
                if last != Some(sample.element_index) {
                    let hex = &mesh.elements[sample.element_index as usize];
                    crate::mapped::validate_extruded_cell(
                        &hex.nodes.map(|node| mesh.nodes[node]),
                        hex.midpoint_weight,
                    )?;
                    last = Some(sample.element_index);
                }
            }
            let coupling = attachment::coupling(fastener, &samples, mesh.nodes.len())
                .map_err(|e| e.to_string())?;
            let constrained = fastener.constrained_motions();
            for (motion, &fixed) in constrained.iter().enumerate() {
                if fixed {
                    result.rows.push(Row {
                        fastener: index,
                        motion,
                        values: coupling.rows[motion].clone(),
                    });
                }
            }
            result.connections.push(Connection {
                coupling,
                samples,
                constrained,
            });
        }
        Ok(result)
    }
    pub fn forces(&self, multipliers: &[f64], node_count: usize) -> Vec<[f64; 3]> {
        let mut forces = vec![[0.0; 3]; node_count];
        for (row, &multiplier) in self.rows.iter().zip(multipliers) {
            for &(dof, value) in &row.values {
                forces[dof / 3][dof % 3] -= multiplier * value;
            }
        }
        forces
    }
    pub fn multipliers(&self, fastener: usize, values: &[f64]) -> [f64; 6] {
        let mut result = [0.0; 6];
        for (row, &value) in self.rows.iter().zip(values) {
            if row.fastener == fastener {
                result[row.motion] = -value;
            }
        }
        result
    }
    pub fn constraint_values(&self, displacement: &[[f64; 3]]) -> Vec<f64> {
        self.rows
            .iter()
            .map(|row| {
                row.values
                    .iter()
                    .map(|&(dof, c)| c * displacement[dof / 3][dof % 3])
                    .sum()
            })
            .collect()
    }
}

/// Small partial-pivot elimination with no regularization or row deletion.
/// All motions use the same displacement units. Dependent constraints refuse.
pub(crate) fn dense_solve(matrix: &[Vec<f64>], rhs: &[f64]) -> Result<Vec<f64>, String> {
    let n = rhs.len();
    let mut matrix = matrix.to_vec();
    let mut rhs = rhs.to_vec();
    let scale = matrix.iter().flatten().map(|v| v.abs()).fold(0.0, f64::max);
    for column in 0..n {
        let pivot = (column..n)
            .max_by(|&a, &b| matrix[a][column].abs().total_cmp(&matrix[b][column].abs()))
            .unwrap();
        if !matrix[pivot][column].is_finite() || matrix[pivot][column].abs() <= scale * 1e-12 {
            return Err("DisconnectedOrMechanism: distributing constraints are dependent or unresolved with the selected face restraints; choose independent supports".into());
        }
        matrix.swap(column, pivot);
        rhs.swap(column, pivot);
        let pivot_row = matrix[column].clone();
        let pivot_rhs = rhs[column];
        for row in column + 1..n {
            let ratio = matrix[row][column] / pivot_row[column];
            for j in column + 1..n {
                matrix[row][j] -= ratio * pivot_row[j];
            }
            rhs[row] -= ratio * pivot_rhs;
        }
    }
    let mut result = vec![0.0; n];
    for row in (0..n).rev() {
        result[row] = (rhs[row]
            - (row + 1..n)
                .map(|j| matrix[row][j] * result[j])
                .sum::<f64>())
            / matrix[row][row];
    }
    if result.iter().any(|v| !v.is_finite()) {
        return Err("NonConverged: nonfinite exact constraint multipliers".into());
    }
    Ok(result)
}
