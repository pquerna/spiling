// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Process-limited segment-cell wall construction. Native material remains the
//! authority: a diagram proposes deposition, never source topology or coverage.
use super::{MAX_RETAINED_GEOMETRY_UNITS, boolean, region};
use crate::{check_cancel, error};
mod beading;
mod contours;
mod fitting;
mod qualification;
mod separation;
mod skeleton;
use skeleton::{BoundarySupport, MedialEdge};
#[cfg(test)]
mod tests;
use geo::{Area, MultiPolygon, Polygon};
use spiling_contracts::manufacturing::*;
use std::{collections::BTreeMap, sync::atomic::AtomicBool};

const SITE_SCALE: f64 = 1_000_000.0;
const FIT_ERROR_MM: f64 = 0.0015;
const MAX_REFINEMENT: u8 = 28;

/// Supports are local section-boundary indices, never persistent native faces.
#[derive(Clone)]
pub(super) struct BoundaryEdge {
    pub(super) points: [[f64; 2]; 2],
    pub(super) width_mm: f64,
    /// Continuing proposal field at points[0/1], not commanded occupancy.
    pub(super) source_widths: [f64; 2],
    pub(super) supports: [u32; 2],
    pub(super) facing_boundary: Option<u32>,
    pub(super) ordinal: usize,
}
#[derive(Clone)]
pub(super) struct Junction {
    pub(super) center: [f64; 2],
    pub(super) radius_mm: f64,
    pub(super) valence: usize,
    pub(super) ports: Vec<[[f64; 2]; 2]>,
    pub(super) transition: Option<TransitionCorridor>,
}
#[derive(Clone)]
pub(super) struct TransitionCorridor {
    pub(super) points: Vec<[f64; 2]>,
    pub(super) incoming_count: usize,
    pub(super) outgoing_count: usize,
}

pub(super) struct WallPlan {
    pub(super) paths: Vec<DepositionPath>,
    pub(super) fill: MultiPolygon<f64>,
    pub(super) correspondence: Vec<BoundaryCorrespondence>,
    pub(super) retained_points: usize,
    pub(super) retained_bytes: usize,
    pub(super) section_area_mm2: f64,
    pub(super) wall_area_mm2: f64,
    pub(super) protected_hole_area_mm2: f64,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct BoundaryCorrespondence {
    pub(super) path_index: usize,
    pub(super) edge_index: usize,
    pub(super) supports: [u32; 2],
    pub(super) facing_boundary: Option<u32>,
    pub(super) ordinal: usize,
}

impl WallPlan {
    pub(super) fn point_count(&self) -> usize {
        self.retained_points
    }

    pub(super) fn visit_boundary_edges(
        &self,
        boundary: u32,
        mut visit: impl FnMut(DepositionEdge<'_>) -> Result<(), ManufacturingError>,
    ) -> Result<(), ManufacturingError> {
        let mut offset = 0usize;
        for (path_index, path) in self.paths.iter().enumerate() {
            let edges = path.checked_edges()?;
            let end = offset
                .checked_add(edges.len())
                .ok_or_else(|| resource("wall correspondence overflow"))?;
            let correspondence = self
                .correspondence
                .get(offset..end)
                .ok_or_else(|| unsupported("wall edge correspondence is incomplete"))?;
            for (edge_index, (edge, association)) in edges.zip(correspondence).enumerate() {
                if association.path_index != path_index || association.edge_index != edge_index {
                    return Err(unsupported("wall edge correspondence ordering mismatch"));
                }
                if association.ordinal == 0
                    && association
                        .facing_boundary
                        .map_or(association.supports.contains(&boundary), |side| {
                            side == boundary
                        })
                {
                    visit(edge)?;
                }
            }
            offset = end;
        }
        if offset != self.correspondence.len() {
            return Err(unsupported("wall correspondence has extra edges"));
        }
        Ok(())
    }

    pub(super) fn append_paths(
        &self,
        lower_z: f64,
        upper_z: f64,
        output: &mut Vec<DepositionPath>,
        segments: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        for path in &self.paths {
            check_cancel(cancel)?;
            let count = path.checked_edges()?.len();
            *segments = segments
                .checked_add(count)
                .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
                .ok_or_else(|| resource("wall deposition segment limit exceeded"))?;
            let mut path = path.clone();
            path.physical_z_mm = [lower_z, upper_z];
            for point in &mut path.points_mm {
                point[2] = upper_z;
            }
            output.push(path);
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
pub(super) struct WallRequest<'a> {
    pub(super) material: &'a MultiPolygon<f64>,
    pub(super) required: &'a region::RequiredMaterial<'a>,
    pub(super) recipe: &'a PlanarPrintRecipe,
    pub(super) limits: &'a DepositionLimits,
    pub(super) nozzle_diameter_mm: f64,
    pub(super) lower_z: f64,
    pub(super) upper_z: f64,
    pub(super) retained_points: usize,
    /// TOTAL already-live caller allocations in the planner byte budget.
    /// Semantic retained points are not implicitly converted to bytes.
    pub(super) owned_bytes: usize,
}

#[derive(Clone, Copy)]
struct Allocation {
    count: usize,
    width: f64,
    residual: bool,
}
fn admitted_width_range(request: &WallRequest<'_>) -> Result<[f64; 2], ManufacturingError> {
    width_envelope(request.limits, request.upper_z - request.lower_z)
}

/// Resolve the source proposal objective BEFORE count, depth, birth and packing
/// decisions. Authored preference can lie below a thick layer's guarded floor;
/// it is not permission to emit below that floor or inflate a fitted command.
/// This does not change captured intent or the independent requested-D target.
fn source_width_mm(request: &WallRequest<'_>) -> Result<f64, ManufacturingError> {
    let [minimum, maximum] = admitted_width_range(request)?;
    Ok(request.recipe.bead_width_mm.clamp(minimum, maximum))
}

/// The only physical-height dependency of the XY wall construction. Plans with
/// this exact envelope can share XY paths/qualification; append_paths assigns
/// each actual physical interval, and downstream skins/support remain uncached.
pub(super) fn width_envelope(
    limits: &DepositionLimits,
    height: f64,
) -> Result<[f64; 2], ManufacturingError> {
    let guard = 2.0 * program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM;
    let minimum = limits.min_bead_width_mm.max(height) + guard;
    let maximum = limits.max_bead_width_mm - guard;
    if !height.is_finite() || height <= 0.0 || !minimum.is_finite() || minimum > maximum {
        return Err(unsupported(
            "process width envelope cannot enclose the declared command uncertainty",
        ));
    }
    Ok([minimum, maximum])
}
fn allocate(radius: f64, request: &WallRequest<'_>) -> Result<Allocation, ManufacturingError> {
    let thickness = 2.0 * radius;
    let [minimum, maximum] = admitted_width_range(request)?;
    let preferred = source_width_mm(request)?;
    let target = 2 * request.recipe.perimeter_count as usize;
    if thickness >= target as f64 * preferred {
        return Ok(Allocation {
            count: target,
            width: preferred,
            residual: true,
        });
    }
    if thickness < minimum {
        return Ok(Allocation {
            count: 0,
            width: 0.0,
            residual: false,
        });
    }
    let lower = (thickness / maximum).ceil().max(1.0) as usize;
    let upper = (thickness / minimum).floor() as usize;
    if lower > upper || lower > target {
        return Err(unsupported(
            "local wall thickness has no admitted integer bead allocation",
        ));
    }
    // Prefer the smallest departure from nominal width, then fewer commands.
    let count = (lower..=upper.min(target))
        .min_by(|&a, &b| {
            (thickness / a as f64 - preferred)
                .abs()
                .total_cmp(&(thickness / b as f64 - preferred).abs())
                .then(a.cmp(&b))
        })
        .ok_or_else(|| unsupported("empty wall allocation interval"))?;
    Ok(Allocation {
        count,
        width: thickness / count as f64,
        residual: false,
    })
}

fn adjacent_supports(supports: [BoundarySupport; 2]) -> bool {
    supports.iter().all(|site| site.point.is_none())
        && (supports[0].a == supports[1].a
            || supports[0].a == supports[1].b
            || supports[0].b == supports[1].a
            || supports[0].b == supports[1].b)
}

fn refinement_allocation(
    curve: &MedialEdge,
    t: f64,
    request: &WallRequest<'_>,
) -> Result<Allocation, ManufacturingError> {
    let radius = curve.supports[0].radius(curve.at(t));
    if adjacent_supports(curve.supports) {
        // Sampling seed only: adjacent cells are not opposing thickness.
        // Actual corner widths/contour roots inherit the resolved incoming
        // source-node field in beading::emit_corners.
        let width = source_width_mm(request)?;
        let pairs =
            ((radius / width + 0.5).floor() as usize).min(request.recipe.perimeter_count as usize);
        return Ok(Allocation {
            count: 2 * pairs,
            width,
            residual: radius >= request.recipe.perimeter_count as f64 * width,
        });
    }
    allocate(radius, request)
}

pub(super) fn plan_section_walls(
    request: WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<WallPlan, ManufacturingError> {
    plan_section_walls_against(request, false, work, cancel)
}

fn plan_section_walls_against<'a>(
    request: WallRequest<'a>,
    strict_material: bool,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<WallPlan, ManufacturingError> {
    check_cancel(cancel)?;

    request.limits.validate()?;
    if request.material.0.is_empty() || request.required.polygons.is_empty() {
        return Err(unsupported(
            "wall construction requires requested and mandatory material",
        ));
    }
    // Maximum contour-depth is captured intent, not a promise of N closed
    // loops. Define its material target before proposing any deposited sweep.
    let mut fill = region::offset_admitted(
        request.material,
        -(request.recipe.perimeter_count as f64) * request.recipe.bead_width_mm,
        |scratch, scratch_bytes, overlay| {
            if request
                .retained_points
                .checked_add(scratch)
                .is_none_or(|n| n > MAX_RETAINED_GEOMETRY_UNITS)
            {
                return Err(resource("wall target construction staging exceeded"));
            }
            admit_stage(&request, scratch_bytes)?;
            if let Some((a, b)) = overlay {
                boolean::budget_accumulated(a, b, work, cancel)?;
            }
            Ok(())
        },
        cancel,
    )?;
    let target_points = request
        .retained_points
        .checked_add(region::point_count(&fill))
        .ok_or_else(|| resource("wall target point inventory overflow"))?;
    let fill_bytes = super::native::material_bytes(&mut fill);
    let constructor_owned_bytes = request
        .owned_bytes
        .checked_add(fill_bytes)
        .ok_or_else(|| resource("wall constructor external ownership overflow"))?;

    let skeleton = skeleton::construct(
        request.material,
        target_points,
        constructor_owned_bytes,
        work,
        cancel,
    )?;
    let external_owned_bytes = request.owned_bytes;
    let external_retained_points = request.retained_points;
    let mut request = request;
    request.retained_points = target_points
        .checked_add(skeleton.retained_points)
        .ok_or_else(|| resource("wall source-vertex inventory overflow"))?;
    request.owned_bytes = constructor_owned_bytes
        .checked_add(skeleton.edges.capacity() * std::mem::size_of::<MedialEdge>())
        .and_then(|b| {
            b.checked_add(
                skeleton.junctions.capacity() * std::mem::size_of::<skeleton::MedialJunction>(),
            )
        })
        .ok_or_else(|| resource("wall source staging byte overflow"))?;
    admit_stage(&request, 0)?;

    let mut edge_output = Vec::new();
    let mut junctions = Vec::new();
    let mut source_plan =
        beading::construct(&skeleton, &fill, &request, &mut junctions, work, cancel)?;

    let (paths, correspondence, qualified_fill) = loop {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;

        let candidate: Result<_, ManufacturingError> = (|| {
            beading::emit(
                &source_plan,
                &skeleton,
                &fill,
                &request,
                &mut edge_output,
                &mut junctions,
                work,
                cancel,
            )?;

            // The retained immutable source plan, graph and candidate buffers
            // coexist through assembly and independent qualification.
            let mut candidate_request = request;
            candidate_request.owned_bytes = candidate_request
                .owned_bytes
                .checked_add(beading::retained_bytes(&source_plan))
                .and_then(|b| {
                    b.checked_add(edge_output.capacity() * std::mem::size_of::<BoundaryEdge>())
                })
                .and_then(|b| b.checked_add(permission_bytes(&junctions)))
                .ok_or_else(|| resource("wall candidate staging byte overflow"))?;
            let assembly_bytes = edge_output.len()
                * (3 * std::mem::size_of::<DepositionPath>() + 6 * std::mem::size_of::<[f64; 3]>()
                + 3 * std::mem::size_of::<BoundaryCorrespondence>()
                // Source variable-width growth can retain N old + 2N new
                // values; constant-to-fitted output needs at most two N buffers.
                + 3 * std::mem::size_of::<f64>() + 256);
            let assembly_request = staged_request(&candidate_request, assembly_bytes)?;
            let (mut paths, correspondence) = assemble(
                &edge_output,
                &junctions,
                &assembly_request,
                strict_material,
                work,
                cancel,
            )?;
            if strict_material {
                fit_material_boundaries(&mut paths, &candidate_request, work, cancel)?;
            }
            candidate_request.owned_bytes = candidate_request
                .owned_bytes
                .checked_add(path_bytes(&paths))
                .and_then(|b| {
                    b.checked_add(
                        correspondence.capacity() * std::mem::size_of::<BoundaryCorrespondence>(),
                    )
                })
                .ok_or_else(|| resource("wall command candidate byte overflow"))?;

            let qualified_fill = qualification::qualify(
                &candidate_request,
                &paths,
                &junctions,
                &fill,
                work,
                cancel,
            )?;
            Ok((paths, correspondence, qualified_fill))
        })();

        match candidate {
            Ok(qualified) => break qualified,
            Err(reason)
                if reason.code == ManufacturingErrorCode::UnsupportedGeometry
                    && beading::advance(&mut source_plan) => {}
            Err(reason) => return Err(reason),
        }
    };
    drop(source_plan);
    drop(edge_output);
    drop(skeleton);
    drop(junctions);
    drop(fill);
    request.retained_points = external_retained_points;
    let mut fill = qualified_fill;
    let retained_bytes = std::mem::size_of::<WallPlan>()
        .checked_add(path_bytes(&paths))
        .and_then(|b| {
            b.checked_add(correspondence.capacity() * std::mem::size_of::<BoundaryCorrespondence>())
        })
        .and_then(|b| b.checked_add(super::native::material_bytes(&mut fill)))
        .ok_or_else(|| resource("retained wall byte inventory overflow"))?;
    request.owned_bytes = external_owned_bytes
        .checked_add(retained_bytes)
        .ok_or_else(|| resource("retained wall external ownership overflow"))?;
    // Geometry reservation units and actual allocated bytes are independent of
    // the published command ceiling. XYZ3/width/association staging must never
    // silently turn the declared100k-command policy into a50k-command policy.
    let retained_points =
        paths.iter().map(|p| p.points_mm.len()).sum::<usize>() + region::point_count(&fill);
    if retained_points
        .checked_add(request.retained_points)
        .is_none_or(|n| n > MAX_RETAINED_GEOMETRY_UNITS)
    {
        return Err(resource("retained wall construction limit exceeded"));
    }
    admit_stage(&request, 0)?;
    let section_area_mm2 = request.material.unsigned_area();
    let wall_area_mm2 = (section_area_mm2 - fill.unsigned_area()).max(0.0);
    let protected_hole_area_mm2 = request
        .material
        .0
        .iter()
        .flat_map(Polygon::interiors)
        .map(|ring| {
            ring.0.first().map_or(0.0, |origin| {
                0.5 * ring
                    .0
                    .windows(2)
                    .map(|pair| {
                        cross(
                            [pair[0].x - origin.x, pair[0].y - origin.y],
                            [pair[1].x - origin.x, pair[1].y - origin.y],
                        )
                    })
                    .sum::<f64>()
                    .abs()
            })
        })
        .sum();

    Ok(WallPlan {
        paths,
        fill,
        correspondence,
        retained_points,
        retained_bytes,
        section_area_mm2,
        wall_area_mm2,
        protected_hole_area_mm2,
    })
}

/// Use the same source wall construction and qualification with an additional
/// actual-edge width fit at material boundaries. Neither source domains nor the
/// existing coverage allowance change.
pub(super) fn plan_material_domain_walls(
    request: WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<WallPlan, ManufacturingError> {
    plan_section_walls_against(request, true, work, cancel)
}

fn fit_material_boundaries(
    paths: &mut [DepositionPath],
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    use geo::{Coord, Distance, Euclidean, Line};
    for path in paths {
        let mut widths = Vec::with_capacity(path.points_mm.len() - 1);
        for edge in path.checked_edges()? {
            let a = program_coordinates::project_xyz6_point_mm(edge.points_mm[0])?;
            let b = program_coordinates::project_xyz6_point_mm(edge.points_mm[1])?;
            let line = Line::new(Coord { x: a[0], y: a[1] }, Coord { x: b[0], y: b[1] });
            let mut clearance = f64::INFINITY;
            for boundary in request
                .material
                .0
                .iter()
                .flat_map(|p| std::iter::once(p.exterior()).chain(p.interiors()))
                .flat_map(|r| r.lines())
            {
                check_cancel(cancel)?;
                boolean::charge(work, 1)?;
                clearance = clearance.min(Euclidean.distance(&line, &boundary));
            }
            // One Cartesian quantum provides a strict candidate reserve. The
            // independent interval capsule predicate owns final admission.
            let width = edge.width_mm.min(2.0 * (clearance - 0.000001));
            if !request.limits.admits(width, path.height_mm()) {
                return Err(unsupported(
                    "material boundary leaves no admitted full-bead width",
                ));
            }
            widths.push(width);
        }
        if widths.iter().any(|w| !w.is_finite() || *w <= 0.0) {
            return Err(unsupported("unconditioned material-boundary width"));
        }
        if widths.iter().all(|w| *w == widths[0]) {
            path.width = WidthProfile::Constant {
                width_mm: widths[0],
            };
        } else {
            path.width = WidthProfile::PerEdge { widths_mm: widths };
        }
    }
    Ok(())
}

fn refine(
    curve: &MedialEdge,
    lo: f64,
    hi: f64,
    depth: u8,
    request: &WallRequest<'_>,
    output: &mut Vec<(f64, [f64; 2])>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    boolean::charge(work, 1)?;
    let a = curve.at(lo);
    let b = curve.at(hi);
    let mid_t = (lo + hi) * 0.5;
    let bound = curve.chord_error_bound(lo, hi);
    let length = norm(sub(b, a));
    let allocations = [
        refinement_allocation(curve, lo, request)?,
        refinement_allocation(curve, mid_t, request)?,
        refinement_allocation(curve, hi, request)?,
    ];
    let stable = allocations.iter().all(|allocation| {
        allocation.count == allocations[1].count && allocation.residual == allocations[1].residual
    });
    let low_width = allocations
        .iter()
        .map(|allocation| allocation.width)
        .fold(f64::INFINITY, f64::min);
    let high_width = allocations
        .iter()
        .map(|allocation| allocation.width)
        .fold(0.0, f64::max);
    let mut fit = bound;
    for side in curve.supports {
        if let Some(focus) = side.point {
            let u = sub(a, focus);
            let v = sub(b, focus);
            let angle = cross(u, v).atan2(dot(u, v)).abs();
            let offset = request.recipe.perimeter_count as f64 * source_width_mm(request)?;
            fit = fit.max(offset * (1.0 - (angle * 0.5).cos()) + bound);
        }
    }
    // Constant-width straight portions need no artificial command subdivision.
    // A finite count/birth event is localized inside its preauthorized junction
    // patch; final actual occupancy, command and overlap qualification still gate it.
    let event = !stable && length <= 0.02 && bound <= FIT_ERROR_MM;
    if (stable && fit + (high_width - low_width) * 0.5 <= FIT_ERROR_MM) || event {
        if output.len() >= MAX_DEPOSITION_SEGMENTS as usize {
            return Err(resource("wall-cell subdivision limit exceeded"));
        }
        reserve_stage(output, 1, request, 0)?;
        output.push((hi, b));
        return Ok(());
    }
    if depth == MAX_REFINEMENT {
        return Err(unsupported(
            "wall-cell refinement failed to enclose the construction",
        ));
    }
    refine(curve, lo, mid_t, depth + 1, request, output, work, cancel)?;
    refine(curve, mid_t, hi, depth + 1, request, output, work, cancel)
}

fn path_bytes(paths: &Vec<DepositionPath>) -> usize {
    paths.capacity() * std::mem::size_of::<DepositionPath>()
        + paths
            .iter()
            .map(|path| {
                path.points_mm.capacity() * std::mem::size_of::<[f64; 3]>()
                    + match &path.width {
                        WidthProfile::Constant { .. } => 0,
                        WidthProfile::PerEdge { widths_mm } => {
                            widths_mm.capacity() * std::mem::size_of::<f64>()
                        }
                    }
            })
            .sum::<usize>()
}
fn permission_bytes(permissions: &Vec<Junction>) -> usize {
    permissions.capacity() * std::mem::size_of::<Junction>()
        + permissions
            .iter()
            .map(|j| {
                j.ports.capacity() * std::mem::size_of::<[[f64; 2]; 2]>()
                    + j.transition
                        .as_ref()
                        .map_or(0, |t| t.points.capacity() * std::mem::size_of::<[f64; 2]>())
            })
            .sum::<usize>()
}
fn admit_stage(request: &WallRequest<'_>, bytes: usize) -> Result<(), ManufacturingError> {
    if request.retained_points > MAX_RETAINED_GEOMETRY_UNITS
        || request
            .owned_bytes
            .checked_add(bytes)
            .is_none_or(|total| total as u64 > MAX_MANUFACTURING_RETAINED_BYTES)
    {
        return Err(resource("wall owned/staging byte budget exceeded"));
    }
    Ok(())
}
fn staged_request<'a>(
    request: &WallRequest<'a>,
    bytes: usize,
) -> Result<WallRequest<'a>, ManufacturingError> {
    let mut staged = *request;
    staged.owned_bytes = staged
        .owned_bytes
        .checked_add(bytes)
        .ok_or_else(|| resource("wall staging ownership overflow"))?;
    admit_stage(&staged, 0)?;
    Ok(staged)
}

/// Geometric growth: admit the old and replacement allocations before reserve.
/// `other_bytes` excludes this vector; `request` already owns the shared pool.
fn reserve_stage<T>(
    values: &mut Vec<T>,
    additional: usize,
    request: &WallRequest<'_>,
    other_bytes: usize,
) -> Result<(), ManufacturingError> {
    let needed = values
        .len()
        .checked_add(additional)
        .ok_or_else(|| resource("wall staging element-count overflow"))?;
    if needed <= values.capacity() {
        return Ok(());
    }
    let next = needed.max(
        values
            .capacity()
            .max(4)
            .checked_mul(2)
            .ok_or_else(|| resource("wall staging capacity overflow"))?,
    );
    let bytes = values
        .capacity()
        .checked_add(next)
        .and_then(|n| n.checked_mul(std::mem::size_of::<T>()))
        .and_then(|b| b.checked_add(other_bytes))
        .ok_or_else(|| resource("wall staging allocation-byte overflow"))?;
    admit_stage(request, bytes)?;
    values
        .try_reserve_exact(next - values.len())
        .map_err(|_| resource("wall staging allocation refused"))?;
    admit_stage(
        request,
        other_bytes
            .checked_add(values.capacity() * std::mem::size_of::<T>())
            .ok_or_else(|| resource("wall staging retained-byte overflow"))?,
    )
}

fn push_edge(
    output: &mut Vec<BoundaryEdge>,
    points: [[f64; 2]; 2],
    width_mm: f64,
    supports: [u32; 2],
    facing_boundary: Option<u32>,
    ordinal: usize,
    source_widths: [f64; 2],
    request: &WallRequest<'_>,
) -> Result<(), ManufacturingError> {
    if points.iter().flatten().any(|p| !p.is_finite())
        || !width_mm.is_finite()
        || width_mm <= 0.0
        || source_widths
            .into_iter()
            .any(|width| !width.is_finite() || width <= 0.0)
    {
        return Err(unsupported(&format!(
            "nonfinite/nonpositive wall allocation: points={points:?}, width={width_mm}, supports={supports:?}, facing={facing_boundary:?}, ordinal={ordinal}"
        )));
    }
    if key(points[0])? == key(points[1])? {
        return Ok(());
    }
    if output.len() >= MAX_DEPOSITION_SEGMENTS as usize {
        return Err(resource("wall command count exceeded"));
    }
    reserve_stage(output, 1, request, 0)?;
    output.push(BoundaryEdge {
        points,
        width_mm,
        source_widths,
        supports,
        facing_boundary,
        ordinal,
    });
    Ok(())
}
type Key = [i64; 2];
fn key(p: [f64; 2]) -> Result<Key, ManufacturingError> {
    let project = program_coordinates::project_xyz6_coordinate_mm;
    Ok([
        (project(p[0])? * SITE_SCALE).round() as i64,
        (project(p[1])? * SITE_SCALE).round() as i64,
    ])
}
fn point(k: Key) -> [f64; 2] {
    k.map(|v| v as f64 / SITE_SCALE)
}
/// Pair continuing source ordinals before walking or fitting actual commands.
fn source_mates(
    edges: &[BoundaryEdge],
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Vec<[Option<(usize, usize)>; 2]>, ManufacturingError> {
    let mut incidence = BTreeMap::<Key, Vec<(usize, usize)>>::new();
    for (index, edge) in edges.iter().enumerate() {
        for side in 0..2 {
            incidence
                .entry(key(edge.points[side])?)
                .or_default()
                .push((index, side));
        }
    }
    let mut mate = vec![[None::<(usize, usize)>; 2]; edges.len()];
    for (&node, incident) in &incidence {
        loop {
            let mut best: Option<((usize, usize), (usize, usize), usize, f64)> = None;
            for (at, &(a, a_side)) in incident.iter().enumerate() {
                if mate[a][a_side].is_some() {
                    continue;
                }
                for &(b, b_side) in &incident[at + 1..] {
                    check_cancel(cancel)?;
                    boolean::charge(work, 1)?;
                    if a == b || mate[b][b_side].is_some() || edges[a].ordinal != edges[b].ordinal {
                        continue;
                    }
                    if edges[a].facing_boundary.is_some()
                        && edges[b].facing_boundary.is_some()
                        && edges[a].facing_boundary != edges[b].facing_boundary
                    {
                        continue;
                    }
                    let da = sub(point(key(edges[a].points[1 - a_side])?), point(node));
                    let db = sub(point(key(edges[b].points[1 - b_side])?), point(node));
                    let lengths = [norm(da), norm(db)];
                    if lengths
                        .into_iter()
                        .any(|length| length <= 0.0 || !length.is_finite())
                    {
                        continue;
                    }
                    let same_supports = edges[a].supports == edges[b].supports
                        || edges[a].supports == [edges[b].supports[1], edges[b].supports[0]];
                    let continuous_source = same_supports
                        && edges[a].facing_boundary == edges[b].facing_boundary
                        && edges[a].source_widths[a_side] == edges[b].source_widths[b_side];
                    if !continuous_source
                        && (edges[a].width_mm - edges[b].width_mm).abs()
                            > program_coordinates::MAX_WALL_WIDTH_GRADIENT
                                * (lengths[0] + lengths[1])
                                * 0.5
                    {
                        continue;
                    }
                    let association =
                        usize::from(edges[a].facing_boundary != edges[b].facing_boundary);
                    let tangent = dot(da, db) / (lengths[0] * lengths[1]);
                    if best.as_ref().is_none_or(|candidate| {
                        association < candidate.2
                            || association == candidate.2 && tangent.total_cmp(&candidate.3).is_lt()
                    }) {
                        best = Some(((a, a_side), (b, b_side), association, tangent));
                    }
                }
            }
            let Some((a, b, _, _)) = best else {
                break;
            };
            mate[a.0][a.1] = Some(b);
            mate[b.0][b.1] = Some(a);
        }
    }
    Ok(mate)
}

fn assemble(
    edges: &[BoundaryEdge],
    permissions: &[Junction],
    request: &WallRequest<'_>,
    omit_inner_subcommand_loops: bool,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(Vec<DepositionPath>, Vec<BoundaryCorrespondence>), ManufacturingError> {
    let [minimum_width_mm, _] = admitted_width_range(request)?;
    let (lower_z, upper_z) = (request.lower_z, request.upper_z);
    let mate = source_mates(edges, work, cancel)?;
    let mut used = vec![false; edges.len()];
    let mut paths = Vec::new();
    let mut correspondence = Vec::with_capacity(edges.len());
    for pass in 0..2 {
        for seed in 0..edges.len() {
            check_cancel(cancel)?;
            if used[seed] {
                continue;
            }
            let ends = edges[seed].points.map(key);
            let ends = [
                ends[0].as_ref().map_err(Clone::clone)?.to_owned(),
                ends[1].as_ref().map_err(Clone::clone)?.to_owned(),
            ];
            if pass == 0 && mate[seed].iter().all(Option::is_some) {
                continue;
            }
            let start_side = usize::from(mate[seed][0].is_some() && mate[seed][1].is_none());
            let start = ends[start_side];
            let mut current = (seed, start_side);
            let correspondence_base = correspondence.len();
            let mut points_mm = vec![[point(start)[0], point(start)[1], upper_z]];
            let mut width = WidthProfile::Constant {
                width_mm: edges[seed].width_mm,
            };
            loop {
                boolean::charge(work, 1)?;
                check_cancel(cancel)?;
                let (index, side) = current;
                if used[index] {
                    break;
                }
                used[index] = true;
                correspondence.push(BoundaryCorrespondence {
                    path_index: paths.len(),
                    edge_index: points_mm.len() - 1,
                    supports: edges[index].supports,
                    facing_boundary: edges[index].facing_boundary,
                    ordinal: edges[index].ordinal,
                });
                let end = key(edges[index].points[1 - side])?;
                let p = point(end);
                let next_width = edges[index].width_mm;
                match &mut width {
                    WidthProfile::Constant { width_mm } if *width_mm == next_width => {}
                    WidthProfile::Constant { width_mm } => {
                        let mut widths_mm = vec![*width_mm; points_mm.len() - 1];
                        widths_mm.push(next_width);
                        width = WidthProfile::PerEdge { widths_mm };
                    }
                    WidthProfile::PerEdge { widths_mm } => widths_mm.push(next_width),
                }
                points_mm.push([p[0], p[1], upper_z]);
                let Some(next) = mate[index][1 - side] else {
                    break;
                };
                if next == (seed, start_side) {
                    break;
                }
                if used[next.0] {
                    return Err(unsupported(
                        "wall source walk returned through a different consumed port",
                    ));
                }
                current = next;
            }
            // A count-death proposal can close into a sub-command inner loop
            // through skeleton/command rounding. It is not an admitted path or
            // source boundary. Retain all exterior ordinals and open strokes;
            // propose omission only for the complete tiny inner loop, then run
            // unchanged original-component coverage, extent, source/void, dose
            // and junction-connection qualification over the complete candidate.
            if omit_inner_subcommand_loops
                && points_mm.first() == points_mm.last()
                && correspondence[correspondence_base..]
                    .iter()
                    .all(|c| c.ordinal > 0)
                && points_mm.len() <= 8
            {
                boolean::charge(work, points_mm.len() * points_mm.len())?;
                if points_mm.iter().all(|a| {
                    points_mm.iter().all(|b| {
                        (a[0] - b[0]).hypot(a[1] - b[1])
                            < program_coordinates::MIN_DEPOSITION_COMMAND_MM
                    })
                }) {
                    correspondence.truncate(correspondence_base);
                    continue;
                }
            }
            let mut path = DepositionPath {
                material_domain: None,
                kind: DepositionKind::Perimeter,
                owner: Some(DepositionOwner::Wall),
                width,
                physical_z_mm: [lower_z, upper_z],
                points_mm,
            };
            let original = fitting::OriginalPath {
                edges,
                mates: &mate,
                first: (seed, start_side),
                minimum_width_mm,
                owned_bytes: request.owned_bytes,
            };
            fitting::coalesce(
                &mut path,
                &mut correspondence,
                correspondence_base,
                permissions,
                original,
                work,
                cancel,
            )?;
            // Assembly's admitted old/new coordinate/profile peaks include this
            // compaction. Do not cache microscopic source-sampling capacities for
            // every layer after most of their points have been fitted away.
            compact_retained(&mut path.points_mm, work, cancel)?;
            if let WidthProfile::PerEdge { widths_mm } = &mut path.width {
                compact_retained(widths_mm, work, cancel)?;
            }
            paths.push(path);
        }
    }
    compact_retained(&mut correspondence, work, cancel)?;
    Ok((paths, correspondence))
}

fn compact_retained<T: Copy>(
    values: &mut Vec<T>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    // Preserve ordinary geometric-growth slack; replace only an allocation
    // retaining at least twice its live payload. The caller's assembly peak
    // admits the old and replacement buffers together before this reserve.
    if values.capacity() < values.len().saturating_mul(2) {
        return Ok(());
    }
    check_cancel(cancel)?;
    boolean::charge(work, values.len())?;
    let mut retained = Vec::new();
    retained
        .try_reserve_exact(values.len())
        .map_err(|_| resource("wall retained-buffer compaction allocation failed"))?;
    retained.extend_from_slice(values);
    *values = retained;
    check_cancel(cancel)
}

fn unsupported(message: &str) -> ManufacturingError {
    error(ManufacturingErrorCode::UnsupportedGeometry, message)
}
fn resource(message: &str) -> ManufacturingError {
    error(ManufacturingErrorCode::ResourceLimit, message)
}
fn add(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] + b[0], a[1] + b[1]]
}
fn sub(a: [f64; 2], b: [f64; 2]) -> [f64; 2] {
    [a[0] - b[0], a[1] - b[1]]
}
fn mul(a: [f64; 2], b: f64) -> [f64; 2] {
    [a[0] * b, a[1] * b]
}
fn dot(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[0] + a[1] * b[1]
}
fn cross(a: [f64; 2], b: [f64; 2]) -> f64 {
    a[0] * b[1] - a[1] * b[0]
}
fn norm(a: [f64; 2]) -> f64 {
    a[0].hypot(a[1])
}
