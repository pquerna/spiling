// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::{replay::Vertex, scene::FocusTarget};
use glam::DVec3;
use spiling_contracts::{geometry::AabbMm, manufacturing::*};

pub const MAX_INSPECTION_VERTICES: usize = 4096;
pub const INSPECTION_RESERVE_BYTES: usize = MAX_INSPECTION_VERTICES * size_of::<Vertex>() * 4;
const MAX_INSPECTION_WORK: usize = 262_144;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FunctionalArea {
    Region,
    ProtectedBase,
    InheritedBase,
}
impl FunctionalArea {
    pub fn label(self) -> &'static str {
        match self {
            Self::Region => "Region · actual region command centerlines",
            Self::ProtectedBase => {
                "Protected Base · base command centerlines inside the captured solid zone"
            }
            Self::InheritedBase => {
                "Inherited Base · base command centerlines outside the captured solid zone"
            }
        }
    }
    fn color(self) -> [f32; 3] {
        match self {
            Self::Region => [0.9, 0.45, 1.0],
            Self::ProtectedBase => [1.0, 0.7, 0.2],
            Self::InheritedBase => [0.3, 0.9, 0.55],
        }
    }
}

#[derive(Debug)]
pub struct Area {
    pub(crate) vertices: Vec<Vertex>,
    pub(crate) target: FocusTarget,
}
impl Area {
    /// An import finding marks bounds, not a recovered face or proof region.
    pub fn import_location(
        bounds: AabbMm,
        pose: spiling_contracts::geometry::RigidPoseMm,
        scene_bounds: Option<AabbMm>,
    ) -> Result<Self, String> {
        bounds.validate().map_err(|e| e.to_string())?;
        pose.validate().map_err(|e| e.to_string())?;
        let scene = scene_bounds.ok_or("No scene frame for import location")?;
        let origin = DVec3::from_array(scene.min) * 0.5 + DVec3::from_array(scene.max) * 0.5;
        let rotation = glam::DQuat::from_array(pose.rotation_xyzw);
        let translation = DVec3::from_array(pose.translation_mm);
        let corners: Vec<_> = (0..8)
            .map(|i| {
                let p = DVec3::from_array(std::array::from_fn(|a| {
                    let lo = bounds.min[a];
                    let hi = bounds.max[a];
                    // Give point/line findings a visible marker, without changing
                    // the retained issue bounds or any material geometry.
                    let pad = if hi == lo { 0.25 } else { 0. };
                    if i & (1 << a) == 0 {
                        lo - pad
                    } else {
                        hi + pad
                    }
                }));
                rotation * p + translation
            })
            .collect();
        let mut vertices = Vec::with_capacity(24);
        let mut min = DVec3::splat(f64::INFINITY);
        let mut max = DVec3::splat(f64::NEG_INFINITY);
        for i in 0..8 {
            for a in 0..3 {
                if i & (1 << a) != 0 {
                    continue;
                }
                for p in [corners[i], corners[i | (1 << a)]] {
                    let relative = p - origin;
                    if !relative.is_finite()
                        || (relative.as_vec3().as_dvec3() - relative).length() > 0.001
                    {
                        return Err("Import location exceeds display coordinate precision".into());
                    }
                    min = min.min(p);
                    max = max.max(p);
                    vertices.push(Vertex {
                        position: relative.as_vec3().to_array(),
                        color: [1., 0.65, 0.12],
                    });
                }
            }
        }
        let target = FocusTarget {
            bounds_mm: AabbMm::new(min.to_array(), max.to_array()).map_err(|e| e.to_string())?,
            normal: None,
        };
        Ok(Self { vertices, target })
    }
    /// Display-only subsets of XYZ6 strokes bound by the caller to fresh actual
    /// replay. Clipping does not construct material or a new machine command.
    pub fn functional_commands(
        paths: &[DepositionPath],
        physical_z_mm: [f64; 2],
        frame: &FunctionalMaterialFrame,
        scope: FunctionalArea,
        origin: DVec3,
    ) -> Result<Self, String> {
        let zone = functional_zone_bounds(frame, physical_z_mm)?;
        if scope == FunctionalArea::ProtectedBase && zone.is_none() {
            return Err("No captured solid base zone is active on this physical interval".into());
        }
        let role = if scope == FunctionalArea::Region {
            MaterialConstructionRole::FunctionalRegion
        } else {
            MaterialConstructionRole::FunctionalBase
        };
        let reference = MaterialDomainRef {
            construction_index: 0,
            role,
        };
        let mut builder = Builder::new(origin, DVec3::from_array(frame.outward_normal));
        let mut work = paths.len();
        for path in paths {
            work = work
                .checked_add(path.points_mm.len())
                .ok_or("Command inspection work overflow")?;
            if work > MAX_INSPECTION_WORK {
                return Err("Command centerlines exceed the bounded inspection work budget".into());
            }
            if path.material_domain != Some(reference) || path.physical_z_mm != physical_z_mm {
                continue;
            }
            for edge in path.points_mm.windows(2) {
                let start = DVec3::from_array(
                    program_coordinates::project_xyz6_point_mm(edge[0])
                        .map_err(|error| error.to_string())?,
                );
                let end = DVec3::from_array(
                    program_coordinates::project_xyz6_point_mm(edge[1])
                        .map_err(|error| error.to_string())?,
                );
                if start == end {
                    continue;
                }
                let clipped = if scope == FunctionalArea::Region {
                    None
                } else {
                    zone.map(|bounds| segment_aabb_interval(start, end, bounds))
                        .transpose()?
                        .flatten()
                };
                match (scope, clipped) {
                    (FunctionalArea::ProtectedBase, Some([low, high])) => {
                        builder.segment(start.lerp(end, low), start.lerp(end, high))?
                    }
                    (FunctionalArea::ProtectedBase, None) => {}
                    (FunctionalArea::InheritedBase, Some([low, high])) => {
                        if low > 0.0 {
                            builder.segment(start, start.lerp(end, low))?;
                        }
                        if high < 1.0 {
                            builder.segment(start.lerp(end, high), end)?;
                        }
                    }
                    _ => builder.segment(start, end)?,
                }
            }
        }
        if builder.vertices.is_empty() {
            return Err(
                "No actual command centerlines match this material and physical interval scope"
                    .into(),
            );
        }
        let mut area = builder.finish()?;
        for vertex in &mut area.vertices {
            vertex.color = scope.color();
        }
        Ok(area)
    }
    pub fn functional_join(
        value: &ResolvedMaterialDomain,
        base: &ResolvedMaterialDomain,
        stock: AabbMm,
        origin: DVec3,
    ) -> Result<Self, String> {
        value.validate().map_err(|error| error.to_string())?;
        base.validate().map_err(|error| error.to_string())?;
        stock.validate().map_err(|error| error.to_string())?;
        if value.reference.role != MaterialConstructionRole::FunctionalRegion
            || base.reference.role != MaterialConstructionRole::FunctionalBase
            || value.reference.construction_index != base.reference.construction_index
            || value.physical_z_mm != base.physical_z_mm
        {
            return Err("Join inspection requires both matching realized material owners".into());
        }
        let rings = |domain: &ResolvedMaterialDomain| {
            domain
                .polygons
                .iter()
                .flat_map(|polygon| std::iter::once(&polygon.exterior_mm).chain(&polygon.holes_mm))
                .map(|ring| ring.len().saturating_sub(1))
                .sum::<usize>()
        };
        if rings(value)
            .checked_mul(rings(base))
            .is_none_or(|work| work > MAX_INSPECTION_WORK)
        {
            return Err("Shared join outline exceeds the bounded inspection work budget".into());
        }
        let mut builder = Builder::new(origin, DVec3::Z);
        for polygon in &value.polygons {
            for ring in std::iter::once(&polygon.exterior_mm).chain(&polygon.holes_mm) {
                for edge in ring.windows(2) {
                    if (0..2).any(|axis| {
                        [stock.min[axis], stock.max[axis]]
                            .into_iter()
                            .any(|boundary| edge[0][axis] == boundary && edge[1][axis] == boundary)
                    }) {
                        continue;
                    }
                    let shared = base.polygons.iter().any(|polygon| {
                        std::iter::once(&polygon.exterior_mm)
                            .chain(&polygon.holes_mm)
                            .any(|ring| {
                                ring.windows(2).any(|other| {
                                    (edge[0] == other[0] && edge[1] == other[1])
                                        || (edge[0] == other[1] && edge[1] == other[0])
                                })
                            })
                    });
                    if !shared {
                        return Err(
                            "Realized owners do not expose an exact complete shared join outline"
                                .into(),
                        );
                    }
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

/// The full face U extent is captured stock width, not the authored patch U.
/// A crossing physical interval refuses rather than borrowing a nearby zone.
pub fn functional_zone_bounds(
    frame: &FunctionalMaterialFrame,
    physical: [f64; 2],
) -> Result<Option<AabbMm>, String> {
    frame.validate_values().map_err(|error| error.to_string())?;
    if !physical.iter().all(|height| height.is_finite())
        || physical[0] >= physical[1]
        || physical[0] < frame.stock_bounds_mm.min[2]
        || physical[1] > frame.stock_bounds_mm.max[2]
    {
        return Err("Command inspection requires a complete current physical interval".into());
    }
    let Some(zone) = frame
        .solid_base_zones
        .iter()
        .find(|zone| zone.physical_z_mm[0] < physical[1] && physical[0] < zone.physical_z_mm[1])
    else {
        return Ok(None);
    };
    if zone.physical_z_mm[0] > physical[0] || physical[1] > zone.physical_z_mm[1] {
        return Err("Physical interval crosses a captured solid base zone boundary".into());
    }
    let axis = usize::from(frame.outward_normal[1] != 0.0);
    let depth = zone
        .inward_depth_mm
        .map(|depth| frame.world_origin_mm[axis] - frame.outward_normal[axis] * depth);
    let mut bounds = frame.stock_bounds_mm;
    bounds.min[axis] = depth[0].min(depth[1]);
    bounds.max[axis] = depth[0].max(depth[1]);
    bounds.min[2] = program_coordinates::project_xyz6_coordinate_mm(physical[0])
        .map_err(|error| error.to_string())?;
    bounds.max[2] = program_coordinates::project_xyz6_coordinate_mm(physical[1])
        .map_err(|error| error.to_string())?;
    bounds.validate().map_err(|error| error.to_string())?;
    Ok(Some(bounds))
}

/// Finite parametric segment/slab clipping for display only. Tangency has no
/// positive-length protected subsegment; the complement retains that stroke.
fn segment_aabb_interval(
    start: DVec3,
    end: DVec3,
    bounds: AabbMm,
) -> Result<Option<[f64; 2]>, String> {
    bounds.validate().map_err(|error| error.to_string())?;
    let direction = end - start;
    if !start.is_finite() || !end.is_finite() || !direction.is_finite() {
        return Err("Nonfinite command centerline cannot be clipped for display".into());
    }
    let mut low: f64 = 0.0;
    let mut high: f64 = 1.0;
    for axis in 0..3 {
        if direction[axis] == 0.0 {
            if start[axis] < bounds.min[axis] || start[axis] > bounds.max[axis] {
                return Ok(None);
            }
            continue;
        }
        let lower_cut = (bounds.min[axis] - start[axis]) / direction[axis];
        let upper_cut = (bounds.max[axis] - start[axis]) / direction[axis];
        if !lower_cut.is_finite() || !upper_cut.is_finite() {
            return Err("Command clipping exceeds finite display arithmetic".into());
        }
        low = low.max(lower_cut.min(upper_cut));
        high = high.min(lower_cut.max(upper_cut));
        if low >= high {
            return Ok(None);
        }
    }
    Ok(Some([low, high]))
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
    fn functional_frame() -> FunctionalMaterialFrame {
        use spiling_contracts::geometry::*;
        FunctionalMaterialFrame {
            occurrence_id: OccurrenceId::new(1).unwrap(),
            definition_id: DefinitionId::from_source_sha256(&[1; 32]),
            source_hash: SourceHash::from_digest(&[1; 32]),
            face_id: FaceId::from_step_entity(1).unwrap(),
            origin_mm: [0.0; 3],
            world_origin_mm: [0.0; 3],
            stock_bounds_mm: AabbMm::new([0.0; 3], [20.0; 3]).unwrap(),
            horizontal_direction: [1.0, 0.0, 0.0],
            outward_normal: [0.0, -1.0, 0.0],
            width_mm: 20.0,
            source_z_mm: [0.0, 20.0],
            selected_u_mm: [6.0, 14.0],
            region_z_mm: [5.0, 15.0],
            key_middle_z_mm: [6.0, 14.0],
            key_centers_u_mm: vec![10.0],
            solid_base_zones: vec![FunctionalBaseSolidZone {
                kind: FunctionalBaseSolidZoneKind::Root,
                inward_depth_mm: [3.0, 8.0],
                physical_z_mm: [5.0, 15.0],
            }],
        }
    }
    fn command_path(role: MaterialConstructionRole, physical: [f64; 2]) -> DepositionPath {
        DepositionPath {
            material_domain: Some(MaterialDomainRef {
                construction_index: 0,
                role,
            }),
            kind: DepositionKind::SolidFill,
            owner: None,
            width: WidthProfile::Constant { width_mm: 0.45 },
            physical_z_mm: physical,
            points_mm: vec![
                [2.0000004, 1.0, physical[1]],
                [2.0000004, 11.0, physical[1]],
            ],
            arcs: Vec::new(),
        }
    }
    #[test]
    fn commands_clip_and_complement_the_same_full_u_zone_after_xyz6_projection() {
        let frame = functional_frame();
        let physical = [8.0, 8.2];
        let path = command_path(MaterialConstructionRole::FunctionalBase, physical);
        let protected = Area::functional_commands(
            std::slice::from_ref(&path),
            physical,
            &frame,
            FunctionalArea::ProtectedBase,
            DVec3::ZERO,
        )
        .unwrap();
        let inherited = Area::functional_commands(
            std::slice::from_ref(&path),
            physical,
            &frame,
            FunctionalArea::InheritedBase,
            DVec3::ZERO,
        )
        .unwrap();
        let positions = |area: &Area| {
            area.vertices
                .iter()
                .map(|vertex| vertex.position)
                .collect::<Vec<_>>()
        };
        assert_eq!(positions(&protected), [[2.0, 3.0, 8.2], [2.0, 8.0, 8.2]]);
        assert_eq!(
            positions(&inherited),
            [
                [2.0, 1.0, 8.2],
                [2.0, 3.0, 8.2],
                [2.0, 8.0, 8.2],
                [2.0, 11.0, 8.2]
            ]
        );
        assert_ne!(protected.vertices[0].color, inherited.vertices[0].color);
        let bounds = functional_zone_bounds(&frame, physical).unwrap().unwrap();
        let start = DVec3::new(2.0, 1.0, 8.2);
        let end = DVec3::new(2.0, 11.0, 8.2);
        assert_eq!(
            segment_aabb_interval(start, end, bounds).unwrap(),
            Some([0.2, 0.7])
        );
        assert_eq!(
            segment_aabb_interval(end, start, bounds).unwrap(),
            Some([0.3, 0.8])
        );
        assert_eq!(
            segment_aabb_interval(
                DVec3::new(1.0, 3.0, 8.2),
                DVec3::new(19.0, 3.0, 8.2),
                bounds
            )
            .unwrap(),
            Some([0.0, 1.0])
        );
        assert_eq!(
            segment_aabb_interval(
                DVec3::new(1.0, 2.0, 8.2),
                DVec3::new(19.0, 2.0, 8.2),
                bounds
            )
            .unwrap(),
            None
        );
        assert_eq!(
            segment_aabb_interval(
                DVec3::new(-1.0, 4.0, 8.2),
                DVec3::new(1.0, 2.0, 8.2),
                bounds
            )
            .unwrap(),
            None
        );
    }
    #[test]
    fn command_selection_excludes_foreign_material_construction_and_physical_intervals() {
        let frame = functional_frame();
        let physical = [8.0, 8.2];
        let region = command_path(MaterialConstructionRole::FunctionalRegion, physical);
        let mut foreign = region.clone();
        foreign.material_domain.as_mut().unwrap().construction_index = 1;
        let paths = vec![
            command_path(MaterialConstructionRole::FunctionalBase, physical),
            command_path(MaterialConstructionRole::Core, physical),
            command_path(MaterialConstructionRole::FunctionalRegion, [8.2, 8.4]),
            foreign,
            region.clone(),
        ];
        let area = Area::functional_commands(
            &paths,
            physical,
            &frame,
            FunctionalArea::Region,
            DVec3::ZERO,
        )
        .unwrap();
        assert_eq!(area.vertices.len(), 2);
        assert_eq!(area.vertices[0].position, [2.0, 1.0, 8.2]);
        assert_eq!(area.vertices[1].position, [2.0, 11.0, 8.2]);
        assert!(
            Area::functional_commands(
                &[region],
                physical,
                &frame,
                FunctionalArea::ProtectedBase,
                DVec3::ZERO
            )
            .is_err()
        );
    }
    #[test]
    fn absent_zone_inherits_all_base_and_landings_and_covers_work_outside_region_z() {
        let mut frame = functional_frame();
        for (kind, physical) in [
            (FunctionalBaseSolidZoneKind::LowerLanding, [4.0, 5.0]),
            (FunctionalBaseSolidZoneKind::UpperCover, [15.0, 16.0]),
        ] {
            frame.solid_base_zones.push(FunctionalBaseSolidZone {
                kind,
                physical_z_mm: physical,
                inward_depth_mm: [0.0, 8.0],
            });
            let path = command_path(MaterialConstructionRole::FunctionalBase, physical);
            let area = Area::functional_commands(
                &[path],
                physical,
                &frame,
                FunctionalArea::ProtectedBase,
                DVec3::ZERO,
            )
            .unwrap();
            assert_eq!(area.vertices.len(), 2);
            assert_eq!(area.vertices[0].position[1], 1.0);
            assert_eq!(area.vertices[1].position[1], 8.0);
        }
        let physical = [16.0, 16.2];
        assert!(functional_zone_bounds(&frame, physical).unwrap().is_none());
        let path = command_path(MaterialConstructionRole::FunctionalBase, physical);
        let area = Area::functional_commands(
            std::slice::from_ref(&path),
            physical,
            &frame,
            FunctionalArea::InheritedBase,
            DVec3::ZERO,
        )
        .unwrap();
        assert_eq!(area.vertices.len(), 2);
        assert_eq!(area.vertices[1].position[1], 11.0);
        assert!(
            Area::functional_commands(
                &[path],
                physical,
                &frame,
                FunctionalArea::ProtectedBase,
                DVec3::ZERO
            )
            .is_err()
        );
        assert!(functional_zone_bounds(&frame, [4.9, 5.1]).is_err());
        assert!(functional_zone_bounds(&frame, [f64::NAN, 5.1]).is_err());
    }
    #[test]
    fn captured_zone_clipping_uses_world_frame_and_command_top() {
        let mut frame = functional_frame();
        frame.outward_normal = [1.0, 0.0, 0.0];
        frame.horizontal_direction = [0.0, -1.0, 0.0];
        frame.world_origin_mm = [20.0, 20.0, 0.0];
        let physical = [8.0, 8.2000004];
        let bounds = functional_zone_bounds(&frame, physical).unwrap().unwrap();
        assert_eq!(bounds.min, [12.0, 0.0, 8.0]);
        assert_eq!(bounds.max, [17.0, 20.0, 8.2]);
        let mut path = command_path(MaterialConstructionRole::FunctionalBase, physical);
        path.points_mm = vec![[10.0, 2.0, physical[1]], [19.0, 2.0, physical[1]]];
        assert_eq!(
            Area::functional_commands(
                &[path],
                physical,
                &frame,
                FunctionalArea::ProtectedBase,
                DVec3::ZERO
            )
            .unwrap()
            .vertices
            .len(),
            2
        );
    }
    #[test]
    fn command_overlay_refuses_work_and_vertex_overflow_without_partial_publication() {
        let frame = functional_frame();
        let physical = [8.0, 8.2];
        let mut path = command_path(MaterialConstructionRole::FunctionalRegion, physical);
        path.points_mm = (0..=MAX_INSPECTION_VERTICES / 2)
            .map(|index| [2.0, 1.0 + (index % 2) as f64, 8.2])
            .collect();
        assert_eq!(
            Area::functional_commands(
                std::slice::from_ref(&path),
                physical,
                &frame,
                FunctionalArea::Region,
                DVec3::ZERO
            )
            .unwrap()
            .vertices
            .len(),
            MAX_INSPECTION_VERTICES
        );
        path.points_mm.push([2.0, 3.0, 8.2]);
        assert!(
            Area::functional_commands(
                std::slice::from_ref(&path),
                physical,
                &frame,
                FunctionalArea::Region,
                DVec3::ZERO
            )
            .is_err()
        );
        path.points_mm = vec![[2.0, 1.0, 8.2]; MAX_INSPECTION_WORK];
        assert!(
            Area::functional_commands(
                &[path],
                physical,
                &frame,
                FunctionalArea::Region,
                DVec3::ZERO
            )
            .unwrap_err()
            .contains("work budget")
        );
        let mut path = command_path(MaterialConstructionRole::FunctionalRegion, physical);
        path.points_mm[1][0] = f64::INFINITY;
        assert!(
            Area::functional_commands(
                &[path],
                physical,
                &frame,
                FunctionalArea::Region,
                DVec3::ZERO
            )
            .is_err()
        );
    }
    #[test]
    fn functional_outline_keeps_shared_concave_join_and_excludes_source_exterior() {
        let origin = DVec3::new(1e9, 2e9, 30.0);
        let boundary = [
            [0.0, 0.0],
            [12.0, 0.0],
            [12.0, 5.0],
            [7.0, 5.0],
            [7.0, 8.0],
            [9.0, 8.0],
            [9.0, 9.0],
            [3.0, 9.0],
            [3.0, 8.0],
            [5.0, 8.0],
            [5.0, 5.0],
            [0.0, 5.0],
            [0.0, 0.0],
        ];
        let mut domain = ResolvedMaterialDomain {
            reference: MaterialDomainRef {
                construction_index: 0,
                role: MaterialConstructionRole::FunctionalRegion,
            },
            physical_z_mm: [31.0, 35.0],
            polygons: vec![MaterialDomainPolygon {
                exterior_mm: boundary
                    .map(|p| [origin.x + p[0], origin.y + p[1]])
                    .to_vec(),
                holes_mm: vec![],
            }],
        };
        let mut base = ResolvedMaterialDomain {
            reference: MaterialDomainRef {
                construction_index: 0,
                role: MaterialConstructionRole::FunctionalBase,
            },
            physical_z_mm: domain.physical_z_mm,
            polygons: vec![MaterialDomainPolygon {
                exterior_mm: [
                    [0.0, 5.0],
                    [5.0, 5.0],
                    [5.0, 8.0],
                    [3.0, 8.0],
                    [3.0, 9.0],
                    [9.0, 9.0],
                    [9.0, 8.0],
                    [7.0, 8.0],
                    [7.0, 5.0],
                    [12.0, 5.0],
                    [12.0, 12.0],
                    [0.0, 12.0],
                    [0.0, 5.0],
                ]
                .map(|point| [origin.x + point[0], origin.y + point[1]])
                .to_vec(),
                holes_mm: vec![],
            }],
        };
        let stock = AabbMm::new(
            [origin.x, origin.y, 30.0],
            [origin.x + 12.0, origin.y + 12.0, 40.0],
        )
        .unwrap();
        let area = Area::functional_join(&domain, &base, stock, origin).unwrap();
        assert_eq!(area.target.bounds_mm.min, [origin.x, origin.y + 5.0, 31.0]);
        assert_eq!(
            area.target.bounds_mm.max,
            [origin.x + 12.0, origin.y + 9.0, 35.0]
        );
        assert!(
            area.vertices
                .iter()
                .any(|vertex| vertex.position == [5.0, 8.0, 1.0])
        );
        assert!(
            area.vertices
                .iter()
                .any(|vertex| vertex.position == [5.0, 8.0, 5.0])
        );
        assert_eq!(area.vertices.len(), (boundary.len() - 4) * 6);
        base.physical_z_mm[1] = 34.0;
        assert!(Area::functional_join(&domain, &base, stock, origin).is_err());
        base.physical_z_mm = domain.physical_z_mm;
        base.polygons[0].exterior_mm[1][0] += 0.01;
        assert!(Area::functional_join(&domain, &base, stock, origin).is_err());
        domain.reference.role = MaterialConstructionRole::FunctionalBase;
        assert!(Area::functional_join(&domain, &base, stock, origin).is_err());
    }
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

#[cfg(test)]
mod import_tests {
    use super::*;
    use spiling_contracts::geometry::RigidPoseMm;
    #[test]
    fn import_marker_uses_current_occurrence_transform_and_rebased_scene_frame() {
        let bounds = AabbMm::new([0., 0., 0.], [2., 4., 6.]).unwrap();
        let pose = RigidPoseMm {
            rotation_xyzw: [
                0.,
                0.,
                std::f64::consts::FRAC_1_SQRT_2,
                std::f64::consts::FRAC_1_SQRT_2,
            ],
            translation_mm: [1000000000., 20., 30.],
        };
        let scene = AabbMm::new([999999990., 10., 20.], [1000000010., 40., 50.]).unwrap();
        let marker = Area::import_location(bounds, pose, Some(scene)).unwrap();
        assert_eq!(marker.vertices.len(), 24);
        assert!((marker.target.bounds_mm.min[0] - (1000000000. - 4.)).abs() < 0.001);
        assert!((marker.target.bounds_mm.max[1] - 22.).abs() < 0.001);
        assert!(Area::import_location(bounds, pose, None).is_err());
        assert!(
            marker
                .vertices
                .iter()
                .all(|v| v.position.iter().all(|x| x.is_finite() && x.abs() < 100.))
        );
    }
}
