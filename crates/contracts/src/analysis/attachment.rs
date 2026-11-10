// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Deterministic physical contact quadrature and ideal distributing connections.
//! Rotations are scaled to millimetres by the contact extent; no contact stiffness,
//! preload, friction, clearance or thread-strength model is implied.
use super::{chart, fastener::*, invalid};
use crate::project::ProjectError;
use std::collections::{BTreeMap, BTreeSet};

const MAX_SAMPLES: usize = 20_000;

pub struct AttachmentFacet {
    pub element_index: u32,
    pub axis: usize,
    pub side: usize,
    pub nodes: [u32; 27],
    pub positions: [[f64; 3]; 27],
    pub midpoint_weight: f64,
}
#[derive(Clone)]
pub struct AttachmentSample {
    pub element_index: u32,
    pub local_point: [f64; 3],
    pub point_mm: [f64; 3],
    pub area_mm2: f64,
    pub nodes: [u32; 27],
    pub values: [f64; 27],
}

fn extent(fastener: &AnalysisFastener) -> f64 {
    let radius = match fastener.attachment {
        FastenerAttachment::Clamp {
            washer_diameter_mm, ..
        } => washer_diameter_mm * 0.5,
        _ => fastener.hole.radius_mm,
    };
    radius.max(fastener.hole.depth_mm)
}
fn basis(point: [f64; 3], center: [f64; 3], scale: f64) -> [[f64; 6]; 3] {
    let offset = std::array::from_fn(|axis| (point[axis] - center[axis]) / scale);
    std::array::from_fn(|axis| {
        std::array::from_fn(|motion| {
            if motion < 3 {
                if axis == motion { 1.0 } else { 0.0 }
            } else {
                let unit = std::array::from_fn(
                    |direction| if direction == motion - 3 { 1.0 } else { 0.0 },
                );
                chart::cross(unit, offset)[axis]
            }
        })
    })
}
fn evaluate(
    facet: &AttachmentFacet,
    local: [f64; 3],
    weight: f64,
    axis: usize,
) -> Result<AttachmentSample, ProjectError> {
    let mapped = chart::mapped(&facet.positions, facet.midpoint_weight, local).map_err(invalid)?;
    let normal = chart::cross(
        mapped.tangents[(axis + 1) % 3],
        mapped.tangents[(axis + 2) % 3],
    );
    let area = weight * chart::dot(normal, normal).sqrt();
    if !area.is_finite() || area <= 0.0 {
        return Err(invalid(
            "attachment has no finite positive physical Jacobian",
        ));
    }
    Ok(AttachmentSample {
        element_index: facet.element_index,
        local_point: local,
        point_mm: mapped.position,
        area_mm2: area,
        nodes: facet.nodes,
        values: mapped.values,
    })
}

// All circle/band crossings are roots of this quartic, including tangencies.
// Splitting at them avoids integrating a discontinuous clipped band indicator.
fn circle_crossings(
    facet: &AttachmentFacet,
    radial: usize,
    center: [f64; 3],
    radius: f64,
) -> Vec<f64> {
    let w = facet.midpoint_weight;
    let q = [w, 0.0, 1.0 - w];
    let h: [[f64; 3]; 2] = std::array::from_fn(|axis| {
        let p = [0, 1, 2].map(|angle| facet.positions[angle + 3 * radial][axis] - center[axis]);
        [
            w * p[1],
            0.5 * (p[2] - p[0]),
            0.5 * (p[0] + p[2]) - w * p[1],
        ]
    });
    let mut polynomial = vec![0.0; 5];
    for i in 0..3 {
        for j in 0..3 {
            polynomial[i + j] +=
                h[0][i] * h[0][j] + h[1][i] * h[1][j] - radius * radius * q[i] * q[j];
        }
    }
    roots(&polynomial)
}
fn roots(polynomial: &[f64]) -> Vec<f64> {
    let scale = polynomial.iter().map(|v| v.abs()).sum::<f64>();
    if scale == 0.0 {
        return Vec::new();
    }
    let mut degree = polynomial.len() - 1;
    while degree > 0 && polynomial[degree].abs() <= scale * 1e-14 {
        degree -= 1;
    }
    if degree == 0 {
        return Vec::new();
    }
    let value = |x: f64| {
        polynomial[..=degree]
            .iter()
            .rev()
            .fold(0.0, |v, c| v * x + c)
    };
    let derivative = (1..=degree)
        .map(|i| i as f64 * polynomial[i])
        .collect::<Vec<_>>();
    let mut cuts = vec![-1.0];
    cuts.extend(roots(&derivative));
    cuts.push(1.0);
    let mut result = cuts
        .iter()
        .copied()
        .filter(|x| value(*x).abs() <= scale * 1e-13)
        .collect::<Vec<_>>();
    for pair in cuts.windows(2) {
        let (mut a, mut b) = (pair[0], pair[1]);
        let mut fa = value(a);
        if fa * value(b) >= 0.0 {
            continue;
        }
        for _ in 0..60 {
            let middle = a.midpoint(b);
            let fm = value(middle);
            if (fa > 0.0) == (fm > 0.0) {
                a = middle;
                fa = fm;
            } else {
                b = middle;
            }
        }
        result.push(a.midpoint(b));
    }
    result.sort_by(f64::total_cmp);
    result.dedup_by(|a, b| (*a - *b).abs() < 1e-11);
    result
}
fn radial_interval(
    facet: &AttachmentFacet,
    fastener: &AnalysisFastener,
    angular: f64,
    radius: f64,
) -> Result<Option<[f64; 2]>, ProjectError> {
    let a = chart::mapped(
        &facet.positions,
        facet.midpoint_weight,
        [angular, -1.0, 0.0],
    )
    .map_err(invalid)?
    .position;
    let b = chart::mapped(&facet.positions, facet.midpoint_weight, [angular, 1.0, 0.0])
        .map_err(invalid)?
        .position;
    let d = [b[0] - a[0], b[1] - a[1], 0.0];
    let p = [
        a[0] - fastener.hole.center_mm[0],
        a[1] - fastener.hole.center_mm[1],
        0.0,
    ];
    let aa = chart::dot(d, d);
    let bb = chart::dot(p, d);
    let cc = chart::dot(p, p) - radius * radius;
    let discriminant = bb * bb - aa * cc;
    if aa <= 0.0 || !discriminant.is_finite() {
        return Err(invalid("attachment radial chart is degenerate"));
    }
    if discriminant <= 0.0 {
        return Ok(None);
    }
    // Stable quadratic roots for the physical line/circle intersection.
    let q = -bb - discriminant.sqrt().copysign(bb);
    let (r0, r1) = if q == 0.0 {
        (-bb / aa, -bb / aa)
    } else {
        (q / aa, cc / q)
    };
    let lower = r0.min(r1).max(0.0);
    let upper = r0.max(r1).min(1.0);
    Ok((upper > lower).then_some([2.0 * lower - 1.0, 2.0 * upper - 1.0]))
}
fn strip(
    facet: &AttachmentFacet,
    fastener: &AnalysisFastener,
    angular: [f64; 2],
    axial: Option<[f64; 2]>,
) -> Result<Vec<AttachmentSample>, ProjectError> {
    let mut result = Vec::new();
    for (a, wa) in chart::GAUSS5 {
        let angle = angular[0].midpoint(angular[1]) + 0.5 * (angular[1] - angular[0]) * a;
        let interval = if let Some(axial) = axial {
            axial
        } else {
            let FastenerAttachment::Clamp {
                washer_diameter_mm, ..
            } = fastener.attachment
            else {
                unreachable!()
            };
            let Some(interval) = radial_interval(facet, fastener, angle, 0.5 * washer_diameter_mm)?
            else {
                continue;
            };
            interval
        };
        for (b, wb) in chart::GAUSS3 {
            let other = interval[0].midpoint(interval[1]) + 0.5 * (interval[1] - interval[0]) * b;
            let local = if axial.is_some() {
                [angle, -1.0, other]
            } else {
                [angle, other, 2.0 * facet.side as f64 - 1.0]
            };
            result.push(evaluate(
                facet,
                local,
                0.25 * (angular[1] - angular[0]) * (interval[1] - interval[0]) * wa * wb,
                if axial.is_some() { 1 } else { 2 },
            )?);
        }
    }
    Ok(result)
}
fn moments(samples: &[AttachmentSample], fastener: &AnalysisFastener) -> [f64; 121] {
    let mut result = [0.0; 121];
    for sample in samples {
        let p: [f64; 3] = std::array::from_fn(|axis| {
            (sample.point_mm[axis] - fastener.hole.center_mm[axis]) / extent(fastener)
        });
        let a = sample.area_mm2;
        result[0] += a;
        for i in 0..3 {
            result[1 + i] += a * p[i];
            for j in 0..3 {
                result[4 + 3 * i + j] += a * p[i] * p[j];
            }
        }
        for node in 0..27 {
            result[13 + node] += a * sample.values[node];
            for axis in 0..3 {
                result[40 + 3 * node + axis] += a * p[axis] * sample.values[node];
            }
        }
    }
    result
}
fn adaptive(
    facet: &AttachmentFacet,
    fastener: &AnalysisFastener,
    angular: [f64; 2],
    axial: Option<[f64; 2]>,
    depth: usize,
    result: &mut Vec<AttachmentSample>,
) -> Result<(), ProjectError> {
    let coarse = strip(facet, fastener, angular, axial)?;
    let middle = angular[0].midpoint(angular[1]);
    let mut fine = strip(facet, fastener, [angular[0], middle], axial)?;
    fine.extend(strip(facet, fastener, [middle, angular[1]], axial)?);
    let a = moments(&coarse, fastener);
    let b = moments(&fine, fastener);
    let span = facet
        .positions
        .iter()
        .flat_map(|point| (0..3).map(move |axis| (point[axis] - facet.positions[0][axis]).abs()))
        .fold(0.0, f64::max);
    let roundoff = 128.0 * f64::EPSILON * span.powi(2) * (angular[1] - angular[0]);
    let tolerance = (1e-11 * b[0]).max(roundoff);
    if a.iter().zip(b).any(|(a, b)| (a - b).abs() > tolerance) {
        if depth == 12 {
            return Err(invalid(&format!(
                "attachment quadrature did not resolve its physical footprint: element={} angular={angular:?} area={} difference={} tolerance={tolerance}",
                facet.element_index,
                b[0],
                a.iter()
                    .zip(b)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0, f64::max)
            )));
        }
        adaptive(
            facet,
            fastener,
            [angular[0], middle],
            axial,
            depth + 1,
            result,
        )?;
        adaptive(
            facet,
            fastener,
            [middle, angular[1]],
            axial,
            depth + 1,
            result,
        )?;
    } else {
        if result.len() + fine.len() > MAX_SAMPLES {
            return Err(invalid(
                "attachment exceeds the 20000 physical sample budget",
            ));
        }
        result.extend(fine);
    }
    Ok(())
}

/// Reconstruct physical contact from admitted exterior facets, never display data.
/// The caller owns whole-cell mapping/exterior certification and native face identity.
pub fn samples(
    fastener: &AnalysisFastener,
    facets: &[AttachmentFacet],
) -> Result<Vec<AttachmentSample>, ProjectError> {
    fastener.validate()?;
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    let center = fastener.hole.center_mm;
    for facet in facets {
        if facet.axis > 2
            || facet.side > 1
            || !seen.insert((facet.element_index, facet.axis, facet.side))
            || facet.positions.iter().flatten().any(|v| !v.is_finite())
        {
            return Err(invalid("attachment requires unique finite exterior facets"));
        }
        match fastener.attachment {
            FastenerAttachment::Clamp {
                end,
                washer_diameter_mm,
            } => {
                let side = usize::from(end == HoleEnd::End);
                let z = center[2] + side as f64 * fastener.hole.depth_mm;
                if facet.axis != 2
                    || facet.side != side
                    || chart::facet_nodes(2, side)
                        .any(|node| (facet.positions[node][2] - z).abs() > 1e-7)
                {
                    continue;
                }
                let mut cuts = vec![-1.0, 1.0];
                for radial in [0, 2] {
                    cuts.extend(circle_crossings(
                        facet,
                        radial,
                        center,
                        0.5 * washer_diameter_mm,
                    ));
                }
                cuts.sort_by(f64::total_cmp);
                cuts.dedup_by(|a, b| (*a - *b).abs() < 1e-11);
                for pair in cuts.windows(2) {
                    adaptive(facet, fastener, [pair[0], pair[1]], None, 0, &mut result)?;
                }
            }
            _ => {
                if facet.axis != 1
                    || facet.side != 0
                    || chart::facet_nodes(1, 0).any(|node| {
                        ((facet.positions[node][0] - center[0])
                            .hypot(facet.positions[node][1] - center[1])
                            - fastener.hole.radius_mm)
                            .abs()
                            > 1e-7 * (1.0 + fastener.hole.radius_mm)
                    })
                {
                    continue;
                }
                let range = match fastener.attachment {
                    FastenerAttachment::BondedEngagement {
                        end: HoleEnd::Start,
                        engagement_mm,
                    } => [center[2], center[2] + engagement_mm],
                    FastenerAttachment::BondedEngagement {
                        end: HoleEnd::End,
                        engagement_mm,
                    } => [
                        center[2] + fastener.hole.depth_mm - engagement_mm,
                        center[2] + fastener.hole.depth_mm,
                    ],
                    _ => [center[2], center[2] + fastener.hole.depth_mm],
                };
                let z0 = facet.positions[0][2];
                let z1 = facet.positions[18][2];
                if z1 <= z0 {
                    return Err(invalid("attachment needs increasing source-Z extrusion"));
                }
                let lo = range[0].max(z0);
                let hi = range[1].min(z1);
                if hi > lo {
                    adaptive(
                        facet,
                        fastener,
                        [-1.0, 1.0],
                        Some([
                            2.0 * (lo - z0) / (z1 - z0) - 1.0,
                            2.0 * (hi - z0) / (z1 - z0) - 1.0,
                        ]),
                        0,
                        &mut result,
                    )?;
                }
            }
        }
    }
    result.sort_by(|a, b| {
        a.element_index
            .cmp(&b.element_index)
            .then_with(|| a.local_point[0].total_cmp(&b.local_point[0]))
            .then_with(|| a.local_point[1].total_cmp(&b.local_point[1]))
            .then_with(|| a.local_point[2].total_cmp(&b.local_point[2]))
    });
    let expected = match fastener.attachment {
        FastenerAttachment::Clamp {
            washer_diameter_mm, ..
        } => {
            std::f64::consts::PI
                * ((washer_diameter_mm * 0.5).powi(2) - fastener.hole.radius_mm.powi(2))
        }
        FastenerAttachment::BondedEngagement { engagement_mm, .. } => {
            std::f64::consts::TAU * fastener.hole.radius_mm * engagement_mm
        }
        _ => std::f64::consts::TAU * fastener.hole.radius_mm * fastener.hole.depth_mm,
    };
    let area = result.iter().map(|s| s.area_mm2).sum::<f64>();
    if !area.is_finite() || (area / expected - 1.0).abs() > 1e-9 {
        return Err(invalid(
            "attachment must cover the complete physical washer annulus or declared bore length; check footprint and native seat",
        ));
    }
    Ok(result)
}

/// The reference is `hole.center_mm` (the lower source-Z bore center).
/// Rotation displacements are angle times `max(hole.depth_mm, contact_radius)`;
/// contact radius is the washer radius for Clamp and the hole radius otherwise.
/// A traction multiplier therefore has units N for each of the six motions.
pub struct Coupling {
    pub area_mm2: f64,
    /// Six generalized displacements in mm; DOFs use node*3+source_axis.
    pub rows: [Vec<(usize, f64)>; 6],
    inverse: [[f64; 6]; 6],
    center: [f64; 3],
    scale: f64,
}
impl Coupling {
    /// Multipliers are generalized forces conjugate to the six scaled motions.
    pub fn traction(&self, multipliers: [f64; 6], point: [f64; 3]) -> [f64; 3] {
        let dual: [f64; 6] =
            std::array::from_fn(|i| (0..6).map(|j| self.inverse[j][i] * multipliers[j]).sum());
        let b = basis(point, self.center, self.scale);
        std::array::from_fn(|axis| {
            (0..6).map(|i| b[axis][i] * dual[i]).sum::<f64>() / self.area_mm2
        })
    }
    pub fn motions(&self, displacement: &[[f64; 3]]) -> [f64; 6] {
        std::array::from_fn(|i| {
            self.rows[i]
                .iter()
                .map(|&(dof, value)| value * displacement[dof / 3][dof % 3])
                .sum()
        })
    }
}
pub fn coupling(
    fastener: &AnalysisFastener,
    samples: &[AttachmentSample],
    node_count: usize,
) -> Result<Coupling, ProjectError> {
    fastener.validate()?;
    if samples.is_empty() || samples.len() > MAX_SAMPLES || node_count.checked_mul(3).is_none() {
        return Err(invalid(
            "attachment coupling requires bounded physical contact samples",
        ));
    }
    let area = samples.iter().map(|s| s.area_mm2).sum::<f64>();
    if !area.is_finite() || area <= 0.0 {
        return Err(invalid(
            "attachment coupling requires positive physical area",
        ));
    }
    let scale = extent(fastener);
    let center = fastener.hole.center_mm;
    let mut gram = [[0.0; 6]; 6];
    let mut rhs: [BTreeMap<usize, f64>; 6] = std::array::from_fn(|_| BTreeMap::new());
    for sample in samples {
        if sample.nodes.iter().any(|node| *node as usize >= node_count)
            || sample
                .values
                .iter()
                .chain(&sample.point_mm)
                .any(|v| !v.is_finite())
            || !sample.area_mm2.is_finite()
            || sample.area_mm2 <= 0.0
        {
            return Err(invalid("invalid attachment sample coefficient or node"));
        }
        let b = basis(sample.point_mm, center, scale);
        let weight = sample.area_mm2 / area;
        for i in 0..6 {
            for j in 0..6 {
                gram[i][j] += weight * (0..3).map(|axis| b[axis][i] * b[axis][j]).sum::<f64>();
            }
            for node in 0..27 {
                for (axis, basis_row) in b.iter().enumerate() {
                    *rhs[i]
                        .entry(3 * sample.nodes[node] as usize + axis)
                        .or_default() += weight * basis_row[i] * sample.values[node];
                }
            }
        }
    }
    if gram
        .iter()
        .flatten()
        .chain(rhs.iter().flat_map(|row| row.values()))
        .any(|v| !v.is_finite())
    {
        return Err(invalid("nonfinite attachment projection integral"));
    }
    let mut inverse: [[f64; 6]; 6] =
        std::array::from_fn(|i| std::array::from_fn(|j| if i == j { 1.0 } else { 0.0 }));
    let maximum = gram.iter().flatten().map(|v| v.abs()).fold(0.0, f64::max);
    for column in 0..6 {
        let pivot = (column..6)
            .max_by(|&a, &b| gram[a][column].abs().total_cmp(&gram[b][column].abs()))
            .unwrap();
        if gram[pivot][column].abs() <= 1e-12 * maximum {
            return Err(invalid(
                "physical attachment rigid-motion projection is rank deficient",
            ));
        }
        gram.swap(column, pivot);
        inverse.swap(column, pivot);
        let divisor = gram[column][column];
        for j in 0..6 {
            gram[column][j] /= divisor;
            inverse[column][j] /= divisor;
        }
        let pivot_gram = gram[column];
        let pivot_inverse = inverse[column];
        for i in 0..6 {
            if i != column {
                let factor = gram[i][column];
                for j in 0..6 {
                    gram[i][j] -= factor * pivot_gram[j];
                    inverse[i][j] -= factor * pivot_inverse[j];
                }
            }
        }
    }
    let mut rows: [BTreeMap<usize, f64>; 6] = std::array::from_fn(|_| BTreeMap::new());
    for i in 0..6 {
        for j in 0..6 {
            for (&dof, &value) in &rhs[j] {
                *rows[i].entry(dof).or_default() += inverse[i][j] * value;
            }
        }
    }
    let rows = rows.map(|row| row.into_iter().filter(|(_, v)| *v != 0.0).collect());
    Ok(Coupling {
        area_mm2: area,
        rows,
        inverse,
        center,
        scale,
    })
}
