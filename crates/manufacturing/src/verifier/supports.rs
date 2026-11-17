// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::{cancelled, failed};
use crate::planner::support::{
    AnchorCandidate, AnchorWorkspace, GapKind, ProjectedAnchorPaths, StrokeRuns, gap_kind,
};
use spiling_contracts::manufacturing::{program_coordinates::*, *};
use std::sync::atomic::AtomicBool;

fn charge(work: &mut usize, cancel: &AtomicBool) -> Result<(), ManufacturingError> {
    cancelled(cancel)?;
    *work = work.checked_add(1).ok_or_else(|| {
        ManufacturingError::new(
            ManufacturingErrorCode::ResourceLimit,
            "support replay work overflow",
        )
    })?;
    Ok(())
}

fn support(path: &DepositionPath) -> bool {
    matches!(
        path.kind,
        DepositionKind::Support | DepositionKind::SupportInterface
    )
}

fn height(path: &DepositionPath) -> Result<[f64; 2], ManufacturingError> {
    project_xyz6_bead_height_mm(path.physical_z_mm[1], path.height_mm())
}

#[expect(
    clippy::too_many_arguments,
    reason = "Optional scope-local metadata preserves the supplied previous and contact iterators"
)]
fn anchored<'a>(
    path: &DepositionPath,
    previous: impl Iterator<Item = &'a DepositionPath> + Clone,
    contacts: impl Iterator<Item = &'a DepositionPath> + Clone,
    prepared: Option<(&ProjectedAnchorPaths<'a>, bool)>,
    recipe: &PlanarPrintRecipe,
    work: &mut usize,
    anchors: &mut AnchorWorkspace,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    for (first, last, width) in StrokeRuns::for_path(path)? {
        charge(work, cancel)?;
        let endpoints = [path.points_mm[first], path.points_mm[last]];
        let projected = [
            project_xyz6_point_mm(endpoints[0])?,
            project_xyz6_point_mm(endpoints[1])?,
        ];
        let length = (projected[1][0] - projected[0][0]).hypot(projected[1][1] - projected[0][1]);
        let candidate = AnchorCandidate {
            endpoints_mm: projected,
            bead_width_mm: width,
        };
        let gaps = if let Some((prepared, with_contacts)) = prepared {
            anchors.gaps_reused_projected(
                candidate,
                path.physical_z_mm[0],
                with_contacts,
                prepared,
                work,
                cancel,
            )?
        } else {
            anchors.gaps_projected(
                candidate,
                previous
                    .clone()
                    .filter(|lower| lower.physical_z_mm[1] == path.physical_z_mm[0]),
                contacts.clone(),
                work,
                cancel,
            )?
        };
        for gap in gaps {
            let kind = gap_kind(
                gap,
                (gap.end - gap.start) * length,
                width,
                recipe.process.max_bridge_span_mm,
            );
            // Sparse infill and wall connectors keep the one short-span rule:
            // they may span lower strokes within the bridge limit, or run
            // unanchored for at most three bead widths, and still root what
            // rests on them. A road joined by a connector is judged as a road.
            let sparse = path.kind == DepositionKind::SparseFill
                || (first..last).all(|edge| path.is_connector_edge(edge));
            let span = (gap.end - gap.start) * length;
            if matches!(kind, GapKind::Demand)
                && !(sparse && span <= MAX_SPARSE_UNSUPPORTED_RUN_WIDTHS * width)
                || (matches!(kind, GapKind::Bridge)
                    && path.kind.is_model()
                    && path.kind != DepositionKind::Bridge
                    && !sparse)
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

struct LayerBounds {
    index: usize,
    interval: [f64; 2],
    prefix_upper: f64,
}

fn edge_bounds(points: &[[f64; 3]]) -> [[f64; 2]; 2] {
    [
        [
            points[0][0].min(points[1][0]),
            points[0][1].min(points[1][1]),
        ],
        [
            points[0][0].max(points[1][0]),
            points[0][1].max(points[1][1]),
        ],
    ]
}

fn path_bounds(
    path: &DepositionPath,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<([[f64; 2]; 2], f64), ManufacturingError> {
    let width = match &path.width {
        WidthProfile::Constant { width_mm } => *width_mm,
        WidthProfile::PerEdge { widths_mm } => widths_mm.iter().copied().fold(0.0, f64::max),
    };
    let mut bounds = [[f64::INFINITY; 2], [f64::NEG_INFINITY; 2]];
    for point in &path.points_mm {
        charge(work, cancel)?;
        for axis in 0..2 {
            bounds[0][axis] = bounds[0][axis].min(point[axis]);
            bounds[1][axis] = bounds[1][axis].max(point[axis]);
        }
    }
    Ok((bounds, width))
}

fn reserve<T>(
    values: &mut Vec<T>,
    additional: usize,
    live: usize,
) -> Result<(), ManufacturingError> {
    let needed = values.len().checked_add(additional).ok_or_else(|| {
        ManufacturingError::new(
            ManufacturingErrorCode::ResourceLimit,
            "support replay capacity overflow",
        )
    })?;
    if needed <= values.capacity() {
        return Ok(());
    }
    let capacity = needed.max(values.capacity().saturating_mul(2));
    capacity
        .checked_add(values.capacity())
        .and_then(|count| count.checked_mul(std::mem::size_of::<T>()))
        .and_then(|bytes| live.checked_add(bytes))
        .filter(|bytes| *bytes <= crate::working_set::bytes())
        .ok_or_else(|| {
            ManufacturingError::new(
                ManufacturingErrorCode::ResourceLimit,
                "support replay working set exceeds live byte allowance",
            )
        })?;
    values
        .try_reserve_exact(capacity - values.len())
        .map_err(|_| {
            ManufacturingError::new(
                ManufacturingErrorCode::ResourceLimit,
                "support replay allocation refused",
            )
        })
}

fn first_unseparated(
    layers: &[PrintLayer],
    floor: f64,
    gap: f64,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<usize, ManufacturingError> {
    let (mut start, mut end) = (0, layers.len());
    while start < end {
        charge(work, cancel)?;
        let middle = start + (end - start) / 2;
        let layer = &layers[middle];
        let top = project_xyz6_bead_height_mm(layer.z_mm, layer.height_mm)?[1];
        if enclosed_vertical_gap_mm(floor, top)? >= gap {
            start = middle + 1;
        } else {
            end = middle;
        }
    }
    Ok(start)
}

fn capsule(
    edge: &[[f64; 3]],
    width: f64,
) -> Result<spiling_geometry::PlanarCapsuleMm, ManufacturingError> {
    let points = [
        project_xyz6_point_mm(edge[0])?,
        project_xyz6_point_mm(edge[1])?,
    ];
    let mut endpoint_error = 0.0_f64;
    for point in points {
        let error = xyz6_coordinate_error_mm(point[0])?.hypot(xyz6_coordinate_error_mm(point[1])?);
        endpoint_error = endpoint_error.max(if error == 0.0 { 0.0 } else { error.next_up() });
    }
    Ok(spiling_geometry::PlanarCapsuleMm {
        segment_xy_mm: points.map(|point| [point[0], point[1]]),
        radius_mm: width * 0.5,
        endpoint_error_mm: endpoint_error,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "On-demand replay ancestry admits masks beside retained contacts and shared anchor scratch"
)]
fn establish_until(
    layers: &[PrintLayer],
    established: &mut Vec<Vec<bool>>,
    recipe: &PlanarPrintRecipe,
    gap: f64,
    live: &mut usize,
    contact_bytes: usize,
    work: &mut usize,
    anchors: &mut AnchorWorkspace,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    while established.len() < layers.len() {
        let index = established.len();
        let layer = &layers[index];
        let mut contact_top = None;
        for (lower_index, lower) in layers[..index].iter().enumerate().rev() {
            charge(work, cancel)?;
            let top = project_xyz6_bead_height_mm(lower.z_mm, lower.height_mm)?[1];
            let floor = project_xyz6_bead_height_mm(layer.z_mm, layer.height_mm)?[0];
            if enclosed_vertical_gap_mm(floor, top)? >= gap {
                contact_top = Some(lower_index);
                break;
            }
        }
        let mut valid = Vec::new();
        reserve(
            &mut valid,
            layer.paths.len(),
            *live + contact_bytes + anchors.retained_bytes(),
        )?;
        anchors.admit_beside(*live + valid.capacity() + contact_bytes)?;
        let owned_bytes = *live + valid.capacity() + contact_bytes;
        let prepared = if index == 0 {
            None
        } else {
            ProjectedAnchorPaths::build(
                layers[index - 1]
                    .paths
                    .iter()
                    .zip(&established[index - 1])
                    .filter_map(|(path, valid)| (support(path) || *valid).then_some(path)),
                contact_top.into_iter().flat_map(|top| {
                    layers[top]
                        .paths
                        .iter()
                        .filter(|path| path.kind == DepositionKind::SupportInterface)
                }),
                anchors,
                owned_bytes,
                work,
                cancel,
            )?
        };
        anchors.admit_beside(
            owned_bytes
                + prepared
                    .as_ref()
                    .map_or(0, ProjectedAnchorPaths::retained_bytes),
        )?;
        for path in &layer.paths {
            charge(work, cancel)?;
            let rooted = path.kind.is_model()
                && (index == 0
                    || anchored(
                        path,
                        layers[index - 1]
                            .paths
                            .iter()
                            .zip(&established[index - 1])
                            .filter_map(|(path, valid)| (support(path) || *valid).then_some(path)),
                        contact_top.into_iter().flat_map(|top| {
                            layers[top]
                                .paths
                                .iter()
                                .filter(|path| path.kind == DepositionKind::SupportInterface)
                        }),
                        prepared.as_ref().map(|prepared| (prepared, true)),
                        recipe,
                        work,
                        anchors,
                        cancel,
                    )?);
            valid.push(rooted);
        }
        drop(prepared);
        *live += valid.capacity();
        established.push(valid);
    }
    Ok(())
}

pub(super) fn verify(
    plan: &NormalizedPrintPlan,
    recipe: &PlanarPrintRecipe,
    work: &mut usize,
    anchors: &mut AnchorWorkspace,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    if !plan
        .layers
        .iter()
        .flat_map(|layer| &layer.paths)
        .any(support)
    {
        return Ok(());
    }
    let SupportPolicy::Breakaway {
        z_gap_mm,
        xy_clearance_mm,
        ..
    } = recipe.supports
    else {
        return Err(failed("actual support requires breakaway policy"));
    };
    let mut native_work = spiling_geometry::GeometryWorkBudget::monitored();
    let last_support = plan
        .layers
        .iter()
        .rposition(|layer| layer.paths.iter().any(support))
        .unwrap_or(0);
    let mut bounds = Vec::new();
    reserve(&mut bounds, plan.layers.len(), anchors.retained_bytes())?;
    for (index, layer) in plan.layers.iter().enumerate() {
        let mut interval = [f64::INFINITY, f64::NEG_INFINITY];
        for path in layer.paths.iter().filter(|path| path.kind.is_model()) {
            charge(work, cancel)?;
            let actual = height(path)?;
            interval[0] = interval[0].min(actual[0]);
            interval[1] = interval[1].max(actual[1]);
        }
        if interval[0].is_finite() {
            bounds.push(LayerBounds {
                index,
                interval,
                prefix_upper: interval[1],
            });
        }
    }
    bounds.sort_unstable_by(|a, b| {
        a.interval[0]
            .total_cmp(&b.interval[0])
            .then(a.index.cmp(&b.index))
    });
    let mut upper = f64::NEG_INFINITY;
    for bound in &mut bounds {
        upper = upper.max(bound.interval[1]);
        bound.prefix_upper = upper;
    }
    let mut established: Vec<Vec<bool>> = Vec::new();
    let mut live = bounds.capacity() * std::mem::size_of::<LayerBounds>();
    reserve(
        &mut established,
        last_support + 1,
        live + anchors.retained_bytes(),
    )?;
    live += established.capacity() * std::mem::size_of::<Vec<bool>>();
    let mut contacts = Vec::new();
    for (index, layer) in plan.layers.iter().enumerate() {
        let mut contact_floor = None;
        let mut prepared = None;
        let mut prepared_attempted = false;
        for path in layer.paths.iter().filter(|path| support(path)) {
            charge(work, cancel)?;
            let support_height = height(path)?;
            let support_edges = path.checked_edges()?;
            let (support_bounds, support_width) = path_bounds(path, work, cancel)?;
            anchors.admit_beside(
                live + contacts.capacity() * std::mem::size_of::<&DepositionPath>(),
            )?;
            if index != 0 && !prepared_attempted {
                prepared = ProjectedAnchorPaths::build(
                    plan.layers[index - 1]
                        .paths
                        .iter()
                        .filter(|path| support(path)),
                    contacts.iter().copied().filter(|_| contact_floor.is_some()),
                    anchors,
                    live + contacts.capacity() * std::mem::size_of::<&DepositionPath>(),
                    work,
                    cancel,
                )?;
                prepared_attempted = true;
            }
            anchors.admit_beside(
                live + contacts.capacity() * std::mem::size_of::<&DepositionPath>()
                    + prepared
                        .as_ref()
                        .map_or(0, ProjectedAnchorPaths::retained_bytes),
            )?;
            let continuing = index == 0
                || anchored(
                    path,
                    plan.layers[index - 1]
                        .paths
                        .iter()
                        .filter(|path| support(path)),
                    std::iter::empty(),
                    prepared.as_ref().map(|prepared| (prepared, false)),
                    recipe,
                    work,
                    anchors,
                    cancel,
                )?;
            if !continuing
                && path.kind == DepositionKind::SupportInterface
                && contact_floor != Some(support_height[0].to_bits())
            {
                drop(prepared.take());
                prepared_attempted = false;
                establish_until(
                    &plan.layers[..index],
                    &mut established,
                    recipe,
                    z_gap_mm,
                    &mut live,
                    contacts.capacity() * std::mem::size_of::<&DepositionPath>(),
                    work,
                    anchors,
                    cancel,
                )?;
                contacts.clear();
                contact_floor = Some(support_height[0].to_bits());
                let end = first_unseparated(
                    &plan.layers[..index],
                    support_height[0],
                    z_gap_mm,
                    work,
                    cancel,
                )?;
                let start = if index > 0 {
                    let previous = &plan.layers[index - 1];
                    let floor = project_xyz6_bead_height_mm(previous.z_mm, previous.height_mm)?[0];
                    first_unseparated(
                        &plan.layers[..end.min(index.saturating_sub(1))],
                        floor,
                        z_gap_mm,
                        work,
                        cancel,
                    )?
                } else {
                    0
                };
                for (offset, lower) in plan.layers[start..end].iter().enumerate() {
                    let lower_index = start + offset;
                    for object in lower
                        .paths
                        .iter()
                        .zip(&established[lower_index])
                        .filter_map(|(path, valid)| {
                            (path.kind.is_model() && *valid).then_some(path)
                        })
                    {
                        charge(work, cancel)?;
                        let top = height(object)?[1];
                        if enclosed_vertical_gap_mm(support_height[0], top)? < z_gap_mm {
                            continue;
                        }
                        if contacts.len() == contacts.capacity() {
                            let needed = contacts
                                .len()
                                .checked_add(1)
                                .filter(|count| {
                                    count
                                        .saturating_mul(std::mem::size_of::<&DepositionPath>())
                                        .saturating_add(anchors.retained_bytes())
                                        <= crate::working_set::bytes()
                                })
                                .ok_or_else(|| {
                                    ManufacturingError::new(
                                        ManufacturingErrorCode::ResourceLimit,
                                        "support replay contacts exceed live byte allowance",
                                    )
                                })?;
                            let additional = needed - contacts.len();
                            reserve(&mut contacts, additional, live + anchors.retained_bytes())?;
                        }
                        contacts.push(object);
                    }
                }
            }
            anchors.admit_beside(
                live + contacts.capacity() * std::mem::size_of::<&DepositionPath>(),
            )?;
            if index != 0 && !prepared_attempted {
                prepared = ProjectedAnchorPaths::build(
                    plan.layers[index - 1]
                        .paths
                        .iter()
                        .filter(|path| support(path)),
                    contacts.iter().copied().filter(|_| contact_floor.is_some()),
                    anchors,
                    live + contacts.capacity() * std::mem::size_of::<&DepositionPath>(),
                    work,
                    cancel,
                )?;
                prepared_attempted = true;
            }
            anchors.admit_beside(
                live + contacts.capacity() * std::mem::size_of::<&DepositionPath>()
                    + prepared
                        .as_ref()
                        .map_or(0, ProjectedAnchorPaths::retained_bytes),
            )?;
            if !continuing
                && !anchored(
                    path,
                    plan.layers[index - 1]
                        .paths
                        .iter()
                        .filter(|path| support(path)),
                    contacts
                        .iter()
                        .copied()
                        .filter(|_| path.kind == DepositionKind::SupportInterface),
                    prepared
                        .as_ref()
                        .map(|prepared| (prepared, path.kind == DepositionKind::SupportInterface)),
                    recipe,
                    work,
                    anchors,
                    cancel,
                )?
            {
                return Err(failed(format!(
                    "actual support layer {index} has no established bed, continuing support or earlier-part contact"
                )));
            }
            let end = bounds.partition_point(|bound| {
                bound.interval[0] <= (support_height[1] + z_gap_mm).next_up()
            });
            let start = bounds[..end].partition_point(|bound| {
                bound.prefix_upper < (support_height[0] - z_gap_mm).next_down()
            });
            for bound in &bounds[start..end] {
                charge(work, cancel)?;
                if enclosed_vertical_gap_mm(bound.interval[0], support_height[1])? >= z_gap_mm
                    || enclosed_vertical_gap_mm(support_height[0], bound.interval[1])? >= z_gap_mm
                {
                    continue;
                }
                let object_layer = &plan.layers[bound.index];
                for object in object_layer
                    .paths
                    .iter()
                    .filter(|path| path.kind.is_model())
                {
                    charge(work, cancel)?;
                    let object_height = height(object)?;
                    if enclosed_vertical_gap_mm(object_height[0], support_height[1])? >= z_gap_mm
                        || enclosed_vertical_gap_mm(support_height[0], object_height[1])?
                            >= z_gap_mm
                    {
                        continue;
                    }
                    let object_edges = object.checked_edges()?;
                    let (object_bounds, object_width) = path_bounds(object, work, cancel)?;
                    if spiling_geometry::planar_bounds_clear(
                        support_bounds,
                        object_bounds,
                        (support_width + object_width) * 0.5 + 2.0 * XYZ6_PROJECTION_SCREEN_MM,
                        xy_clearance_mm,
                    ) {
                        continue;
                    }
                    for support_edge in support_edges {
                        let support_capsule =
                            capsule(support_edge.points_mm, support_edge.width_mm)?;
                        for object_edge in object_edges {
                            charge(work, cancel)?;
                            if spiling_geometry::planar_bounds_clear(
                                edge_bounds(support_edge.points_mm),
                                edge_bounds(object_edge.points_mm),
                                (support_edge.width_mm + object_edge.width_mm) * 0.5
                                    + 2.0 * XYZ6_PROJECTION_SCREEN_MM,
                                xy_clearance_mm,
                            ) {
                                continue;
                            }
                            let object_capsule =
                                capsule(object_edge.points_mm, object_edge.width_mm)?;
                            let bounds = |segment: [[f64; 2]; 2]| {
                                [
                                    [
                                        segment[0][0].min(segment[1][0]),
                                        segment[0][1].min(segment[1][1]),
                                    ],
                                    [
                                        segment[0][0].max(segment[1][0]),
                                        segment[0][1].max(segment[1][1]),
                                    ],
                                ]
                            };
                            let reach = ((support_capsule.radius_mm + object_capsule.radius_mm)
                                .next_up()
                                + support_capsule.endpoint_error_mm)
                                .next_up();
                            let reach = (reach + object_capsule.endpoint_error_mm).next_up();
                            if spiling_geometry::planar_bounds_clear(
                                bounds(support_capsule.segment_xy_mm),
                                bounds(object_capsule.segment_xy_mm),
                                reach,
                                xy_clearance_mm,
                            ) {
                                continue;
                            }
                            spiling_geometry::certify_planar_capsule_pair_clearance([
                                support_capsule,
                                object_capsule,
                            ], xy_clearance_mm, &mut native_work, cancel).map_err(|failure| failed(format!("actual support layer {index} violates object XY/Z clearance: {failure:?}")))?;
                        }
                    }
                }
            }
        }
        drop(prepared);
        if index > last_support {
            break;
        }
    }
    anchors.set_budget(crate::working_set::units());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (ManufacturingIntent, NormalizedPrintPlan) {
        let (mut intent, mut plan) = crate::verifier::tests::fixture();
        intent.printer.capabilities.generated_supports = true;
        intent.recipe.supports = SupportPolicy::Breakaway {
            density: 0.5,
            xy_clearance_mm: 0.25,
            z_gap_mm: 0.5,
            interface_thickness_mm: 1.0,
        };
        let template = plan.layers[0].clone();
        plan.layers = (0..5)
            .map(|index| {
                let mut layer = template.clone();
                layer.index = index;
                layer.hatch_course = index;
                layer.lower_z_mm = index as f64;
                layer.z_mm = index as f64 + 1.0;
                layer.height_mm = 1.0;
                layer.section_z_mm = index as f64 + 0.5;
                layer.paths.clear();
                layer.region_coverage.section_area_mm2 = 0.0;
                layer.region_coverage.source_section_area_mm2 = 0.0;
                layer.region_coverage.wall_area_mm2 = 0.0;
                layer.region_coverage.nominal_deposition_volume_mm3 = 0.0;
                if index.is_multiple_of(2) {
                    layer.paths.push(DepositionPath {
                        material_domain: None,
                        kind: if index == 2 {
                            DepositionKind::SupportInterface
                        } else {
                            DepositionKind::BottomSkin
                        },
                        width: WidthProfile::Constant { width_mm: 1.0 },
                        physical_z_mm: [layer.lower_z_mm, layer.z_mm],
                        owner: (index != 2).then_some(DepositionOwner::MandatorySkin {
                            skin: ProcessSkinKind::BottomSkin,
                        }),
                        points_mm: vec![[1.0, 1.0, layer.z_mm], [5.0, 1.0, layer.z_mm]],
                        arcs: Vec::new(),
                        exposed: false,
                        connectors: Vec::new(),
                    });
                    if index == 2 {
                        layer.region_coverage.support_area_mm2 = 4.0;
                    } else {
                        layer.region_coverage.section_area_mm2 = 4.0;
                        layer.region_coverage.source_section_area_mm2 = 4.0;
                        layer.region_coverage.bottom_skin_area_mm2 = 4.0;
                    }
                }
                layer
            })
            .collect();
        crate::verifier::tests::refresh_volumes(&intent, &mut plan);
        (intent, plan)
    }

    fn check(
        intent: &ManufacturingIntent,
        plan: &NormalizedPrintPlan,
    ) -> Result<(), ManufacturingError> {
        verify(
            plan,
            &intent.recipe,
            &mut 0,
            &mut AnchorWorkspace::default(),
            &AtomicBool::new(false),
        )
    }

    #[test]
    fn projected_scope_matches_ordered_anchored_roles_and_malformed_errors() {
        let (intent, plan) = fixture();
        let cancel = AtomicBool::new(false);
        for kind in [
            DepositionKind::BottomSkin,
            DepositionKind::SparseFill,
            DepositionKind::Bridge,
            DepositionKind::SupportInterface,
        ] {
            for malformed in [false, true] {
                let mut candidate = plan.layers[0].paths[0].clone();
                candidate.kind = kind;
                candidate.physical_z_mm = [1.0, 2.0];
                candidate.points_mm = vec![[1.0, 1.0, 2.0], [5.0, 1.0, 2.0]];
                let mut left = plan.layers[0].paths[0].clone();
                left.points_mm = vec![[1.0, 1.0, 1.0], [2.0, 1.0, 1.0]];
                let mut right = left.clone();
                right.points_mm = vec![[4.0, 1.0, 1.0], [5.0, 1.0, 1.0]];
                let mut contact = right.clone();
                contact.physical_z_mm = [2.0, 3.0];
                contact.points_mm = vec![[4.0, 1.0, 3.0], [5.0, 1.0, 3.0]];
                if malformed {
                    right.width = WidthProfile::PerEdge { widths_mm: vec![] };
                }
                let previous = [left, right];
                let contacts = [contact];
                let mut scalar_workspace = AnchorWorkspace::default();
                let mut cached_workspace = AnchorWorkspace::default();
                let prepared = ProjectedAnchorPaths::build(
                    previous.iter(),
                    contacts.iter(),
                    &cached_workspace,
                    0,
                    &mut 0,
                    &cancel,
                )
                .unwrap()
                .unwrap();
                cached_workspace
                    .admit_beside(prepared.retained_bytes())
                    .unwrap();
                let expected = anchored(
                    &candidate,
                    previous.iter(),
                    contacts.iter(),
                    None,
                    &intent.recipe,
                    &mut 0,
                    &mut scalar_workspace,
                    &cancel,
                );
                let actual = anchored(
                    &candidate,
                    previous.iter(),
                    contacts.iter(),
                    Some((&prepared, true)),
                    &intent.recipe,
                    &mut 0,
                    &mut cached_workspace,
                    &cancel,
                );
                match (expected, actual) {
                    (Ok(expected), Ok(actual)) => assert_eq!(expected, actual),
                    (Err(expected), Err(actual)) => {
                        assert_eq!(expected.code, actual.code);
                        assert_eq!(expected.message, actual.message);
                    }
                    _ => panic!("cached replay changed anchored acceptance"),
                }
            }
        }
    }

    #[test]
    fn on_part_contacts_replay_both_gaps_without_bed_support() {
        let (intent, plan) = fixture();
        check(&intent, &plan).unwrap();
        plan.validate(&intent).unwrap();
        let cancel = AtomicBool::new(false);
        let program = crate::backend::emit(&plan, &intent, &cancel).unwrap();
        assert!(
            crate::verifier::verify(&program, &plan, &intent, &cancel)
                .unwrap()
                .verified
        );
        let text = String::from_utf8(program.to_vec().unwrap()).unwrap();
        let mut missing = false;
        let missing_root = text
            .lines()
            .filter(|line| {
                if !missing && line.starts_with("G1 ") {
                    missing = true;
                    false
                } else {
                    true
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(missing);
        let program = crate::ChunkSource::from_slice(missing_root.as_bytes(), u64::MAX).unwrap();
        assert!(crate::verifier::verify(&program, &plan, &intent, &cancel).is_err());
    }

    #[test]
    fn retained_contacts_reject_missing_moved_and_tangent_roots() {
        let (intent, plan) = fixture();
        for offset in [1.0, 20.0] {
            let mut moved = plan.clone();
            for point in &mut moved.layers[0].paths[0].points_mm {
                point[1] += offset;
            }
            assert!(check(&intent, &moved).is_err());
            let cancel = AtomicBool::new(false);
            let program = crate::backend::emit(&moved, &intent, &cancel).unwrap();
            assert!(crate::verifier::verify(&program, &moved, &intent, &cancel).is_err());
        }
        let mut missing = plan;
        missing.layers[0].paths.clear();
        assert!(check(&intent, &missing).is_err());
    }

    #[test]
    fn retained_contacts_reject_future_same_layer_and_unsupported_roots() {
        let (intent, plan) = fixture();
        for destination in [1, 2, 4] {
            let mut changed = plan.clone();
            let mut root = changed.layers[0].paths.remove(0);
            root.physical_z_mm = [destination as f64, destination as f64 + 1.0];
            for point in &mut root.points_mm {
                point[2] = destination as f64 + 1.0;
            }
            changed.layers[destination].paths.push(root);
            assert!(check(&intent, &changed).is_err());
        }
    }

    #[test]
    fn retained_contacts_reject_insufficient_lower_and_upper_gap() {
        let (mut intent, plan) = fixture();
        if let SupportPolicy::Breakaway { z_gap_mm, .. } = &mut intent.recipe.supports {
            *z_gap_mm = 1.1;
        }
        assert!(check(&intent, &plan).is_err());
        let (intent, mut changed) = fixture();
        changed.layers[4].paths[0].physical_z_mm = [3.25, 4.25];
        for point in &mut changed.layers[4].paths[0].points_mm {
            point[2] = 4.25;
        }
        assert!(check(&intent, &changed).is_err());
    }

    #[test]
    fn ordinary_model_gap_cannot_supply_a_hypothetical_bridge_root() {
        let (intent, plan) = fixture();
        let mut path = plan.layers[0].paths[0].clone();
        path.physical_z_mm = [1.0, 2.0];
        path.points_mm = vec![[1.0, 1.0, 2.0], [5.0, 1.0, 2.0]];
        let mut left = plan.layers[0].paths[0].clone();
        left.points_mm = vec![[1.0, 1.0, 1.0], [2.0, 1.0, 1.0]];
        let mut right = left.clone();
        right.points_mm = vec![[4.0, 1.0, 1.0], [5.0, 1.0, 1.0]];
        let lower = [left, right];
        let mut anchors = AnchorWorkspace::default();
        let mut work = 0;
        let cancel = AtomicBool::new(false);
        assert!(
            !anchored(
                &path,
                lower.iter(),
                std::iter::empty(),
                None,
                &intent.recipe,
                &mut work,
                &mut anchors,
                &cancel
            )
            .unwrap()
        );
        // Sparse infill spanning the rows beneath it roots what rests on it.
        path.kind = DepositionKind::SparseFill;
        assert!(
            anchored(
                &path,
                lower.iter(),
                std::iter::empty(),
                None,
                &intent.recipe,
                &mut work,
                &mut anchors,
                &cancel
            )
            .unwrap()
        );
        // So does a wall connector, judged by the same short-span rule.
        let mut connected = path.clone();
        connected.kind = DepositionKind::TopSkin;
        connected.points_mm = vec![
            [1.0, 0.0, 2.0],
            [1.0, 1.0, 2.0],
            [5.0, 1.0, 2.0],
            [5.0, 0.0, 2.0],
        ];
        connected.connectors = vec![ConnectorRun { first: 1, last: 2 }];
        let mut under = lower.to_vec();
        for x in [1.0, 5.0] {
            let mut post = lower[0].clone();
            post.points_mm = vec![[x, -0.5, 1.0], [x, 1.5, 1.0]];
            under.push(post);
        }
        assert!(
            anchored(
                &connected,
                under.iter(),
                std::iter::empty(),
                None,
                &intent.recipe,
                &mut work,
                &mut anchors,
                &cancel
            )
            .unwrap()
        );
        connected.connectors.clear();
        assert!(
            !anchored(
                &connected,
                under.iter(),
                std::iter::empty(),
                None,
                &intent.recipe,
                &mut work,
                &mut anchors,
                &cancel
            )
            .unwrap()
        );
        path.kind = DepositionKind::Bridge;
        assert!(
            anchored(
                &path,
                lower.iter(),
                std::iter::empty(),
                None,
                &intent.recipe,
                &mut work,
                &mut anchors,
                &cancel
            )
            .unwrap()
        );
    }
}
