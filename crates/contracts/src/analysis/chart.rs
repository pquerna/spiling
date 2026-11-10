// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

mod certificate;
pub use certificate::validate_extruded_cell;

pub const GAUSS3: [(f64, f64); 3] = [
    (-0.7745966692414834, 5.0 / 9.0),
    (0.0, 8.0 / 9.0),
    (0.7745966692414834, 5.0 / 9.0),
];
pub const GAUSS5: [(f64, f64); 5] = [
    (-0.906179845938664, 0.23692688505618908),
    (-0.5384693101056831, 0.47862867049936647),
    (0.0, 0.5688888888888889),
    (0.5384693101056831, 0.47862867049936647),
    (0.906179845938664, 0.23692688505618908),
];

#[derive(Clone)]
pub struct MappedPoint {
    pub position: [f64; 3],
    pub values: [f64; 27],
    pub gradients: [[f64; 3]; 27],
    pub tangents: [[f64; 3]; 3],
    pub determinant: f64,
}

#[inline]
pub fn reference(point: [f64; 3], midpoint_weight: f64) -> ([f64; 27], [[f64; 3]; 27]) {
    let polynomial = point.map(|coordinate| {
        [
            0.5 * coordinate * (coordinate - 1.0),
            1.0 - coordinate * coordinate,
            0.5 * coordinate * (coordinate + 1.0),
        ]
    });
    let derivative =
        point.map(|coordinate| [coordinate - 0.5, -2.0 * coordinate, coordinate + 0.5]);
    let weights = [1.0, midpoint_weight, 1.0];
    let denominator = midpoint_weight + (1.0 - midpoint_weight) * point[0] * point[0];
    let denominator_derivative = 2.0 * (1.0 - midpoint_weight) * point[0];
    let mut values = [0.0; 27];
    let mut derivatives = [[0.0; 3]; 27];
    for (node, value) in values.iter_mut().enumerate() {
        let coordinates = [node % 3, (node / 3) % 3, node / 9];
        *value = (0..3)
            .map(|axis| polynomial[axis][coordinates[axis]])
            .product::<f64>()
            * weights[coordinates[0]]
            / denominator;
        for (axis, gradient) in derivatives[node].iter_mut().enumerate() {
            *gradient = (0..3)
                .map(|direction| {
                    if direction == axis {
                        derivative[direction][coordinates[direction]]
                    } else {
                        polynomial[direction][coordinates[direction]]
                    }
                })
                .product::<f64>()
                * weights[coordinates[0]]
                / denominator;
        }
        derivatives[node][0] -= *value * denominator_derivative / denominator;
    }
    (values, derivatives)
}

#[inline]
pub fn mapped(
    nodes: &[[f64; 3]; 27],
    midpoint_weight: f64,
    point: [f64; 3],
) -> Result<MappedPoint, &'static str> {
    if !midpoint_weight.is_finite() || !(0.5..=1.0).contains(&midpoint_weight) {
        return Err("invalid rational Hex27 weight");
    }
    let (values, derivatives) = reference(point, midpoint_weight);
    let origin = nodes[0];
    let position = std::array::from_fn(|axis| {
        origin[axis]
            + (0..27)
                .map(|node| values[node] * (nodes[node][axis] - origin[axis]))
                .sum::<f64>()
    });
    let tangents: [[f64; 3]; 3] = std::array::from_fn(|direction| {
        std::array::from_fn(|axis| {
            (0..27)
                .map(|node| derivatives[node][direction] * (nodes[node][axis] - origin[axis]))
                .sum()
        })
    });
    let cofactors: [[f64; 3]; 3] = std::array::from_fn(|direction| {
        cross(tangents[(direction + 1) % 3], tangents[(direction + 2) % 3])
    });
    let determinant = dot(tangents[0], cofactors[0]);
    if !determinant.is_finite() || determinant <= 0.0 {
        return Err("nonpositive or nonfinite mapped Hex27 Jacobian");
    }
    let gradients = std::array::from_fn(|node| {
        std::array::from_fn(|axis| {
            (0..3)
                .map(|direction| derivatives[node][direction] * cofactors[direction][axis])
                .sum::<f64>()
                / determinant
        })
    });
    Ok(MappedPoint {
        position,
        values,
        gradients,
        tangents,
        determinant,
    })
}

#[inline]
pub fn cross(first: [f64; 3], second: [f64; 3]) -> [f64; 3] {
    [
        first[1] * second[2] - first[2] * second[1],
        first[2] * second[0] - first[0] * second[2],
        first[0] * second[1] - first[1] * second[0],
    ]
}
#[inline]
pub fn dot(first: [f64; 3], second: [f64; 3]) -> f64 {
    (0..3).map(|axis| first[axis] * second[axis]).sum()
}

pub fn service_rotation(pose: crate::geometry::RigidPoseMm) -> [[f64; 3]; 3] {
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

pub fn strain(gradients: &[[f64; 3]; 27], displacements: &[[f64; 3]; 27]) -> [f64; 6] {
    let derivative: [[f64; 3]; 3] = std::array::from_fn(|component| {
        std::array::from_fn(|direction| {
            (0..27)
                .map(|node| displacements[node][component] * gradients[node][direction])
                .sum()
        })
    });
    [
        derivative[0][0],
        derivative[1][1],
        derivative[2][2],
        derivative[1][2] + derivative[2][1],
        derivative[0][2] + derivative[2][0],
        derivative[0][1] + derivative[1][0],
    ]
}

pub fn service_stress(
    tensor: &[[f64; 6]; 6],
    rotation: [[f64; 3]; 3],
    strain: [f64; 6],
) -> [f64; 6] {
    let service = [
        [strain[0], 0.5 * strain[5], 0.5 * strain[4]],
        [0.5 * strain[5], strain[1], 0.5 * strain[3]],
        [0.5 * strain[4], 0.5 * strain[3], strain[2]],
    ];
    let rotated: [[f64; 3]; 3] = std::array::from_fn(|row| {
        std::array::from_fn(|column| {
            (0..3)
                .flat_map(|first| {
                    (0..3).map(move |second| {
                        rotation[row][first] * service[first][second] * rotation[column][second]
                    })
                })
                .sum()
        })
    });
    let engineering = [
        rotated[0][0],
        rotated[1][1],
        rotated[2][2],
        2.0 * rotated[1][2],
        2.0 * rotated[0][2],
        2.0 * rotated[0][1],
    ];
    let stress: [f64; 6] = std::array::from_fn(|row| {
        (0..6)
            .map(|column| tensor[row][column] * engineering[column])
            .sum()
    });
    let print = [
        [stress[0], stress[5], stress[4]],
        [stress[5], stress[1], stress[3]],
        [stress[4], stress[3], stress[2]],
    ];
    let service: [[f64; 3]; 3] = std::array::from_fn(|row| {
        std::array::from_fn(|column| {
            (0..3)
                .flat_map(|first| {
                    (0..3).map(move |second| {
                        rotation[first][row] * print[first][second] * rotation[second][column]
                    })
                })
                .sum()
        })
    });
    [
        service[0][0],
        service[1][1],
        service[2][2],
        service[1][2],
        service[0][2],
        service[0][1],
    ]
}

pub fn internal_force(gradient: [f64; 3], stress: [f64; 6]) -> [f64; 3] {
    [
        gradient[0] * stress[0] + gradient[1] * stress[5] + gradient[2] * stress[4],
        gradient[0] * stress[5] + gradient[1] * stress[1] + gradient[2] * stress[3],
        gradient[0] * stress[4] + gradient[1] * stress[3] + gradient[2] * stress[2],
    ]
}

pub fn quadrature(midpoint_weight: f64, mut visit: impl FnMut([f64; 3], f64)) {
    let in_plane: &[(f64, f64)] = if midpoint_weight == 1.0 {
        &GAUSS3
    } else {
        &GAUSS5
    };
    for &(first, first_weight) in in_plane {
        for &(second, second_weight) in in_plane {
            for (third, third_weight) in GAUSS3 {
                visit(
                    [first, second, third],
                    first_weight * second_weight * third_weight,
                );
            }
        }
    }
}

pub fn volume(nodes: &[[f64; 3]; 27], midpoint_weight: f64) -> Result<f64, &'static str> {
    let mut volume = 0.0;
    let mut error = None;
    quadrature(midpoint_weight, |point, weight| {
        match mapped(nodes, midpoint_weight, point) {
            Ok(mapped) => volume += weight * mapped.determinant,
            Err(reason) => error = Some(reason),
        }
    });
    if let Some(reason) = error {
        return Err(reason);
    }
    Ok(volume)
}

pub fn facet_nodes(axis: usize, side: usize) -> impl Iterator<Item = usize> {
    (0..27).filter(move |node| [node % 3, (node / 3) % 3, node / 9][axis] == 2 * side)
}

pub fn rectangle_facet(
    nodes: &[[f64; 3]; 27],
    axis: usize,
    side: usize,
) -> Result<(usize, [f64; 3], [f64; 3]), &'static str> {
    let selected = facet_nodes(axis, side)
        .map(|node| nodes[node])
        .collect::<Vec<_>>();
    let lower: [f64; 3] = std::array::from_fn(|axis| {
        selected
            .iter()
            .map(|point| point[axis])
            .fold(f64::INFINITY, f64::min)
    });
    let upper: [f64; 3] = std::array::from_fn(|axis| {
        selected
            .iter()
            .map(|point| point[axis])
            .fold(f64::NEG_INFINITY, f64::max)
    });
    let normal = (0..3)
        .find(|axis| upper[*axis] - lower[*axis] <= 1e-8)
        .ok_or("facet is not an axis-aligned planar rectangle")?;
    if (0..3)
        .filter(|axis| *axis != normal)
        .any(|axis| upper[axis] - lower[axis] <= 1e-10)
    {
        return Err("facet rectangle has no finite positive area");
    }
    let mut center = std::array::from_fn(|axis| lower[axis].midpoint(upper[axis]));
    let mut size = std::array::from_fn(|axis| upper[axis] - lower[axis]);
    center[normal] -= 0.5;
    size[normal] = 1.0;
    Ok((normal, center, size))
}

pub fn locate(
    nodes: &[[f64; 3]; 27],
    midpoint_weight: f64,
    position: [f64; 3],
) -> Result<[f64; 3], &'static str> {
    let mut local = [0.0; 3];
    for _ in 0..20 {
        let mapped = mapped(nodes, midpoint_weight, local)?;
        let residual = std::array::from_fn(|axis| position[axis] - mapped.position[axis]);
        if residual.iter().map(|value| value * value).sum::<f64>() <= 1e-18 {
            return Ok(local);
        }
        for (direction, coordinate) in local.iter_mut().enumerate() {
            *coordinate += dot(
                residual,
                cross(
                    mapped.tangents[(direction + 1) % 3],
                    mapped.tangents[(direction + 2) % 3],
                ),
            ) / mapped.determinant;
        }
        if local
            .iter()
            .any(|value| !value.is_finite() || value.abs() > 4.0)
        {
            break;
        }
    }
    Err("physical point cannot be mapped into the admitted Hex27 chart")
}
