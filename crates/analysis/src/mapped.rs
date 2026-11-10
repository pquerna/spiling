// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::element;
use spiling_contracts::analysis::chart;

pub(crate) fn stiffness(
    nodes: &[[f64; 3]; 27],
    midpoint_weight: f64,
    tensor: &element::Tensor,
) -> Result<Vec<f64>, String> {
    validate_extruded_cell(nodes, midpoint_weight)?;
    if tensor.iter().flatten().any(|value| !value.is_finite()) {
        return Err("InvalidMappedCell: nonfinite constitutive tensor".into());
    }
    let mut matrix = vec![0.0; 81 * 81];
    let mut error = None;
    chart::quadrature(midpoint_weight, |point, weight| {
        if error.is_some() {
            return;
        }
        let mapped = match chart::mapped(nodes, midpoint_weight, point) {
            Ok(mapped) => mapped,
            Err(reason) => {
                error = Some(format!("InvalidMappedCell: {reason}"));
                return;
            }
        };
        let mut kinematic = [[0.0; 81]; 6];
        for (node, gradient) in mapped.gradients.iter().enumerate() {
            for axis in 0..3 {
                kinematic[axis][3 * node + axis] = gradient[axis];
            }
            for (strain, (first, second)) in [(1, 2), (0, 2), (0, 1)].into_iter().enumerate() {
                kinematic[3 + strain][3 * node + first] = gradient[second];
                kinematic[3 + strain][3 * node + second] = gradient[first];
            }
        }
        let constitutive: [[f64; 81]; 6] = std::array::from_fn(|row| {
            std::array::from_fn(|dof| {
                (0..6)
                    .map(|column| tensor[row][column] * kinematic[column][dof])
                    .sum()
            })
        });
        for row in 0..81 {
            for column in 0..=row {
                matrix[81 * row + column] += weight
                    * mapped.determinant
                    * (0..6)
                        .map(|strain| kinematic[strain][row] * constitutive[strain][column])
                        .sum::<f64>();
            }
        }
    });
    if let Some(reason) = error {
        return Err(reason);
    }
    if matrix.iter().any(|value| !value.is_finite()) {
        return Err("UnresolvedJacobian: nonfinite mapped stiffness integral".into());
    }
    Ok(matrix)
}

pub(crate) use spiling_contracts::analysis::chart::validate_extruded_cell;

#[cfg(test)]
mod tests {
    use super::*;

    fn tensor() -> element::Tensor {
        let coupling = [3.0, 2.0, 4.0, 1.0, 2.0, 1.0];
        std::array::from_fn(|row| {
            std::array::from_fn(|column| {
                coupling[row] * coupling[column]
                    + if row == column {
                        [2100.0, 1800.0, 1200.0, 500.0, 600.0, 700.0][row]
                    } else {
                        0.0
                    }
            })
        })
    }

    fn unit() -> [[f64; 3]; 27] {
        std::array::from_fn(|node| {
            [
                (node % 3) as f64 * 0.5,
                ((node / 3) % 3) as f64 * 0.5,
                (node / 9) as f64 * 0.5,
            ]
        })
    }

    fn sector(half_arc: f64) -> ([[f64; 3]; 27], f64) {
        let mut nodes = [[0.0; 3]; 27];
        for angular in 0..3 {
            let angle = (angular as f64 - 1.0) * half_arc;
            for (radial, radius) in [(0, 2.0), (2, 1.0)] {
                nodes[angular + 3 * radial] = [radius * angle.cos(), radius * angle.sin(), 0.0];
            }
            for axis in [0, 1] {
                nodes[angular + 3][axis] = nodes[angular][axis].midpoint(nodes[angular + 6][axis]);
            }
        }
        for node in 9..27 {
            nodes[node] = [
                nodes[node % 9][0],
                nodes[node % 9][1],
                (node / 9) as f64 * 0.5,
            ];
        }
        (nodes, (1.0 + half_arc.cos()) * 0.5)
    }

    fn energy(matrix: &[f64], displacement: &[[f64; 3]; 27]) -> f64 {
        (0..81)
            .flat_map(|row| {
                (0..81).map(move |column| {
                    0.5 * displacement[row / 3][row % 3]
                        * displacement[column / 3][column % 3]
                        * matrix[81 * row.max(column) + row.min(column)]
                })
            })
            .sum()
    }

    #[test]
    fn affine_unit_cell_matches_existing_stiffness_and_volume() {
        let nodes = unit();
        validate_extruded_cell(&nodes, 1.0).unwrap();
        let expected = element::stiffness([1.0; 3], &tensor());
        let actual = stiffness(&nodes, 1.0, &tensor()).unwrap();
        let scale = expected.iter().copied().map(f64::abs).fold(0.0, f64::max);
        for row in 0..81 {
            for column in 0..81 {
                let index = 81 * row + column;
                assert!((actual[index] - expected[index]).abs() < 2e-13 * scale);
                if column > row {
                    assert_eq!(actual[index], 0.0);
                }
            }
        }
        assert!((chart::volume(&nodes, 1.0).unwrap() - 1.0).abs() < 1e-14);
    }

    #[test]
    fn rational_edges_remain_circular_with_analytic_sector_volume() {
        let half_arc = 0.3;
        let (nodes, weight) = sector(half_arc);
        validate_extruded_cell(&nodes, weight).unwrap();
        for radial in [-1.0, 1.0] {
            let radius = if radial == -1.0 { 2.0 } else { 1.0 };
            for angular in [-1.0, -0.83, -0.42, 0.0, 0.19, 0.67, 1.0] {
                for zeta in [-1.0, 0.0, 1.0] {
                    let point = chart::mapped(&nodes, weight, [angular, radial, zeta]).unwrap();
                    assert!((point.position[0].hypot(point.position[1]) - radius).abs() < 2e-14);
                    assert!((point.position[2] - (zeta + 1.0) * 0.5).abs() < 2e-14);
                    assert!(point.determinant > 0.0);
                }
            }
        }
        let expected_volume = half_arc * (4.0 - 1.0);
        assert!((chart::volume(&nodes, weight).unwrap() / expected_volume - 1.0).abs() < 1e-9);
        let (semicircle, minimum_weight) = sector(std::f64::consts::FRAC_PI_2);
        assert_eq!(minimum_weight, 0.5);
        validate_extruded_cell(&semicircle, minimum_weight).unwrap();
    }

    #[test]
    fn rational_cell_preserves_all_rigid_modes() {
        let (nodes, weight) = sector(0.6);
        let matrix = stiffness(&nodes, weight, &tensor()).unwrap();
        let scale = matrix.iter().copied().map(f64::abs).fold(0.0, f64::max);
        for mode in 0..6 {
            let displacement = nodes.map(|point| {
                if mode < 3 {
                    std::array::from_fn(|axis| if axis == mode { 1.0 } else { 0.0 })
                } else {
                    let rotation =
                        std::array::from_fn(|axis| if axis == mode - 3 { 1.0 } else { 0.0 });
                    chart::cross(rotation, point)
                }
            });
            chart::quadrature(weight, |local, _| {
                let point = chart::mapped(&nodes, weight, local).unwrap();
                let strain = element::state(&point.gradients, &displacement, &tensor()).0;
                assert!(strain.iter().all(|value| value.abs() < 2e-13));
            });
            for row in 0..81 {
                let residual: f64 = (0..81)
                    .map(|column| {
                        matrix[81 * row.max(column) + row.min(column)]
                            * displacement[column / 3][column % 3]
                    })
                    .sum();
                assert!(residual.abs() < 2e-12 * scale, "mode {mode}: {residual}");
            }
            assert!(energy(&matrix, &displacement).abs() < 2e-11 * scale);
        }
    }

    #[test]
    fn physical_affine_patch_reproduces_coupled_stress_and_energy() {
        let half_arc = 0.3;
        let (nodes, weight) = sector(half_arc);
        let tensor = tensor();
        let derivative = [
            [0.01, 0.02, -0.03],
            [0.04, -0.05, 0.06],
            [-0.07, 0.08, 0.09],
        ];
        let affine = |point: [f64; 3]| {
            std::array::from_fn(|axis| {
                [0.25, -0.5, 0.75][axis]
                    + (0..3)
                        .map(|direction| derivative[axis][direction] * point[direction])
                        .sum::<f64>()
            })
        };
        let displacement = nodes.map(affine);
        let expected_strain = [0.01, -0.05, 0.09, 0.14, -0.10, 0.06];
        let expected_stress: [f64; 6] = std::array::from_fn(|row| {
            (0..6)
                .map(|column| tensor[row][column] * expected_strain[column])
                .sum()
        });
        chart::quadrature(weight, |local, _| {
            let point = chart::mapped(&nodes, weight, local).unwrap();
            let (strain, stress) = element::state(&point.gradients, &displacement, &tensor);
            for axis in 0..6 {
                assert!((strain[axis] - expected_strain[axis]).abs() < 2e-13);
                assert!((stress[axis] - expected_stress[axis]).abs() < 5e-10);
            }
            for (axis, expected) in affine(point.position).into_iter().enumerate() {
                let actual: f64 = (0..27)
                    .map(|node| point.values[node] * displacement[node][axis])
                    .sum();
                assert!((actual - expected).abs() < 2e-14);
            }
        });
        let density: f64 = 0.5
            * (0..6)
                .map(|axis| expected_strain[axis] * expected_stress[axis])
                .sum::<f64>();
        let matrix = stiffness(&nodes, weight, &tensor).unwrap();
        let actual = energy(&matrix, &displacement);
        let integrated = density * chart::volume(&nodes, weight).unwrap();
        let analytic = density * half_arc * (4.0 - 1.0);
        assert!((actual / integrated - 1.0).abs() < 2e-11);
        assert!((actual / analytic - 1.0).abs() < 1e-9);
    }

    #[test]
    fn inverted_nonextruded_and_nonmidpoint_cells_refuse() {
        let nodes = unit();
        let inverted = nodes.map(|[x, y, z]| [1.0 - x, y, z]);
        assert!(validate_extruded_cell(&inverted, 1.0).is_err());
        assert!(stiffness(&inverted, 1.0, &tensor()).is_err());
        let mut distorted = nodes;
        distorted[18][0] += 0.01;
        assert!(
            validate_extruded_cell(&distorted, 1.0)
                .unwrap_err()
                .contains("prismatic")
        );
        let distorted = nodes.map(|[x, y, z]| [x, y, if z == 0.5 { 0.51 } else { z }]);
        assert!(
            validate_extruded_cell(&distorted, 1.0)
                .unwrap_err()
                .contains("Z planes")
        );
        let distorted = nodes.map(|[x, y, z]| [x, if y == 0.5 { 0.51 } else { y }, z]);
        assert!(
            validate_extruded_cell(&distorted, 1.0)
                .unwrap_err()
                .contains("radial-midpoint")
        );
        let inverted_z = nodes.map(|[x, y, z]| [x, y, 1.0 - z]);
        assert!(validate_extruded_cell(&inverted_z, 1.0).is_err());
    }

    #[test]
    fn whole_cell_certificate_refuses_folds_missed_by_gauss_points() {
        let boundary_fold = unit().map(|[x, y, z]| {
            let local = 2.0 * x - 1.0;
            [local + 0.525 * local * local, y, z]
        });
        let interior_fold = unit().map(|[x, y, z]| {
            let local = 2.0 * x - 1.0;
            let width = (local - 0.3).powi(2) - 0.01;
            [local, (2.0 * y - 1.0) * width, z]
        });
        for nodes in [boundary_fold, interior_fold] {
            chart::quadrature(1.0, |point, _| {
                assert!(chart::mapped(&nodes, 1.0, point).unwrap().determinant > 0.0);
            });
            assert!(validate_extruded_cell(&nodes, 1.0).is_err());
            assert!(stiffness(&nodes, 1.0, &tensor()).is_err());
        }
    }

    #[test]
    fn bounded_bernstein_subdivision_can_certify_positive_cells() {
        let nodes = unit().map(|[x, y, z]| {
            let local = 2.0 * x - 1.0;
            let width = (local - 0.3).powi(2) + 0.01;
            [local, (2.0 * y - 1.0) * width, z]
        });
        validate_extruded_cell(&nodes, 1.0).unwrap();
    }

    #[test]
    fn admission_and_stiffness_are_stable_under_large_exact_translation() {
        let nodes = unit();
        let offset = [2.0_f64.powi(40), -2.0_f64.powi(42), 2.0_f64.powi(39)];
        let translated = nodes.map(|point| std::array::from_fn(|axis| point[axis] + offset[axis]));
        for weight in [0.6, 0.8, 1.0] {
            validate_extruded_cell(&nodes, weight).unwrap();
            validate_extruded_cell(&translated, weight).unwrap();
            assert_eq!(
                stiffness(&nodes, weight, &tensor()).unwrap(),
                stiffness(&translated, weight, &tensor()).unwrap()
            );
        }
    }

    #[test]
    fn invalid_and_unresolvable_inputs_refuse() {
        for weight in [0.49, 1.01, f64::NAN, f64::INFINITY] {
            assert!(validate_extruded_cell(&unit(), weight).is_err());
        }
        let mut nodes = unit();
        nodes[0][0] = f64::NAN;
        assert!(validate_extruded_cell(&nodes, 1.0).is_err());
        let flat = unit().map(|[x, y, _]| [x, y, 0.0]);
        assert!(validate_extruded_cell(&flat, 1.0).is_err());
        assert!(validate_extruded_cell(&unit(), 0.5).is_err());
        let mut law = tensor();
        law[0][0] = f64::INFINITY;
        assert!(stiffness(&unit(), 1.0, &law).is_err());
        let overflowing = unit().map(|[x, y, z]| [(2.0 * x - 1.0) * f64::MAX, y, z]);
        assert!(
            validate_extruded_cell(&overflowing, 1.0)
                .unwrap_err()
                .starts_with("UnresolvedJacobian:")
        );
    }
}
