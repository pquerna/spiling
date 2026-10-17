// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Occurrence queries use the actual quaternion polynomial, not the transpose
//! of an assumed orthogonal matrix. Accepted near-unit quaternions need not
//! describe an exactly orthogonal binary64 transform.

use super::{
    AnalysisPolicy, BoundsPolicy, GeometryWorkBudget, HeightAnalysis, HeightCapture,
    HeightFunctional, InvariantHeightDomain, NativeEndpointStratum, NativeOwnedEndpoint,
    PlacedSlabCapture, PlacedSlabRequest, ProjectedBounds, SlabAnalysis, SlabRequest,
    projected_bounds,
};
use crate::{
    Definition, GeometryError, NativeSection, PlaneFrameMm, PlaneMm, PreparedSectionSource,
    parametric::Interval,
};
use spiling_contracts::geometry::{
    GeometryErrorCode, RigidPoseMm, SECTION_PLANE_TOLERANCE_MM, SECTION_SAMPLING_TOLERANCE_MM,
};
use std::sync::atomic::AtomicBool;

mod material_coordinates;
mod sample_candidate;
pub(super) use material_coordinates::{ActualMaterialCoordinates, MaterialCoordinatesCapture};
use sample_candidate::WorldSampleCandidate;

pub(super) type Matrix = [[Interval; 3]; 3];

fn error(code: GeometryErrorCode, message: &'static str) -> GeometryError {
    GeometryError::new(code, message)
}

// Error-free transforms preserve singleton coefficients only when the binary64
// operation is actually exact. In particular, these are not epsilon snaps of
// small quaternion products, event widths, or transformed axial directions.
fn add(a: Interval, b: Interval) -> Interval {
    if a.lo == 0.0 && a.hi == 0.0 {
        return b;
    }
    if b.lo == 0.0 && b.hi == 0.0 {
        return a;
    }
    if a.lo == a.hi && b.lo == b.hi {
        let sum = a.lo + b.lo;
        let shifted = sum - a.lo;
        let residual = (a.lo - (sum - shifted)) + (b.lo - shifted);
        if sum.is_finite() && residual == 0.0 {
            return Interval::point(sum);
        }
    }
    a.add(b)
}
fn sub(a: Interval, b: Interval) -> Interval {
    add(a, b.neg())
}
fn mul(a: Interval, b: Interval) -> Interval {
    if (a.lo == 0.0 && a.hi == 0.0) || (b.lo == 0.0 && b.hi == 0.0) {
        return Interval::point(0.0);
    }
    if a.lo == 1.0 && a.hi == 1.0 {
        return b;
    }
    if b.lo == 1.0 && b.hi == 1.0 {
        return a;
    }
    if a.lo == -1.0 && a.hi == -1.0 {
        return b.neg();
    }
    if b.lo == -1.0 && b.hi == -1.0 {
        return a.neg();
    }
    if a.lo == a.hi && b.lo == b.hi {
        let product = a.lo * b.lo;
        // A rounded FMA residual can itself underflow to zero. Above 2^-969,
        // every possible nonzero binary64 product residual is representable.
        let residual_resolved = product.abs() >= 2.0 * f64::MIN_POSITIVE / f64::EPSILON;
        if product.is_finite() && residual_resolved && a.lo.mul_add(b.lo, -product) == 0.0 {
            return Interval::point(product);
        }
    }
    a.mul(b)
}
fn div(a: Interval, b: Interval) -> Result<Interval, GeometryError> {
    if b.contains_zero() {
        return Err(error(
            GeometryErrorCode::ConditioningFailure,
            "placed functional scale cannot be separated from zero",
        ));
    }
    if b.lo == 1.0 && b.hi == 1.0 {
        return Ok(a);
    }
    if a.lo == 0.0 && a.hi == 0.0 {
        return Ok(a);
    }
    if a.lo == a.hi && b.lo == b.hi {
        let quotient = a.lo / b.lo;
        if quotient.is_finite() {
            let product = mul(Interval::point(quotient), b);
            if product.lo == a.lo && product.hi == a.hi {
                return Ok(Interval::point(quotient));
            }
        }
    }
    a.div(b)
}
fn sqrt(a: Interval) -> Result<Interval, GeometryError> {
    if a.lo == a.hi && a.lo >= 0.0 {
        let root = a.lo.sqrt();
        let squared = mul(Interval::point(root), Interval::point(root));
        if squared.lo == a.lo && squared.hi == a.hi {
            return Ok(Interval::point(root));
        }
    }
    Interval {
        lo: a.lo.max(0.0),
        hi: a.hi,
    }
    .sqrt()
}
fn dot(a: [Interval; 3], b: [Interval; 3]) -> Interval {
    (0..3).fold(Interval::point(0.0), |sum, i| add(sum, mul(a[i], b[i])))
}
fn cross(a: [Interval; 3], b: [Interval; 3]) -> [Interval; 3] {
    [
        sub(mul(a[1], b[2]), mul(a[2], b[1])),
        sub(mul(a[2], b[0]), mul(a[0], b[2])),
        sub(mul(a[0], b[1]), mul(a[1], b[0])),
    ]
}
fn length(a: [Interval; 3]) -> Result<Interval, GeometryError> {
    let squared = a.into_iter().fold(Interval::point(0.0), |sum, value| {
        let square = if value.lo == value.hi {
            mul(value, value)
        } else {
            value.square()
        };
        add(sum, square)
    });
    sqrt(squared)
}
fn width(value: Interval) -> f64 {
    sub(Interval::point(value.hi), Interval::point(value.lo)).hi
}
fn finite(value: Interval) -> Result<Interval, GeometryError> {
    if value.lo.is_finite() && value.hi.is_finite() && value.lo <= value.hi {
        Ok(value)
    } else {
        Err(error(
            GeometryErrorCode::ConditioningFailure,
            "placed transform enclosure is nonfinite",
        ))
    }
}
fn normalized(direction: [f64; 3]) -> Result<[Interval; 3], GeometryError> {
    if direction.iter().any(|value| !value.is_finite()) {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "placed support direction must be finite",
        ));
    }
    let scale = direction.into_iter().map(f64::abs).fold(0.0, f64::max);
    if scale == 0.0 {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "placed support direction must be nonzero",
        ));
    }
    let scaled = [
        div(Interval::point(direction[0]), Interval::point(scale))?,
        div(Interval::point(direction[1]), Interval::point(scale))?,
        div(Interval::point(direction[2]), Interval::point(scale))?,
    ];
    let norm = length(scaled)?;
    Ok([
        div(scaled[0], norm)?,
        div(scaled[1], norm)?,
        div(scaled[2], norm)?,
    ])
}

/// Enclose R(q) for p + 2w(q_xyz × p) + 2q_xyz × (q_xyz × p).
/// The literal 1 on the diagonal is important: replacing it by |q|² would
/// silently change the existing transform for an accepted near-unit q.
pub(super) fn rotation(pose: RigidPoseMm) -> Result<Matrix, GeometryError> {
    pose.validate()?;
    let [x, y, z, w] = pose.rotation_xyzw.map(Interval::point);
    let one = Interval::point(1.0);
    let two = Interval::point(2.0);
    let result = [
        [
            sub(one, mul(two, add(mul(y, y), mul(z, z)))),
            mul(two, sub(mul(x, y), mul(z, w))),
            mul(two, add(mul(x, z), mul(y, w))),
        ],
        [
            mul(two, add(mul(x, y), mul(z, w))),
            sub(one, mul(two, add(mul(x, x), mul(z, z)))),
            mul(two, sub(mul(y, z), mul(x, w))),
        ],
        [
            mul(two, sub(mul(x, z), mul(y, w))),
            mul(two, add(mul(y, z), mul(x, w))),
            sub(one, mul(two, add(mul(x, x), mul(y, y)))),
        ],
    ];
    for row in result {
        for coefficient in row {
            finite(coefficient)?;
        }
    }
    if finite(dot(result[0], cross(result[1], result[2])))?.lo <= 0.0 {
        return Err(error(
            GeometryErrorCode::ConditioningFailure,
            "placed quaternion transform does not prove positive orientation",
        ));
    }
    Ok(result)
}
fn apply(matrix: Matrix, point: [Interval; 3]) -> [Interval; 3] {
    matrix.map(|row| dot(row, point))
}
fn source_extent(definition: &Definition) -> [f64; 3] {
    let bounds = definition.bounds_mm();
    std::array::from_fn(|i| bounds.min[i].abs().max(bounds.max[i].abs()))
}
fn coefficient_error(
    coefficients: [Interval; 3],
    nominal: [f64; 3],
    extent: [f64; 3],
) -> Result<f64, GeometryError> {
    let bound = (0..3).fold(Interval::point(0.0), |sum, i| {
        add(
            sum,
            mul(
                Interval::point(sub(coefficients[i], Interval::point(nominal[i])).abs_upper()),
                Interval::point(extent[i]),
            ),
        )
    });
    Ok(finite(bound)?.hi)
}
fn symmetric(radius: f64) -> Interval {
    Interval {
        lo: -radius,
        hi: radius,
    }
}

/// Tight world support brackets for the mathematical normalization of the
/// supplied world direction. Source hulls bound only coefficient error; native
/// feasible trimmed-domain witnesses still determine both support endpoints.
pub fn placed_bounds(
    definition: &Definition,
    pose: RigidPoseMm,
    world_direction: [f64; 3],
    policy: BoundsPolicy,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<ProjectedBounds, GeometryError> {
    placed_bounds_inner(
        crate::section::OriginalTrimSource::standalone(definition),
        pose,
        world_direction,
        policy,
        None,
        work,
        cancel,
    )
}

/// Scratch allowance excludes the pinned source proof and every caller-owned
/// buffer that coexists with this complete native directional support query.
pub(super) fn placed_bounds_with_scratch_limit(
    source: crate::section::OriginalTrimSource<'_, '_>,
    pose: RigidPoseMm,
    world_direction: [f64; 3],
    policy: BoundsPolicy,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<ProjectedBounds, GeometryError> {
    placed_bounds_inner(
        source,
        pose,
        world_direction,
        policy,
        Some(remaining_bytes),
        work,
        cancel,
    )
}

struct PlacedBoundsFrame {
    matrix: Matrix,
    coefficients: [Interval; 3],
    nominal: [f64; 3],
    norm: Interval,
    offset: Interval,
    policy: BoundsPolicy,
    source_policy: BoundsPolicy,
}
impl PlacedBoundsFrame {
    fn new(
        pose: RigidPoseMm,
        world_direction: [f64; 3],
        policy: BoundsPolicy,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        work.charge(1, cancel)?;
        let tolerance = policy.maximum_endpoint_uncertainty_mm;
        if !tolerance.is_finite() || tolerance <= 0.0 {
            return Err(error(
                GeometryErrorCode::InvalidGeometry,
                "placed bounds require positive finite endpoint uncertainty",
            ));
        }
        let matrix = rotation(pose)?;
        let world = normalized(world_direction)?;
        let coefficients =
            std::array::from_fn(|i| dot(std::array::from_fn(|j| matrix[j][i]), world));
        let nominal = coefficients.map(Interval::mid);
        let norm = length(nominal.map(Interval::point))?;
        if norm.lo <= 0.0 {
            return Err(error(
                GeometryErrorCode::ConditioningFailure,
                "posed support functional has no resolved positive norm",
            ));
        }
        let offset = dot(world, pose.translation_mm.map(Interval::point));
        let source_tolerance = div(Interval::point(tolerance * 0.25), norm)?.lo;
        if !source_tolerance.is_finite() || source_tolerance <= 0.0 {
            return Err(error(
                GeometryErrorCode::ConditioningFailure,
                "placed endpoint policy has no representable source uncertainty",
            ));
        }
        Ok(Self {
            matrix,
            coefficients,
            nominal,
            norm,
            offset,
            policy,
            source_policy: BoundsPolicy {
                maximum_endpoint_uncertainty_mm: source_tolerance,
            },
        })
    }

    fn finish(
        &self,
        source: ProjectedBounds,
        extent: [f64; 3],
    ) -> Result<ProjectedBounds, GeometryError> {
        let residual = coefficient_error(self.coefficients, self.nominal, extent)?;
        let transform = |bracket: [f64; 2]| -> Result<Interval, GeometryError> {
            finite(add(
                add(
                    mul(self.norm, Interval::new(bracket[0], bracket[1])?),
                    self.offset,
                ),
                symmetric(residual),
            ))
        };
        let minimum = transform(source.minimum)?;
        let maximum = transform(source.maximum)?;
        let achieved = width(minimum).max(width(maximum));
        if achieved > self.policy.maximum_endpoint_uncertainty_mm {
            return Err(error(
                GeometryErrorCode::ConditioningFailure,
                "placed endpoint arithmetic exceeds the requested uncertainty",
            ));
        }
        Ok(ProjectedBounds {
            minimum: [minimum.lo, minimum.hi],
            maximum: [maximum.lo, maximum.hi],
            achieved_uncertainty_mm: achieved,
        })
    }
}

fn placed_bounds_inner(
    source_context: crate::section::OriginalTrimSource<'_, '_>,
    pose: RigidPoseMm,
    world_direction: [f64; 3],
    policy: BoundsPolicy,
    remaining_bytes: Option<usize>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<ProjectedBounds, GeometryError> {
    let definition = source_context.definition();
    let frame = PlacedBoundsFrame::new(pose, world_direction, policy, work, cancel)?;
    let source = match remaining_bytes {
        Some(bytes) => super::bounds::projected_bounds_with_scratch_limit(
            source_context,
            frame.nominal,
            frame.source_policy,
            bytes,
            work,
            cancel,
        ),
        None => projected_bounds(definition, frame.nominal, frame.source_policy, work, cancel),
    }?;
    frame.finish(source, source_extent(definition))
}

/// Opaque complete material support bound for one exact source, recipe, pose
/// and directional query. Its terminal brackets are global material extrema,
/// not isolated critical events or permission to merge nearby source heights.
#[derive(Debug)]
pub struct PlacedMaterialProjectedBounds {
    source: super::bounds::MaterialProjectedBounds,
    pose: RigidPoseMm,
    world_direction: [f64; 3],
    policy: BoundsPolicy,
    bounds: ProjectedBounds,
    source_realization_error_mm: f64,
    scalar_transport_is_identically_zero: bool,
}
impl PlacedMaterialProjectedBounds {
    pub fn bounds(&self) -> ProjectedBounds {
        self.bounds
    }
    pub fn definition_id(&self) -> &spiling_contracts::geometry::DefinitionId {
        self.source.definition_id()
    }
    pub fn placed_pose(&self) -> RigidPoseMm {
        self.pose
    }
    pub fn world_direction(&self) -> [f64; 3] {
        self.world_direction
    }
    pub fn policy(&self) -> BoundsPolicy {
        self.policy
    }
    pub fn source_realization_error_mm(&self) -> f64 {
        self.source_realization_error_mm
    }
    /// True only for a whole-source scalar identity at the actual posed
    /// covector. An uncertain coefficient box cannot inherit nominal equality.
    pub fn scalar_transport_is_identically_zero(&self) -> bool {
        self.scalar_transport_is_identically_zero
    }
    /// Match the immutable original definition and its exact material recipe.
    /// Pose, direction and policy remain independently bound query fields.
    pub fn is_compatible_with_definition(&self, definition: &Definition) -> bool {
        self.source.definition_id() == definition.id()
            && self.source.material_realization_identity()
                == crate::kernel::MaterialRealizationIdentity::new(definition)
    }
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self
                .source
                .retained_bytes()
                .saturating_sub(std::mem::size_of::<super::bounds::MaterialProjectedBounds>())
    }
}

pub fn placed_material_bounds(
    definition: &Definition,
    pose: RigidPoseMm,
    world_direction: [f64; 3],
    policy: BoundsPolicy,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<PlacedMaterialProjectedBounds, GeometryError> {
    let remaining_bytes = super::slab::MAX_SLAB_SCRATCH_BYTES
        .checked_sub(definition.retained_query_proof_bytes())
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "placed bounds proof capacity exhausted",
            )
        })?;
    placed_material_bounds_with_scratch_limit(
        crate::section::OriginalTrimSource::standalone(definition),
        pose,
        world_direction,
        policy,
        remaining_bytes,
        work,
        cancel,
    )
}

pub(super) fn placed_material_bounds_with_scratch_limit(
    source_context: crate::section::OriginalTrimSource<'_, '_>,
    pose: RigidPoseMm,
    world_direction: [f64; 3],
    policy: BoundsPolicy,
    query_byte_limit: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<PlacedMaterialProjectedBounds, GeometryError> {
    let definition = source_context.definition();
    let frame = PlacedBoundsFrame::new(pose, world_direction, policy, work, cancel)?;
    let source = super::bounds::material_projected_bounds_with_scratch_limit(
        source_context,
        frame.nominal,
        frame.source_policy,
        query_byte_limit,
        work,
        cancel,
    )?;
    if source.definition_id() != definition.id()
        || source.material_realization_identity()
            != crate::kernel::MaterialRealizationIdentity::new(definition)
    {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "placed material support changed its source recipe",
        ));
    }
    let source_error = source.source_realization_error_mm();
    if !source_error.is_finite() || source_error < 0.0 {
        return Err(error(
            GeometryErrorCode::ConditioningFailure,
            "invalid source material realization error",
        ));
    }
    // Quaternion/coefficient residuals must cover actual G, not just the raw
    // source AABB. The physical source error enlarges each coordinate extent.
    let extent = source_extent(definition)
        .map(|extent| add(Interval::point(extent), Interval::point(source_error)).hi);
    let bounds = frame.finish(source.bounds(), extent)?;
    let source_realization_error_mm = finite(mul(
        Interval::point(source_error),
        Interval::point(spectral_bound(frame.matrix)?),
    ))?
    .hi;
    let scalar_transport_is_identically_zero = source.scalar_transport_is_identically_zero()
        && (0..3).all(|axis| {
            frame.coefficients[axis].lo == frame.nominal[axis]
                && frame.coefficients[axis].hi == frame.nominal[axis]
        });
    Ok(PlacedMaterialProjectedBounds {
        source,
        pose,
        world_direction,
        policy,
        bounds,
        source_realization_error_mm,
        scalar_transport_is_identically_zero,
    })
}

struct HeightMap {
    pose: RigidPoseMm,
    matrix: Matrix,
    frame: PlaneFrameMm,
    scale: f64,
    residual_mm: f64,
}
impl HeightMap {
    fn new(definition: &Definition, pose: RigidPoseMm) -> Result<Self, GeometryError> {
        let matrix = rotation(pose)?;
        let nominal = matrix[2].map(Interval::mid);
        let frame = PlaneFrameMm::from_plane(PlaneMm {
            origin_mm: [0.0; 3],
            normal: nominal,
        })?;
        let n = frame.z_axis;
        let norm_squared = n.into_iter().map(|x| x * x).sum::<f64>();
        let scale = (0..3).map(|i| nominal[i] * n[i]).sum::<f64>() / norm_squared;
        if !scale.is_finite() || scale <= 0.0 {
            return Err(error(
                GeometryErrorCode::ConditioningFailure,
                "placed height requires a resolved positive source scale",
            ));
        }
        // k and n are exact stored binary64 values; enclose their product too.
        let residual = std::array::from_fn(|i| {
            sub(
                matrix[2][i],
                mul(Interval::point(scale), Interval::point(n[i])),
            )
        });
        let residual_mm = coefficient_error(residual, [0.0; 3], source_extent(definition))?;
        Ok(Self {
            pose,
            matrix,
            frame,
            scale,
            residual_mm,
        })
    }
    fn source_range(&self, world: [f64; 2]) -> Result<[f64; 2], GeometryError> {
        // Evaluate CLOSED endpoints separately so error-free point arithmetic
        // preserves an exact inverse cap identity. A single wide interval
        // subtraction would unnecessarily round even world == translation
        // below zero and lose a genuinely complete source product domain.
        let endpoint = |world| {
            finite(div(
                add(
                    sub(
                        Interval::point(world),
                        Interval::point(self.pose.translation_mm[2]),
                    ),
                    symmetric(self.residual_mm),
                ),
                Interval::point(self.scale),
            )?)
        };
        Ok([endpoint(world[0])?.lo, endpoint(world[1])?.hi])
    }
    fn world_value(&self, source: [f64; 2]) -> Result<[f64; 2], GeometryError> {
        let value = finite(add(
            mul(
                Interval::point(self.scale),
                Interval {
                    lo: source[0],
                    hi: source[1],
                },
            ),
            Interval::point(self.pose.translation_mm[2]),
        ))?;
        Ok([value.lo, value.hi])
    }
    fn vertically_invariant(&self) -> bool {
        let direction = apply(self.matrix, self.frame.z_axis.map(Interval::point));
        direction[0].lo == 0.0
            && direction[0].hi == 0.0
            && direction[1].lo == 0.0
            && direction[1].hi == 0.0
            && !direction[2].contains_zero()
    }
}
fn world_frame() -> PlaneFrameMm {
    PlaneFrameMm {
        origin_mm: [0.0; 3],
        x_axis: [1.0, 0.0, 0.0],
        y_axis: [0.0, 1.0, 0.0],
        z_axis: [0.0, 0.0, 1.0],
    }
}
fn validate_range(range: [f64; 2]) -> Result<(), GeometryError> {
    if range.iter().all(|value| value.is_finite()) && range[0] < range[1] {
        Ok(())
    } else {
        Err(error(
            GeometryErrorCode::InvalidGeometry,
            "placed height range must be finite and increasing",
        ))
    }
}
fn placed_invariance(
    source: Option<InvariantHeightDomain>,
    map: &HeightMap,
    pose: RigidPoseMm,
    source_range: [f64; 2],
    range: [f64; 2],
) -> Option<InvariantHeightDomain> {
    let certificate = source.filter(|_| map.vertically_invariant())?;
    // source_range already encloses every source height in the unchanged
    // CLOSED world request, including functional residual and inverse rounding.
    // Containment therefore proves the entire requested world domain. An
    // inward-rounded forward roundtrip can lose endpoints that this proof owns.
    if certificate.height_interval_mm[0] <= source_range[0]
        && certificate.height_interval_mm[1] >= source_range[1]
    {
        return Some(InvariantHeightDomain {
            definition_id: certificate.definition_id,
            material_realization: certificate.material_realization,
            frame: world_frame(),
            height_interval_mm: range,
            pose: Some(pose),
        });
    }
    // A source proof may cover only the inner exact cap-to-cap interval, not
    // the entire requested support bracket. Never widen its reuse domain.
    let lower = add(
        add(
            mul(
                Interval::point(map.scale),
                Interval::point(certificate.height_interval_mm[0]),
            ),
            Interval::point(pose.translation_mm[2]),
        ),
        Interval::point(map.residual_mm),
    )
    .hi
    .max(range[0]);
    let upper = sub(
        add(
            mul(
                Interval::point(map.scale),
                Interval::point(certificate.height_interval_mm[1]),
            ),
            Interval::point(pose.translation_mm[2]),
        ),
        Interval::point(map.residual_mm),
    )
    .lo
    .min(range[1]);
    (lower.is_finite() && upper.is_finite() && lower < upper).then_some(InvariantHeightDomain {
        definition_id: certificate.definition_id,
        material_realization: certificate.material_realization,
        frame: world_frame(),
        height_interval_mm: [lower, upper],
        pose: Some(pose),
    })
}

/// Isolate actual world-height events. A height-value error band alone cannot
/// move nominal critical roots soundly: non-collinear coefficient uncertainty
/// therefore goes through the shared enclosed-functional critical analysis.
pub fn analyze_placed_height_domain(
    definition: &Definition,
    pose: RigidPoseMm,
    world_height_interval_mm: [f64; 2],
    policy: AnalysisPolicy,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<HeightAnalysis, GeometryError> {
    let remaining_bytes = super::slab::MAX_SLAB_SCRATCH_BYTES
        .checked_sub(definition.retained_query_proof_bytes())
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "placed height proof capacity exhausted",
            )
        })?;
    analyze_placed_height_domain_with_scratch_limit(
        crate::section::OriginalTrimSource::standalone(definition),
        pose,
        world_height_interval_mm,
        policy,
        remaining_bytes,
        work,
        cancel,
    )
}

pub(super) fn analyze_placed_height_domain_with_scratch_limit(
    source_context: crate::section::OriginalTrimSource<'_, '_>,
    pose: RigidPoseMm,
    world_height_interval_mm: [f64; 2],
    policy: AnalysisPolicy,
    query_byte_limit: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<HeightAnalysis, GeometryError> {
    let definition = source_context.definition();
    work.charge(1, cancel)?;
    validate_range(world_height_interval_mm)?;
    if !policy.maximum_height_uncertainty_mm.is_finite()
        || policy.maximum_height_uncertainty_mm <= 0.0
        || !policy.maximum_shape_error_mm.is_finite()
        || policy.maximum_shape_error_mm <= 0.0
    {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "placed analysis policies must be finite and positive",
        ));
    }
    let map = HeightMap::new(definition, pose)?;
    let mut result = if map.residual_mm == 0.0 {
        let range = map.source_range(world_height_interval_mm)?;
        let source_uncertainty = div(
            Interval::point(policy.maximum_height_uncertainty_mm),
            Interval::point(map.scale),
        )?
        .lo;
        if !source_uncertainty.is_finite() || source_uncertainty <= 0.0 {
            return Err(error(
                GeometryErrorCode::ConditioningFailure,
                "placed height policy has no representable source uncertainty",
            ));
        }
        let mut source = super::slab::analyze_height_domain_with_scratch_limit(
            source_context,
            map.frame,
            range,
            AnalysisPolicy {
                maximum_height_uncertainty_mm: source_uncertainty,
                maximum_shape_error_mm: policy.maximum_shape_error_mm,
            },
            query_byte_limit,
            work,
            cancel,
        )?;
        for event in source
            .event_brackets_mm
            .iter_mut()
            .chain(source.proved_event_brackets_mm.iter_mut())
        {
            work.charge(1, cancel)?;
            *event = map.world_value(*event)?;
            if width(Interval {
                lo: event[0],
                hi: event[1],
            }) > policy.maximum_height_uncertainty_mm
            {
                return Err(error(
                    GeometryErrorCode::AnalysisIndeterminate,
                    "placed critical-height bracket exceeds the requested uncertainty",
                ));
            }
        }
        for support in &mut source.capture.transport_support {
            work.charge(1, cancel)?;
            support.base_interval = map.world_value(support.base_interval)?;
            support.corrected_interval = map.world_value(support.corrected_interval)?;
            let displacement = finite(mul(
                Interval::point(map.scale),
                Interval::new(
                    support.displacement_interval[0],
                    support.displacement_interval[1],
                )?,
            ))?;
            support.displacement_interval = [displacement.lo, displacement.hi];
        }
        source.invariance = placed_invariance(
            source.invariance,
            &map,
            pose,
            range,
            world_height_interval_mm,
        );
        source
    } else {
        let mut enclosed = super::slab::analyze_height_domain_enclosed_with_scratch_limit(
            &super::slab::HeightDomainQuery {
                source: source_context,
                frame: map.frame,
                functional: HeightFunctional {
                    direction: map.matrix[2],
                    offset: Interval::point(pose.translation_mm[2]),
                },
                height_interval_mm: world_height_interval_mm,
                policy,
            },
            query_byte_limit,
            work,
            cancel,
        )?;
        // An enclosed root exclusion is not an exact extrusion certificate.
        enclosed.invariance = None;
        enclosed
    };
    let material_realization = result.material_realization_identity();
    if result.definition_id() != definition.id()
        || material_realization != crate::kernel::MaterialRealizationIdentity::new(definition)
    {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "placed height capture changed its original material realization",
        ));
    }
    result.capture = HeightCapture {
        definition_id: definition.id().clone(),
        material_realization,
        transport_support: std::mem::take(&mut result.capture.transport_support),
        transport_support_complete: result.capture.transport_support_complete,
        frame: world_frame(),
        height_interval_mm: world_height_interval_mm,
        policy,
        pose: Some(pose),
    };
    Ok(result)
}

/// sqrt(||R^T R||_infinity) bounds the Euclidean operator norm even when R is
/// not orthogonal. Every absolute row sum includes coefficient arithmetic.
fn spectral_bound(matrix: Matrix) -> Result<f64, GeometryError> {
    let mut upper: f64 = 0.0;
    for i in 0..3 {
        let sum = (0..3).fold(Interval::point(0.0), |sum, j| {
            let entry = dot(
                std::array::from_fn(|k| matrix[k][i]),
                std::array::from_fn(|k| matrix[k][j]),
            );
            add(sum, Interval::point(entry.abs_upper()))
        });
        upper = upper.max(finite(sum)?.hi);
    }
    Ok(sqrt(Interval::point(upper))?.hi)
}
fn forward(pose: RigidPoseMm, p: [f64; 3]) -> [f64; 3] {
    let [x, y, z, w] = pose.rotation_xyzw;
    let t = [
        2.0 * (y * p[2] - z * p[1]),
        2.0 * (z * p[0] - x * p[2]),
        2.0 * (x * p[1] - y * p[0]),
    ];
    [
        p[0] + w * t[0] + y * t[2] - z * t[1] + pose.translation_mm[0],
        p[1] + w * t[1] + z * t[0] - x * t[2] + pose.translation_mm[1],
        p[2] + w * t[2] + x * t[1] - y * t[0] + pose.translation_mm[2],
    ]
}
pub(super) fn transformed_position(
    matrix: Matrix,
    pose: RigidPoseMm,
    position: [Interval; 3],
) -> Result<[Interval; 3], GeometryError> {
    let rotated = apply(matrix, position);
    let result = std::array::from_fn(|i| add(rotated[i], Interval::point(pose.translation_mm[i])));
    for coordinate in result {
        finite(coordinate)?;
    }
    Ok(result)
}

fn certify_orientation(
    matrix: Matrix,
    source: PlaneFrameMm,
    target: PlaneFrameMm,
) -> Result<(), GeometryError> {
    let x = apply(matrix, source.x_axis.map(Interval::point));
    let y = apply(matrix, source.y_axis.map(Interval::point));
    let orientation = finite(dot(cross(x, y), target.z_axis.map(Interval::point)))?;
    if orientation.lo <= 0.0 {
        return Err(error(
            GeometryErrorCode::ConditioningFailure,
            "placed section does not prove material-left world orientation",
        ));
    }
    Ok(())
}

fn transform_points(
    section: &mut NativeSection,
    matrix: Matrix,
    pose: RigidPoseMm,
    world_plane: PlaneMm,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(f64, f64), GeometryError> {
    let normal = normalized(world_plane.normal)?;
    let mut arithmetic: f64 = 0.0;
    let mut plane_error: f64 = 0.0;
    for boundary in &mut section.loops {
        for point in &mut boundary.points_mm {
            work.charge(1, cancel)?;
            let enclosure = transformed_position(matrix, pose, point.map(Interval::point))?;
            let actual = forward(pose, *point);
            if actual.iter().any(|coordinate| !coordinate.is_finite()) {
                return Err(error(
                    GeometryErrorCode::ConditioningFailure,
                    "placed native section point is nonfinite",
                ));
            }
            arithmetic = arithmetic.max(
                length(std::array::from_fn(|i| {
                    sub(enclosure[i], Interval::point(actual[i]))
                }))?
                .hi,
            );
            let relative = std::array::from_fn(|i| {
                sub(
                    Interval::point(actual[i]),
                    Interval::point(world_plane.origin_mm[i]),
                )
            });
            plane_error = plane_error.max(finite(dot(normal, relative))?.abs_upper());
            *point = actual;
        }
    }
    Ok((arithmetic, plane_error))
}

fn record_world_accuracy(
    section: &mut NativeSection,
    sigma: f64,
    plane_error: f64,
    arithmetic: f64,
) -> Result<(), GeometryError> {
    let chord_error = finite(add(
        mul(
            Interval::point(sigma),
            Interval::point(section.certificate.maximum_chord_error_mm),
        ),
        Interval::point(arithmetic),
    ))?
    .hi;
    let plane_error = finite(add(
        Interval::point(plane_error),
        Interval::point(arithmetic),
    ))?
    .hi;
    if chord_error > SECTION_SAMPLING_TOLERANCE_MM || plane_error > SECTION_PLANE_TOLERANCE_MM {
        return Err(error(
            GeometryErrorCode::ShapeErrorExceeded,
            "placed native section exceeds frozen world chord or plane accuracy",
        ));
    }
    section.certificate.maximum_chord_error_mm = chord_error;
    section.certificate.maximum_plane_error_mm = plane_error;
    Ok(())
}
struct SamplePlacement<'a> {
    map: &'a HeightMap,
    pose: RigidPoseMm,
    sample: f64,
    sigma: f64,
    extent: [f64; 3],
}

// A source occupied envelope bounds the transverse projection. World XY also
// sees the axial component R*n*(h-h_sample)/|n|²; dropping it assumes exact
// orthogonality and is unsound for accepted near-unit quaternions.
fn axial_drift(
    map: &HeightMap,
    section: &NativeSection,
    source_range: [f64; 2],
    extent: [f64; 3],
) -> Result<f64, GeometryError> {
    let n = map.frame.z_axis.map(Interval::point);
    let norm_squared = dot(n, n);
    let axis = apply(map.matrix, n);
    let transverse = length([axis[0], axis[1], Interval::point(0.0)])?;
    let sample_height = dot(section.plane.origin_mm.map(Interval::point), n);
    let delta = sub(
        Interval {
            lo: source_range[0],
            hi: source_range[1],
        },
        sample_height,
    )
    .abs_upper();
    let sample_height_error = world_plane_error(
        section,
        HeightFunctional {
            direction: n,
            offset: sample_height.neg(),
        },
        extent,
    )?;
    let delta = add(Interval::point(delta), Interval::point(sample_height_error));
    Ok(finite(div(mul(transverse, delta), norm_squared)?)?.hi)
}
fn transform_occupied(
    section: &mut NativeSection,
    map: &HeightMap,
    pose: RigidPoseMm,
    sample: f64,
    drift: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let mut lower = [f64::INFINITY; 2];
    let mut upper = [f64::NEG_INFINITY; 2];
    for boundary in &section.loops {
        for point in &boundary.points_mm {
            work.charge(1, cancel)?;
            let enclosure = transformed_position(map.matrix, pose, point.map(Interval::point))?;
            for i in 0..2 {
                let expanded = add(enclosure[i], symmetric(drift));
                lower[i] = lower[i].min(expanded.lo);
                upper[i] = upper[i].max(expanded.hi);
            }
        }
    }
    // The outward world rectangle encloses the affine image as well as axial
    // drift; unlike a rounded affine corner it cannot shave the outer envelope.
    if lower.iter().all(|coordinate| coordinate.is_finite()) {
        work.charge(5, cancel)?;
        section.loops.truncate(1);
        let boundary = &mut section.loops[0];
        boundary.is_hole = false;
        boundary.points_mm.clear();
        boundary.points_mm.extend([
            [lower[0], lower[1], sample],
            [upper[0], lower[1], sample],
            [upper[0], upper[1], sample],
            [lower[0], upper[1], sample],
            [lower[0], lower[1], sample],
        ]);
    } else {
        section.loops.clear();
    }
    section.plane = PlaneMm {
        origin_mm: [0.0, 0.0, sample],
        normal: [0.0, 0.0, 1.0],
    };
    section.frame = PlaneFrameMm {
        origin_mm: section.plane.origin_mm,
        ..world_frame()
    };
    section.certificate.maximum_chord_error_mm = 0.0;
    section.certificate.maximum_plane_error_mm = 0.0;
    Ok(())
}

fn classify_endpoint_world_height(
    cover: Interval,
    constant: Option<Interval>,
    world_range: [f64; 2],
    expected_endpoint: f64,
) -> Result<bool, GeometryError> {
    let constant = constant.or_else(|| (cover.lo == cover.hi).then_some(cover));
    let cover = if let Some(height) = constant {
        finite(Interval {
            lo: cover.lo.max(height.lo),
            hi: cover.hi.min(height.hi),
        })?
    } else {
        finite(cover)?
    };
    // Closed endpoints belong to the captured slab. Equality is never enough
    // to discard material, nor may an uncertain bracket become a contact.
    if cover.hi < world_range[0] || cover.lo > world_range[1] {
        return Ok(false);
    }
    match constant {
        Some(height) if height.lo == height.hi && height.lo == expected_endpoint => Ok(true),
        Some(height) if height.lo == height.hi => Err(error(
            GeometryErrorCode::AnalysisIndeterminate,
            "native endpoint material maps to a different world-height stratum",
        )),
        _ => Err(error(
            GeometryErrorCode::AnalysisIndeterminate,
            "native endpoint material has an unresolved or positive-thickness world-height band",
        )),
    }
}

fn place_endpoint_stratum(
    stratum: &mut NativeEndpointStratum,
    definition: &Definition,
    map: &HeightMap,
    capture: &PlacedSlabCapture,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    let bounds = stratum.source_position_bounds_mm;
    let source = [
        Interval::new(bounds[0][0], bounds[0][1])?,
        Interval::new(bounds[1][0], bounds[1][1])?,
        Interval::new(bounds[2][0], bounds[2][1])?,
    ];
    let world = transformed_position(map.matrix, capture.pose, source)?;
    let range = capture.world_height_interval_mm;
    if world[2].hi < range[0] || world[2].lo > range[1] {
        return Ok(false);
    }
    let mut constant = if map.residual_mm == 0.0 && stratum.request.frame == map.frame {
        let height = map.world_value([stratum.height_mm; 2])?;
        Some(Interval {
            lo: height[0],
            hi: height[1],
        })
    } else {
        None
    };
    if world[2].lo == world[2].hi {
        constant = Some(world[2]);
    }
    if constant.is_none_or(|height| height.lo != height.hi)
        && let Some(height) = super::slab::constant_source_face_height(
            definition,
            stratum.face_index,
            HeightFunctional {
                direction: map.matrix[2],
                offset: Interval::point(capture.pose.translation_mm[2]),
            },
            work,
            cancel,
        )?
    {
        constant = Some(height);
    }
    let endpoint = range[match stratum.endpoint {
        NativeOwnedEndpoint::Lower => 0,
        NativeOwnedEndpoint::Upper => 1,
    }];
    if !classify_endpoint_world_height(world[2], constant, range, endpoint)? {
        return Ok(false);
    }
    if finite(dot(
        map.matrix[2],
        stratum.request.frame.z_axis.map(Interval::point),
    ))?
    .lo <= 0.0
    {
        return Err(error(
            GeometryErrorCode::ConditioningFailure,
            "placed endpoint material does not prove the captured outward-height orientation",
        ));
    }
    // Retain the original source height, frame and patch cover; the immutable
    // definition owns the original coedge lineage checked by the constructor.
    // Only this actual-functional proof may add the separate posed capture.
    stratum.world_xy_bounds_mm = Some([[world[0].lo, world[0].hi], [world[1].lo, world[1].hi]]);
    stratum.placed = Some(*capture);
    Ok(true)
}

fn place_endpoint_strata(
    strata: &mut Vec<NativeEndpointStratum>,
    definition: &Definition,
    map: &HeightMap,
    capture: &PlacedSlabCapture,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let mut failure = None;
    strata.retain_mut(|stratum| {
        if failure.is_some() {
            return false;
        }
        match place_endpoint_stratum(stratum, definition, map, capture, work, cancel) {
            Ok(retain) => retain,
            Err(error) => {
                failure = Some(error);
                false
            }
        }
    });
    if let Some(error) = failure {
        return Err(error);
    }
    Ok(())
}

pub(crate) fn analyze_placed_slab(
    source: &crate::PreparedMaterialSource<'_>,
    request: &PlacedSlabRequest,
    query_byte_limit: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SlabAnalysis, GeometryError> {
    work.charge(1, cancel)?;
    let query_byte_limit = query_byte_limit.min(
        crate::MAX_PREPARED_SECTION_BYTES
            .checked_sub(source.retained_bytes())
            .ok_or_else(|| {
                error(
                    GeometryErrorCode::ResourceLimit,
                    "placed slab source exceeds shared capacity",
                )
            })?,
    );
    let coexisting_bytes = crate::MAX_PREPARED_SECTION_BYTES - query_byte_limit;
    validate_range(request.world_height_interval_mm)?;
    if !request.sample_world_z_mm.is_finite()
        || request.sample_world_z_mm <= request.world_height_interval_mm[0]
        || request.sample_world_z_mm >= request.world_height_interval_mm[1]
        || !request.native_shape_error_mm.is_finite()
        || request.native_shape_error_mm <= 0.0
    {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "placed slab requires an interior finite sample and positive finite shape error",
        ));
    }
    let map = HeightMap::new(source.definition(), request.pose)?;
    let range = map.source_range(request.world_height_interval_mm)?;
    let sample = div(
        sub(
            Interval::point(request.sample_world_z_mm),
            Interval::point(request.pose.translation_mm[2]),
        ),
        Interval::point(map.scale),
    )?
    .mid();
    let sigma = spectral_bound(map.matrix)?;
    let source_policy = div(
        Interval::point(request.native_shape_error_mm),
        Interval::point(sigma),
    )?
    .lo;
    if !source_policy.is_finite()
        || source_policy <= 0.0
        || !sample.is_finite()
        || sample <= range[0]
        || sample >= range[1]
    {
        return Err(error(
            GeometryErrorCode::ConditioningFailure,
            "placed slab policy or sample cannot be resolved in the source frame",
        ));
    }
    let source_request = SlabRequest {
        frame: map.frame,
        height_interval_mm: range,
        sample_height_mm: sample,
        native_shape_error_mm: source_policy,
    };
    let mut prepared =
        super::slab::prepare_slab_sample(source, source_request, query_byte_limit, work, cancel)?;
    let placement = SamplePlacement {
        map: &map,
        pose: request.pose,
        sample: request.sample_world_z_mm,
        sigma,
        extent: source_extent(source.definition()),
    };
    let fixed = coexisting_bytes
        .saturating_add(prepared.section.retained_bytes())
        .saturating_add(source.definition().id().allocated_bytes())
        .saturating_add(ActualMaterialCoordinates::SCRATCH_BYTES)
        .saturating_add(std::mem::size_of_val(&prepared));
    let world_candidate = WorldSampleCandidate::prepare(
        &prepared.section,
        &placement,
        super::slab::remaining_scratch(fixed)?,
        work,
        cancel,
    )?;
    let candidate_bytes = world_candidate.retained_bytes();
    prepared.source_retained_bytes = prepared
        .source_retained_bytes
        .checked_add(candidate_bytes)
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "coexisting world candidate capacity overflow",
            )
        })?;
    // Preserve the cheap exact-source continuation lane. A source with
    // nonzero correspondence uncertainty cannot acquire exact invariance;
    // try its complete actual-coordinate radial proof before the expensive
    // sufficient face cover. Annuli also avoid a meridian-convexity premise.
    // This only orders proof attempts, never authorizes a geometry family.
    let radial_first = !prepared.material_transport_zero
        || prepared.section.loops.len() == 2
            && prepared
                .section
                .loops
                .iter()
                .filter(|ring| ring.is_hole)
                .count()
                == 1;
    let source_proof = if radial_first {
        None
    } else {
        Some(
            match super::slab::analyze_slab_sample(source, &prepared, work, cancel) {
                Ok(proof) => Ok(proof),
                Err(failure)
                    if matches!(
                        failure.code,
                        GeometryErrorCode::AnalysisIndeterminate
                            | GeometryErrorCode::ShapeErrorExceeded
                            | GeometryErrorCode::ParameterCorrespondenceUnavailable
                    ) =>
                {
                    Err(failure)
                }
                Err(failure) => return Err(failure),
            },
        )
    };
    let needs_radial_metric = source_proof.as_ref().is_none_or(|proof| match proof {
        Ok(proof) => proof.has_transition_material(),
        Err(_) => true,
    });
    let proof_inline_bytes = std::mem::size_of_val(&source_proof);
    let radial_fixed = fixed.saturating_add(proof_inline_bytes).saturating_add(
        source_proof
            .as_ref()
            .and_then(|proof| proof.as_ref().ok())
            .map_or(0, |proof| proof.retained_bytes()),
    );
    let radial_shape_failure = if needs_radial_metric {
        let coordinates = ActualMaterialCoordinates::placed(
            source.definition(),
            &map,
            request.world_height_interval_mm,
            request.sample_world_z_mm,
            work,
            cancel,
        )?;
        let radial = if prepared.section.loops.is_empty() {
            Ok(None)
        } else {
            super::joint::certify_closed_radial_slab(
                source.trim_source(),
                &source_request,
                &coordinates,
                &world_candidate.loops,
                request.native_shape_error_mm,
                super::slab::remaining_scratch(
                    radial_fixed.saturating_add(world_candidate.retained_bytes()),
                )?,
                work,
                cancel,
            )
        };
        match radial {
            Ok(Some(radial)) => {
                // Only a COMPLETE topology+filled-metric proof wins. Keep the
                // unchanged raw sample available for the other sufficient lane.
                drop(source_proof);
                let mut capture = super::SlabCapture::new(
                    source.definition(),
                    source_request,
                    &prepared.section,
                    work,
                    cancel,
                )?;
                let band = capture.source_sample_height_band_mm;
                if band[0] < range[0] || band[1] > range[1] {
                    return Err(error(
                        GeometryErrorCode::AnalysisIndeterminate,
                        "actual native sample plane is not inside the captured slab",
                    ));
                }
                let transform_limit = query_byte_limit
                    .checked_sub(
                        capture.definition_id.allocated_bytes()
                            + ActualMaterialCoordinates::SCRATCH_BYTES
                            + std::mem::size_of_val(&radial)
                            + std::mem::size_of_val(&prepared)
                            + proof_inline_bytes
                            + std::mem::size_of::<WorldSampleCandidate>(),
                    )
                    .ok_or_else(|| {
                        error(
                            GeometryErrorCode::ResourceLimit,
                            "closed radial sample commit exceeds coexisting capacity",
                        )
                    })?;
                world_candidate.commit(
                    &mut prepared.section,
                    &placement,
                    transform_limit,
                    work,
                    cancel,
                )?;
                let bound = radial.bound_mm();
                let occupied_bytes = coexisting_bytes
                    .saturating_add(prepared.section.retained_bytes())
                    .saturating_add(capture.definition_id.allocated_bytes())
                    .saturating_add(ActualMaterialCoordinates::SCRATCH_BYTES)
                    .saturating_add(std::mem::size_of_val(&radial))
                    .saturating_add(std::mem::size_of_val(&prepared))
                    .saturating_add(proof_inline_bytes);
                let occupied = super::slab::occupied_envelope(
                    &prepared.section,
                    bound,
                    super::slab::remaining_scratch(occupied_bytes)?,
                    work,
                    cancel,
                )?;
                capture.placed = Some(PlacedSlabCapture {
                    pose: request.pose,
                    world_height_interval_mm: request.world_height_interval_mm,
                    sample_world_z_mm: request.sample_world_z_mm,
                    native_shape_error_mm: request.native_shape_error_mm,
                });
                return Ok(SlabAnalysis {
                    section: prepared.section,
                    occupied: Some(occupied),
                    boundary_motion_bound_mm: bound,
                    invariance: None,
                    critical: None,
                    endpoint_strata: Vec::new(),
                    collar_complex: None,
                    radial_slab: Some(radial),
                    capture,
                });
            }
            Ok(None) => None,
            Err(failure) if failure.code == GeometryErrorCode::ShapeErrorExceeded => Some(failure),
            Err(failure)
                if matches!(
                    failure.code,
                    GeometryErrorCode::AnalysisIndeterminate
                        | GeometryErrorCode::ParameterCorrespondenceUnavailable
                ) =>
            {
                None
            }
            Err(failure) => return Err(failure),
        }
    } else {
        None
    };
    let coexisting_bytes = coexisting_bytes.saturating_add(candidate_bytes);
    let source_proof = match source_proof {
        Some(proof) => proof,
        None => super::slab::analyze_slab_sample(source, &prepared, work, cancel),
    };
    let mut result = match source_proof {
        Ok(proof) => proof.into_analysis(prepared.section),
        Err(failure)
            if matches!(
                failure.code,
                GeometryErrorCode::AnalysisIndeterminate
                    | GeometryErrorCode::ShapeErrorExceeded
                    | GeometryErrorCode::ParameterCorrespondenceUnavailable
            ) =>
        {
            return Err(radial_shape_failure.unwrap_or(failure));
        }
        Err(failure) => return Err(failure),
    };
    if !result.matches_original_material(source.definition())
        || result.endpoint_strata.iter().any(|stratum| {
            stratum.material_realization_identity() != result.material_realization_identity()
        })
    {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "placed slab cannot change its original material realization",
        ));
    }
    let placed = PlacedSlabCapture {
        pose: request.pose,
        world_height_interval_mm: request.world_height_interval_mm,
        sample_world_z_mm: request.sample_world_z_mm,
        native_shape_error_mm: request.native_shape_error_mm,
    };
    place_endpoint_strata(
        &mut result.endpoint_strata,
        source.definition(),
        &map,
        &placed,
        work,
        cancel,
    )?;
    if let Some(seed) = result.collar_complex.as_ref() {
        // A nominal source foliation does not certify actual world critical
        // incidence. Retain the complete native event-face lineage, but prove
        // its posed world band and regular complement before exposing it.
        let remaining = super::slab::remaining_scratch(
            coexisting_bytes.saturating_add(result.retained_bytes()),
        )?;
        let actual = super::collar::place_native_collar_complex(
            source.trim_source(),
            super::collar::PlacedCollarComplexQuery {
                seed,
                pose: request.pose,
                interval: request.world_height_interval_mm,
                sample: request.sample_world_z_mm,
            },
            remaining,
            work,
            cancel,
        )?;
        result.collar_complex = Some(actual);
    }
    let extent = source_extent(source.definition());
    let drift = axial_drift(&map, &result.section, range, extent)?;
    let section_byte_limit = query_byte_limit
        .checked_sub(
            result
                .retained_bytes()
                .saturating_sub(result.section.retained_bytes())
                .saturating_add(std::mem::size_of::<WorldSampleCandidate>()),
        )
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "placed slab leaves no native section transform capacity",
            )
        })?;
    let arithmetic = world_candidate.commit(
        &mut result.section,
        &placement,
        section_byte_limit,
        work,
        cancel,
    )?;
    let bound = finite(add(
        add(
            mul(
                Interval::point(sigma),
                Interval::point(result.boundary_motion_bound_mm),
            ),
            Interval::point(drift),
        ),
        Interval::point(arithmetic),
    ))?
    .hi;
    if bound > request.native_shape_error_mm {
        return Err(error(
            GeometryErrorCode::ShapeErrorExceeded,
            "placed slab shape bound exceeds the requested world policy",
        ));
    }
    if let Some(occupied) = &mut result.occupied {
        transform_occupied(
            occupied,
            &map,
            request.pose,
            request.sample_world_z_mm,
            drift,
            work,
            cancel,
        )?;
    } else {
        return Err(error(
            GeometryErrorCode::AnalysisIndeterminate,
            "placed slab lacks a conservative occupied projection",
        ));
    }
    result.invariance = placed_invariance(
        result.invariance,
        &map,
        request.pose,
        range,
        request.world_height_interval_mm,
    );
    result.boundary_motion_bound_mm = bound;
    result.capture.placed = Some(placed);
    if result.closed_shape_error_mm().is_some()
        && !result.section.loops.is_empty()
        && result.invariance.is_none()
        && !result.has_native_convex_section_topology_proof()
    {
        let coordinates = ActualMaterialCoordinates::placed(
            source.definition(),
            &map,
            request.world_height_interval_mm,
            request.sample_world_z_mm,
            work,
            cancel,
        )?;
        let remaining = super::slab::remaining_scratch(
            coexisting_bytes
                .saturating_sub(candidate_bytes)
                .saturating_add(result.retained_bytes())
                .saturating_add(ActualMaterialCoordinates::SCRATCH_BYTES),
        )?;
        if let Some(product) =
            super::critical::material_link::certify_closed_radial_material_product(
                source.trim_source(),
                &source_request,
                &coordinates,
                remaining,
                work,
                cancel,
            )?
        {
            result.radial_slab = super::joint::seal_closed_radial_slab_from_complete_metric(
                source.definition(),
                &result,
                product,
                remaining,
                work,
                cancel,
            )?;
        }
    }
    Ok(result)
}

fn plane_functional(
    matrix: Matrix,
    pose: RigidPoseMm,
    plane: PlaneMm,
) -> Result<HeightFunctional, GeometryError> {
    let direction = normalized(plane.normal)?;
    let local = std::array::from_fn(|i| dot(std::array::from_fn(|j| matrix[j][i]), direction));
    let relative = std::array::from_fn(|i| {
        sub(
            Interval::point(pose.translation_mm[i]),
            Interval::point(plane.origin_mm[i]),
        )
    });
    let offset = finite(dot(direction, relative))?;
    Ok(HeightFunctional {
        direction: local,
        offset,
    })
}

fn nominal_source_plane(functional: HeightFunctional) -> Result<PlaneMm, GeometryError> {
    let nominal = functional.direction.map(Interval::mid);
    let normal = PlaneMm {
        origin_mm: [0.0; 3],
        normal: nominal,
    }
    .normalized()?
    .normal;
    let denominator = dot(nominal.map(Interval::point), normal.map(Interval::point));
    if denominator.lo <= 0.0 {
        return Err(error(
            GeometryErrorCode::ConditioningFailure,
            "placed section source covector has no resolved positive scale",
        ));
    }
    // This is a scalar plane-equation solve, not R^T applied to a point. The
    // latter is not an inverse for an accepted near-unit quaternion.
    let factor = finite(div(functional.offset.neg(), denominator)?)?.mid();
    let origin_mm =
        normal.map(|coordinate| mul(Interval::point(coordinate), Interval::point(factor)).mid());
    let plane = PlaneMm { origin_mm, normal };
    plane.validate()?;
    Ok(plane)
}

fn source_plane_functional(
    section: &NativeSection,
    target: HeightFunctional,
) -> Result<HeightFunctional, GeometryError> {
    let normal = section.plane.normal.map(Interval::point);
    let scale = finite(div(
        dot(
            target.direction.map(|value| Interval::point(value.mid())),
            normal,
        ),
        dot(normal, normal),
    )?)?
    .mid();
    if scale <= 0.0 {
        return Err(error(
            GeometryErrorCode::ConditioningFailure,
            "placed section source covector scale is not positive",
        ));
    }
    let direction = normal.map(|coefficient| mul(Interval::point(scale), coefficient));
    let offset = dot(direction, section.plane.origin_mm.map(Interval::point)).neg();
    Ok(HeightFunctional { direction, offset })
}

fn world_plane_error(
    section: &NativeSection,
    target: HeightFunctional,
    extent: [f64; 3],
) -> Result<f64, GeometryError> {
    let source = source_plane_functional(section, target)?;
    let residual = std::array::from_fn(|i| sub(target.direction[i], source.direction[i]));
    let coefficient_reserve = coefficient_error(residual, [0.0; 3], extent)?;
    let offset_reserve = sub(target.offset, source.offset).abs_upper();
    let native_reserve = mul(
        length(source.direction)?,
        Interval::point(section.certificate.maximum_plane_error_mm),
    );
    Ok(finite(add(
        add(
            Interval::point(coefficient_reserve),
            Interval::point(offset_reserve),
        ),
        native_reserve,
    ))?
    .hi)
}

fn identical_functional(a: HeightFunctional, b: HeightFunctional) -> bool {
    a.direction
        .into_iter()
        .zip(b.direction)
        .chain([(a.offset, b.offset)])
        .all(|(a, b)| a.lo == a.hi && b.lo == b.hi && a.lo == b.lo)
}

fn certify_plane_homotopy(
    trim_source: crate::section::OriginalTrimSource<'_, '_>,
    section: &NativeSection,
    target: HeightFunctional,
    query_byte_limit: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let source = source_plane_functional(section, target)?;
    if identical_functional(source, target) {
        return Ok(());
    }
    // The coefficient hull covers every plane along the affine homotopy,
    // not just the nominal and final roots. Metric residual alone cannot rule
    // out an intervening branch birth at a physical trim or critical surface.
    let family = HeightFunctional {
        direction: std::array::from_fn(|i| source.direction[i].hull(target.direction[i])),
        offset: source.offset.hull(target.offset),
    };
    let tolerance = spiling_contracts::geometry::SECTION_BOOLEAN_TOLERANCE_MM;
    // The caller's query allowance already excludes every retained prepared
    // source, including this definition's pinned continuation. Only the actual
    // returned section still coexists with this homotopy's scratch buffers.
    let remaining = query_byte_limit
        .checked_sub(section.retained_bytes())
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "placed section leaves no plane-homotopy capacity",
            )
        })?;
    let analysis = super::slab::analyze_height_domain_enclosed_with_scratch_limit(
        &super::slab::HeightDomainQuery {
            source: trim_source,
            frame: PlaneFrameMm::from_plane(section.plane)?,
            functional: family,
            height_interval_mm: [-tolerance, tolerance],
            policy: AnalysisPolicy {
                maximum_height_uncertainty_mm: tolerance,
                maximum_shape_error_mm: SECTION_SAMPLING_TOLERANCE_MM,
            },
        },
        remaining,
        work,
        cancel,
    )?;
    if !analysis.event_brackets_mm.is_empty() {
        return Err(error(
            GeometryErrorCode::AnalysisIndeterminate,
            "placed section plane homotopy meets a native critical stratum",
        ));
    }
    Ok(())
}

/// Intersect a borrowed native source with an arbitrary world plane. Only its
/// covector is pulled back; no assumed quaternion inverse transforms points.
/// Stored output points remain actual transformed native intersection points.
pub(crate) fn section_placed_with_work(
    source: &PreparedSectionSource<'_>,
    pose: RigidPoseMm,
    plane: PlaneMm,
    query_byte_limit: u32,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<NativeSection, GeometryError> {
    work.charge(1, cancel)?;
    let query_byte_limit = query_byte_limit.min(
        crate::MAX_PREPARED_SECTION_BYTES
            .checked_sub(source.retained_bytes())
            .ok_or_else(|| {
                error(
                    GeometryErrorCode::ResourceLimit,
                    "placed section source exceeds shared capacity",
                )
            })? as u32,
    );
    plane.validate()?;
    let matrix = rotation(pose)?;
    let target = plane_functional(matrix, pose, plane)?;
    let local_plane = nominal_source_plane(target)?;
    let mut section =
        source.section_with_byte_limit(local_plane, cancel, query_byte_limit, work)?;
    let declared_plane = plane.normalized()?;
    let declared_frame = PlaneFrameMm::from_plane(declared_plane)?;
    let declared_functional = plane_functional(matrix, pose, declared_plane)?;
    let family = HeightFunctional {
        direction: std::array::from_fn(|i| {
            target.direction[i].hull(declared_functional.direction[i])
        }),
        offset: target.offset.hull(declared_functional.offset),
    };
    certify_plane_homotopy(
        source.trim_source(),
        &section,
        family,
        query_byte_limit as usize,
        work,
        cancel,
    )?;
    certify_orientation(matrix, section.frame, declared_frame)?;
    let extent = source_extent(source.definition());
    // Normalizing the public plane rounds its coefficients. Include residuals
    // against both the original query and the declared normalized metadata.
    let requested_error = world_plane_error(&section, target, extent)?;
    let declared_error = world_plane_error(&section, declared_functional, extent)?;
    let sigma = spectral_bound(matrix)?;
    let (arithmetic, point_plane_error) =
        transform_points(&mut section, matrix, pose, declared_plane, work, cancel)?;
    record_world_accuracy(
        &mut section,
        sigma,
        requested_error.max(declared_error).max(point_plane_error),
        arithmetic,
    )?;
    section.plane = declared_plane;
    section.frame = declared_frame;
    crate::section::canonicalize_section_in_place(&mut section, query_byte_limit, work, cancel)?;
    Ok(section)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placed_material_bounds_retain_source_recipe_pose_and_terminal_brackets() {
        let cancel = AtomicBool::new(false);
        let mut definition = crate::import_step(
            include_bytes!("../../../../fixtures/geometry/box-mm.step"),
            &cancel,
        )
        .unwrap();
        let pose = RigidPoseMm {
            translation_mm: [2.0, -3.0, 4.0],
            ..RigidPoseMm::IDENTITY
        };
        let direction = [0.0, 0.0, 3.0];
        let policy = BoundsPolicy {
            maximum_endpoint_uncertainty_mm: 1.0e-7,
        };
        let mut work = GeometryWorkBudget::new(5_000_000);
        let material =
            placed_material_bounds(&definition, pose, direction, policy, &mut work, &cancel)
                .unwrap();
        let literal =
            placed_bounds(&definition, pose, direction, policy, &mut work, &cancel).unwrap();
        assert_eq!(material.definition_id(), definition.id());
        assert_eq!(material.placed_pose(), pose);
        assert_eq!(material.world_direction(), direction);
        assert_eq!(
            material.policy().maximum_endpoint_uncertainty_mm,
            policy.maximum_endpoint_uncertainty_mm
        );
        assert!(material.is_compatible_with_definition(&definition));
        assert!(material.scalar_transport_is_identically_zero());
        assert_eq!(material.source_realization_error_mm(), 0.0);
        let bounds = material.bounds();
        assert!(bounds.minimum[0] <= literal.minimum[1] && literal.minimum[0] <= bounds.minimum[1]);
        assert!(bounds.maximum[0] <= literal.maximum[1] && literal.maximum[0] <= bounds.maximum[1]);
        assert!(bounds.achieved_uncertainty_mm <= policy.maximum_endpoint_uncertainty_mm);
        assert!(material.retained_bytes() >= std::mem::size_of::<PlacedMaterialProjectedBounds>());
        assert_eq!(
            placed_material_bounds(
                &definition,
                pose,
                direction,
                policy,
                &mut GeometryWorkBudget::new(0),
                &cancel,
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::ResourceLimit,
        );
        assert_eq!(
            placed_material_bounds(
                &definition,
                pose,
                direction,
                policy,
                &mut work,
                &AtomicBool::new(true),
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::Cancelled,
        );
        definition.provenance.uncertainty_mm = if definition.provenance.uncertainty_mm.is_some() {
            None
        } else {
            Some(0.0)
        };
        assert!(!material.is_compatible_with_definition(&definition));
    }

    #[test]
    fn exact_operations_preserve_only_actual_singletons() {
        assert_eq!(add(Interval::point(4.0), Interval::point(8.0)).lo, 12.0);
        assert_eq!(add(Interval::point(4.0), Interval::point(8.0)).hi, 12.0);
        let rounded = add(Interval::point(1.0), Interval::point(f64::EPSILON * 0.5));
        assert!(rounded.lo < rounded.hi);
        let tiny = mul(Interval::point(0.25), Interval::point(f64::from_bits(1)));
        assert!(tiny.lo < tiny.hi);
        let product = mul(Interval::point(0.1), Interval::point(0.1));
        assert!(product.lo < product.hi);
        // The exact product has a half-subnormal residual: FMA rounds that
        // residual to zero, which must not masquerade as an exact operation.
        let almost_subnormal = mul(
            Interval::point(2.0_f64.powi(-511)),
            Interval::point(2.0_f64.powi(-512) * (1.0 + f64::EPSILON)),
        );
        assert!(almost_subnormal.lo < almost_subnormal.hi);
    }

    #[test]
    fn identity_and_signed_axes_are_exact_polynomial_rotations() {
        for q in [
            [0.0, 0.0, 0.0, 1.0],
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.5, 0.5, 0.5, 0.5],
            [0.5, -0.5, 0.5, 0.5],
        ] {
            let matrix = rotation(RigidPoseMm {
                rotation_xyzw: q,
                ..RigidPoseMm::IDENTITY
            })
            .unwrap();
            for row in matrix {
                for value in row {
                    assert_eq!(value.lo, value.hi);
                }
            }
            assert_eq!(spectral_bound(matrix).unwrap(), 1.0);
        }
    }

    #[test]
    fn yaw_preserves_exact_world_height_despite_transverse_rounding() {
        let pose = RigidPoseMm {
            rotation_xyzw: [0.0, 0.0, 0.3_f64.sin(), 0.3_f64.cos()],
            ..RigidPoseMm::IDENTITY
        };
        let matrix = rotation(pose).unwrap();
        for (coefficient, expected) in matrix[2].into_iter().zip([0.0, 0.0, 1.0]) {
            assert_eq!([coefficient.lo, coefficient.hi], [expected, expected]);
        }
        let event = add(
            mul(Interval::point(1.0), Interval::point(12.0)),
            Interval::point(3.0),
        );
        assert_eq!([event.lo, event.hi], [15.0, 15.0]);
    }

    #[test]
    fn near_unit_rotation_is_not_silently_orthogonalized() {
        let half = std::f64::consts::FRAC_1_SQRT_2;
        let pose = RigidPoseMm {
            rotation_xyzw: [half * (1.0 + 2e-11), 0.0, 0.0, half * (1.0 + 2e-11)],
            ..RigidPoseMm::IDENTITY
        };
        let matrix = rotation(pose).unwrap();
        assert!(spectral_bound(matrix).unwrap() > 1.0);
        assert!(matrix[2][2].hi < 0.0);
        let axis = apply(
            matrix,
            [
                Interval::point(0.0),
                Interval::point(0.0),
                Interval::point(1.0),
            ],
        );
        assert!(axis[1].abs_upper() > 0.9);
    }

    #[test]
    fn quaternion_polynomial_encloses_actual_forward_arithmetic_error() {
        let pose = RigidPoseMm {
            translation_mm: [1e6, -7.0, 9.0],
            rotation_xyzw: [0.2, -0.3, 0.1, 0.86_f64.sqrt()],
        };
        let matrix = rotation(pose).unwrap();
        let point = [12.0, -5.0, 2.0];
        let output = forward(pose, point);
        let enclosure = transformed_position(matrix, pose, point.map(Interval::point)).unwrap();
        let rounding = length(std::array::from_fn(|i| {
            sub(enclosure[i], Interval::point(output[i]))
        }))
        .unwrap()
        .hi;
        assert!(rounding.is_finite());
        assert!(rounding < 1e-7);
    }

    #[test]
    fn source_height_band_includes_coefficient_and_translation_error() {
        let pose = RigidPoseMm {
            translation_mm: [0.0, 0.0, 3.0],
            ..RigidPoseMm::IDENTITY
        };
        let map = HeightMap {
            pose,
            matrix: rotation(RigidPoseMm::IDENTITY).unwrap(),
            frame: world_frame(),
            scale: 2.0,
            residual_mm: 0.25,
        };
        let source = map.source_range([7.0, 11.0]).unwrap();
        assert!(source[0] <= 1.875 && source[1] >= 4.125);
        // An uncertain critical bracket remains a bracket, never its midpoint.
        let event = map.world_value([2.0, 2.125]).unwrap();
        assert!(event[0] <= 7.0 && event[1] >= 7.25 && event[0] < event[1]);
    }

    #[test]
    fn normalized_world_support_retains_nonunit_pullback_scale() {
        let d = normalized([2.0, 0.0, 0.0]).unwrap();
        assert_eq!([d[0].lo, d[0].hi], [1.0, 1.0]);
        let pulled = [
            Interval::point(2.0),
            Interval::point(0.0),
            Interval::point(0.0),
        ];
        assert_eq!(length(pulled).unwrap().lo, 2.0);
        assert_eq!(mul(length(pulled).unwrap(), Interval::point(7.0)).hi, 14.0);
    }

    fn square_section() -> NativeSection {
        NativeSection {
            plane: PlaneMm {
                origin_mm: [0.0, 0.0, 2.0],
                normal: [0.0, 0.0, 1.0],
            },
            frame: PlaneFrameMm {
                origin_mm: [0.0, 0.0, 2.0],
                ..world_frame()
            },
            loops: vec![crate::SectionLoopMm {
                is_hole: false,
                points_mm: vec![
                    [0.0, 0.0, 2.0],
                    [2.0, 0.0, 2.0],
                    [2.0, 2.0, 2.0],
                    [0.0, 2.0, 2.0],
                    [0.0, 0.0, 2.0],
                ],
            }],
            loop_source_faces: vec![vec![0]],
            sampling_tolerance_mm: SECTION_SAMPLING_TOLERANCE_MM,
            boolean_tolerance_mm: spiling_contracts::geometry::SECTION_BOOLEAN_TOLERANCE_MM,
            certificate: crate::NativeSectionCertificate {
                definition_id: spiling_contracts::geometry::DefinitionId::from_source_sha256(
                    &[0; 32],
                ),
                authority: crate::SectionAuthority::NativeIntersection,
                maximum_chord_error_mm: 1e-5,
                maximum_plane_error_mm: 0.0,
            },
        }
    }

    #[test]
    fn transformed_native_points_are_not_projected_onto_the_declared_plane() {
        let mut section = square_section();
        let pose = RigidPoseMm {
            translation_mm: [4.0, -7.0, 3.0],
            ..RigidPoseMm::IDENTITY
        };
        let map = HeightMap {
            pose,
            matrix: rotation(pose).unwrap(),
            frame: world_frame(),
            scale: 1.0,
            residual_mm: 0.0,
        };
        let declared = 5.0 + SECTION_PLANE_TOLERANCE_MM * 0.25;
        let mut work = GeometryWorkBudget::new(1_000);
        let placement = SamplePlacement {
            map: &map,
            pose,
            sample: declared,
            sigma: 1.0,
            extent: [2.0; 3],
        };
        WorldSampleCandidate::prepare(
            &section,
            &placement,
            spiling_contracts::geometry::MAX_SECTION_BYTES as usize,
            &mut work,
            &AtomicBool::new(false),
        )
        .unwrap()
        .commit(
            &mut section,
            &placement,
            spiling_contracts::geometry::MAX_SECTION_BYTES as usize,
            &mut work,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(section.is_native_intersection());
        assert_eq!(section.loops[0].points_mm[0], [4.0, -7.0, 5.0]);
        assert_eq!(section.plane.origin_mm[2], declared);
        assert!(section.achieved_plane_error_mm() > 0.0);
        assert!(section.achieved_plane_error_mm() <= SECTION_PLANE_TOLERANCE_MM);
        assert_eq!(section.sampling_tolerance_mm, SECTION_SAMPLING_TOLERANCE_MM);
    }

    #[test]
    fn occupied_projection_includes_axial_drift_and_retains_outer_authority() {
        let mut occupied = square_section();
        occupied.certificate.authority = crate::SectionAuthority::ConservativeSlabEnvelope;
        let pose = RigidPoseMm {
            translation_mm: [4.0, -7.0, 3.0],
            ..RigidPoseMm::IDENTITY
        };
        let map = HeightMap {
            pose,
            matrix: rotation(pose).unwrap(),
            frame: world_frame(),
            scale: 1.0,
            residual_mm: 0.0,
        };
        let mut work = GeometryWorkBudget::new(1_000);
        transform_occupied(
            &mut occupied,
            &map,
            pose,
            5.25,
            0.5,
            &mut work,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(!occupied.is_native_intersection());
        assert_eq!(occupied.loops[0].points_mm[0], [3.5, -7.5, 5.25]);
        assert_eq!(occupied.loops[0].points_mm[2], [6.5, -4.5, 5.25]);
        assert!(
            occupied.loops[0]
                .points_mm
                .iter()
                .all(|point| point[2] == 5.25)
        );
        assert_eq!(occupied.achieved_plane_error_mm(), 0.0);
    }

    #[test]
    fn near_unit_build_direction_drift_is_not_an_invariance_certificate() {
        let angle = 0.4_f64;
        let factor = 1.0 + 2e-11;
        let pose = RigidPoseMm {
            rotation_xyzw: [angle.sin() * factor, 0.0, 0.0, angle.cos() * factor],
            ..RigidPoseMm::IDENTITY
        };
        let matrix = rotation(pose).unwrap();
        let frame = PlaneFrameMm::from_plane(PlaneMm {
            origin_mm: [0.0; 3],
            normal: matrix[2].map(Interval::mid),
        })
        .unwrap();
        let map = HeightMap {
            pose,
            matrix,
            frame,
            scale: 1.0,
            residual_mm: 0.0,
        };
        assert!(!map.vertically_invariant());
        assert!(axial_drift(&map, &square_section(), [0.0, 4.0], [2.0; 3]).unwrap() > 0.0);
    }

    #[test]
    fn placed_invariance_never_widens_a_narrow_source_domain() {
        let pose = RigidPoseMm {
            translation_mm: [0.0, 0.0, 3.0],
            ..RigidPoseMm::IDENTITY
        };
        let map = HeightMap {
            pose,
            matrix: rotation(pose).unwrap(),
            frame: world_frame(),
            scale: 1.0,
            residual_mm: 0.0,
        };
        let certificate = InvariantHeightDomain {
            definition_id: spiling_contracts::geometry::DefinitionId::from_source_sha256(&[0; 32]),
            material_realization: crate::kernel::MaterialRealizationIdentity::with_allowance(None),
            frame: world_frame(),
            height_interval_mm: [0.0, 2.0],
            pose: None,
        };
        let range = [2.0, 6.0];
        let source_range = map.source_range(range).unwrap();
        let placed =
            placed_invariance(Some(certificate.clone()), &map, pose, source_range, range).unwrap();
        assert_eq!(placed.height_interval_mm(), [3.0, 5.0]);
        assert_eq!(placed.pose(), Some(pose));

        // The inverse enclosure covers both CLOSED world endpoints. Its exact
        // containment is sufficient even if forward interval arithmetic would
        // round a mapped endpoint back into the requested interval.
        let complete = InvariantHeightDomain {
            height_interval_mm: source_range,
            ..certificate
        };
        let placed = placed_invariance(Some(complete), &map, pose, source_range, range).unwrap();
        assert_eq!(placed.height_interval_mm(), range);
        assert_eq!(placed.pose(), Some(pose));
    }

    #[test]
    fn world_plane_policy_failure_is_typed_without_inflating_metadata() {
        let mut section = square_section();
        let map = HeightMap {
            pose: RigidPoseMm::IDENTITY,
            matrix: rotation(RigidPoseMm::IDENTITY).unwrap(),
            frame: world_frame(),
            scale: 1.0,
            residual_mm: 0.0,
        };
        let mut work = GeometryWorkBudget::new(1_000);
        let placement = SamplePlacement {
            map: &map,
            pose: RigidPoseMm::IDENTITY,
            sample: 2.0 + 2.0 * SECTION_PLANE_TOLERANCE_MM,
            sigma: 1.0,
            extent: [2.0; 3],
        };
        let failure = WorldSampleCandidate::prepare(
            &section,
            &placement,
            spiling_contracts::geometry::MAX_SECTION_BYTES as usize,
            &mut work,
            &AtomicBool::new(false),
        )
        .unwrap()
        .commit(
            &mut section,
            &placement,
            spiling_contracts::geometry::MAX_SECTION_BYTES as usize,
            &mut work,
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(failure.code, GeometryErrorCode::ShapeErrorExceeded);
        assert_eq!(section.sampling_tolerance_mm, SECTION_SAMPLING_TOLERANCE_MM);
    }

    #[test]
    fn arbitrary_world_plane_pulls_back_a_covector_not_a_point_inverse() {
        let pose = RigidPoseMm {
            translation_mm: [4.0, -3.0, 7.0],
            rotation_xyzw: [0.5, 0.5, 0.5, 0.5],
        };
        let matrix = rotation(pose).unwrap();
        let plane = PlaneMm {
            origin_mm: [6.0, -3.0, 7.0],
            normal: [7.0, 0.0, 0.0],
        };
        let functional = plane_functional(matrix, pose, plane).unwrap();
        assert!(identical_functional(
            functional,
            HeightFunctional {
                direction: [
                    Interval::point(0.0),
                    Interval::point(0.0),
                    Interval::point(1.0)
                ],
                offset: Interval::point(-2.0),
            }
        ));
        let local = nominal_source_plane(functional).unwrap();
        assert_eq!(local.origin_mm, [0.0, 0.0, 2.0]);
        let section = square_section();
        assert_eq!(
            world_plane_error(&section, functional, [2.0; 3]).unwrap(),
            0.0
        );
        certify_orientation(
            matrix,
            section.frame,
            PlaneFrameMm::from_plane(plane).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn equal_uncertain_functionals_do_not_skip_homotopy_exclusion() {
        let uncertain = HeightFunctional {
            direction: [
                Interval::point(0.0),
                Interval::point(0.0),
                Interval {
                    lo: 0.999,
                    hi: 1.001,
                },
            ],
            offset: Interval::point(-2.0),
        };
        assert!(!identical_functional(uncertain, uncertain));
    }

    #[test]
    fn arbitrary_placed_native_plane_reports_world_metrics_and_canonical_order() {
        let cancel = AtomicBool::new(false);
        let definition = crate::import_step(
            include_bytes!("../../../../fixtures/geometry/box-mm.step"),
            &cancel,
        )
        .unwrap();
        let mut work = GeometryWorkBudget::new(40_000_000);
        let source = crate::prepare_section_source(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &cancel,
            &mut work,
        )
        .unwrap();
        let pose = RigidPoseMm {
            translation_mm: [13.0, -7.0, 9.0],
            rotation_xyzw: [0.2, -0.3, 0.1, 0.86_f64.sqrt()],
        };
        let plane = PlaneMm {
            origin_mm: forward(pose, [10.0, 5.0, 4.0]),
            normal: [1.0, 2.0, 3.0],
        };
        let first = section_placed_with_work(
            &source,
            pose,
            plane,
            spiling_contracts::geometry::MAX_SECTION_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let second = section_placed_with_work(
            &source,
            pose,
            plane,
            spiling_contracts::geometry::MAX_SECTION_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(first.is_native_intersection());
        assert!(!first.loops.is_empty());
        assert!(first.achieved_chord_error_mm() <= SECTION_SAMPLING_TOLERANCE_MM);
        assert!(first.achieved_plane_error_mm() <= SECTION_PLANE_TOLERANCE_MM);
        for (a, b) in first.loops.iter().zip(&second.loops) {
            assert_eq!(a.is_hole, b.is_hole);
            assert_eq!(a.points_mm, b.points_mm);
            assert_eq!(a.points_mm.first(), a.points_mm.last());
            for point in &a.points_mm {
                let relative = std::array::from_fn(|i| {
                    sub(
                        Interval::point(point[i]),
                        Interval::point(plane.origin_mm[i]),
                    )
                });
                assert!(
                    dot(relative, normalized(plane.normal).unwrap()).abs_upper()
                        <= first.achieved_plane_error_mm()
                );
            }
        }
    }

    #[test]
    fn source_plane_direction_drift_is_not_only_an_origin_offset() {
        let mut section = square_section();
        section.plane.normal = [0.25, 0.0, 1.0];
        for point in &mut section.loops[0].points_mm {
            point[2] = 2.0 - 0.25 * point[0];
        }
        let target = HeightFunctional {
            direction: [
                Interval::point(0.0),
                Interval::point(0.0),
                Interval::point(1.0),
            ],
            offset: Interval::point(-2.0),
        };
        let error = world_plane_error(&section, target, [2.0; 3]).unwrap();
        assert!(error >= 0.5);
        for point in &section.loops[0].points_mm {
            assert!(
                add(
                    dot(target.direction, point.map(Interval::point)),
                    target.offset
                )
                .abs_upper()
                    <= error
            );
        }
    }

    #[test]
    fn shifting_a_world_plane_origin_within_its_plane_preserves_the_source_solve() {
        let factor = 1.0 + 2e-11;
        let pose = RigidPoseMm {
            rotation_xyzw: [
                0.3 * factor,
                0.4 * factor,
                0.5 * factor,
                0.5_f64.sqrt() * factor,
            ],
            translation_mm: [7.0, -11.0, 3.0],
        };
        let matrix = rotation(pose).unwrap();
        let a = plane_functional(
            matrix,
            pose,
            PlaneMm {
                origin_mm: [0.0, 0.0, 4.0],
                normal: [0.0, 0.0, 1.0],
            },
        )
        .unwrap();
        let b = plane_functional(
            matrix,
            pose,
            PlaneMm {
                origin_mm: [1e6, 1e6, 4.0],
                normal: [0.0, 0.0, 1.0],
            },
        )
        .unwrap();
        for (a, b) in a
            .direction
            .into_iter()
            .zip(b.direction)
            .chain([(a.offset, b.offset)])
        {
            assert_eq!([a.lo, a.hi], [b.lo, b.hi]);
        }
        assert_eq!(
            nominal_source_plane(a).unwrap(),
            nominal_source_plane(b).unwrap()
        );
    }

    #[test]
    fn endpoint_material_requires_exact_actual_world_height_not_a_midpoint() {
        let range = [4.0, 5.0];
        let uncertain = Interval {
            lo: 3.999,
            hi: 4.001,
        };
        assert!(
            classify_endpoint_world_height(uncertain, Some(Interval::point(4.0)), range, 4.0)
                .unwrap()
        );
        assert_eq!(
            classify_endpoint_world_height(uncertain, Some(uncertain), range, 4.0)
                .unwrap_err()
                .code,
            GeometryErrorCode::AnalysisIndeterminate
        );
        assert_eq!(
            classify_endpoint_world_height(uncertain, None, range, 4.0)
                .unwrap_err()
                .code,
            GeometryErrorCode::AnalysisIndeterminate
        );
        assert_eq!(
            classify_endpoint_world_height(
                Interval::point(4.5),
                Some(Interval::point(4.5)),
                range,
                4.0
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::AnalysisIndeterminate
        );
    }

    #[test]
    fn endpoint_material_is_discarded_only_strictly_outside_the_closed_world_slab() {
        let range = [4.0, 5.0];
        assert!(
            !classify_endpoint_world_height(Interval { lo: 3.0, hi: 3.999 }, None, range, 4.0)
                .unwrap()
        );
        assert!(
            !classify_endpoint_world_height(Interval { lo: 5.001, hi: 6.0 }, None, range, 5.0)
                .unwrap()
        );
        assert!(classify_endpoint_world_height(Interval::point(4.0), None, range, 4.0).unwrap());
        assert!(classify_endpoint_world_height(Interval::point(5.0), None, range, 5.0).unwrap());
        assert!(
            classify_endpoint_world_height(Interval { lo: 3.999, hi: 4.0 }, None, range, 4.0)
                .is_err()
        );
    }

    #[test]
    fn endpoint_position_enclosures_use_the_actual_forward_polynomial() {
        let pose = RigidPoseMm {
            translation_mm: [7.0, -3.0, 11.0],
            rotation_xyzw: [0.5, 0.5, 0.5, 0.5],
        };
        let world = transformed_position(
            rotation(pose).unwrap(),
            pose,
            [
                Interval { lo: 1.0, hi: 2.0 },
                Interval { lo: 3.0, hi: 4.0 },
                Interval::point(5.0),
            ],
        )
        .unwrap();
        assert_eq!([world[0].lo, world[0].hi], [12.0, 12.0]);
        assert!(world[1].lo <= -2.0 && world[1].hi >= -1.0);
        assert!(world[2].lo <= 14.0 && world[2].hi >= 15.0);
    }
}
