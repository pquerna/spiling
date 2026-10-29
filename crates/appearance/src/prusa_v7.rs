// SPDX-FileCopyrightText: 2026 Ondrej Bartas (Prusa Research s.r.o.) and contributors
// SPDX-License-Identifier: MIT
//
// MIT License
//
// Copyright (c) 2026 Ondrej Bartas (Prusa Research s.r.o.) and contributors
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

//! Allocation-free port of prusa-fdm-mixer's shipped v7 functions at
//! 007bb922994cdf985c324cb734f6504e805f6790: src/color.ts,
//! src/yule-nielsen.ts and src/prusa-fdm-mixer.ts. These reference-specific
//! matrices, rounded base bytes, clamped display conversion, 0.9999 guard and
//! coefficients deliberately differ from Spiling's unclipped CSS colorimetry.
//! No upstream fitting-set or vendor measurement data is included.
use std::f64::consts::PI;

#[derive(Debug, Clone, Copy)]
pub(crate) struct BaseColor {
    rgb: [u8; 3],
    lab: [f64; 3],
    roots: [f64; 3],
}
impl BaseColor {
    pub(crate) fn new(rgb: [u8; 3]) -> Self {
        Self {
            rgb,
            lab: rgb_to_lab(rgb),
            roots: rgb.map(|v| srgb_to_linear(v as f64).powf(1.0 / 3.0)),
        }
    }
}
#[derive(Clone, Copy)]
pub(crate) struct Part<'a> {
    pub color: &'a BaseColor,
    pub ratio: f64,
}
#[derive(Debug)]
pub(crate) struct Prediction {
    pub lab: [f64; 3],
    pub rgb: [u8; 3],
}

fn srgb_to_linear(c: f64) -> f64 {
    let v = c / 255.0;
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
fn linear_to_srgb(c: f64) -> f64 {
    let x = c.clamp(0.0, 1.0);
    (if x <= 0.0031308 {
        12.92 * x
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    }) * 255.0
}
fn byte(v: f64) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}
fn rgb_to_lab(rgb: [u8; 3]) -> [f64; 3] {
    let [r, g, b] = rgb.map(|v| srgb_to_linear(v as f64));
    let f = |t: f64| {
        if t > 0.008856 {
            t.cbrt()
        } else {
            7.787 * t + 16.0 / 116.0
        }
    };
    let fx = f((r * 0.4124564 + g * 0.3575761 + b * 0.1804375) / 0.95047);
    let fy = f(r * 0.2126729 + g * 0.7151522 + b * 0.0721750);
    let fz = f((r * 0.0193339 + g * 0.1191920 + b * 0.9503041) / 1.08883);
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}
fn lab_to_rgb([l, a, b]: [f64; 3]) -> [u8; 3] {
    let fy = (l + 16.0) / 116.0;
    let fx = a / 500.0 + fy;
    let fz = fy - b / 200.0;
    let inv = |t: f64| {
        let cube = t.powf(3.0);
        if cube > 0.008856 {
            cube
        } else {
            (t - 16.0 / 116.0) / 7.787
        }
    };
    let x = 0.95047 * inv(fx);
    let y = inv(fy);
    let z = 1.08883 * inv(fz);
    [
        byte(linear_to_srgb(
            x * 3.2404542 + y * -1.5371385 + z * -0.4985314,
        )),
        byte(linear_to_srgb(
            x * -0.9692660 + y * 1.8760108 + z * 0.0415560,
        )),
        byte(linear_to_srgb(
            x * 0.0556434 + y * -0.2040259 + z * 1.0572252,
        )),
    ]
}

/// The wrapper supplies 1..3 positive-exposure parts; ratios are normalized here
/// exactly as upstream. All scratch is fixed-size stack storage.
pub(crate) fn predict(parts: &[Part<'_>]) -> Prediction {
    assert!(!parts.is_empty() && parts.len() <= 3);
    let total: f64 = parts.iter().map(|p| p.ratio).sum();
    let mut normalized = [0.0; 3];
    for (i, part) in parts.iter().enumerate() {
        normalized[i] = part.ratio / total;
        if normalized[i] >= 0.9999 {
            return Prediction {
                lab: part.color.lab,
                rgb: part.color.rgb,
            };
        }
    }
    let mut roots = [0.0; 3];
    let mut min_l = f64::INFINITY;
    let mut max_l = f64::NEG_INFINITY;
    let mut product = 1.0;
    for (i, part) in parts.iter().enumerate() {
        for (channel, root) in roots.iter_mut().enumerate() {
            *root += part.color.roots[channel] * normalized[i];
        }
        min_l = min_l.min(part.color.lab[0]);
        max_l = max_l.max(part.color.lab[0]);
        product *= normalized[i];
    }
    let base = rgb_to_lab(roots.map(|v| byte(linear_to_srgb(v.max(0.0).powf(3.0)))));
    let gap = max_l - min_l;
    let n = parts.len() as f64;
    let w = (n.powf(n) * product).clamp(0.0, 1.0) * 1.375;
    let mut dl = -0.0477 * gap - 2.112;
    if gap > 15.0 {
        dl += -0.06 * (gap - 15.0);
    }
    let new_l = base[0] + dl * w;
    let base_c = base[1].hypot(base[2]);
    let mut a = base[1];
    let mut b = base[2];
    if base_c >= 0.01 {
        let dc = (0.278 * new_l - 15.58) * w;
        let scale = (base_c + dc).max(0.0) / base_c;
        a = base[1] * scale;
        b = base[2] * scale;
    }
    let new_c = a.hypot(b);
    if new_c >= 1.0 {
        let hue = (b.atan2(a) * 180.0 / PI + 360.0) % 360.0;
        let distance = (hue - 210.0).abs();
        if distance < 30.0 {
            let correction = 10.38 * (1.0 - distance / 30.0) * w;
            let radians = ((hue + correction) % 360.0) * PI / 180.0;
            a = new_c * radians.cos();
            b = new_c * radians.sin();
        }
    }
    let lab = [new_l, a, b];
    Prediction {
        lab,
        rgb: lab_to_rgb(lab),
    }
}
