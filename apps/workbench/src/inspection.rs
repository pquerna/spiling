// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::{replay::Vertex, scene::FocusTarget};
use glam::DVec3;
use spiling_contracts::{geometry::AabbMm, manufacturing::*};

pub const MAX_INSPECTION_VERTICES: usize = 4096;
pub const INSPECTION_RESERVE_BYTES: usize = MAX_INSPECTION_VERTICES * size_of::<Vertex>() * 4;

#[derive(Debug)]
pub struct Area {
    pub(crate) vertices: Vec<Vertex>,
    pub(crate) target: FocusTarget,
}
impl Area {
    pub fn separator(
        value: &ProcessInterfaceCoverage,
        physical_z_mm: [f64; 2],
        origin: DVec3,
    ) -> Result<Self, String> {
        if !physical_z_mm.iter().all(|value| value.is_finite())
            || physical_z_mm[0] >= physical_z_mm[1]
            || !value.thickness_mm.is_finite()
            || value.thickness_mm <= 0.0
        {
            return Err(
                "Separator inspection needs a finite physical interval and positive thickness"
                    .into(),
            );
        }
        let normal = match value.axis {
            0 => DVec3::X,
            1 => DVec3::Y,
            _ => return Err("Unsupported separator axis".into()),
        };
        let mut builder = Builder::new(origin, normal);
        for line in &value.centerlines_mm {
            let point = |index: usize, upper: bool| {
                DVec3::new(
                    line[index][0],
                    line[index][1],
                    physical_z_mm[usize::from(upper)],
                )
            };
            builder.segment(point(0, false), point(1, false))?;
            builder.segment(point(0, true), point(1, true))?;
            builder.segment(point(0, false), point(0, true))?;
            builder.segment(point(1, false), point(1, true))?;
        }
        let mut area = builder.finish()?;
        for axis in 0..2 {
            area.target.bounds_mm.min[axis] -= value.thickness_mm * 0.5;
            area.target.bounds_mm.max[axis] += value.thickness_mm * 0.5;
        }
        if !area.target.valid() {
            return Err("Separator bounds cannot be displayed safely".into());
        }
        Ok(area)
    }
    pub fn plate(value: &ProcessDiaphragmCoverage, origin: DVec3) -> Result<Self, String> {
        if !value.physical_z_mm.iter().all(|value| value.is_finite())
            || value.physical_z_mm[0] >= value.physical_z_mm[1]
        {
            return Err("Plate inspection needs a finite physical interval".into());
        }
        let mut builder = Builder::new(origin, DVec3::Z);
        for polygon in &value.extent {
            for ring in std::iter::once(&polygon.outer).chain(&polygon.holes) {
                for edge in ring.windows(2) {
                    let point = |index: usize, upper: bool| {
                        DVec3::new(
                            edge[index][0],
                            edge[index][1],
                            value.physical_z_mm[usize::from(upper)],
                        )
                    };
                    builder.segment(point(0, false), point(1, false))?;
                    builder.segment(point(0, true), point(1, true))?;
                    builder.segment(point(0, false), point(0, true))?;
                }
            }
        }
        builder.finish()
    }
}

struct Builder {
    origin: DVec3,
    min: DVec3,
    max: DVec3,
    normal: DVec3,
    vertices: Vec<Vertex>,
}
impl Builder {
    fn new(origin: DVec3, normal: DVec3) -> Self {
        Self {
            origin,
            min: DVec3::splat(f64::INFINITY),
            max: DVec3::splat(f64::NEG_INFINITY),
            normal,
            vertices: Vec::with_capacity(MAX_INSPECTION_VERTICES),
        }
    }
    fn segment(&mut self, start: DVec3, end: DVec3) -> Result<(), String> {
        if self.vertices.len() + 2 > MAX_INSPECTION_VERTICES {
            return Err("Construction outline exceeds the bounded inspection budget; use ordinary layer review".into());
        }
        for point in [start, end] {
            let rebased = point - self.origin;
            if !point.is_finite()
                || !rebased.is_finite()
                || (rebased.as_vec3().as_dvec3() - rebased).length() > 0.001
            {
                return Err("Construction outline exceeds 0.001 mm display precision".into());
            }
            self.min = self.min.min(point);
            self.max = self.max.max(point);
            self.vertices.push(Vertex {
                position: rebased.as_vec3().to_array(),
                color: [0.15, 0.85, 1.0],
            });
        }
        Ok(())
    }
    fn finish(self) -> Result<Area, String> {
        let target = FocusTarget {
            bounds_mm: AabbMm {
                min: self.min.to_array(),
                max: self.max.to_array(),
            },
            normal: Some(self.normal),
        };
        if self.vertices.is_empty() || !target.valid() {
            return Err("No finite compiled construction outline is available".into());
        }
        Ok(Area {
            vertices: self.vertices,
            target,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rebases_before_rounding_and_never_publishes_a_partial_outline() {
        let origin = DVec3::splat(1e9);
        let mut builder = Builder::new(origin, DVec3::Z);
        builder.segment(origin, origin + DVec3::X * 0.125).unwrap();
        let area = builder.finish().unwrap();
        assert_eq!(area.vertices[1].position, [0.125, 0.0, 0.0]);
        assert_eq!(area.target.bounds_mm.min, origin.to_array());
        let mut builder = Builder::new(DVec3::ZERO, DVec3::Z);
        assert!(builder.segment(origin + DVec3::X * 0.1, origin).is_err());
        let mut builder = Builder::new(DVec3::ZERO, DVec3::Z);
        for _ in 0..MAX_INSPECTION_VERTICES / 2 {
            builder.segment(DVec3::ZERO, DVec3::X).unwrap();
        }
        assert!(builder.segment(DVec3::ZERO, DVec3::Y).is_err());
    }
    #[test]
    fn empty_and_nonfinite_outlines_refuse() {
        assert!(Builder::new(DVec3::ZERO, DVec3::Z).finish().is_err());
        assert!(
            Builder::new(DVec3::ZERO, DVec3::Z)
                .segment(DVec3::splat(f64::NAN), DVec3::ZERO)
                .is_err()
        );
    }
    #[test]
    fn plate_outline_preserves_holes_and_the_complete_physical_height() {
        use spiling_contracts::{geometry::*, process::*};
        let provenance = ProcessProvenance {
            layer_id: ProcessLayerId::new(1).unwrap(),
            operation_id: ProcessOperationId::new(1).unwrap(),
        };
        let plate = ProcessDiaphragmCoverage {
            occurrence_id: OccurrenceId::new(1).unwrap(),
            definition_id: DefinitionId::from_source_sha256(&[1; 32]),
            provenance,
            source_plane_mm: 2.0,
            requested_thickness_mm: 0.3,
            physical_z_mm: [2.0, 2.3],
            neighbors: [
                ResolvedProcessValue {
                    fraction: 0.2,
                    provenance: Some(provenance),
                },
                ResolvedProcessValue {
                    fraction: 0.6,
                    provenance: Some(provenance),
                },
            ],
            region_index: 0,
            area_mm2: 84.0,
            extent: vec![DiaphragmPolygonMm {
                outer: vec![
                    [0.0, 0.0],
                    [10.0, 0.0],
                    [10.0, 10.0],
                    [0.0, 10.0],
                    [0.0, 0.0],
                ],
                holes: vec![vec![
                    [3.0, 3.0],
                    [3.0, 7.0],
                    [7.0, 7.0],
                    [7.0, 3.0],
                    [3.0, 3.0],
                ]],
            }],
        };
        let area = Area::plate(&plate, DVec3::ZERO).unwrap();
        assert_eq!(area.vertices.len(), 48);
        assert_eq!(area.target.bounds_mm.min, [0.0, 0.0, 2.0]);
        assert_eq!(area.target.bounds_mm.max, [10.0, 10.0, 2.3]);
        assert!(
            area.vertices
                .iter()
                .any(|vertex| vertex.position == [3.0, 3.0, 2.0])
        );
        assert_eq!(area.target.normal, Some(DVec3::Z));
    }
}
