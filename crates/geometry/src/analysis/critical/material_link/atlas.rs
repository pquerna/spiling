// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Complete native trimmed-boundary cylinder atlas and independent all-face
//! physical fiber calibration. No source-transport homotopy participates.

use super::*;
use crate::analysis::placed::ActualMaterialCoordinates;
use crate::parametric::{
    SurfaceFunctional, surface_functional_enclosures, surface_piecewise_first_enclosure,
};
type Box3 = [Interval; 3];
type BoundaryVisitor<'a> = dyn for<'s> FnMut(
        BoundaryCell<'s>,
        bool,
        bool,
        &mut GeometryWorkBudget,
        &AtomicBool,
    ) -> Result<BoundaryVisit, GeometryError>
    + 'a;
const STACK_BYTES: usize = UV_EMBEDDING_STACK_BYTES
    + std::mem::size_of::<[([[f64; 2]; 2], u8, bool); MAX_DEPTH as usize + 1]>()
    + crate::analysis::slab::HEIGHT_CONTRACTION_SCRATCH_BYTES
    + crate::parametric::surface_functional_scratch_bytes::<1>()
    + std::mem::size_of::<PairedMaterialLink<'_, '_>>();
fn outside(z: Interval, interval: [f64; 2]) -> bool {
    z.hi < interval[0] || z.lo > interval[1]
}

struct RadialInventory<'v, 'f> {
    visitor: &'v mut BoundaryVisitor<'f>,
    outer_minimum: Option<f64>,
    inner_maximum: Option<f64>,
}
impl RadialInventory<'_, '_> {
    fn include(
        &mut self,
        cell: BoundaryCell<'_>,
        normal: Box3,
        coordinates: &ActualMaterialCoordinates<'_>,
        can_refine: bool,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<BoundaryVisit, GeometryError> {
        let q = coordinates.position_relative_to_center_family(cell.position)?;
        let n = coordinates.normal(normal)?;
        let determinant = n[0].mul(q[0]).add(n[1].mul(q[1]));
        let radius = q[0].square().add(q[1].square());
        if radius.lo <= 0.0 {
            return Ok(BoundaryVisit::RefineSpatial);
        }
        let inward = if determinant.lo > 0.0 {
            false
        } else if determinant.hi < 0.0 {
            true
        } else {
            return Ok(BoundaryVisit::RefineSpatial);
        };
        let visited = (self.visitor)(cell, inward, can_refine, work, cancel)?;
        if visited != BoundaryVisit::Accepted {
            return Ok(visited);
        }
        if inward {
            self.inner_maximum = Some(self.inner_maximum.map_or(radius.hi, |r| r.max(radius.hi)));
        } else {
            self.outer_minimum = Some(self.outer_minimum.map_or(radius.lo, |r| r.min(radius.lo)));
        }
        Ok(BoundaryVisit::Accepted)
    }
    fn ordered(&self) -> bool {
        match (self.inner_maximum, self.outer_minimum) {
            (Some(inner), Some(outer)) => inner < outer,
            (None, Some(_)) => true,
            _ => false,
        }
    }
}

fn height_functional(coordinates: &ActualMaterialCoordinates<'_>) -> HeightFunctional {
    let direction = coordinates.affine_rows()[2];
    HeightFunctional {
        direction,
        offset: coordinates.height_at_anchor().sub(dot(
            direction,
            coordinates.source_anchor().map(Interval::point),
        )),
    }
}

fn visit_native_faces(
    source: MaterialSourceView<'_, '_>,
    coordinates: &ActualMaterialCoordinates<'_>,
    remaining: usize,
    inventory: &mut RadialInventory<'_, '_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let height = height_functional(coordinates);
    let target = coordinates.world_interval();
    for fi in 0..source.definition().face_ids().len() {
        let domain = source.face_domain(fi, remaining, work, cancel)?;
        if !domain.is_native_material() {
            return Err(GeometryError::new(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "native boundary atlas received a provenance-only chart",
            ));
        }
        let surface = domain.surface_operand();
        let whole = surface_piecewise_first_enclosure(&surface, domain.uv_domain(), work, cancel)?;
        if outside(height.height(whole.position), target) {
            continue;
        }
        let sphere = source_chart::SphereChart::new(&surface, domain, work, cancel)?;
        // Reuse complete native C1 admission. Unresolved carriers still need
        // the actual CLOSED-scope interface/G1 qualification before partitioning.
        if !prepared_carrier_c1_covers(domain, domain.uv_domain()) {
            capture_face_gluing(
                &surface,
                domain.uv_domain(),
                height,
                target,
                fi,
                work,
                cancel,
            )?;
        }
        for span in surface_parameter_boxes(&surface, domain.uv_domain(), work, cancel)? {
            let mut stack = [([[0.0; 2]; 2], 0_u8, false); MAX_DEPTH as usize + 1];
            stack[0] = (span, 0, false);
            let mut pending = 1;
            while pending != 0 {
                work.charge(1, cancel)?;
                pending -= 1;
                let (uv, depth, mut inside) = stack[pending];
                let Some(uv) = crate::analysis::slab::clip_height_parameter_box(
                    &surface, uv, height, target, work, cancel,
                )?
                else {
                    continue;
                };
                let jet = surface_piecewise_first_enclosure(&surface, uv, work, cancel)?;
                if outside(coordinates.position(jet.position)?[2], target) {
                    continue;
                }
                let n = match &sphere {
                    Some(sphere) => sphere.normal(jet.position)?,
                    None => first_normal(&jet, domain.orientation()),
                };
                let can_refine = depth < MAX_DEPTH
                    && uv.iter().any(|r| {
                        let m = r[0] * 0.5 + r[1] * 0.5;
                        r[0] < m && m < r[1]
                    });
                let included = inventory.include(
                    BoundaryCell {
                        position: jet.position,
                        native_face: Some((surface, uv)),
                    },
                    n,
                    coordinates,
                    can_refine,
                    work,
                    cancel,
                )?;
                if included == BoundaryVisit::Accepted {
                    continue;
                }
                if !inside {
                    match domain.classify_with_byte_limit(uv, remaining, work, cancel)? {
                        TrimClassification::Outside => continue,
                        TrimClassification::Inside => inside = true,
                        TrimClassification::Boundary => {}
                    }
                }
                if !can_refine {
                    return Ok(false);
                }
                let children = if included == BoundaryVisit::RefineActualXy {
                    let rows = coordinates.affine_rows();
                    let du = [dot(rows[0], jet.du), dot(rows[1], jet.du)];
                    let dv = [dot(rows[0], jet.dv), dot(rows[1], jet.dv)];
                    split_metric_derivatives(uv, depth, fi, &du, &dv)?
                } else {
                    split_metric_derivatives(uv, depth, fi, &jet.du, &jet.dv)?
                };
                if pending + children.len() > stack.len() {
                    return Err(resource());
                }
                for child in children {
                    stack[pending] = (child, depth + 1, inside);
                    pending += 1;
                }
            }
        }
    }
    Ok(true)
}

fn visit_native_links(
    source: MaterialSourceView<'_, '_>,
    incidence: &SourceEdgeIncidence<'_>,
    coordinates: &ActualMaterialCoordinates<'_>,
    remaining: usize,
    inventory: &mut RadialInventory<'_, '_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let interval = coordinates.world_interval();
    for uses in incidence.values() {
        let Some(second) = uses.second else {
            return Ok(false);
        };
        let first = uses.first;
        let a = source.face_domain(first.face, remaining, work, cancel)?;
        let b = source.face_domain(second.face, remaining, work, cancel)?;
        let range = a.coedges()[first.coedge].native_range;
        let Some(link) = PairedMaterialLink::new(
            source.definition(),
            a,
            first,
            b,
            second,
            range,
            remaining,
            work,
            cancel,
        )?
        else {
            return Ok(false);
        };
        let cuts = native_coedge_parameter_cuts(
            [(a, first.coedge), (b, second.coedge)],
            remaining,
            work,
            cancel,
        )?;
        let available = remaining
            .checked_sub(cuts.capacity().saturating_mul(std::mem::size_of::<f64>()))
            .ok_or_else(resource)?;
        let completed = a.with_native_spatial_curve(
            first.coedge,
            available,
            work,
            cancel,
            |curve, work, cancel| {
                for pair in cuts.windows(2) {
                    if pair[0] >= pair[1] {
                        continue;
                    }
                    let mut stack = [([0.0; 2], 0_u8); MAX_DEPTH as usize + 1];
                    stack[0] = ([pair[0], pair[1]], 0);
                    let mut pending = 1;
                    while pending != 0 {
                        pending -= 1;
                        let (range, depth) = stack[pending];
                        let jet = crate::parametric::curve_enclosure(
                            curve,
                            (range[0], range[1]),
                            work,
                            cancel,
                        )?;
                        let position = link.swept_enclosure(range, &jet, work, cancel)?;
                        if outside(coordinates.position(position)?[2], interval) {
                            continue;
                        }
                        let mid = range[0] * 0.5 + range[1] * 0.5;
                        let can_refine = depth < MAX_DEPTH && range[0] < mid && mid < range[1];
                        let mut accepted = false;
                        if let Ok((position, first_normal, second_normal)) =
                            link.normals(range, &jet, work, cancel)?
                        {
                            // Both same-oriented material halves have a nonzero
                            // determinant in the SAME native cylinder projection.
                            let q = coordinates.position_relative_to_center_family(position)?;
                            let na = coordinates.normal(first_normal)?;
                            let nb = coordinates.normal(second_normal)?;
                            let da = na[0].mul(q[0]).add(na[1].mul(q[1]));
                            let db = nb[0].mul(q[0]).add(nb[1].mul(q[1]));
                            if da.lo > 0.0 && db.lo > 0.0 || da.hi < 0.0 && db.hi < 0.0 {
                                accepted = inventory.include(
                                    BoundaryCell {
                                        position,
                                        native_face: None,
                                    },
                                    first_normal,
                                    coordinates,
                                    can_refine,
                                    work,
                                    cancel,
                                )? == BoundaryVisit::Accepted;
                            }
                        }
                        if accepted {
                            continue;
                        }
                        if !can_refine {
                            return Ok(false);
                        }
                        if pending + 2 > stack.len() {
                            return Err(resource());
                        }
                        stack[pending] = ([mid, range[1]], depth + 1);
                        stack[pending + 1] = ([range[0], mid], depth + 1);
                        pending += 2;
                    }
                }
                Ok(true)
            },
        )?;
        if !completed {
            return Ok(false);
        }
    }
    let vertices = incidence.vertices().ok_or_else(resource)?;
    let rows = coordinates.affine_rows();
    for vertex in vertices {
        let point = source
            .trim_source()
            .native_vertex_point(vertex, work, cancel)?
            .map(Interval::point);
        if outside(coordinates.position(point)?[2], interval) {
            continue;
        }
        let q = coordinates.position_relative_to_center_family(point)?;
        let theta =
            std::array::from_fn(|axis| rows[1][axis].mul(q[0]).sub(rows[0][axis].mul(q[1])));
        if !vertex_fan::qualify_native_vertex(
            source,
            incidence,
            vertex,
            [theta, rows[2]],
            remaining,
            work,
            cancel,
        )? {
            return Ok(false);
        }
        // Zero-dimensional native strata carry the SAME independent physical
        // metric/inner visitor as surfaces and coedges. Fan topology alone is
        // not a shape-error or occupied-material certificate.
        let mut accepted = true;
        let cycle = incidence.visit_vertex_link(
            source.definition(),
            vertex,
            work,
            cancel,
            &mut |use_, endpoint, work, cancel| {
                let domain = source.face_domain(use_.face, remaining, work, cancel)?;
                let coedge = &domain.coedges()[use_.coedge];
                let uv = coedge
                    .native_chart_jet([coedge.native_range[endpoint]; 2], work, cancel)?
                    .position;
                let surface = domain.surface_operand();
                let jet = surface_piecewise_first_enclosure(&surface, uv_box(uv), work, cancel)?;
                let normal = match source_chart::SphereChart::new(&surface, domain, work, cancel)? {
                    Some(sphere) => sphere.normal(jet.position)?,
                    None => first_normal(&jet, domain.orientation()),
                };
                accepted &= inventory.include(
                    BoundaryCell {
                        position: point,
                        native_face: None,
                    },
                    normal,
                    coordinates,
                    false,
                    work,
                    cancel,
                )? == BoundaryVisit::Accepted;
                Ok(())
            },
        )?;
        if !cycle || !accepted {
            return Ok(false);
        }
    }
    Ok(true)
}

fn cylinder_fiber(coordinates: &ActualMaterialCoordinates<'_>, alpha: f64) -> fiber::NativeFiber {
    let rows = coordinates.affine_rows();
    let eta = coordinates.center_offset_family();
    let a = Interval::point(alpha);
    fiber::NativeFiber {
        center: coordinates.source_anchor(),
        rows: [
            std::array::from_fn(|i| rows[1][i].sub(a.mul(rows[0][i]))),
            rows[2],
        ],
        offsets: [
            eta[1].sub(a.mul(eta[0])),
            coordinates
                .height_at_anchor()
                .sub(Interval::point(coordinates.sample_world_z())),
        ],
        forward: std::array::from_fn(|i| rows[0][i].add(a.mul(rows[1][i]))),
        forward_offset: eta[0].add(a.mul(eta[1])),
        min_forward: 0.0,
        core: None,
    }
}

pub(super) fn certify(
    source: MaterialSourceView<'_, '_>,
    request: &SlabRequest,
    coordinates: &ActualMaterialCoordinates<'_>,
    remaining: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    visitor: &mut BoundaryVisitor<'_>,
) -> Result<Option<RadialRegionKind>, GeometryError> {
    work.charge(1, cancel)?;
    let available = remaining.checked_sub(STACK_BYTES).ok_or_else(resource)?;
    let mut incidence = SourceEdgeIncidence::new(source.definition(), available, work, cancel)?;
    incidence.prepare_vertices(available, work, cancel)?;
    let available = available
        .checked_sub(incidence.owned_bytes())
        .ok_or_else(resource)?;
    let mut inventory = RadialInventory {
        visitor,
        outer_minimum: None,
        inner_maximum: None,
    };
    if !visit_native_faces(source, coordinates, available, &mut inventory, work, cancel)?
        || !visit_native_links(
            source,
            &incidence,
            coordinates,
            available,
            &mut inventory,
            work,
            cancel,
        )?
        || !inventory.ordered()
    {
        return Ok(None);
    }
    let height = height_functional(coordinates);
    let census_request = SlabRequest {
        frame: request.frame,
        height_interval_mm: coordinates.world_interval(),
        sample_height_mm: coordinates.sample_world_z(),
        native_shape_error_mm: request.native_shape_error_mm,
    };
    for alpha in [0.0, 1.0, -1.0, 0.5, -0.5, 2.0, -2.0, 0.2718281828459045] {
        let fiber = cylinder_fiber(coordinates, alpha);
        if !coverage::native_fiber_misses_vertex_supports(source, &incidence, &fiber, work, cancel)?
        {
            continue;
        }
        if let Some(census) = fiber::native_cylinder_fiber_census(
            source,
            &fiber,
            &census_request,
            height,
            available,
            work,
            cancel,
        )? {
            let annular = census.inner_forward().is_some();
            if annular != inventory.inner_maximum.is_some() {
                return Ok(None);
            }
            return Ok(Some(if annular {
                RadialRegionKind::Annulus
            } else {
                RadialRegionKind::Disk
            }));
        }
    }
    Ok(None)
}
