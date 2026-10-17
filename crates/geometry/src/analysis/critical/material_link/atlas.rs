// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Query-local evidence over the immutable material realization. Raw F boxes
//! over-cover the EXACT complement of the reference ribbons/corners; they are
//! not asserted to partition the actual remainder. No polygon participates.

use super::*;
use crate::analysis::placed::ActualMaterialCoordinates;
use super::radial_product::RadialRegionKind;

type Box3 = [Interval; 3];
const STACK_BYTES: usize = UV_EMBEDDING_STACK_BYTES
    + std::mem::size_of::<[([[f64; 2]; 2], u8); MAX_DEPTH as usize + 1]>()
    + std::mem::size_of::<[([f64; 2], u8); MAX_DEPTH as usize + 1]>()
    + std::mem::size_of::<RadialInventory>();

fn resource() -> GeometryError {
    GeometryError::new(GeometryErrorCode::ResourceLimit,
        "closed radial material atlas exhausts remaining scratch allowance")
}

fn outside(z: Interval, interval: [f64; 2]) -> bool {
    z.hi < interval[0] || z.lo > interval[1]
}

/// Reference ownership is distinct from inner-half actual-G ownership. This
/// receipt binds the pure embedding to the original raw domain, and therefore
/// supplies the complementary F half-neighborhood at w=1.
struct ReferenceRibbon<'a, 'd> {
    embedding: UvRibbonEmbedding<'a, 'd>,
}

impl ReferenceRibbon<'_, '_> {
    fn covers(&self, collar: &OriginalCollar<'_, '_>, range: [f64; 2]) -> bool {
        self.embedding.covers(collar, range, [0.0, 1.0])
    }
}

impl<'a, 'd> OriginalCollar<'a, 'd> {
    fn reference_owned(
        &self,
        range: [f64; 2],
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<ReferenceRibbon<'a, 'd>>, GeometryError> {
        let Some(embedding) = self.embedding(range, [0.0, 1.0], work, cancel)? else {
            return Ok(None);
        };
        if matches!(self.transport, CarrierTransport::Exact) {
            return Ok(Some(ReferenceRibbon { embedding }));
        }
        let normal = Interval { lo: 0.0, hi: self.bubble(range)?.0.hi };
        for coedge in 0..self.domain.native().coedges().len() {
            let correspondence = self.domain.original_coedge_correspondence(coedge)?;
            let whole = correspondence.native_range();
            let cuts = original_coedge_parameter_cuts(
                [(self.domain, coedge); 2], remaining_bytes, work, cancel,
            )?;
            let mut start = whole[0];
            for end in cuts.iter().copied().filter(|t| whole[0] < *t && *t < whole[1])
                .chain(std::iter::once(whole[1]))
            {
                let mut stack = [([0.0; 2], [0.0; 2], 0_u8); MAX_DEPTH as usize + 1];
                stack[0] = (range, [start, end], 0);
                start = end;
                let mut pending = 1;
                while pending != 0 {
                    pending -= 1;
                    let (a, b, depth) = stack[pending];
                    work.charge(1, cancel)?;
                    let image = self.normal_image(a, normal, work, cancel)?;
                    // Literal original coefficients and their ORIGINAL native
                    // parameter map, never a source/prepared-UV chart alias.
                    let raw = self.domain.original_coedge_uv_piecewise_jet(coedge, b, work, cancel)?;
                    if (0..2).any(|i| image[i].hi < raw.position[i].lo
                        || raw.position[i].hi < image[i].lo)
                    {
                        continue;
                    }
                    if coedge == self.use_.coedge {
                        let hull = [a[0].min(b[0]), a[1].max(b[1])];
                        if self.domain.original_coedge_uv_is_c1(coedge, hull, work, cancel)?
                            && self.normal_chart(hull, normal, work, cancel)?
                        {
                            // On this common injective (t,r) chart, equality
                            // with gamma(s)=U(s,0) forces r=0,t=s. Thus the
                            // ENTIRE open ribbon avoids its owning boundary,
                            // including unused pieces, without demanding false
                            // positive clearance at its exact shared trace.
                            continue;
                        }
                    }
                    if depth >= MAX_DEPTH {
                        return Ok(None);
                    }
                    let a_fraction = (a[1] - a[0]) / (range[1] - range[0]);
                    let b_fraction = (b[1] - b[0]) / (end - stack_span_start(b, whole, &cuts));
                    let split_a = a_fraction >= b_fraction;
                    let selected = if split_a { a } else { b };
                    let mid = selected[0] * 0.5 + selected[1] * 0.5;
                    if mid <= selected[0] || mid >= selected[1] {
                        return Ok(None);
                    }
                    if pending + 2 > stack.len() {
                        return Err(resource());
                    }
                    if split_a {
                        stack[pending] = ([mid, a[1]], b, depth + 1);
                        stack[pending + 1] = ([a[0], mid], b, depth + 1);
                    } else {
                        stack[pending] = (a, [mid, b[1]], depth + 1);
                        stack[pending + 1] = (a, [b[0], mid], depth + 1);
                    }
                    pending += 2;
                }
            }
        }
        // Native joint/completion images are not ordinary original coedges.
        // Exclude every one explicitly from the full reference ribbon.
        for joint in self.domain.native().joints() {
            let joint = joint.enclosure.map(|v| Interval { lo: v.lower, hi: v.upper });
            let mut stack = [([0.0; 2], 0_u8); MAX_DEPTH as usize + 1];
            stack[0] = (range, 0);
            let mut pending = 1;
            while pending != 0 {
                pending -= 1;
                let (a, depth) = stack[pending];
                work.charge(1, cancel)?;
                let image = self.normal_image(a, normal, work, cancel)?;
                if (0..2).any(|i| image[i].hi < joint[i].lo || joint[i].hi < image[i].lo) {
                    continue;
                }
                let mid = a[0] * 0.5 + a[1] * 0.5;
                if depth >= MAX_DEPTH || mid <= a[0] || mid >= a[1] {
                    return Ok(None);
                }
                stack[pending] = ([mid, a[1]], depth + 1);
                stack[pending + 1] = ([a[0], mid], depth + 1);
                pending += 2;
            }
        }
        // Every open reference point is boundary-free. A single strict Inside
        // calibration, including retained source/native uncertainty, fixes its
        // winding on the COMPLETE connected open ribbon.
        let t = range[0] * 0.5 + range[1] * 0.5;
        let beta = self.bubble([t; 2])?.0;
        let point = self.normal_image([t; 2], mul(beta, Interval::point(0.75)), work, cancel)?;
        let inclusive = remaining_bytes.checked_add(self.domain.retained_bytes()).ok_or_else(resource)?
            .min(MAX_SCRATCH_BYTES);
        if self.domain.classify_with_byte_limit(
            uv_box(add_margin(point, self.margin)), inclusive, work, cancel,
        )? != TrimClassification::Inside {
            return Ok(None);
        }
        Ok(Some(ReferenceRibbon { embedding }))
    }
}

// The original unsplit span's left endpoint, used only to balance a bounded
// pair cover. This selects work order, never a geometric exclusion.
fn stack_span_start(span: [f64; 2], whole: [f64; 2], cuts: &[f64]) -> f64 {
    cuts.iter().copied().filter(|x| whole[0] <= *x && *x <= span[0])
        .last().unwrap_or(whole[0])
}

#[derive(Default)]
struct RadialInventory {
    outer_minimum: Option<f64>,
    inner_maximum: Option<f64>,
    outer_cells: u64,
    inner_cells: u64,
}

impl RadialInventory {
    /// The cross product is formed from an interval FIRST-derivative matrix,
    /// not a hull of endpoint normals. It therefore includes every convex
    /// combination of one-sided matrices at native knots and intersections.
    fn include(
        &mut self,
        position: Box3,
        normal: Box3,
        coordinates: &ActualMaterialCoordinates<'_>,
    ) -> Result<bool, GeometryError> {
        let q = coordinates.position_relative_to_center_family(position)?;
        let radius = add(square(q[0]), square(q[1]));
        if radius.lo <= 0.0 {
            return Ok(false);
        }
        let n = coordinates.normal(normal)?;
        let determinant = add(mul(n[0], q[0]), mul(n[1], q[1]));
        if determinant.lo > 0.0 {
            self.outer_minimum = Some(self.outer_minimum.map_or(radius.lo, |r| r.min(radius.lo)));
            self.outer_cells += 1;
        } else if determinant.hi < 0.0 {
            self.inner_maximum = Some(self.inner_maximum.map_or(radius.hi, |r| r.max(radius.hi)));
            self.inner_cells += 1;
        } else {
            return Ok(false);
        }
        Ok(true)
    }

    fn ordered(&self) -> bool {
        match (self.inner_maximum, self.outer_minimum) {
            (Some(inner), Some(outer)) => inner < outer,
            (None, Some(_)) => true,
            _ => false,
        }
    }
}

fn visit_original_faces(
    source: OriginalTrimSource<'_, '_>,
    coordinates: &ActualMaterialCoordinates<'_>,
    remaining_bytes: usize,
    inventory: &mut RadialInventory,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let interval = coordinates.world_interval();
    for (fi, _) in source.definition().solid.face_iter().enumerate() {
        let domain = source.face_domain(fi, remaining_bytes, work, cancel)?;
        let face = domain.native().face();
        let completed = face.with_surface(|surface| -> Result<bool, GeometryError> {
            // A successful piecewise evaluation admits continuity/Lipschitzness
            // on the WHOLE closed native domain. Discontinuous knots and nested
            // carrier poles are real Native errors, not silently split away.
            crate::parametric::surface_piecewise_enclosure(surface, domain.uv_bounds(), work, cancel)?;
            for joint in domain.native().joints() {
                work.charge(1, cancel)?;
                let uv = joint.enclosure.map(|v| [v.lower, v.upper]);
                let position = crate::parametric::surface_piecewise_enclosure(surface, uv, work, cancel)?.position;
                if !outside(coordinates.position(position)?[2], interval) {
                    return Ok(false);
                }
            }
            for span in surface_parameter_boxes(surface, domain.uv_bounds(), work, cancel)? {
                let mut stack = [([[0.0; 2]; 2], 0_u8); MAX_DEPTH as usize + 1];
                stack[0] = (span, 0);
                let mut pending = 1;
                while pending != 0 {
                    pending -= 1;
                    let (uv, depth) = stack[pending];
                    work.charge(1, cancel)?;
                    let jet = crate::parametric::surface_piecewise_enclosure(surface, uv, work, cancel)?;
                    if outside(coordinates.position(jet.position)?[2], interval)
                        || domain.classify(uv, work, cancel)? == TrimClassification::Outside
                    {
                        continue;
                    }
                    if inventory.include(jet.position, normal(&jet, face.orientation()), coordinates)? {
                        continue;
                    }
                    if depth >= MAX_DEPTH {
                        return Ok(false);
                    }
                    let children = match split(uv, depth, fi) {
                        Ok(children) => children,
                        Err(error) if error.code == GeometryErrorCode::AnalysisIndeterminate => return Ok(false),
                        Err(error) => return Err(error),
                    };
                    if pending + children.len() > stack.len() {
                        return Err(resource());
                    }
                    for child in children {
                        stack[pending] = (child, depth + 1);
                        pending += 1;
                    }
                }
            }
            Ok(true)
        })?;
        if !completed {
            return Ok(false);
        }
    }
    Ok(true)
}

#[allow(clippy::too_many_arguments)]
fn visit_original_links(
    source: OriginalTrimSource<'_, '_>,
    incidence: &SourceEdgeIncidence,
    coordinates: &ActualMaterialCoordinates<'_>,
    links: &mut ClosedRadialLinks,
    remaining_bytes: usize,
    inventory: &mut RadialInventory,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let allowance = source_allowance(source.definition())?;
    let interval = coordinates.world_interval();
    for uses in incidence.values() {
        work.charge(1, cancel)?;
        let Some(second) = uses.second else { return Ok(false); };
        if uses.first.orientation == second.orientation {
            return Ok(false);
        }
        for use_ in [uses.first, second] {
            let available = remaining_bytes.checked_sub(links.allocated_bytes()).ok_or_else(resource)?;
            let domain = source.face_domain(use_.face, available, work, cancel)?;
            let available = available.checked_sub(domain.owned_bytes()).ok_or_else(resource)?;
            let Some(collar) = OriginalCollar::for_enclosure(&domain, use_, work, cancel)? else {
                return Ok(false);
            };
            let face = domain.native().face();
            let edge = &face.absolute_boundaries()[use_.wire][use_.index];
            let correspondence = domain.original_coedge_correspondence(use_.coedge)?;
            let central = links.central_native_range(edge.id(), correspondence.native_range(),
                [use_.face, use_.wire, use_.index])?;
            if !domain.original_coedge_uv_is_c1(use_.coedge, central, work, cancel)? {
                // A true C0 gamma makes U=gamma+k*beta*w*Jgamma' discontinuous.
                // There is no canonical knot-joint completion in this lane.
                return Ok(false);
            }
            let Some(reference) = collar.reference_owned(central, available, work, cancel)? else {
                return Ok(false);
            };
            if !reference.covers(&collar, central) {
                return Err(GeometryError::new(GeometryErrorCode::KernelFailure,
                    "reference ribbon lost its exact source/map/across binding"));
            }
            let Some(embedding) = collar.owned_embedding(central, available, work, cancel)? else {
                return Ok(false);
            };
            let cuts = original_coedge_parameter_cuts([(&domain, use_.coedge); 2], available, work, cancel)?;
            let cut_bytes = cuts.capacity().checked_mul(std::mem::size_of::<f64>()).ok_or_else(resource)?;
            let available = available.checked_sub(cut_bytes).ok_or_else(resource)?;
            let completed = domain.with_original_spatial_curve(use_.coedge, available, work, cancel,
                |prepared, work, cancel| -> Result<bool, GeometryError> {
                // Native's piecewise curve evaluator admits continuous C3D and
                // all bounded one-sided jets, including closed knot endpoints.
                prepared.jet((central[0], central[1]), work, cancel)?;
                let mut start = central[0];
                for end in cuts.iter().copied().filter(|t| central[0] < *t && *t < central[1])
                    .chain(std::iter::once(central[1]))
                {
                    let mut stack = [([0.0; 2], 0_u8); MAX_DEPTH as usize + 1];
                    stack[0] = ([start, end], 0);
                    start = end;
                    let mut pending = 1;
                    while pending != 0 {
                        pending -= 1;
                        let (range, depth) = stack[pending];
                        work.charge(1, cancel)?;
                        let curve = prepared.jet((range[0], range[1]), work, cancel)?;
                        let support = collar.swept_enclosure(range, &curve, allowance, work, cancel)?;
                        if outside(coordinates.position(support)?[2], interval) {
                            continue;
                        }
                        let cell = if matches!(collar.transport, CarrierTransport::Exact) {
                            let uv = chart_jet(&domain, use_.coedge, range, work, cancel)?;
                            let jet = face.with_surface(|surface|
                                crate::parametric::surface_piecewise_enclosure(surface,
                                    uv_box(add_margin(uv.position, collar.margin)), work, cancel))?;
                            Some(MaterialLinkCell {
                                position: curve.position,
                                source_error_mm: 0.0,
                                convex_link: false,
                                outward: normal(&jet, face.orientation()),
                                second_outward: normal(&jet, face.orientation()),
                            })
                        } else {
                            collar.geometry_cell_proof_kind(&embedding, range, &curve, allowance,
                                true, work, cancel)?.ok()
                        };
                        if let Some(cell) = cell {
                            if inventory.include(cell.position, cell.outward, coordinates)? {
                                // This is the FULL lambda support, containing
                                // removed reference F and added actual G, not
                                // merely a tube around the realized seam.
                                links.record(&cell, work, cancel)?;
                                if links.allocated_bytes().saturating_add(cut_bytes)
                                    .saturating_add(domain.owned_bytes())
                                    .saturating_add(prepared.retained_bytes()) > remaining_bytes
                                { return Err(resource()); }
                                continue;
                            }
                        }
                        let mid = range[0] * 0.5 + range[1] * 0.5;
                        if depth >= MAX_DEPTH || mid <= range[0] || mid >= range[1] {
                            return Ok(false);
                        }
                        stack[pending] = ([mid, range[1]], depth + 1);
                        stack[pending + 1] = ([range[0], mid], depth + 1);
                        pending += 2;
                    }
                }
                Ok(true)
            })?;
            if !completed { return Ok(false); }
        }
    }
    Ok(true)
}

pub(super) fn certify(
    source: OriginalTrimSource<'_, '_>,
    request: &SlabRequest,
    coordinates: &ActualMaterialCoordinates<'_>,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<RadialRegionKind>, GeometryError> {
    work.charge(1, cancel)?;
    let available = remaining_bytes.checked_sub(STACK_BYTES).ok_or_else(resource)?;
    let originals = source.prepare_domains_if_missing(available, work, cancel)?;
    let available = available.checked_sub(originals.as_ref().map_or(0, |owner| owner.retained_bytes()))
        .ok_or_else(resource)?;
    let source = source.borrowing_domains(originals.as_ref());
    let mut incidence = SourceEdgeIncidence::new(source.definition(), available, work, cancel)?;
    incidence.prepare_vertices(available, work, cancel)?;
    let available = available.checked_sub(incidence.owned_bytes()).ok_or_else(resource)?;
    let Some(mut links) = ClosedRadialLinks::outside_closed_scope(
        source, &incidence, coordinates, available, work, cancel,
    )? else { return Ok(None); };
    let mut inventory = RadialInventory::default();
    if !visit_original_faces(source, coordinates,
        available.checked_sub(links.allocated_bytes()).ok_or_else(resource)?,
        &mut inventory, work, cancel)?
        || !visit_original_links(source, &incidence, coordinates, &mut links,
            available, &mut inventory, work, cancel)?
        || !inventory.ordered()
    { return Ok(None); }
    let anchor = coordinates.source_anchor();
    let rows = coordinates.affine_rows();
    let offset = coordinates.center_offset_family();
    let interval = coordinates.world_interval();
    let sample = coordinates.sample_world_z();
    let height = HeightFunctional {
        direction: rows[2],
        offset: coordinates.height_at_anchor().sub(dot(rows[2], anchor.map(Interval::point))),
    };
    let census_request = SlabRequest {
        frame: request.frame,
        height_interval_mm: interval,
        sample_height_mm: sample,
        native_shape_error_mm: request.native_shape_error_mm,
    };
    for alpha in [0.0, 1.0, -1.0, 0.5, -0.5, 2.0, -2.0, 0.2718281828459045] {
        work.charge(1, cancel)?;
        let a = Interval::point(alpha);
        let angular = std::array::from_fn(|i| rows[1][i].sub(mul(a, rows[0][i])));
        let forward = std::array::from_fn(|i| add(rows[0][i], mul(a, rows[1][i])));
        let fiber = fiber::OriginalFiber {
            center: anchor,
            rows: [angular, rows[2]],
            offsets: [offset[1].sub(mul(a, offset[0])),
                coordinates.height_at_anchor().sub(Interval::point(sample))],
            forward,
            forward_offset: add(offset[0], mul(a, offset[1])),
            min_forward: 0.0,
            core: None,
        };
        if !links.misses_all_supports(&fiber, work, cancel)? { continue; }
        if let Some(census) = fiber::original_cylinder_fiber_census(source, &fiber, &census_request,
            height, available.checked_sub(links.allocated_bytes()).ok_or_else(resource)?, work, cancel)?
        {
            let kind = if census.inner_forward().is_some() {
                RadialRegionKind::Annulus
            } else { RadialRegionKind::Disk };
            if (kind == RadialRegionKind::Annulus) != inventory.inner_maximum.is_some() {
                return Ok(None);
            }
            // Complete compact glued charts, strict closed-cut regularity and
            // the exhaustive SAME physical cylinder fibre give proper local
            // homeomorphisms of total degree one for every role. The full
            // boundary inventory and outward material crossing then fill the
            // disk, or the strictly ordered interval between the two graphs.
            return Ok(Some(kind));
        }
    }
    Ok(None)
}
