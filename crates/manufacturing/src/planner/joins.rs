// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::{color, native, preflight, region, surface};
use crate::{CompilerInput, check_cancel, error};
use geo::{Area, Coord, LineString, MultiPolygon, Polygon};
use spiling_contracts::{geometry::AabbMm, manufacturing::*};
use std::{collections::BTreeMap, mem::size_of, sync::atomic::AtomicBool};

fn unsupported(message: &str) -> ManufacturingError {
    error(
        ManufacturingErrorCode::UnsupportedGeometry,
        format!("functional join: {message}"),
    )
}

pub(super) fn append_z_events(
    input: &CompilerInput<'_>,
    admission: &mut preflight::Admission,
    events: &mut Vec<f64>,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let JobWorkflow::FixedDualMarlin { job } = &input.intent.printer.workflow else {
        return Ok(());
    };
    let Some(request) = &job.functional_construction else {
        return Ok(());
    };
    let occurrence = input
        .occurrences
        .iter()
        .find(|occurrence| {
            occurrence.occurrence_id == request.occurrence_id
                && occurrence.definition_id == request.definition_id
        })
        .ok_or_else(|| unsupported("source occurrence is absent"))?;
    let definition = input
        .definitions
        .get(&request.definition_id)
        .ok_or_else(|| unsupported("source definition is absent"))?;
    if definition.provenance().source_hash != request.source_hash
        || occurrence.pose.rotation_xyzw != [0.0, 0.0, 0.0, 1.0]
    {
        return Err(unsupported(
            "collar scheduling requires the unchanged source and translation-only pose",
        ));
    }
    let face = definition
        .vertical_wall_face_with_work(&request.face_id, &mut admission.geometry_work, cancel)
        .map_err(|failure| super::native_error(failure, "functional collar face certificate"))?;
    let selected_z = request
        .face_patch
        .as_ref()
        .map_or([face.min_z_mm, face.max_z_mm], |patch| patch.source_z_mm);
    if selected_z[0] < face.min_z_mm || selected_z[1] > face.max_z_mm {
        return Err(unsupported(
            "selected height interval exceeds the native face",
        ));
    }
    let collar = request.join.end_collar_height_mm;
    let region_z = selected_z.map(|height| height + occurrence.pose.translation_mm[2]);
    let middle = [region_z[0] + collar, region_z[1] - collar];
    if !middle.iter().all(|height| height.is_finite()) || middle[0] >= middle[1] {
        return Err(unsupported(
            "end collars consume the complete native height",
        ));
    }
    for height in [region_z[0], middle[0], middle[1], region_z[1]] {
        check_cancel(cancel)?;
        if let Err(position) = events.binary_search_by(|old| old.total_cmp(&height)) {
            if events.len() >= 3 * MAX_PRINT_LAYERS as usize {
                return Err(error(
                    ManufacturingErrorCode::ResourceLimit,
                    "functional join event limit exceeded",
                ));
            }
            events.insert(position, height);
        }
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "Native region construction borrows the admitted source, complete schedule and shared resource authorities"
)]
pub(super) fn capture_or_color(
    input: &CompilerInput<'_>,
    sections: &[super::SectionSpan],
    schedule: &[super::LayerElevation],
    native_bounds: &BTreeMap<spiling_contracts::geometry::OccurrenceId, AabbMm>,
    retained_points: usize,
    owned_bytes: usize,
    work: &mut usize,
    geometry_work: &mut spiling_geometry::GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<color::Construction, ManufacturingError> {
    let request = match &input.intent.printer.workflow {
        JobWorkflow::FixedDualMarlin { job } => job.functional_construction.as_ref(),
        _ => None,
    };
    let Some(request) = request else {
        return color::capture(
            input,
            sections,
            schedule,
            native_bounds,
            retained_points,
            owned_bytes,
            work,
            geometry_work,
            cancel,
        );
    };
    if input.intent.recipe.infill_fraction != 1.0 {
        return Err(unsupported(
            "first functional construction requires an explicitly solid base recipe",
        ));
    }
    if input.occurrences.len() != 1 {
        return Err(unsupported("requires one native occurrence"));
    }
    let occurrence = &input.occurrences[0];
    let definition = &input.definitions[&occurrence.definition_id];
    if occurrence.occurrence_id != request.occurrence_id
        || occurrence.definition_id != request.definition_id
        || definition.provenance().source_hash != request.source_hash
        || occurrence.pose.rotation_xyzw != [0.0, 0.0, 0.0, 1.0]
    {
        return Err(unsupported(
            "requires the exact source/face binding and translation-only placement",
        ));
    }
    let face = definition
        .vertical_wall_face_with_work(&request.face_id, geometry_work, cancel)
        .map_err(|failure| super::native_error(failure, "functional native-face certificate"))?;
    surface::admit_extrusion(
        input,
        occurrence,
        native_bounds[&occurrence.occurrence_id],
        &request.face_id,
        face.min_z_mm,
        face.max_z_mm,
        cancel,
    )?;
    let depth_axis = (0..2)
        .find(|&axis| face.outward_normal[axis].abs() == 1.0)
        .ok_or_else(|| unsupported("first functional lane requires an axis-aligned native face"))?;
    let horizontal_axis = 1 - depth_axis;
    if face.horizontal_direction[horizontal_axis].abs() != 1.0
        || face.outward_normal[horizontal_axis] != 0.0
        || face.horizontal_direction[depth_axis] != 0.0
    {
        return Err(unsupported("native face frame is not exactly axis aligned"));
    }
    let origin = super::forward(&occurrence.pose, face.origin_mm);
    let selected_u = request
        .face_patch
        .as_ref()
        .map_or([0.0, face.width_mm], |patch| patch.horizontal_mm);
    let selected_z = request
        .face_patch
        .as_ref()
        .map_or([face.min_z_mm, face.max_z_mm], |patch| patch.source_z_mm);
    if selected_u[0] < 0.0
        || selected_u[1] > face.width_mm
        || selected_z[0] < face.min_z_mm
        || selected_z[1] > face.max_z_mm
    {
        return Err(unsupported(
            "selected patch exceeds the certified native face",
        ));
    }
    let region_z = selected_z.map(|height| height + occurrence.pose.translation_mm[2]);
    let depth_sign = -face.outward_normal[depth_axis];
    let horizontal_sign = face.horizontal_direction[horizontal_axis];
    let width = input.intent.recipe.bead_width_mm;
    let join = &request.join;
    if [
        join.stem_width_mm,
        join.head_depth_mm,
        join.edge_margin_mm,
        join.pitch_mm - join.head_width_mm,
        join.collar_head_width_mm - join.stem_width_mm,
        join.head_width_mm - join.collar_head_width_mm,
    ]
    .into_iter()
    .any(|dimension| dimension < 2.0 * width)
    {
        return Err(unsupported(
            "key, shoulder, retainer and gap features require at least two declared bead widths",
        ));
    }
    let total_depth =
        request.minimum_pure_depth_mm + join.reach_into_region_mm + join.reach_into_base_mm;
    let usable = selected_u[1] - selected_u[0] - 2.0 * join.edge_margin_mm;
    let count = ((usable - join.head_width_mm) / join.pitch_mm).floor() + 1.0;
    if !count.is_finite() || !(1.0..=16.0).contains(&count) {
        return Err(unsupported(
            "face must accommodate one to sixteen complete keys with protected edge margins",
        ));
    }
    let mut count = count as usize;
    if count.is_multiple_of(2) {
        count -= 1;
    }
    while count > 1 {
        let left = (selected_u[0] + selected_u[1]) * 0.5
            - (count - 1) as f64 * 0.5 * join.pitch_mm
            - join.head_width_mm * 0.5;
        if left - selected_u[0] >= join.edge_margin_mm
            && selected_u[1] - (selected_u[0] + selected_u[1] - left) >= join.edge_margin_mm
        {
            break;
        }
        count -= 2;
    }
    let centers: Vec<_> = (0..count)
        .map(|index| {
            (selected_u[0] + selected_u[1]) * 0.5
                + (index as f64 - (count - 1) as f64 * 0.5) * join.pitch_mm
        })
        .collect();
    let middle = [
        region_z[0] + join.end_collar_height_mm,
        region_z[1] - join.end_collar_height_mm,
    ];
    let mut construction = color::Construction::default();
    let mut stock_bounds = None;
    let staging_points = 32_768usize;
    let staging_bytes = staging_points * size_of::<Coord<f64>>() * 8;
    super::remaining_geometry_units(
        retained_points + staging_points,
        owned_bytes + staging_bytes,
    )?;
    for section in sections {
        check_cancel(cancel)?;
        if section.material.0.len() != 1 || !section.material.0[0].interiors().is_empty() {
            return Err(unsupported(
                "first functional lane requires a complete rectangular native stock without holes",
            ));
        }
        let source = &section.material.0[0];
        let mut extents = [[f64::INFINITY, f64::NEG_INFINITY]; 2];
        for point in &source.exterior().0 {
            for (axis, value) in [point.x, point.y].into_iter().enumerate() {
                extents[axis][0] = extents[axis][0].min(value);
                extents[axis][1] = extents[axis][1].max(value);
            }
        }
        for edge in source.exterior().lines() {
            if !((edge.start.x == edge.end.x && extents[0].contains(&edge.start.x))
                || (edge.start.y == edge.end.y && extents[1].contains(&edge.start.y)))
            {
                return Err(unsupported("native stock has a nonrectangular boundary"));
            }
        }
        if definition.faces().len() != 6
            || definition.faces().iter().any(|info| {
                info.geometry.family != spiling_contracts::geometry::SurfaceFamily::Plane
            })
            || (extents[horizontal_axis][1] - extents[horizontal_axis][0]) != face.width_mm
            || !extents[depth_axis].contains(&origin[depth_axis])
            || extents[depth_axis][1] - extents[depth_axis][0] < total_depth + 2.0 * width
        {
            return Err(unsupported(
                "protected pure depth, complete keys and a two-bead parent root do not fit this native rectangular stock",
            ));
        }
        let stock = AabbMm::new(
            [
                extents[0][0],
                extents[1][0],
                face.min_z_mm + occurrence.pose.translation_mm[2],
            ],
            [
                extents[0][1],
                extents[1][1],
                face.max_z_mm + occurrence.pose.translation_mm[2],
            ],
        )
        .map_err(|failure| unsupported(&failure.message))?;
        if stock_bounds.is_some_and(|previous| previous != stock) {
            return Err(unsupported(
                "native stock rectangle changed across closed source captures",
            ));
        }
        stock_bounds = Some(stock);
        let mut walls = [false; 4];
        let mut caps = [false; 2];
        for info in definition.faces() {
            check_cancel(cancel)?;
            let Some(spiling_contracts::geometry::AnalyticSurfaceDescription::Plane {
                origin_mm,
                normal,
            }) = &info.geometry.analytic
            else {
                return Err(unsupported(
                    "native stock requires six certified planar boundary carriers",
                ));
            };
            if normal[0] == 0.0 && normal[1] == 0.0 && normal[2].abs() == 1.0 {
                let cap = if origin_mm[2] == face.min_z_mm {
                    0
                } else if origin_mm[2] == face.max_z_mm {
                    1
                } else {
                    return Err(unsupported("native stock has an intermediate planar cap"));
                };
                if caps[cap] {
                    return Err(unsupported("native stock has a duplicate cap"));
                }
                caps[cap] = true;
                continue;
            }
            let wall = definition
                .vertical_wall_face_with_work(&info.face_id, geometry_work, cancel)
                .map_err(|failure| {
                    super::native_error(failure, "functional complete native stock wall")
                })?;
            let axis = (0..2)
                .find(|&axis| wall.outward_normal[axis].abs() == 1.0)
                .ok_or_else(|| unsupported("native stock wall is not axis aligned"))?;
            let side = usize::from(wall.outward_normal[axis] > 0.0);
            let slot = axis * 2 + side;
            let location = wall.origin_mm[axis] + occurrence.pose.translation_mm[axis];
            if walls[slot]
                || wall.outward_normal[1 - axis] != 0.0
                || wall.min_z_mm != face.min_z_mm
                || wall.max_z_mm != face.max_z_mm
                || location != extents[axis][side]
                || wall.width_mm != extents[1 - axis][1] - extents[1 - axis][0]
            {
                return Err(unsupported(
                    "complete native rectangular walls do not match the admitted source section",
                ));
            }
            walls[slot] = true;
        }
        if walls.contains(&false) || caps.contains(&false) {
            return Err(unsupported("native stock boundary inventory is incomplete"));
        }
        let lower = schedule[section.start as usize].lower_z;
        let upper = schedule[section.end as usize - 1].z;
        let boundaries = [
            lower,
            region_z[0].clamp(lower, upper),
            middle[0].clamp(lower, upper),
            middle[1].clamp(lower, upper),
            region_z[1].clamp(lower, upper),
            upper,
        ];
        for interval in boundaries.windows(2) {
            if interval[0] >= interval[1] {
                continue;
            }
            let physical_z_mm = [interval[0], interval[1]];
            if interval[1] <= region_z[0] || interval[0] >= region_z[1] {
                retain(
                    &mut construction,
                    section.material.as_ref().clone(),
                    MaterialConstructionRole::FunctionalBase,
                    physical_z_mm,
                )?;
                super::remaining_geometry_units(
                    retained_points + staging_points + construction.points,
                    owned_bytes + staging_bytes + construction.bytes,
                )?;
                continue;
            }
            let head_width = if interval[0] >= middle[0] && interval[1] <= middle[1] {
                join.head_width_mm
            } else {
                join.collar_head_width_mm
            };
            super::boolean::charge(work, centers.len() * 8 + 16)?;
            let (region, base) = paired_domains(
                request,
                origin,
                depth_axis,
                depth_sign,
                horizontal_sign,
                face.width_mm,
                selected_u,
                extents,
                &centers,
                head_width,
            )?;
            for (role, material) in [
                (MaterialConstructionRole::FunctionalRegion, region),
                (MaterialConstructionRole::FunctionalBase, base),
            ] {
                retain(&mut construction, material, role, physical_z_mm)?;
            }
            super::remaining_geometry_units(
                retained_points + staging_points + construction.points,
                owned_bytes + staging_bytes + construction.bytes,
            )?;
        }
    }
    construction.functional_frame = Some(FunctionalMaterialFrame {
        occurrence_id: request.occurrence_id,
        definition_id: request.definition_id.clone(),
        source_hash: request.source_hash.clone(),
        face_id: request.face_id.clone(),
        origin_mm: face.origin_mm,
        world_origin_mm: origin,
        stock_bounds_mm: stock_bounds
            .ok_or_else(|| unsupported("no complete native stock capture"))?,
        horizontal_direction: face.horizontal_direction,
        outward_normal: face.outward_normal,
        width_mm: face.width_mm,
        source_z_mm: [face.min_z_mm, face.max_z_mm],
        selected_u_mm: selected_u,
        region_z_mm: region_z,
        key_middle_z_mm: middle,
        key_centers_u_mm: centers,
    });
    construction.bytes += size_of::<FunctionalMaterialFrame>()
        + MAX_FUNCTIONAL_KEYS * size_of::<f64>()
        + request.definition_id.as_str().len()
        + request.source_hash.as_str().len()
        + request.face_id.as_str().len();
    super::remaining_geometry_units(
        retained_points + construction.points,
        owned_bytes + construction.bytes,
    )?;
    Ok(construction)
}

#[expect(
    clippy::too_many_arguments,
    reason = "One shared boundary partitions the complete qualified stock in its exact native axis frame"
)]
fn paired_domains(
    request: &FunctionalMaterialConstruction,
    origin: [f64; 3],
    axis: usize,
    depth_sign: f64,
    horizontal_sign: f64,
    width: f64,
    selected_u: [f64; 2],
    stock: [[f64; 2]; 2],
    centers: &[f64],
    head_width: f64,
) -> Result<(MultiPolygon<f64>, MultiPolygon<f64>), ManufacturingError> {
    let join = &request.join;
    let pure = request.minimum_pure_depth_mm;
    let end = pure + join.reach_into_region_mm + join.reach_into_base_mm;
    let neck_end = end - join.head_depth_mm;
    let horizontal_start = stock[1 - axis][usize::from(horizontal_sign < 0.0)];
    let horizontal_end = stock[1 - axis][usize::from(horizontal_sign > 0.0)];
    let depth_start = stock[axis][usize::from(depth_sign < 0.0)];
    let depth_end = stock[axis][usize::from(depth_sign > 0.0)];
    if origin[1 - axis] != horizontal_start || origin[axis] != depth_start {
        return Err(unsupported(
            "native frame does not coincide with the literal stock corner",
        ));
    }
    let pure_coordinate = origin[axis] + depth_sign * pure;
    let pure_lower = program_coordinates::enclosed_vertical_gap_mm(
        depth_sign * pure_coordinate,
        depth_sign * origin[axis],
    )?;
    let root_lower = program_coordinates::enclosed_vertical_gap_mm(
        depth_sign * depth_end,
        depth_sign * (origin[axis] + depth_sign * end),
    )?;
    if pure_lower < pure || root_lower <= 0.0 {
        return Err(error(
            ManufacturingErrorCode::ProgramCoordinateConditioningFailure,
            "functional cut cannot establish the requested minimum pure depth and a positive parent root",
        ));
    }
    let point = |u: f64, depth: f64| {
        let mut xy = [origin[0], origin[1]];
        xy[axis] += depth_sign * depth;
        xy[1 - axis] += horizontal_sign * u;
        if u == 0.0 {
            xy[1 - axis] = horizontal_start;
        }
        if u == width {
            xy[1 - axis] = horizontal_end;
        }
        Coord::from(xy)
    };
    let mut boundary = Vec::with_capacity(centers.len() * 8 + 2);
    boundary.push(point(selected_u[0], pure));
    for &center in centers {
        let neck = [
            center - join.stem_width_mm * 0.5,
            center + join.stem_width_mm * 0.5,
        ];
        let head = [center - head_width * 0.5, center + head_width * 0.5];
        for [u, depth] in [
            [neck[0], pure],
            [neck[0], neck_end],
            [head[0], neck_end],
            [head[0], end],
            [head[1], end],
            [head[1], neck_end],
            [neck[1], neck_end],
            [neck[1], pure],
        ] {
            boundary.push(point(u, depth));
        }
    }
    boundary.push(point(selected_u[1], pure));
    if boundary.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(unsupported(
            "key boundary collapsed at the source coordinate precision",
        ));
    }
    let mut region_ring = Vec::with_capacity(boundary.len() + 3);
    region_ring.extend([point(selected_u[0], 0.0), point(selected_u[1], 0.0)]);
    region_ring.extend(boundary.iter().rev().copied());
    region_ring.push(region_ring[0]);
    let mut base_ring = boundary;
    if selected_u[1] < width {
        base_ring.extend([point(selected_u[1], 0.0), point(width, 0.0)]);
    }
    let mut right = point(width, 0.0);
    let mut left = point(0.0, 0.0);
    if axis == 0 {
        right.x = depth_end;
        left.x = depth_end;
    } else {
        right.y = depth_end;
        left.y = depth_end;
    }
    base_ring.extend([right, left]);
    if selected_u[0] > 0.0 {
        base_ring.extend([point(0.0, 0.0), point(selected_u[0], 0.0)]);
    }
    base_ring.push(base_ring[0]);
    Ok((
        MultiPolygon(vec![Polygon::new(LineString(region_ring), Vec::new())]),
        MultiPolygon(vec![Polygon::new(LineString(base_ring), Vec::new())]),
    ))
}

fn retain(
    construction: &mut color::Construction,
    mut material: MultiPolygon<f64>,
    role: MaterialConstructionRole,
    physical_z_mm: [f64; 2],
) -> Result<(), ManufacturingError> {
    region::canonicalize(&mut material);
    region::check_size(&material)?;
    if construction.domains.len() >= MAX_MATERIAL_DOMAINS || material.unsigned_area() <= 0.0 {
        return Err(unsupported(
            "functional material domain inventory exceeded or empty",
        ));
    }
    let reference = MaterialDomainRef {
        construction_index: 0,
        role,
    };
    let stored = ResolvedMaterialDomain {
        reference,
        physical_z_mm,
        polygons: material
            .0
            .iter()
            .map(|polygon| MaterialDomainPolygon {
                exterior_mm: polygon
                    .exterior()
                    .0
                    .iter()
                    .map(|point| [point.x, point.y])
                    .collect(),
                holes_mm: polygon
                    .interiors()
                    .iter()
                    .map(|ring| ring.0.iter().map(|point| [point.x, point.y]).collect())
                    .collect(),
            })
            .collect(),
    };
    stored.validate()?;
    construction.points += region::point_count(&material);
    construction.bytes += native::material_bytes(&mut material) * 2
        + 2 * size_of::<color::Domain>()
        + 2 * size_of::<ResolvedMaterialDomain>();
    construction.stored.push(stored);
    construction.domains.push(color::Domain {
        reference,
        physical_z_mm,
        material,
    });
    Ok(())
}
