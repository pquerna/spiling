// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Actual commanded-wall occupancy, independent of the beading/count field.
//! Required material is source-derived before this module sees the paths.
use super::*;
use geo::algorithm::convex_hull::quick_hull;
use geo::{
    BooleanOps, BoundingRect, Contains, Coord, Distance, Euclidean, Intersects, Line, LineString,
    Point, unary_union,
};

const DISK_SIDES: usize = 64;
const COMMAND_CAPSULE_VERTICES: usize = DISK_SIDES * 2 + 4;
const AREA_ROUNDOFF: f64 = 0.0000001;

// Input-independent polygon directions. Preserve the original angle arithmetic,
// separate sin/cos calls, vertex order and radius multiplication at each use.
static DISK_DIRECTIONS: std::sync::LazyLock<[[f64; 2]; DISK_SIDES]> =
    std::sync::LazyLock::new(|| {
        std::array::from_fn(|i| {
            let angle = std::f64::consts::TAU * i as f64 / DISK_SIDES as f64;
            [angle.cos(), angle.sin()]
        })
    });

type UnionRegion = (MultiPolygon<f64>, geo::Rect<f64>, usize);
type PrefixComponent = (Polygon<f64>, geo::Rect<f64>);

// An overlap permission, never a material/coverage certificate. The ball
// enclosure stays inside the inscribed source-junction permission polygon.
pub(super) fn junction_contains_capsule(
    center: [f64; 2],
    points: [[f64; 2]; 2],
    radius: f64,
    limit: f64,
) -> bool {
    let arithmetic = 256.0
        * f64::EPSILON
        * center
            .into_iter()
            .fold(limit.max(1.0), |a, b| a.max(b.abs()));
    points.into_iter().all(|p| {
        norm(sub(p, center)) + radius
            <= limit * (std::f64::consts::PI / DISK_SIDES as f64).cos() - arithmetic
    })
}

pub(super) fn permission_contains_capsule(
    permission: &Junction,
    points: [[f64; 2]; 2],
    radius: f64,
) -> bool {
    if let Some(corridor) = &permission.transition {
        let reach = permission.radius_mm * (std::f64::consts::PI / DISK_SIDES as f64).cos();
        corridor.points.windows(2).any(|edge| {
            let line = Line::new(Coord::from(edge[0]), Coord::from(edge[1]));
            points
                .into_iter()
                .all(|p| Euclidean.distance(&Point::from(p), &line) + radius < reach)
        })
    } else {
        junction_contains_capsule(permission.center, points, radius, permission.radius_mm)
    }
}

pub(super) fn qualify(
    request: &WallRequest<'_>,
    paths: &[DepositionPath],
    permissions: &[Junction],
    nominal_core: &MultiPolygon<f64>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    let mut phase = "target";
    let mut boundaries = [None; 10];
    boundaries[0] = Some(*work);
    let outcome = (|| {
        let mut live = *request;
        let request = &mut live;
        let mut required_wall = overlay(
            request.material,
            nominal_core,
            Operation::Difference,
            request,
            work,
            cancel,
        )?;
        retain_region(request, &mut required_wall)?;
        // C/native mandatory coverage remains separate from requested D. Only the
        // caller's already-certified image-owned exception can omit additions here.
        boolean::budget_parts_accumulated(
            &request.required.polygons,
            &required_wall,
            work,
            cancel,
        )?;
        let mut mandatory_wall = request.required.intersection(&required_wall);
        region::check_size(&mandatory_wall)?;
        retain_region(request, &mut mandatory_wall)?;
        boundaries[1] = Some(*work);
        phase = "inner occupancy";

        let mut inner = occupancy(
            paths,
            -program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM,
            request,
            work,
            cancel,
        )?;
        retain_region(request, &mut inner)?;
        boundaries[2] = Some(*work);
        phase = "nominal occupancy";

        let mut nominal = occupancy(paths, 0.0, request, work, cancel)?;
        retain_region(request, &mut nominal)?;
        boundaries[3] = Some(*work);
        phase = "outer occupancy";

        let mut outer = occupancy(
            paths,
            program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM,
            request,
            work,
            cancel,
        )?;
        retain_region(request, &mut outer)?;
        boundaries[4] = Some(*work);
        phase = "source/component coverage";

        exclude_source(paths, request, work, cancel)?;

        // Every original material component keeps its own allowance and occupancy.
        // A large housing cannot pay for a missing detached rib or small island.
        let mut missing = overlay(
            &mandatory_wall,
            &inner,
            Operation::Difference,
            request,
            work,
            cancel,
        )?;
        retain_region(request, &mut missing)?;
        for component in &request.material.0 {
            check_cancel(cancel)?;
            let mut component_request = *request;
            let points = component.exterior().0.len()
                + component
                    .interiors()
                    .iter()
                    .map(|ring| ring.0.len())
                    .sum::<usize>();
            staging(&component_request, points * 48)?;
            let mut domain = MultiPolygon(vec![component.clone()]);
            retain_region(&mut component_request, &mut domain)?;
            let request = &mut component_request;
            let mut target = overlay(
                &mandatory_wall,
                &domain,
                Operation::Intersection,
                request,
                work,
                cancel,
            )?;
            retain_region(request, &mut target)?;
            let mut defect = overlay(
                &missing,
                &domain,
                Operation::Intersection,
                request,
                work,
                cancel,
            )?;
            retain_region(request, &mut defect)?;
            if defect.unsigned_area() > target.unsigned_area() * 0.02 + AREA_ROUNDOFF {
                return Err(unsupported(&format!(
                    "wall-required component underfill exceeds 2%: missing {} of {} mm2",
                    defect.unsigned_area(),
                    target.unsigned_area()
                )));
            }
            let occupied = overlay(
                &inner,
                &domain,
                Operation::Intersection,
                request,
                work,
                cancel,
            )?;
            if target.unsigned_area() > AREA_ROUNDOFF && occupied.0.is_empty() {
                return Err(unsupported(
                    "required material component has no guaranteed-inner wall occupancy",
                ));
            }
        }
        boundaries[5] = Some(*work);
        phase = "underfill extent";

        let extent = request.nozzle_diameter_mm * 0.25;
        // The shared offset's circumscribed disk adds at most .0025 mm. Subtract
        // that bound before construction, so this witness stays within the adopted
        // inward defect extent, rather than enlarging it by the offset policy.
        let mut reach = region::offset_admitted(
            &inner,
            extent - super::super::OFFSET_TOLERANCE_MM * 0.5,
            |scratch, scratch_bytes, pair| {
                if request
                    .retained_points
                    .checked_add(scratch)
                    .is_none_or(|units| units > super::super::MAX_RETAINED_GEOMETRY_UNITS)
                {
                    return Err(resource(
                        "wall underfill reach geometry reservation exceeded",
                    ));
                }
                staging(request, scratch_bytes)?;
                if let Some((a, b)) = pair {
                    boolean::budget_accumulated(a, b, work, cancel)?;
                }
                Ok(())
            },
            cancel,
        )?;
        retain_region(request, &mut reach)?;
        if overlay(
            &missing,
            &reach,
            Operation::Difference,
            request,
            work,
            cancel,
        )?
        .unsigned_area()
            > AREA_ROUNDOFF
        {
            return Err(unsupported(
                "wall underfill exceeds the source-local inward extent allowance",
            ));
        }
        boundaries[6] = Some(*work);
        phase = "source permissions";

        let required_area = required_wall.unsigned_area();
        // Coverage and extent are now qualified. Release their owned witnesses
        // before constructing permissions; only the immutable target area is still
        // needed for the aggregate excess denominator.
        for mut witness in [required_wall, mandatory_wall, missing, reach] {
            request.owned_bytes = request
                .owned_bytes
                .checked_sub(super::super::native::material_bytes(&mut witness))
                .ok_or_else(|| resource("wall witness ownership underflow"))?;
        }
        // Reuse HOST admission metadata only. All full source operands still reach
        // the same overlay kernel in their original order and float frame.

        let nominal_prepared = if permissions.is_empty() {
            None
        } else {
            boolean::Prepared::new(&nominal, available_stage(request)?, work, cancel)?
        };
        let nominal_metadata = nominal_prepared
            .as_ref()
            .map_or(0, boolean::Prepared::retained_bytes);
        *request = staged_request(request, nominal_metadata)?;
        let material_prepared = if permissions.is_empty() {
            None
        } else {
            boolean::Prepared::new(request.material, available_stage(request)?, work, cancel)?
        };
        let material_metadata = material_prepared
            .as_ref()
            .map_or(0, boolean::Prepared::retained_bytes);
        *request = staged_request(request, material_metadata)?;
        let prepared_metadata = nominal_metadata
            .checked_add(material_metadata)
            .ok_or_else(|| resource("wall prepared metadata inventory overflow"))?;
        // This capacity is not in request: prepared helpers admit it themselves;
        // every other helper receives an explicit current-capacity receipt.
        let mut workspace = boolean::Workspace::new();

        // Preserve original source order and regions for the final balanced union.
        // A normalized prefix supplies exactly P_i \ union(P_0..P_{i-1}) with
        // one difference against only its conservative interacting components.
        let mut regions = Vec::<UnionRegion>::new();
        let mut regions_bytes = 0usize;
        let mut seen = Vec::<PrefixComponent>::new();
        let mut seen_bytes = 0usize;
        let mut allowance = 0.0f64;
        for permission in permissions {
            check_cancel(cancel)?;
            let prefix_bytes = permission_prefix_bytes(&seen, seen_bytes)?;
            let local = staged_request(
                request,
                regions_bytes
                    .checked_add(regions.capacity() * std::mem::size_of::<UnionRegion>())
                    .and_then(|n| n.checked_add(prefix_bytes))
                    .ok_or_else(|| resource("wall permission prefix live-byte overflow"))?,
            )?;
            let construction_request = staged_request(&local, workspace.retained_bytes())?;
            let mut raw_region =
                permission_region(permission, &construction_request, work, cancel)?;
            let raw_bytes = super::super::native::material_bytes(&mut raw_region);
            let raw_request = staged_request(&local, raw_bytes)?;
            // R is a minimum coverage target, not a ceiling on finite transitions.
            // Original D remains the material authority and R the 5% denominator.
            let mut region = match material_prepared.as_ref() {
                Some(prepared) => prepared_raw_intersection(
                    &raw_region,
                    prepared,
                    &raw_request,
                    &mut workspace,
                    work,
                    cancel,
                ),
                None => overlay(
                    &raw_region,
                    request.material,
                    Operation::Intersection,
                    &staged_request(&raw_request, workspace.retained_bytes())?,
                    work,
                    cancel,
                ),
            }?;
            drop(raw_region);
            let region_bytes = super::super::native::material_bytes(&mut region);
            let region_request = staged_request(&local, region_bytes)?;
            boolean::charge(work, region::point_count(&region))?;
            let Some(bounds) = region.bounding_rect() else {
                continue;
            };
            let prefix_request = staged_request(&region_request, workspace.retained_bytes())?;
            let mut hit = take_permission_prefix_hit(
                &mut seen,
                &mut seen_bytes,
                bounds,
                &prefix_request,
                work,
                cancel,
            )?;
            // Moving the hit polygons did not duplicate their coordinate buffers:
            // region_request still counts every old prefix payload exactly once.
            let hit_metadata = hit.0.capacity() * std::mem::size_of::<Polygon<f64>>();
            let hit_request = staged_request(&region_request, hit_metadata)?;
            let mut fresh_owned = if hit.0.is_empty() {
                None
            } else {
                Some(overlay(
                    &region,
                    &hit,
                    Operation::Difference,
                    &staged_request(&hit_request, workspace.retained_bytes())?,
                    work,
                    cancel,
                )?)
            };
            let fresh_bytes = fresh_owned
                .as_mut()
                .map_or(0, super::super::native::material_bytes);
            let fresh_request = staged_request(&hit_request, fresh_bytes)?;
            let fresh = fresh_owned.as_ref().unwrap_or(&region);
            if !fresh.0.is_empty() {
                let fraction = if region.unsigned_area() == 0.0 {
                    0.0
                } else {
                    fresh.unsigned_area() / region.unsigned_area()
                };
                let limit = 0.5
                    * permission.valence.saturating_sub(1) as f64
                    * request.limits.max_bead_width_mm.powi(2);
                qualify_local_dose(
                    paths,
                    &nominal,
                    nominal_prepared.as_ref(),
                    fresh,
                    fraction * limit,
                    &fresh_request,
                    &mut workspace,
                    work,
                    cancel,
                )?;
                allowance += fraction * limit;
            }
            drop(fresh_owned);

            let connection_request = staged_request(&hit_request, workspace.retained_bytes())?;
            local_connections(
                &inner,
                &region,
                permission,
                &connection_request,
                work,
                cancel,
            )?;
            extend_permission_prefix(
                &mut seen,
                &mut seen_bytes,
                &mut hit,
                &region,
                &connection_request,
                work,
                cancel,
            )?;
            let growth_bytes = permission_prefix_bytes(&seen, seen_bytes)?
                .checked_add(workspace.retained_bytes())
                .ok_or_else(|| resource("wall permission workspace inventory overflow"))?;
            let growth_request = staged_request(request, growth_bytes)?;
            reserve_stage(
                &mut regions,
                1,
                &growth_request,
                regions_bytes + region_bytes,
            )?;
            regions_bytes = regions_bytes
                .checked_add(region_bytes)
                .ok_or_else(|| resource("wall permission-region ownership overflow"))?;
            let ordinal = regions.len();
            regions.push((region, bounds, ordinal));
        }
        drop(seen);
        drop(nominal_prepared);
        drop(material_prepared);
        drop(workspace);
        request.owned_bytes = request
            .owned_bytes
            .checked_sub(prepared_metadata)
            .ok_or_else(|| resource("wall prepared metadata ownership underflow"))?;

        let mut authorized = union_regions(regions, regions_bytes, request, work, cancel)?;
        retain_region(request, &mut authorized)?;
        boundaries[7] = Some(*work);
        phase = "aggregate/overlap";

        let mut strip_area = 0.0;
        for path in paths {
            for edge in path.checked_edges()? {
                let a = [edge.points_mm[0][0], edge.points_mm[0][1]];
                let b = [edge.points_mm[1][0], edge.points_mm[1][1]];
                strip_area += norm(sub(b, a)) * edge.width_mm;
            }
        }
        // No per-segment stadium/cap summation: harmless G1 subdivision leaves
        // both this strip integral and the whole continuing-tube union unchanged.
        let excess = (strip_area - nominal.unsigned_area()).max(0.0);
        if !excess.is_finite()
            || excess > required_area * 0.05 + AREA_ROUNDOFF
            || excess > allowance + AREA_ROUNDOFF
        {
            return Err(unsupported(&format!(
                "wall excess exceeds source-authorized local/aggregate allowance: {excess} mm2, local {allowance}, target {required_area}"
            )));
        }
        qualify_overlap_locations(paths, permissions, &authorized, request, work, cancel)?;
        boundaries[8] = Some(*work);
        phase = "residual ownership";

        // The source target above is immutable. Only residual ownership changes:
        // actual outer wall occupancy is removed from the nominal core before any
        // skin/hatch consumes it, including a transition spilling into that core.
        let fill = overlay(
            nominal_core,
            &outer,
            Operation::Difference,
            request,
            work,
            cancel,
        )?;
        boundaries[9] = Some(*work);
        Ok(fill)
    })();

    outcome.map_err(|mut failure: ManufacturingError| {
        if failure.code == ManufacturingErrorCode::ResourceLimit {
            failure.message = format!(
                "{}; wall phase={phase}, cumulative boundaries[entry,target,inner,nominal,outer,components,reach,permissions,overlap,end]={boundaries:?}, current={}",
                failure.message, *work,
            );
        }
        failure
    })
}

fn permission_prefix_bytes(
    seen: &Vec<PrefixComponent>,
    payload: usize,
) -> Result<usize, ManufacturingError> {
    seen.capacity()
        .checked_mul(std::mem::size_of::<PrefixComponent>())
        .and_then(|n| n.checked_add(payload))
        .ok_or_else(|| resource("wall permission prefix inventory overflow"))
}

fn prefix_bounds_union(a: geo::Rect<f64>, b: geo::Rect<f64>) -> geo::Rect<f64> {
    geo::Rect::new(
        Coord {
            x: a.min().x.min(b.min().x),
            y: a.min().y.min(b.min().y),
        },
        Coord {
            x: a.max().x.max(b.max().x),
            y: a.max().y.max(b.max().y),
        },
    )
}
fn padded_prefix_bounds(
    bounds: geo::Rect<f64>,
    padding: f64,
) -> Result<geo::Rect<f64>, ManufacturingError> {
    let min = [
        (bounds.min().x - padding).next_down(),
        (bounds.min().y - padding).next_down(),
    ];
    let max = [
        (bounds.max().x + padding).next_up(),
        (bounds.max().y + padding).next_up(),
    ];
    if min.into_iter().chain(max).any(|v| !v.is_finite()) {
        return Err(resource("wall permission prefix bounds overflow"));
    }
    Ok(geo::Rect::new(Coord::from(min), Coord::from(max)))
}

fn take_permission_prefix_hit(
    seen: &mut Vec<PrefixComponent>,
    payload: &mut usize,
    bounds: geo::Rect<f64>,
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    let mut combined = bounds;
    for (_, previous) in seen.iter() {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        combined = prefix_bounds_union(combined, *previous);
    }
    let padding = boolean::overlay_padding(
        [combined.min().x, combined.min().y],
        [combined.max().x, combined.max().y],
    )?;
    let mut closure = bounds;
    let mut hit = MultiPolygon(Vec::new());
    loop {
        let mut changed = false;
        let mut index = 0;
        while index < seen.len() {
            check_cancel(cancel)?;
            boolean::charge(work, 1)?;
            // Close over WHOLE component boxes. Re-overlay can round a long
            // selected component's far boundary, not just the new P_i patch.
            if padded_prefix_bounds(closure, padding)?
                .intersects(&padded_prefix_bounds(seen[index].1, padding)?)
            {
                // request already owns all original prefix geometry and its
                // component vector; only hit's old/new polygon records grow.
                reserve_stage(&mut hit.0, 1, request, 0)?;
                let (mut polygon, previous) = seen.swap_remove(index);
                let bytes = super::super::native::polygon_bytes(&mut polygon);
                *payload = payload
                    .checked_sub(bytes)
                    .ok_or_else(|| resource("wall permission prefix extraction underflow"))?;
                closure = prefix_bounds_union(closure, previous);
                hit.0.push(polygon);
                changed = true;
            } else {
                index += 1;
            }
        }
        if !changed {
            return Ok(hit);
        }
    }
}

fn extend_permission_prefix(
    seen: &mut Vec<PrefixComponent>,
    payload: &mut usize,
    hit: &mut MultiPolygon<f64>,
    region: &MultiPolygon<f64>,
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    // Union ORIGINAL P_i, never merely the numerically reconstructed fresh
    // difference. The invariant is seen == union of all original prior P_i.
    let mut joined = overlay(hit, region, Operation::Union, request, work, cancel)?;
    let joined_bytes = super::super::native::material_bytes(&mut joined);
    staging(request, joined_bytes)?;
    let hit_bytes = super::super::native::material_bytes(hit);
    let old_metadata = seen.capacity() * std::mem::size_of::<PrefixComponent>();
    // Admit the new result while the old hit still exists, then release that
    // consumed owner before any component-vector replacement allocation.
    let mut growth_request = *request;
    growth_request.owned_bytes = growth_request
        .owned_bytes
        .checked_sub(hit_bytes)
        .and_then(|n| n.checked_sub(old_metadata))
        .ok_or_else(|| resource("wall permission prefix replacement underflow"))?;
    *hit = MultiPolygon(Vec::new());
    reserve_stage(seen, joined.0.len(), &growth_request, joined_bytes)?;
    for mut polygon in joined.0 {
        check_cancel(cancel)?;
        let points = polygon.exterior().0.len()
            + polygon
                .interiors()
                .iter()
                .map(|ring| ring.0.len())
                .sum::<usize>();
        boolean::charge(work, points)?;
        let bounds = polygon
            .bounding_rect()
            .ok_or_else(|| resource("wall permission prefix component has no bounds"))?;
        let bytes = super::super::native::polygon_bytes(&mut polygon);
        *payload = payload
            .checked_add(bytes)
            .ok_or_else(|| resource("wall permission prefix payload overflow"))?;
        seen.push((polygon, bounds));
    }
    staging(
        &growth_request,
        seen.capacity()
            .checked_mul(std::mem::size_of::<PrefixComponent>())
            .and_then(|metadata| metadata.checked_add(joined_bytes))
            .ok_or_else(|| resource("wall permission prefix post-growth overflow"))?,
    )
}

fn union_regions(
    mut regions: Vec<UnionRegion>,
    mut bytes: usize,
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    let metadata = regions
        .capacity()
        .checked_mul(std::mem::size_of::<UnionRegion>())
        .ok_or_else(|| resource("wall region union metadata overflow"))?;
    staging(
        request,
        bytes
            .checked_add(metadata)
            .ok_or_else(|| resource("wall region union ownership overflow"))?,
    )?;
    union_region_subtree(&mut regions, &mut bytes, metadata, request, work, cancel)?;
    // All children have been moved/dropped. Move the root out; dropping this
    // existing vector also releases the reduction metadata before retention.
    Ok(regions.first_mut().map_or_else(
        || MultiPolygon(Vec::new()),
        |root| std::mem::replace(&mut root.0, MultiPolygon(Vec::new())),
    ))
}

fn union_region_subtree(
    regions: &mut [UnionRegion],
    bytes: &mut usize,
    metadata: usize,
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    if regions.len() < 2 {
        return Ok(());
    }
    // Spatial median splits keep normalized region reduction balanced, even
    // for coincident/clustered inputs. Permission freshness and local budgets
    // are frozen before this helper; occupancy batches have no local budgets.
    let mut min = [f64::INFINITY; 2];
    let mut max = [f64::NEG_INFINITY; 2];
    for (_, bounds, _) in regions.iter() {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        min[0] = min[0].min(bounds.min().x);
        min[1] = min[1].min(bounds.min().y);
        max[0] = max[0].max(bounds.max().x);
        max[1] = max[1].max(bounds.max().y);
    }
    let axis = usize::from(max[1] - min[1] > max[0] - min[0]);
    let center = |entry: &UnionRegion| {
        let bounds = entry.1;
        if axis == 0 {
            bounds.min().x * 0.5 + bounds.max().x * 0.5
        } else {
            bounds.min().y * 0.5 + bounds.max().y * 0.5
        }
    };
    boolean::sort_by(regions, center, work, cancel)?;
    // The shared cancellable heapsort need not be stable. Restore original
    // input ordinals within equal spatial keys, without another buffer.
    let mut start = 0;
    while start < regions.len() {
        let key = center(&regions[start]);
        let mut end = start + 1;
        while end < regions.len() {
            check_cancel(cancel)?;
            boolean::charge(work, 1)?;
            if !center(&regions[end]).total_cmp(&key).is_eq() {
                break;
            }
            end += 1;
        }
        if end - start > 1 {
            boolean::sort_by(
                &mut regions[start..end],
                |entry| entry.2 as f64,
                work,
                cancel,
            )?;
        }
        start = end;
    }
    let middle = regions.len() / 2;
    let (left, right) = regions.split_at_mut(middle);
    union_region_subtree(left, bytes, metadata, request, work, cancel)?;
    union_region_subtree(right, bytes, metadata, request, work, cancel)?;
    boolean::charge(work, 1)?;
    // bytes includes EVERY still-live subtree, not just these two inputs.
    // Existing overlay admission supplies its ordinary Boolean scratch; all
    // old input capacities, the new result and metadata coexist until commit.
    let live = staged_request(
        request,
        bytes
            .checked_add(metadata)
            .ok_or_else(|| resource("wall region union live-byte overflow"))?,
    )?;
    let mut joined = overlay(
        &left[0].0,
        &right[0].0,
        Operation::Union,
        &live,
        work,
        cancel,
    )?;
    let joined_bytes = super::super::native::material_bytes(&mut joined);
    staging(&live, joined_bytes)?;
    let left_bytes = super::super::native::material_bytes(&mut left[0].0);
    let right_bytes = super::super::native::material_bytes(&mut right[0].0);
    let next_bytes = left_bytes
        .checked_add(right_bytes)
        .and_then(|old| bytes.checked_sub(old))
        .and_then(|remaining| remaining.checked_add(joined_bytes))
        .ok_or_else(|| resource("wall region union replacement-byte overflow"))?;
    left[0].0 = joined;
    right[0].0 = MultiPolygon(Vec::new());
    *bytes = next_bytes;
    Ok(())
}

fn qualify_local_dose(
    paths: &[DepositionPath],
    nominal: &MultiPolygon<f64>,
    nominal_prepared: Option<&boolean::Prepared<'_>>,
    fresh: &MultiPolygon<f64>,
    local_budget: f64,
    request: &WallRequest<'_>,
    workspace: &mut boolean::Workspace,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let available = available_stage(request)?
        .checked_sub(workspace.retained_bytes())
        .ok_or_else(|| resource("wall dose workspace exceeds available metadata bytes"))?;
    let fresh_prepared = boolean::Prepared::new(fresh, available, work, cancel)?;
    let live = staged_request(
        request,
        fresh_prepared
            .as_ref()
            .map_or(0, boolean::Prepared::retained_bytes),
    )?;
    let local_strip = strip_area_query(
        paths,
        fresh,
        fresh_prepared.as_ref(),
        &live,
        workspace,
        work,
        cancel,
    )?;

    let local_union = match (nominal_prepared, fresh_prepared.as_ref()) {
        (Some(a), Some(b)) => prepared_pair_intersection(a, b, &live, workspace, work, cancel),
        _ => {
            let live = staged_request(&live, workspace.retained_bytes())?;
            overlay(nominal, fresh, Operation::Intersection, &live, work, cancel)
        }
    }?
    .unsigned_area();
    let excess = (local_strip - local_union).max(0.0);
    if excess > local_budget + AREA_ROUNDOFF {
        return Err(unsupported(&format!(
            "wall excess exceeds its source-local junction/count budget: excess={excess} mm², allowed={local_budget} mm², strip={local_strip} mm², union={local_union} mm², domain={:?}",
            fresh.bounding_rect(),
        )));
    }
    Ok(())
}

#[cfg(test)]
fn strip_area_in(
    paths: &[DepositionPath],
    domain: &MultiPolygon<f64>,
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<f64, ManufacturingError> {
    strip_area_query(
        paths,
        domain,
        None,
        request,
        &mut boolean::Workspace::new(),
        work,
        cancel,
    )
}
fn strip_area_query(
    paths: &[DepositionPath],
    domain: &MultiPolygon<f64>,
    prepared: Option<&boolean::Prepared<'_>>,
    request: &WallRequest<'_>,
    workspace: &mut boolean::Workspace,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<f64, ManufacturingError> {
    let Some(bounds) = domain.bounding_rect() else {
        return Ok(0.0);
    };
    let mut area = 0.0;
    for path in paths {
        for edge in path.checked_edges()? {
            check_cancel(cancel)?;
            boolean::charge(work, 1)?;
            let a = [edge.points_mm[0][0], edge.points_mm[0][1]];
            let b = [edge.points_mm[1][0], edge.points_mm[1][1]];
            let delta = sub(b, a);
            let length = norm(delta);
            let normal = mul([-delta[1], delta[0]], edge.width_mm * 0.5 / length);
            let vertices = [
                add(a, normal),
                add(b, normal),
                sub(b, normal),
                sub(a, normal),
                add(a, normal),
            ];
            if vertices.iter().all(|p| p[0] < bounds.min().x)
                || vertices.iter().all(|p| p[0] > bounds.max().x)
                || vertices.iter().all(|p| p[1] < bounds.min().y)
                || vertices.iter().all(|p| p[1] > bounds.max().y)
            {
                continue;
            }
            staging(
                request,
                workspace.retained_bytes()
                    + 5 * std::mem::size_of::<Coord<f64>>()
                    + std::mem::size_of::<Polygon<f64>>(),
            )?;
            let mut strip = MultiPolygon(vec![Polygon::new(
                LineString::from(vertices.to_vec()),
                Vec::new(),
            )]);
            let live = staged_request(request, super::super::native::material_bytes(&mut strip))?;
            area += match prepared {
                Some(b) => prepared_raw_intersection(&strip, b, &live, workspace, work, cancel)?,
                None => overlay(
                    &strip,
                    domain,
                    Operation::Intersection,
                    &staged_request(&live, workspace.retained_bytes())?,
                    work,
                    cancel,
                )?,
            }
            .unsigned_area();
        }
    }
    Ok(area)
}

#[derive(Clone, Copy)]
enum Operation {
    Union,
    Difference,
    Intersection,
}
fn overlay(
    a: &MultiPolygon<f64>,
    b: &MultiPolygon<f64>,
    op: Operation,
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    check_cancel(cancel)?;
    staging(
        request,
        (region::point_count(a) + region::point_count(b)) * 48,
    )?;
    // Exact set identities avoid both an unnecessary overlay and its sweep.
    // Every nonempty operand here is already an admitted original/candidate
    // region; this does not replace input, material or command qualification.
    if matches!(op, Operation::Intersection) && (a.0.is_empty() || b.0.is_empty())
        || matches!(op, Operation::Difference) && a.0.is_empty()
        || a.0.is_empty() && b.0.is_empty()
    {
        return Ok(MultiPolygon(Vec::new()));
    }
    if a.0.is_empty() || b.0.is_empty() {
        let source = if a.0.is_empty() { b } else { a };
        boolean::charge(work, region::point_count(source))?;
        return Ok(source.clone());
    }
    boolean::budget_accumulated(a, b, work, cancel)?;
    let result = match op {
        Operation::Union => a.union(b),
        Operation::Difference => a.difference(b),
        Operation::Intersection => a.intersection(b),
    };
    check_cancel(cancel)?;
    region::check_size(&result)?;
    staging(
        request,
        (region::point_count(a) + region::point_count(b) + region::point_count(&result)) * 48,
    )?;
    Ok(result)
}

fn available_stage(request: &WallRequest<'_>) -> Result<usize, ManufacturingError> {
    (MAX_MANUFACTURING_RETAINED_BYTES as usize)
        .checked_sub(request.owned_bytes)
        .ok_or_else(|| resource("wall prepared metadata byte budget exceeded"))
}

fn prepared_intersection_space(
    a: &MultiPolygon<f64>,
    b: &MultiPolygon<f64>,
    request: &WallRequest<'_>,
    workspace: &boolean::Workspace,
    cancel: &AtomicBool,
) -> Result<(usize, usize), ManufacturingError> {
    check_cancel(cancel)?;
    let scratch = (region::point_count(a) + region::point_count(b))
        .checked_mul(48)
        .ok_or_else(|| resource("wall prepared intersection scratch overflow"))?;
    staging(
        request,
        workspace
            .retained_bytes()
            .checked_add(scratch)
            .ok_or_else(|| resource("wall prepared intersection retained overflow"))?,
    )?;
    let available = available_stage(request)?
        .checked_sub(scratch)
        .ok_or_else(|| resource("wall prepared intersection byte budget exceeded"))?;
    Ok((available, scratch))
}

fn prepared_raw_intersection(
    a: &MultiPolygon<f64>,
    b: &boolean::Prepared<'_>,
    request: &WallRequest<'_>,
    workspace: &mut boolean::Workspace,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    let (available, scratch) =
        prepared_intersection_space(a, b.source(), request, workspace, cancel)?;
    if a.0.is_empty() || b.source().0.is_empty() {
        return Ok(MultiPolygon(Vec::new()));
    }
    workspace.budget_raw(a, b, available, work, cancel)?;
    finish_prepared_intersection(a, b.source(), request, workspace, scratch, cancel)
}

fn prepared_pair_intersection(
    a: &boolean::Prepared<'_>,
    b: &boolean::Prepared<'_>,
    request: &WallRequest<'_>,
    workspace: &mut boolean::Workspace,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    let (available, scratch) =
        prepared_intersection_space(a.source(), b.source(), request, workspace, cancel)?;
    if a.source().0.is_empty() || b.source().0.is_empty() {
        return Ok(MultiPolygon(Vec::new()));
    }
    workspace.budget_pair(a, b, available, work, cancel)?;
    finish_prepared_intersection(a.source(), b.source(), request, workspace, scratch, cancel)
}

fn finish_prepared_intersection(
    a: &MultiPolygon<f64>,
    b: &MultiPolygon<f64>,
    request: &WallRequest<'_>,
    workspace: &boolean::Workspace,
    scratch: usize,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    // IDENTICAL full operands/order as the old gate; no crop, local union,
    // surrogate area, coordinate normalization or sufficient certificate.
    staging(
        request,
        scratch
            .checked_add(workspace.retained_bytes())
            .ok_or_else(|| resource("wall prepared kernel staging overflow"))?,
    )?;

    let mut result = a.intersection(b);

    check_cancel(cancel)?;
    region::check_size(&result)?;
    let result_bytes = super::super::native::material_bytes(&mut result);
    staging(
        request,
        scratch
            .checked_add(workspace.retained_bytes())
            .and_then(|n| n.checked_add(result_bytes))
            .ok_or_else(|| resource("wall prepared intersection result overflow"))?,
    )?;
    Ok(result)
}
fn staging(request: &WallRequest<'_>, bytes: usize) -> Result<(), ManufacturingError> {
    admit_stage(request, bytes)
}
fn retain_region(
    request: &mut WallRequest<'_>,
    region: &mut MultiPolygon<f64>,
) -> Result<(), ManufacturingError> {
    request.owned_bytes = request
        .owned_bytes
        .checked_add(super::super::native::material_bytes(region))
        .ok_or_else(|| resource("wall qualification ownership overflow"))?;
    admit_stage(request, 0)
}

fn capsule(points: [[f64; 2]; 2], radius: f64, outer: bool) -> Polygon<f64> {
    let (mut vertices, _) = capsule_vertices(points, radius, outer, 0);
    Polygon::new(quick_hull(&mut vertices), Vec::new())
}

// Command occupancy must retain the straight tangent rails at every angle.
// Source permission polygons deliberately keep their original fixed vertices.
fn command_capsule(points: [[f64; 2]; 2], radius: f64, outer: bool) -> Polygon<f64> {
    let (mut vertices, radius) = capsule_vertices(points, radius, outer, if outer { 0 } else { 4 });
    if !outer {
        let delta = [points[1][0] - points[0][0], points[1][1] - points[0][1]];
        let length = delta[0].hypot(delta[1]);
        if length > 0.0 {
            let normal = [-delta[1] / length, delta[0] / length];
            for p in points {
                for sign in [-1.0, 1.0] {
                    vertices.push(Coord {
                        x: p[0] + sign * radius * normal[0],
                        y: p[1] + sign * radius * normal[1],
                    });
                }
            }
        }
    }
    Polygon::new(quick_hull(&mut vertices), Vec::new())
}

fn capsule_vertices(
    points: [[f64; 2]; 2],
    radius: f64,
    outer: bool,
    extra_vertices: usize,
) -> (Vec<Coord<f64>>, f64) {
    let magnitude = points
        .into_iter()
        .flatten()
        .fold(radius.max(1.0), |a, b| a.max(b.abs()));
    let arithmetic = (128.0 * f64::EPSILON * magnitude).next_up();
    let radius = if outer {
        (radius + arithmetic) / (std::f64::consts::PI / DISK_SIDES as f64).cos()
    } else {
        radius - arithmetic
    };
    let mut vertices = Vec::with_capacity(DISK_SIDES * 2 + extra_vertices);
    for p in points {
        for direction in DISK_DIRECTIONS.iter() {
            vertices.push(Coord {
                x: p[0] + radius * direction[0],
                y: p[1] + radius * direction[1],
            });
        }
    }
    (vertices, radius)
}
fn occupancy(
    paths: &[DepositionPath],
    uncertainty: f64,
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    let mut primitives = Vec::new();
    let mut batch_points = 0usize;
    let mut primitive_bytes = 0usize;
    let mut commands = 0usize;
    let mut batches = Vec::<UnionRegion>::new();
    let mut batch_bytes = 0usize;
    let capsule_scratch = COMMAND_CAPSULE_VERTICES * 3 * std::mem::size_of::<Coord<f64>>();
    for path in paths {
        for edge in path.checked_edges()? {
            check_cancel(cancel)?;
            commands = commands
                .checked_add(1)
                .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
                .ok_or_else(|| resource("wall occupancy command limit exceeded"))?;
            // Bound all command hull input vertices plus closure.
            // Flush BEFORE construction, so the next primitive and its scratch
            // never coexist with an over-limit Boolean input batch.
            if batch_points
                .checked_add(COMMAND_CAPSULE_VERTICES + 1)
                .is_none_or(|n| n > MAX_DEPOSITION_SEGMENTS as usize)
            {
                flush_occupancy_batch(
                    &mut primitives,
                    batch_points,
                    &mut batches,
                    &mut batch_bytes,
                    request,
                    work,
                    cancel,
                )?;
                batch_points = 0;
                primitive_bytes = 0;
            }
            let saved = batch_bytes
                .checked_add(batches.capacity() * std::mem::size_of::<UnionRegion>())
                .ok_or_else(|| resource("wall occupancy saved-batch overflow"))?;
            let live = staged_request(request, saved)?;
            boolean::charge(work, COMMAND_CAPSULE_VERTICES)?;
            let radius = edge.width_mm * 0.5 + uncertainty;
            if radius <= 0.0 {
                return Err(unsupported("wall command has no positive inner occupancy"));
            }
            let scratch = primitive_bytes
                .checked_add(capsule_scratch)
                .ok_or_else(|| resource("wall occupancy construction-byte overflow"))?;
            reserve_stage(&mut primitives, 1, &live, scratch)?;
            staging(
                &live,
                primitives
                    .capacity()
                    .checked_mul(std::mem::size_of::<Polygon<f64>>())
                    .and_then(|n| n.checked_add(scratch))
                    .ok_or_else(|| resource("wall occupancy staging-byte overflow"))?,
            )?;

            let mut primitive = command_capsule(
                std::array::from_fn(|i| [edge.points_mm[i][0], edge.points_mm[i][1]]),
                radius,
                uncertainty > 0.0,
            );

            // Semantic commands, Boolean boundary entries and allocation bytes
            // are three distinct inventories. Capacity is NEVER command count.
            batch_points = batch_points
                .checked_add(primitive.exterior().0.len())
                .ok_or_else(|| resource("wall occupancy batch-point overflow"))?;
            primitive_bytes = primitive_bytes
                .checked_add(super::super::native::polygon_bytes(&mut primitive))
                .ok_or_else(|| resource("wall occupancy primitive-byte overflow"))?;
            primitives.push(primitive);
        }
    }
    flush_occupancy_batch(
        &mut primitives,
        batch_points,
        &mut batches,
        &mut batch_bytes,
        request,
        work,
        cancel,
    )?;
    drop(primitives);
    union_regions(batches, batch_bytes, request, work, cancel)
}

fn flush_occupancy_batch(
    primitives: &mut Vec<Polygon<f64>>,
    points: usize,
    batches: &mut Vec<UnionRegion>,
    batch_bytes: &mut usize,
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    if primitives.is_empty() {
        return Ok(());
    }
    let mut all = MultiPolygon(std::mem::take(primitives));
    let input_bytes = super::super::native::material_bytes(&mut all);
    let saved = batch_bytes
        .checked_add(batches.capacity() * std::mem::size_of::<UnionRegion>())
        .and_then(|n| n.checked_add(input_bytes))
        .ok_or_else(|| resource("wall occupancy batch ownership overflow"))?;
    let live = staged_request(request, saved)?;
    staging(&live, points * 48)?;
    boolean::budget_accumulated(&all, &all, work, cancel)?;

    let mut normalized = MultiPolygon(unary_union(&all.0).0);

    check_cancel(cancel)?;
    region::check_size(&normalized)?;
    let normalized_bytes = super::super::native::material_bytes(&mut normalized);
    staging(
        &live,
        normalized_bytes
            .checked_add(points * 32)
            .ok_or_else(|| resource("wall occupancy batch result-byte overflow"))?,
    )?;
    // Release input coordinate buffers but retain the geometrically-grown
    // primitive-record vector for the next batch. No primitive is rebuilt.
    boolean::charge(work, all.0.len())?;
    all.0.clear();
    check_cancel(cancel)?;
    let retained_metadata = all.0.capacity() * std::mem::size_of::<Polygon<f64>>();
    let live = staged_request(request, retained_metadata)?;
    boolean::charge(work, region::point_count(&normalized))?;
    if let Some(bounds) = normalized.bounding_rect() {
        let other = batch_bytes
            .checked_add(normalized_bytes)
            .ok_or_else(|| resource("wall occupancy batch staging overflow"))?;
        // reserve_stage includes old AND replacement saved-batch metadata,
        // all saved geometry, the new normalized result and reusable metadata.
        reserve_stage(batches, 1, &live, other)?;
        *batch_bytes = other;
        let ordinal = batches.len();
        batches.push((normalized, bounds, ordinal));
    }
    *primitives = all.0;
    Ok(())
}

fn exclude_source(
    paths: &[DepositionPath],
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    for path in paths {
        for edge in path.checked_edges()? {
            if edge.width_mm < request.limits.min_bead_width_mm
                || edge.width_mm > request.limits.max_bead_width_mm
                || edge.width_mm < path.height_mm()
            {
                return Err(unsupported(
                    "actual wall packing has no admitted process-width/height envelope",
                ));
            }
            let ends: [[f64; 2]; 2] =
                std::array::from_fn(|i| [edge.points_mm[i][0], edge.points_mm[i][1]]);
            let segment = Line::new(Coord::from(ends[0]), Coord::from(ends[1]));
            let middle = mul(add(ends[0], ends[1]), 0.5);
            if !request.material.contains(&Point::from(middle)) {
                return Err(unsupported(
                    "actual wall command center leaves original material or enters a hole",
                ));
            }
            for polygon in &request.material.0 {
                for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
                    for edge_boundary in ring.lines() {
                        check_cancel(cancel)?;
                        boolean::charge(work, 1)?;
                        let clearance = Euclidean.distance(&segment, &edge_boundary);
                        let needed = edge.width_mm * 0.5
                            + program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM;
                        // Preserve the existing section/offset approximation;
                        // this adds no wall-specific outward or hole allowance.
                        if !clearance.is_finite()
                            || clearance + super::super::OFFSET_TOLERANCE_MM < needed
                        {
                            return Err(unsupported(&format!(
                                "actual outer wall capsule violates original exterior/hole clearance: {clearance} < {needed}"
                            )));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn permission_region(
    permission: &Junction,
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    if !permission.radius_mm.is_finite()
        || permission.radius_mm > request.limits.max_bead_width_mm
        || permission.radius_mm <= 0.0
    {
        return Err(unsupported(
            "wall permission exceeds its predeclared source-derived radius",
        ));
    }
    let mut result = MultiPolygon(Vec::new());
    if let Some(corridor) = &permission.transition {
        if corridor.incoming_count.abs_diff(corridor.outgoing_count) != 1
            || corridor.points.len() < 2
        {
            return Err(unsupported(
                "wall transition permission has no finite single-bead count event",
            ));
        }
        for pair in corridor.points.windows(2) {
            let piece = MultiPolygon(vec![capsule(
                [pair[0], pair[1]],
                permission.radius_mm,
                false,
            )]);
            result = overlay(&result, &piece, Operation::Union, request, work, cancel)?;
        }
    } else {
        result
            .0
            .push(capsule([permission.center; 2], permission.radius_mm, false));
    }
    Ok(result)
}

fn local_connections(
    inner: &MultiPolygon<f64>,
    region: &MultiPolygon<f64>,
    permission: &Junction,
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    // Count corridors and permissions without required source-arm ports have
    // no local-connectivity predicate. Do not intersect the whole wall union
    // merely to discover that the predicate is inapplicable.
    if permission.transition.is_some()
        || permission.valence < 3
        || permission.ports.is_empty()
        || !region.contains(&Point::from(permission.center))
    {
        return Ok(());
    }
    let local = overlay(
        inner,
        region,
        Operation::Intersection,
        request,
        work,
        cancel,
    )?;
    if !local.0.iter().any(|component| {
        permission.ports.iter().all(|port| {
            component.intersects(&Line::new(Coord::from(port[0]), Coord::from(port[1])))
        })
    }) {
        return Err(unsupported(&format!(
            "guaranteed-inner arm ports do not connect through their local junction patch: center={:?}, radius={}, valence={}, components={}, ports={}, first_ports={:?}",
            permission.center,
            permission.radius_mm,
            permission.valence,
            local.0.len(),
            permission.ports.len(),
            &permission.ports[..permission.ports.len().min(4)]
        )));
    }
    Ok(())
}

fn qualify_overlap_locations(
    paths: &[DepositionPath],
    permissions: &[Junction],
    authorized: &MultiPolygon<f64>,
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    unique(paths, authorized, request, work, cancel)?;
    // Compare complete continuing paths, never segment 1 with segment 3 of the
    // same harmless subdivision. The exact command scan independently retains
    // nonlocal proper-crossing and retrace obligations under path stitching.
    let mut prior = MultiPolygon(Vec::new());
    let mut prior_bytes = 0usize;
    for (path_index, path) in paths.iter().enumerate() {
        let mut live = *request;
        live.owned_bytes = live
            .owned_bytes
            .checked_add(prior_bytes)
            .ok_or_else(|| resource("wall prior-occupancy ownership overflow"))?;
        let mut tube = occupancy(std::slice::from_ref(path), 0.0, &live, work, cancel)?;
        retain_region(&mut live, &mut tube)?;
        {
            let mut raw_overlap =
                overlay(&prior, &tube, Operation::Intersection, &live, work, cancel)?;
            let mut overlap_request = live;
            retain_region(&mut overlap_request, &mut raw_overlap)?;
            // Permissions allocate overlap in modeled material. The complete outer
            // capsules already pass the independent original-D/exterior/hole gate.
            let mut overlap = overlay(
                &raw_overlap,
                request.material,
                Operation::Intersection,
                &overlap_request,
                work,
                cancel,
            )?;
            drop(raw_overlap);
            let mut overlap_request = live;
            retain_region(&mut overlap_request, &mut overlap)?;
            let mut outside = overlay(
                &overlap,
                authorized,
                Operation::Difference,
                &overlap_request,
                work,
                cancel,
            )?;
            // Nominal adjacent lane boundaries have zero-area contact. Reserve the
            // fixed disk-polygon enclosure only, not a bead-scale overlap waiver.
            if outside.unsigned_area() > AREA_ROUNDOFF {
                let mut failure_request = overlap_request;
                retain_region(&mut failure_request, &mut outside)?;
                // A positive ALL-pairs actual-command certificate can refute a
                // polygon-conversion sliver. An inconclusive certificate leaves
                // this exact existing location refusal and its area policy intact.
                if !separation::certificate(
                    &paths[..path_index],
                    path,
                    permissions,
                    &failure_request,
                    work,
                    cancel,
                )? {
                    let witness = overlap_failure_witness(
                        paths,
                        path_index,
                        &outside,
                        &failure_request,
                        work,
                        cancel,
                    )?;
                    let witness = witness.map_or_else(
                        || "no positive earlier-path pair reconstructed".to_owned(),
                        |witness| witness.to_string(),
                    );
                    return Err(unsupported(&format!(
                        "wall path {path_index} overlaps outside predeclared junction/count permissions: {} mm2, bounds {:?}; {witness}",
                        outside.unsigned_area(),
                        outside.bounding_rect()
                    )));
                }
            }
        }
        let mut next_prior = overlay(&prior, &tube, Operation::Union, &live, work, cancel)?;
        prior_bytes = super::super::native::material_bytes(&mut next_prior);
        staging(&live, prior_bytes)?;
        prior = next_prior;
    }
    Ok(())
}

// Failure-only evidence: a complete earlier/current path pair first, then ONE
// actual cross-path command pair inside that already-proved unauthorized set.
// This never substitutes segment-cap overlap for the whole-path location gate.
struct OverlapFailureWitness {
    paths: [usize; 2],
    physical_z_mm: [[f64; 2]; 2],
    area_mm2: f64,
    bounds: Option<geo::Rect<f64>>,
    commands: Option<CommandPairWitness>,
    boundary_probe: Option<BoundaryProbe>,
}
struct CommandPairWitness {
    indices: [usize; 2],
    points_mm: [[[f64; 3]; 2]; 2],
    widths_mm: [f64; 2],
    area_mm2: f64,
    bounds: Option<geo::Rect<f64>>,
}
impl std::fmt::Debug for BoundaryProbe {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BoundaryProbe")
            .field("point", &self.point)
            .field("nearest", &self.nearest)
            .finish()
    }
}
struct BoundaryProbe {
    point: [f64; 2],
    nearest: [Option<NearestCommand>; 2],
}
impl std::fmt::Debug for NearestCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NearestCommand")
            .field("index", &self.index)
            .field("points_mm", &self.points_mm)
            .field("width_mm", &self.width_mm)
            .field(
                "nominal_clearance_estimate_mm",
                &self.nominal_clearance_estimate_mm,
            )
            .finish()
    }
}
struct NearestCommand {
    index: usize,
    points_mm: [[f64; 3]; 2],
    width_mm: f64,
    nominal_clearance_estimate_mm: f64,
}
impl std::fmt::Display for OverlapFailureWitness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "earlier/current paths={:?} physical_z_mm={:?} unauthorized_pair_area_mm2={} pair_bounds={:?}",
            self.paths, self.physical_z_mm, self.area_mm2, self.bounds
        )?;
        if let Some(commands) = &self.commands {
            write!(
                f,
                " command_indices={:?} xyz_mm={:?} widths_mm={:?} unauthorized_command_pair_area_mm2={} command_pair_bounds={:?}",
                commands.indices,
                commands.points_mm,
                commands.widths_mm,
                commands.area_mm2,
                commands.bounds
            )
        } else {
            write!(
                f,
                " no positive command pair reconstructed; boundary-only diagnostic={:?}",
                self.boundary_probe
            )
        }
    }
}

fn overlap_failure_witness(
    paths: &[DepositionPath],
    current: usize,
    outside: &MultiPolygon<f64>,
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<OverlapFailureWitness>, ManufacturingError> {
    for (earlier, previous) in paths[..current].iter().enumerate() {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        let mut live = *request;
        let mut tube = occupancy(std::slice::from_ref(previous), 0.0, &live, work, cancel)?;
        retain_region(&mut live, &mut tube)?;
        let mut pair = overlay(&tube, outside, Operation::Intersection, &live, work, cancel)?;
        let area_mm2 = pair.unsigned_area();
        if area_mm2 <= 0.0 {
            continue;
        }
        let bounds = pair.bounding_rect();
        retain_region(&mut live, &mut pair)?;
        let commands =
            overlap_command_witness(previous, &paths[current], &pair, &live, work, cancel)?;
        let boundary_probe = if commands.is_none() {
            if let Some(coord) = pair.0.iter().find_map(|p| p.exterior().0.first()) {
                let point = [coord.x, coord.y];
                Some(BoundaryProbe {
                    point,
                    nearest: [
                        nearest_command_at_boundary(previous, point, work, cancel)?,
                        nearest_command_at_boundary(&paths[current], point, work, cancel)?,
                    ],
                })
            } else {
                None
            }
        } else {
            None
        };
        return Ok(Some(OverlapFailureWitness {
            paths: [earlier, current],
            physical_z_mm: [previous.physical_z_mm, paths[current].physical_z_mm],
            area_mm2,
            bounds,
            commands,
            boundary_probe,
        }));
    }
    Ok(None)
}

// Failure-only scalar estimates: these points/nearest commands authorize
// nothing. They distinguish a real analytic capsule contact from polygon-union
// representation effects when no individual nominal polygon pair reproduces it.
fn nearest_command_at_boundary(
    path: &DepositionPath,
    p: [f64; 2],
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<NearestCommand>, ManufacturingError> {
    let mut nearest: Option<NearestCommand> = None;
    for (index, edge) in path.checked_edges()?.enumerate() {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        let line = Line::new(
            Coord::from([edge.points_mm[0][0], edge.points_mm[0][1]]),
            Coord::from([edge.points_mm[1][0], edge.points_mm[1][1]]),
        );
        let clearance = Euclidean.distance(&Point::from(p), &line) - edge.width_mm * 0.5;
        if nearest
            .as_ref()
            .is_none_or(|old| clearance < old.nominal_clearance_estimate_mm)
        {
            nearest = Some(NearestCommand {
                index,
                points_mm: [edge.points_mm[0], edge.points_mm[1]],
                width_mm: edge.width_mm,
                nominal_clearance_estimate_mm: clearance,
            });
        }
    }
    Ok(nearest)
}

fn overlap_command_witness(
    previous: &DepositionPath,
    current: &DepositionPath,
    domain: &MultiPolygon<f64>,
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<CommandPairWitness>, ManufacturingError> {
    let Some(bounds) = domain.bounding_rect() else {
        return Ok(None);
    };
    for (a_index, a) in previous.checked_edges()?.enumerate() {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        let a_points = [a.points_mm[0], a.points_mm[1]];
        if !diagnostic_command_bounds(a_points, a.width_mm).intersects(&bounds) {
            continue;
        }
        let mut primitive = diagnostic_capsule(a_points, a.width_mm, request, work, cancel)?;
        let primitive_request = staged_request(
            request,
            super::super::native::material_bytes(&mut primitive),
        )?;
        let mut clipped = overlay(
            &primitive,
            domain,
            Operation::Intersection,
            &primitive_request,
            work,
            cancel,
        )?;
        drop(primitive);
        if clipped.unsigned_area() <= 0.0 {
            continue;
        }
        let Some(clip_bounds) = clipped.bounding_rect() else {
            continue;
        };
        let clipped_request =
            staged_request(request, super::super::native::material_bytes(&mut clipped))?;
        for (b_index, b) in current.checked_edges()?.enumerate() {
            check_cancel(cancel)?;
            boolean::charge(work, 1)?;
            let b_points = [b.points_mm[0], b.points_mm[1]];
            if !diagnostic_command_bounds(b_points, b.width_mm).intersects(&clip_bounds) {
                continue;
            }
            let mut primitive =
                diagnostic_capsule(b_points, b.width_mm, &clipped_request, work, cancel)?;
            let primitive_request = staged_request(
                &clipped_request,
                super::super::native::material_bytes(&mut primitive),
            )?;
            let mut overlap = overlay(
                &clipped,
                &primitive,
                Operation::Intersection,
                &primitive_request,
                work,
                cancel,
            )?;
            staging(
                &primitive_request,
                super::super::native::material_bytes(&mut overlap),
            )?;
            let area_mm2 = overlap.unsigned_area();
            if area_mm2 > 0.0 {
                return Ok(Some(CommandPairWitness {
                    indices: [a_index, b_index],
                    points_mm: [a_points, b_points],
                    widths_mm: [a.width_mm, b.width_mm],
                    area_mm2,
                    bounds: overlap.bounding_rect(),
                }));
            }
        }
    }
    Ok(None)
}

fn diagnostic_command_bounds(points: [[f64; 3]; 2], width: f64) -> geo::Rect<f64> {
    let radius = width * 0.5;
    geo::Rect::new(
        Coord {
            x: (points[0][0].min(points[1][0]) - radius).next_down(),
            y: (points[0][1].min(points[1][1]) - radius).next_down(),
        },
        Coord {
            x: (points[0][0].max(points[1][0]) + radius).next_up(),
            y: (points[0][1].max(points[1][1]) + radius).next_up(),
        },
    )
}
fn diagnostic_capsule(
    points: [[f64; 3]; 2],
    width: f64,
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    check_cancel(cancel)?;
    boolean::charge(work, COMMAND_CAPSULE_VERTICES)?;
    staging(
        request,
        COMMAND_CAPSULE_VERTICES * 3 * std::mem::size_of::<Coord<f64>>()
            + std::mem::size_of::<Polygon<f64>>(),
    )?;
    Ok(MultiPolygon(vec![command_capsule(
        std::array::from_fn(|i| [points[i][0], points[i][1]]),
        width * 0.5,
        false,
    )]))
}

struct Command {
    ends: [Key; 2],
    min: [i64; 2],
    max: [i64; 2],
}
fn unique(
    paths: &[DepositionPath],
    authorized: &MultiPolygon<f64>,
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let count = paths.iter().try_fold(0usize, |count, path| {
        count
            .checked_add(path.checked_edges()?.len())
            .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
            .ok_or_else(|| resource("wall uniqueness command limit exceeded"))
    })?;
    staging(request, count * std::mem::size_of::<Command>())?;
    let mut commands = Vec::with_capacity(count);
    for path in paths {
        for edge in path.checked_edges()? {
            let ends = [
                key([edge.points_mm[0][0], edge.points_mm[0][1]])?,
                key([edge.points_mm[1][0], edge.points_mm[1][1]])?,
            ];
            if ends[0] == ends[1] {
                return Err(unsupported("wall command collapses at XYZ6"));
            }
            commands.push(Command {
                ends,
                min: [ends[0][0].min(ends[1][0]), ends[0][1].min(ends[1][1])],
                max: [ends[0][0].max(ends[1][0]), ends[0][1].max(ends[1][1])],
            });
            if commands.len() > MAX_DEPOSITION_SEGMENTS as usize {
                return Err(resource("wall uniqueness command limit exceeded"));
            }
        }
    }
    boolean::sort_by(&mut commands, |c| c.min[0] as f64, work, cancel)?;
    for (i, a) in commands.iter().enumerate() {
        check_cancel(cancel)?;
        for b in &commands[i + 1..] {
            boolean::charge(work, 1)?;
            if b.min[0] > a.max[0] {
                break;
            }
            if b.min[1] > a.max[1] || a.min[1] > b.max[1] {
                continue;
            }
            let side_a = b.ends.map(|p| exact_orientation(a.ends[0], a.ends[1], p));
            let side_b = a.ends.map(|p| exact_orientation(b.ends[0], b.ends[1], p));
            if side_a == [0, 0] {
                let axis = usize::from(a.ends[0][0] == a.ends[1][0]);
                if a.min[axis].max(b.min[axis]) < a.max[axis].min(b.max[axis]) {
                    return Err(unsupported(
                        "duplicate, reversed, cyclic or partially retraced XYZ6 wall command",
                    ));
                }
            }
            if side_a[0].signum() * side_a[1].signum() == -1
                && side_b[0].signum() * side_b[1].signum() == -1
            {
                // Strict opposite orientations exclude consecutive endpoints
                // and cyclic seams without an adjacency/subdivision heuristic.
                // Integer determinants are exact for admitted XYZ6 coordinates.
                let da = [a.ends[1][0] - a.ends[0][0], a.ends[1][1] - a.ends[0][1]];
                let db = [b.ends[1][0] - b.ends[0][0], b.ends[1][1] - b.ends[0][1]];
                let determinant = exact_orientation([0, 0], da, db);
                if determinant == 0 {
                    return Err(unsupported(
                        "proper wall crossing has indeterminate intersection",
                    ));
                }
                let delta = [b.ends[0][0] - a.ends[0][0], b.ends[0][1] - a.ends[0][1]];
                let t = exact_orientation([0, 0], delta, db) as f64 / determinant as f64;
                let p = Point::new(
                    (a.ends[0][0] as f64 + da[0] as f64 * t) / SITE_SCALE,
                    (a.ends[0][1] as f64 + da[1] as f64 * t) / SITE_SCALE,
                );
                let magnitude = a
                    .ends
                    .into_iter()
                    .chain(b.ends)
                    .flatten()
                    .fold(1.0f64, |m, value| m.max((value as f64).abs() / SITE_SCALE));
                let arithmetic = 1024.0 * f64::EPSILON * magnitude;
                boolean::charge(work, region::point_count(authorized).saturating_mul(2))?;
                let permitted = authorized.0.iter().any(|polygon| {
                    polygon.contains(&p)
                        && std::iter::once(polygon.exterior())
                            .chain(polygon.interiors())
                            .all(|ring| {
                                ring.lines()
                                    .all(|line| Euclidean.distance(&p, &line) > arithmetic)
                            })
                });
                if !permitted {
                    return Err(unsupported(
                        "proper nonlocal XYZ6 wall crossing lies outside predeclared permissions",
                    ));
                }
            }
        }
    }
    Ok(())
}

fn exact_orientation(a: Key, b: Key, p: Key) -> i128 {
    (i128::from(b[0]) - i128::from(a[0])) * (i128::from(p[1]) - i128::from(a[1]))
        - (i128::from(b[1]) - i128::from(a[1])) * (i128::from(p[0]) - i128::from(a[0]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::polygon;

    fn path(points: &[[f64; 2]]) -> DepositionPath {
        DepositionPath {
            kind: DepositionKind::Perimeter,
            owner: Some(DepositionOwner::Wall),
            width: WidthProfile::Constant { width_mm: 0.6 },
            physical_z_mm: [0.0, 0.2],
            points_mm: points.iter().map(|p| [p[0], p[1], 0.2]).collect(),
        }
    }

    fn request<'a>(
        material: &'a MultiPolygon<f64>,
        required: &'a region::RequiredMaterial<'a>,
        recipe: &'a PlanarPrintRecipe,
        limits: &'a DepositionLimits,
    ) -> WallRequest<'a> {
        WallRequest {
            material,
            required,
            recipe,
            limits,
            nozzle_diameter_mm: 0.4,
            lower_z: 0.0,
            upper_z: 0.2,
            retained_points: region::point_count(material),
            owned_bytes: 0,
        }
    }

    #[test]
    fn occupancy_batches_preserve_long_inner_stroke_and_distinct_last_batch() {
        let material = MultiPolygon(vec![polygon![
            (x: -1., y: -1.), (x: 47., y: -1.), (x: 47., y: 3.), (x: -1., y: 3.),
        ]]);
        let required = region::RequiredMaterial::from_section(&material);
        let recipe = super::super::super::tests::recipe(0.25);
        let limits = DepositionLimits {
            min_bead_width_mm: 0.3,
            max_bead_width_mm: 0.75,
            min_bead_height_mm: 0.000001,
            max_bead_height_mm: 0.3,
        };
        let request = request(&material, &required, &recipe, &limits);
        let uncertainty = -program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM;
        let radius = 0.3 + uncertainty;
        let primitive_points = capsule([[0., 0.], [0.025, 0.]], radius, false)
            .exterior()
            .0
            .len();
        assert!(
            primitive_points * 1600 > MAX_DEPOSITION_SEGMENTS as usize,
            "legal command subdivision exceeds the Boolean primitive boundary staging cap"
        );
        let subdivided = DepositionPath {
            points_mm: (0..=1600).map(|i| [i as f64 / 40.0, 0., 0.2]).collect(),
            ..path(&[])
        };
        let paths = [subdivided, path(&[[45., 2.], [46., 2.]])];
        let cancel = AtomicBool::new(false);
        let mut work = 0;
        let mut actual = occupancy(&paths, uncertainty, &request, &mut work, &cancel)
            .expect("1601 legal commands do not become 100k semantic commands via disk vertices");
        let live = staged_request(
            &request,
            super::super::super::native::material_bytes(&mut actual),
        )
        .unwrap();
        // Inscribed-disk radius under the SAME arithmetic inset bounds full
        // guaranteed-inner rectangles, not just nonempty/count observations.
        let arithmetic = (128.0 * f64::EPSILON * 46.0).next_up();
        let half_width = (radius - arithmetic) * (std::f64::consts::PI / DISK_SIDES as f64).cos();
        let mut guaranteed = MultiPolygon(vec![
            polygon![(x: 0., y: -half_width), (x: 40., y: -half_width),
                (x: 40., y: half_width), (x: 0., y: half_width)],
            polygon![(x: 45., y: 2. - half_width), (x: 46., y: 2. - half_width),
                (x: 46., y: 2. + half_width), (x: 45., y: 2. + half_width)],
        ]);
        let live = staged_request(
            &live,
            super::super::super::native::material_bytes(&mut guaranteed),
        )
        .unwrap();
        assert!(
            overlay(
                &guaranteed,
                &actual,
                Operation::Difference,
                &live,
                &mut work,
                &cancel
            )
            .unwrap()
            .unsigned_area()
                <= AREA_ROUNDOFF,
            "every part of both guaranteed-inner strokes survives, including the distinct last batch"
        );
    }

    #[test]
    fn permission_prefix_keeps_ordered_fresh_budget_and_unaffected_buffers() {
        let material = MultiPolygon(vec![polygon![
            (x: 0., y: 0.), (x: 12., y: 0.), (x: 12., y: 4.), (x: 0., y: 4.),
        ]]);
        let required = region::RequiredMaterial::from_section(&material);
        let recipe = super::super::super::tests::recipe(0.25);
        let limits = DepositionLimits {
            min_bead_width_mm: 0.3,
            max_bead_width_mm: 0.75,
            min_bead_height_mm: 0.000001,
            max_bead_height_mm: 0.3,
        };
        let request = request(&material, &required, &recipe, &limits);
        let mut original = MultiPolygon(vec![
            Polygon::new(
                LineString::from(vec![[0., 0.], [4., 0.], [4., 4.], [0., 4.], [0., 0.]]),
                vec![LineString::from(vec![
                    [1., 1.],
                    [1., 2.],
                    [2., 2.],
                    [2., 1.],
                    [1., 1.],
                ])],
            ),
            polygon![(x: 10., y: 0.), (x: 11., y: 0.), (x: 11., y: 1.), (x: 10., y: 1.)],
        ]);
        let mut next = MultiPolygon(vec![polygon![
            (x: 3., y: 1.), (x: 5., y: 1.), (x: 5., y: 3.), (x: 3., y: 3.),
        ]]);
        let base = staged_request(
            &request,
            super::super::super::native::material_bytes(&mut original)
                + super::super::super::native::material_bytes(&mut next),
        )
        .unwrap();
        let cancel = AtomicBool::new(false);
        let mut work = 0;
        let mut seen = Vec::new();
        let mut payload = 0;
        extend_permission_prefix(
            &mut seen,
            &mut payload,
            &mut MultiPolygon(Vec::new()),
            &original,
            &base,
            &mut work,
            &cancel,
        )
        .unwrap();
        let far_buffer = seen
            .iter()
            .find(|(_, bounds)| bounds.min().x == 10.)
            .unwrap()
            .0
            .exterior()
            .0
            .as_ptr();
        let live = staged_request(&base, permission_prefix_bytes(&seen, payload).unwrap()).unwrap();
        let mut hit = take_permission_prefix_hit(
            &mut seen,
            &mut payload,
            next.bounding_rect().unwrap(),
            &live,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert_eq!(
            hit.0.len(),
            1,
            "only the near connected component enters the overlay"
        );
        assert_eq!(
            seen.len(),
            1,
            "the distant component stays owned and untouched"
        );
        let live = staged_request(
            &live,
            hit.0.capacity() * std::mem::size_of::<Polygon<f64>>(),
        )
        .unwrap();
        let mut fresh = overlay(
            &next,
            &hit,
            Operation::Difference,
            &live,
            &mut work,
            &cancel,
        )
        .unwrap();
        let query = staged_request(
            &live,
            super::super::super::native::material_bytes(&mut fresh),
        )
        .unwrap();
        let mut reference = overlay(
            &next,
            &original,
            Operation::Difference,
            &query,
            &mut work,
            &cancel,
        )
        .unwrap();
        let query = staged_request(
            &query,
            super::super::super::native::material_bytes(&mut reference),
        )
        .unwrap();
        assert!(
            overlay(
                &fresh,
                &reference,
                Operation::Difference,
                &query,
                &mut work,
                &cancel
            )
            .unwrap()
            .unsigned_area()
                <= AREA_ROUNDOFF
        );
        assert!(
            overlay(
                &reference,
                &fresh,
                Operation::Difference,
                &query,
                &mut work,
                &cancel
            )
            .unwrap()
            .unsigned_area()
                <= AREA_ROUNDOFF
        );
        assert_eq!(
            fresh.unsigned_area() / next.unsigned_area(),
            0.5,
            "source order keeps exactly half the local budget"
        );
        drop(fresh);
        drop(reference);
        extend_permission_prefix(
            &mut seen,
            &mut payload,
            &mut hit,
            &next,
            &live,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert_eq!(
            far_buffer,
            seen.iter()
                .find(|(_, bounds)| bounds.min().x == 10.)
                .unwrap()
                .0
                .exterior()
                .0
                .as_ptr()
        );
        assert!(
            !seen
                .iter()
                .any(|(polygon, _)| polygon.contains(&Point::new(1.5, 1.5))),
            "prefix union preserves the source hole"
        );
        let live = staged_request(&base, permission_prefix_bytes(&seen, payload).unwrap()).unwrap();
        let hit = take_permission_prefix_hit(
            &mut seen,
            &mut payload,
            next.bounding_rect().unwrap(),
            &live,
            &mut work,
            &cancel,
        )
        .unwrap();
        let live = staged_request(
            &live,
            hit.0.capacity() * std::mem::size_of::<Polygon<f64>>(),
        )
        .unwrap();
        assert!(
            overlay(
                &next,
                &hit,
                Operation::Difference,
                &live,
                &mut work,
                &cancel
            )
            .unwrap()
            .0
            .is_empty(),
            "a later repeated permission receives no second freshness/budget share"
        );
    }

    #[test]
    fn isolated_diagonal_stroke_does_not_spend_a_junction_excess_budget() {
        let material = MultiPolygon(vec![polygon![
            (x: -1., y: -1.), (x: 2., y: -1.), (x: 2., y: 2.), (x: -1., y: 2.),
        ]]);
        let required = region::RequiredMaterial::from_section(&material);
        let recipe = super::super::super::tests::recipe(0.25);
        let limits = DepositionLimits {
            min_bead_width_mm: 0.3,
            max_bead_width_mm: 0.70,
            min_bead_height_mm: 0.000001,
            max_bead_height_mm: 0.3,
        };
        let request = request(&material, &required, &recipe, &limits);
        let paths = [path(&[[0., 0.], [0.7, 0.3]])];
        // Cross the straight side well away from both end caps. An inscribed
        // disk with fixed world-axis vertices must not erase this actual rail.
        let fresh = MultiPolygon(vec![polygon![
            (x: 0.20, y: 0.39), (x: 0.27, y: 0.39),
            (x: 0.27, y: 0.46), (x: 0.20, y: 0.46),
        ]]);
        let cancel = AtomicBool::new(false);
        let mut work = 0;
        let nominal = occupancy(&paths, 0.0, &request, &mut work, &cancel).unwrap();
        let strip = strip_area_in(&paths, &fresh, &request, &mut work, &cancel).unwrap();
        let union = overlay(
            &nominal,
            &fresh,
            Operation::Intersection,
            &request,
            &mut work,
            &cancel,
        )
        .unwrap()
        .unsigned_area();
        let excess = (strip - union).max(0.0);
        assert!(
            excess <= AREA_ROUNDOFF,
            "one non-retracing command has no local overlap excess: {excess}"
        );
    }

    #[test]
    fn gentle_continuing_bend_preserves_negative_exact_local_dose() {
        let material = MultiPolygon(vec![polygon![
            (x: -1., y: -1.), (x: 1., y: -1.), (x: 1., y: 1.), (x: -1., y: 1.),
        ]]);
        let required = region::RequiredMaterial::from_section(&material);
        let recipe = super::super::super::tests::recipe(0.25);
        let limits = DepositionLimits {
            min_bead_width_mm: 0.3,
            max_bead_width_mm: 0.75,
            min_bead_height_mm: 0.000001,
            max_bead_height_mm: 0.3,
        };
        let request = request(&material, &required, &recipe, &limits);
        // One gentle, non-retracing continuing bend. Its actual nominal
        // end-cap credit makes the original signed local dose negative.
        let paths = [path(&[
            [-0.15, 0.],
            [-0.05, 0.],
            [0.05, 0.001],
            [0.15, 0.001],
        ])];
        let cancel = AtomicBool::new(false);
        let mut work = 0;
        let mut nominal = occupancy(&paths, 0.0, &request, &mut work, &cancel).unwrap();
        let live = staged_request(
            &request,
            super::super::super::native::material_bytes(&mut nominal),
        )
        .unwrap();
        let permission = Junction {
            center: [0., 0.],
            radius_mm: 0.75,
            valence: 1,
            ports: Vec::new(),
            transition: None,
        };
        let mut fresh = permission_region(&permission, &live, &mut work, &cancel).unwrap();
        let live = staged_request(
            &live,
            super::super::super::native::material_bytes(&mut fresh),
        )
        .unwrap();
        let signed = strip_area_in(&paths, &fresh, &live, &mut work, &cancel).unwrap()
            - overlay(
                &nominal,
                &fresh,
                Operation::Intersection,
                &live,
                &mut work,
                &cancel,
            )
            .unwrap()
            .unsigned_area();
        assert!(
            signed < 0.0,
            "nominal cap credit makes the exact local gate admit: {signed}"
        );
        let local_budget =
            0.5 * permission.valence.saturating_sub(1) as f64 * limits.max_bead_width_mm.powi(2);
        qualify_local_dose(
            &paths,
            &nominal,
            None,
            &fresh,
            local_budget,
            &live,
            &mut boolean::Workspace::new(),
            &mut work,
            &cancel,
        )
        .expect("negative exact local dose must admit even with zero local budget");
    }

    #[test]
    fn balanced_permission_union_preserves_holes_overlap_and_detached_regions() {
        let material = MultiPolygon(vec![polygon![
            (x: 0., y: 0.), (x: 10., y: 0.), (x: 10., y: 5.), (x: 0., y: 5.),
        ]]);
        let required = region::RequiredMaterial::from_section(&material);
        let recipe = super::super::super::tests::recipe(0.25);
        let limits = DepositionLimits {
            min_bead_width_mm: 0.3,
            max_bead_width_mm: 0.75,
            min_bead_height_mm: 0.000001,
            max_bead_height_mm: 0.3,
        };
        let request = request(&material, &required, &recipe, &limits);
        let operands = [
            MultiPolygon(vec![Polygon::new(
                LineString::from(vec![[0., 0.], [4., 0.], [4., 4.], [0., 4.], [0., 0.]]),
                vec![LineString::from(vec![
                    [1., 1.],
                    [1., 2.],
                    [2., 2.],
                    [2., 1.],
                    [1., 1.],
                ])],
            )]),
            MultiPolygon(vec![
                polygon![(x: 3., y: 1.), (x: 5., y: 1.), (x: 5., y: 3.), (x: 3., y: 3.)],
            ]),
            MultiPolygon(vec![
                polygon![(x: 8., y: 0.), (x: 9., y: 0.), (x: 9., y: 1.), (x: 8., y: 1.)],
            ]),
            MultiPolygon(vec![
                polygon![(x: 3., y: 1.), (x: 3.5, y: 1.), (x: 3.5, y: 2.), (x: 3., y: 2.)],
            ]),
        ];
        let mut expected =
            MultiPolygon(unary_union(operands.iter().flat_map(|region| &region.0)).0);
        let live = staged_request(
            &request,
            super::super::super::native::material_bytes(&mut expected),
        )
        .unwrap();
        let mut regions = Vec::new();
        let mut bytes = 0;
        for (ordinal, mut region) in operands.into_iter().enumerate() {
            bytes += super::super::super::native::material_bytes(&mut region);
            let bounds = region.bounding_rect().unwrap();
            regions.push((region, bounds, ordinal));
        }
        let cancel = AtomicBool::new(false);
        let mut work = 0;
        let mut actual = union_regions(regions, bytes, &live, &mut work, &cancel).unwrap();
        let live = staged_request(
            &live,
            super::super::super::native::material_bytes(&mut actual),
        )
        .unwrap();
        assert!(
            overlay(
                &actual,
                &expected,
                Operation::Difference,
                &live,
                &mut work,
                &cancel
            )
            .unwrap()
            .unsigned_area()
                <= AREA_ROUNDOFF
        );
        assert!(
            overlay(
                &expected,
                &actual,
                Operation::Difference,
                &live,
                &mut work,
                &cancel
            )
            .unwrap()
            .unsigned_area()
                <= AREA_ROUNDOFF
        );
        assert!(
            !actual.contains(&Point::new(1.5, 1.5)),
            "the protected hole stays unfilled"
        );
        assert!(
            actual.contains(&Point::new(8.5, 0.5)),
            "the detached permission stays present"
        );
    }

    #[test]
    fn actual_xyz6_partial_reverse_and_cyclic_retraces_refuse_independently_of_excess() {
        let material = MultiPolygon(vec![
            polygon![(x: -4., y: -4.), (x: 4., y: -4.), (x: 4., y: 4.), (x: -4., y: 4.)],
        ]);
        let required = region::RequiredMaterial::from_section(&material);
        let recipe = super::super::super::tests::recipe(0.25);
        let limits = DepositionLimits {
            min_bead_width_mm: 0.3,
            max_bead_width_mm: 0.75,
            min_bead_height_mm: 0.000001,
            max_bead_height_mm: 0.3,
        };
        let request = request(&material, &required, &recipe, &limits);
        for paths in [
            vec![path(&[[0., 0.], [3., 0.]]), path(&[[0., 0.], [3., 0.]])],
            vec![path(&[[0., 0.], [3., 0.]]), path(&[[2., 0.], [1., 0.]])],
            vec![path(&[[0., 0.], [3., 0.], [1., 0.]])],
            vec![path(&[
                [0., 0.],
                [2., 0.],
                [2., 2.],
                [0., 0.],
                [2., 0.],
                [2., 2.],
                [0., 0.],
            ])],
        ] {
            let failure = unique(
                &paths,
                &MultiPolygon(Vec::new()),
                &request,
                &mut 0,
                &AtomicBool::new(false),
            )
            .unwrap_err();
            assert!(
                failure.message.contains("retraced"),
                "a small duplicate cannot hide behind negative strip excess"
            );
        }
        unique(
            &[path(&[[0., 0.], [1., 0.], [2., 0.]])],
            &MultiPolygon(Vec::new()),
            &request,
            &mut 0,
            &AtomicBool::new(false),
        )
        .expect("harmless collinear subdivision has no positive repeated interval");
    }

    #[test]
    fn stitching_cannot_hide_a_nonlocal_crossing_behind_open_cap_credit() {
        let material = MultiPolygon(vec![polygon![
            (x: 0., y: 0.), (x: 10., y: 0.), (x: 10., y: 10.), (x: 0., y: 10.),
        ]]);
        let required = region::RequiredMaterial::from_section(&material);
        let mut recipe = super::super::super::tests::recipe(0.25);
        recipe.bead_width_mm = 0.45;
        recipe.perimeter_count = 2;
        let limits = DepositionLimits {
            min_bead_width_mm: 0.3,
            max_bead_width_mm: 0.75,
            min_bead_height_mm: 0.000001,
            max_bead_height_mm: 0.3,
        };
        let request = request(&material, &required, &recipe, &limits);
        let core = MultiPolygon(vec![polygon![
            (x: 0.9, y: 0.9), (x: 9.1, y: 0.9), (x: 9.1, y: 9.1), (x: 0.9, y: 9.1),
        ]]);
        let bead = |points: &[[f64; 2]]| {
            let mut bead = path(points);
            bead.width = WidthProfile::Constant { width_mm: 0.45 };
            bead
        };
        let mut walls = Vec::new();
        let mut permissions = Vec::new();
        for inset in [0.225, 0.675] {
            let corners = [
                [inset, inset],
                [10. - inset, inset],
                [10. - inset, 10. - inset],
                [inset, 10. - inset],
            ];
            walls.push(bead(&[
                corners[0], corners[1], corners[2], corners[3], corners[0],
            ]));
            // Original rectangle's explicit source contour-turn events.
            permissions.extend(corners.map(|center| Junction {
                center,
                radius_mm: 0.75,
                valence: 2,
                ports: Vec::new(),
                transition: None,
            }));
        }
        let cancel = AtomicBool::new(false);
        qualify(&request, &walls, &permissions, &core, &mut 0, &cancel)
            .expect("ordinary closed rectangle, including cyclic seams, covers independent R");
        let crossing = [[4.98, 4.98], [5.02, 5.02], [4.98, 5.02], [5.02, 4.98]];
        let mut stitched = walls.clone();
        stitched.push(bead(&crossing));
        let strip: f64 = stitched
            .iter()
            .flat_map(|path| path.checked_edges().unwrap())
            .map(|edge| {
                let p = edge.points_mm;
                norm([p[1][0] - p[0][0], p[1][1] - p[0][1]]) * edge.width_mm
            })
            .sum();
        assert!(
            strip
                < occupancy(&stitched, 0.0, &request, &mut 0, &cancel)
                    .unwrap()
                    .unsigned_area(),
            "whole-path open-cap credit masks this extra stroke in a global excess-only test"
        );
        let mut split = walls;
        split.extend(crossing.windows(2).map(bead));
        for paths in [stitched, split] {
            let failure =
                qualify(&request, &paths, &permissions, &core, &mut 0, &cancel).unwrap_err();
            assert!(
                failure
                    .message
                    .contains("proper nonlocal XYZ6 wall crossing"),
                "{failure}"
            );
        }
    }

    #[test]
    fn wall_qualification_cannot_trade_a_missing_arm_for_global_area() {
        let material = MultiPolygon(vec![polygon![
            (x: -3., y: -0.3), (x: 3., y: -0.3), (x: 3., y: 0.3),
            (x: 0.3, y: 0.3), (x: 0.3, y: 3.), (x: -0.3, y: 3.),
            (x: -0.3, y: 0.3), (x: -3., y: 0.3),
        ]]);
        let required = region::RequiredMaterial::from_section(&material);
        let recipe = super::super::super::tests::recipe(0.25);
        let limits = DepositionLimits {
            min_bead_width_mm: 0.3,
            max_bead_width_mm: 0.75,
            min_bead_height_mm: 0.000001,
            max_bead_height_mm: 0.3,
        };
        let request = request(&material, &required, &recipe, &limits);
        let failure = qualify(
            &request,
            &[path(&[[-2.7, 0.], [2.7, 0.]])],
            &[],
            &MultiPolygon(Vec::new()),
            &mut 0,
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(failure.message.contains("underfill"));
    }
}
