// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! An ephemeral covering rectangle for inspection of source-unbound tensor seams.
//! Every branch is an exact native homogeneous row/knot correspondence. Choosing
//! a closed display rectangle does NOT select or restore an original PCURVE.
use super::{coverage, invalid};
use crate::{GeometryWorkBudget, kernel::Definition};
use monstertruck_modeling::{
    BoundedCurve, Curve, Curve2D, NativeSurfaceBoundary, ParametricCurve, Point3,
    NativeTrimCurve, PreparedFaceParameterDomain, Surface, TrimDomainError, TrimDomainLimits,
    ValidatedGeometryBudget, ValidatedGeometryError,
};
use monstertruck_topology::trimmed::TrimmedFace;
use spiling_contracts::geometry::{GeometryError, GeometryErrorCode};
use std::{mem::size_of, sync::atomic::AtomicBool};

type NativeFace = TrimmedFace<Point3, Curve, Surface, Curve2D>;

pub(super) struct DisplayFace<'source> {
    face: TrimmedFace<Point3, Curve, Surface, DisplayTrim<'source>>,
    bounds: [[f64; 2]; 2],
    retained_bytes: usize,
}
struct Candidate<'source> {
    curve: DisplayTrim<'source>,
    endpoints: [[f64; 2]; 2],
}

pub(super) enum DisplayTrim<'source> {
    Original(&'source Curve2D),
    NativeRow(Curve2D),
}
impl NativeTrimCurve for DisplayTrim<'_> {
    fn native_uv_curve(&self) -> &Curve2D {
        match self {
            Self::Original(curve) => curve,
            Self::NativeRow(curve) => curve,
        }
    }
}

impl<'source> DisplayFace<'source> {
    pub(super) fn face(&self) -> &TrimmedFace<Point3, Curve, Surface, DisplayTrim<'source>> {
        &self.face
    }
    pub(super) fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }
    pub(super) fn bounds(&self) -> [[f64; 2]; 2] {
        self.bounds
    }

    pub(super) fn prepare(
        definition: &Definition,
        ordinal: usize,
        original: &'source NativeFace,
        outside_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        let mut missing = false;
        for wire in original.trims() {
            work.charge(wire.len() as u64 + 1, cancel)?;
            missing |= wire.iter().any(Option::is_none);
        }
        if !missing {
            return Ok(None);
        }
        let identity = &definition.face_boundaries[ordinal];
        let Some(bounds) = identity.parameter.native_uv_domain else {
            return Err(unavailable("missing source seam has no original bounded tensor carrier"));
        };
        let [wire] = original.face().absolute_boundaries().as_slice() else {
            return Err(unavailable("display-only tensor chart requires one original boundary wire"));
        };
        if wire.is_empty() {
            return Err(invalid("display tensor chart has an empty original wire"));
        }
        let source = definition.source_topology.faces.get(ordinal)
            .filter(|face| face.wires.len() == 1)
            .map(|face| &face.wires[0])
            .ok_or_else(|| invalid("display face lost its original source wire"))?;
        if identity.face != original.face().id()
            || identity.wires.len() != 1
            || identity.wires[0].len() != wire.len()
            || source.coedges.len() != wire.len()
        {
            return Err(invalid("display face changed original boundary identity/layout"));
        }
        // Scope the face by an actual unbound original opposed seam. Ordinary
        // rim uses on that SAME complete wire can also lack derived charts;
        // every one still needs the exact native row/knot proof below.
        let mut unbound_seam = false;
        for (index, trim) in original.trims()[0].iter().enumerate() {
            work.charge(wire.len() as u64 + 1, cancel)?;
            let retained = &identity.wires[0][index];
            let original_use = &source.coedges[index];
            if retained.edge != wire[index].id()
                || retained.orientation != wire[index].orientation()
                || original_use.native_edge_index != retained.witness
                || original_use.native_orientation != retained.orientation
            {
                return Err(invalid("display coedge changed original source/native association"));
            }
            if trim.is_none() {
                if retained.source_chart_certificate.is_certified()
                    || retained.source_chart_declared
                    || original_use.source_edge.is_none()
                {
                    return Err(unavailable("missing display trim has no unproved original source use"));
                }
                unbound_seam |= source.coedges.iter().enumerate().any(|(other_index, other)| {
                    original_use.source_edge == other.source_edge
                        && original_use.source_coedge_index != other.source_coedge_index
                        && original_use.source_orientation != other.source_orientation
                        && original_use.restricted_curve_range == other.restricted_curve_range
                        && wire[other_index].id() == wire[index].id()
                        && wire[other_index].orientation() != wire[index].orientation()
                        && original.trims()[0][other_index].is_none()
                        && !identity.wires[0][other_index].source_chart_certificate.is_certified()
                        && !identity.wires[0][other_index].source_chart_declared
                });
            }
        }
        if !unbound_seam {
            return Err(unavailable("missing display chart has no original unbound opposed seam"));
        }
        // Native row extraction returns only a two-control affine spline and
        // at most 32 original restriction wrappers. Reserve that complete upper
        // bound before native allocation, including all four simultaneous choices
        // and the output/reference metadata while candidates are still live.
        let curve_bytes = 32 * size_of::<Curve2D>()
            + 4 * size_of::<f64>() + 2 * size_of::<monstertruck_modeling::Point2>();
        let retained_bytes = wire.len().saturating_mul(
            curve_bytes + size_of::<Option<DisplayTrim<'_>>>()
                + size_of::<monstertruck_modeling::Edge>(),
        ).saturating_add(size_of::<NativeFace>()
            + 2 * size_of::<Vec<Option<DisplayTrim<'_>>>>()
            + size_of::<monstertruck_modeling::Wire>());
        let candidate_bytes = wire.len().saturating_mul(
            4 * (curve_bytes + size_of::<Option<Candidate<'_>>>()) + 2 * size_of::<usize>(),
        ).saturating_add(3 * size_of::<Vec<usize>>());
        coverage::check_memory(outside_bytes.saturating_add(retained_bytes).saturating_add(candidate_bytes))?;
        work.charge(wire.len() as u64 + 3, cancel)?;
        let mut choices = Vec::with_capacity(wire.len());
        for (index, edge) in wire.iter().enumerate() {
            let mut candidates = [const { None }; 4];
            if let Some(chart) = original.trims()[0][index].as_ref() {
                // A retained expression is never replaced to obtain a simpler
                // display rectangle. Prove its complete native correspondence
                // independently, and then borrow the original unchanged.
                let certified = original.face().with_surface(|surface| edge.with_curve(|curve| {
                    crate::import::certify_display_native_chart(
                        curve, surface, chart, bounds, work, cancel,
                    )
                })).map_err(|cause| GeometryError::new(cause.code, format!(
                    "retained native-row proof coedge={index} source_edge={:?}: {}",
                    source.coedges[index].source_edge, cause.message,
                )))?;
                if !certified {
                    return Err(unavailable("retained trim lacks exact display tensor-row correspondence"));
                }
                work.charge(2, cancel)?;
                let (a, b) = chart.range_tuple();
                let mut endpoints = [chart.subs(a), chart.subs(b)].map(|p| [p.x, p.y]);
                if !edge.orientation() {
                    endpoints.swap(0, 1);
                }
                let side = [
                    (0, bounds[0][0]), (0, bounds[0][1]),
                    (1, bounds[1][0]), (1, bounds[1][1]),
                ].into_iter().position(|(axis, value)| {
                    endpoints.iter().all(|point| point[axis] == value)
                }).ok_or_else(|| unavailable("retained display trim is not a natural rectangle side"))?;
                candidates[side] = Some(Candidate {
                    curve: DisplayTrim::Original(chart), endpoints,
                });
                choices.push(candidates);
                continue;
            }
            for (side, boundary) in [
                NativeSurfaceBoundary::UFront,
                NativeSurfaceBoundary::UBack,
                NativeSurfaceBoundary::VFront,
                NativeSurfaceBoundary::VBack,
            ].into_iter().enumerate() {
                let mut native = ValidatedGeometryBudget::new(
                    usize::try_from(work.remaining()).unwrap_or(usize::MAX),
                );
                let result = original.face().with_surface(|surface| edge.with_curve(|curve| {
                    native_row(curve, surface, boundary, bounds, &mut native, cancel)
                }));
                crate::parametric::debit_native_work(work, native.consumed(), cancel)?;
                let curve = match result {
                    Ok(Some(curve)) => curve,
                    Ok(None) => continue,
                    Err(cause) => {
                        let cause = super::coverage::trim_error(cause);
                        return Err(GeometryError::new(cause.code, format!(
                            "exact native-row candidate coedge={index} source_edge={:?} boundary={boundary:?} native_work={}: {}",
                            source.coedges[index].source_edge, native.consumed(), cause.message,
                        )));
                    }
                };
                work.charge(2, cancel)?;
                let (a, b) = curve.range_tuple();
                // Endpoint evaluation only orders already coefficient-proved
                // candidates. Native preparation below independently certifies
                // all joints, full spans and original vertex/use incidence.
                let mut endpoints = [curve.subs(a), curve.subs(b)].map(|p| [p.x, p.y]);
                if !edge.orientation() {
                    endpoints.swap(0, 1);
                }
                if endpoints.iter().flatten().any(|v| !v.is_finite()) {
                    return Err(invalid("display tensor row has nonfinite endpoints"));
                }
                candidates[side] = Some(Candidate {
                    curve: DisplayTrim::NativeRow(curve), endpoints,
                });
            }
            choices.push(candidates);
        }
        // A complete natural rectangle, never a nearest branch or a partial
        // chart. Search is iterative and charged to the unchanged job allowance.
        let mut selected = vec![0_usize; wire.len()];
        let mut depth = 0;
        loop {
            work.charge(1, cancel)?;
            if depth == choices.len() {
                work.charge(selected.len() as u64, cancel)?;
                let first = choices[0][selected[0]].as_ref().unwrap();
                let last = choices[depth - 1][selected[depth - 1]].as_ref().unwrap();
                let sides = selected.iter().fold(0_u8, |mask, side| mask | (1 << side));
                if sides == 15 && first.endpoints[0] == last.endpoints[1] {
                    break;
                }
                depth -= 1;
                selected[depth] += 1;
                continue;
            }
            if selected[depth] == 4 {
                selected[depth] = 0;
                if depth == 0 {
                    return Err(unavailable("native rows do not form a complete ordered display rectangle"));
                }
                depth -= 1;
                selected[depth] += 1;
                continue;
            }
            if let Some(candidate) = &choices[depth][selected[depth]]
                && (depth == 0 || choices[depth - 1][selected[depth - 1]].as_ref().unwrap().endpoints[1] == candidate.endpoints[0])
            {
                depth += 1;
            } else {
                selected[depth] += 1;
            }
        }
        work.charge(wire.len() as u64 + 2, cancel)?;
        let trims = choices.into_iter().zip(selected).map(|(mut choices, index)| {
            Some(choices[index].take().unwrap().curve)
        }).collect();
        let face = TrimmedFace::try_new(original.face().clone(), vec![trims])
            .map_err(|_| invalid("display chart changed original coedge layout"))?;
        Ok(Some(Self { face, bounds, retained_bytes }))
    }

    pub(super) fn prepare_domain<'a>(
        &'a self,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<PreparedFaceParameterDomain<'a>, GeometryError> {
        let mut native = ValidatedGeometryBudget::new(
            usize::try_from(work.remaining()).unwrap_or(usize::MAX),
        );
        let result = monstertruck_modeling::prepare_face_parameter_domain(
            &self.face,
            TrimDomainLimits {
                max_coedges: 2 * spiling_contracts::geometry::MAX_STEP_ENTITIES as usize,
                max_subdivisions: 1_000_000,
                max_retained_bytes: remaining_bytes,
            },
            &mut native,
            cancel,
        ).and_then(|mut domain| {
            domain.restrict_parameter_domain(self.bounds, &mut native, cancel)?;
            Ok(domain)
        });
        crate::parametric::debit_native_work(work, native.consumed(), cancel)?;
        result.map_err(|cause| {
            let cause = super::coverage::trim_error(cause);
            GeometryError::new(cause.code, format!(
                "accepted display native trim-domain/joint preparation native_work={} available_bytes={remaining_bytes}: {}",
                native.consumed(), cause.message,
            ))
        })
    }
}

/// Exact original-knot subsets also cover native subdivisions of closed rows.
/// No de Boor cutting, sampled inverse, or implicit-branch search is involved.
fn native_row(
    curve: &Curve,
    surface: &Surface,
    boundary: NativeSurfaceBoundary,
    bounds: [[f64; 2]; 2],
    budget: &mut ValidatedGeometryBudget,
    cancel: &AtomicBool,
) -> Result<Option<Curve2D>, TrimDomainError> {
    let varying = match boundary {
        NativeSurfaceBoundary::UFront | NativeSurfaceBoundary::UBack => 1,
        _ => 0,
    };
    let knots = match surface {
        Surface::BsplineSurface(surface) => surface.knot_vectors(),
        Surface::NurbsSurface(surface) => surface.knot_vectors(),
        _ => return Ok(None),
    };
    let knots = if varying == 0 { &knots.0 } else { &knots.1 };
    let mut found = None;
    for (i, &a) in knots.iter().enumerate() {
        budget.charge(1, cancel)?;
        if a < bounds[varying][0] || a >= bounds[varying][1]
            || i > 0 && knots[i - 1] == a
        {
            continue;
        }
        for (j, &b) in knots.iter().enumerate().skip(i + 1) {
            budget.charge(1, cancel)?;
            if b <= a || b > bounds[varying][1] || j > 0 && knots[j - 1] == b {
                continue;
            }
            match monstertruck_modeling::certified_surface_boundary_parameter_curve(
                curve, surface, boundary, [a, b], budget, cancel,
            ) {
                Ok(chart) => {
                    if found.is_some() {
                        // Multiple exact spatial images do not determine one
                        // display interval. Do not silently select the first.
                        return Err(TrimDomainError::UnsupportedSeam);
                    }
                    found = Some(chart);
                }
                Err(TrimDomainError::Geometry(
                    ValidatedGeometryError::Unsupported | ValidatedGeometryError::Unresolved,
                )) => {}
                Err(cause) => return Err(cause),
            }
        }
    }
    Ok(found)
}

fn unavailable(message: &str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::ParameterCorrespondenceUnavailable, message)
}
