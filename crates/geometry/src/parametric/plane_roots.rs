// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Complete original-span boundary root isolation. Interval exclusion supplies
//! absence; strict endpoint signs or interval Newton inclusion supply existence.
//! Retained interior cells cover possible zeros and prove a nonzero gradient;
//! they are not sampled section curves or whole-material incidence authority.
use super::{Interval, PreparedValidatedCurve, analytic_eval::surface_order};
use crate::{GeometryError, GeometryWorkBudget};
use solidcore::{
    surface::surface::Surface,
    topology::{EdgeId, FaceId},
};
use spiling_contracts::geometry::{GeometryErrorCode, PlaneMm, SECTION_PLANE_TOLERANCE_MM};
use std::sync::atomic::AtomicBool;
pub const MAX_ROOT_SUBDIVISION_DEPTH: usize = 32;
const ZERO: Interval = Interval::point(0.0);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FacePlaneRelation {
    Excluded,
    CoplanarSameNormal,
    CoplanarOppositeNormal,
    Transverse,
}
#[derive(Clone, Debug)]
pub struct FacePlaneTrimRoot {
    pub coedge_index: usize,
    pub original_edge_id: EdgeId,
    pub parameter_range: [f64; 2],
    pub parameter_mid: f64,
    /// Image of the bound original UV curve over the certified root bracket.
    pub uv_box: [Interval; 2],
    pub position: [Interval; 3],
}
#[derive(Clone, Debug)]
pub struct BoundaryCoedgeRoots {
    pub coedge_index: usize,
    pub original_edge_id: EdgeId,
    pub roots: Vec<FacePlaneTrimRoot>,
}
#[derive(Clone, Debug)]
pub struct FacePlaneCell {
    pub u_range: [f64; 2],
    pub v_range: [f64; 2],
    pub distance: Interval,
    pub gradient: [Interval; 2],
    pub depth: u8,
}
pub struct PreparedBoundaryBinding<'a> {
    pub coedge_index: usize,
    pub original_edge_id: EdgeId,
    pub curve: &'a PreparedValidatedCurve<'a>,
    /// Original UV curve; a numerical point projection is not a substitute.
    pub chart: &'a PreparedValidatedCurve<'a>,
    pub parameter_range: [f64; 2],
    /// Literal t_chart = offset + slope*t_spatial, from source correspondence.
    pub chart_parameter_map: [f64; 2],
}
pub struct PreparedFacePlaneRoots<'a> {
    pub face_id: FaceId,
    pub surface: &'a Surface,
    pub plane: PlaneMm,
    pub relation: FacePlaneRelation,
    pub boundary_roots: Vec<Option<BoundaryCoedgeRoots>>,
    pub interior_cells: Vec<FacePlaneCell>,
    pub retained_bytes: usize,
}
fn unresolved(message: &str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::AnalysisIndeterminate, message)
}
fn resource() -> GeometryError {
    GeometryError::new(
        GeometryErrorCode::ResourceLimit,
        "native plane root live byte allowance exhausted",
    )
}
fn project(position: [Interval; 3], plane: &PlaneMm) -> Interval {
    (0..3).fold(ZERO, |s, i| {
        s.add(
            Interval::point(plane.normal[i])
                .mul(position[i].sub(Interval::point(plane.origin_mm[i]))),
        )
    })
}
fn project_vector(v: [Interval; 3], plane: &PlaneMm) -> Interval {
    (0..3).fold(ZERO, |s, i| {
        s.add(Interval::point(plane.normal[i]).mul(v[i]))
    })
}
fn push<T>(
    values: &mut Vec<T>,
    value: T,
    owned: &mut usize,
    limit: usize,
) -> Result<(), GeometryError> {
    if values.len() == values.capacity() {
        let size = std::mem::size_of::<T>();
        let slots = limit.checked_sub(*owned).ok_or_else(resource)? / size;
        if slots == 0 {
            return Err(resource());
        }
        let increment = values.capacity().max(1).min(slots);
        let old = values.capacity();
        values
            .try_reserve_exact(increment)
            .map_err(|_| resource())?;
        *owned = owned
            .checked_add(
                (values.capacity() - old)
                    .checked_mul(size)
                    .ok_or_else(resource)?,
            )
            .ok_or_else(resource)?;
        if *owned > limit {
            return Err(resource());
        }
    }
    values.push(value);
    Ok(())
}

impl<'a> PreparedFacePlaneRoots<'a> {
    pub const fn scratch_bytes() -> usize {
        std::mem::size_of::<[([f64; 2], u8); MAX_ROOT_SUBDIVISION_DEPTH + 2]>()
            + std::mem::size_of::<[([[f64; 2]; 2], u8); MAX_ROOT_SUBDIVISION_DEPTH + 2]>()
            + PreparedValidatedCurve::scratch_bytes()
            + super::nurbs_eval::SURFACE_EVALUATION_SCRATCH_BYTES
    }
    pub fn new(
        face_id: FaceId,
        surface: &'a Surface,
        face_orientation: bool,
        uv_domain: [[f64; 2]; 2],
        plane: &PlaneMm,
        boundaries: &[PreparedBoundaryBinding<'a>],
        max_retained_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        work.charge(1, cancel)?;
        plane.validate()?;
        let limit = max_retained_bytes
            .checked_sub(Self::scratch_bytes())
            .ok_or_else(resource)?;
        let mut owned = std::mem::size_of::<Self>();
        if owned > limit {
            return Err(resource());
        }
        let mut out = Self {
            face_id,
            surface,
            plane: *plane,
            relation: FacePlaneRelation::Excluded,
            boundary_roots: Vec::new(),
            interior_cells: Vec::new(),
            retained_bytes: 0,
        };
        let whole = surface_order(surface, uv_domain, 1, work, cancel)?;
        let distance = project(whole.position, plane);
        let gradient = [
            project_vector(whole.du, plane),
            project_vector(whole.dv, plane),
        ];
        // Exact constant zero, not a distance-tolerance promotion. The topology
        // owner still supplies the face-use orientation separately.
        let coplanar = distance == ZERO && gradient == [ZERO; 2];
        if coplanar {
            let dot = project_vector(super::cross(whole.du, whole.dv), plane)
                .mul(Interval::point(if face_orientation { 1.0 } else { -1.0 }));
            out.relation = if dot.lo > 0.0 {
                FacePlaneRelation::CoplanarSameNormal
            } else if dot.hi < 0.0 {
                FacePlaneRelation::CoplanarOppositeNormal
            } else {
                return Err(unresolved("coplanar chart orientation is singular"));
            };
        }
        if !distance.contains_zero() || coplanar {
            for _ in boundaries {
                work.charge(1, cancel)?;
                push(&mut out.boundary_roots, None, &mut owned, limit)?;
            }
            out.retained_bytes = owned;
            return Ok(out);
        }
        for b in boundaries {
            work.charge(1, cancel)?;
            if b.chart.scale() != 1.0 {
                return Err(unresolved(
                    "original UV chart must retain native parameter units",
                ));
            }
            for c in b.chart_parameter_map {
                Interval::new(c, c)?;
            }
            if b.chart_parameter_map[1] == 0.0 {
                return Err(unresolved("degenerate source chart correspondence"));
            }
            for value in b.parameter_range {
                Interval::new(value, value)?;
            }
            let full = [
                b.parameter_range[0].min(b.parameter_range[1]),
                b.parameter_range[0].max(b.parameter_range[1]),
            ];
            b.curve.position_bounds(full, work, cancel)?;
            let mut roots: Vec<FacePlaneTrimRoot> = Vec::new();
            for span in b.curve.parameter_intervals() {
                let a = span[0].max(full[0]);
                let z = span[1].min(full[1]);
                if a > z {
                    continue;
                }
                let mut stack = [([0.0; 2], 0u8); MAX_ROOT_SUBDIVISION_DEPTH + 2];
                stack[0] = ([a, z], 0);
                let mut live = 1;
                while live > 0 {
                    live -= 1;
                    let (mut range, depth) = stack[live];
                    let mut exists = false;
                    let mut accepted = false;
                    for _ in 0..=MAX_ROOT_SUBDIVISION_DEPTH {
                        work.charge(1, cancel)?;
                        let jet = b.curve.jet(range, work, cancel)?;
                        let f = project(jet.position, plane);
                        if !f.contains_zero() {
                            accepted = true;
                            break;
                        }
                        if !b.curve.is_smooth_on(range, work, cancel)? {
                            break;
                        }
                        let derivative = project_vector(jet.derivative, plane);
                        if derivative.contains_zero() {
                            break;
                        }
                        let left =
                            project(b.curve.position_bounds([range[0]; 2], work, cancel)?, plane);
                        let right =
                            project(b.curve.position_bounds([range[1]; 2], work, cancel)?, plane);
                        exists |= (left.hi < 0.0 && right.lo > 0.0)
                            || (left.lo > 0.0 && right.hi < 0.0)
                            || left == ZERO
                            || right == ZERO;
                        let mid = range[0] * 0.5 + range[1] * 0.5;
                        let fm = project(b.curve.position_bounds([mid; 2], work, cancel)?, plane);
                        let newton = Interval::point(mid).sub(fm.div(derivative)?);
                        exists |= newton.lo > range[0] && newton.hi < range[1];
                        let Some(contracted) =
                            newton.intersection(Interval::new(range[0], range[1])?)
                        else {
                            accepted = true;
                            break;
                        };
                        let diameter = jet
                            .position
                            .into_iter()
                            .map(Interval::width)
                            .fold(0.0_f64, f64::max);
                        if exists && diameter <= SECTION_PLANE_TOLERANCE_MM {
                            let t = Interval::new(range[0], range[1])?;
                            let mapped = Interval::point(b.chart_parameter_map[0])
                                .add(Interval::point(b.chart_parameter_map[1]).mul(t));
                            let chart =
                                b.chart
                                    .position_bounds([mapped.lo, mapped.hi], work, cancel)?;
                            let uv_box = [chart[0], chart[1]];
                            if (0..2).any(|axis| {
                                uv_box[axis].lo < uv_domain[axis][0]
                                    || uv_box[axis].hi > uv_domain[axis][1]
                            }) {
                                return Err(unresolved(
                                    "bound original root chart exceeds the declared face domain",
                                ));
                            }
                            if let Some(previous) = roots.last_mut() {
                                if range[0] <= previous.parameter_range[1] {
                                    let exact_same_parameter =
                                        range[0] == range[1] && previous.parameter_range == range;
                                    if !exact_same_parameter
                                        || project(
                                            b.curve.position_bounds([range[0]; 2], work, cancel)?,
                                            plane,
                                        ) != ZERO
                                    {
                                        return Err(unresolved(
                                            "overlapping original boundary root brackets have no proved unique branch identity",
                                        ));
                                    }
                                    // Both complete adjacent spans prove this same
                                    // literal source parameter is a root. Count the
                                    // intersection once, without losing either bound.
                                    for axis in 0..2 {
                                        previous.uv_box[axis] =
                                            previous.uv_box[axis].hull(uv_box[axis]);
                                    }
                                    for axis in 0..3 {
                                        previous.position[axis] =
                                            previous.position[axis].hull(jet.position[axis]);
                                    }
                                    accepted = true;
                                    break;
                                }
                            }
                            push(
                                &mut roots,
                                FacePlaneTrimRoot {
                                    coedge_index: b.coedge_index,
                                    original_edge_id: b.original_edge_id,
                                    parameter_range: range,
                                    parameter_mid: mid,
                                    uv_box,
                                    position: jet.position,
                                },
                                &mut owned,
                                limit,
                            )?;
                            accepted = true;
                            break;
                        }
                        if contracted.width() >= Interval::new(range[0], range[1])?.width() * 0.875
                        {
                            break;
                        }
                        range = [contracted.lo, contracted.hi];
                    }
                    if accepted {
                        continue;
                    }
                    if depth as usize >= MAX_ROOT_SUBDIVISION_DEPTH {
                        return Err(unresolved(
                            "complete original boundary root existence or uniqueness unresolved",
                        ));
                    }
                    let middle = range[0] * 0.5 + range[1] * 0.5;
                    if middle <= range[0] || middle >= range[1] {
                        return Err(unresolved(
                            "boundary root subdivision exhausted binary64 parameters",
                        ));
                    }
                    stack[live] = ([middle, range[1]], depth + 1);
                    stack[live + 1] = ([range[0], middle], depth + 1);
                    live += 2;
                }
            }
            let entry = if roots.is_empty() {
                None
            } else {
                Some(BoundaryCoedgeRoots {
                    coedge_index: b.coedge_index,
                    original_edge_id: b.original_edge_id,
                    roots,
                })
            };
            push(&mut out.boundary_roots, entry, &mut owned, limit)?;
        }
        // Complete original knot product. Each retained possible-zero cell has
        // a proved nonzero level gradient; unresolved critical cells refuse.
        for initial in super::surface_parameter_boxes(surface, uv_domain, work, cancel)? {
            let mut stack = [([[0.0; 2]; 2], 0u8); MAX_ROOT_SUBDIVISION_DEPTH + 2];
            stack[0] = (initial, 0);
            let mut live = 1;
            while live > 0 {
                live -= 1;
                let (cell, depth) = stack[live];
                work.charge(1, cancel)?;
                let jet = surface_order(surface, cell, 1, work, cancel)?;
                let dist = project(jet.position, plane);
                if !dist.contains_zero() {
                    continue;
                }
                let grad = [project_vector(jet.du, plane), project_vector(jet.dv, plane)];
                if grad.iter().any(|d| !d.contains_zero())
                    && super::surface_parameter_span_smooth(surface, cell, work, cancel)?
                {
                    push(
                        &mut out.interior_cells,
                        FacePlaneCell {
                            u_range: cell[0],
                            v_range: cell[1],
                            distance: dist,
                            gradient: grad,
                            depth,
                        },
                        &mut owned,
                        limit,
                    )?;
                    continue;
                }
                if depth as usize >= MAX_ROOT_SUBDIVISION_DEPTH {
                    return Err(unresolved(
                        "face-plane critical or knot incidence requires separate qualification",
                    ));
                }
                let axis = if cell[0][1] - cell[0][0] >= cell[1][1] - cell[1][0] {
                    0
                } else {
                    1
                };
                let mid = cell[axis][0] * 0.5 + cell[axis][1] * 0.5;
                if mid <= cell[axis][0] || mid >= cell[axis][1] {
                    return Err(unresolved(
                        "interior level subdivision exhausted binary64 parameters",
                    ));
                }
                let mut left = cell;
                let mut right = cell;
                left[axis][1] = mid;
                right[axis][0] = mid;
                stack[live] = (right, depth + 1);
                stack[live + 1] = (left, depth + 1);
                live += 2;
            }
        }
        out.relation =
            if out.interior_cells.is_empty() && out.boundary_roots.iter().all(Option::is_none) {
                FacePlaneRelation::Excluded
            } else {
                FacePlaneRelation::Transverse
            };
        out.retained_bytes = owned;
        Ok(out)
    }
    pub fn root_chart_for_cut(
        &self,
        coedge_index: usize,
        source_cut_parameter: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[Interval; 2], GeometryError> {
        work.charge(1, cancel)?;
        Interval::new(source_cut_parameter, source_cut_parameter)?;
        let mut chart: Option<[Interval; 2]> = None;
        for boundary in self.boundary_roots.iter().flatten() {
            if boundary.coedge_index != coedge_index {
                continue;
            }
            for root in &boundary.roots {
                work.charge(1, cancel)?;
                if root.parameter_range[0] <= source_cut_parameter
                    && source_cut_parameter <= root.parameter_range[1]
                {
                    chart = Some(match chart {
                        None => root.uv_box,
                        Some(old) => [old[0].hull(root.uv_box[0]), old[1].hull(root.uv_box[1])],
                    });
                }
            }
        }
        chart.ok_or_else(|| unresolved("source cut has no certified original boundary root chart"))
    }
    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }
    pub fn relation(&self) -> FacePlaneRelation {
        self.relation
    }
}
