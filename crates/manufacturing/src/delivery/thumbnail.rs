// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Plate thumbnails rendered from the plan's deposition paths: a top view of
//! every bead centreline, later (higher) layers over earlier ones, shaded by
//! height. A display picture only; deterministic for one plan.

use crate::{check_cancel, error};
use spiling_contracts::manufacturing::{
    DepositionKind, ManufacturingError, ManufacturingErrorCode, NormalizedPrintPlan, WidthProfile,
};
use std::sync::atomic::AtomicBool;

/// RGBA PNG of `size` × `size` pixels.
pub fn render(
    plan: &NormalizedPrintPlan,
    size: u32,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, ManufacturingError> {
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    let mut top = f64::NEG_INFINITY;
    let mut bottom = f64::INFINITY;
    for layer in &plan.layers {
        for path in &layer.paths {
            for p in &path.points_mm {
                lo = [lo[0].min(p[0]), lo[1].min(p[1])];
                hi = [hi[0].max(p[0]), hi[1].max(p[1])];
            }
        }
        top = top.max(layer.z_mm);
        bottom = bottom.min(layer.z_mm);
    }
    let n = size as usize;
    let mut rgba = vec![0u8; n * n * 4];
    if lo[0] <= hi[0] && lo[1] <= hi[1] {
        let span = (hi[0] - lo[0]).max(hi[1] - lo[1]).max(1.0) * 1.1;
        let centre = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
        let scale = f64::from(size) / span;
        let pixel = |p: &[f64; 3]| {
            [
                (p[0] - centre[0]) * scale + f64::from(size) * 0.5,
                // Image rows grow downward; bed Y grows away from the viewer.
                f64::from(size) * 0.5 - (p[1] - centre[1]) * scale,
            ]
        };
        let height = (top - bottom).max(f64::EPSILON);
        for layer in &plan.layers {
            check_cancel(cancel)?;
            let shade = 0.45 + 0.55 * ((layer.z_mm - bottom) / height).clamp(0.0, 1.0);
            for path in &layer.paths {
                let base: [f64; 3] = match path.kind {
                    DepositionKind::Support | DepositionKind::SupportInterface => {
                        [0.85, 0.80, 0.70]
                    }
                    _ => [0.20, 0.62, 0.86],
                };
                let colour = base.map(|c| (c * shade * 255.0).round() as u8);
                let width = match &path.width {
                    WidthProfile::Constant { width_mm } => *width_mm,
                    WidthProfile::PerEdge { widths_mm } => {
                        widths_mm.iter().copied().fold(0.0, f64::max)
                    }
                };
                let radius = (width * scale * 0.5).clamp(1.0, 16.0);
                for pair in path.points_mm.windows(2) {
                    line(
                        &mut rgba,
                        n,
                        pixel(&pair[0]),
                        pixel(&pair[1]),
                        radius,
                        colour,
                    );
                }
            }
        }
    }
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, size, size);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Balanced);
        let encode = |e: png::EncodingError| {
            error(ManufacturingErrorCode::InvalidSpecification, e.to_string())
        };
        let mut writer = encoder.write_header().map_err(encode)?;
        writer.write_image_data(&rgba).map_err(encode)?;
    }
    Ok(png)
}

/// A capsule of `radius` pixels around segment `a`–`b`.
fn line(rgba: &mut [u8], n: usize, a: [f64; 2], b: [f64; 2], radius: f64, colour: [u8; 3]) {
    let lo = [a[0].min(b[0]) - radius, a[1].min(b[1]) - radius];
    let hi = [a[0].max(b[0]) + radius, a[1].max(b[1]) + radius];
    let (x0, y0) = (
        lo[0].floor().max(0.0) as usize,
        lo[1].floor().max(0.0) as usize,
    );
    let (x1, y1) = (
        (hi[0].ceil().max(0.0) as usize).min(n),
        (hi[1].ceil().max(0.0) as usize).min(n),
    );
    let d = [b[0] - a[0], b[1] - a[1]];
    let length2 = d[0] * d[0] + d[1] * d[1];
    for py in y0..y1 {
        for px in x0..x1 {
            let p = [px as f64 + 0.5 - a[0], py as f64 + 0.5 - a[1]];
            let t = if length2 > 0.0 {
                ((p[0] * d[0] + p[1] * d[1]) / length2).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let (ex, ey) = (p[0] - d[0] * t, p[1] - d[1] * t);
            if ex * ex + ey * ey <= radius * radius {
                let at = (py * n + px) * 4;
                rgba[at..at + 3].copy_from_slice(&colour);
                rgba[at + 3] = 255;
            }
        }
    }
}
