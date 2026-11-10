// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use glam::{DQuat, DVec3};
use spiling_contracts::{analysis::fields::AnalysisFields, geometry::RigidPoseMm};
use std::collections::BTreeMap;

pub const DISPLAY_RESERVE_BYTES: usize = 32 * 1024 * 1024;
pub const FIELD_NAMES: [&str; 4] = [
    "Macro stress · MPa",
    "Layer-normal stress · MPa",
    "Displacement · mm",
    "Energy concentration · ×",
];

/// An explicit user choice, never a material/catalog-derived default.
pub fn reference_model()
-> Result<spiling_contracts::analysis::mechanics::AnalysisRunSettings, String> {
    let settings: spiling_contracts::analysis::mechanics::AnalysisRunSettings =
        serde_json::from_str(include_str!(
            "../../../fixtures/analysis/hook-assumptions.json"
        ))
        .map_err(|error| error.to_string())?;
    settings.validate().map_err(|error| error.to_string())?;
    Ok(settings)
}

pub fn read_model(
    path: &spiling_contracts::geometry::NativePath,
) -> Result<spiling_contracts::analysis::mechanics::AnalysisRunSettings, String> {
    use std::io::Read;
    let path = std::path::PathBuf::from(path.to_os_string().map_err(|error| error.to_string())?);
    if !std::fs::metadata(&path)
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("Analysis model must be a regular JSON file".into());
    }
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("Analysis model must be a regular JSON file".into());
    }
    let mut bytes = Vec::new();
    file.take(48 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > 48 * 1024 {
        return Err("Analysis model exceeds 48 KiB".into());
    }
    let settings: spiling_contracts::analysis::mechanics::AnalysisRunSettings =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    settings.validate().map_err(|error| error.to_string())?;
    Ok(settings)
}

#[derive(Debug)]
pub struct Surface {
    pub points: [DVec3; 9],
    pub element: usize,
}

#[derive(Debug)]
pub struct Display {
    pub contacts: Vec<(DVec3, Vec<DVec3>)>,
    pub surfaces: Vec<Surface>,
    pub values: Vec<[f64; 4]>,
    pub ranges: [[f64; 2]; 4],
}

fn insert_faces(
    faces: &mut BTreeMap<[u32; 9], Option<Surface>>,
    element_nodes: [u32; 27],
    element_index: usize,
    position: impl Fn(u32) -> DVec3,
) -> Result<(), String> {
    for axis in 0..3 {
        for side in [0, 2] {
            let nodes = std::array::from_fn(|index| {
                let mut coordinates = [0; 3];
                coordinates[axis] = side;
                coordinates[(axis + 1) % 3] = index % 3;
                coordinates[(axis + 2) % 3] = index / 3;
                element_nodes[coordinates[0] + 3 * coordinates[1] + 9 * coordinates[2]]
            });
            let mut key = nodes;
            key.sort_unstable();
            match faces.entry(key) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(Some(Surface {
                        points: nodes.map(&position),
                        element: element_index,
                    }));
                }
                std::collections::btree_map::Entry::Occupied(mut entry) => {
                    if entry.get().is_none() {
                        return Err("Non-manifold mechanical display face".into());
                    }
                    entry.insert(None);
                }
            }
        }
    }
    Ok(())
}

fn load_contacts(
    load_count: usize,
    patch_count: usize,
    sample_at: impl Fn(usize) -> Option<spiling_contracts::analysis::fields::AnalysisPatchSample>,
) -> Result<Vec<(DVec3, Vec<DVec3>)>, String> {
    let mut contacts = vec![(DVec3::ZERO, Vec::new()); load_count];
    let mut areas = vec![0.0; contacts.len()];
    let mut counts = vec![0_usize; contacts.len()];
    for index in 0..patch_count {
        let sample = sample_at(index).ok_or("Missing contact sample")?;
        if let Some(count) = counts.get_mut(sample.patch_index as usize) {
            *count += 1;
        }
    }
    let mut seen = vec![0_usize; contacts.len()];
    for index in 0..patch_count {
        let sample = sample_at(index).ok_or("Missing contact sample")?;
        // SPLA2 retains connector reaction samples after the load patches.
        // They remain in the physical packet, never in load-contact averages.
        let Some(contact) = contacts.get_mut(sample.patch_index as usize) else {
            continue;
        };
        let point = DVec3::from_array(sample.point_mm);
        contact.0 += point * sample.area_mm2;
        if seen[sample.patch_index as usize]
            .is_multiple_of(counts[sample.patch_index as usize].div_ceil(128))
        {
            contact.1.push(point);
        }
        seen[sample.patch_index as usize] += 1;
        areas[sample.patch_index as usize] += sample.area_mm2;
    }
    for (contact, area) in contacts.iter_mut().zip(areas) {
        if area <= 0.0 {
            return Err("Mechanical contact has no retained area".into());
        }
        contact.0 /= area;
    }
    Ok(contacts)
}

impl Display {
    pub fn new(fields: &AnalysisFields<'_>) -> Result<Self, String> {
        let mut faces = BTreeMap::<[u32; 9], Option<Surface>>::new();
        let mut values = Vec::with_capacity(fields.element_count);
        let mut ranges = [[f64::INFINITY, f64::NEG_INFINITY]; 4];
        let rotation = DQuat::from_array(fields.summary.pose.rotation_xyzw);
        let layer_axis = (0..3)
            .max_by(|first, second| {
                (rotation * DVec3::AXES[*first])
                    .z
                    .abs()
                    .total_cmp(&(rotation * DVec3::AXES[*second]).z.abs())
            })
            .unwrap();
        for element_index in 0..fields.element_count {
            let element = fields.element(element_index).ok_or("Missing field cell")?;
            let stress = element.stress_mpa;
            let displacement = fields
                .node(element.nodes[13] as usize)
                .ok_or("Missing field node")?;
            let cell_values = [
                (0.5 * ((stress[0] - stress[1]).powi(2)
                    + (stress[1] - stress[2]).powi(2)
                    + (stress[2] - stress[0]).powi(2))
                    + 3.0 * stress[3..].iter().map(|value| value * value).sum::<f64>())
                .sqrt(),
                stress[layer_axis],
                DVec3::from_array(displacement.displacement_mm).length(),
                element.concentration,
            ];
            for (range, value) in ranges.iter_mut().zip(cell_values) {
                range[0] = range[0].min(value);
                range[1] = range[1].max(value);
            }
            values.push(cell_values);
            insert_faces(&mut faces, element.nodes, element_index, |node| {
                DVec3::from_array(fields.node(node as usize).unwrap().position_mm)
            })?;
        }
        let contacts = load_contacts(
            fields.summary.case.case.loads.len(),
            fields.patch_count,
            |index| fields.patch(index),
        )?;
        Ok(Self {
            contacts,
            surfaces: faces.into_values().flatten().collect(),
            values,
            ranges,
        })
    }

    pub fn vertices(
        &self,
        field: usize,
        pose: &RigidPoseMm,
        origin: DVec3,
    ) -> Result<Vec<Vertex>, String> {
        let field = field.min(3);
        let rotation = DQuat::from_array(pose.rotation_xyzw);
        let translation = DVec3::from_array(pose.translation_mm);
        let vertex_count = self
            .surfaces
            .len()
            .checked_mul(24)
            .ok_or("Mechanical display size overflow")?;
        if vertex_count
            .checked_mul(size_of::<Vertex>() * 2)
            .is_none_or(|bytes| bytes > DISPLAY_RESERVE_BYTES / 2)
        {
            return Err("Mechanical display exceeds reserved CPU/GPU budget".into());
        }
        let mut vertices = Vec::with_capacity(vertex_count);
        for surface in &self.surfaces {
            let color = color(self.values[surface.element][field], self.ranges[field]);
            let mut points = [[0.0_f32; 3]; 9];
            for (output, point) in points.iter_mut().zip(surface.points) {
                let local = rotation * point + translation - origin;
                let projected = local.as_vec3();
                if !projected.is_finite() || (local - projected.as_dvec3()).length() > 0.001 {
                    return Err("Mechanical display exceeds 0.001 mm projection precision".into());
                }
                *output = projected.to_array();
            }
            for row in 0..2 {
                for column in 0..2 {
                    let corner = 3 * row + column;
                    for index in [
                        corner,
                        corner + 1,
                        corner + 4,
                        corner,
                        corner + 4,
                        corner + 3,
                    ] {
                        vertices.push(Vertex {
                            position: points[index],
                            color,
                        });
                    }
                }
            }
        }
        Ok(vertices)
    }

    pub fn probe(&self, origin: DVec3, direction: DVec3) -> Option<usize> {
        let mut nearest = f64::INFINITY;
        let mut result = None;
        for surface in &self.surfaces {
            for row in 0..2 {
                for column in 0..2 {
                    let corner = 3 * row + column;
                    for indices in [
                        [corner, corner + 1, corner + 4],
                        [corner, corner + 4, corner + 3],
                    ] {
                        let [first, second, third] = indices.map(|index| surface.points[index]);
                        let edge = second - first;
                        let cross = direction.cross(third - first);
                        let determinant = edge.dot(cross);
                        if determinant.abs() < 1e-14 {
                            continue;
                        }
                        let relative = origin - first;
                        let along = relative.dot(cross) / determinant;
                        let across = direction.dot(relative.cross(edge)) / determinant;
                        let distance = (third - first).dot(relative.cross(edge)) / determinant;
                        if along >= 0.0
                            && across >= 0.0
                            && along + across <= 1.0
                            && distance > 0.0
                            && distance < nearest
                        {
                            nearest = distance;
                            result = Some(surface.element);
                        }
                    }
                }
            }
        }
        result
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub color: [f32; 3],
}

pub fn color(value: f64, range: [f64; 2]) -> [f32; 3] {
    let fraction = if range[1] > range[0] {
        ((value - range[0]) / (range[1] - range[0])).clamp(0.0, 1.0) as f32
    } else {
        0.5
    };
    let stops = [
        [0.13, 0.35, 0.73],
        [0.10, 0.69, 0.76],
        [0.96, 0.83, 0.30],
        [0.89, 0.21, 0.12],
    ];
    let segment = (fraction * 3.0).floor().min(2.0) as usize;
    let blend = fraction * 3.0 - segment as f32;
    std::array::from_fn(|axis| {
        stops[segment][axis] * (1.0 - blend) + stops[segment + 1][axis] * blend
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fastener_reaction_samples_do_not_index_or_bias_load_contacts() {
        use spiling_contracts::analysis::fields::AnalysisPatchSample;
        let samples = [
            AnalysisPatchSample {
                element_index: 0,
                patch_index: 0,
                point_mm: [1.0, 2.0, 3.0],
                area_mm2: 2.0,
                traction_mpa: [0.0; 3],
            },
            AnalysisPatchSample {
                element_index: 1,
                patch_index: 1,
                point_mm: [100.0; 3],
                area_mm2: 1000.0,
                traction_mpa: [-10.0; 3],
            },
            AnalysisPatchSample {
                element_index: 0,
                patch_index: 0,
                point_mm: [3.0, 2.0, 3.0],
                area_mm2: 2.0,
                traction_mpa: [0.0; 3],
            },
        ];
        let contacts =
            load_contacts(1, samples.len(), |index| samples.get(index).cloned()).unwrap();
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].0, DVec3::new(2.0, 2.0, 3.0));
        assert_eq!(contacts[0].1.len(), 2);
        assert_eq!(samples[1].traction_mpa, [-10.0; 3]);
    }
    #[test]
    fn shared_reserve_covers_retained_pending_and_renderer_field_data() {
        use spiling_contracts::analysis::mechanics::{
            MAX_MECHANICAL_ELEMENTS, MAX_MECHANICAL_FIELD_BYTES,
        };
        let faces = MAX_MECHANICAL_ELEMENTS * 6;
        let capture = MAX_MECHANICAL_FIELD_BYTES
            + spiling_contracts::analysis::MAX_ANALYSIS_PATCHES * 128 * size_of::<DVec3>()
            + faces * size_of::<Surface>()
            + MAX_MECHANICAL_ELEMENTS * size_of::<[f64; 4]>();
        let renderer = faces * 24 * size_of::<Vertex>() * 3;
        let construction = faces * (size_of::<Surface>() + size_of::<[u32; 9]>() + 128);
        let annotations =
            4096 * 3 * (2 * size_of::<[DVec3; 2]>() + size_of::<[[u64; 3]; 2]>() + 128);
        assert!(capture * 3 + renderer + construction + annotations < DISPLAY_RESERVE_BYTES);
    }
    #[test]
    fn bounded_scale_includes_equal_and_signed_ranges() {
        assert_eq!(color(-10.0, [-10.0, 10.0]), color(0.0, [0.0, 1.0]));
        assert_eq!(color(10.0, [-10.0, 10.0]), color(1.0, [0.0, 1.0]));
        assert!(
            color(1.0, [1.0, 1.0])
                .iter()
                .all(|component| component.is_finite())
        );
    }

    fn pair() -> Display {
        let mut faces = BTreeMap::new();
        for offset in [0, 2] {
            let nodes = std::array::from_fn(|index| {
                offset + (index % 3) as u32 + 5 * ((index / 3) % 3) as u32 + 15 * (index / 9) as u32
            });
            insert_faces(&mut faces, nodes, offset as usize / 2, |node| {
                DVec3::new(
                    f64::from(node % 5),
                    f64::from((node / 5) % 3),
                    f64::from(node / 15),
                )
            })
            .unwrap();
        }
        Display {
            contacts: vec![],
            surfaces: faces.into_values().flatten().collect(),
            values: vec![[1.0; 4], [2.0; 4]],
            ranges: [[1.0, 2.0]; 4],
        }
    }

    #[test]
    fn exterior_cells_remove_shared_region_faces_and_probe_only_material() {
        let display = pair();
        assert_eq!(display.surfaces.len(), 10);
        assert_eq!(
            display.probe(DVec3::new(1.0, 1.0, 10.0), -DVec3::Z),
            Some(0)
        );
        assert_eq!(
            display.probe(DVec3::new(3.0, 1.0, 10.0), -DVec3::Z),
            Some(1)
        );
        assert_eq!(display.probe(DVec3::new(5.0, 1.0, 10.0), -DVec3::Z), None);
        let vertices = display
            .vertices(0, &RigidPoseMm::IDENTITY, DVec3::ZERO)
            .unwrap();
        assert_eq!(vertices.len(), 240);
        assert_eq!(
            vertices[0].color,
            color(display.values[display.surfaces[0].element][0], [1.0, 2.0])
        );
    }

    #[test]
    fn rotation_and_large_translation_are_rebased_before_float_projection() {
        let display = pair();
        let rotation = DQuat::from_rotation_y(std::f64::consts::FRAC_PI_2);
        let translation = DVec3::new(1e6, -1e6, 1e6);
        let pose = RigidPoseMm {
            rotation_xyzw: rotation.to_array(),
            translation_mm: translation.to_array(),
        };
        let vertices = display.vertices(0, &pose, translation).unwrap();
        let expected = rotation * display.surfaces[0].points[0];
        assert!(
            (DVec3::from_array(vertices[0].position.map(f64::from)) - expected).length() < 1e-6
        );
        assert!(display.vertices(0, &pose, DVec3::splat(1e20)).is_err());
    }

    #[test]
    fn model_file_is_strict_bounded_and_recipe_compatible() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("model.json");
        let native =
            spiling_contracts::geometry::NativePath::from_os_str(path.as_os_str()).unwrap();
        let original = include_bytes!("../../../fixtures/analysis/hook-assumptions.json");
        std::fs::write(&path, original).unwrap();
        assert!(read_model(&native).is_ok());
        let mut model: serde_json::Value = serde_json::from_slice(original).unwrap();
        model["recipe"]["infill_fraction"] = 0.6.into();
        std::fs::write(&path, serde_json::to_vec(&model).unwrap()).unwrap();
        assert!(read_model(&native).is_err());
        std::fs::write(&path, b"{\"unknown\":true}").unwrap();
        assert!(read_model(&native).is_err());
        std::fs::write(&path, vec![b' '; 48 * 1024 + 1]).unwrap();
        assert!(read_model(&native).is_err());
    }
}
