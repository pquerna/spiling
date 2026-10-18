// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::{SectionSpan, boolean_budget, region, span_at};
use crate::{check_cancel, error};
use geo::{Area, BooleanOps, BoundingRect, MultiPolygon, Polygon};
use spiling_contracts::manufacturing::*;
use std::sync::{Arc, Weak, atomic::AtomicBool};
mod anchors;
pub(super) use anchors::{AnchorCandidate, AnchorWorkspace};
mod strokes;
pub(super) use strokes::StrokeRuns;
mod raster;

struct Demand {
    target: usize,
    failure: strokes::Failure,
}
pub(super) struct RasterCandidate {
    pub(super) paths: Vec<DepositionPath>,
    pub(super) whole_rows: Vec<DepositionPath>,
}

pub(super) struct ProtectedVoidSet {
    pub regions: MultiPolygon<f64>,
    pub scopes: Vec<spiling_geometry::ProtectedVoidHeightScope>,
}

/// Admission inputs shared by whole-row ownership and deposited-anchor proof.
struct BridgeRowContext<'a> {
    axis: region::HatchAxis,
    retained_points: usize,
    recipe: &'a PlanarPrintRecipe,
}

/// One union for an incoming owner and the fixed column at this cache's scope.
/// The weak source key must not keep an old contour array alive.
struct SharedUnion {
    source: Weak<MultiPolygon<f64>>,
    result: Arc<MultiPolygon<f64>>,
}

// Identical fixed-XY columns share immutable contours between layers. Reuse a
// union for identical incoming owners; a weak key retains no old point arrays.
// Each cache is scoped to one fixed immutable `other` column below.
fn merge_shared(
    into: &mut Arc<MultiPolygon<f64>>,
    other: &Arc<MultiPolygon<f64>>,
    cached: &mut Option<SharedUnion>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    if other.0.is_empty() {
        return Ok(());
    }
    if into.0.is_empty() {
        *into = Arc::clone(other);
        return Ok(());
    }
    if let Some(cached) = cached
        && cached.source.as_ptr() == Arc::as_ptr(into)
    {
        *into = Arc::clone(&cached.result);
        return Ok(());
    }
    charge_boolean(into, other, work, cancel)?;
    let mut result = into.as_ref().union(other.as_ref());
    region::check_size(&result)?;
    region::canonicalize(&mut result);
    check_cancel(cancel)?;
    let result = Arc::new(result);
    *cached = Some(SharedUnion {
        source: Arc::downgrade(into),
        result: Arc::clone(&result),
    });
    *into = result;
    Ok(())
}

/// Realized object geometry and the retained native ownership used by support.
pub(super) struct SupportInput<'a> {
    pub(super) sections: &'a [SectionSpan],
    pub(super) schedule: &'a [super::LayerElevation],
    pub(super) native_points: usize,
    pub(super) recipe: &'a PlanarPrintRecipe,
}

// Anchors come only from actual deposited capsules. Corridor exclusion consumes
// every lower slab's continuous-target outer occupancy, separately from the
// sampled/scheduled D used by walls and physical-distance skin classification.
pub(super) fn realize(
    layers: &mut [PrintLayer],
    input: &SupportInput<'_>,
    segments: &mut usize,
    cancel: &AtomicBool,
    native_work: &mut spiling_geometry::GeometryWorkBudget,
    mut raster: impl FnMut(
        usize,
        region::HatchAxis,
        &mut usize,
        usize,
    ) -> Result<RasterCandidate, ManufacturingError>,
    mut project_voids: impl FnMut(
        &[[[f64; 2]; 2]],
        usize,
        &mut usize,
        &mut spiling_geometry::GeometryWorkBudget,
    ) -> Result<ProtectedVoidSet, ManufacturingError>,
) -> Result<(), ManufacturingError> {
    let SupportInput {
        sections,
        schedule,
        native_points,
        recipe,
    } = *input;
    let width = recipe.bead_width_mm;
    let mut demands: Vec<Demand> = Vec::new();
    let retained = native_points;
    let mut work = 0usize;
    // Deposited-stroke comparisons are not polygon Boolean work.
    let mut anchor_work = 0usize;
    let mut anchors = AnchorWorkspace::default();
    let mut axes: Vec<_> = layers
        .iter()
        .map(|layer| region::HatchAxis::preferred(layer.index))
        .collect();
    let mut joined_layers = vec![false; layers.len()];
    let mut original_bridge_roles = Vec::new();
    anchors.set_budget(remaining_geometry_budget(retained, &[], &[], &[])?);
    for index in 1..layers.len() {
        let (prior, remaining) = layers.split_at_mut(index);
        let layer = &mut remaining[0];
        check_cancel(cancel)?;
        let previous = prior[index - 1].paths.iter();
        let mut admission = strokes::analyze(
            &layer.paths,
            previous.clone(),
            std::iter::empty(),
            recipe,
            &mut anchors,
            &mut anchor_work,
            cancel,
        )?;
        if admission.failure.is_some() {
            // Test both axes against actual strokes. Required contiguous pieces
            // may extend to direct anchors only inside their positive-fill row.
            let without_old = *segments - path_segments(&layer.paths);
            for axis in [axes[index], axes[index].orthogonal()] {
                let mut staged_segments = without_old;
                anchors.set_budget(0);
                let mut candidate = raster(index, axis, &mut staged_segments, 0)?;
                let joined = coalesce_bridge_rows(
                    &mut candidate,
                    BridgeRowContext {
                        axis,
                        retained_points: retained,
                        recipe,
                    },
                    previous.clone(),
                    std::iter::empty(),
                    &mut anchors,
                    &mut anchor_work,
                    cancel,
                )?;
                anchors.set_budget(candidate_budget(&candidate.paths, retained)?);
                let other = strokes::analyze(
                    &candidate.paths,
                    previous.clone(),
                    std::iter::empty(),
                    recipe,
                    &mut anchors,
                    &mut anchor_work,
                    cancel,
                )?;
                let complete = other.failure.is_none();
                // Same-axis joins preserve every required piece and have their
                // own direct-anchor proof. Keep that monotone improvement even
                // when another stroke still needs support; do not demand a
                // corridor for fragments that we can already anchor.
                if complete || (axis == axes[index] && joined) {
                    *segments = without_old + path_segments(&candidate.paths);
                    layer.paths = candidate.paths;
                    axes[index] = axis;
                    joined_layers[index] = joined;
                    admission = other;
                    if complete {
                        break;
                    }
                }
            }
            anchors.set_budget(remaining_geometry_budget(retained, &[], &[], &[])?);
        }
        if let Some(failure) = admission.failure.take() {
            if matches!(recipe.supports, SupportPolicy::Disabled) {
                return Err(admission_error(
                    index,
                    layer.z_mm,
                    &layer.paths,
                    &failure,
                    recipe,
                ));
            }
            demands.push(Demand {
                target: index,
                failure,
            });
        }
        for path in admission.bridges {
            original_bridge_roles.push((index, path, layer.paths[path].kind));
            layer.paths[path].kind = DepositionKind::Bridge;
        }
    }
    if demands.is_empty() {
        object_volumes(layers, recipe);
        return Ok(());
    }
    anchors.set_budget(0);
    let SupportPolicy::Breakaway {
        density,
        xy_clearance_mm,
        z_gap_mm,
        interface_thickness_mm,
    } = recipe.supports
    else {
        unreachable!("disabled support demand refused above")
    };
    // Native all-Z void branches are projected only after actual strokes prove
    // demand. Scheduled hole samples cannot certify a protected projection.
    let mut demand_bounds = Vec::with_capacity(demands.len());
    for demand in &demands {
        check_cancel(cancel)?;
        let candidate = &sections[span_at(sections, demand.target as u32)].material;
        let bounds = candidate.bounding_rect().ok_or_else(|| {
            error(
                ManufacturingErrorCode::NativeAnalysisIndeterminate,
                "actual support demand has no material footprint bounds",
            )
        })?;
        demand_bounds.push([
            [bounds.min().x, bounds.max().x],
            [bounds.min().y, bounds.max().y],
        ]);
    }
    let projected = project_voids(
        &demand_bounds,
        remaining_geometry_budget(retained, &[], &[], &[])?,
        &mut work,
        native_work,
    )?;
    let ProtectedVoidSet {
        regions: protected,
        scopes,
    } = projected;
    if protected.0.len() != scopes.len() {
        return Err(error(
            ManufacturingErrorCode::NativeAnalysisIndeterminate,
            "native protected void scopes do not match their outer masks",
        ));
    }
    region::check_size(&protected)?;
    let empty = Arc::new(MultiPolygon(Vec::new()));
    let mut support_regions = vec![Arc::clone(&empty); layers.len()];
    let mut interfaces = vec![empty; layers.len()];
    let mut object_start = vec![0; layers.len()];
    let mut contact_tops = vec![None; layers.len()];
    remaining_geometry_budget(retained, &support_regions, &interfaces, &[&protected])?;
    for demand in &demands {
        check_cancel(cancel)?;
        let floor = program_coordinates::project_xyz6_bead_height_mm(
            layers[demand.target].z_mm,
            layers[demand.target].height_mm,
        )?[0];
        let mut contact_top = None;
        for i in (0..demand.target).rev() {
            super::native::charge_support_clearance(&mut work, cancel)?;
            let upper = program_coordinates::project_xyz6_bead_height_mm(
                layers[i].z_mm,
                layers[i].height_mm,
            )?[1];
            if program_coordinates::enclosed_vertical_gap_mm(floor, upper)? >= z_gap_mm {
                contact_top = Some(i);
                break;
            }
        }
        let Some(top) = contact_top else {
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                format!(
                    "layer {}: no printable support layer fits below the declared Z gap",
                    demand.target
                ),
            ));
        };
        contact_tops[demand.target] = Some(top);
        let interface_bottom = layers[top].z_mm - interface_thickness_mm;
        // Realized object D supplies a conservative support candidate only
        // AFTER actual deposited strokes prove demand; it is never an anchor map.
        // Original/derived boundaries avoid scalloped capsule-strip contours.
        let candidate = &sections[span_at(sections, demand.target as u32)].material;
        remaining_geometry_budget(
            retained,
            &support_regions,
            &interfaces,
            &[&protected, candidate],
        )?;
        let mut column = candidate.clone();
        // Certify a fixed-XY corridor all the way to the bed before publishing.
        // Top-down publication before lower clipping leaves floating components.
        for section in &sections[..=span_at(sections, top as u32)] {
            if column.0.is_empty() {
                break;
            }
            for (occupied, height) in section.occupied_pieces() {
                check_cancel(cancel)?;
                if column.0.is_empty() {
                    break;
                }
                work = work
                    .checked_add(1)
                    .filter(|n| *n <= super::MAX_BOOLEAN_PAIRS)
                    .ok_or_else(|| {
                        error(
                            ManufacturingErrorCode::ResourceLimit,
                            "cumulative continuous occupied scope work exceeded",
                        )
                    })?;
                if let Some([lower, upper]) = height {
                    let bed = layers[0].z_mm - layers[0].height_mm;
                    if lower > layers[top].z_mm || upper < bed {
                        continue;
                    }
                }
                // Independent certified outer operands may overlap. A single
                // even-odd normalization would erase their overlap; subtract
                // each expanded operand instead, borrowing its original rings.
                super::boolean::budget_polygon_accumulated(occupied, &mut work, cancel)?;
                let distance = candidate_clearance_distance(occupied, &column, xy_clearance_mm)?;
                let clearance = region::offset_polygon(occupied, distance, cancel)?;
                remaining_geometry_budget(
                    retained,
                    &support_regions,
                    &interfaces,
                    &[&protected, &column, &clearance],
                )?;
                charge_boolean(&column, &clearance, &mut work, cancel)?;
                column = column.difference(&clearance);
                region::check_size(&column)?;
                remaining_geometry_budget(
                    retained,
                    &support_regions,
                    &interfaces,
                    &[&protected, &column, &clearance],
                )?;
            }
        }
        for (mask, scope) in protected.0.iter().zip(&scopes) {
            if column.0.is_empty() {
                break;
            }
            check_cancel(cancel)?;
            work = work
                .checked_add(1)
                .filter(|n| *n <= super::MAX_BOOLEAN_PAIRS)
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "cumulative protected-void scope work budget exceeded",
                    )
                })?;
            let corridor_lower = layers[0].z_mm - layers[0].height_mm;
            let corridor_upper = layers[top].z_mm;
            let relevant = match scope {
                spiling_geometry::ProtectedVoidHeightScope::Bounded { lower_mm, upper_mm } => {
                    *lower_mm <= corridor_upper && *upper_mm >= corridor_lower
                }
                spiling_geometry::ProtectedVoidHeightScope::OpenBelow { upper_mm } => {
                    *upper_mm >= corridor_lower
                }
            };
            if !relevant {
                continue;
            }
            super::boolean::budget_polygon_accumulated(mask, &mut work, cancel)?;
            let distance = candidate_clearance_distance(mask, &column, xy_clearance_mm)?;
            let clearance = region::offset_polygon(mask, distance, cancel)?;
            remaining_geometry_budget(
                retained,
                &support_regions,
                &interfaces,
                &[&protected, &column, &clearance],
            )?;
            charge_boolean(&column, &clearance, &mut work, cancel)?;
            column = column.difference(&clearance);
            region::check_size(&column)?;
            remaining_geometry_budget(
                retained,
                &support_regions,
                &interfaces,
                &[&protected, &column, &clearance],
            )?;
        }
        region::check_size(&column)?;
        if column.0.is_empty() {
            let layer = &layers[demand.target];
            let failure = &demand.failure;
            let path = &layer.paths[failure.path];
            let a = path.points_mm[failure.first];
            let b = path.points_mm[failure.last];
            let previous = &layers[demand.target - 1];
            let nearest_a = nearest_stroke([a[0], a[1]], previous.paths.iter(), &mut work, cancel)?;
            let nearest_b = nearest_stroke([b[0], b[1]], previous.paths.iter(), &mut work, cancel)?;
            return Err(error(
                ManufacturingErrorCode::NativeAnalysisIndeterminate,
                format!(
                    "layer {} Z {:.9}: unresolved continuous outer occupancy/protected geometry cannot certify any bed-anchored support corridor; conservative exclusion is not proof of actual collision; role {:?}, stroke {:?} to {:?}, gap ({:.12},{:.12}) span {:.12}; previous layer {} Z {:.9}, nearest start {:?}, nearest end {:?}; nominal bead width {:.9} mm, XY clearance {:.9} mm, Z gap {:.9} mm, bridge limit {:.9} mm",
                    demand.target,
                    layer.z_mm,
                    path.kind,
                    a,
                    b,
                    failure.start,
                    failure.end,
                    failure.span,
                    previous.index,
                    previous.z_mm,
                    nearest_a,
                    nearest_b,
                    width,
                    xy_clearance_mm,
                    z_gap_mm,
                    recipe.process.max_bridge_span_mm
                ),
            ));
        }
        region::canonicalize(&mut column);
        let column = Arc::new(column);
        let mut shared_union = None;
        for i in 0..=top {
            check_cancel(cancel)?;
            merge_shared(
                &mut support_regions[i],
                &column,
                &mut shared_union,
                &mut work,
                cancel,
            )?;
            if layers[i].z_mm > interface_bottom {
                merge_shared(
                    &mut interfaces[i],
                    &column,
                    &mut shared_union,
                    &mut work,
                    cancel,
                )?;
            }
            remaining_geometry_budget(
                retained,
                &support_regions,
                &interfaces,
                &[&protected, &column],
            )?;
        }
    }
    // Support centers stay half a bead inside the clipped support region. This
    // gives the nominal bead, not only its centerline, the declared XY clearance.
    for i in 0..layers.len() {
        anchors.set_budget(0);
        let (prior, remaining) = layers.split_at_mut(i);
        let layer = &mut remaining[0];
        let interface = interfaces[i].as_ref();
        let retained_domains = MAX_DEPOSITION_SEGMENTS as usize
            - remaining_geometry_budget(retained, &support_regions, &interfaces, &[&protected])?;
        let domains = raster::Domains::new(
            &support_regions[i],
            interface,
            Arc::ptr_eq(&support_regions[i], &interfaces[i]),
            width,
            retained_domains,
            &mut work,
            cancel,
        )?;
        let preferred = region::HatchAxis::preferred(layer.index);
        let mut paths = Vec::new();
        for axis in [preferred, preferred.orthogonal()] {
            anchors.set_budget(0);
            paths = domains.paths(
                interface,
                density,
                &region::HatchLayer {
                    z: layer.z_mm,
                    height: layer.height_mm,
                    axis,
                    recipe,
                },
                *segments,
                &mut work,
                cancel,
            )?;
            if !support_regions[i].0.is_empty() && paths.is_empty() {
                return Err(error(
                    ManufacturingErrorCode::UnsupportedGeometry,
                    format!("layer {i}: support clearance leaves no printable bead-width support"),
                ));
            }
            if i == 0 {
                break;
            }
            anchors.set_budget(domains.anchor_budget(&paths)?);
            let failure = raster::failure(
                &paths,
                prior[i - 1]
                    .paths
                    .iter()
                    .filter(|path| support_role(path.kind)),
                recipe,
                &mut anchors,
                &mut anchor_work,
                cancel,
            )?;
            let Some(failure) = failure else {
                break;
            };
            if axis != preferred {
                let path = &paths[failure.path];
                return Err(error(
                    ManufacturingErrorCode::UnsupportedGeometry,
                    format!(
                        "layer {i}: neither orthogonal support/interface raster has bounded deposited anchors; role {:?}, stroke {:?} to {:?}, gap ({:.12},{:.12}) span {:.12}; increase support density or revise support corridor",
                        path.kind,
                        path.points_mm[failure.first],
                        path.points_mm[failure.last],
                        failure.start,
                        failure.end,
                        failure.span,
                    ),
                ));
            }
            // Release failed paths and anchor records before staging the other
            // axis. Neither attempt changes object or support ownership.
            paths = Vec::new();
        }
        *segments += path_segments(&paths);
        anchors.set_budget(0);
        let bead_area = bead_area(layer, recipe);
        layer.region_coverage.support_area_mm2 = support_regions[i].unsigned_area();
        layer.region_coverage.support_nominal_deposition_volume_mm3 = paths
            .iter()
            .flat_map(|path| path.points_mm.windows(2))
            .map(|edge| (edge[1][0] - edge[0][0]).hypot(edge[1][1] - edge[0][1]) * bead_area)
            .sum();
        object_start[i] = paths.len();
        paths.append(&mut layer.paths);
        layer.paths = paths;
    }
    // Restore original object roles before reevaluating the actual emitted
    // support/contact strokes. A supported contour is not a fake Bridge.
    for (index, path, kind) in original_bridge_roles {
        layers[index].paths[object_start[index] + path].kind = kind;
    }
    let retained_regions = MAX_DEPOSITION_SEGMENTS as usize
        - remaining_geometry_budget(retained, &support_regions, &interfaces, &[&protected])?;
    // Raster staging must include these live support/protected domains without
    // counting the caller's object/cache reservation twice.
    let retained_support_points = retained_regions.checked_sub(retained).ok_or_else(|| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "support retained point accounting underflow",
        )
    })?;
    anchors.set_budget(MAX_DEPOSITION_SEGMENTS as usize - retained_regions);
    for index in 1..layers.len() {
        let (prior, remaining) = layers.split_at_mut(index);
        let layer = &mut remaining[0];
        let begin = object_start[index];
        let previous = prior[index - 1].paths.iter();
        let contact_paths = contact_tops[index]
            .map(|top| prior[top].paths.as_slice())
            .unwrap_or(&[]);
        let contact = contact_paths.iter().filter(|path| support_role(path.kind));
        if joined_layers[index] {
            // Other support demands can change the eventual previous footprints.
            // Recreate ownership pieces; never retain a stale composite Bridge.
            let without_old = *segments - path_segments(&layer.paths[begin..]);
            let mut staged_segments = without_old;
            anchors.set_budget(0);
            let mut candidate = raster(
                index,
                axes[index],
                &mut staged_segments,
                retained_support_points,
            )?;
            coalesce_bridge_rows(
                &mut candidate,
                BridgeRowContext {
                    axis: axes[index],
                    retained_points: retained_regions,
                    recipe,
                },
                previous.clone(),
                contact.clone(),
                &mut anchors,
                &mut anchor_work,
                cancel,
            )?;
            *segments = without_old + path_segments(&candidate.paths);
            layer.paths.truncate(begin);
            layer.paths.extend(candidate.paths);
            anchors.set_budget(MAX_DEPOSITION_SEGMENTS as usize - retained_regions);
        }
        let mut admission = strokes::analyze(
            &layer.paths[begin..],
            previous.clone(),
            contact.clone(),
            recipe,
            &mut anchors,
            &mut anchor_work,
            cancel,
        )?;
        if admission.failure.is_some() {
            let without_old = *segments - path_segments(&layer.paths[begin..]);
            for axis in [axes[index], axes[index].orthogonal()] {
                let mut staged_segments = without_old;
                anchors.set_budget(0);
                let mut candidate =
                    raster(index, axis, &mut staged_segments, retained_support_points)?;
                coalesce_bridge_rows(
                    &mut candidate,
                    BridgeRowContext {
                        axis,
                        retained_points: retained_regions,
                        recipe,
                    },
                    previous.clone(),
                    contact.clone(),
                    &mut anchors,
                    &mut anchor_work,
                    cancel,
                )?;
                anchors.set_budget(candidate_budget(&candidate.paths, retained_regions)?);
                let other = strokes::analyze(
                    &candidate.paths,
                    previous.clone(),
                    contact.clone(),
                    recipe,
                    &mut anchors,
                    &mut anchor_work,
                    cancel,
                )?;
                if other.failure.is_none() {
                    *segments = without_old + path_segments(&candidate.paths);
                    layer.paths.truncate(begin);
                    layer.paths.extend(candidate.paths);
                    axes[index] = axis;
                    admission = other;
                    break;
                }
            }
            anchors.set_budget(MAX_DEPOSITION_SEGMENTS as usize - retained_regions);
        }
        if let Some(failure) = admission.failure {
            let path = &layer.paths[begin + failure.path];
            let a = path.points_mm[failure.first];
            let b = path.points_mm[failure.last];
            let t = (failure.start + failure.end) * 0.5;
            let point = [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t];
            let nearest_contact = nearest_stroke(
                point,
                contact_paths.iter().filter(|path| support_role(path.kind)),
                &mut work,
                cancel,
            )?;
            let nearest_object = nearest_stroke(
                point,
                prior[index - 1]
                    .paths
                    .iter()
                    .filter(|path| !support_role(path.kind)),
                &mut work,
                cancel,
            )?;
            return Err(error(
                ManufacturingErrorCode::UnsupportedGeometry,
                format!(
                    "layer {index} Z {:.6}: neither orthogonal native raster has bounded actual deposited/contact anchors; role {:?}, stroke {:?} to {:?}, gap ({:.12},{:.12}) span {:.12}; contact layer {:?}, nearest contact {:?}, nearest previous object {:?}",
                    layer.z_mm,
                    path.kind,
                    a,
                    b,
                    failure.start,
                    failure.end,
                    failure.span,
                    contact_tops[index],
                    nearest_contact,
                    nearest_object
                ),
            ));
        }
        for path in admission.bridges {
            layer.paths[begin + path].kind = DepositionKind::Bridge;
        }
    }
    // Contact selection is only a proposal until actual final paths exist.
    // Check the same closed, physical-decimal bead enclosure used by the XY
    // exclusion gates; no schedule epsilon can authorize a smaller Z gap.
    for (target, top) in contact_tops.iter().enumerate() {
        let Some(top) = *top else {
            continue;
        };
        let mut object_bottom = f64::INFINITY;
        let mut support_top = f64::NEG_INFINITY;
        for path in &layers[target].paths {
            if support_role(path.kind) {
                continue;
            }
            for edge in path.points_mm.windows(2) {
                super::native::charge_support_clearance(&mut work, cancel)?;
                object_bottom = object_bottom.min(
                    program_coordinates::project_xyz6_segment_bead_height_mm(
                        edge[0][2],
                        edge[1][2],
                        layers[target].height_mm,
                    )?[0],
                );
            }
        }
        for path in &layers[top].paths {
            if !support_role(path.kind) {
                continue;
            }
            for edge in path.points_mm.windows(2) {
                super::native::charge_support_clearance(&mut work, cancel)?;
                support_top = support_top.max(
                    program_coordinates::project_xyz6_segment_bead_height_mm(
                        edge[0][2],
                        edge[1][2],
                        layers[top].height_mm,
                    )?[1],
                );
            }
        }
        if !object_bottom.is_finite()
            || !support_top.is_finite()
            || program_coordinates::enclosed_vertical_gap_mm(object_bottom, support_top)? < z_gap_mm
        {
            return Err(error(
                ManufacturingErrorCode::ProgramCoordinateConditioningFailure,
                format!(
                    "layer {target}: actual emitted contact does not prove the minimum Z gap {z_gap_mm} mm"
                ),
            ));
        }
    }
    // Offset/overlay corridor masks only propose candidates. Certify every
    // final actual emitted support capsule against the unchanged raw all-Z C
    // and scoped positive void masks, after both raster/anchor alternatives.
    super::native::certify_support_clearance(
        layers,
        &super::native::SupportClearanceGeometry {
            sections,
            schedule,
            protected: &protected,
            scopes: &scopes,
        },
        recipe,
        native_work,
        &mut work,
        cancel,
    )?;
    object_volumes(layers, recipe);
    Ok(())
}

fn support_role(kind: DepositionKind) -> bool {
    matches!(
        kind,
        DepositionKind::Support | DepositionKind::SupportInterface
    )
}

fn path_segments(paths: &[DepositionPath]) -> usize {
    paths.iter().map(|path| path.points_mm.len() - 1).sum()
}

fn candidate_budget(
    paths: &[DepositionPath],
    retained: usize,
) -> Result<usize, ManufacturingError> {
    paths
        .iter()
        .try_fold(retained, |total, path| {
            total
                .checked_add(path.points_mm.len())
                .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "retained native/support/alternative raster point budget exceeded",
                    )
                })
        })
        .map(|total| MAX_DEPOSITION_SEGMENTS as usize - total)
}

fn coalesce_bridge_rows<'a>(
    candidate: &mut RasterCandidate,
    context: BridgeRowContext<'_>,
    previous: impl Iterator<Item = &'a DepositionPath> + Clone,
    contacts: impl Iterator<Item = &'a DepositionPath> + Clone,
    anchors: &mut AnchorWorkspace,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    let BridgeRowContext {
        axis,
        retained_points,
        recipe,
    } = context;
    if candidate.whole_rows.is_empty() {
        return Ok(false);
    }
    let fixed = if axis == region::HatchAxis::X { 1 } else { 0 };
    let along = 1 - fixed;
    // Compare actual emitted-grid coordinates, not a widened anchor radius or
    // native tolerance. Whole rows themselves come from authoritative fill.
    let grid = program_coordinates::project_xyz6_coordinate_mm;
    let budget = candidate_budget(&candidate.paths, retained_points)?;
    let staged = MAX_DEPOSITION_SEGMENTS as usize - budget;
    let budget = candidate_budget(&candidate.whole_rows, staged)?;
    let mut pieces = Vec::new();
    for (index, path) in candidate.paths.iter().enumerate() {
        check_cancel(cancel)?;
        if path.points_mm.len() != 2 || path.kind == DepositionKind::Perimeter {
            continue;
        }
        let a = path.points_mm[0];
        let b = path.points_mm[1];
        let row = grid(a[fixed])?;
        if row != grid(b[fixed])? {
            continue;
        }
        let lo = grid(a[along])?.min(grid(b[along])?);
        let hi = grid(a[along])?.max(grid(b[along])?);
        if pieces.len() >= budget {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "combined alternative raster/row ownership budget exceeded",
            ));
        }
        pieces.push((row, lo, hi, index));
    }
    let budget = budget.checked_sub(pieces.len()).ok_or_else(|| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "combined alternative raster/row ownership budget exceeded",
        )
    })?;
    anchors.set_budget(budget);
    pieces.sort_unstable_by(|a, b| {
        a.0.total_cmp(&b.0)
            .then(a.1.total_cmp(&b.1))
            .then(a.2.total_cmp(&b.2))
    });
    let original_len = candidate.paths.len();
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Ownership {
        Required,
        Replaced,
        Removed,
    }
    let mut ownership = vec![Ownership::Required; original_len];
    let mut joined = false;
    for mut row in std::mem::take(&mut candidate.whole_rows) {
        check_cancel(cancel)?;
        let key = grid(row.points_mm[0][fixed])?;
        let lo = grid(row.points_mm[0][along])?.min(grid(row.points_mm[1][along])?);
        let hi = grid(row.points_mm[0][along])?.max(grid(row.points_mm[1][along])?);
        let whole_start = row.points_mm[0][along].min(row.points_mm[1][along]);
        let whole_end = row.points_mm[0][along].max(row.points_mm[1][along]);
        let begin = pieces.partition_point(|piece| piece.0 < key);
        let end = pieces.partition_point(|piece| piece.0 <= key);
        // Each piece is visited at most twice while finding disjoint groups.
        *work = work
            .checked_add((end - begin) * 2)
            .filter(|n| *n <= anchors::MAX_ANCHOR_PAIR_WORK)
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "cumulative bridge row ownership work budget exceeded",
                )
            })?;
        let mut cursor = begin;
        let mut previous_required_end = None;
        let mut previous_emitted_end = None;
        while cursor < end {
            check_cancel(cancel)?;
            let group_begin = cursor;
            let mut first = None;
            let mut last = None;
            let mut covered: f64 = f64::NEG_INFINITY;
            let mut required_start = f64::INFINITY;
            let mut required_end = f64::NEG_INFINITY;
            let mut count = 0;
            while cursor < end {
                check_cancel(cancel)?;
                let (_, start, stop, index) = pieces[cursor];
                if ownership[index] != Ownership::Required
                    || start < lo
                    || stop > hi
                    || stop <= start
                {
                    cursor += 1;
                    continue;
                }
                if let Some(last) = last
                    && start > covered
                {
                    let previous_piece: &DepositionPath = &candidate.paths[last];
                    let current_piece = &candidate.paths[index];
                    let endpoint =
                        if current_piece.points_mm[0][along] <= current_piece.points_mm[1][along] {
                            current_piece.points_mm[0]
                        } else {
                            current_piece.points_mm[1]
                        };
                    // A same-row centerline gap is not missing material when
                    // the actual emitted current bead capsules already overlap
                    // positively. Tangency is not connectivity. This candidate
                    // connector still needs independent previous-anchor proof.
                    if !(AnchorCandidate {
                        endpoints_mm: [previous_piece.points_mm[0], previous_piece.points_mm[1]],
                        bead_width_mm: recipe.bead_width_mm,
                    })
                    .emitted_endpoint_footprint_overlaps(endpoint, recipe.bead_width_mm)?
                    {
                        break;
                    }
                }
                first.get_or_insert(index);
                // Nested or overlapping owners cannot move the union's end
                // backwards or discard material owned by an earlier piece.
                if stop > covered {
                    covered = stop;
                    last = Some(index);
                }
                for point in &candidate.paths[index].points_mm {
                    required_start = required_start.min(point[along]);
                    required_end = required_end.max(point[along]);
                }
                count += 1;
                cursor += 1;
            }
            let Some(first) = first else {
                continue;
            };
            // Neighboring required groups bound each search, but are not
            // anchors. A group may reach the nearest real previous interval
            // inside an unrequired gap without filling that gap completely.
            let left_emitted_end = previous_emitted_end;
            let next_required_start = if cursor < end {
                let next = &candidate.paths[pieces[cursor].3].points_mm;
                Some(next[0][along].min(next[1][along]))
            } else {
                None
            };
            let extension_start = previous_required_end.unwrap_or(whole_start);
            let extension_end = next_required_start.unwrap_or(whole_end);
            previous_required_end = Some(required_end);
            previous_emitted_end = Some(grid(required_end)?);
            row.points_mm[0][along] = required_start;
            row.points_mm[1][along] = required_end;
            let required_proof = strokes::analyze(
                std::slice::from_ref(&row),
                previous.clone(),
                contacts.clone(),
                recipe,
                anchors,
                work,
                cancel,
            )?;
            let proof = if let Some(failure) = &required_proof.failure {
                // End extension cannot repair an over-limit interior span.
                if failure.start > 0.0 && failure.end < 1.0 {
                    continue;
                }
                let whole = AnchorCandidate {
                    endpoints_mm: [
                        {
                            let mut p = row.points_mm[0];
                            p[along] = extension_start;
                            p
                        },
                        {
                            let mut p = row.points_mm[1];
                            p[along] = extension_end;
                            p
                        },
                    ],
                    bead_width_mm: recipe.bead_width_mm,
                };
                let length = extension_end - extension_start;
                let mut start = ((required_start - extension_start) / length).clamp(0.0, 1.0);
                let mut end = ((required_end - extension_start) / length).clamp(0.0, 1.0);
                let mut anchored = true;
                let gaps =
                    anchors.gaps(whole, previous.clone(), std::iter::empty(), work, cancel)?;
                for gap in gaps {
                    if gap.start <= start && start < gap.end {
                        anchored &= gap.left;
                        start = gap.start;
                    }
                    if gap.start < end && end <= gap.end {
                        anchored &= gap.right;
                        end = gap.end;
                    }
                }
                if !anchored {
                    continue;
                }
                row.points_mm[0][along] = extension_start + start * length;
                row.points_mm[1][along] = extension_start + end * length;
                strokes::analyze(
                    std::slice::from_ref(&row),
                    previous.clone(),
                    contacts.clone(),
                    recipe,
                    anchors,
                    work,
                    cancel,
                )?
            } else {
                if count < 2 || required_proof.bridges.is_empty() {
                    continue;
                }
                required_proof
            };
            if proof.failure.is_some() || proof.bridges.is_empty() {
                continue;
            }
            let emitted_start = grid(row.points_mm[0][along])?;
            let emitted_end = grid(row.points_mm[1][along])?;
            if let Some(left) = left_emitted_end
                && emitted_start < grid(required_start)?
                && anchors::emitted_coordinate_separation_lower_mm(emitted_start, left)?
                    <= recipe.bead_width_mm
            {
                continue;
            }
            if let Some(right) = next_required_start
                && emitted_end > grid(required_end)?
                && anchors::emitted_coordinate_separation_lower_mm(grid(right)?, emitted_end)?
                    <= recipe.bead_width_mm
            {
                continue;
            }
            // Preserve an actual footprint gap between independent groups,
            // including after an earlier group acquired its own extension.
            previous_emitted_end = Some(emitted_end);
            for &(_, start, stop, index) in &pieces[group_begin..cursor] {
                if ownership[index] == Ownership::Required && start >= lo && stop <= hi {
                    ownership[index] = Ownership::Removed;
                }
            }
            // Reuse an existing two-point owner instead of retaining both all
            // original pieces and newly allocated group paths until compaction.
            candidate.paths[first].kind = row.kind;
            candidate.paths[first]
                .points_mm
                .copy_from_slice(&row.points_mm);
            ownership[first] = Ownership::Replaced;
            joined = true;
        }
    }
    let mut index = 0;
    candidate.paths.retain(|_| {
        let keep = ownership[index] != Ownership::Removed;
        index += 1;
        keep
    });
    Ok(joined)
}

fn admission_error(
    index: usize,
    z: f64,
    paths: &[DepositionPath],
    failure: &strokes::Failure,
    recipe: &PlanarPrintRecipe,
) -> ManufacturingError {
    let path = &paths[failure.path];
    error(
        ManufacturingErrorCode::UnsupportedGeometry,
        format!(
            "layer {index} Z {z:.6}: neither orthogonal native raster has bounded deposited anchors (bridge limit {:.3} mm); role {:?}, stroke {:?} to {:?}, gap ({:.12},{:.12}) span {:.12}; enable breakaway supports or revise placement/infill",
            recipe.process.max_bridge_span_mm,
            path.kind,
            path.points_mm[failure.first],
            path.points_mm[failure.last],
            failure.start,
            failure.end,
            failure.span
        ),
    )
}

fn object_volumes(layers: &mut [PrintLayer], recipe: &PlanarPrintRecipe) {
    for layer in layers {
        let area = bead_area(layer, recipe);
        layer.region_coverage.nominal_deposition_volume_mm3 = layer
            .paths
            .iter()
            .filter(|path| !support_role(path.kind))
            .map(|path| {
                let factor = if path.kind == DepositionKind::Bridge {
                    recipe.process.bridge_flow_factor
                } else {
                    1.0
                };
                path.points_mm
                    .windows(2)
                    .map(|edge| {
                        (edge[1][0] - edge[0][0]).hypot(edge[1][1] - edge[0][1]) * area * factor
                    })
                    .sum::<f64>()
            })
            .sum();
    }
}

struct NearestStroke {
    distance_mm: f64,
    kind: DepositionKind,
    endpoints_mm: [[f64; 3]; 2],
}

impl std::fmt::Debug for NearestStroke {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NearestStroke")
            .field("distance_mm", &self.distance_mm)
            .field("kind", &self.kind)
            .field("endpoints_mm", &self.endpoints_mm)
            .finish()
    }
}

fn nearest_stroke<'a>(
    point: [f64; 2],
    paths: impl Iterator<Item = &'a DepositionPath>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<NearestStroke>, ManufacturingError> {
    let mut nearest: Option<NearestStroke> = None;
    for path in paths {
        for edge in path.points_mm.windows(2) {
            check_cancel(cancel)?;
            *work = work
                .checked_add(1)
                .filter(|n| *n <= 20_000_000)
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "cumulative deposited-stroke diagnostic work budget exceeded",
                    )
                })?;
            let delta = [edge[1][0] - edge[0][0], edge[1][1] - edge[0][1]];
            let length = delta[0].hypot(delta[1]);
            let unit = [delta[0] / length, delta[1] / length];
            let along = ((point[0] - edge[0][0]) * unit[0] + (point[1] - edge[0][1]) * unit[1])
                .clamp(0.0, length);
            let distance = (point[0] - edge[0][0] - along * unit[0])
                .hypot(point[1] - edge[0][1] - along * unit[1]);
            if !distance.is_finite() {
                return Err(error(
                    ManufacturingErrorCode::UnsupportedGeometry,
                    "nonfinite deposited-stroke diagnostic distance",
                ));
            }
            if nearest
                .as_ref()
                .is_none_or(|best| distance < best.distance_mm)
            {
                nearest = Some(NearestStroke {
                    distance_mm: distance,
                    kind: path.kind,
                    endpoints_mm: [edge[0], edge[1]],
                });
            }
        }
    }
    Ok(nearest)
}

fn bead_area(layer: &PrintLayer, recipe: &PlanarPrintRecipe) -> f64 {
    let h = layer.height_mm;
    ((recipe.bead_width_mm - h) * h + std::f64::consts::PI * (h * 0.5).powi(2))
        * recipe.flow_multiplier
        * if layer.index == 0 {
            recipe.process.first_layer_flow_factor
        } else {
            1.0
        }
}

fn charge_boolean(
    a: &MultiPolygon<f64>,
    b: &MultiPolygon<f64>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    boolean_budget(a, b, cancel)?;
    let pairs = region::point_count(a)
        .checked_mul(region::point_count(b))
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "support Boolean work overflow",
            )
        })?;
    *work = work
        .checked_add(pairs)
        .filter(|n| *n <= 20_000_000)
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "cumulative support Boolean/projection work budget exceeded",
            )
        })?;
    Ok(())
}

fn remaining_geometry_budget(
    native: usize,
    support: &[Arc<MultiPolygon<f64>>],
    interfaces: &[Arc<MultiPolygon<f64>>],
    temporary: &[&MultiPolygon<f64>],
) -> Result<usize, ManufacturingError> {
    let mut total = native;
    // Counting shared owners repeatedly is conservative and allocation-free.
    for region in support
        .iter()
        .chain(interfaces)
        .map(|region| region.as_ref())
        .chain(temporary.iter().copied())
    {
        total = total
            .checked_add(region::point_count(region))
            .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "combined retained native/support/temporary contour budget exceeded",
                )
            })?;
    }
    (MAX_DEPOSITION_SEGMENTS as usize)
        .checked_sub(total)
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "combined retained contour budget exceeded",
            )
        })
}

fn candidate_clearance_distance(
    occupied: &Polygon<f64>,
    column: &MultiPolygon<f64>,
    requested: f64,
) -> Result<f64, ManufacturingError> {
    let a = occupied.bounding_rect().ok_or_else(|| {
        error(
            ManufacturingErrorCode::NativeAnalysisIndeterminate,
            "native outer operand has no finite bounds",
        )
    })?;
    let Some(b) = column.bounding_rect() else {
        return Ok(requested);
    };
    // Offset allowance and Boolean padding only propose a candidate work
    // frame. They do not prove raw-C containment after overlay. Final actual
    // XYZ6 support capsules have a separate shared geometry proof.
    let reserve = super::OFFSET_TOLERANCE_MM + 2.0f64.sqrt() * (super::LAYER_QUANTIZATION_MM * 0.5);
    let base = (requested + reserve).next_up();
    let min = [a.min().x.min(b.min().x), a.min().y.min(b.min().y)];
    let max = [a.max().x.max(b.max().x), a.max().y.max(b.max().y)];
    let padding = super::boolean::overlay_padding(
        min.map(|v| (v - base - super::OFFSET_TOLERANCE_MM).next_down()),
        max.map(|v| (v + base + super::OFFSET_TOLERANCE_MM).next_up()),
    )?;
    let guard = (2.0 * 2.0f64.sqrt() * padding).next_up();
    let distance = (base + guard).next_up();
    let actual_padding = super::boolean::overlay_padding(
        min.map(|v| (v - distance - super::OFFSET_TOLERANCE_MM).next_down()),
        max.map(|v| (v + distance + super::OFFSET_TOLERANCE_MM).next_up()),
    )?;
    if !distance.is_finite() || (2.0f64.sqrt() * actual_padding).next_up() > guard {
        return Err(error(
            ManufacturingErrorCode::NativeConditioningFailure,
            "support exclusion candidate cannot admit its offset/overlay/XYZ6 work frame",
        ));
    }
    Ok(distance)
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{line_string, polygon};

    fn row(kind: DepositionKind, a: f64, b: f64, z: f64) -> DepositionPath {
        DepositionPath {
            kind,
            points_mm: vec![[a, 0.0, z], [b, 0.0, z]],
        }
    }

    #[test]
    fn genuine_bridge_rows_join_existing_ownership_pieces_without_extra_fill() {
        let mut candidate = RasterCandidate {
            paths: vec![
                row(DepositionKind::SolidFill, 1.0, 3.0, 0.8),
                row(DepositionKind::BottomSkin, 3.0, 7.0, 0.8),
                row(DepositionKind::SolidFill, 7.0, 9.0, 0.8),
            ],
            whole_rows: vec![row(DepositionKind::Bridge, 1.0, 9.0, 0.8)],
        };
        let previous = [
            row(DepositionKind::SolidFill, 0.0, 3.0, 0.4),
            row(DepositionKind::SolidFill, 7.0, 10.0, 0.4),
        ];
        let recipe = crate::planner::tests::recipe(1.0);
        let mut query = AnchorWorkspace::default();
        assert!(
            coalesce_bridge_rows(
                &mut candidate,
                BridgeRowContext {
                    axis: region::HatchAxis::X,
                    retained_points: 0,
                    recipe: &recipe,
                },
                previous.iter(),
                std::iter::empty(),
                &mut query,
                &mut 0,
                &AtomicBool::new(false)
            )
            .unwrap()
        );
        assert_eq!(candidate.paths.len(), 1);
        assert_eq!(candidate.paths[0].kind, DepositionKind::Bridge);
        assert_eq!(
            candidate.paths[0].points_mm,
            [[1.0, 0.0, 0.8], [9.0, 0.0, 0.8]]
        );
    }

    #[test]
    fn bridge_row_candidates_cannot_fill_a_missing_sparse_piece() {
        let mut candidate = RasterCandidate {
            paths: vec![
                row(DepositionKind::SparseFill, 1.0, 3.0, 0.8),
                row(DepositionKind::SparseFill, 7.0, 9.0, 0.8),
            ],
            whole_rows: vec![row(DepositionKind::Bridge, 1.0, 9.0, 0.8)],
        };
        let previous = [
            row(DepositionKind::SolidFill, 0.0, 3.0, 0.4),
            row(DepositionKind::SolidFill, 7.0, 10.0, 0.4),
        ];
        let recipe = crate::planner::tests::recipe(0.2);
        let mut query = AnchorWorkspace::default();
        assert!(
            !coalesce_bridge_rows(
                &mut candidate,
                BridgeRowContext {
                    axis: region::HatchAxis::X,
                    retained_points: 0,
                    recipe: &recipe,
                },
                previous.iter(),
                std::iter::empty(),
                &mut query,
                &mut 0,
                &AtomicBool::new(false)
            )
            .unwrap()
        );
        assert_eq!(candidate.paths.len(), 2);
        assert!(
            candidate
                .paths
                .iter()
                .all(|path| path.kind == DepositionKind::SparseFill)
        );
    }

    #[test]
    fn overlapping_row_groups_preserve_required_material_and_unrequested_gaps() {
        let recipe = crate::planner::tests::recipe(0.2);
        let cancel = AtomicBool::new(false);
        let previous = [3.0, 7.5].map(|x| DepositionPath {
            kind: DepositionKind::SparseFill,
            points_mm: vec![[x, -1.0, 0.4], [x, 1.0, 0.4]],
        });
        for nested in [false, true] {
            let required = vec![
                row(DepositionKind::SparseFill, 1.0, 1.1, 0.8),
                row(
                    DepositionKind::TopSkin,
                    3.2,
                    if nested { 6.8 } else { 5.1 },
                    0.8,
                ),
                row(
                    DepositionKind::SparseFill,
                    if nested { 4.0 } else { 6.8 },
                    if nested { 3.4 } else { 4.9 },
                    0.8,
                ),
            ];
            let mut candidate = RasterCandidate {
                paths: required.clone(),
                whole_rows: vec![row(DepositionKind::Bridge, 0.5, 9.0, 0.8)],
            };
            let mut anchors = AnchorWorkspace::default();
            let mut work = 0;
            assert!(
                coalesce_bridge_rows(
                    &mut candidate,
                    BridgeRowContext {
                        axis: region::HatchAxis::X,
                        retained_points: 0,
                        recipe: &recipe,
                    },
                    previous.iter(),
                    std::iter::empty(),
                    &mut anchors,
                    &mut work,
                    &cancel,
                )
                .unwrap()
            );
            for owner in &required {
                let start = owner.points_mm[0][0].min(owner.points_mm[1][0]);
                let end = owner.points_mm[0][0].max(owner.points_mm[1][0]);
                assert!(
                    candidate.paths.iter().any(|actual| {
                        actual.points_mm[0][0].min(actual.points_mm[1][0]) <= start
                            && actual.points_mm[0][0].max(actual.points_mm[1][0]) >= end
                    }),
                    "overlap union must retain every required owner"
                );
            }
            assert!(
                candidate.paths.iter().all(|actual| {
                    actual.points_mm[0][0].max(actual.points_mm[1][0]) <= 1.1
                        || actual.points_mm[0][0].min(actual.points_mm[1][0]) >= 3.2
                }),
                "a separate required group must not fill the unrequested interval"
            );
            let repaired: Vec<_> = candidate
                .paths
                .iter()
                .filter(|path| path.points_mm.iter().all(|point| point[0] >= 3.2))
                .cloned()
                .collect();
            assert!(
                strokes::analyze(
                    &repaired,
                    previous.iter(),
                    std::iter::empty(),
                    &recipe,
                    &mut anchors,
                    &mut work,
                    &cancel,
                )
                .unwrap()
                .failure
                .is_none(),
                "an unrelated unanchored group must not prevent a real bridge"
            );
            assert!(
                strokes::analyze(
                    &candidate.paths,
                    previous.iter(),
                    std::iter::empty(),
                    &recipe,
                    &mut anchors,
                    &mut work,
                    &cancel,
                )
                .unwrap()
                .failure
                .is_some(),
                "repairing one group cannot suppress another group's demand"
            );
        }
    }

    #[test]
    fn separate_required_groups_extend_only_to_nearest_real_interior_anchors() {
        let recipe = crate::planner::tests::recipe(0.2);
        let cancel = AtomicBool::new(false);
        let required = vec![
            row(DepositionKind::TopSkin, 2.0, 2.4, 0.8),
            row(DepositionKind::SparseFill, 8.0, 9.0, 0.8),
        ];
        for interior_anchor in [None, Some(4.0), Some(7.7)] {
            let previous: Vec<_> = [Some(1.7), interior_anchor, Some(8.5)]
                .into_iter()
                .flatten()
                .map(|x| DepositionPath {
                    kind: DepositionKind::SparseFill,
                    points_mm: vec![[x, -1.0, 0.4], [x, 1.0, 0.4]],
                })
                .collect();
            let mut candidate = RasterCandidate {
                paths: required.clone(),
                whole_rows: vec![row(DepositionKind::Bridge, 1.0, 10.0, 0.8)],
            };
            let mut anchors = AnchorWorkspace::default();
            let mut work = 0;
            assert!(
                strokes::analyze(
                    &candidate.paths,
                    previous.iter(),
                    std::iter::empty(),
                    &recipe,
                    &mut anchors,
                    &mut work,
                    &cancel,
                )
                .unwrap()
                .failure
                .is_some(),
                "one endpoint anchor cannot support the original over-limit overhang"
            );
            coalesce_bridge_rows(
                &mut candidate,
                BridgeRowContext {
                    axis: region::HatchAxis::X,
                    retained_points: 0,
                    recipe: &recipe,
                },
                previous.iter(),
                std::iter::empty(),
                &mut anchors,
                &mut work,
                &cancel,
            )
            .unwrap();
            let admitted = strokes::analyze(
                &candidate.paths,
                previous.iter(),
                std::iter::empty(),
                &recipe,
                &mut anchors,
                &mut work,
                &cancel,
            )
            .unwrap();
            if interior_anchor == Some(4.0) {
                assert!(admitted.failure.is_none());
                assert!(
                    candidate.paths.iter().all(|path| {
                        path.points_mm.iter().all(|point| point[0] <= 4.0)
                            || path.points_mm.iter().all(|point| point[0] >= 8.0)
                    }),
                    "the nearest anchor must stop extension well before the unrequested gap is filled"
                );
                for owner in &required {
                    let start = owner.points_mm[0][0].min(owner.points_mm[1][0]);
                    let end = owner.points_mm[0][0].max(owner.points_mm[1][0]);
                    assert!(candidate.paths.iter().any(|actual| {
                        actual.points_mm[0][0].min(actual.points_mm[1][0]) <= start
                            && actual.points_mm[0][0].max(actual.points_mm[1][0]) >= end
                    }));
                }
            } else {
                assert!(admitted.failure.is_some());
                assert_eq!(
                    candidate.paths, required,
                    "absent interior anchors or an extension that erases the current footprint gap cannot repair demand"
                );
            }
        }
    }

    #[test]
    fn current_row_footprint_overlap_is_strict_and_never_a_previous_anchor() {
        let mut recipe = crate::planner::tests::recipe(0.2);
        let cancel = AtomicBool::new(false);
        let unit = super::super::LAYER_QUANTIZATION_MM;
        let end = 4.0;
        for (axis, width) in [
            (region::HatchAxis::X, 0.5),
            (region::HatchAxis::Y, 0.5),
            (region::HatchAxis::X, 0.6),
            (region::HatchAxis::Y, 0.6),
        ] {
            recipe.bead_width_mm = width;
            let orient = |mut path: DepositionPath| {
                if axis == region::HatchAxis::Y {
                    for point in &mut path.points_mm {
                        point.swap(0, 1);
                    }
                }
                path
            };
            let previous = [2.5, 7.5].map(|x| {
                orient(DepositionPath {
                    kind: DepositionKind::SparseFill,
                    points_mm: vec![[x, -1.0, 0.4], [x, 1.0, 0.4]],
                })
            });
            for (left_end, right_start, connected) in [
                (end, end + recipe.bead_width_mm - unit, true),
                // Non-dyadic decimal tangency must not become a positive
                // overlap merely because decoded subtraction rounds below it.
                (end, end + recipe.bead_width_mm, false),
                (end, end + recipe.bead_width_mm + unit, false),
                // A nominal overlap may disappear on the actual emitted row.
                (
                    end + 0.49 * unit,
                    end + recipe.bead_width_mm + 0.01 * unit,
                    false,
                ),
                // A tiny nominal gap may straddle the rounding half-grid.
                (end + 0.49 * unit, end + 0.51 * unit, true),
            ] {
                for missing_previous in [false, true] {
                    let required = vec![
                        orient(row(DepositionKind::TopSkin, 3.2, left_end, 0.8)),
                        orient(row(DepositionKind::SparseFill, right_start, 6.8, 0.8)),
                    ];
                    let mut candidate = RasterCandidate {
                        paths: required.clone(),
                        whole_rows: vec![orient(row(DepositionKind::Bridge, 1.0, 9.0, 0.8))],
                    };
                    let prior = previous.iter().filter(|_| !missing_previous);
                    let mut anchors = AnchorWorkspace::default();
                    let mut work = 0;
                    assert!(
                        strokes::analyze(
                            &candidate.paths,
                            prior.clone(),
                            std::iter::empty(),
                            &recipe,
                            &mut anchors,
                            &mut work,
                            &cancel,
                        )
                        .unwrap()
                        .failure
                        .is_some()
                    );
                    coalesce_bridge_rows(
                        &mut candidate,
                        BridgeRowContext {
                            axis,
                            retained_points: 0,
                            recipe: &recipe,
                        },
                        prior.clone(),
                        std::iter::empty(),
                        &mut anchors,
                        &mut work,
                        &cancel,
                    )
                    .unwrap();
                    let admitted = strokes::analyze(
                        &candidate.paths,
                        prior,
                        std::iter::empty(),
                        &recipe,
                        &mut anchors,
                        &mut work,
                        &cancel,
                    )
                    .unwrap();
                    assert_eq!(
                        admitted.failure.is_none(),
                        connected && !missing_previous,
                        "only positive current footprint connectivity plus real previous anchors can repair the row"
                    );
                    if !connected || missing_previous {
                        assert_eq!(
                            candidate.paths, required,
                            "tangency, missing current material or absent previous anchors cannot create a connector"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn top_skin_rows_extend_only_to_actual_sparse_anchors() {
        let cancel = AtomicBool::new(false);
        let recipe = crate::planner::tests::recipe(0.1);
        let fill = MultiPolygon(vec![polygon![
            (x: 1.0, y: -2.0), (x: 9.0, y: -2.0),
            (x: 9.0, y: 2.0), (x: 1.0, y: 2.0), (x: 1.0, y: -2.0),
        ]]);
        let required = vec![row(DepositionKind::TopSkin, 3.2, 6.8, 0.8)];
        let prior = [2.5, 7.5].map(|x| DepositionPath {
            kind: DepositionKind::SparseFill,
            points_mm: vec![[x, -1.0, 0.4], [x, 1.0, 0.4]],
        });
        let layer = region::HatchLayer {
            z: 0.8,
            height: 0.4,
            axis: region::HatchAxis::X,
            recipe: &recipe,
        };
        for missing_anchor in [false, true] {
            let previous = prior.iter().filter(|_| !missing_anchor);
            let mut candidate = RasterCandidate {
                whole_rows: region::bridge_rows(&fill, &required, &layer, &mut 0, &cancel).unwrap(),
                paths: required.clone(),
            };
            let mut anchors = AnchorWorkspace::default();
            let mut work = 0;
            assert!(
                strokes::analyze(
                    &candidate.paths,
                    previous.clone(),
                    std::iter::empty(),
                    &recipe,
                    &mut anchors,
                    &mut work,
                    &cancel,
                )
                .unwrap()
                .failure
                .is_some()
            );
            coalesce_bridge_rows(
                &mut candidate,
                BridgeRowContext {
                    axis: region::HatchAxis::X,
                    retained_points: 0,
                    recipe: &recipe,
                },
                previous.clone(),
                std::iter::empty(),
                &mut anchors,
                &mut work,
                &cancel,
            )
            .unwrap();
            let admission = strokes::analyze(
                &candidate.paths,
                previous,
                std::iter::empty(),
                &recipe,
                &mut anchors,
                &mut work,
                &cancel,
            )
            .unwrap();
            if missing_anchor {
                assert!(
                    admission.failure.is_some(),
                    "source fill cannot supply an anchor"
                );
            } else {
                assert!(admission.failure.is_none());
                assert!(
                    candidate.paths.iter().any(|actual| {
                        actual.kind == DepositionKind::Bridge
                            && actual.points_mm[0][0].min(actual.points_mm[1][0]) <= 3.2
                            && actual.points_mm[0][0].max(actual.points_mm[1][0]) >= 6.8
                            && actual
                                .points_mm
                                .iter()
                                .all(|point| (1.0..=9.0).contains(&point[0]))
                    }),
                    "a genuinely anchored bridge must retain the required skin inside positive fill"
                );
            }
        }
    }

    #[test]
    fn pre_floor_skin_deposits_anchors_under_upcoming_pocket_perimeters() {
        use crate::planner::classification::{ClassifiedRegions, WallLayout, fixed_point_budget};
        let cancel = AtomicBool::new(false);
        let recipe = crate::planner::tests::recipe(0.25);
        let section = MultiPolygon(vec![polygon![
            (x: 0.0, y: 0.0), (x: 10.0, y: 0.0),
            (x: 10.0, y: 10.0), (x: 0.0, y: 10.0), (x: 0.0, y: 0.0),
        ]]);
        let upper = MultiPolygon(vec![Polygon::new(
            section.0[0].exterior().clone(),
            vec![line_string![
                (x: 4.0, y: 4.0), (x: 4.0, y: 6.0),
                (x: 6.0, y: 6.0), (x: 6.0, y: 4.0), (x: 4.0, y: 4.0),
            ]],
        )]);
        let layout = |material: &MultiPolygon<f64>| {
            Arc::new(
                WallLayout::new(
                    material,
                    &region::RequiredMaterial::from_section(material),
                    &recipe,
                    region::point_count(material),
                    &cancel,
                )
                .unwrap(),
            )
        };
        let inset = (recipe.perimeter_count as f64 - 0.5) * recipe.bead_width_mm;
        let section_inset = region::offset(&section, -inset, &cancel).unwrap();
        let upper_inset = region::offset(&upper, -inset, &cancel).unwrap();
        let previous = ClassifiedRegions::new(
            layout(&section),
            &section_inset,
            &upper_inset,
            fixed_point_budget(0),
            &recipe,
            &cancel,
        )
        .unwrap()
        .paths(0.4, 0.4, region::HatchAxis::X, &recipe, &mut 0, &cancel)
        .unwrap()
        .0;
        let upcoming = ClassifiedRegions::new(
            layout(&upper),
            &section_inset,
            &upper_inset,
            fixed_point_budget(0),
            &recipe,
            &cancel,
        )
        .unwrap()
        .paths(0.8, 0.4, region::HatchAxis::Y, &recipe, &mut 0, &cancel)
        .unwrap()
        .0
        .into_iter()
        .filter(|path| path.kind == DepositionKind::Perimeter)
        .collect::<Vec<_>>();
        let admission = strokes::analyze(
            &upcoming,
            previous.iter(),
            std::iter::empty(),
            &recipe,
            &mut AnchorWorkspace::default(),
            &mut 0,
            &cancel,
        )
        .unwrap();
        assert!(
            admission.failure.is_none(),
            "future walls need real deposited anchors"
        );
        assert!(
            admission.bridges.is_empty(),
            "a curved pocket rim cannot borrow straight-bridge authority"
        );
    }

    #[test]
    fn anchor_extension_cannot_cross_a_positive_fill_hole() {
        let cancel = AtomicBool::new(false);
        let recipe = crate::planner::tests::recipe(0.1);
        let exterior = geo::line_string![
            (x: 1.0, y: -2.0), (x: 9.0, y: -2.0),
            (x: 9.0, y: 2.0), (x: 1.0, y: 2.0), (x: 1.0, y: -2.0),
        ];
        let hole = geo::line_string![
            (x: 4.2, y: -1.0), (x: 4.2, y: 1.0),
            (x: 5.8, y: 1.0), (x: 5.8, y: -1.0), (x: 4.2, y: -1.0),
        ];
        let fill = MultiPolygon(vec![Polygon::new(exterior, vec![hole])]);
        let required = vec![
            row(DepositionKind::TopSkin, 3.0, 4.0, 0.8),
            row(DepositionKind::TopSkin, 6.0, 7.0, 0.8),
        ];
        let mut candidate = RasterCandidate {
            whole_rows: region::bridge_rows(
                &fill,
                &required,
                &region::HatchLayer {
                    z: 0.8,
                    height: 0.4,
                    axis: region::HatchAxis::X,
                    recipe: &recipe,
                },
                &mut 0,
                &cancel,
            )
            .unwrap(),
            paths: required.clone(),
        };
        assert_eq!(candidate.whole_rows.len(), 2);
        let previous = [
            row(DepositionKind::SparseFill, 1.0, 2.0, 0.4),
            row(DepositionKind::SparseFill, 8.0, 9.0, 0.4),
        ];
        let mut anchors = AnchorWorkspace::default();
        assert!(
            !coalesce_bridge_rows(
                &mut candidate,
                BridgeRowContext {
                    axis: region::HatchAxis::X,
                    retained_points: 0,
                    recipe: &recipe,
                },
                previous.iter(),
                std::iter::empty(),
                &mut anchors,
                &mut 0,
                &cancel,
            )
            .unwrap()
        );
        assert_eq!(candidate.paths, required);
        assert!(
            strokes::analyze(
                &candidate.paths,
                previous.iter(),
                std::iter::empty(),
                &recipe,
                &mut anchors,
                &mut 0,
                &cancel,
            )
            .unwrap()
            .failure
            .is_some()
        );
    }

    #[test]
    fn auxiliary_bridge_bounds_ignore_unrequested_tangent_and_thin_intervals() {
        let cancel = AtomicBool::new(false);
        let recipe = crate::planner::tests::recipe(0.1);
        let layer = region::HatchLayer {
            z: 0.8,
            height: 0.4,
            axis: region::HatchAxis::X,
            recipe: &recipe,
        };
        let required = vec![DepositionPath {
            kind: DepositionKind::TopSkin,
            points_mm: vec![[1.0, 2.0, 0.8], [9.0, 2.0, 0.8]],
        }];
        let polygon = |points: Vec<(f64, f64)>| Polygon::new(geo::LineString::from(points), vec![]);
        for tip in [2.0, 2.0 - 0.00000001] {
            let isolated = MultiPolygon(vec![
                polygon(vec![
                    (0.0, 0.0),
                    (10.0, 0.0),
                    (10.0, 4.0),
                    (0.0, 4.0),
                    (0.0, 0.0),
                ]),
                polygon(vec![(20.0, 4.0), (22.0, tip), (24.0, 4.0), (20.0, 4.0)]),
            ]);
            let connected = MultiPolygon(vec![polygon(vec![
                (0.0, 0.0),
                (10.0, 0.0),
                (10.0, 3.0),
                (20.0, 3.0),
                (22.0, tip),
                (24.0, 3.0),
                (24.0, 4.0),
                (0.0, 4.0),
                (0.0, 0.0),
            ])]);
            for fill in [&isolated, &connected] {
                let rows = region::bridge_rows(fill, &required, &layer, &mut 0, &cancel).unwrap();
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].points_mm, [[0.0, 2.0, 0.8], [10.0, 2.0, 0.8]]);
            }
            // The same thin interval is still a genuine error if ordinary
            // deposition requests it. Only auxiliary, unowned bounds are skipped.
            assert_eq!(
                region::hatch(
                    &connected,
                    DepositionKind::SparseFill,
                    f64::MIN_POSITIVE,
                    &layer,
                    &mut Vec::new(),
                    &mut 0,
                    &cancel,
                )
                .unwrap_err()
                .code,
                ManufacturingErrorCode::UnsupportedGeometry,
            );
        }
    }
}
