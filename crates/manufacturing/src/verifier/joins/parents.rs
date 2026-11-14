// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::{cancelled, charge, core_radius, failed, project_xyz6_point_mm, strict_capsule_point};
use geo::{Coord, Intersects, Line};
use spiling_contracts::manufacturing::*;
use std::{mem::size_of, sync::atomic::AtomicBool};

struct Stroke {
    layer: usize,
    physical_z: [f64; 2],
    path: usize,
    edge: usize,
    points: [[f64; 3]; 2],
    radius: f64,
    role: MaterialConstructionRole,
    parent: usize,
    seeds: u8,
}

pub(super) struct Parents(Vec<Stroke>);

impl Parents {
    pub(super) fn attached(&self, path: usize, edge: usize) -> bool {
        self.0
            .binary_search_by_key(&(path, edge), |stroke| (stroke.path, stroke.edge))
            .is_ok()
    }
}

fn resource() -> ManufacturingError {
    ManufacturingError::new(
        ManufacturingErrorCode::ResourceLimit,
        "functional local-parent stroke workspace exceeds its bound",
    )
}

fn window(frame: &FunctionalMaterialFrame, horizontal: [f64; 2], depth: [f64; 2]) -> [[f64; 2]; 2] {
    std::array::from_fn(|axis| {
        let values = if frame.outward_normal[axis] != 0.0 {
            depth.map(|value| frame.world_origin_mm[axis] - frame.outward_normal[axis] * value)
        } else {
            horizontal
                .map(|value| frame.world_origin_mm[axis] + frame.horizontal_direction[axis] * value)
        };
        [values[0].min(values[1]), values[0].max(values[1])]
    })
}

fn inward(value: f64, lower: bool) -> Result<f64, ManufacturingError> {
    let outside = |candidate| {
        if lower {
            candidate < value
        } else {
            candidate > value
        }
    };
    let mut projected = program_coordinates::project_xyz6_coordinate_mm(value)?;
    if outside(projected) {
        projected = program_coordinates::project_xyz6_coordinate_mm(
            projected + if lower { 0.000001 } else { -0.000001 },
        )?;
    }
    if outside(projected) {
        return Err(failed(
            "functional finite-window cut is not conditioned on XYZ6",
        ));
    }
    Ok(projected)
}

fn clip(
    mut points: [[f64; 3]; 2],
    window: [[f64; 2]; 2],
) -> Result<Option<[[f64; 3]; 2]>, ManufacturingError> {
    for axis in 0..2 {
        let low = points[0][axis].min(points[1][axis]).max(window[axis][0]);
        let high = points[0][axis].max(points[1][axis]).min(window[axis][1]);
        if low > high {
            return Ok(None);
        }
        if points[0][axis] == points[1][axis] {
            continue;
        }
        if points[0][1 - axis] != points[1][1 - axis] {
            if points
                .iter()
                .any(|point| point[axis] < window[axis][0] || point[axis] > window[axis][1])
            {
                return Err(failed(
                    "functional parent needs an unproved oblique finite-window intersection",
                ));
            }
            continue;
        }
        let low = inward(low, true)?;
        let high = inward(high, false)?;
        if low >= high {
            return Ok(None);
        }
        if points[0][axis] < points[1][axis] {
            points[0][axis] = low;
            points[1][axis] = high;
        } else {
            points[0][axis] = high;
            points[1][axis] = low;
        }
    }
    Ok((points[0][..2] != points[1][..2]).then_some(points))
}

fn root(
    strokes: &mut [Stroke],
    mut index: usize,
    work: &mut usize,
) -> Result<usize, ManufacturingError> {
    while strokes[index].parent != index {
        charge(work, 1)?;
        strokes[index].parent = strokes[strokes[index].parent].parent;
        index = strokes[index].parent;
    }
    Ok(index)
}

fn crossing(left: [[f64; 3]; 2], right: [[f64; 3]; 2]) -> bool {
    let line = |points: [[f64; 3]; 2]| {
        Line::new(
            Coord {
                x: points[0][0],
                y: points[0][1],
            },
            Coord {
                x: points[1][0],
                y: points[1][1],
            },
        )
    };
    line(left).intersects(&line(right))
}

fn connected(left: &Stroke, right: &Stroke) -> Result<bool, ManufacturingError> {
    if left.role != right.role {
        return Ok(false);
    }
    if left.layer == right.layer {
        if left.physical_z != right.physical_z || left.points[0][2] != right.points[0][2] {
            return Ok(false);
        }
    } else {
        let (lower, upper) = if left.layer < right.layer {
            (left, right)
        } else {
            (right, left)
        };
        if upper.layer - lower.layer != 1
            || lower.physical_z[1] != upper.physical_z[0]
            || lower.points[0][2]
                != program_coordinates::project_xyz6_coordinate_mm(upper.physical_z[0])?
        {
            return Ok(false);
        }
    }
    let radius = (left.radius + right.radius).next_down();
    if !radius.is_finite() || left.radius <= 0.0 || right.radius <= 0.0 || radius <= 0.0 {
        return Ok(false);
    }
    Ok(crossing(left.points, right.points)
        || strict_capsule_point(left.points[0], right.points, radius)?
        || strict_capsule_point(left.points[1], right.points, radius)?
        || strict_capsule_point(right.points[0], left.points, radius)?
        || strict_capsule_point(right.points[1], left.points, radius)?)
}

fn connect(
    strokes: &mut [Stroke],
    current_layer: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    for index in 0..strokes.len() {
        for other in 0..index {
            cancelled(cancel)?;
            charge(work, 1)?;
            if !connected(&strokes[index], &strokes[other])? {
                continue;
            }
            let left = root(strokes, index, work)?;
            let right = root(strokes, other, work)?;
            if left != right {
                strokes[right].parent = left;
                strokes[left].seeds |= strokes[right].seeds;
            }
        }
    }
    for index in 0..strokes.len() {
        cancelled(cancel)?;
        if strokes[index].layer != current_layer {
            continue;
        }
        let parent = root(strokes, index, work)?;
        let required = if strokes[index].role == MaterialConstructionRole::FunctionalBase {
            3
        } else {
            1
        };
        if strokes[parent].seeds != required {
            return Err(failed(format!(
                "functional local core connection is unproved: Base needs both finite source-exterior root wall runs; Region needs its pure parent strip; remote wall-loop continuity and deep endpoints grant no attachment (role {:?}, path {}, edge {}, points {:?}, seeds {})",
                strokes[index].role,
                strokes[index].path,
                strokes[index].edge,
                strokes[index].points,
                strokes[parent].seeds,
            )));
        }
    }
    Ok(())
}

pub(super) fn verify(
    plan: &NormalizedPrintPlan,
    layer_index: usize,
    frame: &FunctionalMaterialFrame,
    request: &FunctionalMaterialConstruction,
    recipe: &PlanarPrintRecipe,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Parents, ManufacturingError> {
    let layer = plan
        .layers
        .get(layer_index)
        .ok_or_else(|| failed("functional parent layer is absent"))?;
    if layer.lower_z_mm < frame.region_z_mm[0] || layer.z_mm > frame.region_z_mm[1] {
        return Err(failed(
            "functional parent layer straddles its source-bound region",
        ));
    }
    let previous = layer_index.checked_sub(1).filter(|&index| {
        let previous = &plan.layers[index];
        previous.z_mm == layer.lower_z_mm && previous.lower_z_mm >= frame.region_z_mm[0]
    });
    let next = layer_index.checked_add(1).filter(|&index| {
        plan.layers
            .get(index)
            .is_some_and(|next| next.lower_z_mm == layer.z_mm && next.z_mm <= frame.region_z_mm[1])
    });
    let layers = [Some(layer_index), previous, next];
    let depth_axis = usize::from(frame.outward_normal[1] != 0.0);
    let horizontal_axis = 1 - depth_axis;
    let total = request.minimum_pure_depth_mm
        + request.join.reach_into_region_mm
        + request.join.reach_into_base_mm;
    let root_depth = [total, total + request.minimum_solid_base_root_depth_mm];
    let base = window(
        frame,
        [0.0, frame.width_mm],
        [request.minimum_pure_depth_mm, root_depth[1]],
    );
    let region = window(frame, frame.selected_u_mm, [0.0, total]);
    let root_band = window(frame, [0.0, frame.width_mm], root_depth);
    let count = layers
        .iter()
        .flatten()
        .flat_map(|&index| &plan.layers[index].paths)
        .try_fold(0usize, |count, path| {
            count
                .checked_add(path.points_mm.len().saturating_sub(1))
                .ok_or_else(resource)
        })?;
    charge(
        work,
        plan.layers
            .iter()
            .map(|layer| layer.paths.len() + layer.region_coverage.process_regions.len() + 1)
            .sum(),
    )?;
    let retained = super::inventory::retained_bytes(plan)?;
    if count > crate::working_set::units()
        || count
            .checked_mul(size_of::<Stroke>())
            .and_then(|bytes| bytes.checked_add(retained))
            .is_none_or(|bytes| bytes as u64 > crate::working_set::bytes_u64())
    {
        return Err(resource());
    }
    let mut strokes = Vec::new();
    strokes.try_reserve_exact(count).map_err(|_| resource())?;
    let mut witnessed = 0u8;
    for index in layers.into_iter().flatten() {
        let layer = &plan.layers[index];
        let current = index == layer_index;
        let projected_z = [
            program_coordinates::project_xyz6_coordinate_mm(layer.lower_z_mm)?,
            program_coordinates::project_xyz6_coordinate_mm(layer.z_mm)?,
        ];
        if layer.lower_z_mm >= layer.z_mm || projected_z[0] >= projected_z[1] {
            return Err(failed(
                "functional parent has no positive closed physical interval",
            ));
        }
        for (path_index, path) in layer.paths.iter().enumerate() {
            let Some(reference) = path.material_domain else {
                continue;
            };
            let base_owned = reference.role == MaterialConstructionRole::FunctionalBase;
            if !base_owned && reference.role != MaterialConstructionRole::FunctionalRegion {
                continue;
            }
            if reference.construction_index != 0 {
                return Err(failed(
                    "functional local parent belongs to another construction",
                ));
            }
            if path.physical_z_mm != [layer.lower_z_mm, layer.z_mm] {
                return Err(failed(
                    "functional local parent has a different physical interval",
                ));
            }
            for (edge_index, edge) in path.checked_edges()?.enumerate() {
                cancelled(cancel)?;
                charge(work, 1)?;
                let projected = [
                    project_xyz6_point_mm(edge.points_mm[0])?,
                    project_xyz6_point_mm(edge.points_mm[1])?,
                ];
                if projected.iter().any(|point| point[2] != projected_z[1]) {
                    return Err(failed(
                        "functional parent command does not close its physical interval",
                    ));
                }
                let Some(points) = clip(projected, if base_owned { base } else { region })? else {
                    continue;
                };
                let radius = program_coordinates::enclosed_vertical_gap_mm(
                    core_radius(&edge, layer.z_mm - layer.lower_z_mm)?,
                    2.0 * program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM,
                )?;
                if radius <= 0.0 {
                    return Err(failed(
                        "functional finite-window core enclosure is not positive",
                    ));
                }
                let mut seeds = 0;
                if current && base_owned {
                    if path.owner == Some(DepositionOwner::Wall)
                        && projected[0][horizontal_axis] == projected[1][horizontal_axis]
                        && clip(projected, root_band)?.is_some()
                    {
                        for side in 0..2 {
                            let source = [
                                frame.stock_bounds_mm.min[horizontal_axis],
                                frame.stock_bounds_mm.max[horizontal_axis],
                            ][side];
                            let direction = if side == 0 { 1.0 } else { -1.0 };
                            let contour = program_coordinates::project_xyz6_coordinate_mm(
                                source + direction * recipe.bead_width_mm * 0.5,
                            )?;
                            if projected[0][horizontal_axis] == contour {
                                seeds |= 1 << side;
                            }
                        }
                        witnessed |= seeds;
                    }
                } else if current {
                    let pure_end = request.minimum_pure_depth_mm - edge.width_mm;
                    if pure_end > 0.0
                        && clip(points, window(frame, frame.selected_u_mm, [0.0, pure_end]))?
                            .is_some()
                    {
                        seeds = 1;
                        witnessed |= 4;
                    }
                }
                strokes.push(Stroke {
                    layer: index,
                    physical_z: path.physical_z_mm,
                    path: path_index,
                    edge: edge_index,
                    points,
                    radius,
                    role: reference.role,
                    parent: strokes.len(),
                    seeds,
                });
            }
        }
    }
    if witnessed != 7 {
        return Err(failed(
            "functional parent lacks finite Base-owned source wall runs at both U ends inside the root reserve or a pure Region parent strip",
        ));
    }
    connect(&mut strokes, layer_index, work, cancel)?;
    strokes.retain(|stroke| stroke.layer == layer_index);
    Ok(Parents(strokes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(points: [[f64; 3]; 2], parent: usize, seeds: u8) -> Stroke {
        Stroke {
            layer: 0,
            physical_z: [0.75, 1.0],
            path: 0,
            edge: parent,
            points,
            radius: 0.1,
            role: MaterialConstructionRole::FunctionalBase,
            parent,
            seeds,
        }
    }

    #[test]
    fn functional_sparse_parent_requires_both_local_source_wall_runs() {
        let mut strokes = [
            stroke([[0.0, 0.0, 1.0], [0.0, 1.0, 1.0]], 0, 1),
            stroke([[0.0, 0.5, 1.0], [2.0, 0.5, 1.0]], 1, 0),
            stroke([[2.0, 0.0, 1.0], [2.0, 1.0, 1.0]], 2, 2),
        ];
        connect(&mut strokes, 0, &mut 0, &AtomicBool::new(false)).unwrap();
        strokes[0].parent = 0;
        strokes[0].seeds = 1;
        strokes[1].parent = 1;
        strokes[1].seeds = 0;
        strokes[2].parent = 2;
        strokes[2].seeds = 0;
        assert!(connect(&mut strokes, 0, &mut 0, &AtomicBool::new(false)).is_err());
    }

    #[test]
    fn functional_sparse_parent_remote_same_path_loop_is_not_a_connection() {
        let mut strokes = [
            stroke([[0.0, 0.0, 1.0], [0.0, 1.0, 1.0]], 0, 1),
            stroke([[2.0, 0.0, 1.0], [2.0, 1.0, 1.0]], 1, 2),
        ];
        assert!(connect(&mut strokes, 0, &mut 0, &AtomicBool::new(false)).is_err());
        assert!(
            clip([[0.0, 5.0, 1.0], [2.0, 5.0, 1.0]], [[0.0, 2.0], [0.0, 1.0]])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn functional_sparse_parent_tangent_or_foreign_material_cannot_join_the_root() {
        for foreign in [false, true] {
            let mut strokes = [
                stroke([[0.0, 0.0, 1.0], [1.0, 0.0, 1.0]], 0, 1),
                stroke([[0.0, 0.2, 1.0], [1.0, 0.2, 1.0]], 1, 2),
            ];
            if foreign {
                strokes[1].points = strokes[0].points;
                strokes[1].role = MaterialConstructionRole::FunctionalRegion;
                strokes[1].seeds = 1;
            }
            assert!(connect(&mut strokes, 0, &mut 0, &AtomicBool::new(false)).is_err());
        }
    }

    #[test]
    fn functional_parent_interior_crossings_use_exact_projected_segments() {
        let left = stroke([[-2.0, 0.0, 1.0], [2.0, 0.0, 1.0]], 0, 1);
        let right = stroke([[0.0, -2.0, 1.0], [0.0, 2.0, 1.0]], 1, 2);
        assert!(connected(&left, &right).unwrap());
        assert!(crossing(
            [[-2.0, -2.0, 1.0], [2.0, 2.0, 1.0]],
            [[-2.0, 2.0, 1.0], [2.0, -2.0, 1.0]],
        ));
        assert!(!crossing(
            [[0.0, 0.0, 1.0], [4_000_000_000.0, 4_000_000_000.0, 1.0]],
            [
                [0.0, 0.000001, 1.0],
                [4_000_000_000.0, 4_000_000_000.000001, 1.0]
            ],
        ));
    }

    fn adjacent_stroke() -> Stroke {
        let mut next = stroke([[0.0, -2.0, 1.25], [0.0, 2.0, 1.25]], 1, 0);
        next.layer = 1;
        next.physical_z = [1.0, 1.25];
        next
    }

    #[test]
    fn functional_parent_cross_layer_bond_requires_a_complete_closed_interface() {
        let current = stroke([[-2.0, 0.0, 1.0], [2.0, 0.0, 1.0]], 0, 0);
        let next = adjacent_stroke();
        assert!(connected(&current, &next).unwrap());
        assert!(connected(&next, &current).unwrap());
        for interval in [[1.000001, 1.25], [0.999999, 1.25]] {
            let mut next = adjacent_stroke();
            next.physical_z = interval;
            assert!(!connected(&current, &next).unwrap());
        }
        let mut nonadjacent = adjacent_stroke();
        nonadjacent.layer = 2;
        assert!(!connected(&current, &nonadjacent).unwrap());
        let mut foreign = adjacent_stroke();
        foreign.role = MaterialConstructionRole::FunctionalRegion;
        assert!(!connected(&current, &foreign).unwrap());
        let mut tangent = adjacent_stroke();
        tangent.points = [[-2.0, 0.2, 1.25], [2.0, 0.2, 1.25]];
        assert!(!connected(&current, &tangent).unwrap());
        let mut disconnected = adjacent_stroke();
        disconnected.points = [[-2.0, 0.3, 1.25], [2.0, 0.3, 1.25]];
        assert!(!connected(&current, &disconnected).unwrap());
        let mut zero_core = adjacent_stroke();
        zero_core.radius = 0.0;
        assert!(!connected(&current, &zero_core).unwrap());
    }

    fn parent_fixture() -> (
        NormalizedPrintPlan,
        FunctionalMaterialFrame,
        FunctionalMaterialConstruction,
        PlanarPrintRecipe,
    ) {
        use spiling_contracts::geometry::{AabbMm, DefinitionId, FaceId, OccurrenceId, SourceHash};
        let (intent, mut plan) = super::super::super::tests::fixture();
        let mut recipe = intent.recipe;
        recipe.bead_width_mm = 0.5;
        let occurrence_id = OccurrenceId::new(1).unwrap();
        let definition_id = DefinitionId::from_source_sha256(&[0; 32]);
        let source_hash = SourceHash::from_bytes(b"finite parent fixture");
        let face_id = FaceId::parse("step:1").unwrap();
        let request = FunctionalMaterialConstruction {
            interface_policy: FunctionalInterfacePolicy::MechanicalCapture,
            occurrence_id,
            definition_id: definition_id.clone(),
            source_hash: source_hash.clone(),
            face_id: face_id.clone(),
            face_patch: Some(FunctionalFacePatch {
                horizontal_mm: [2.0, 8.0],
                source_z_mm: [0.0, 3.0],
            }),
            material_id: "region".into(),
            base_material_id: "base".into(),
            minimum_pure_depth_mm: 3.0,
            minimum_solid_base_root_depth_mm: 4.0,
            join: PlanarTKeyJoin {
                reach_into_region_mm: 1.0,
                reach_into_base_mm: 1.0,
                pitch_mm: 6.0,
                stem_width_mm: 1.0,
                head_width_mm: 3.0,
                head_depth_mm: 1.0,
                edge_margin_mm: 1.0,
                end_collar_height_mm: 0.5,
                collar_head_width_mm: 2.0,
            },
        };
        let frame = FunctionalMaterialFrame {
            occurrence_id,
            definition_id,
            source_hash,
            face_id,
            origin_mm: [0.0; 3],
            world_origin_mm: [0.0; 3],
            stock_bounds_mm: AabbMm::new([0.0; 3], [10.0, 12.0, 3.0]).unwrap(),
            horizontal_direction: [1.0, 0.0, 0.0],
            outward_normal: [0.0, -1.0, 0.0],
            width_mm: 10.0,
            source_z_mm: [0.0, 3.0],
            selected_u_mm: [2.0, 8.0],
            region_z_mm: [0.0, 3.0],
            key_middle_z_mm: [0.5, 2.5],
            key_centers_u_mm: vec![5.0],
            solid_base_zones: Vec::new(),
        };
        let path = |points, role, wall, interval: [f64; 2]| DepositionPath {
            material_domain: Some(MaterialDomainRef {
                construction_index: 0,
                role,
            }),
            kind: if wall {
                DepositionKind::Perimeter
            } else {
                DepositionKind::SolidFill
            },
            owner: wall.then_some(DepositionOwner::Wall),
            width: WidthProfile::Constant { width_mm: 0.5 },
            physical_z_mm: interval,
            points_mm: points,
            arcs: Vec::new(),
            external_wall: false,
        };
        let base = MaterialConstructionRole::FunctionalBase;
        let region = MaterialConstructionRole::FunctionalRegion;
        for (index, layer) in plan.layers.iter_mut().enumerate() {
            [layer.lower_z_mm, layer.z_mm] = [[1.4, 1.6], [1.6, 1.8]][index];
            layer.height_mm = layer.z_mm - layer.lower_z_mm;
            layer.section_z_mm = (layer.lower_z_mm + layer.z_mm) * 0.5;
        }
        let top_z = plan.layers[0].z_mm;
        let interval = [plan.layers[0].lower_z_mm, top_z];
        plan.layers[0].paths = vec![
            path(
                vec![[0.25, 3.1, top_z], [0.25, 8.9, top_z]],
                base,
                true,
                interval,
            ),
            path(
                vec![[9.75, 3.1, top_z], [9.75, 8.9, top_z]],
                base,
                true,
                interval,
            ),
            path(
                vec![[4.0, 3.1, top_z], [4.0, 8.9, top_z]],
                base,
                false,
                interval,
            ),
            path(
                vec![[6.0, 3.1, top_z], [6.0, 8.9, top_z]],
                base,
                false,
                interval,
            ),
            path(
                vec![[2.5, 0.25, top_z], [7.5, 0.25, top_z]],
                region,
                false,
                interval,
            ),
        ];
        let top_z = plan.layers[1].z_mm;
        let interval = [plan.layers[1].lower_z_mm, top_z];
        plan.layers[1].paths = vec![
            path(
                vec![
                    [0.25, 6.0, top_z],
                    [1.0, 6.0, top_z],
                    [9.0, 6.0, top_z],
                    [9.75, 6.0, top_z],
                ],
                base,
                false,
                interval,
            ),
            path(
                vec![[3.0, 2.0, top_z], [7.0, 2.0, top_z]],
                region,
                false,
                interval,
            ),
        ];
        (plan, frame, request, recipe)
    }

    #[test]
    fn functional_parent_adjacent_orthogonal_rows_join_only_current_receipts() {
        let (plan, frame, request, recipe) = parent_fixture();
        let parents = verify(
            &plan,
            0,
            &frame,
            &request,
            &recipe,
            &mut 0,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(parents.0.len(), plan.layers[0].paths.len());
        assert!(parents.attached(2, 0));
        assert!(
            !parents.attached(0, 1),
            "a neighbor edge is not a current receipt"
        );
        let mut isolated = plan.clone();
        isolated.layers.truncate(1);
        assert!(
            verify(
                &isolated,
                0,
                &frame,
                &request,
                &recipe,
                &mut 0,
                &AtomicBool::new(false)
            )
            .is_err()
        );

        let mut reversed = plan;
        let old_current = reversed.layers.remove(0);
        reversed.layers.push(old_current);
        for (index, layer) in reversed.layers.iter_mut().enumerate() {
            [layer.lower_z_mm, layer.z_mm] = [[1.4, 1.6], [1.6, 1.8]][index];
            layer.height_mm = layer.z_mm - layer.lower_z_mm;
            layer.section_z_mm = (layer.lower_z_mm + layer.z_mm) * 0.5;
            for path in &mut layer.paths {
                path.physical_z_mm = [layer.lower_z_mm, layer.z_mm];
                for point in &mut path.points_mm {
                    point[2] = layer.z_mm;
                }
            }
        }
        verify(
            &reversed,
            1,
            &frame,
            &request,
            &recipe,
            &mut 0,
            &AtomicBool::new(false),
        )
        .unwrap();
    }

    #[test]
    fn functional_parent_neighbor_cannot_replace_current_source_seeds() {
        for missing in [0, 1, 4] {
            let (mut plan, frame, request, recipe) = parent_fixture();
            let mut moved = plan.layers[0].paths.remove(missing);
            moved.physical_z_mm = [plan.layers[1].lower_z_mm, plan.layers[1].z_mm];
            for point in &mut moved.points_mm {
                point[2] = plan.layers[1].z_mm;
            }
            plan.layers[1].paths.push(moved);
            assert!(
                verify(
                    &plan,
                    0,
                    &frame,
                    &request,
                    &recipe,
                    &mut 0,
                    &AtomicBool::new(false)
                )
                .is_err()
            );
        }
    }

    #[test]
    fn functional_parent_neighbor_cannot_borrow_a_remote_wall_loop() {
        let (mut plan, frame, request, recipe) = parent_fixture();
        let top_z = plan.layers[1].z_mm;
        plan.layers[1].paths[0].points_mm = vec![
            [0.25, 8.0, top_z],
            [0.25, 10.0, top_z],
            [9.75, 10.0, top_z],
            [9.75, 8.0, top_z],
        ];
        assert!(
            verify(
                &plan,
                0,
                &frame,
                &request,
                &recipe,
                &mut 0,
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }

    #[test]
    fn functional_parent_neighbor_requires_complete_scoped_command_interfaces() {
        for mutation in 0..6 {
            let (mut plan, mut frame, request, recipe) = parent_fixture();
            match mutation {
                0 => plan.layers[1].lower_z_mm += 0.000001,
                1 => plan.layers[1].lower_z_mm -= 0.000001,
                2 => plan.layers[1].paths[0].physical_z_mm[0] += 0.000001,
                3 => plan.layers[1].paths[0].points_mm[0][2] += 0.000001,
                4 => frame.region_z_mm[1] = plan.layers[0].z_mm,
                5 => {
                    plan.layers[1].paths[0]
                        .material_domain
                        .as_mut()
                        .unwrap()
                        .construction_index = 1
                }
                _ => unreachable!(),
            }
            assert!(
                verify(
                    &plan,
                    0,
                    &frame,
                    &request,
                    &recipe,
                    &mut 0,
                    &AtomicBool::new(false)
                )
                .is_err()
            );
        }
    }

    #[test]
    fn functional_parent_every_current_stroke_still_needs_its_own_parent() {
        let (mut plan, frame, request, recipe) = parent_fixture();
        let mut detached = plan.layers[0].paths[2].clone();
        let top_z = plan.layers[0].z_mm;
        detached.points_mm = vec![[5.0, 3.1, top_z], [5.0, 4.5, top_z]];
        plan.layers[0].paths.push(detached);
        assert!(
            verify(
                &plan,
                0,
                &frame,
                &request,
                &recipe,
                &mut 0,
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }

    #[test]
    fn functional_parent_region_cannot_borrow_neighbor_continuity_outside_selected_u() {
        let (mut plan, frame, request, recipe) = parent_fixture();
        let mut detached = plan.layers[0].paths[4].clone();
        let top_z = plan.layers[0].z_mm;
        detached.points_mm = vec![[3.5, 4.5, top_z], [6.5, 4.5, top_z]];
        plan.layers[0].paths.push(detached);
        let top_z = plan.layers[1].z_mm;
        plan.layers[1].paths[1].points_mm = vec![
            [4.0, 4.5, top_z],
            [1.0, 4.5, top_z],
            [1.0, 0.25, top_z],
            [4.0, 0.25, top_z],
        ];
        assert!(
            verify(
                &plan,
                0,
                &frame,
                &request,
                &recipe,
                &mut 0,
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }

    #[test]
    fn functional_parent_window_cuts_stay_inside_the_finite_xyz6_belt() {
        let points = clip(
            [[0.0, 0.0, 1.0], [0.0, 10.0, 1.0]],
            [[-1.0, 1.0], [5.0000004, 8.9999996]],
        )
        .unwrap()
        .unwrap();
        assert_eq!(points, [[0.0, 5.000001, 1.0], [0.0, 8.999999, 1.0]]);
    }
}
