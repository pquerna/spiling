// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::{cancelled, failed, project_xyz6_point_mm};
use crate::planner::support::strict_capsule_point;
use geo::{Contains, Coord, Intersects, Line, LineString, MultiPolygon, Point, Polygon, Rect};
use spiling_contracts::manufacturing::*;
use std::sync::atomic::AtomicBool;

fn charge(work: &mut usize, count: usize) -> Result<(), ManufacturingError> {
    *work = work
        .checked_add(count)
        .filter(|total| *total <= 128_000_000)
        .ok_or_else(|| {
            ManufacturingError::new(
                ManufacturingErrorCode::ResourceLimit,
                "functional join replay work exceeded",
            )
        })?;
    Ok(())
}

fn rectangle(
    frame: &FunctionalMaterialFrame,
    horizontal: [f64; 2],
    depth: [f64; 2],
) -> Polygon<f64> {
    let points = [
        [horizontal[0], depth[0]],
        [horizontal[1], depth[0]],
        [horizontal[1], depth[1]],
        [horizontal[0], depth[1]],
        [horizontal[0], depth[0]],
    ];
    Polygon::new(
        LineString(
            points
                .into_iter()
                .map(|[u, d]| {
                    let mut point = Coord {
                        x: frame.world_origin_mm[0] + frame.horizontal_direction[0] * u
                            - frame.outward_normal[0] * d,
                        y: frame.world_origin_mm[1] + frame.horizontal_direction[1] * u
                            - frame.outward_normal[1] * d,
                    };
                    if frame.horizontal_direction[0] != 0.0 {
                        if u == 0.0 {
                            point.x = frame.world_origin_mm[0];
                        }
                        if u == frame.width_mm {
                            point.x = if frame.horizontal_direction[0] > 0.0 {
                                frame.stock_bounds_mm.max[0]
                            } else {
                                frame.stock_bounds_mm.min[0]
                            };
                        }
                    } else {
                        if u == 0.0 {
                            point.y = frame.world_origin_mm[1];
                        }
                        if u == frame.width_mm {
                            point.y = if frame.horizontal_direction[1] > 0.0 {
                                frame.stock_bounds_mm.max[1]
                            } else {
                                frame.stock_bounds_mm.min[1]
                            };
                        }
                    }
                    point
                })
                .collect(),
        ),
        Vec::new(),
    )
}

fn material(domain: &ResolvedMaterialDomain) -> MultiPolygon<f64> {
    MultiPolygon(
        domain
            .polygons
            .iter()
            .map(|polygon| {
                let ring = |points: &[[f64; 2]]| {
                    LineString(points.iter().map(|point| Coord::from(*point)).collect())
                };
                Polygon::new(
                    ring(&polygon.exterior_mm),
                    polygon.holes_mm.iter().map(|points| ring(points)).collect(),
                )
            })
            .collect(),
    )
}

fn core_radius(edge: &DepositionEdge<'_>, height: f64) -> Result<f64, ManufacturingError> {
    let vertical = program_coordinates::project_xyz6_segment_bead_height_mm(
        edge.points_mm[0][2],
        edge.points_mm[1][2],
        height,
    )?;
    let height_upper = -program_coordinates::enclosed_vertical_gap_mm(-vertical[1], -vertical[0])?;
    let radius = program_coordinates::enclosed_vertical_gap_mm(
        program_coordinates::enclosed_vertical_gap_mm(edge.width_mm, height_upper)? * 0.5,
        program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM,
    )?;
    if !radius.is_finite() || radius <= 0.0 {
        return Err(failed(
            "functional attachment has no positive nominal bead core",
        ));
    }
    Ok(radius)
}

fn connected(
    left: &DepositionPath,
    right: &DepositionPath,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    let height = left.physical_z_mm[1] - left.physical_z_mm[0];
    for a in left.checked_edges()? {
        for b in right.checked_edges()? {
            cancelled(cancel)?;
            charge(work, 1)?;
            let a_points = [
                project_xyz6_point_mm(a.points_mm[0])?,
                project_xyz6_point_mm(a.points_mm[1])?,
            ];
            let b_points = [
                project_xyz6_point_mm(b.points_mm[0])?,
                project_xyz6_point_mm(b.points_mm[1])?,
            ];
            let radius = (core_radius(&a, height)? + core_radius(&b, height)?).next_down();
            if strict_capsule_point(a_points[0], b_points, radius)?
                || strict_capsule_point(a_points[1], b_points, radius)?
                || strict_capsule_point(b_points[0], a_points, radius)?
                || strict_capsule_point(b_points[1], a_points, radius)?
            {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub(super) fn verify(
    plan: &NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let JobWorkflow::FixedDualMarlin { job } = &intent.printer.workflow else {
        return Ok(());
    };
    let Some(request) = &job.functional_construction else {
        return Ok(());
    };
    let frame = plan
        .functional_material_frame
        .as_ref()
        .ok_or_else(|| failed("functional replay has no captured frame"))?;
    let join = &request.join;
    let total_depth =
        request.minimum_pure_depth_mm + join.reach_into_region_mm + join.reach_into_base_mm;
    let neck_end = total_depth - join.head_depth_mm;
    let depth_axis = (0..2)
        .find(|&axis| frame.outward_normal[axis].abs() == 1.0)
        .ok_or_else(|| failed("functional replay requires an exact Cartesian native frame"))?;
    let sign = -frame.outward_normal[depth_axis];
    let pure = frame.world_origin_mm[depth_axis] + sign * request.minimum_pure_depth_mm;
    if program_coordinates::enclosed_vertical_gap_mm(
        sign * pure,
        sign * frame.world_origin_mm[depth_axis],
    )? < request.minimum_pure_depth_mm
    {
        return Err(failed(
            "functional replay cannot establish the minimum pure depth after coordinate rounding",
        ));
    }
    let bounds = frame.stock_bounds_mm;
    let stock = Rect::new(
        Coord {
            x: bounds.min[0],
            y: bounds.min[1],
        },
        Coord {
            x: bounds.max[0],
            y: bounds.max[1],
        },
    )
    .to_polygon();
    for domain in &plan.material_domains {
        cancelled(cancel)?;
        let active = frame.region_z_mm[0] <= domain.physical_z_mm[0]
            && domain.physical_z_mm[1] <= frame.region_z_mm[1];
        let middle = domain.physical_z_mm[0] >= frame.key_middle_z_mm[0]
            && domain.physical_z_mm[1] <= frame.key_middle_z_mm[1];
        let head_width = if middle {
            join.head_width_mm
        } else {
            join.collar_head_width_mm
        };
        let mut expected = Vec::new();
        if active {
            expected.push(rectangle(
                frame,
                frame.selected_u_mm,
                [0.0, request.minimum_pure_depth_mm],
            ));
        }
        for &center in frame.key_centers_u_mm.iter().filter(|_| active) {
            for (width, depth) in [
                (
                    join.stem_width_mm,
                    [request.minimum_pure_depth_mm, neck_end],
                ),
                (head_width, [neck_end, total_depth]),
            ] {
                charge(work, 5)?;
                expected.push(rectangle(
                    frame,
                    [center - width * 0.5, center + width * 0.5],
                    depth,
                ));
            }
        }
        let actual = material(domain);
        verify_grid(
            &actual,
            &expected,
            &stock,
            domain.reference.role == MaterialConstructionRole::FunctionalBase,
            work,
            cancel,
        )?;
    }
    let roles = [
        MaterialConstructionRole::FunctionalRegion,
        MaterialConstructionRole::FunctionalBase,
    ];
    let mut rooted = Vec::new();
    for (layer_index, layer) in plan.layers.iter().enumerate() {
        cancelled(cancel)?;
        charge(work, layer.paths.len())?;
        rooted.clear();
        rooted.try_reserve_exact(layer.paths.len()).map_err(|_| {
            ManufacturingError::new(
                ManufacturingErrorCode::ResourceLimit,
                "functional attachment workspace allocation refused",
            )
        })?;
        rooted.resize(layer.paths.len(), false);
        for role in roles {
            let owns = |path: &DepositionPath| {
                path.material_domain
                    .is_some_and(|reference| reference.role == role)
            };
            let mut remaining = layer.paths.iter().filter(|path| owns(path)).count();
            for (index, path) in layer
                .paths
                .iter()
                .enumerate()
                .filter(|(_, path)| owns(path))
            {
                for edge in path.checked_edges()? {
                    charge(work, 1)?;
                    core_radius(&edge, path.physical_z_mm[1] - path.physical_z_mm[0])?;
                    for point in edge.points_mm {
                        let point = project_xyz6_point_mm(*point)?;
                        let depth = -(point[0] - frame.world_origin_mm[0])
                            * frame.outward_normal[0]
                            - (point[1] - frame.world_origin_mm[1]) * frame.outward_normal[1];
                        if (role == MaterialConstructionRole::FunctionalRegion
                            && depth < request.minimum_pure_depth_mm - intent.recipe.bead_width_mm)
                            || (role == MaterialConstructionRole::FunctionalBase
                                && depth > total_depth + intent.recipe.bead_width_mm * 0.5)
                        {
                            rooted[index] = true;
                        }
                    }
                }
                if rooted[index] {
                    remaining -= 1;
                }
            }
            while remaining > 0 {
                let before = remaining;
                for (index, path) in layer
                    .paths
                    .iter()
                    .enumerate()
                    .filter(|(_, path)| owns(path))
                {
                    cancelled(cancel)?;
                    charge(work, layer.paths.len())?;
                    if rooted[index] {
                        continue;
                    }
                    for (other, witness) in layer.paths.iter().enumerate() {
                        if other != index
                            && rooted[other]
                            && owns(witness)
                            && connected(path, witness, work, cancel)?
                        {
                            rooted[index] = true;
                            remaining -= 1;
                            break;
                        }
                    }
                }
                if before == remaining {
                    return Err(failed(
                        "functional key or retainer has no positive same-material core path to its parent",
                    ));
                }
            }
        }
        let middle =
            layer.lower_z_mm >= frame.key_middle_z_mm[0] && layer.z_mm <= frame.key_middle_z_mm[1];
        if layer.z_mm <= frame.region_z_mm[0] || layer.lower_z_mm >= frame.region_z_mm[1] {
            continue;
        }
        let role = if middle {
            MaterialConstructionRole::FunctionalRegion
        } else {
            MaterialConstructionRole::FunctionalBase
        };
        for &center in &frame.key_centers_u_mm {
            for sign in [-1.0, 1.0] {
                let margin = intent.recipe.bead_width_mm * 0.25;
                let side = [
                    center + sign * (join.collar_head_width_mm * 0.5 + margin),
                    center + sign * (join.head_width_mm * 0.5 - margin),
                ];
                let polygon = rectangle(
                    frame,
                    [side[0].min(side[1]), side[0].max(side[1])],
                    [neck_end + margin, total_depth - margin],
                );
                let coordinates = &polygon.exterior().0;
                let bounds = Rect::new(
                    Coord {
                        x: coordinates
                            .iter()
                            .map(|point| point.x)
                            .fold(f64::INFINITY, f64::min),
                        y: coordinates
                            .iter()
                            .map(|point| point.y)
                            .fold(f64::INFINITY, f64::min),
                    },
                    Coord {
                        x: coordinates
                            .iter()
                            .map(|point| point.x)
                            .fold(f64::NEG_INFINITY, f64::max),
                        y: coordinates
                            .iter()
                            .map(|point| point.y)
                            .fold(f64::NEG_INFINITY, f64::max),
                    },
                );
                let mut present = false;
                for (index, path) in layer.paths.iter().enumerate().filter(|(_, path)| {
                    path.material_domain
                        .is_some_and(|reference| reference.role == role)
                }) {
                    for edge in path.checked_edges()? {
                        cancelled(cancel)?;
                        charge(work, 1)?;
                        let a = project_xyz6_point_mm(edge.points_mm[0])?;
                        let b = project_xyz6_point_mm(edge.points_mm[1])?;
                        let line =
                            Line::new(Coord { x: a[0], y: a[1] }, Coord { x: b[0], y: b[1] });
                        present |= rooted[index]
                            && (bounds.intersects(&line)
                                || bounds.contains(&Point::from(line.start)));
                    }
                }
                if !present {
                    return Err(failed(
                        "functional join is missing an actual attached head shoulder or end retainer",
                    ));
                }
                if frame.key_middle_z_mm.contains(&layer.lower_z_mm) {
                    let previous = layer_index
                        .checked_sub(1)
                        .and_then(|index| plan.layers.get(index))
                        .ok_or_else(|| {
                            failed("functional end retainer has no preceding physical layer")
                        })?;
                    let old_role = if middle {
                        MaterialConstructionRole::FunctionalBase
                    } else {
                        MaterialConstructionRole::FunctionalRegion
                    };
                    if previous.z_mm != layer.lower_z_mm
                        || !phase_overlap(previous, layer, old_role, role, bounds, work, cancel)?
                    {
                        return Err(failed(
                            "functional head and end retainer have no positive actual overlap across their physical phase boundary",
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

fn phase_overlap(
    previous: &PrintLayer,
    current: &PrintLayer,
    previous_role: MaterialConstructionRole,
    current_role: MaterialConstructionRole,
    bounds: Rect<f64>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    for lower in previous.paths.iter().filter(|path| {
        path.material_domain
            .is_some_and(|reference| reference.role == previous_role)
    }) {
        for upper in current.paths.iter().filter(|path| {
            path.material_domain
                .is_some_and(|reference| reference.role == current_role)
        }) {
            for lower_edge in lower.checked_edges()? {
                for upper_edge in upper.checked_edges()? {
                    cancelled(cancel)?;
                    charge(work, 1)?;
                    let lower_points = [
                        project_xyz6_point_mm(lower_edge.points_mm[0])?,
                        project_xyz6_point_mm(lower_edge.points_mm[1])?,
                    ];
                    let upper_points = [
                        project_xyz6_point_mm(upper_edge.points_mm[0])?,
                        project_xyz6_point_mm(upper_edge.points_mm[1])?,
                    ];
                    for (points, other, edge, height) in [
                        (
                            lower_points,
                            upper_points,
                            &upper_edge,
                            upper.physical_z_mm[1] - upper.physical_z_mm[0],
                        ),
                        (
                            upper_points,
                            lower_points,
                            &lower_edge,
                            lower.physical_z_mm[1] - lower.physical_z_mm[0],
                        ),
                    ] {
                        for point in points {
                            if bounds.contains(&Point::new(point[0], point[1]))
                                && strict_capsule_point(point, other, core_radius(edge, height)?)?
                            {
                                return Ok(true);
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(false)
}

fn verify_grid(
    actual: &MultiPolygon<f64>,
    expected: &[Polygon<f64>],
    stock: &Polygon<f64>,
    complement: bool,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let mut horizontal = Vec::with_capacity((expected.len() + 1) * 5);
    let mut vertical = Vec::with_capacity((expected.len() + 1) * 5);
    for point in expected
        .iter()
        .chain(std::iter::once(stock))
        .flat_map(|polygon| &polygon.exterior().0)
    {
        horizontal.push(point.x);
        vertical.push(point.y);
    }
    horizontal.sort_by(f64::total_cmp);
    vertical.sort_by(f64::total_cmp);
    horizontal.dedup();
    vertical.dedup();
    for polygon in &actual.0 {
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            for point in &ring.0 {
                charge(work, horizontal.len() + vertical.len())?;
                if !horizontal.contains(&point.x) || !vertical.contains(&point.y) {
                    return Err(failed(
                        "functional boundary leaves its exact requested cut arrangement",
                    ));
                }
            }
            if ring
                .lines()
                .any(|edge| edge.start.x != edge.end.x && edge.start.y != edge.end.y)
            {
                return Err(failed(
                    "functional boundary is not the requested orthogonal construction",
                ));
            }
        }
    }
    for x in horizontal.windows(2) {
        for y in vertical.windows(2) {
            cancelled(cancel)?;
            let center = Point::new(x[0] * 0.5 + x[1] * 0.5, y[0] * 0.5 + y[1] * 0.5);
            if !(x[0] < center.x() && center.x() < x[1] && y[0] < center.y() && center.y() < y[1]) {
                return Err(failed(
                    "functional verification cell has no conditioned interior sample",
                ));
            }
            charge(
                work,
                actual
                    .0
                    .iter()
                    .map(|polygon| {
                        polygon.exterior().0.len()
                            + polygon
                                .interiors()
                                .iter()
                                .map(|ring| ring.0.len())
                                .sum::<usize>()
                    })
                    .sum::<usize>()
                    + expected.len() * 5,
            )?;
            let inside = expected.iter().any(|polygon| polygon.contains(&center));
            let required = stock.contains(&center) && if complement { !inside } else { inside };
            if actual.contains(&center) != required {
                return Err(failed(
                    "functional region does not realize its pure depth and complete requested key phase",
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shoulder_presence_is_not_a_head_retainer_overlap_receipt() {
        let (_, plan) = super::super::tests::fixture();
        let mut previous = plan.layers[0].clone();
        previous.paths.truncate(1);
        previous.paths[0].points_mm = vec![[0.0, 1.0, 0.25], [1.0, 1.0, 0.25]];
        previous.paths[0].width = WidthProfile::Constant { width_mm: 1.0 };
        previous.paths[0].physical_z_mm = [0.0, 0.25];
        previous.paths[0].material_domain = Some(MaterialDomainRef {
            construction_index: 0,
            role: MaterialConstructionRole::FunctionalBase,
        });
        let mut current = previous.clone();
        current.paths[0].points_mm = vec![[0.0, 3.0, 0.5], [1.0, 3.0, 0.5]];
        current.paths[0].physical_z_mm = [0.25, 0.5];
        current.paths[0].material_domain.as_mut().unwrap().role =
            MaterialConstructionRole::FunctionalRegion;
        let bounds = Rect::new(Coord { x: -1.0, y: 0.0 }, Coord { x: 2.0, y: 4.0 });
        let cancel = AtomicBool::new(false);
        assert!(
            !phase_overlap(
                &previous,
                &current,
                MaterialConstructionRole::FunctionalBase,
                MaterialConstructionRole::FunctionalRegion,
                bounds,
                &mut 0,
                &cancel
            )
            .unwrap()
        );
        current.paths[0]
            .points_mm
            .iter_mut()
            .for_each(|point| point[1] = 1.1);
        assert!(
            phase_overlap(
                &previous,
                &current,
                MaterialConstructionRole::FunctionalBase,
                MaterialConstructionRole::FunctionalRegion,
                bounds,
                &mut 0,
                &cancel
            )
            .unwrap()
        );
    }

    #[test]
    fn complete_base_complement_is_required_even_without_a_stroke_in_the_missing_cell() {
        let stock = Rect::new(Coord { x: 0.0, y: 0.0 }, Coord { x: 4.0, y: 4.0 }).to_polygon();
        let pure = Rect::new(Coord { x: 0.0, y: 0.0 }, Coord { x: 2.0, y: 4.0 }).to_polygon();
        let base = MultiPolygon(vec![
            Rect::new(Coord { x: 2.0, y: 0.0 }, Coord { x: 4.0, y: 4.0 }).to_polygon(),
        ]);
        let cancel = AtomicBool::new(false);
        verify_grid(
            &base,
            std::slice::from_ref(&pure),
            &stock,
            true,
            &mut 0,
            &cancel,
        )
        .unwrap();
        assert!(
            verify_grid(
                &MultiPolygon(Vec::new()),
                &[pure],
                &stock,
                true,
                &mut 0,
                &cancel
            )
            .is_err()
        );
    }
}
