// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Closed periodic construction on actual native edges, never an analytic replacement.
use super::{
    PeriodicProfile, SURFACE_ROUNDING_MM, SURFACE_TOLERANCE_MM, forward, proof, region, rotate,
    unsupported,
};
use crate::{CompilerInput, check_cancel, error};
use geo::kernels::{Kernel, Orientation, RobustKernel};
use geo::{Area, Coord, Intersects, LineString, MultiPolygon, Point, Polygon};
use spiling_contracts::geometry::{
    AabbMm, FaceId, GeometryErrorCode, OccurrenceRecord, SECTION_SAMPLING_TOLERANCE_MM,
};
use spiling_contracts::manufacturing::{
    MAX_DEPOSITION_SEGMENTS, ManufacturingError, ManufacturingErrorCode,
};
use std::{
    f64::consts::{PI, TAU},
    sync::atomic::AtomicBool,
};

pub(super) struct CylinderField {
    pub(super) face_id: FaceId,
    pub(super) bounds: AabbMm,
    pub(super) samples: usize,
    center: Coord<f64>,
    x: [f64; 2],
    y: [f64; 2],
    radius: f64,
    depth: f64,
    count: u32,
    phase: f64,
    arithmetic: f64,
    pub(super) image: Option<super::ImageMapping>,
}

fn resource(message: &str) -> ManufacturingError {
    error(ManufacturingErrorCode::ResourceLimit, message)
}
fn cross(a: Coord<f64>, b: Coord<f64>) -> f64 {
    a.x * b.y - a.y * b.x
}
fn sub(a: Coord<f64>, b: Coord<f64>) -> Coord<f64> {
    Coord {
        x: a.x - b.x,
        y: a.y - b.y,
    }
}
fn norm(a: Coord<f64>) -> f64 {
    a.x.hypot(a.y)
}

pub(super) fn metadata(
    input: &CompilerInput<'_>,
    occurrence: &OccurrenceRecord,
    source_bounds: AabbMm,
    face_id: &FaceId,
    depth: f64,
    count: u32,
    phase: f64,
    sample_limit: usize,
    cancel: &AtomicBool,
    image: Option<(
        &spiling_contracts::geometry::SourceHash,
        &spiling_contracts::surface::ImageReliefParameters,
        spiling_contracts::surface::SurfaceLayerId,
    )>,
) -> Result<CylinderField, ManufacturingError> {
    check_cancel(cancel)?;
    let native = &input.definitions[&occurrence.definition_id];
    let cert = native
        .vertical_cylinder_face_with_cancel(face_id, cancel)
        .map_err(|e| {
            error(
                match e.code {
                    GeometryErrorCode::Cancelled => ManufacturingErrorCode::Cancelled,
                    GeometryErrorCode::ResourceLimit => ManufacturingErrorCode::ResourceLimit,
                    _ => ManufacturingErrorCode::UnsupportedGeometry,
                },
                format!("surface face {}: {e}", face_id.as_str()),
            )
        })?;
    let image = image
        .map(|(hash, parameters, layer_id)| {
            super::ImageMapping::new(
                input,
                occurrence,
                hash,
                parameters,
                layer_id,
                cert.min_z_mm,
                cert.max_z_mm,
            )
        })
        .transpose()?;
    let bead = input.intent.recipe.bead_width_mm;
    if image
        .as_ref()
        .is_some_and(|image| image.parameters.edge_margin_mm != 0.0)
        || (image.is_none()
            && (count < 2
                || count > 256
                || !depth.is_finite()
                || !phase.is_finite()
                || !(0.0..1.0).contains(&phase)
                || depth <= 0.022
                || depth > 0.5 * bead
                || TAU * cert.radius_mm / (count as f64) < 4.0 * bead))
    {
        return Err(unsupported(
            face_id,
            "cylindrical ribs require count 2..=256, phase in [0,1), pitch >= four bead widths, depth > 0.022 mm and depth <= half a bead width",
        ));
    }
    super::admit_extrusion(
        input,
        occurrence,
        source_bounds,
        face_id,
        cert.min_z_mm,
        cert.max_z_mm,
        cancel,
    )?;
    let center = forward(
        &occurrence.pose,
        [cert.center_mm[0], cert.center_mm[1], cert.min_z_mm],
    );
    let x = rotate(&occurrence.pose, [1.0, 0.0, 0.0]);
    let y = rotate(&occurrence.pose, [0.0, 1.0, 0.0]);
    let top = forward(
        &occurrence.pose,
        [cert.center_mm[0], cert.center_mm[1], cert.max_z_mm],
    );
    let scale = cert
        .center_mm
        .into_iter()
        .chain(occurrence.pose.translation_mm)
        .chain([cert.radius_mm, cert.min_z_mm, cert.max_z_mm, depth])
        .fold(1.0f64, |a, b| a.max(b.abs()));
    let arithmetic = 4096.0 * f64::EPSILON * scale;
    let count = image
        .as_ref()
        .map_or(count, |i| i.image.record().width * i.parameters.u_repeats);
    let min_radius = cert.radius_mm - SECTION_SAMPLING_TOLERANCE_MM - 4.0 * arithmetic;
    let theta_error = 8.0 * arithmetic / min_radius + 64.0 * f64::EPSILON;
    let endpoint_error = 5.0 * arithmetic
        + depth * (count as f64 / 2.0 + 1.0) * theta_error
        + PI * depth * 64.0 * f64::EPSILON;
    let frame_ok = x
        .into_iter()
        .chain(y)
        .chain(center)
        .chain(top)
        .all(f64::is_finite)
        && (x[0] * x[0] + x[1] * x[1] - 1.0).abs() <= 64.0 * f64::EPSILON
        && (y[0] * y[0] + y[1] * y[1] - 1.0).abs() <= 64.0 * f64::EPSILON
        && (x[0] * y[0] + x[1] * y[1]).abs() <= 64.0 * f64::EPSILON;
    if !frame_ok
        || min_radius <= 0.0
        || !endpoint_error.is_finite()
        || endpoint_error
            > if image.is_some() {
                SURFACE_ROUNDING_MM * 3.0 / 8.0
            } else {
                SURFACE_ROUNDING_MM / 2.0
            }
    {
        return Err(unsupported(
            face_id,
            "source-local cylindrical frame/phase arithmetic exceeds the reserved construction bound",
        ));
    }
    // Convex radial native envelopes bound the total native perimeter. Native
    // endpoints are charged separately when the actual section arrives.
    let n = count as f64;
    let profile = PeriodicProfile::new(depth, n, TAU * phase);
    let lifted_bound = if let Some(image) = &image {
        4.0 * image.global_du() / TAU + 3.0 * depth
    } else {
        profile.second_derivative_bound()
            + 4.0 * profile.first_derivative_bound()
            + 3.0 * profile.maximum()
    };
    let factor = (lifted_bound / (8.0 * (SURFACE_TOLERANCE_MM - SURFACE_ROUNDING_MM))).sqrt();
    let required = (TAU * (cert.radius_mm + SECTION_SAMPLING_TOLERANCE_MM + arithmetic) * factor
        / min_radius)
        .ceil()
        + count as f64
        + 2.0;
    if !required.is_finite() || required > sample_limit as f64 {
        return Err(resource(
            "combined cylindrical profile sampling budget exceeded",
        ));
    }
    let extent = depth + SECTION_SAMPLING_TOLERANCE_MM + SURFACE_ROUNDING_MM;
    Ok(CylinderField {
        face_id: face_id.clone(),
        bounds: AabbMm {
            min: [
                source_bounds.min[0] - extent,
                source_bounds.min[1] - extent,
                center[2].min(top[2]),
            ],
            max: [
                source_bounds.max[0] + extent,
                source_bounds.max[1] + extent,
                center[2].max(top[2]),
            ],
        },
        samples: required as usize,
        center: Coord {
            x: center[0],
            y: center[1],
        },
        x: [x[0], x[1]],
        y: [y[0], y[1]],
        radius: cert.radius_mm,
        depth,
        count,
        phase,
        arithmetic,
        image,
    })
}
impl CylinderField {
    fn profile(&self) -> PeriodicProfile {
        PeriodicProfile::new(self.depth, self.count as f64, TAU * self.phase)
    }
    fn displacement(&self, theta: f64, sample: f64) -> f64 {
        self.image.as_ref().map_or_else(
            || self.profile().value(theta),
            |i| i.value(theta / TAU, sample),
        )
    }
    fn derivative_bounds(&self) -> (f64, f64, f64) {
        if let Some(i) = &self.image {
            (i.global_du() / TAU, 0.0, self.depth)
        } else {
            let p = self.profile();
            (
                p.first_derivative_bound(),
                p.second_derivative_bound(),
                p.maximum(),
            )
        }
    }
    pub(super) fn source_u(&self, point: Coord<f64>) -> f64 {
        (self.angle(point) / TAU).rem_euclid(1.0)
    }
    pub(super) fn owns_realized(
        &self,
        polygon: &Polygon<f64>,
        cancel: &AtomicBool,
    ) -> Result<bool, ManufacturingError> {
        let mut turn = 0.0;
        for edge in polygon.exterior().0.windows(2) {
            check_cancel(cancel)?;
            let a = sub(edge[0], self.center);
            let b = sub(edge[1], self.center);
            let delta = cross(a, b).atan2(a.x * b.x + a.y * b.y);
            turn += delta;
            let r = norm(a);
            if delta <= 0.0
                || delta >= PI / 2.0
                || r < self.radius - SECTION_SAMPLING_TOLERANCE_MM - SURFACE_ROUNDING_MM
                || r > self.radius
                    + self.depth
                    + SECTION_SAMPLING_TOLERANCE_MM
                    + SURFACE_ROUNDING_MM
            {
                return Ok(false);
            }
        }
        Ok((turn - TAU).abs() <= 256.0 * f64::EPSILON * polygon.exterior().0.len() as f64)
    }
    fn angle(&self, point: Coord<f64>) -> f64 {
        let v = sub(point, self.center);
        (v.x * self.y[0] + v.y * self.y[1]).atan2(v.x * self.x[0] + v.y * self.x[1])
    }
    fn ray(&self, theta: f64) -> Coord<f64> {
        let (s, c) = theta.sin_cos();
        Coord {
            x: c * self.x[0] + s * self.y[0],
            y: c * self.x[1] + s * self.y[1],
        }
    }
    fn edge_geometry(&self, a: Coord<f64>, b: Coord<f64>) -> (f64, f64) {
        let v = sub(b, a);
        let length = norm(v);
        let lower_length = length - 4.0 * self.arithmetic;
        // Center/endpoint subtraction and normalization sensitivity grow for
        // short edges. Never treat a tiny uncertain chord as a conditioned line.
        let error = self.arithmetic * (8.0 + 8.0 * norm(sub(a, self.center)) / lower_length);
        let distance = if lower_length > 0.0 {
            cross(sub(a, self.center), v) / length - error
        } else {
            f64::NEG_INFINITY
        };
        (length + 4.0 * self.arithmetic, distance)
    }
    fn edge_intervals(&self, a: Coord<f64>, b: Coord<f64>) -> Result<usize, ManufacturingError> {
        let (length, distance) = self.edge_geometry(a, b);
        if !length.is_finite() || !distance.is_finite() || distance <= 0.0 {
            return Err(unsupported(
                &self.face_id,
                "native cylindrical edge has no conditioned positive axis distance",
            ));
        }
        let (first, second, maximum) = self.derivative_bounds();
        let bound = (length / distance).powi(2) * (second + 4.0 * first + 3.0 * maximum);
        let count = (bound / (8.0 * (SURFACE_TOLERANCE_MM - SURFACE_ROUNDING_MM)))
            .sqrt()
            .ceil()
            .max(1.0);
        if !count.is_finite() || count > MAX_DEPOSITION_SEGMENTS as f64 / 4.0 {
            return Err(resource(
                "native-edge cylindrical subdivision exceeds sampling budget",
            ));
        }
        Ok(count as usize)
    }
}

fn owner(
    source: &MultiPolygon<f64>,
    field: &CylinderField,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<usize, ManufacturingError> {
    let mut owner = None;
    for (index, polygon) in source.0.iter().enumerate() {
        if owns_native_island(polygon, field, work, cancel)? && owner.replace(index).is_some() {
            return Err(unsupported(
                &field.face_id,
                "cylindrical certificate matches more than one native exterior ring",
            ));
        }
    }
    owner.ok_or_else(|| unsupported(&field.face_id, "full cylindrical native exterior is not convex, full-turn, axis-interior and within the native radial envelope"))
}

pub(super) fn owns_native_island(
    polygon: &Polygon<f64>,
    field: &CylinderField,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    let ring = polygon.exterior();
    if ring.0.len() < 4 || ring.0.first() != ring.0.last() {
        return Ok(false);
    }
    proof::charge(work, ring.0.len())?;
    let mut valid = true;
    let mut turn = 0.0;
    for (i, edge) in ring.0.windows(2).enumerate() {
        check_cancel(cancel)?;
        let a = sub(edge[0], field.center);
        let b = sub(edge[1], field.center);
        let next = ring.0[(i + 2) % (ring.0.len() - 1)];
        let (_, d) = field.edge_geometry(edge[0], edge[1]);
        let delta = cross(a, b).atan2(a.x * b.x + a.y * b.y);
        valid &= RobustKernel::orient2d(edge[0], edge[1], field.center)
            == Orientation::CounterClockwise
            && RobustKernel::orient2d(edge[0], edge[1], next) != Orientation::Clockwise
            && delta > 0.0
            && delta < PI / 2.0
            && RobustKernel::dot_product_sign(a, b) == Orientation::CounterClockwise
            && norm(a) <= field.radius + SECTION_SAMPLING_TOLERANCE_MM - field.arithmetic
            && d >= field.radius - SECTION_SAMPLING_TOLERANCE_MM;
        turn += delta;
    }
    Ok(valid
        && turn.is_finite()
        && (turn - TAU).abs() <= 256.0 * f64::EPSILON * ring.0.len() as f64)
}

fn evaluate(
    field: &CylinderField,
    a: Coord<f64>,
    b: Coord<f64>,
    t: f64,
    previous: Coord<f64>,
    sample: f64,
) -> Result<Coord<f64>, ManufacturingError> {
    let base = if t == 0.0 {
        a
    } else {
        Coord {
            x: a.x + (b.x - a.x) * t,
            y: a.y + (b.y - a.y) * t,
        }
    };
    let radial = sub(base, field.center);
    let radius = norm(radial);
    let depth = field.displacement(field.angle(base), sample);
    if depth == 0.0 && t == 0.0 {
        return Ok(a);
    }
    let mut point = Coord {
        x: base.x + depth * radial.x / radius,
        y: base.y + depth * radial.y / radius,
    };
    let nominal = point;
    // Outward directed rounding, not a tolerance accepting missing material.
    // Native vertices must satisfy BOTH adjacent original supporting lines.
    for _ in 0..=8 {
        let outside = RobustKernel::orient2d(a, b, point) != Orientation::CounterClockwise
            && (t != 0.0
                || RobustKernel::orient2d(previous, a, point) != Orientation::CounterClockwise);
        if outside {
            if norm(sub(point, nominal)) + field.arithmetic <= SURFACE_ROUNDING_MM / 2.0 {
                return Ok(point);
            }
            break;
        }
        point.x = if radial.x > 0.0 {
            point.x.next_up()
        } else if radial.x < 0.0 {
            point.x.next_down()
        } else {
            point.x
        };
        point.y = if radial.y > 0.0 {
            point.y.next_up()
        } else if radial.y < 0.0 {
            point.y.next_down()
        } else {
            point.y
        };
    }
    Err(unsupported(
        &field.face_id,
        "rounded cylindrical sample cannot retain both native edge supports within the construction reserve",
    ))
}

pub(super) fn realize(
    source: MultiPolygon<f64>,
    field: &CylinderField,
    sample: f64,
    retained: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    check_cancel(cancel)?;
    if field.image.as_ref().is_some_and(|i| i.zero_row(sample)) {
        return Ok(source);
    }
    if sample <= field.bounds.min[2] || sample >= field.bounds.max[2] {
        return Err(unsupported(
            &field.face_id,
            "section lies outside the certified full-height cylinder interval",
        ));
    }
    let index = owner(&source, field, work, cancel)?;
    let old = source.0[index].exterior();
    let edges = old.0.len() - 1;
    let source_points = region::point_count(&source);
    let capacity = field
        .samples
        .checked_add(edges)
        .ok_or_else(|| resource("cylindrical point accounting overflow"))?;
    retained
        .checked_add(14 * source_points)
        .and_then(|v| v.checked_add(8 * capacity))
        .filter(|v| *v <= MAX_DEPOSITION_SEGMENTS as usize)
        .ok_or_else(|| resource("native cylindrical construction staging point budget exceeded"))?;
    // Sampling is pre-admitted; mandatory robust boundary proofs subsequently
    // admit their bounded index/sort and charge every exact candidate predicate.
    // A global Cartesian product is not the work performed by that proof.
    let sampling_work = edges
        .checked_mul(field.count as usize * 16)
        .and_then(|v| v.checked_add(capacity))
        .ok_or_else(|| resource("cylindrical sampling work overflow"))?;
    if work
        .checked_add(sampling_work)
        .is_none_or(|v| v > super::super::MAX_BOOLEAN_PAIRS)
    {
        return Err(resource(
            "cumulative cylindrical sampling work budget exceeded",
        ));
    }
    let mut points = Vec::with_capacity(capacity);
    let mut peaks = Vec::with_capacity(field.count as usize);
    for (i, edge) in old.0.windows(2).enumerate() {
        check_cancel(cancel)?;
        let intervals = field.edge_intervals(edge[0], edge[1])?;
        peaks.clear();
        proof::charge(work, field.count as usize * 16 + intervals)?;
        let v = sub(edge[1], edge[0]);
        let (edge_length, edge_distance) = field.edge_geometry(edge[0], edge[1]);
        let (first, _, maximum) = field.derivative_bounds();
        let speed = edge_length + (first + maximum) * edge_length / edge_distance;
        for peak in 0..field.count {
            check_cancel(cancel)?;
            let theta = if let Some(image) = &field.image {
                TAU * (peak as f64 / image.image.record().width as f64
                    - image.parameters.u_phase_fraction)
                    / image.parameters.u_repeats as f64
            } else {
                TAU * (peak as f64 + field.phase + 0.5) / field.count as f64
            };
            let ray = field.ray(theta);
            let denominator = cross(v, ray);
            if denominator == 0.0 {
                continue;
            }
            let t = cross(sub(field.center, edge[0]), ray) / denominator;
            if t > 0.0 && t < 1.0 {
                // Arithmetic-coincident peak/uniform/native endpoints share the
                // existing node, never a snapped source parameter. The Lipschitz
                // lifted-profile uncertainty is reserved in endpoint_error.
                let nearest = (t * intervals as f64).round() / intervals as f64;
                if field.image.is_none() && (t - nearest).abs() * speed <= field.arithmetic {
                    continue;
                }
                let p = Coord {
                    x: edge[0].x + t * v.x - field.center.x,
                    y: edge[0].y + t * v.y - field.center.y,
                };
                if p.x * ray.x + p.y * ray.y > 0.0 {
                    peaks.push(t);
                }
            }
        }
        peaks.sort_by(f64::total_cmp);
        let mut peak = 0usize;
        let mut uniform = 0usize;
        while uniform < intervals || peak < peaks.len() {
            check_cancel(cancel)?;
            let u = if uniform < intervals {
                uniform as f64 / intervals as f64
            } else {
                1.0
            };
            let p = peaks.get(peak).copied().unwrap_or(1.0);
            let t = u.min(p);
            if u == t {
                uniform += 1;
            }
            if p == t {
                peak += 1;
            }
            if points.len() + 1 >= capacity {
                return Err(resource(
                    "actual native cylindrical sampling exceeds its admitted profile bound",
                ));
            }
            let previous = old.0[(i + edges - 1) % edges];
            let point = evaluate(field, edge[0], edge[1], t, previous, sample)?;
            if let Some(last) = points.last().copied() {
                // All endpoints remain outside their one owning native segment.
                // At the start of an edge the prior point belongs to the prior
                // interval; its own proof was performed in that interval.
                if t > 0.0
                    && RobustKernel::orient2d(edge[0], edge[1], last)
                        == Orientation::CounterClockwise
                {
                    return Err(unsupported(
                        &field.face_id,
                        "cylindrical chord leaves its native-edge supporting exterior",
                    ));
                }
            }
            points.push(point);
        }
    }
    points.push(points[0]); // Exact seam copy, no second phase evaluation.
    let derived = LineString(points);
    let shell = Polygon::new(derived, Vec::new());
    if shell.signed_area() <= 0.0 {
        return Err(unsupported(
            &field.face_id,
            "cylindrical output reverses native winding",
        ));
    }
    proof::charge(work, edges * (shell.exterior().0.len() - 1))?;
    for p in &old.0 {
        check_cancel(cancel)?;
        if !shell.intersects(&Point(*p)) {
            return Err(unsupported(
                &field.face_id,
                "cylindrical output loses a native material vertex",
            ));
        }
    }
    proof::certify_retention(old, shell.exterior(), work, cancel)?;
    let source_area = source.unsigned_area();
    let (boundary, _) = shell.into_inner();
    let mut result = source;
    // Move hole buffers unchanged, retaining exact coordinates and order.
    let original = std::mem::replace(
        &mut result.0[index],
        Polygon::new(LineString(Vec::new()), Vec::new()),
    );
    let (_, holes) = original.into_inner();
    result.0[index] = Polygon::new(boundary, holes);
    proof::certify_simple(&result, work, cancel)?;
    region::check_size(&result)?;
    if result.unsigned_area() <= source_area {
        return Err(unsupported(
            &field.face_id,
            "cylindrical construction produced no strictly additive material",
        ));
    }
    Ok(result)
}

pub(super) fn certify_first_wall(
    centers: &MultiPolygon<f64>,
    field: &CylinderField,
    sample: f64,
    bead: f64,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    if sample <= field.bounds.min[2] || sample >= field.bounds.max[2] {
        return Ok(());
    }
    let error_bound =
        SECTION_SAMPLING_TOLERANCE_MM + SURFACE_TOLERANCE_MM + super::super::OFFSET_TOLERANCE_MM;
    let expected = field.radius + field.depth - bead / 2.0;
    let mut matching = None;
    for polygon in &centers.0 {
        check_cancel(cancel)?;
        let ring = polygon.exterior();
        // Attribute a full angular exterior, not an unrelated island supplying
        // a convenient maximum. The axis may lie in an unchanged protected hole.
        let mut turn = 0.0;
        let mut maximum = 0.0f64;
        let mut valid = true;
        for edge in ring.0.windows(2) {
            check_cancel(cancel)?;
            let a = sub(edge[0], field.center);
            let b = sub(edge[1], field.center);
            let delta = cross(a, b).atan2(a.x * b.x + a.y * b.y);
            valid &= delta > 0.0
                && delta < PI / 2.0
                && RobustKernel::orient2d(edge[0], edge[1], field.center)
                    == Orientation::CounterClockwise
                && norm(a) >= field.radius - bead / 2.0 - error_bound - field.arithmetic
                && norm(a) <= expected + error_bound + field.arithmetic;
            turn += delta;
            maximum = maximum.max(norm(a));
        }
        valid &= (turn - TAU).abs() <= 256.0 * f64::EPSILON * ring.0.len() as f64;
        if valid && matching.replace(maximum).is_some() {
            return Err(unsupported(
                &field.face_id,
                "ordinary first wall has ambiguous cylindrical exterior attribution",
            ));
        }
    }
    let peak = matching.ok_or_else(|| {
        unsupported(
            &field.face_id,
            "ordinary first wall lost the attributed full cylindrical exterior",
        )
    })?;
    if peak - field.arithmetic < expected - error_bound
        || peak + field.arithmetic > expected + error_bound
    {
        return Err(unsupported(
            &field.face_id,
            "requested cylindrical radial peak did not survive the ordinary first-wall offset",
        ));
    }
    Ok(())
}

pub(super) fn witness(
    field: &CylinderField,
    ring: &LineString<f64>,
    point: Coord<f64>,
    sample: f64,
    cancel: &AtomicBool,
) -> Result<Option<(Coord<f64>, f64)>, ManufacturingError> {
    let ray = sub(point, field.center);
    let count = ring.0.len().saturating_sub(1);
    for (index, edge) in ring.0.windows(2).enumerate() {
        check_cancel(cancel)?;
        let v = sub(edge[1], edge[0]);
        let denominator = cross(v, ray);
        if denominator == 0.0 {
            continue;
        }
        let parameter = cross(sub(field.center, edge[0]), ray) / denominator;
        let t = parameter.clamp(0.0, 1.0);
        if (t - parameter).abs() * norm(v) > field.arithmetic {
            continue;
        }
        let base = Coord {
            x: edge[0].x + t * v.x,
            y: edge[0].y + t * v.y,
        };
        let radial = sub(base, field.center);
        if radial.x * ray.x + radial.y * ray.y <= 0.0 {
            continue;
        }
        let previous = ring.0[(index + count - 1) % count];
        let lifted = if t == 1.0 {
            evaluate(
                field,
                edge[1],
                ring.0[(index + 2) % count],
                0.0,
                edge[0],
                sample,
            )?
        } else {
            evaluate(field, edge[0], edge[1], t, previous, sample)?
        };
        return Ok(Some((lifted, index as f64 + t)));
    }
    Ok(None)
}
#[cfg(test)]
mod tests {
    use super::*;
    use geo::MapCoords;

    // An algorithm fixture, not evidence that imported cylindrical holes are
    // certified. Unequal endpoint radii exercise the actual-edge L/d bound.
    fn closed_native_ring(samples: usize, seam: usize) -> (MultiPolygon<f64>, CylinderField) {
        let center = Coord { x: 3.0, y: -2.0 };
        let mut outer = (0..samples)
            .map(|i| {
                let theta = TAU * i as f64 / samples as f64;
                let r = 5.0 + 0.0002 * (3.0 * theta).sin();
                Coord {
                    x: center.x + r * theta.cos(),
                    y: center.y + r * theta.sin(),
                }
            })
            .collect::<Vec<_>>();
        outer.rotate_left(seam);
        outer.push(outer[0]);
        let hole = LineString(vec![
            Coord { x: 2.0, y: -3.0 },
            Coord { x: 2.0, y: -1.0 },
            Coord { x: 4.0, y: -1.0 },
            Coord { x: 4.0, y: -3.0 },
            Coord { x: 2.0, y: -3.0 },
        ]);
        let field = CylinderField {
            face_id: FaceId::from_step_entity(1).unwrap(),
            bounds: AabbMm {
                min: [-2.13, -7.13, 0.0],
                max: [8.13, 3.13, 8.0],
            },
            samples: 1000,
            center,
            x: [1.0, 0.0],
            y: [0.0, 1.0],
            radius: 5.0,
            depth: 0.12,
            count: 7,
            phase: 0.31,
            arithmetic: 4096.0 * f64::EPSILON * 8.0,
            image: None,
        };
        (
            MultiPolygon(vec![Polygon::new(LineString(outer), vec![hole])]),
            field,
        )
    }

    #[test]
    fn required_policy_preserves_cylindrical_ribs_but_not_image_additions() {
        use super::super::{PreparedFinish, PreparedShape, realize_required};
        use geo::{Intersects, Point};
        let (source, rib) = closed_native_ring(128, 19);
        let cancel = AtomicBool::new(false);
        let fields = [PreparedFinish {
            shape: PreparedShape::Cylindrical(rib),
        }];
        assert!(!fields[0].is_image());
        let required = realize_required(source.clone(), &fields, 2.0, 0, &mut 0, &cancel).unwrap();
        assert!(required.unsigned_area() > source.unsigned_area());
        assert_eq!(required.0[0].interiors(), source.0[0].interiors());
        for point in &source.0[0].exterior().0 {
            assert!(required.0[0].intersects(&Point(*point)));
        }
        // Independently evaluate each strict rib peak in its source XY frame.
        for peak in 0..7 {
            let angle = (TAU * 0.31 + std::f64::consts::PI + TAU * peak as f64) / 7.0;
            let point = Point::new(3.0 + 5.11 * angle.cos(), -2.0 + 5.11 * angle.sin());
            assert!(required.0[0].intersects(&point));
        }
        let (source, mut image) = closed_native_ring(128, 19);
        image.image = Some(super::super::tests::mapping(image.depth, 0.0));
        let fields = [PreparedFinish {
            shape: PreparedShape::Cylindrical(image),
        }];
        assert!(fields[0].is_image());
        assert_eq!(
            source,
            realize_required(source.clone(), &fields, 2.0, 0, &mut 0, &cancel).unwrap()
        );
        let requested =
            super::super::realize(source.clone(), &fields, 2.0, 0, &mut 0, &cancel).unwrap();
        assert!(requested.unsigned_area() > source.unsigned_area());
    }

    #[test]
    fn image_rows_native_chords_phase_wrap_and_holes_are_not_analytic_circle_substitutes() {
        let (source, mut field) = closed_native_ring(128, 19);
        field.image = Some(super::super::tests::mapping(field.depth, 0.0));
        field.count = 3;
        field.phase = 0.17;
        let cancel = AtomicBool::new(false);
        assert!(owns_native_island(&source.0[0], &field, &mut 0, &cancel).unwrap());
        let other_island = source.0[0].map_coords(|point| Coord {
            x: point.x + 0.01,
            y: point.y,
        });
        assert!(!owns_native_island(&other_island, &field, &mut 0, &cancel).unwrap());
        let blank = realize(source.clone(), &field, 0.5, 0, &mut 0, &cancel).unwrap();
        assert_eq!(blank, source);
        let derived = realize(source.clone(), &field, 2.0, 0, &mut 0, &cancel).unwrap();
        assert_eq!(source.0[0].interiors(), derived.0[0].interiors());
        assert_eq!(
            derived.0[0].exterior().0.first(),
            derived.0[0].exterior().0.last()
        );
        for p in &source.0[0].exterior().0 {
            assert!(derived.0[0].intersects(&Point(*p)));
        }
        for chord in derived.0[0].exterior().0.windows(2) {
            let middle = Coord {
                x: (chord[0].x + chord[1].x) / 2.0,
                y: (chord[0].y + chord[1].y) / 2.0,
            };
            let ray = sub(middle, field.center);
            let edge = source.0[0]
                .exterior()
                .0
                .windows(2)
                .find(|edge| {
                    cross(sub(edge[0], field.center), ray) >= 0.0
                        && cross(ray, sub(edge[1], field.center)) >= 0.0
                })
                .unwrap();
            let v = sub(edge[1], edge[0]);
            let base = |point: Coord<f64>| {
                let ray = sub(point, field.center);
                let multiplier = cross(sub(edge[0], field.center), v) / cross(ray, v);
                Coord {
                    x: field.center.x + multiplier * ray.x,
                    y: field.center.y + multiplier * ray.y,
                }
            };
            let a = base(chord[0]);
            let b = base(chord[1]);
            for t in [0.125, 0.25, 0.5, 0.75, 0.875] {
                let p = Coord {
                    x: a.x + t * (b.x - a.x),
                    y: a.y + t * (b.y - a.y),
                };
                let radial = sub(p, field.center);
                let height = field
                    .image
                    .as_ref()
                    .unwrap()
                    .value(field.angle(p) / TAU, 2.0);
                let exact = Coord {
                    x: p.x + height * radial.x / norm(radial),
                    y: p.y + height * radial.y / norm(radial),
                };
                let chord_point = Coord {
                    x: chord[0].x + t * (chord[1].x - chord[0].x),
                    y: chord[0].y + t * (chord[1].y - chord[0].y),
                };
                assert!(norm(sub(exact, chord_point)) <= SURFACE_TOLERANCE_MM);
            }
        }
        let mut flipped = super::super::tests::mapping(field.depth, 0.0);
        flipped.translation_z = 3.0;
        flipped.sigma = -1.0;
        field.image = Some(flipped);
        let reversed = realize(source.clone(), &field, 1.0, 0, &mut 0, &cancel).unwrap();
        assert_eq!(reversed, derived);
        let placed_source = source.map_coords(|point| Coord {
            x: -point.y + 11.0,
            y: point.x - 7.0,
        });
        field.center = Coord {
            x: -field.center.y + 11.0,
            y: field.center.x - 7.0,
        };
        field.x = [0.0, 1.0];
        field.y = [-1.0, 0.0];
        field.arithmetic = 4096.0 * f64::EPSILON * 16.0;
        let placed = realize(placed_source, &field, 1.0, 0, &mut 0, &cancel).unwrap();
        let expected = derived.map_coords(|point| Coord {
            x: -point.y + 11.0,
            y: point.x - 7.0,
        });
        assert_eq!(
            placed.0[0].exterior().0.len(),
            expected.0[0].exterior().0.len()
        );
        for (actual, expected) in placed.0[0]
            .exterior()
            .0
            .iter()
            .zip(&expected.0[0].exterior().0)
        {
            assert!(norm(sub(*actual, *expected)) <= SURFACE_ROUNDING_MM);
        }
    }

    #[test]
    fn periodic_native_edges_retain_exact_holes_material_and_per_edge_chord_bound() {
        let (source, field) = closed_native_ring(128, 0);
        let cancel = AtomicBool::new(false);
        let derived = realize(source.clone(), &field, 2.0, 0, &mut 0, &cancel).unwrap();
        assert_eq!(derived.0[0].interiors(), source.0[0].interiors());
        assert_eq!(
            derived.0[0].exterior().0.first(),
            derived.0[0].exterior().0.last()
        );
        assert!(
            source.0[0]
                .exterior()
                .0
                .iter()
                .all(|p| derived.0[0].intersects(&Point(*p)))
        );
        assert!(derived.unsigned_area() > source.unsigned_area());
        let native = source.0[0].exterior();
        for chord in derived.0[0].exterior().0.windows(2) {
            let middle = Coord {
                x: (chord[0].x + chord[1].x) / 2.0,
                y: (chord[0].y + chord[1].y) / 2.0,
            };
            let ray = sub(middle, field.center);
            let edge = native
                .0
                .windows(2)
                .find(|edge| {
                    cross(sub(edge[0], field.center), ray) >= 0.0
                        && cross(ray, sub(edge[1], field.center)) >= 0.0
                })
                .unwrap();
            let v = sub(edge[1], edge[0]);
            let base = |p: Coord<f64>| {
                let ray = sub(p, field.center);
                let multiplier = cross(sub(edge[0], field.center), v) / cross(ray, v);
                Coord {
                    x: field.center.x + multiplier * ray.x,
                    y: field.center.y + multiplier * ray.y,
                }
            };
            let endpoints = [base(chord[0]), base(chord[1])];
            for t in [0.125, 0.25, 0.5, 0.75, 0.875] {
                let p = Coord {
                    x: endpoints[0].x + t * (endpoints[1].x - endpoints[0].x),
                    y: endpoints[0].y + t * (endpoints[1].y - endpoints[0].y),
                };
                let radial = sub(p, field.center);
                let f = field.profile().value(field.angle(p));
                let exact = Coord {
                    x: p.x + f * radial.x / norm(radial),
                    y: p.y + f * radial.y / norm(radial),
                };
                let interpolated = Coord {
                    x: chord[0].x + t * (chord[1].x - chord[0].x),
                    y: chord[0].y + t * (chord[1].y - chord[0].y),
                };
                assert!(norm(sub(exact, interpolated)) <= SURFACE_TOLERANCE_MM);
            }
        }
    }

    #[test]
    fn periodic_native_ring_refuses_proof_retention_and_cancel_exhaustion() {
        let (source, field) = closed_native_ring(128, 0);
        assert_eq!(
            realize(
                source.clone(),
                &field,
                2.0,
                0,
                &mut 0,
                &AtomicBool::new(true)
            )
            .unwrap_err()
            .code,
            ManufacturingErrorCode::Cancelled
        );
        assert_eq!(
            realize(
                source.clone(),
                &field,
                2.0,
                MAX_DEPOSITION_SEGMENTS as usize,
                &mut 0,
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .code,
            ManufacturingErrorCode::ResourceLimit
        );
        let mut exhausted = super::super::super::MAX_BOOLEAN_PAIRS;
        assert_eq!(
            realize(
                source,
                &field,
                2.0,
                0,
                &mut exhausted,
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .code,
            ManufacturingErrorCode::ResourceLimit
        );
    }

    #[test]
    fn native_periodic_subdivision_and_ring_seam_do_not_define_source_phase() {
        for (samples, seam) in [(128, 0), (192, 17), (256, 79)] {
            let (source, field) = closed_native_ring(samples, seam);
            let derived = realize(
                source.clone(),
                &field,
                2.0,
                0,
                &mut 0,
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(source.0[0].interiors(), derived.0[0].interiors());
            assert_eq!(
                derived.0[0].exterior().0.first(),
                derived.0[0].exterior().0.last()
            );
            for point in &derived.0[0].exterior().0 {
                let actual = norm(sub(*point, field.center)) - field.radius;
                let expected = field.profile().value(field.angle(*point));
                assert!(
                    (actual - expected).abs()
                        <= SECTION_SAMPLING_TOLERANCE_MM + SURFACE_ROUNDING_MM
                );
            }
        }
    }
}
