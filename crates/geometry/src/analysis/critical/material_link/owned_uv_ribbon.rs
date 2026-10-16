// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Face-side ownership is distinct from a pure normal-ribbon embedding.
//! A single winding calibration is consumed only after the complete connected
//! inner image avoids every original boundary and admitted joint completion.

use super::*;

// These proof phases run sequentially, after the pure embedding has returned.
// Their largest explicit stack fits the existing caller-reserved UV allowance.
const _: () = assert!(
    std::mem::size_of::<[UvPairCell; MAX_DEPTH as usize + 1]>() <= UV_EMBEDDING_STACK_BYTES
);
const _: () = assert!(
    std::mem::size_of::<[([f64; 2], u8); MAX_DEPTH as usize + 1]>() <= UV_EMBEDDING_STACK_BYTES
);

/// Sealed whole-used-scope ownership. The contained pure receipt binds the
/// exact original occurrence/map, source range, width, side and beta law.
/// This owns no geometry and cannot select a different G realization.
pub(super) struct OwnedUvRibbon<'a, 'd> {
    embedding: UvRibbonEmbedding<'a, 'd>,
}

impl OwnedUvRibbon<'_, '_> {
    pub(super) fn covers(
        &self,
        collar: &OriginalCollar<'_, '_>,
        range: [f64; 2],
        across: [f64; 2],
    ) -> bool {
        self.embedding.covers(collar, range, across)
    }
}

fn apart(a: [Interval; 2], b: [Interval; 2]) -> bool {
    (0..2).any(|axis| a[axis].hi < b[axis].lo || b[axis].hi < a[axis].lo)
}

fn inside_box(image: [Interval; 2], bounds: [[f64; 2]; 2]) -> bool {
    (0..2).all(|axis| image[axis].lo >= bounds[axis][0] && image[axis].hi <= bounds[axis][1])
}

fn resource_at(
    mut error: GeometryError,
    owner: CoedgeUse,
    stage: &'static str,
    native: [f64; 2],
    boundary: Option<(usize, [f64; 2])>,
) -> GeometryError {
    if error.code == GeometryErrorCode::ResourceLimit {
        error.message = format!(
            "{}; complete owned UV ribbon stage={stage} owner={:?} native={native:?} boundary_uv={boundary:?}",
            error.message,
            [owner.face, owner.wire, owner.index],
        );
    }
    error
}

/// For U(t,r)=gamma(t)+width*r*R gamma'(t), a whole interval Jacobian
/// encloses EVERY secant matrix on its convex rectangular proof domain.
/// If U(t,r) could meet gamma(s) within error e, its inverse second row gives
/// r*|det J| <= |J_t.y|*e.x + |J_t.x|*e.y. Strict failure of that inequality
/// excludes the entire owning curve restriction, including unused end pieces.
fn normal_secant_separates(
    jet: &ChartCurveEnclosure,
    width: f64,
    inward: f64,
    beta: Interval,
    error: [f64; 2],
) -> Result<bool, GeometryError> {
    if beta.lo <= 0.0 {
        return Ok(false);
    }
    let (tangent, transverse) = normal_uv_jet(
        jet,
        width,
        inward,
        Interval {
            lo: 0.0,
            hi: beta.hi,
        },
    );
    let scale = tangent
        .iter()
        .chain(transverse.iter())
        .map(|value| value.lo.abs().max(value.hi.abs()))
        .fold(0.0_f64, f64::max);
    if !scale.is_finite() || scale <= 0.0 {
        return Err(GeometryError::new(
            GeometryErrorCode::ConditioningFailure,
            "original UV ribbon secant matrix has no finite positive scale",
        ));
    }
    let tangent = [
        tangent[0].div(Interval::point(scale))?,
        tangent[1].div(Interval::point(scale))?,
    ];
    let transverse = [
        transverse[0].div(Interval::point(scale))?,
        transverse[1].div(Interval::point(scale))?,
    ];
    let determinant = determinant(tangent, transverse);
    let lower = if determinant.lo > 0.0 {
        determinant.lo
    } else if determinant.hi < 0.0 {
        -determinant.hi
    } else {
        return Ok(false);
    };
    let r_min = mul(Interval::point(0.5), beta).lo;
    let left = mul(
        mul(Interval::point(r_min), Interval::point(scale)),
        Interval::point(lower),
    );
    let right = add(
        mul(
            Interval::point(tangent[1].lo.abs().max(tangent[1].hi.abs())),
            Interval::point(error[0]),
        ),
        mul(
            Interval::point(tangent[0].lo.abs().max(tangent[0].hi.abs())),
            Interval::point(error[1]),
        ),
    );
    Ok(left.lo > right.hi)
}

impl<'a, 'd> OriginalCollar<'a, 'd> {
    fn inner_image(
        &self,
        range: [f64; 2],
        error: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[Interval; 2], GeometryError> {
        let beta = self.bubble(range)?.0;
        let normal = mul(beta, Interval { lo: 0.5, hi: 1.0 });
        Ok(add_margin(
            self.normal_image(range, normal, work, cancel)?,
            error,
        ))
    }

    fn raw_boundary_image(
        &self,
        coedge: usize,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[Interval; 2], GeometryError> {
        let original = &self.domain.native().coedges()[coedge];
        let mut native = monstertruck_modeling::ValidatedGeometryBudget::new(
            usize::try_from(work.remaining()).unwrap_or(usize::MAX),
        );
        let result = monstertruck_modeling::validated_curve2d_jet(
            original.curve,
            range,
            &mut native,
            cancel,
        );
        crate::parametric::debit_native_work(work, native.consumed(), cancel)?;
        Ok(result
            .map_err(crate::parametric::native_error)?
            .position
            .map(|value| Interval {
                lo: value.lower,
                hi: value.upper,
            }))
    }

    fn inner_misses_joints_and_domain(
        &self,
        range: [f64; 2],
        error: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let mut stack = [([0.0; 2], 0_u8); MAX_DEPTH as usize + 1];
        stack[0] = (range, 0);
        let mut pending = 1;
        while pending != 0 {
            pending -= 1;
            let (range, depth) = stack[pending];
            let context = |error| resource_at(error, self.use_, "joints and domain", range, None);
            work.charge(1, cancel).map_err(&context)?;
            let image = self
                .inner_image(range, error, work, cancel)
                .map_err(&context)?;
            let mut separated = inside_box(image, self.domain.native().closed_parameter_box());
            if let Some(bounds) = self.domain.certified_uv_domain() {
                separated &= inside_box(image, bounds);
            }
            for joint in self.domain.native().joints() {
                work.charge(1, cancel).map_err(&context)?;
                let enclosure = joint.enclosure.map(|value| Interval {
                    lo: value.lower,
                    hi: value.upper,
                });
                separated &= apart(image, enclosure);
            }
            if separated {
                continue;
            }
            let mid = range[0] * 0.5 + range[1] * 0.5;
            if depth >= MAX_DEPTH || mid <= range[0] || mid >= range[1] {
                return Ok(false);
            }
            stack[pending] = ([range[0], mid], depth + 1);
            stack[pending + 1] = ([mid, range[1]], depth + 1);
            pending += 2;
        }
        Ok(true)
    }

    fn inner_misses_original_boundaries(
        &self,
        range: [f64; 2],
        error: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let CarrierTransport::Collar { width, inward } = self.transport else {
            return Ok(true);
        };
        let own = self
            .domain
            .original_coedge_correspondence(self.use_.coedge)?;
        for coedge in 0..self.domain.native().coedges().len() {
            // Retain the association of EVERY original boundary occurrence.
            self.domain.original_coedge_correspondence(coedge)?;
            for span in self.domain.boundary_parameter_intervals(coedge)? {
                let mut stack: [UvPairCell; MAX_DEPTH as usize + 1] =
                    [([0.0; 2], [0.0; 2], 0); MAX_DEPTH as usize + 1];
                stack[0] = (range, span, 0);
                let mut pending = 1;
                while pending != 0 {
                    pending -= 1;
                    let (a, b, depth) = stack[pending];
                    let context = |error| {
                        resource_at(
                            error,
                            self.use_,
                            "complete original boundary",
                            a,
                            Some((coedge, b)),
                        )
                    };
                    work.charge(1, cancel).map_err(&context)?;
                    let image = self.inner_image(a, error, work, cancel).map_err(&context)?;
                    let boundary = self
                        .raw_boundary_image(coedge, b, work, cancel)
                        .map_err(&context)?;
                    if apart(image, boundary) {
                        continue;
                    }
                    if coedge == self.use_.coedge {
                        let Some(native) = own.affine_native_parameter_preimage(b)? else {
                            return Ok(false);
                        };
                        let hull = [a[0].min(native.lo), a[1].max(native.hi)];
                        // The SAME original strict C2 operator supplies this
                        // convex proof hull; genuine knots cannot be crossed.
                        let jet =
                            chart_jet(self.domain, coedge, hull, work, cancel).map_err(&context)?;
                        if normal_secant_separates(&jet, width, inward, self.bubble(a)?.0, error)? {
                            continue;
                        }
                    }
                    if depth >= MAX_DEPTH {
                        return Ok(false);
                    }
                    let a_fraction = if range[1] > range[0] {
                        (a[1] - a[0]) / (range[1] - range[0])
                    } else {
                        0.0
                    };
                    let b_fraction = if span[1] > span[0] {
                        (b[1] - b[0]) / (span[1] - span[0])
                    } else {
                        0.0
                    };
                    let split_a = a[0] < a[1] && (a_fraction >= b_fraction || b[0] == b[1]);
                    let split = if split_a { a } else { b };
                    let mid = split[0] * 0.5 + split[1] * 0.5;
                    if mid <= split[0] || mid >= split[1] {
                        return Ok(false);
                    }
                    let left = [split[0], mid];
                    let right = [mid, split[1]];
                    stack[pending] = if split_a {
                        (left, b, depth + 1)
                    } else {
                        (a, left, depth + 1)
                    };
                    stack[pending + 1] = if split_a {
                        (right, b, depth + 1)
                    } else {
                        (a, right, depth + 1)
                    };
                    pending += 2;
                }
            }
        }
        Ok(true)
    }

    pub(super) fn owned_embedding(
        &self,
        range: [f64; 2],
        remaining_scratch_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<OwnedUvRibbon<'a, 'd>>, GeometryError> {
        let Some(embedding) = self.embedding(range, [0.0, 1.0], work, cancel)? else {
            return Ok(None);
        };
        if matches!(self.transport, CarrierTransport::Exact) {
            // No G collar exists in the exact-coefficient, zero-transport
            // lane. Retain its original boundary ownership without inventing
            // an inner ribbon or an Inside calibration.
            return Ok(Some(OwnedUvRibbon { embedding }));
        }
        // Native expands a classifier query by its whole-domain uncertainty.
        // Keep that error AND the original anchor margin in complete exclusion;
        // never mutate either retained source margin or its parameter map.
        let uncertainty = self.domain.native().uncertainty_uv();
        let error = std::array::from_fn(|axis| {
            add(
                Interval::point(self.margin[axis]),
                Interval::point(uncertainty[axis]),
            )
            .hi
        });
        if !self.inner_misses_joints_and_domain(range, error, work, cancel)?
            || !self.inner_misses_original_boundaries(range, error, work, cancel)?
        {
            return Ok(None);
        }
        // The actual inner image, including both retained error boxes, is
        // connected and misses the COMPLETE boundary inventory. One strict
        // native winding calibration now applies to that whole image, not its
        // curved image's overlarge Cartesian bounding box.
        let t = range[0] * 0.5 + range[1] * 0.5;
        let jet = chart_jet(self.domain, self.use_.coedge, [t; 2], work, cancel)?;
        let beta = self.bubble([t; 2])?.0;
        let CarrierTransport::Collar { width, inward } = self.transport else {
            unreachable!()
        };
        let normal = mul(beta, Interval::point(0.75));
        let offset = rotate(jet.derivative, inward)
            .map(|value| mul(mul(Interval::point(width), normal), value));
        let point = std::array::from_fn(|axis| add(jet.position[axis], offset[axis]));
        let inclusive = remaining_scratch_bytes
            .checked_add(self.domain.retained_bytes())
            .ok_or_else(|| {
                GeometryError::new(
                    GeometryErrorCode::ResourceLimit,
                    "UV ribbon classifier byte count overflow",
                )
            })?
            .min(MAX_SCRATCH_BYTES);
        if self
            .domain
            .classify_with_byte_limit(
                uv_box(add_margin(point, self.margin)),
                inclusive,
                work,
                cancel,
            )
            .map_err(|error| {
                resource_at(error, self.use_, "strict winding calibration", range, None)
            })?
            != TrimClassification::Inside
        {
            return Ok(None);
        }
        Ok(Some(OwnedUvRibbon { embedding }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owning_complement_secant_bound_retains_both_uv_errors() {
        for scale in [1.0e-150, 1.0, 1.0e150] {
            // Exact original affine gamma(t)=(scale*t,0) on a proof hull
            // containing BOTH the used interval and its unused complement.
            let jet = ChartCurveEnclosure {
                position: [
                    Interval::new(-2.0 * scale, 3.0 * scale).unwrap(),
                    Interval::point(0.0),
                ],
                derivative: [Interval::point(scale), Interval::point(0.0)],
                second_derivative: [Interval::point(0.0); 2],
            };
            let width = 1.0 / 1024.0;
            let beta = Interval::point(0.25);
            let one_error = scale * width / 16.0;
            for inward in [-1.0, 1.0] {
                assert!(
                    normal_secant_separates(&jet, width, inward, beta, [0.0, one_error],).unwrap()
                );
                let both_errors =
                    add(Interval::point(one_error), Interval::point(one_error * 2.0)).hi;
                assert!(
                    !normal_secant_separates(&jet, width, inward, beta, [0.0, both_errors],)
                        .unwrap(),
                    "admitted boundary uncertainty reaches the used inner image"
                );
            }
        }
    }

    #[test]
    fn owning_secant_cannot_certify_a_collapsed_original_germ() {
        let jet = ChartCurveEnclosure {
            position: [Interval::point(0.0); 2],
            derivative: [Interval::point(0.0); 2],
            second_derivative: [Interval::point(0.0); 2],
        };
        assert_eq!(
            normal_secant_separates(&jet, 1.0 / 1024.0, 1.0, Interval::point(0.25), [0.0; 2],)
                .unwrap_err()
                .code,
            GeometryErrorCode::ConditioningFailure,
        );
    }
}
