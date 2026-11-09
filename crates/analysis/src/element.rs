// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

pub type Tensor = [[f64; 6]; 6];
pub const GAUSS: [(f64, f64); 3] = [
    (-0.7745966692414834, 5.0 / 9.0),
    (0.0, 8.0 / 9.0),
    (0.7745966692414834, 5.0 / 9.0),
];

pub fn basis(point: [f64; 3], size: [f64; 3]) -> ([f64; 27], [[f64; 3]; 27]) {
    let polynomial = point.map(|coordinate| {
        [
            0.5 * coordinate * (coordinate - 1.0),
            1.0 - coordinate * coordinate,
            0.5 * coordinate * (coordinate + 1.0),
        ]
    });
    let derivative =
        point.map(|coordinate| [coordinate - 0.5, -2.0 * coordinate, coordinate + 0.5]);
    let mut values = [0.0; 27];
    let mut gradients = [[0.0; 3]; 27];
    for z_index in 0..3 {
        for y_index in 0..3 {
            for x_index in 0..3 {
                let index = x_index + 3 * y_index + 9 * z_index;
                let coordinates = [x_index, y_index, z_index];
                values[index] = (0..3)
                    .map(|axis| polynomial[axis][coordinates[axis]])
                    .product();
                for direction in 0..3 {
                    gradients[index][direction] = 2.0 / size[direction]
                        * (0..3)
                            .map(|axis| {
                                if axis == direction {
                                    derivative[axis][coordinates[axis]]
                                } else {
                                    polynomial[axis][coordinates[axis]]
                                }
                            })
                            .product::<f64>();
                }
            }
        }
    }
    (values, gradients)
}

pub fn quadrature(mut visit: impl FnMut([f64; 3], f64)) {
    for (zeta, z_weight) in GAUSS {
        for (eta, y_weight) in GAUSS {
            for (xi, x_weight) in GAUSS {
                visit([xi, eta, zeta], x_weight * y_weight * z_weight / 8.0);
            }
        }
    }
}

pub fn stiffness(size: [f64; 3], tensor: &Tensor) -> Vec<f64> {
    let mut matrix = vec![0.0; 81 * 81];
    quadrature(|point, weight| {
        let (_, gradients) = basis(point, size);
        let mut kinematic = [[0.0; 81]; 6];
        for (node, gradient) in gradients.iter().enumerate() {
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
                    * size.iter().product::<f64>()
                    * (0..6)
                        .map(|strain| kinematic[strain][row] * constitutive[strain][column])
                        .sum::<f64>();
            }
        }
    });
    matrix
}

pub fn state(
    gradients: &[[f64; 3]; 27],
    displacement: &[[f64; 3]; 27],
    tensor: &Tensor,
) -> ([f64; 6], [f64; 6]) {
    let derivative: [[f64; 3]; 3] = std::array::from_fn(|component| {
        std::array::from_fn(|direction| {
            (0..27)
                .map(|node| displacement[node][component] * gradients[node][direction])
                .sum()
        })
    });
    let strain = [
        derivative[0][0],
        derivative[1][1],
        derivative[2][2],
        derivative[1][2] + derivative[2][1],
        derivative[0][2] + derivative[2][0],
        derivative[0][1] + derivative[1][0],
    ];
    let stress = std::array::from_fn(|row| {
        (0..6)
            .map(|column| tensor[row][column] * strain[column])
            .sum()
    });
    (strain, stress)
}

pub fn force(gradient: [f64; 3], stress: [f64; 6]) -> [f64; 3] {
    [
        gradient[0] * stress[0] + gradient[1] * stress[5] + gradient[2] * stress[4],
        gradient[0] * stress[5] + gradient[1] * stress[1] + gradient[2] * stress[3],
        gradient[0] * stress[4] + gradient[1] * stress[3] + gradient[2] * stress[2],
    ]
}
pub fn stress_matrix(stress: [f64; 6]) -> [[f64; 3]; 3] {
    [
        [stress[0], stress[5], stress[4]],
        [stress[5], stress[1], stress[3]],
        [stress[4], stress[3], stress[2]],
    ]
}
pub fn rotate(tensor: &Tensor, service_to_print: [[f64; 3]; 3]) -> Tensor {
    let mut rotated = [[0.0; 6]; 6];
    for column in 0..6 {
        let mut engineering = [0.0; 6];
        engineering[column] = 1.0;
        let mut source = stress_matrix(engineering);
        for (row, values) in source.iter_mut().enumerate() {
            for (entry, value) in values.iter_mut().enumerate() {
                if row != entry {
                    *value *= 0.5;
                }
            }
        }
        let print: [[f64; 3]; 3] = std::array::from_fn(|row| {
            std::array::from_fn(|entry| {
                (0..3)
                    .flat_map(|first| {
                        (0..3).map(move |second| {
                            service_to_print[row][first]
                                * source[first][second]
                                * service_to_print[entry][second]
                        })
                    })
                    .sum()
            })
        });
        let print_strain = [
            print[0][0],
            print[1][1],
            print[2][2],
            2.0 * print[1][2],
            2.0 * print[0][2],
            2.0 * print[0][1],
        ];
        let print_stress = stress_matrix(std::array::from_fn(|row| {
            (0..6)
                .map(|entry| tensor[row][entry] * print_strain[entry])
                .sum()
        }));
        let service: [[f64; 3]; 3] = std::array::from_fn(|row| {
            std::array::from_fn(|entry| {
                (0..3)
                    .flat_map(|first| {
                        (0..3).map(move |second| {
                            service_to_print[first][row]
                                * print_stress[first][second]
                                * service_to_print[second][entry]
                        })
                    })
                    .sum()
            })
        });
        let vector = [
            service[0][0],
            service[1][1],
            service[2][2],
            service[1][2],
            service[0][2],
            service[0][1],
        ];
        for row in 0..6 {
            rotated[row][column] = vector[row];
        }
    }
    rotated
}
