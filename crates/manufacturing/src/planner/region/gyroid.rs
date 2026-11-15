// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Uniform Gyroid, derived from sin(x)cos(y)+sin(y)cos(z)+sin(z)cos(x)=0.
//! Solve along the well-conditioned horizontal direction, refine two periodic
//! profiles once, then translate and clip them. CubeCL screens boundary pairs;
//! the host owns curve approximation, robust intersections and material entry.

use super::*;
use geo::line_intersection::{LineIntersection, line_intersection};
use geo::{Distance, Euclidean, Intersects, Line, Point};
use std::f64::consts::{FRAC_PI_4, PI, TAU};

// Bulk contour length calibration of the implicit surface, not an empirical
// strength or final-part porosity. See docs/protocol/manufacturing.md.
const DENSITY_MULTIPLIER: f64 = 2.55577;

struct Wave {
    sine_z: f64,
    cosine_z: f64,
    along_x: bool,
    scale: f64,
}

impl Wave {
    fn ordinate(&self, u: f64, branch: usize) -> f64 {
        let (s, c) = u.sin_cos();
        if self.along_x {
            let angle = (s / self.cosine_z).atan();
            let rhs = (-self.sine_z * c * self.cosine_z.signum() / s.hypot(self.cosine_z))
                .clamp(-1.0, 1.0);
            let root = rhs.asin();
            if branch == 0 {
                root - angle
            } else {
                PI - root - angle
            }
        } else {
            let angle = (c / self.sine_z).atan();
            let rhs =
                (-self.cosine_z * s * self.sine_z.signum() / c.hypot(self.sine_z)).clamp(-1.0, 1.0);
            angle + if branch == 0 { rhs.acos() } else { -rhs.acos() }
        }
    }

    fn profile(
        &self,
        first: f64,
        last: f64,
        branch: usize,
        cancel: &AtomicBool,
    ) -> Result<Vec<[f64; 2]>, ManufacturingError> {
        let mut points = vec![[first, self.ordinate(first, branch)]];
        let mut u = first;
        while u < last {
            check_cancel(cancel)?;
            let next = (u + FRAC_PI_4).min(last);
            if next <= u {
                return Err(unsupported(
                    "Gyroid profile exceeds coordinate conditioning",
                ));
            }
            self.refine(
                [u, self.ordinate(u, branch)],
                [next, self.ordinate(next, branch)],
                branch,
                &mut points,
                cancel,
            )?;
            u = next;
        }
        Ok(points)
    }

    fn refine(
        &self,
        a: [f64; 2],
        b: [f64; 2],
        branch: usize,
        output: &mut Vec<[f64; 2]>,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        check_cancel(cancel)?;
        let du = b[0] - a[0];
        let dv = b[1] - a[1];
        let length = du.hypot(dv);
        // Quarter points also detect an S bend whose midpoint is on the chord.
        let error = [0.25, 0.5, 0.75].into_iter().fold(0.0_f64, |error, t| {
            let u = a[0] + du * t;
            let v = self.ordinate(u, branch);
            error.max((du * (v - a[1]) - dv * (u - a[0])).abs() / length)
        });
        if error * self.scale <= super::super::OFFSET_TOLERANCE_MM * 0.5 {
            reserve_points(output, 1)?;
            output.push(b);
            return Ok(());
        }
        let middle = a[0] + du * 0.5;
        if middle <= a[0] || middle >= b[0] {
            return Err(unsupported(
                "Gyroid refinement exceeds coordinate conditioning",
            ));
        }
        let midpoint = [middle, self.ordinate(middle, branch)];
        self.refine(a, midpoint, branch, output, cancel)?;
        self.refine(midpoint, b, branch, output, cancel)
    }
}

pub(super) fn hatch(
    fill: &MultiPolygon<f64>,
    fraction: f64,
    layer: &HatchLayer<'_>,
    paths: &mut Vec<DepositionPath>,
    segments: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    if fraction == 0.0 || fill.0.is_empty() {
        return Ok(());
    }
    // Pitch belongs to the nominal process, not each event-split interval.
    // Changing it with actual thickness would shift the entire XY field when
    // the scheduler inserts a fine layer. Actual height still owns dose.
    let equivalent_width = layer.width_mm - layer.recipe.layer_height_mm * (1.0 - PI / 4.0);
    let scale = equivalent_width / (DENSITY_MULTIPLIER * fraction);
    if !scale.is_finite() || scale <= 0.0 {
        return Err(unsupported(
            "Gyroid requires finite positive material pitch",
        ));
    }
    let (sine_z, cosine_z) = (layer.z / scale).rem_euclid(TAU).sin_cos();
    let wave = Wave {
        sine_z,
        cosine_z,
        along_x: cosine_z.abs() >= sine_z.abs(),
        scale,
    };
    let bounds = fill
        .bounding_rect()
        .ok_or_else(|| unsupported("Gyroid fill has no bounds"))?;
    let (low, high, across_low, across_high) = if wave.along_x {
        (
            bounds.min().x,
            bounds.max().x,
            bounds.min().y,
            bounds.max().y,
        )
    } else {
        (
            bounds.min().y,
            bounds.max().y,
            bounds.min().x,
            bounds.max().x,
        )
    };
    // The integer-period origin preserves one build-frame phase across islands
    // and changing layer extents. Work near the actual bounds, not from zero.
    let origin = (low / (TAU * scale)).floor() * TAU * scale;
    let u_first = (low - origin) / scale;
    let u_last = (high - origin) / scale;
    let period = u_last - u_first >= TAU;
    let first = if period { 0.0 } else { u_first };
    let last = if period { TAU } else { u_last };
    let profiles = [
        wave.profile(first, last, 0, cancel)?,
        wave.profile(first, last, 1, cancel)?,
    ];
    let boundaries = Boundary::new(fill)?;
    let first_row = (across_low / (TAU * scale)).floor() - 1.0;
    let last_row = (across_high / (TAU * scale)).ceil() + 1.0;
    let rows = last_row - first_row + 1.0;
    if !rows.is_finite() || rows >= usize::MAX as f64 {
        return Err(resource("Gyroid row count exceeds addressable coordinates"));
    }
    for row in 0..rows as usize {
        let offset = (first_row + row as f64) * TAU;
        for profile in &profiles {
            check_cancel(cancel)?;
            let mut curve = Vec::new();
            let repeat_first = if period {
                (u_first / TAU).floor() as usize
            } else {
                0
            };
            let repeat_last = if period {
                (u_last / TAU).ceil() as usize
            } else {
                1
            };
            for repeat in repeat_first..repeat_last {
                for (index, point) in profile.iter().enumerate() {
                    if repeat > repeat_first && index == 0 {
                        continue;
                    }
                    let u = point[0] + if period { repeat as f64 * TAU } else { 0.0 };
                    let along = origin + u * scale;
                    let across = (point[1] + offset) * scale;
                    reserve_points(&mut curve, 1)?;
                    curve.push(if wave.along_x {
                        [along, across]
                    } else {
                        [across, along]
                    });
                }
            }
            boundaries.clip(&curve, fill, layer, paths, segments, cancel)?;
        }
    }
    Ok(())
}

fn reserve_points<T>(values: &mut Vec<T>, additional: usize) -> Result<(), ManufacturingError> {
    if values.len().saturating_add(additional) > crate::working_set::records::<T>() {
        return Err(resource("Gyroid scratch exceeds available working set"));
    }
    values
        .try_reserve(additional)
        .map_err(|_| resource("Gyroid allocation refused"))
}

struct Boundary {
    edges: Vec<Line<f64>>,
    boxes: Vec<[f64; 4]>,
}

fn bounds(a: [f64; 2], b: [f64; 2]) -> [f64; 4] {
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[0].max(b[0]),
        a[1].max(b[1]),
    ]
}

fn overlaps(a: [f64; 4], b: [f64; 4]) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

impl Boundary {
    fn new(fill: &MultiPolygon<f64>) -> Result<Self, ManufacturingError> {
        let mut result = Self {
            edges: Vec::new(),
            boxes: Vec::new(),
        };
        for polygon in &fill.0 {
            for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
                for edge in ring.0.windows(2) {
                    reserve_points(&mut result.edges, 1)?;
                    reserve_points(&mut result.boxes, 1)?;
                    result.edges.push(Line::new(edge[0], edge[1]));
                    result
                        .boxes
                        .push(bounds([edge[0].x, edge[0].y], [edge[1].x, edge[1].y]));
                }
            }
        }
        Ok(result)
    }

    fn clip(
        &self,
        curve: &[[f64; 2]],
        fill: &MultiPolygon<f64>,
        layer: &HatchLayer<'_>,
        paths: &mut Vec<DepositionPath>,
        segments: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        let Some(first) = curve.first() else {
            return Ok(());
        };
        let mut inside = fill.intersects(&Point::new(first[0], first[1]));
        let mut road = Vec::new();
        let tile = (spiling_compute::MAX_MASK_WORDS / self.edges.len().div_ceil(32).max(1))
            .clamp(1, spiling_compute::MAX_BATCH_ITEMS);
        let mut cursor = 0;
        let mut splits = Vec::new();
        while cursor + 1 < curve.len() {
            check_cancel(cancel)?;
            let count = tile.min(curve.len() - 1 - cursor);
            let queries: Vec<_> = curve[cursor..=cursor + count]
                .windows(2)
                .map(|edge| bounds(edge[0], edge[1]))
                .collect();
            let mask = spiling_compute::box_overlaps(&self.boxes, &queries, cancel)
                .map_err(crate::compute_error)?;
            for index in 0..count {
                let a = curve[cursor + index];
                let b = curve[cursor + index + 1];
                let line = Line::new(Coord { x: a[0], y: a[1] }, Coord { x: b[0], y: b[1] });
                let delta = [b[0] - a[0], b[1] - a[1]];
                let axis = usize::from(delta[1].abs() > delta[0].abs());
                splits.clear();
                reserve_points(&mut splits, 2)?;
                splits.extend([0.0, 1.0]);
                let mut boundary_hit = false;
                let mut intersect = |edge: usize| -> Result<(), ManufacturingError> {
                    check_cancel(cancel)?;
                    reserve_points(&mut splits, 2)?;
                    let mut add = |p: Coord<f64>| {
                        let coordinate = if axis == 0 { p.x } else { p.y };
                        splits.push(((coordinate - a[axis]) / delta[axis]).clamp(0.0, 1.0));
                        boundary_hit = true;
                    };
                    match line_intersection(line, self.edges[edge]) {
                        Some(LineIntersection::SinglePoint { intersection, .. }) => {
                            add(intersection)
                        }
                        Some(LineIntersection::Collinear { intersection }) => {
                            add(intersection.start);
                            add(intersection.end);
                        }
                        None => {}
                    }
                    Ok(())
                };
                if let Some(mask) = &mask {
                    for edge in mask
                        .selected_columns(index)
                        .ok_or_else(|| unsupported("Gyroid boundary mask row missing"))?
                    {
                        intersect(edge)?;
                    }
                } else {
                    for (edge, bbox) in self.boxes.iter().enumerate() {
                        if overlaps(*bbox, queries[index]) {
                            intersect(edge)?;
                        }
                    }
                }
                splits.sort_unstable_by(f64::total_cmp);
                splits.dedup();
                for interval in splits.windows(2) {
                    if interval[1] <= interval[0] {
                        continue;
                    }
                    if boundary_hit {
                        let t = (interval[0] + interval[1]) * 0.5;
                        inside =
                            fill.intersects(&Point::new(a[0] + t * delta[0], a[1] + t * delta[1]));
                    }
                    if inside {
                        let start = [
                            a[0] + interval[0] * delta[0],
                            a[1] + interval[0] * delta[1],
                            layer.z,
                        ];
                        let end = [
                            a[0] + interval[1] * delta[0],
                            a[1] + interval[1] * delta[1],
                            layer.z,
                        ];
                        if road.is_empty() {
                            road.push(start);
                        }
                        reserve_points(&mut road, 1)?;
                        road.push(end);
                    } else {
                        finish(&mut road, self, fill, layer, paths, segments, cancel)?;
                    }
                }
            }
            cursor += count;
        }
        finish(&mut road, self, fill, layer, paths, segments, cancel)
    }
}

/// Distance to a closed material set is 1-Lipschitz. Qualify only outside
/// intervals left by robust boundary cuts; inside intervals are already known.
/// This spends the existing profile allowance, never a new hole-sized epsilon.
fn near_material(
    a: [f64; 2],
    b: [f64; 2],
    fill: &MultiPolygon<f64>,
    boundary: &Boundary,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    check_cancel(cancel)?;
    let middle = [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5];
    let point = Point::from(middle);
    if fill.intersects(&point) {
        return Ok(true);
    }
    let distance = boundary
        .edges
        .iter()
        .map(|edge| Euclidean.distance(&point, edge))
        .fold(f64::INFINITY, f64::min);
    let tolerance = super::super::OFFSET_TOLERANCE_MM;
    if distance > tolerance {
        return Ok(false);
    }
    let half_length = (b[0] - a[0]).hypot(b[1] - a[1]) * 0.5;
    if distance + half_length <= tolerance {
        return Ok(true);
    }
    if middle == a || middle == b {
        return Ok(false);
    }
    Ok(near_material(a, middle, fill, boundary, cancel)?
        && near_material(middle, b, fill, boundary, cancel)?)
}

fn finish(
    road: &mut Vec<[f64; 3]>,
    boundary: &Boundary,
    fill: &MultiPolygon<f64>,
    layer: &HatchLayer<'_>,
    paths: &mut Vec<DepositionPath>,
    segments: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    if road.len() < 2 {
        road.clear();
        return Ok(());
    }
    // Remove only sub-command clip artifacts before they can become anchors.
    road.dedup_by(|b, a| (b[0] - a[0]).hypot(b[1] - a[1]) <= 0.000002);
    // Clip endpoints stay fixed. Combine a short command with a neighbour
    // only within the existing profile allowance and unchanged center domain.
    // Unrepresentable fragments refuse explicitly; nothing is dropped.
    use spiling_contracts::manufacturing::program_coordinates::{
        MIN_DEPOSITION_COMMAND_MM, project_xyz6_point_mm,
    };
    let total_length = road
        .windows(2)
        .map(|e| (e[1][0] - e[0][0]).hypot(e[1][1] - e[0][1]))
        .sum::<f64>();
    if total_length < MIN_DEPOSITION_COMMAND_MM {
        // ADR012 permits omission of a complete sub-command fill fragment.
        // This never removes a short edge from an otherwise printable road.
        road.clear();
        return Ok(());
    }
    let mut fit_errors = Vec::<f64>::new();
    loop {
        let mut short = None;
        for (index, edge) in road.windows(2).enumerate() {
            let a = project_xyz6_point_mm(edge[0])?;
            let b = project_xyz6_point_mm(edge[1])?;
            if (b[0] - a[0]).hypot(b[1] - a[1]) < MIN_DEPOSITION_COMMAND_MM {
                short = Some(index);
                break;
            }
        }
        let Some(index) = short else { break };
        if fit_errors.is_empty() {
            reserve_points(&mut fit_errors, road.len() - 1)?;
            fit_errors.resize(road.len() - 1, 0.0);
        }
        let mut merged = false;
        for middle in [index + 1, index] {
            check_cancel(cancel)?;
            if middle == 0 || middle + 1 >= road.len() {
                continue;
            }
            let a = road[middle - 1];
            let p = road[middle];
            let b = road[middle + 1];
            let dx = b[0] - a[0];
            let dy = b[1] - a[1];
            let length = dx.hypot(dy);
            let along = ((p[0] - a[0]) * dx + (p[1] - a[1]) * dy) / (length * length);
            let deviation = (dx * (p[1] - a[1]) - dy * (p[0] - a[0])).abs() / length;
            let error_bound = deviation + fit_errors[middle - 1].max(fit_errors[middle]);
            if !(0.0..=1.0).contains(&along) || error_bound > super::super::OFFSET_TOLERANCE_MM {
                continue;
            }
            let line = Line::new([a[0], a[1]], [b[0], b[1]]);
            let axis = usize::from(dy.abs() > dx.abs());
            let mut cuts = vec![0.0, 1.0];
            for edge in &boundary.edges {
                check_cancel(cancel)?;
                let mut add = |p: Coord<f64>| {
                    let v = if axis == 0 { p.x } else { p.y };
                    cuts.push(((v - a[axis]) / (b[axis] - a[axis])).clamp(0.0, 1.0));
                };
                match line_intersection(line, *edge) {
                    Some(LineIntersection::SinglePoint { intersection, .. }) => add(intersection),
                    Some(LineIntersection::Collinear { intersection }) => {
                        add(intersection.start);
                        add(intersection.end);
                    }
                    None => {}
                }
            }
            cuts.sort_unstable_by(f64::total_cmp);
            let mut admitted = true;
            for cut in cuts.windows(2) {
                if (cut[1] - cut[0]) * length <= 0.000002 {
                    continue;
                }
                let start = [a[0] + cut[0] * dx, a[1] + cut[0] * dy];
                let end = [a[0] + cut[1] * dx, a[1] + cut[1] * dy];
                if !near_material(start, end, fill, boundary, cancel)? {
                    admitted = false;
                    break;
                }
            }
            if admitted {
                road.remove(middle);
                fit_errors[middle - 1] = error_bound;
                fit_errors.remove(middle);
                merged = true;
                break;
            }
        }
        if !merged {
            return Err(unsupported(&format!(
                "Gyroid clipped fragment cannot fit the minimum command within profile tolerance: edge={index} points={} near={:?}",
                road.len(),
                &road[index.saturating_sub(1)..(index + 3).min(road.len())]
            )));
        }
    }
    if road.len() >= 2 {
        reserve_segments(segments, road.len() - 1)?;
        push_path(
            paths,
            DepositionKind::SparseFill,
            std::mem::take(road),
            layer.width_mm,
            [layer.lower_z, layer.z],
        )?;
    } else {
        road.clear();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn generate(domain: &MultiPolygon<f64>, fraction: f64, z: f64) -> Vec<DepositionPath> {
        generate_at_height(domain, fraction, z, 0.2)
    }

    fn generate_at_height(
        domain: &MultiPolygon<f64>,
        fraction: f64,
        z: f64,
        height: f64,
    ) -> Vec<DepositionPath> {
        let mut recipe = super::super::super::tests::recipe(fraction);
        recipe.sparse_infill = SparseInfillPolicy::Gyroid;
        recipe.layer_height_mm = 0.2;
        let layer = HatchLayer {
            roof: None,
            width_mm: 0.45,
            z,
            lower_z: z - height,
            height,
            axis: HatchAxis::X,
            top_axis: HatchAxis::X,
            recipe: &recipe,
        };
        let mut output = Vec::new();
        hatch(
            domain,
            fraction,
            &layer,
            &mut output,
            &mut 0,
            &AtomicBool::new(false),
        )
        .unwrap();
        output
    }

    #[test]
    fn analytic_roots_match_independent_implicit_equation_at_orientation_transitions() {
        for z in [0.0, PI / 4.0, PI / 2.0, PI, 5.0 * PI / 4.0, 1.6 * PI] {
            let (sine_z, cosine_z) = z.sin_cos();
            let wave = Wave {
                sine_z,
                cosine_z,
                along_x: cosine_z.abs() >= sine_z.abs(),
                scale: 1.0,
            };
            for branch in 0..2 {
                for sample in 0..101 {
                    let u = sample as f64 * TAU / 100.0;
                    let v = wave.ordinate(u, branch);
                    let (x, y) = if wave.along_x { (u, v) } else { (v, u) };
                    let f = x.sin() * y.cos() + y.sin() * z.cos() + z.sin() * x.cos();
                    assert!(f.abs() < 1.0e-7, "z={z} x={x} y={y} F={f}");
                }
            }
        }
    }

    #[test]
    fn curved_clipping_preserves_holes_and_disconnected_regions() {
        let outer = geo::Rect::new([-24.0, -24.0], [24.0, 24.0]).to_polygon();
        let hole = geo::Rect::new([-5.0, -5.0], [5.0, 5.0]).to_polygon();
        let ring = Polygon::new(outer.exterior().clone(), vec![hole.exterior().clone()]);
        let domain = MultiPolygon(vec![
            ring,
            geo::Rect::new([30.0, 0.0], [40.0, 10.0]).to_polygon(),
        ]);
        let paths = generate(&domain, 0.15, 2.8);
        assert!(paths.iter().any(|p| p.points_mm.len() > 3));
        assert!(paths.iter().any(|p| p.points_mm[0][0] > 29.0));
        for path in paths {
            for edge in path.points_mm.windows(2) {
                for t in [0.1, 0.5, 0.9] {
                    let p = Point::new(
                        edge[0][0] + t * (edge[1][0] - edge[0][0]),
                        edge[0][1] + t * (edge[1][1] - edge[0][1]),
                    );
                    assert!(domain.intersects(&p), "clipped road crossed void: {edge:?}");
                }
            }
        }
    }

    #[test]
    fn cubecl_boundary_screen_preserves_curved_hole_clipping() {
        fn ring(radius: f64, sides: usize) -> LineString<f64> {
            LineString(
                (0..=sides)
                    .map(|i| {
                        let angle = i as f64 * TAU / sides as f64;
                        Coord {
                            x: 32.0 + radius * angle.cos(),
                            y: 32.0 + radius * angle.sin(),
                        }
                    })
                    .collect(),
            )
        }
        let domain = MultiPolygon(vec![Polygon::new(ring(28.0, 192), vec![ring(7.0, 96)])]);
        let paths = generate(&domain, 0.3, 3.4);
        assert!(!paths.is_empty());
        for path in paths {
            for edge in path.points_mm.windows(2) {
                let mid = Point::new(
                    (edge[0][0] + edge[1][0]) * 0.5,
                    (edge[0][1] + edge[1][1]) * 0.5,
                );
                assert!(domain.intersects(&mid));
                assert!((mid.x() - 32.0).hypot(mid.y() - 32.0) > 6.9);
            }
        }
    }

    /// Reproducible generation measurement; no timing threshold or speed claim.
    /// Run separately with SPILING_GYROID_COMPUTE=cpu / vulkan.
    #[test]
    #[ignore = "generation measurement, run explicitly with --nocapture"]
    fn measure_gyroid_generation() {
        let backend = std::env::var("SPILING_GYROID_COMPUTE").unwrap_or_else(|_| "cpu".into());
        let settings = spiling_compute::ComputeSettings {
            default_backend: spiling_compute::ComputeBackend::parse(&backend).unwrap(),
            fea_backend: None,
        };
        let status = spiling_compute::configure(settings).unwrap();
        eprintln!("device={:?}", status.default_device);
        for size in [48.0, 192.0] {
            for fraction in [0.05, 0.15, 0.30] {
                for curved in [false, true] {
                    let domain = if curved {
                        let ring = |radius: f64, sides: usize| {
                            LineString(
                                (0..=sides)
                                    .map(|i| {
                                        let angle = i as f64 * TAU / sides as f64;
                                        Coord {
                                            x: size * 0.5 + radius * angle.cos(),
                                            y: size * 0.5 + radius * angle.sin(),
                                        }
                                    })
                                    .collect(),
                            )
                        };
                        MultiPolygon(vec![Polygon::new(
                            ring(size * 0.5, 192),
                            vec![ring(size * 0.1, 96)],
                        )])
                    } else {
                        MultiPolygon(vec![geo::Rect::new([0.0, 0.0], [size, size]).to_polygon()])
                    };
                    // Warm backend/compiler caches before the measured layers.
                    generate(&domain, fraction, 0.2);
                    let started = std::time::Instant::now();
                    let mut paths = 0;
                    let mut segments = 0;
                    let mut length = 0.0;
                    for layer in 1..=20 {
                        let output = generate(&domain, fraction, layer as f64 * 0.2);
                        paths += output.len();
                        for path in output {
                            segments += path.points_mm.len() - 1;
                            length += path
                                .points_mm
                                .windows(2)
                                .map(|e| (e[1][0] - e[0][0]).hypot(e[1][1] - e[0][1]))
                                .sum::<f64>();
                        }
                    }
                    eprintln!(
                        "GYROID size={size} fraction={fraction} curved={curved} layers=20 ms={:.3} paths={paths} segments={segments} length_mm={length:.6}",
                        started.elapsed().as_secs_f64() * 1000.0
                    );
                }
            }
        }
    }

    #[test]
    fn scheduled_thickness_changes_dose_without_shifting_the_physical_height_field() {
        let domain = MultiPolygon(vec![
            geo::Rect::new([92.0, 92.0], [140.0, 140.0]).to_polygon(),
        ]);
        let nominal = generate_at_height(&domain, 0.15, 2.4, 0.2);
        assert!(!nominal.is_empty());
        for height in [0.05, 0.1, 0.15, 0.3] {
            let fine = generate_at_height(&domain, 0.15, 2.4, height);
            assert_eq!(fine.len(), nominal.len());
            for (a, b) in fine.iter().zip(&nominal) {
                assert_eq!(
                    a.points_mm, b.points_mm,
                    "event-split thickness shifted Gyroid phase"
                );
                assert!((a.height_mm() - height).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn density_tracks_stadium_volume_across_physical_z_and_zero_is_empty() {
        let domain = MultiPolygon(vec![geo::Rect::new([0.0, 0.0], [96.0, 96.0]).to_polygon()]);
        assert!(generate(&domain, 0.0, 0.2).is_empty());
        for fraction in [0.05, 0.15, 0.30] {
            let equivalent_width = 0.45 - 0.2 * (1.0 - PI / 4.0);
            let scale = equivalent_width / (DENSITY_MULTIPLIER * fraction);
            let mut length = 0.0;
            for index in 0..32 {
                let z = (index as f64 + 0.5) * TAU * scale / 32.0;
                for path in generate(&domain, fraction, z) {
                    length += path
                        .points_mm
                        .windows(2)
                        .map(|e| (e[1][0] - e[0][0]).hypot(e[1][1] - e[0][1]))
                        .sum::<f64>();
                }
            }
            let actual = length * equivalent_width / (96.0 * 96.0 * 32.0);
            assert!(
                (actual / fraction - 1.0).abs() < 0.06,
                "nominal={fraction}, actual={actual}"
            );
        }
    }
}
