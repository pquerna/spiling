// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::super::*;
use crate::analysis::material_source::MaterialSourceView;
use crate::parametric::{
    SurfaceEvaluator, SurfaceFunctional, SurfaceFunctionalEnclosure, surface_functional_enclosures,
    surface_functional_scratch_bytes,
};

/// Optional outer-meridian restriction in the actual stored projection frame.
#[derive(Clone, Copy, Debug)]
pub(super) struct FiberCore {
    pub(super) frame: PlaneFrameMm,
    pub(super) radius: f64,
}

/// A coefficient family of the two affine equations
/// `rows[i].(S-center) + offsets[i] = 0`. The intended exact symbolic fiber
/// must belong to this enclosure. Every exclusion, root inclusion and normal
/// sign holds uniformly over ALL family members, not a midpoint fiber.
/// Disk fibers use a positive forward COVECTOR, strictly increasing along
/// `cross(rows[0], rows[1])`; it need not be that tangent under an affine map.
/// Outer-meridian fibers additionally require `px > 0` AND `rho > radius`.
#[derive(Clone, Copy, Debug)]
pub(super) struct OriginalFiber {
    pub(super) center: [f64; 3],
    pub(super) rows: [[Interval; 3]; 2],
    pub(super) offsets: [Interval; 2],
    pub(super) forward: [Interval; 3],
    pub(super) forward_offset: Interval,
    pub(super) min_forward: f64,
    pub(super) core: Option<FiberCore>,
}

/// Exactly one outward-positive root on the complete original trimmed source.
/// This does NOT certify that correction collars, vertex supports or cut caps
/// miss the fiber. The material-degree consumer must separately prove those
/// gates for this identical fiber and closed request before using the receipt.
pub(super) struct FiberDegreeOne;

/// A complete ORIGINAL-source cylinder-fiber calibration, not an atlas or
/// actual-material receipt. Consumers must independently exclude BOTH removed
/// source and added realization supports from this identical fiber.
pub(super) struct CylinderFiberCensus {
    #[cfg(test)]
    outer_forward: Interval,
    inner_forward: Option<Interval>,
}

impl CylinderFiberCensus {
    #[cfg(test)]
    pub(super) fn outer_forward(&self) -> Interval {
        self.outer_forward
    }

    pub(super) fn inner_forward(&self) -> Option<Interval> {
        self.inner_forward
    }
}

#[derive(Default)]
struct FiberRoots {
    outward: Option<Interval>,
    inward: Option<Interval>,
}

impl FiberRoots {
    fn record(&mut self, inward: bool, forward: Interval) -> bool {
        let slot = if inward {
            &mut self.inward
        } else {
            &mut self.outward
        };
        if slot.is_some() {
            return false;
        }
        *slot = Some(forward);
        self.inward
            .zip(self.outward)
            .is_none_or(|(inner, outer)| inner.hi < outer.lo)
    }
}

#[derive(Clone, Copy)]
struct CensusRestriction<'a> {
    height: HeightFunctional,
    request: &'a SlabRequest,
    faces: Option<&'a [usize]>,
    inward: Option<bool>,
}

#[derive(Clone, Copy)]
struct FiberCell {
    uv: [[f64; 2]; 2],
    depth: u8,
    trim_inside: bool,
}

enum FiberSearchJet {
    Affine([SurfaceFunctionalEnclosure; 4]),
    Spatial(SurfaceEnclosure),
}

impl FiberSearchJet {
    fn excluded(&self, fiber: &OriginalFiber, restriction: CensusRestriction<'_>) -> bool {
        match self {
            Self::Affine(jets) => {
                jets[..2].iter().any(|jet| !jet.position.contains_zero())
                    || jets[2].position.hi < fiber.min_forward
                    || !overlaps(jets[3].position, restriction.request.height_interval_mm)
            }
            Self::Spatial(jet) => {
                fiber_image_excluded(fiber, jet.position, restriction.height, restriction.request)
            }
        }
    }

    fn jacobian(&self, fiber: &OriginalFiber) -> [[Interval; 2]; 2] {
        match self {
            Self::Affine(jets) => std::array::from_fn(|i| [jets[i].du, jets[i].dv]),
            Self::Spatial(jet) => fiber.rows.map(|row| [dot(row, jet.du), dot(row, jet.dv)]),
        }
    }
}

fn fiber_equations(fiber: &OriginalFiber) -> [SurfaceFunctional; 2] {
    std::array::from_fn(|i| SurfaceFunctional {
        direction: fiber.rows[i],
        origin: fiber.center.map(Interval::point),
        offset: fiber.offsets[i],
    })
}

fn fiber_search_jet(
    surface: &(impl SurfaceEvaluator + ?Sized),
    uv: [[f64; 2]; 2],
    smooth: bool,
    fiber: &OriginalFiber,
    height: HeightFunctional,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<FiberSearchJet, GeometryError> {
    if fiber.core.is_some() {
        // The nonlinear meridian restriction still needs its spatial image.
        let jet = if smooth {
            surface_enclosure(surface, uv, work, cancel)?
        } else {
            crate::parametric::surface_piecewise_enclosure(surface, uv, work, cancel)?
        };
        return Ok(FiberSearchJet::Spatial(jet));
    }
    let [first, second] = fiber_equations(fiber);
    let jets = surface_functional_enclosures(
        surface,
        uv,
        &[
            first,
            second,
            SurfaceFunctional {
                direction: fiber.forward,
                origin: fiber.center.map(Interval::point),
                offset: fiber.forward_offset,
            },
            SurfaceFunctional {
                direction: height.direction,
                origin: [Interval::point(0.0); 3],
                offset: height.offset,
            },
        ],
        work,
        cancel,
    )?;
    Ok(FiberSearchJet::Affine(jets))
}

const FIBER_STACK_CAPACITY: usize = MAX_DEPTH as usize + 1;
const FIBER_STACK_BYTES: usize = std::mem::size_of::<[Option<FiberCell>; FIBER_STACK_CAPACITY]>()
    + std::mem::size_of::<FiberRoots>()
    + std::mem::size_of::<FiberSearchJet>()
    + crate::kernel::source_operand::SOURCE_OPERAND_SCRATCH_BYTES
    + surface_functional_scratch_bytes::<4>()
    + surface_functional_scratch_bytes::<2>()
    + std::mem::size_of::<[SurfaceFunctional; 4]>()
    + std::mem::size_of::<[SurfaceFunctionalEnclosure; 2]>();

/// Census every original face and every original parameter span. Whole-image
/// exclusions and strictly owned interval roots form a complete cover. No root
/// is selected by a sampled point or merged by physical nearness. A root on a
/// chart, trim, split, knot, half-ray or slab boundary remains unresolved.
pub(super) fn original_fiber_degree_one(
    source: MaterialSourceView<'_, '_>,
    fiber: &OriginalFiber,
    request: &SlabRequest,
    height: HeightFunctional,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<FiberDegreeOne>, GeometryError> {
    work.charge(1, cancel)?;
    validate_fiber(fiber)?;
    let restriction = CensusRestriction {
        height,
        request,
        faces: None,
        inward: Some(false),
    };
    Ok(
        census_original_fiber(source, fiber, restriction, remaining_bytes, work, cancel)?
            .map(|_| FiberDegreeOne),
    )
}

/// Calibrate the COMPLETE declared wall component, with material normals
/// pointing into its bounded void. The caller must separately prove complete
/// correction-support exclusion for this identical physical fiber.
#[allow(clippy::too_many_arguments)]
pub(super) fn original_wall_fiber_degree_one(
    source: MaterialSourceView<'_, '_>,
    faces: &[usize],
    fiber: &OriginalFiber,
    request: &SlabRequest,
    height: HeightFunctional,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<FiberDegreeOne>, GeometryError> {
    work.charge(faces.len() as u64, cancel)?;
    if faces.is_empty()
        || faces
            .iter()
            .any(|&face| face >= source.definition().face_ids().len())
        || faces
            .iter()
            .enumerate()
            .any(|(i, face)| faces[..i].contains(face))
    {
        return Err(GeometryError::new(
            GeometryErrorCode::InvalidGeometry,
            "wall fiber needs a nonempty unique original face inventory",
        ));
    }
    validate_fiber(fiber)?;
    let restriction = CensusRestriction {
        height,
        request,
        faces: Some(faces),
        inward: Some(true),
    };
    Ok(
        census_original_fiber(source, fiber, restriction, remaining_bytes, work, cancel)?
            .map(|_| FiberDegreeOne),
    )
}

/// Census ALL original parameter components, with roles assigned per qualified
/// physical root rather than per face. An extra outward/inner sheet, reversed
/// ordering, a tangent root or any unresolved owner refuses this calibration.
/// In particular a single carrier may supply both the inner and outer root.
#[allow(clippy::too_many_arguments)]
pub(super) fn original_cylinder_fiber_census(
    source: MaterialSourceView<'_, '_>,
    fiber: &OriginalFiber,
    request: &SlabRequest,
    height: HeightFunctional,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<CylinderFiberCensus>, GeometryError> {
    work.charge(1, cancel)?;
    validate_fiber(fiber)?;
    if fiber.core.is_some() {
        return Err(GeometryError::new(
            GeometryErrorCode::InvalidGeometry,
            "cylinder fiber census cannot omit an inner radial core",
        ));
    }
    let restriction = CensusRestriction {
        height,
        request,
        faces: None,
        inward: None,
    };
    Ok(
        census_original_fiber(source, fiber, restriction, remaining_bytes, work, cancel)?.and_then(
            |roots| {
                roots.outward.map(|_outer_forward| CylinderFiberCensus {
                    #[cfg(test)]
                    outer_forward: _outer_forward,
                    inner_forward: roots.inward,
                })
            },
        ),
    )
}

fn census_original_fiber(
    source: MaterialSourceView<'_, '_>,
    fiber: &OriginalFiber,
    restriction: CensusRestriction<'_>,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<FiberRoots>, GeometryError> {
    let domain_bytes = remaining_bytes
        .checked_sub(FIBER_STACK_BYTES)
        .ok_or_else(|| {
            GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "original fiber census stack exceeds remaining scratch allowance",
            )
        })?;
    let definition = source.definition();
    let mut roots = FiberRoots::default();
    let mut stack = [None; FIBER_STACK_CAPACITY];
    for (face_index, _) in definition.face_ids().iter().enumerate() {
        work.charge(1, cancel)?;
        if restriction
            .faces
            .is_some_and(|faces| !faces.contains(&face_index))
        {
            continue;
        }
        let domain = source.face_domain(face_index, domain_bytes, work, cancel)?;
        let surface = &domain.surface_operand();
        let orientation = domain.orientation();
        let before = work.consumed();
        let mut phase = "original parameter spans";
        let mut active_uv = domain.uv_domain();
        let mut active_depth = 0;
        let resolved = (|| -> Result<bool, GeometryError> {
            let spans = surface_parameter_boxes(surface, domain.uv_domain(), work, cancel)?;
            for span in spans {
                active_uv = span;
                active_depth = 0;
                phase = "whole original span regularity";
                work.charge(1, cancel)?;
                // Only whole-position exclusions are valid on an unqualified
                // original C0 span. Subdivision cannot manufacture C1 authority.
                let smooth = surface_parameter_span_smooth(surface, span, work, cancel)?;
                let mut pending = 1_usize;
                stack[0] = Some(FiberCell {
                    uv: span,
                    depth: 0,
                    trim_inside: false,
                });
                while pending != 0 {
                    work.charge(1, cancel)?;
                    pending -= 1;
                    let cell = stack[pending].take().ok_or_else(|| {
                        GeometryError::new(
                            GeometryErrorCode::KernelFailure,
                            "original fiber census lost its bounded cover stack",
                        )
                    })?;
                    active_uv = cell.uv;
                    active_depth = cell.depth;
                    phase = "original root position/first matrix";
                    // Affine fiber equations and their native first matrix
                    // need no full spatial/Hessian jet on every search cell.
                    // Original C1 qualification remains mandatory below.
                    let jet = fiber_search_jet(
                        surface,
                        cell.uv,
                        smooth,
                        fiber,
                        restriction.height,
                        work,
                        cancel,
                    );
                    let jet = match jet {
                        Ok(jet) => jet,
                        Err(error)
                            if matches!(
                                error.code,
                                GeometryErrorCode::ResourceLimit | GeometryErrorCode::Cancelled
                            ) =>
                        {
                            return Err(error);
                        }
                        Err(error) => {
                            // Preserve the former trim-first behavior for a
                            // carrier pole/interface wholly outside original F.
                            // Resource/cancellation failures are never retried
                            // or discarded by this optional exclusion query.
                            phase = "failed original carrier trim exclusion";
                            let classification = domain.classify_with_byte_limit(
                                cell.uv,
                                domain_bytes,
                                work,
                                cancel,
                            );
                            if classification? == TrimClassification::Outside {
                                continue;
                            }
                            return Err(error);
                        }
                    };
                    if jet.excluded(fiber, restriction) {
                        continue;
                    }
                    // A complete carrier image also encloses its entire
                    // trimmed subset. Exclude it before asking winding about
                    // curved trims which cannot contribute a physical root.
                    // An Inside proof is inherited only by subset children.
                    let mut trim_inside = cell.trim_inside;
                    if !trim_inside {
                        phase = "original root trim classification";
                        let classification =
                            domain.classify_with_byte_limit(cell.uv, domain_bytes, work, cancel);
                        match classification? {
                            TrimClassification::Outside => continue,
                            TrimClassification::Inside => trim_inside = true,
                            TrimClassification::Boundary => {}
                        }
                    }
                    if !smooth {
                        return Ok(false);
                    }
                    phase = "uniform affine-family Krawczyk root";
                    let root = fiber_krawczyk_jacobian(
                        surface,
                        cell.uv,
                        jet.jacobian(fiber),
                        fiber,
                        work,
                        cancel,
                    );
                    match root? {
                        FiberNewton::Excluded => continue,
                        FiberNewton::Enclosure { uv, unique: true } => {
                            phase = "strict original root ownership/crossing";
                            active_uv = uv;
                            let root = qualify_fiber_root(
                                surface,
                                domain,
                                uv,
                                fiber,
                                restriction,
                                orientation,
                                domain_bytes,
                                work,
                                cancel,
                            );
                            match root? {
                                QualifiedRoot::Excluded => continue,
                                QualifiedRoot::Unresolved => return Ok(false),
                                QualifiedRoot::Owned { inward, forward } => {
                                    // Distinct original owners are never
                                    // deduplicated by physical nearness.
                                    if !roots.record(inward, forward) {
                                        return Ok(false);
                                    }
                                    continue;
                                }
                            }
                        }
                        FiberNewton::Enclosure { unique: false, .. } | FiberNewton::Unresolved => {}
                    }
                    if cell.depth >= MAX_DEPTH {
                        return Ok(false);
                    }
                    let children = match split(cell.uv, cell.depth, face_index) {
                        Ok(children) => children,
                        Err(cause) if cause.code == GeometryErrorCode::AnalysisIndeterminate => {
                            return Ok(false);
                        }
                        Err(cause) => return Err(cause),
                    };
                    if pending + children.len() > stack.len() {
                        return Err(GeometryError::new(
                            GeometryErrorCode::ResourceLimit,
                            "original fiber census exhausted its bounded cover stack",
                        ));
                    }
                    for uv in children {
                        work.charge(1, cancel)?;
                        stack[pending] = Some(FiberCell {
                            uv,
                            depth: cell.depth + 1,
                            trim_inside,
                        });
                        pending += 1;
                    }
                }
            }
            Ok(true)
        })();
        let resolved = resolved.map_err(|error| work.annotate_error(error, before, format_args!(
            "original cylinder/fiber census face={face_index} phase={phase} uv={active_uv:?} depth={active_depth}"
        )))?;
        if !resolved {
            return Ok(None);
        }
    }
    Ok((roots.outward.is_some() || roots.inward.is_some()).then_some(roots))
}

fn validate_fiber(fiber: &OriginalFiber) -> Result<(), GeometryError> {
    let exact_forward = cross(fiber.rows[0], fiber.rows[1]);
    if fiber.center.into_iter().any(|v| !v.is_finite())
        || fiber
            .rows
            .into_iter()
            .flatten()
            .chain(fiber.offsets)
            .chain(std::iter::once(fiber.forward_offset))
            .any(|v| !v.lo.is_finite() || !v.hi.is_finite() || v.lo > v.hi)
        || !fiber.min_forward.is_finite()
        || fiber.min_forward < 0.0
        || fiber
            .forward
            .iter()
            .any(|value| !value.lo.is_finite() || !value.hi.is_finite() || value.lo > value.hi)
        || dot(fiber.forward, exact_forward).lo <= 0.0
        || length(exact_forward)?.lo <= 0.0
    {
        return Err(GeometryError::new(
            GeometryErrorCode::ConditioningFailure,
            "original fiber lacks finite independent rows and a strictly increasing forward covector",
        ));
    }
    if let Some(core) = fiber.core {
        if !core.radius.is_finite()
            || core.radius <= 0.0
            || core
                .frame
                .origin_mm
                .into_iter()
                .chain(core.frame.x_axis)
                .chain(core.frame.y_axis)
                .chain(core.frame.z_axis)
                .any(|v| !v.is_finite())
        {
            return Err(GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                "original fiber outer-core gate has invalid stored coefficients",
            ));
        }
    }
    Ok(())
}

fn fiber_value(fiber: &OriginalFiber, position: [Interval; 3]) -> [Interval; 2] {
    let relative = std::array::from_fn(|i| position[i].sub(Interval::point(fiber.center[i])));
    std::array::from_fn(|i| dot(fiber.rows[i], relative).add(fiber.offsets[i]))
}

pub(super) fn fiber_forward(fiber: &OriginalFiber, position: [Interval; 3]) -> Interval {
    dot(
        fiber.forward,
        std::array::from_fn(|i| position[i].sub(Interval::point(fiber.center[i]))),
    )
    .add(fiber.forward_offset)
}

fn fiber_core_radius(core: FiberCore, position: [Interval; 3]) -> Interval {
    let xy = project(position, core.frame);
    xy[0].square().add(xy[1].square())
}

fn fiber_image_excluded(
    fiber: &OriginalFiber,
    position: [Interval; 3],
    height: HeightFunctional,
    request: &SlabRequest,
) -> bool {
    fiber_value(fiber, position)
        .into_iter()
        .any(|v| !v.contains_zero())
        || fiber_restriction_excluded(fiber, position, height, request)
}

fn fiber_restriction_excluded(
    fiber: &OriginalFiber,
    position: [Interval; 3],
    height: HeightFunctional,
    request: &SlabRequest,
) -> bool {
    if let Some(core) = fiber.core {
        if project(position, core.frame)[0].hi <= 0.0
            || fiber_core_radius(core, position).hi <= Interval::point(core.radius).square().lo
        {
            return true;
        }
    } else if fiber_forward(fiber, position).hi < fiber.min_forward {
        return true;
    }
    let h = height.height(position);
    h.hi < request.height_interval_mm[0] || h.lo > request.height_interval_mm[1]
}

fn census_root_owned(
    fiber: &OriginalFiber,
    position: [Interval; 3],
    normal: [Interval; 3],
    restriction: CensusRestriction<'_>,
) -> Option<bool> {
    let CensusRestriction {
        height,
        request,
        inward,
        ..
    } = restriction;
    let h = height.height(position);
    let positive_half = match fiber.core {
        Some(core) => {
            project(position, core.frame)[0].lo > 0.0
                && fiber_core_radius(core, position).lo > Interval::point(core.radius).square().hi
        }
        None => fiber_forward(fiber, position).lo > fiber.min_forward,
    };
    if !positive_half
        || h.lo <= request.height_interval_mm[0]
        || h.hi >= request.height_interval_mm[1]
    {
        return None;
    }
    // A normal pairs with the physical ray TANGENT, never with the positive
    // radial covector (the latter differs under a nonorthogonal affine map).
    let crossing = dot(normal, cross(fiber.rows[0], fiber.rows[1]));
    let role = if crossing.lo > 0.0 {
        false
    } else if crossing.hi < 0.0 {
        true
    } else {
        return None;
    };
    inward
        .is_none_or(|required| required == role)
        .then_some(role)
}

#[derive(Clone, Copy)]
pub(super) enum FiberNewton {
    Excluded,
    Enclosure { uv: [[f64; 2]; 2], unique: bool },
    Unresolved,
}

/// A numerical midpoint inverse only proposes the preconditioner. Existence
/// requires strict interval inclusion; uniqueness additionally requires an
/// interval infinity-norm contraction on the ENTIRE owning original cell.
pub(super) fn fiber_krawczyk(
    surface: &(impl SurfaceEvaluator + ?Sized),
    uv: [[f64; 2]; 2],
    jet: &SurfaceEnclosure,
    fiber: &OriginalFiber,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<FiberNewton, GeometryError> {
    fiber_krawczyk_jacobian(
        surface,
        uv,
        fiber.rows.map(|row| [dot(row, jet.du), dot(row, jet.dv)]),
        fiber,
        work,
        cancel,
    )
}

fn fiber_krawczyk_jacobian(
    surface: &(impl SurfaceEvaluator + ?Sized),
    uv: [[f64; 2]; 2],
    jacobian: [[Interval; 2]; 2],
    fiber: &OriginalFiber,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<FiberNewton, GeometryError> {
    work.charge(1, cancel)?;
    let middle = jacobian.map(|row| row.map(Interval::mid));
    let determinant = middle[0][0] * middle[1][1] - middle[0][1] * middle[1][0];
    if !determinant.is_finite() || determinant == 0.0 {
        return Ok(FiberNewton::Unresolved);
    }
    let inverse = [
        [middle[1][1] / determinant, -middle[0][1] / determinant],
        [-middle[1][0] / determinant, middle[0][0] / determinant],
    ];
    if inverse.iter().flatten().any(|v| !v.is_finite()) {
        return Ok(FiberNewton::Unresolved);
    }
    let inverse = inverse.map(|row| row.map(Interval::point));
    if inverse[0][0]
        .mul(inverse[1][1])
        .sub(inverse[0][1].mul(inverse[1][0]))
        .contains_zero()
    {
        return Ok(FiberNewton::Unresolved);
    }
    let center = uv.map(|v| v[0] * 0.5 + v[1] * 0.5);
    let at_center = surface_functional_enclosures(
        surface,
        center.map(|v| [v, v]),
        &fiber_equations(fiber),
        work,
        cancel,
    )?;
    let value = at_center.map(|jet| jet.position);
    let remainder: [[Interval; 2]; 2] = std::array::from_fn(|i| {
        std::array::from_fn(|j| {
            (0..2).fold(Interval::point(if i == j { 1.0 } else { 0.0 }), |s, k| {
                s.sub(inverse[i][k].mul(jacobian[k][j]))
            })
        })
    });
    let boxes = uv.map(|v| Interval { lo: v[0], hi: v[1] });
    let krawczyk: [Interval; 2] = std::array::from_fn(|i| {
        let residual = (0..2).fold(Interval::point(center[i]), |s, k| {
            s.sub(inverse[i][k].mul(value[k]))
        });
        (0..2).fold(residual, |s, j| {
            s.add(remainder[i][j].mul(boxes[j].sub(Interval::point(center[j]))))
        })
    });
    let mut contracted = [[0.0; 2]; 2];
    for axis in 0..2 {
        let Some(overlap) = intersection(boxes[axis], krawczyk[axis]) else {
            return Ok(FiberNewton::Excluded);
        };
        contracted[axis] = [overlap.lo, overlap.hi];
    }
    let contraction = remainder.map(|row| up(row[0].abs_upper() + row[1].abs_upper()));
    let unique = contraction.into_iter().all(|v| v < 1.0)
        && (0..2).all(|i| strictly_contains(boxes[i], krawczyk[i]));
    Ok(FiberNewton::Enclosure {
        uv: contracted,
        unique,
    })
}

enum QualifiedRoot {
    Excluded,
    Owned { inward: bool, forward: Interval },
    Unresolved,
}

#[allow(clippy::too_many_arguments)]
fn qualify_fiber_root(
    surface: &(impl SurfaceEvaluator + ?Sized),
    domain: &PreparedFaceTrimDomain<'_>,
    mut uv: [[f64; 2]; 2],
    fiber: &OriginalFiber,
    restriction: CensusRestriction<'_>,
    orientation: bool,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<QualifiedRoot, GeometryError> {
    for _ in 0..=MAX_DEPTH {
        work.charge(1, cancel)?;
        let membership = domain.classify_with_byte_limit(uv, remaining_bytes, work, cancel)?;
        if membership == TrimClassification::Outside {
            return Ok(QualifiedRoot::Excluded);
        }
        let jet = surface_enclosure(surface, uv, work, cancel)?;
        // Uniqueness on the original owner was already proved. Its contracted
        // enclosure covers ALL roots of that owner, so whole-image exclusions
        // of this enclosure do not discard unexamined roots elsewhere in it.
        if fiber_value(fiber, jet.position)
            .into_iter()
            .any(|v| !v.contains_zero())
        {
            return Err(GeometryError::new(
                GeometryErrorCode::ConditioningFailure,
                "original fiber unique-root image became inconsistent",
            ));
        }
        if fiber_restriction_excluded(fiber, jet.position, restriction.height, restriction.request)
        {
            return Ok(QualifiedRoot::Excluded);
        }
        if membership == TrimClassification::Inside {
            if let Some(inward) =
                census_root_owned(fiber, jet.position, normal(&jet, orientation), restriction)
            {
                return Ok(QualifiedRoot::Owned {
                    inward,
                    forward: fiber_forward(fiber, jet.position),
                });
            }
        }
        match fiber_krawczyk(surface, uv, &jet, fiber, work, cancel)? {
            FiberNewton::Excluded => {
                return Err(GeometryError::new(
                    GeometryErrorCode::ConditioningFailure,
                    "original fiber unique-root enclosure became inconsistent",
                ));
            }
            FiberNewton::Enclosure { uv: next, .. } if next != uv => uv = next,
            FiberNewton::Enclosure { .. } | FiberNewton::Unresolved => {
                return Ok(QualifiedRoot::Unresolved);
            }
        }
    }
    Ok(QualifiedRoot::Unresolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use solidcore::geom::{extent2d::Extent2d, vector2d::Vec2};
    use solidcore::surface::{plane::Plane, surface::p2::SurfaceKind};
    use solidcore::{Point3, Vec3};

    fn affine_surface() -> Surface {
        Surface::new(SurfaceKind::Plane(Plane::new(
            Point3::new(0.0, 0.0, 4.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec2::new(4.0, 3.0),
            &Extent2d::from_bounds(0.0, 0.0, 1.0, 1.0),
        )))
    }

    #[test]
    fn captured_original_radian_carrier_keeps_its_physical_fiber_root() {
        use crate::kernel::source_operand::{
            PreparedOriginalSurface, SOURCE_OPERAND_SCRATCH_BYTES,
        };
        use crate::kernel::trim::PreparedTrimSurface;
        let source = solidcraft_step::geom::Surface::Cylinder {
            position: solidcraft_step::geom::Placement {
                origin: [0.0; 3],
                axis: [0.0, 0.0, 1.0],
                ref_direction: [1.0, 0.0, 0.0],
            },
            radius: 2.0,
        };
        let uv = [[1.5, 1.65], [0.0, 1.0]];
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let prepared = PreparedOriginalSurface::new(
            &source,
            uv,
            SOURCE_OPERAND_SCRATCH_BYTES + 64 * 1024,
            &mut work,
            &cancel,
        )
        .unwrap();
        let operand = PreparedTrimSurface::Original(&prepared);
        let jet = surface_enclosure(&operand, uv, &mut work, &cancel).unwrap();
        let cell = super::super::BoundaryCell {
            position: jet.position,
            original_face: Some((operand, uv)),
        };
        let (captured, captured_uv) = cell.original_face.unwrap();
        let rows = [[1.0, 0.0, 0.0], [0.0, 0.0, -1.0]].map(|row| row.map(Interval::point));
        let ray = OriginalFiber {
            center: [0.0, 0.0, 0.25],
            rows,
            offsets: [Interval::point(0.0); 2],
            forward: cross(rows[0], rows[1]),
            forward_offset: Interval::point(0.0),
            min_forward: 0.0,
            core: None,
        };
        assert!(fiber_forward(&ray, jet.position).lo > 0.0);
        // The root is theta=pi/2 in the ORIGINAL radian law. The same numbers
        // evaluated as native degrees have x near 2 and have no such root.
        assert!(matches!(
            fiber_krawczyk(&captured, captured_uv, &jet, &ray, &mut work, &cancel).unwrap(),
            FiberNewton::Enclosure { unique: true, .. },
        ));
    }

    fn fiber(center: [f64; 3]) -> OriginalFiber {
        OriginalFiber {
            center,
            rows: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0]].map(|v| v.map(Interval::point)),
            offsets: [Interval::point(0.0); 2],
            forward: [
                Interval::point(0.0),
                Interval::point(0.0),
                Interval::point(1.0),
            ],
            forward_offset: Interval::point(0.0),
            min_forward: 0.0,
            core: None,
        }
    }

    fn request() -> SlabRequest {
        SlabRequest {
            frame: PlaneFrameMm {
                origin_mm: [0.0; 3],
                x_axis: [1.0, 0.0, 0.0],
                y_axis: [0.0, 1.0, 0.0],
                z_axis: [0.0, 0.0, 1.0],
            },
            height_interval_mm: [-1.0, 20.0],
            sample_height_mm: 2.0,
            native_shape_error_mm: 0.1,
        }
    }

    #[test]
    fn affine_fiber_root_requires_strict_whole_cell_interval_inclusion() {
        let surface = affine_surface();
        let uv = [[0.0, 1.0], [0.0, 1.0]];
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        let jet = surface_enclosure(&surface, uv, &mut work, &cancel).unwrap();
        assert!(matches!(
            fiber_krawczyk(
                &surface,
                uv,
                &jet,
                &fiber([1.25, 1.75, 2.0]),
                &mut work,
                &cancel,
            )
            .unwrap(),
            FiberNewton::Enclosure { unique: true, .. }
        ));
        assert!(!matches!(
            fiber_krawczyk(
                &surface,
                uv,
                &jet,
                &fiber([0.0, 1.75, 2.0]),
                &mut work,
                &cancel,
            )
            .unwrap(),
            FiberNewton::Enclosure { unique: true, .. }
        ));
        assert!(matches!(
            fiber_krawczyk(
                &surface,
                uv,
                &jet,
                &fiber([8.0, 1.75, 2.0]),
                &mut work,
                &cancel,
            )
            .unwrap(),
            FiberNewton::Excluded
        ));
    }

    #[test]
    fn affine_offset_row_family_has_uniform_not_midpoint_only_root_inclusion() {
        let surface = affine_surface();
        let zero = Interval::point(0.0);
        let one = Interval::point(1.0);
        let uncertainty = Interval::new(-1.0 / 128.0, 1.0 / 128.0).unwrap();
        let rows = [[one, zero, uncertainty], [uncertainty, one, zero]];
        let family = OriginalFiber {
            center: [0.0, 0.0, 2.0],
            rows,
            offsets: [
                Interval::new(-1.3, -1.2).unwrap(),
                Interval::new(-1.8, -1.7).unwrap(),
            ],
            forward: cross(rows[0], rows[1]),
            forward_offset: Interval::point(0.0),
            min_forward: 0.0,
            core: None,
        };
        validate_fiber(&family).unwrap();
        let mut work = GeometryWorkBudget::new(1_000_000);
        let cancel = AtomicBool::new(false);
        let uv = [[0.0, 1.0], [0.0, 1.0]];
        let jet = surface_enclosure(&surface, uv, &mut work, &cancel).unwrap();
        let FiberNewton::Enclosure {
            uv: root,
            unique: true,
        } = fiber_krawczyk(&surface, uv, &jet, &family, &mut work, &cancel).unwrap()
        else {
            panic!("entire affine coefficient family must have one strictly owned root");
        };
        assert!(root.into_iter().all(|v| v[0] < v[1]));
        for c in [uncertainty.lo, uncertainty.hi] {
            for d in [uncertainty.lo, uncertainty.hi] {
                for a in [-1.3, -1.2] {
                    for b in [-1.8, -1.7] {
                        let u = (-a - 2.0 * c) / 4.0;
                        let v = (-b - 4.0 * d * u) / 3.0;
                        assert!(root[0][0] <= u && u <= root[0][1]);
                        assert!(root[1][0] <= v && v <= root[1][1]);
                    }
                }
            }
        }
    }

    #[test]
    fn outer_meridian_gate_excludes_inner_roots_even_when_forward_is_positive() {
        let request = request();
        let height = HeightFunctional::from_frame(request.frame);
        let mut fiber = fiber([0.0; 3]);
        fiber.core = Some(FiberCore {
            frame: request.frame,
            radius: 3.0,
        });
        let inner = [1.0, 1.0, 2.0].map(Interval::point);
        assert!(fiber_forward(&fiber, inner).lo > 0.0);
        assert!(fiber_image_excluded(&fiber, inner, height, &request));
        let outer = [4.0, 1.0, 2.0].map(Interval::point);
        assert!(fiber_core_radius(fiber.core.unwrap(), outer).lo > 9.0);
        let backward_chart = [-4.0, 1.0, 2.0].map(Interval::point);
        assert!(fiber_restriction_excluded(
            &fiber,
            backward_chart,
            height,
            &request
        ));
        fiber.center[2] = 10.0;
        assert!(fiber_forward(&fiber, outer).hi < 0.0);
        assert!(!fiber_restriction_excluded(&fiber, outer, height, &request));
    }

    #[test]
    fn source_census_accepts_original_polynomial_and_rational_affine_carriers() {
        for name in [
            "native-parity-bspline-box.step",
            "native-parity-nurbs-box.step",
        ] {
            let (_, solid, _) = crate::parity_fixtures::recipes()
                .into_iter()
                .find(|(candidate, _, _)| *candidate == name)
                .unwrap();
            let text = crate::parity_fixtures::original(name, solid);
            let cancel = AtomicBool::new(false);
            let definition = crate::from_step_bytes_with_cancel(text.as_bytes(), &cancel).unwrap();
            let mut work = GeometryWorkBudget::new(100_000_000);
            let owner = crate::analysis::material_source::prepare_solid(
                &definition,
                MAX_SCRATCH_BYTES,
                &mut work,
                &cancel,
            )
            .unwrap();
            let source = owner.source_view().unwrap();
            let fiber = fiber([1.23456789, 2.34567891, 2.0]);
            assert!(
                original_fiber_degree_one(
                    source,
                    &fiber,
                    &request(),
                    HeightFunctional::from_frame(request().frame),
                    MAX_SCRATCH_BYTES,
                    &mut work,
                    &cancel,
                )
                .unwrap()
                .is_some(),
                "complete original carrier fiber has one outward root"
            );
        }
    }

    #[test]
    fn multiple_intersections_and_void_entry_do_not_grant_degree_one() {
        let name = "native-parity-nurbs-torus.step";
        let (_, solid, _) = crate::parity_fixtures::recipes()
            .into_iter()
            .find(|(candidate, _, _)| *candidate == name)
            .unwrap();
        let text = crate::parity_fixtures::original(name, solid);
        let cancel = AtomicBool::new(false);
        let definition = crate::from_step_bytes_with_cancel(text.as_bytes(), &cancel).unwrap();
        let fiber = OriginalFiber {
            center: [0.0; 3],
            rows: [[0.0, 1.0, 0.0], [0.0, 0.0, 1.0]].map(|v| v.map(Interval::point)),
            offsets: [Interval::point(0.0); 2],
            forward: [
                Interval::point(1.0),
                Interval::point(0.0),
                Interval::point(0.0),
            ],
            forward_offset: Interval::point(0.0),
            min_forward: 0.0,
            core: None,
        };
        let mut request = request();
        request.height_interval_mm = [-100.0, 100.0];
        let mut work = GeometryWorkBudget::new(100_000_000);
        let owner = crate::analysis::material_source::prepare_solid(
            &definition,
            MAX_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(
            original_fiber_degree_one(
                owner.source_view().unwrap(),
                &fiber,
                &request,
                HeightFunctional::from_frame(request.frame),
                MAX_SCRATCH_BYTES,
                &mut work,
                &cancel,
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn census_scratch_work_and_cancellation_failures_remain_typed() {
        let name = "native-parity-bspline-box.step";
        let (_, solid, _) = crate::parity_fixtures::recipes()
            .into_iter()
            .find(|(candidate, _, _)| *candidate == name)
            .unwrap();
        let text = crate::parity_fixtures::original(name, solid);
        let cancel = AtomicBool::new(false);
        let definition = crate::from_step_bytes_with_cancel(text.as_bytes(), &cancel).unwrap();
        let owner = crate::analysis::material_source::prepare_solid(
            &definition,
            MAX_SCRATCH_BYTES,
            &mut GeometryWorkBudget::new(100_000_000),
            &cancel,
        )
        .unwrap();
        let source = owner.source_view().unwrap();
        let fiber = fiber([1.25, 2.25, 2.0]);
        let mut work = GeometryWorkBudget::new(100);
        let error = original_fiber_degree_one(
            source,
            &fiber,
            &request(),
            HeightFunctional::from_frame(request().frame),
            FIBER_STACK_BYTES - 1,
            &mut work,
            &cancel,
        )
        .err()
        .unwrap();
        assert_eq!(error.code, GeometryErrorCode::ResourceLimit);
        assert_eq!(work.consumed(), 1);
        let mut work = GeometryWorkBudget::new(0);
        let error = original_fiber_degree_one(
            source,
            &fiber,
            &request(),
            HeightFunctional::from_frame(request().frame),
            MAX_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .err()
        .unwrap();
        assert_eq!(error.code, GeometryErrorCode::ResourceLimit);
        let mut work = GeometryWorkBudget::new(100);
        let error = original_fiber_degree_one(
            source,
            &fiber,
            &request(),
            HeightFunctional::from_frame(request().frame),
            MAX_SCRATCH_BYTES,
            &mut work,
            &AtomicBool::new(true),
        )
        .err()
        .unwrap();
        assert_eq!(error.code, GeometryErrorCode::Cancelled);
        assert_eq!(work.consumed(), 1);
    }

    #[test]
    fn affine_cylinder_crossing_uses_ray_tangent_not_radial_covector() {
        let mut ray = OriginalFiber {
            center: [0.0; 3],
            rows: [[0.0, 1.0, 0.0], [0.0, 0.0, 1.0]].map(|r| r.map(Interval::point)),
            offsets: [Interval::point(0.0); 2],
            forward: [1.0, 4.0, 0.0].map(Interval::point),
            forward_offset: Interval::point(0.0),
            min_forward: 0.0,
            core: None,
        };
        validate_fiber(&ray).unwrap();
        let request = request();
        let restriction = CensusRestriction {
            height: HeightFunctional::from_frame(request.frame),
            request: &request,
            faces: None,
            inward: None,
        };
        let position = [2.0, 0.0, 2.0].map(Interval::point);
        let normal = [1.0, -1.0, 0.0].map(Interval::point);
        assert!(dot(normal, ray.forward).hi < 0.0);
        assert_eq!(
            census_root_owned(&ray, position, normal, restriction),
            Some(false)
        );
        assert_eq!(
            census_root_owned(&ray, position, normal.map(Interval::neg), restriction),
            Some(true)
        );
        ray.forward = [-1.0, 4.0, 0.0].map(Interval::point);
        assert_eq!(
            validate_fiber(&ray).unwrap_err().code,
            GeometryErrorCode::ConditioningFailure
        );
    }

    #[test]
    fn cylinder_calibration_refuses_folded_normals_and_closed_cut_touches() {
        let ray = OriginalFiber {
            center: [0.0; 3],
            rows: [[0.0, 1.0, 0.0], [0.0, 0.0, 1.0]].map(|r| r.map(Interval::point)),
            offsets: [Interval::point(0.0); 2],
            forward: [1.0, 0.0, 0.0].map(Interval::point),
            forward_offset: Interval::point(0.0),
            min_forward: 0.0,
            core: None,
        };
        let request = request();
        let restriction = CensusRestriction {
            height: HeightFunctional::from_frame(request.frame),
            request: &request,
            faces: None,
            inward: None,
        };
        let regular = [1.0, 0.0, 0.0].map(Interval::point);
        let folded = [
            Interval::new(-0.1, 1.0).unwrap(),
            Interval::point(0.0),
            Interval::point(0.0),
        ];
        assert_eq!(
            census_root_owned(
                &ray,
                [2.0, 0.0, 2.0].map(Interval::point),
                folded,
                restriction
            ),
            None
        );
        for cut in request.height_interval_mm {
            assert_eq!(
                census_root_owned(
                    &ray,
                    [2.0, 0.0, cut].map(Interval::point),
                    regular,
                    restriction
                ),
                None
            );
        }
    }

    #[test]
    fn independent_role_inventory_refuses_extra_sheets_and_reversed_void_order() {
        let mut disk = FiberRoots::default();
        assert!(disk.record(false, Interval::point(8.0)));
        assert!(!disk.record(false, Interval::point(10.0)));
        let mut annulus = FiberRoots::default();
        assert!(annulus.record(true, Interval::new(3.9, 4.1).unwrap()));
        assert!(annulus.record(false, Interval::new(7.9, 8.1).unwrap()));
        assert!(!annulus.record(true, Interval::point(2.0)));
        let mut reversed = FiberRoots::default();
        assert!(reversed.record(false, Interval::point(4.0)));
        assert!(!reversed.record(true, Interval::point(8.0)));
        let mut touching = FiberRoots::default();
        assert!(touching.record(true, Interval::new(3.9, 4.1).unwrap()));
        assert!(!touching.record(false, Interval::new(4.0, 4.2).unwrap()));
    }

    #[test]
    fn one_original_carrier_can_supply_both_cylinder_roles_but_not_extra_sheets() {
        let name = "native-parity-nurbs-torus.step";
        let (_, solid, _) = crate::parity_fixtures::recipes()
            .into_iter()
            .find(|(candidate, _, _)| *candidate == name)
            .unwrap();
        let text = crate::parity_fixtures::original(name, solid);
        let cancel = AtomicBool::new(false);
        let definition = crate::from_step_bytes_with_cancel(text.as_bytes(), &cancel).unwrap();
        assert_eq!(definition.face_ids().len(), 1);
        let mut request = request();
        request.height_interval_mm = [-1.0, 1.0];
        request.sample_height_mm = 0.37;
        let rows = [[-0.37, 1.0, 0.0], [0.0, 0.0, 1.0]].map(|r| r.map(Interval::point));
        let mut ray = OriginalFiber {
            center: [0.0, 0.0, 0.37],
            rows,
            offsets: [Interval::point(0.0); 2],
            forward: cross(rows[0], rows[1]),
            forward_offset: Interval::point(0.0),
            min_forward: 0.0,
            core: None,
        };
        let mut work = GeometryWorkBudget::new(100_000_000);
        let owner = crate::analysis::material_source::prepare_solid(
            &definition,
            MAX_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let census = original_cylinder_fiber_census(
            owner.source_view().unwrap(),
            &ray,
            &request,
            HeightFunctional::from_frame(request.frame),
            MAX_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap()
        .expect("complete single-face source has both radial roles");
        assert!(census.inner_forward().unwrap().lo > 0.0);
        assert!(census.inner_forward().unwrap().hi < census.outer_forward().lo);
        // Same infinite line and same carrier, but the positive half-ray now
        // crosses both sides of the torus: two inner and two outer sheets.
        // A source-face label cannot distinguish or silently drop these roots.
        ray.center = [-10.0, -3.7, 0.37];
        assert!(
            original_cylinder_fiber_census(
                owner.source_view().unwrap(),
                &ray,
                &request,
                HeightFunctional::from_frame(request.frame),
                MAX_SCRATCH_BYTES,
                &mut work,
                &cancel,
            )
            .unwrap()
            .is_none()
        );
    }
}
