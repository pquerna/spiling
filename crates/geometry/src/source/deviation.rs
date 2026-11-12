// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Recheck retained analytic interpretation against complete source facets.
use super::*;
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| a[i] * b[i]).sum()
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn unit(a: [f64; 3]) -> [f64; 3] {
    let n = dot(a, a).sqrt();
    a.map(|v| v / n)
}
pub(super) fn triangle(
    surface: &AnalyticSurfaceDescription,
    p: [[f64; 3]; 3],
) -> Result<f64, GeometryError> {
    let magnitude = p.iter().flatten().copied().map(f64::abs).fold(1., f64::max);
    let reserve = 128. * f64::EPSILON * magnitude;
    let error = match surface {
        AnalyticSurfaceDescription::Plane { origin_mm, normal } => p
            .map(|p| dot(sub(p, *origin_mm), *normal).abs())
            .into_iter()
            .fold(0., f64::max),
        AnalyticSurfaceDescription::Cylinder {
            axis_origin_mm,
            axis_direction,
            radius_mm,
        } => {
            let axis = unit(*axis_direction);
            let least = (0..3)
                .min_by(|&a, &b| axis[a].abs().total_cmp(&axis[b].abs()))
                .unwrap();
            let seed = std::array::from_fn(|a| if a == least { 1. } else { 0. });
            let u = unit(cross(axis, seed));
            let v = cross(axis, u);
            let xy = p.map(|point| {
                let d = sub(point, *axis_origin_mm);
                [dot(d, u), dot(d, v)]
            });
            let cross2 = |a: [f64; 2], b: [f64; 2]| a[0] * b[1] - a[1] * b[0];
            let signs = [
                cross2(xy[0], xy[1]),
                cross2(xy[1], xy[2]),
                cross2(xy[2], xy[0]),
            ];
            let area = cross2(
                [xy[1][0] - xy[0][0], xy[1][1] - xy[0][1]],
                [xy[2][0] - xy[0][0], xy[2][1] - xy[0][1]],
            );
            let inside =
                area != 0. && (signs.iter().all(|v| *v >= 0.) || signs.iter().all(|v| *v <= 0.));
            let minimum = if inside {
                0.
            } else {
                (0..3)
                    .map(|i| {
                        let a = xy[i];
                        let b = xy[(i + 1) % 3];
                        let d = [b[0] - a[0], b[1] - a[1]];
                        let length = d[0] * d[0] + d[1] * d[1];
                        let t = if length == 0. {
                            0.
                        } else {
                            (-(a[0] * d[0] + a[1] * d[1]) / length).clamp(0., 1.)
                        };
                        (a[0] + t * d[0]).hypot(a[1] + t * d[1])
                    })
                    .fold(f64::INFINITY, f64::min)
            };
            xy.map(|p| (p[0].hypot(p[1]) - radius_mm).abs())
                .into_iter()
                .fold((radius_mm - minimum).abs(), f64::max)
        }
        _ => return Err(invalid("unsupported recovered analytic surface")),
    };
    if !error.is_finite() {
        return Err(invalid("nonfinite recovered surface deviation"));
    }
    Ok((error + reserve).next_up())
}
