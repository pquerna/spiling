// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::{SectionSpan, boolean_budget, region, span_at};
use crate::{check_cancel, error};
use geo::{Area, BooleanOps, MultiPolygon, Polygon};
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

fn merge(
    into: &mut MultiPolygon<f64>,
    other: &MultiPolygon<f64>,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    if other.0.is_empty() {
        return Ok(());
    }
    if into.0.is_empty() {
        *into = other.clone();
    } else {
        boolean_budget(into, other, cancel)?;
        *into = into.union(other);
    }
    check_cancel(cancel)?;
    region::check_size(into)?;
    region::canonicalize(into);
    Ok(())
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

// SectionSpan.material is the same admitted D (native M union finish A) used
// by walls/classification and the raster callback. Never query native-only
// sections here: actual-stroke anchors and all lower corridor clearance use final D.
pub(super) fn realize(
    layers: &mut [PrintLayer],
    sections: &[SectionSpan],
    native_points: usize,
    recipe: &PlanarPrintRecipe,
    segments: &mut usize,
    cancel: &AtomicBool,
    mut raster: impl FnMut(
        usize,
        region::HatchAxis,
        &mut usize,
        usize,
    ) -> Result<RasterCandidate, ManufacturingError>,
) -> Result<(), ManufacturingError> {
    let width = recipe.bead_width_mm;
    let mut demands: Vec<Demand> = Vec::new();
    let retained = native_points;
    let mut work = 0usize;
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
            &mut work,
            cancel,
        )?;
        if admission.failure.is_some() {
            // Test both allowed axes against actual strokes. A joined bridge row
            // uses only already-required pieces of realized perimeter-owned fill.
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
                    &mut work,
                    cancel,
                )?;
                anchors.set_budget(candidate_budget(&candidate.paths, retained)?);
                let other = strokes::analyze(
                    &candidate.paths,
                    previous.clone(),
                    std::iter::empty(),
                    recipe,
                    &mut anchors,
                    &mut work,
                    cancel,
                )?;
                if other.failure.is_none() {
                    *segments = without_old + path_segments(&candidate.paths);
                    layer.paths = candidate.paths;
                    axes[index] = axis;
                    joined_layers[index] = joined;
                    admission = other;
                    break;
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
    // Protect every source hole in projection, not merely a layer's material.
    // This conservative policy never fills a through-hole with breakaway support.
    let mut protected = MultiPolygon(Vec::new());
    for section in sections {
        for polygon in &section.material.0 {
            for hole in polygon.interiors() {
                let hole = MultiPolygon(vec![Polygon::new(hole.clone(), vec![])]);
                remaining_geometry_budget(retained, &[], &[], &[&protected, &hole])?;
                charge_boolean(&protected, &hole, &mut work, cancel)?;
                merge(&mut protected, &hole, cancel)?;
                remaining_geometry_budget(retained, &[], &[], &[&protected])?;
            }
        }
    }
    let protected = region::offset(&protected, xy_clearance_mm, cancel)?;
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
            charge_contact_work(&mut work, cancel)?;
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
            check_cancel(cancel)?;
            charge_boolean(&section.material, &section.material, &mut work, cancel)?;
            let clearance = region::offset(&section.material, xy_clearance_mm, cancel)?;
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
        charge_boolean(&column, &protected, &mut work, cancel)?;
        column = column.difference(&protected);
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
                ManufacturingErrorCode::UnsupportedGeometry,
                format!(
                    "layer {} Z {:.9}: object/protected geometry blocks every bed-anchored support corridor; role {:?}, stroke {:?} to {:?}, gap ({:.12},{:.12}) span {:.12}; previous layer {} Z {:.9}, nearest start {:?}, nearest end {:?}; nominal bead width {:.9} mm, XY clearance {:.9} mm, Z gap {:.9} mm, bridge limit {:.9} mm",
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
                &mut work,
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
                &mut work,
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
            &mut work,
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
                    &mut work,
                    cancel,
                )?;
                anchors.set_budget(candidate_budget(&candidate.paths, retained_regions)?);
                let other = strokes::analyze(
                    &candidate.paths,
                    previous.clone(),
                    contact.clone(),
                    recipe,
                    &mut anchors,
                    &mut work,
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
                charge_contact_work(&mut work, cancel)?;
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
                charge_contact_work(&mut work, cancel)?;
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
    let grid = |value: f64| (value / super::LAYER_QUANTIZATION_MM).round_ties_even();
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
        let row = grid(a[fixed]);
        if row != grid(b[fixed]) {
            continue;
        }
        let lo = grid(a[along]).min(grid(b[along]));
        let hi = grid(a[along]).max(grid(b[along]));
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
    let mut removed = vec![false; original_len];
    let mut joined = false;
    for row in std::mem::take(&mut candidate.whole_rows) {
        check_cancel(cancel)?;
        let key = grid(row.points_mm[0][fixed]);
        let lo = grid(row.points_mm[0][along]).min(grid(row.points_mm[1][along]));
        let hi = grid(row.points_mm[0][along]).max(grid(row.points_mm[1][along]));
        let begin = pieces.partition_point(|piece| piece.0 < key);
        let end = pieces.partition_point(|piece| piece.0 <= key);
        let mut covered = lo;
        let mut count = 0;
        let mut contiguous = true;
        for &(.., start, stop, index) in &pieces[begin..end] {
            *work = work
                .checked_add(1)
                .filter(|n| *n <= 20_000_000)
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "cumulative bridge row ownership work budget exceeded",
                    )
                })?;
            if removed[index] || start < lo || stop > hi {
                continue;
            }
            if start != covered || stop <= start {
                contiguous = false;
                break;
            }
            covered = stop;
            count += 1;
        }
        if !contiguous || covered != hi || count < 2 {
            continue;
        }
        let proof = strokes::analyze(
            std::slice::from_ref(&row),
            previous.clone(),
            contacts.clone(),
            recipe,
            anchors,
            work,
            cancel,
        )?;
        if proof.failure.is_some() || proof.bridges.is_empty() {
            continue;
        }
        for &(_, start, stop, index) in &pieces[begin..end] {
            if start >= lo && stop <= hi {
                removed[index] = true;
            }
        }
        candidate.paths.push(row);
        joined = true;
    }
    let mut index = 0;
    candidate.paths.retain(|_| {
        let keep = index >= original_len || !removed[index];
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

fn charge_contact_work(work: &mut usize, cancel: &AtomicBool) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    *work = work
        .checked_add(1)
        .filter(|n| *n <= 20_000_000)
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "cumulative support contact projection work budget exceeded",
            )
        })?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
