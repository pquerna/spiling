//! Private AVX2 arithmetic adapter. Only `detect` can produce a backend, after
//! runtime CPU/OS detection. Unaligned loads and stores cover complete blocks
//! of initialized slices only; residual lanes use the existing scalar price.
//! The coordinate and price envelope bounds intermediate squares, divisions
//! and score sums without placing a new limit on scalar routing inputs.

use super::{PointMm, Pricing};

pub(super) const BLOCK: usize = 32;
const MAX_MAGNITUDE: f64 = 1e100;

pub(super) fn point_ok(point: PointMm) -> bool {
    point.into_iter().all(|value| value.abs() <= MAX_MAGNITUDE)
}

#[derive(Clone, Copy)]
pub(super) struct Backend(());

impl Backend {
    pub(super) fn detect(pricing: Pricing) -> Option<Self> {
        let Pricing {
            speed,
            lift,
            minimum,
            retraction,
        } = pricing;
        let bounded = |value| (0.0..=MAX_MAGNITUDE).contains(&value);
        let avx2 = {
            #[cfg(target_arch = "x86_64")]
            {
                std::is_x86_feature_detected!("avx2")
            }
            #[cfg(not(target_arch = "x86_64"))]
            {
                false
            }
        };
        (avx2
            && (1e-100..=MAX_MAGNITUDE).contains(&speed)
            && bounded(lift)
            && bounded(retraction)
            && (bounded(minimum) || (minimum == f64::INFINITY && retraction == 0.0)))
            .then_some(Self(()))
    }

    pub(super) fn price(
        self,
        pricing: Pricing,
        from: [&[f64]; 3],
        point: PointMm,
        out: &mut [f64],
    ) {
        #[cfg(target_arch = "x86_64")]
        let whole = {
            let whole = out.len() / 4 * 4;
            let from = from.map(|axis| &axis[..out.len()]);
            unsafe { price_vectors(pricing, from, point, &mut out[..whole]) };
            whole
        };
        #[cfg(not(target_arch = "x86_64"))]
        let whole = 0;
        for ordinal in whole..out.len() {
            out[ordinal] = pricing.seconds(from.map(|axis| axis[ordinal]), point);
        }
    }

    pub(super) fn mask(
        self,
        base: &[f64; BLOCK],
        left: &[f64; BLOCK],
        flipped: &[f64; BLOCK],
        removed: f64,
        incumbent: f64,
        flip: bool,
    ) -> u32 {
        #[cfg(target_arch = "x86_64")]
        unsafe {
            if flip {
                mask_vectors::<true>(base, left, flipped, removed, incumbent)
            } else {
                mask_vectors::<false>(base, left, flipped, removed, incumbent)
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        (0..BLOCK).fold(0, |bits, ordinal| {
            let wins = |value| removed - (value - base[ordinal]) > incumbent;
            bits | (u32::from(wins(left[ordinal]) || (flip && wins(flipped[ordinal]))) << ordinal)
        })
    }
}

#[cfg(target_arch = "x86_64")]
use std::arch::x86_64::*;

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn price_vectors(pricing: Pricing, from: [&[f64]; 3], point: PointMm, out: &mut [f64]) {
    let zero = _mm256_setzero_pd();
    let speed = _mm256_set1_pd(pricing.speed);
    let lift = _mm256_set1_pd(pricing.lift);
    let minimum = _mm256_set1_pd(pricing.minimum);
    let retraction = _mm256_set1_pd(pricing.retraction);
    let [px, py, pz] = point.map(|coordinate| _mm256_set1_pd(coordinate));
    for offset in (0..out.len()).step_by(4) {
        let [dx, dy, dz] = unsafe {
            [
                _mm256_sub_pd(_mm256_loadu_pd(from[0].as_ptr().add(offset)), px),
                _mm256_sub_pd(_mm256_loadu_pd(from[1].as_ptr().add(offset)), py),
                _mm256_sub_pd(_mm256_loadu_pd(from[2].as_ptr().add(offset)), pz),
            ]
        };
        let xy = _mm256_add_pd(_mm256_mul_pd(dx, dx), _mm256_mul_pd(dy, dy));
        let distance = _mm256_sqrt_pd(_mm256_add_pd(xy, _mm256_mul_pd(dz, dz)));
        let extra = _mm256_and_pd(_mm256_cmp_pd::<_CMP_GE_OQ>(distance, minimum), retraction);
        let seconds = _mm256_add_pd(_mm256_div_pd(_mm256_add_pd(distance, lift), speed), extra);
        let nonzero = _mm256_cmp_pd::<_CMP_NEQ_OQ>(distance, zero);
        unsafe {
            _mm256_storeu_pd(
                out.as_mut_ptr().add(offset),
                _mm256_and_pd(seconds, nonzero),
            )
        };
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
unsafe fn mask_vectors<const FLIP: bool>(
    base: &[f64; BLOCK],
    left: &[f64; BLOCK],
    flipped: &[f64; BLOCK],
    removed: f64,
    incumbent: f64,
) -> u32 {
    let removed = _mm256_set1_pd(removed);
    let incumbent = _mm256_set1_pd(incumbent);
    let mut result = 0;
    for offset in (0..BLOCK).step_by(4) {
        let mask = unsafe {
            let base = _mm256_loadu_pd(base.as_ptr().add(offset));
            let threshold = |values| {
                _mm256_cmp_pd::<_CMP_GT_OQ>(
                    _mm256_sub_pd(removed, _mm256_sub_pd(values, base)),
                    incumbent,
                )
            };
            let mut mask = threshold(_mm256_loadu_pd(left.as_ptr().add(offset)));
            if FLIP {
                mask = _mm256_or_pd(
                    mask,
                    threshold(_mm256_loadu_pd(flipped.as_ptr().add(offset))),
                );
            }
            mask
        };
        result |= (_mm256_movemask_pd(mask) as u32) << offset;
    }
    result
}
