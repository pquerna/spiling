// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use glam::{DQuat, DVec3};

#[derive(Clone, Copy, Debug)]
pub struct FocusTarget {
    pub bounds_mm: AabbMm,
    pub normal: Option<DVec3>,
}
impl FocusTarget {
    pub fn valid(&self) -> bool {
        self.bounds_mm.validate().is_ok()
            && self.normal.is_none_or(|normal| {
                normal.is_finite() && (normal.length_squared() - 1.0).abs() < 1e-6
            })
    }
    pub fn center(&self) -> DVec3 {
        DVec3::from_array(self.bounds_mm.min) * 0.5 + DVec3::from_array(self.bounds_mm.max) * 0.5
    }
}

impl Scene {
    pub fn focus_target(
        &self,
        face: Option<&FaceRef>,
        occurrence: Option<OccurrenceId>,
    ) -> Result<FocusTarget, &'static str> {
        let occurrence = self
            .occurrences
            .iter()
            .find(|item| Some(item.occurrence_id) == occurrence)
            .ok_or("Select a current part or face to focus")?;
        let definition = self
            .definition(&occurrence.definition_id)
            .ok_or("The selected part is not in the current display")?;
        let rotation = DQuat::from_array(occurrence.pose.rotation_xyzw);
        let translation = DVec3::from_array(occurrence.pose.translation_mm);
        let transform = |point| rotation * point + translation;
        let mut extent = Extent::default();
        let mut normal_sum = DVec3::ZERO;
        let mut area_sum = 0.0;
        if let Some(face) = face {
            if face.session_id != self.summary.session_id
                || face.scene_revision != self.summary.revision
                || face.occurrence_id != occurrence.occurrence_id
                || face.definition_id != occurrence.definition_id
            {
                return Err("The selected face is out of date; select it again");
            }
            let ordinal = definition
                .faces
                .iter()
                .position(|item| item.face_id == face.face_id)
                .ok_or("The selected face is not in the current display")?;
            for chunk in &definition.chunks {
                let view = spiling_contracts::display::validate_mesh_chunk(
                    &chunk.bytes,
                    &chunk.metadata,
                    &self.summary.session_id,
                    chunk.metadata.artifact_id,
                    &definition.record.definition_id,
                    chunk.metadata.chunk_index,
                )
                .map_err(|_| "The selected face display could not be checked")?;
                for triangle in 0..view.face_ordinals.len() {
                    if view.face_ordinals.get(triangle) != Some(ordinal as u32) {
                        continue;
                    }
                    let vertices: [DVec3; 3] = std::array::from_fn(|corner| {
                        let index = view.indices.get(triangle * 3 + corner).unwrap() as usize;
                        DVec3::from_array(view.positions.get(index).unwrap().map(f64::from))
                    });
                    let area = (vertices[1] - vertices[0])
                        .cross(vertices[2] - vertices[0])
                        .length();
                    for point in vertices {
                        extent.include(transform(point + DVec3::from_array(view.local_origin_mm)));
                    }
                    let mut normal = DVec3::ZERO;
                    for corner in 0..3 {
                        let index = view.indices.get(triangle * 3 + corner).unwrap() as usize;
                        normal +=
                            DVec3::from_array(view.normals.get(index).unwrap().map(f64::from));
                    }
                    if let Some(normal) = normal.try_normalize() {
                        normal_sum += normal * area;
                        area_sum += area;
                    }
                }
            }
        } else {
            let bounds = definition.record.bounds_mm;
            for corner in 0..8 {
                extent.include(transform(DVec3::from_array(std::array::from_fn(|axis| {
                    if corner & (1 << axis) == 0 {
                        bounds.min[axis]
                    } else {
                        bounds.max[axis]
                    }
                }))));
            }
        }
        let target = FocusTarget {
            bounds_mm: extent
                .bounds()
                .ok_or("No display geometry for this face; focus its part instead")?,
            normal: (normal_sum.length() > area_sum * 0.4)
                .then(|| normal_sum.try_normalize().map(|normal| rotation * normal))
                .flatten(),
        };
        target
            .valid()
            .then_some(target)
            .ok_or("The selected area cannot be framed safely")
    }
}

struct Extent {
    min: DVec3,
    max: DVec3,
}
impl Default for Extent {
    fn default() -> Self {
        Self {
            min: DVec3::splat(f64::INFINITY),
            max: DVec3::splat(f64::NEG_INFINITY),
        }
    }
}
impl Extent {
    fn include(&mut self, point: DVec3) {
        self.min = self.min.min(point);
        self.max = self.max.max(point);
    }
    fn bounds(&self) -> Option<AabbMm> {
        (self.min.is_finite() && self.max.is_finite()).then_some(AabbMm {
            min: self.min.to_array(),
            max: self.max.to_array(),
        })
    }
}
