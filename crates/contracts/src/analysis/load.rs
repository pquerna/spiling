// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ForceFootprint {
    WholeFace {},
    Disk {
        center_mm: [f64; 3],
        diameter_mm: f64,
    },
}
impl ForceFootprint {
    pub fn validate(&self) -> Result<(), ProjectError> {
        if let Self::Disk {
            center_mm,
            diameter_mm,
        } = self
            && (center_mm.iter().any(|value| !value.is_finite())
                || !diameter_mm.is_finite()
                || *diameter_mm < 0.01
                || *diameter_mm > 1e6)
        {
            return Err(invalid(
                "contact disk requires a finite center and diameter in 0.01..1000000 mm",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AnalysisFacePreparation {
    pub project_id: ProjectId,
    pub project_revision: ProjectRevision,
    pub reference: FaceRef,
    pub source_hash: SourceHash,
    pub pose: RigidPoseMm,
    pub area_mm2: f64,
    pub centroid_mm: [f64; 3],
    pub outward_normal: [f64; 3],
    pub origin_mm: [f64; 3],
    pub horizontal_direction: [f64; 3],
    pub width_mm: f64,
    pub height_mm: f64,
}

impl AnalysisFacePreparation {
    pub fn validate(&self) -> Result<(), ProjectError> {
        self.pose
            .validate()
            .map_err(|error| invalid(&error.to_string()))?;
        let norm: f64 = self.outward_normal.iter().map(|value| value * value).sum();
        if !self.area_mm2.is_finite()
            || self.area_mm2 <= 0.0
            || self
                .centroid_mm
                .iter()
                .chain(&self.outward_normal)
                .chain(&self.origin_mm)
                .chain(&self.horizontal_direction)
                .any(|value| !value.is_finite())
            || !norm.is_finite()
            || (norm - 1.0).abs() > 1e-10
            || !self.width_mm.is_finite()
            || self.width_mm <= 0.0
            || !self.height_mm.is_finite()
            || self.height_mm <= 0.0
            || (self.area_mm2 - self.width_mm * self.height_mm).abs() > 1e-8 * self.area_mm2
            || (self
                .horizontal_direction
                .iter()
                .map(|value| value * value)
                .sum::<f64>()
                - 1.0)
                .abs()
                > 1e-10
            || self.horizontal_direction[2].abs() > 1e-10
            || self.outward_normal[2].abs() > 1e-10
            || (0..3)
                .map(|axis| self.horizontal_direction[axis] * self.outward_normal[axis])
                .sum::<f64>()
                .abs()
                > 1e-10
            || (0..3).any(|axis| {
                (self.centroid_mm[axis]
                    - self.origin_mm[axis]
                    - self.horizontal_direction[axis] * self.width_mm * 0.5
                    - if axis == 2 { self.height_mm * 0.5 } else { 0.0 })
                .abs()
                    > 1e-7
            })
        {
            return Err(invalid("invalid native analysis face preparation"));
        }
        Ok(())
    }

    pub fn load(
        &self,
        force_n: [f64; 3],
        couple_n_mm: [f64; 3],
    ) -> Result<AnalysisFaceLoad, ProjectError> {
        self.load_on(ForceFootprint::WholeFace {}, force_n, couple_n_mm)
    }

    pub fn project(&self, seed: [f64; 3]) -> Result<[f64; 3], ProjectError> {
        self.validate()?;
        let horizontal: f64 = (0..3)
            .map(|axis| (seed[axis] - self.origin_mm[axis]) * self.horizontal_direction[axis])
            .sum();
        let vertical = seed[2] - self.origin_mm[2];
        if seed.iter().any(|value| !value.is_finite())
            || horizontal < -1e-6
            || horizontal > self.width_mm + 1e-6
            || vertical < -1e-6
            || vertical > self.height_mm + 1e-6
        {
            return Err(invalid("contact center lies outside native trimmed face"));
        }
        Ok(std::array::from_fn(|axis| {
            self.origin_mm[axis]
                + self.horizontal_direction[axis] * horizontal.clamp(0.0, self.width_mm)
                + if axis == 2 {
                    vertical.clamp(0.0, self.height_mm)
                } else {
                    0.0
                }
        }))
    }

    pub fn contact(&self, footprint: &ForceFootprint) -> Result<(f64, [f64; 3]), ProjectError> {
        self.validate()?;
        footprint.validate()?;
        let ForceFootprint::Disk {
            center_mm,
            diameter_mm,
        } = footprint
        else {
            return Ok((self.area_mm2, self.centroid_mm));
        };
        let projected = self.project(*center_mm)?;
        if (0..3).any(|axis| (projected[axis] - center_mm[axis]).abs() > 1e-7) {
            return Err(invalid("contact center is not on native face plane"));
        }
        let horizontal: f64 = (0..3)
            .map(|axis| (center_mm[axis] - self.origin_mm[axis]) * self.horizontal_direction[axis])
            .sum();
        let samples = disk_quadrature(
            [horizontal, center_mm[2] - self.origin_mm[2]],
            *diameter_mm * 0.5,
            [[0.0, self.width_mm], [0.0, self.height_mm]],
        );
        let area = samples.iter().map(|sample| sample.1).sum::<f64>();
        if !area.is_finite() || area <= 1e-12 {
            return Err(invalid("contact has no finite positive native area"));
        }
        let mean: [f64; 2] = std::array::from_fn(|axis| {
            samples
                .iter()
                .map(|sample| sample.0[axis] * sample.1)
                .sum::<f64>()
                / area
        });
        Ok((
            area,
            std::array::from_fn(|axis| {
                self.origin_mm[axis]
                    + self.horizontal_direction[axis] * mean[0]
                    + if axis == 2 { mean[1] } else { 0.0 }
            }),
        ))
    }

    pub fn load_on(
        &self,
        footprint: ForceFootprint,
        force_n: [f64; 3],
        couple_n_mm: [f64; 3],
    ) -> Result<AnalysisFaceLoad, ProjectError> {
        let (_, center) = self.contact(&footprint)?;
        let moment_n_mm = std::array::from_fn(|axis| {
            let next = (axis + 1) % 3;
            let last = (axis + 2) % 3;
            center[next] * force_n[last] - center[last] * force_n[next] + couple_n_mm[axis]
        });
        if force_n
            .iter()
            .chain(&couple_n_mm)
            .chain(&moment_n_mm)
            .any(|value| !value.is_finite())
        {
            return Err(invalid("face force/couple produces a nonfinite wrench"));
        }
        Ok(AnalysisFaceLoad {
            face_id: self.reference.face_id.clone(),
            footprint,
            force_n,
            moment_n_mm,
        })
    }
}

pub fn disk_quadrature(
    center: [f64; 2],
    radius: f64,
    bounds: [[f64; 2]; 2],
) -> Vec<([f64; 2], f64)> {
    const GAUSS: [(f64, f64); 8] = [
        (-0.9602898564975363, 0.1012285362903763),
        (-0.7966664774136267, 0.2223810344533745),
        (-0.525532409916329, 0.3137066458778873),
        (-0.1834346424956498, 0.362683783378362),
        (0.1834346424956498, 0.362683783378362),
        (0.525532409916329, 0.3137066458778873),
        (0.7966664774136267, 0.2223810344533745),
        (0.9602898564975363, 0.1012285362903763),
    ];
    if !radius.is_finite()
        || radius <= 0.0
        || center
            .iter()
            .chain(bounds.iter().flatten())
            .any(|value| !value.is_finite())
        || bounds.iter().any(|bound| bound[0] >= bound[1])
    {
        return Vec::new();
    }
    let lower = bounds[0][0].max(center[0] - radius);
    let upper = bounds[0][1].min(center[0] + radius);
    if lower >= upper {
        return Vec::new();
    }
    let mut cuts = vec![lower, upper];
    for vertical in bounds[1] {
        let delta = vertical - center[1];
        if delta.abs() < radius {
            let offset = (radius * radius - delta * delta).sqrt();
            for point in [center[0] - offset, center[0] + offset] {
                if point > lower && point < upper {
                    cuts.push(point);
                }
            }
        }
    }
    cuts.sort_by(f64::total_cmp);
    cuts.dedup();
    let mut result = Vec::new();
    for pair in cuts.windows(2) {
        let first = ((pair[0] - center[0]) / radius).clamp(-1.0, 1.0).asin();
        let last = ((pair[1] - center[0]) / radius).clamp(-1.0, 1.0).asin();
        for panel in 0..2 {
            let start = first + (last - first) * panel as f64 * 0.5;
            let half = (last - first) * 0.25;
            for (point, weight) in GAUSS {
                let angle = start + half * (point + 1.0);
                let horizontal = center[0] + radius * angle.sin();
                let extent = radius * angle.cos();
                let bottom = bounds[1][0].max(center[1] - extent);
                let top = bounds[1][1].min(center[1] + extent);
                if bottom >= top {
                    continue;
                }
                for (vertical, vertical_weight) in [
                    (-(3.0_f64 / 5.0).sqrt(), 5.0 / 9.0),
                    (0.0, 8.0 / 9.0),
                    ((3.0_f64 / 5.0).sqrt(), 5.0 / 9.0),
                ] {
                    result.push((
                        [horizontal, bottom + (top - bottom) * (vertical + 1.0) * 0.5],
                        weight
                            * half
                            * radius
                            * angle.cos()
                            * (top - bottom)
                            * 0.5
                            * vertical_weight,
                    ));
                }
            }
        }
    }
    result
}

pub fn surface_samples(
    footprint: &ForceFootprint,
    center: [f64; 3],
    size: [f64; 3],
    axis: usize,
    side: usize,
) -> Vec<([f64; 3], f64)> {
    let tangent: Vec<_> = (0..3).filter(|value| *value != axis).collect();
    let plane = center[axis] + (side as f64 - 0.5) * size[axis];
    let mut result = Vec::new();
    match footprint {
        ForceFootprint::WholeFace {} => {
            for (first, first_weight) in [
                (-(3.0_f64 / 5.0).sqrt(), 5.0 / 9.0),
                (0.0, 8.0 / 9.0),
                ((3.0_f64 / 5.0).sqrt(), 5.0 / 9.0),
            ] {
                for (second, second_weight) in [
                    (-(3.0_f64 / 5.0).sqrt(), 5.0 / 9.0),
                    (0.0, 8.0 / 9.0),
                    ((3.0_f64 / 5.0).sqrt(), 5.0 / 9.0),
                ] {
                    let mut point = center;
                    point[axis] = plane;
                    point[tangent[0]] += first * size[tangent[0]] * 0.5;
                    point[tangent[1]] += second * size[tangent[1]] * 0.5;
                    result.push((
                        point,
                        size[tangent[0]] * size[tangent[1]] * first_weight * second_weight * 0.25,
                    ));
                }
            }
        }
        ForceFootprint::Disk {
            center_mm,
            diameter_mm,
        } => {
            if (center_mm[axis] - plane).abs() > 1e-7 {
                return result;
            }
            let bounds = std::array::from_fn(|index| {
                [
                    center[tangent[index]] - size[tangent[index]] * 0.5,
                    center[tangent[index]] + size[tangent[index]] * 0.5,
                ]
            });
            for (sample, weight) in disk_quadrature(
                [center_mm[tangent[0]], center_mm[tangent[1]]],
                diameter_mm * 0.5,
                bounds,
            ) {
                let mut point = center;
                point[axis] = plane;
                point[tangent[0]] = sample[0];
                point[tangent[1]] = sample[1];
                result.push((point, weight));
            }
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disks_reproduce_closed_form_area_centroid_and_second_moments() {
        for radius in [0.005, 0.5, 8.0, 100.0] {
            let samples = disk_quadrature([0.0, 0.0], radius, [[-radius, radius]; 2]);
            let area = samples.iter().map(|sample| sample.1).sum::<f64>();
            assert!((area / (std::f64::consts::PI * radius * radius) - 1.0).abs() < 1e-11);
            for axis in 0..2 {
                let first = samples
                    .iter()
                    .map(|(point, weight)| point[axis] * weight)
                    .sum::<f64>();
                let second = samples
                    .iter()
                    .map(|(point, weight)| point[axis] * point[axis] * weight)
                    .sum::<f64>();
                assert!(first.abs() < 1e-10 * area * radius);
                assert!((second / (area * radius * radius * 0.25) - 1.0).abs() < 1e-10);
            }
            let half = disk_quadrature([0.0, 0.0], radius, [[0.0, radius], [-radius, radius]]);
            let half_area = half.iter().map(|sample| sample.1).sum::<f64>();
            let horizontal = half
                .iter()
                .map(|(point, weight)| point[0] * weight)
                .sum::<f64>()
                / half_area;
            assert!((half_area / area - 0.5).abs() < 1e-11);
            assert!((horizontal / radius - 4.0 / (3.0 * std::f64::consts::PI)).abs() < 1e-11);
        }
    }
    #[test]
    fn clipped_disk_integrals_survive_element_partitioning() {
        for center in [[0.0, 0.0], [0.05, 1.9], [1.5, 0.6], [2.0, 2.0]] {
            for radius in [0.01, 0.35, 1.3, 10.0] {
                let integrate = |samples: Vec<([f64; 2], f64)>| -> [f64; 6] {
                    let mut moments = [0.0; 6];
                    for (point, weight) in samples {
                        for (index, value) in [
                            1.0,
                            point[0],
                            point[1],
                            point[0] * point[0],
                            point[0] * point[1],
                            point[1] * point[1],
                        ]
                        .into_iter()
                        .enumerate()
                        {
                            moments[index] += weight * value;
                        }
                    }
                    moments
                };
                let whole = integrate(disk_quadrature(center, radius, [[0.0, 2.0]; 2]));
                let mut pieces = Vec::new();
                for horizontal in [[0.0, 0.3], [0.3, 1.1], [1.1, 2.0]] {
                    for vertical in [[0.0, 0.8], [0.8, 2.0]] {
                        pieces.extend(disk_quadrature(center, radius, [horizontal, vertical]));
                    }
                }
                let partitioned = integrate(pieces);
                for index in 0..6 {
                    assert!(
                        (whole[index] - partitioned[index]).abs() < 1e-9 * whole[0],
                        "center={center:?} radius={radius} moments={whole:?} vs {partitioned:?}"
                    );
                }
            }
        }
    }
    #[test]
    fn footprint_wire_and_persistence_reject_missing_invalid_or_implicit_selectors() {
        let footprint = ForceFootprint::Disk {
            center_mm: [3.0, 2.0, 1.0],
            diameter_mm: 0.4,
        };
        let wire: crate::rpc::ForceFootprint = footprint.clone().try_into().unwrap();
        assert_eq!(ForceFootprint::try_from(wire).unwrap(), footprint);
        assert!(ForceFootprint::try_from(crate::rpc::ForceFootprint { kind: None }).is_err());
        assert!(
            ForceFootprint::try_from(crate::rpc::ForceFootprint {
                kind: Some(crate::rpc::force_footprint::Kind::WholeFace(false))
            })
            .is_err()
        );
        for diameter in [0.0, -1.0, 0.001, f64::NAN, f64::INFINITY] {
            assert!(
                ForceFootprint::Disk {
                    center_mm: [0.0; 3],
                    diameter_mm: diameter
                }
                .validate()
                .is_err()
            );
        }
        assert!(
            serde_json::from_str::<ForceFootprint>(r#"{"kind":"whole_face","diameter_mm":4}"#)
                .is_err()
        );
        assert!(
            serde_json::from_str::<AnalysisFaceLoad>(
                r#"{"face_id":"step:10","force_n":[0,1,0],"moment_n_mm":[0,0,0]}"#
            )
            .is_err()
        );
    }
}
