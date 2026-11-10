// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use std::ops::{Add, Mul, Neg, Sub};

/// Admits only source-Z prisms with stored radial/Z midpoints equal to `f64::midpoint`.
/// The certificate encloses the stored coordinates, including midpoint rounding;
/// quadrature samples never establish whole-cell positivity.
/// For in-plane `H/Q`, `Q >= 0.5`; the signed numerator
/// `det(Q H_x - H Q_x, H_y)` has degree (4, 3). Its tensor Bernstein
/// enclosure and the separately enclosed Z derivative must both be positive.
pub fn validate_extruded_cell(nodes: &[[f64; 3]; 27], midpoint_weight: f64) -> Result<(), String> {
    if !midpoint_weight.is_finite() || !(0.5..=1.0).contains(&midpoint_weight) {
        return Err("InvalidMappedCell: rational Hex27 weight must be in [0.5, 1]".into());
    }
    if nodes.iter().flatten().any(|value| !value.is_finite()) {
        return Err("InvalidMappedCell: nonfinite Hex27 coordinate".into());
    }
    let heights = [nodes[0][2], nodes[9][2], nodes[18][2]];
    for (node, position) in nodes.iter().enumerate() {
        if position[2] != heights[node / 9]
            || position[0] != nodes[node % 9][0]
            || position[1] != nodes[node % 9][1]
        {
            return Err("InvalidMappedCell: requires an exact source-Z prismatic extrusion".into());
        }
    }
    if heights[1] != heights[0].midpoint(heights[2]) || heights[2] <= heights[0] {
        return Err("InvalidMappedCell: requires increasing midpoint-interpolated Z planes".into());
    }
    for angular in 0..3 {
        for axis in [0, 1] {
            if nodes[angular + 3][axis] != nodes[angular][axis].midpoint(nodes[angular + 6][axis]) {
                return Err("InvalidMappedCell: requires radial-midpoint interpolation".into());
            }
        }
    }
    let z_middle = Interval::point(heights[1]) - Interval::point(heights[0]);
    let z_end = Interval::point(heights[2]) - Interval::point(heights[0]);
    let linear = z_end * Interval::point(0.5);
    let quadratic = (linear - z_middle) * Interval::point(2.0);
    if !(linear - quadratic).positive() || !(linear + quadratic).positive() {
        return Err("UnresolvedJacobian: could not certify the full Z derivative".into());
    }

    let homogeneous = homogeneous(nodes, midpoint_weight);
    let q0 = Interval::point(midpoint_weight);
    let q2 = Interval::point(1.0) - q0;
    let angular: [[[Interval; 3]; 3]; 2] = std::array::from_fn(|axis| {
        std::array::from_fn(|radial| {
            let [constant, linear, quadratic] = homogeneous[axis][radial];
            [
                linear * q0,
                (quadratic * q0 - constant * q2) * Interval::point(2.0),
                -(linear * q2),
            ]
        })
    });
    let radial: [[[Interval; 3]; 2]; 2] = std::array::from_fn(|axis| {
        [
            homogeneous[axis][1],
            homogeneous[axis][2].map(|value| value * Interval::point(2.0)),
        ]
    });
    let mut numerator = [[Interval::ZERO; 5]; 4];
    for angular_y in 0..3 {
        for angular_x in 0..3 {
            for radial_y in 0..2 {
                for radial_x in 0..3 {
                    let coefficient = angular[0][angular_y][angular_x]
                        * radial[1][radial_y][radial_x]
                        - angular[1][angular_y][angular_x] * radial[0][radial_y][radial_x];
                    numerator[angular_y + radial_y][angular_x + radial_x] =
                        numerator[angular_y + radial_y][angular_x + radial_x] + coefficient;
                }
            }
        }
    }
    let angular = numerator.map(bernstein);
    let radial: [[Interval; 4]; 5] = std::array::from_fn(|angular_index| {
        bernstein(std::array::from_fn(|index| angular[index][angular_index]))
    });
    let controls = std::array::from_fn(|y| std::array::from_fn(|x| radial[x][y]));
    let mut remaining = 4095;
    certify(controls, 0, &mut remaining)
}

fn homogeneous(nodes: &[[f64; 3]; 27], weight: f64) -> [[[Interval; 3]; 3]; 2] {
    let basis = [[0.0, -0.5, 0.5], [1.0, 0.0, -1.0], [0.0, 0.5, 0.5]];
    let weights = [1.0, weight, 1.0];
    let mut coefficients = [[[Interval::ZERO; 3]; 3]; 2];
    for radial in 0..3 {
        for angular in 0..3 {
            for axis in 0..2 {
                let coordinate = Interval::point(nodes[angular + 3 * radial][axis])
                    - Interval::point(nodes[0][axis]);
                for radial_power in 0..3 {
                    for angular_power in 0..3 {
                        coefficients[axis][radial_power][angular_power] = coefficients[axis]
                            [radial_power][angular_power]
                            + coordinate
                                * Interval::point(weights[angular])
                                * Interval::point(basis[radial][radial_power])
                                * Interval::point(basis[angular][angular_power]);
                    }
                }
            }
        }
    }
    coefficients
}

#[derive(Clone, Copy, Debug)]
struct Interval {
    lower: f64,
    upper: f64,
}

impl Interval {
    const ZERO: Self = Self::point(0.0);

    const fn point(value: f64) -> Self {
        Self {
            lower: value,
            upper: value,
        }
    }

    fn finite(self) -> bool {
        self.lower.is_finite() && self.upper.is_finite()
    }

    fn positive(self) -> bool {
        self.finite() && self.lower > 0.0
    }

    fn zero(self) -> bool {
        self.lower == 0.0 && self.upper == 0.0
    }

    fn divide(self, denominator: f64) -> Self {
        if self.zero() {
            return self;
        }
        Self {
            lower: (self.lower / denominator).next_down(),
            upper: (self.upper / denominator).next_up(),
        }
    }
}

impl Add for Interval {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        if self.zero() {
            return other;
        }
        if other.zero() {
            return self;
        }
        Self {
            lower: (self.lower + other.lower).next_down(),
            upper: (self.upper + other.upper).next_up(),
        }
    }
}

impl Neg for Interval {
    type Output = Self;

    fn neg(self) -> Self {
        Self {
            lower: -self.upper,
            upper: -self.lower,
        }
    }
}

impl Sub for Interval {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        self + -other
    }
}

impl Mul for Interval {
    type Output = Self;

    fn mul(self, other: Self) -> Self {
        if self.zero() || other.zero() {
            return Self::ZERO;
        }
        let products = [
            self.lower * other.lower,
            self.lower * other.upper,
            self.upper * other.lower,
            self.upper * other.upper,
        ];
        if products.iter().any(|product| product.is_nan()) {
            return Self {
                lower: f64::NEG_INFINITY,
                upper: f64::INFINITY,
            };
        }
        Self {
            lower: products
                .into_iter()
                .fold(f64::INFINITY, f64::min)
                .next_down(),
            upper: products
                .into_iter()
                .fold(f64::NEG_INFINITY, f64::max)
                .next_up(),
        }
    }
}

fn binomial(n: usize, k: usize) -> f64 {
    (0..k).fold(1, |value, index| value * (n - index) / (index + 1)) as f64
}

fn bernstein<const N: usize>(power: [Interval; N]) -> [Interval; N] {
    let unit: [Interval; N] = std::array::from_fn(|degree| {
        let mut coefficient = Interval::ZERO;
        for (source, value) in power.iter().enumerate().skip(degree) {
            let sign = if (source - degree) % 2 == 0 {
                1.0
            } else {
                -1.0
            };
            coefficient = coefficient + *value * Interval::point(sign * binomial(source, degree));
        }
        coefficient * Interval::point((1_u32 << degree) as f64)
    });
    std::array::from_fn(|index| {
        (0..=index).fold(Interval::ZERO, |sum, degree| {
            sum + (unit[degree] * Interval::point(binomial(index, degree)))
                .divide(binomial(N - 1, degree))
        })
    })
}

fn subdivide<const N: usize>(mut controls: [Interval; N]) -> ([Interval; N], [Interval; N]) {
    let mut left = [Interval::ZERO; N];
    let mut right = [Interval::ZERO; N];
    left[0] = controls[0];
    right[N - 1] = controls[N - 1];
    for level in 1..N {
        for index in 0..N - level {
            controls[index] = (controls[index] + controls[index + 1]) * Interval::point(0.5);
        }
        left[level] = controls[0];
        right[N - 1 - level] = controls[N - 1 - level];
    }
    (left, right)
}

fn certify(
    controls: [[Interval; 5]; 4],
    depth: usize,
    remaining: &mut usize,
) -> Result<(), String> {
    if controls.iter().flatten().any(|value| !value.finite()) {
        return Err("UnresolvedJacobian: nonfinite whole-cell interval certificate".into());
    }
    for radial in [0, 3] {
        for angular in [0, 4] {
            if controls[radial][angular].upper <= 0.0 {
                return Err("InvalidMappedCell: nonpositive whole-cell in-plane Jacobian".into());
            }
        }
    }
    if controls.iter().flatten().all(|value| value.positive()) {
        return Ok(());
    }
    if depth == 16 || *remaining == 0 {
        return Err("UnresolvedJacobian: whole-cell positivity unresolved after bounded Bernstein subdivision".into());
    }
    *remaining -= 1;
    let (left, right) = if depth.is_multiple_of(2) {
        let split = controls.map(subdivide);
        (split.map(|pair| pair.0), split.map(|pair| pair.1))
    } else {
        let split: [([Interval; 4], [Interval; 4]); 5] = std::array::from_fn(|angular| {
            subdivide(std::array::from_fn(|radial| controls[radial][angular]))
        });
        (
            std::array::from_fn(|radial| std::array::from_fn(|angular| split[angular].0[radial])),
            std::array::from_fn(|radial| std::array::from_fn(|angular| split[angular].1[radial])),
        )
    };
    certify(left, depth + 1, remaining)?;
    certify(right, depth + 1, remaining)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn positive_bernstein_subdivision_and_unresolved_bounds() {
        let line = bernstein([0.1, -0.6, 1.0, 0.0, 0.0].map(Interval::point));
        assert!(line.iter().any(|coefficient| coefficient.lower < 0.0));
        let mut remaining = 4095;
        certify([line; 4], 0, &mut remaining).unwrap();
        assert!(remaining < 4095);
        let controls = [[Interval {
            lower: -1.0,
            upper: 1.0,
        }; 5]; 4];
        let mut remaining = 1;
        assert!(
            certify(controls, 0, &mut remaining)
                .unwrap_err()
                .contains("unresolved")
        );
    }
}
