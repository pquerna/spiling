// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Bounded manufacturing construction, not an edited BREP or physical certificate.
use super::{forward, region, rotate};
use crate::{CompilerInput, check_cancel, error};
use geo::{Coord, MultiPolygon};
use spiling_contracts::geometry::{
    AabbMm, FaceCarrier, FaceId, GeometryErrorCode, OccurrenceRecord,
};
use spiling_contracts::manufacturing::*;
use spiling_contracts::surface::{SurfaceFinish, SurfaceScope};
use std::{collections::BTreeSet, sync::atomic::AtomicBool};

pub(super) const SURFACE_TOLERANCE_MM: f64 = 0.001;
const SURFACE_ROUNDING_MM: f64 = 0.00001;
mod splice;

pub(super) struct RibField {
    pub face_id: FaceId,
    pub bounds: AabbMm,
    origin: [f64; 3],
    horizontal: [f64; 3],
    normal: [f64; 3],
    width: f64,
    depth: f64,
    pitch: f64,
    margin: f64,
    phase: f64,
    overlap: f64,
    samples: usize,
    ramp_samples: usize,
    interior_samples: usize,
    peak_mm: f64,
}

/// Validate even disabled/shadowed targets: saved native identity remains authority.
pub(super) fn validate_targets(
    input: &CompilerInput<'_>,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    input.surface_overlays.validate().map_err(|e| {
        error(
            if e.code == spiling_contracts::project::ProjectErrorCode::ResourceLimit {
                ManufacturingErrorCode::ResourceLimit
            } else {
                ManufacturingErrorCode::InvalidSpecification
            },
            e.to_string(),
        )
    })?;
    for layer in &input.surface_overlays.layers {
        check_cancel(cancel)?;
        let definition_id = match &layer.scope {
            SurfaceScope::Definition { definition_id } => definition_id,
            SurfaceScope::Occurrence {
                occurrence_id,
                expected_definition_id,
            } => {
                if !input.occurrences.iter().any(|o| {
                    o.occurrence_id == *occurrence_id && o.definition_id == *expected_definition_id
                }) {
                    return Err(error(
                        ManufacturingErrorCode::InvalidSpecification,
                        "surface occurrence target is missing or has a different definition",
                    ));
                }
                expected_definition_id
            }
        };
        let native = input.definitions.get(definition_id).ok_or_else(|| {
            error(
                ManufacturingErrorCode::InvalidSpecification,
                "surface definition target is missing",
            )
        })?;
        if !native.faces().iter().any(|f| f.face_id == layer.face_id) {
            return Err(error(
                ManufacturingErrorCode::InvalidSpecification,
                "surface FaceId does not exist in the exact native source face table",
            ));
        }
    }
    Ok(())
}

/// Native metadata only: this does not prove the section/offset/anchor gates.
pub(super) fn metadata(
    input: &CompilerInput<'_>,
    occurrence: &OccurrenceRecord,
    source_bounds: AabbMm,
    sample_limit: usize,
    cancel: &AtomicBool,
) -> Result<Vec<RibField>, ManufacturingError> {
    let native = &input.definitions[&occurrence.definition_id];
    let mut result = Vec::new();
    let mut samples = 0usize;
    let candidates = input
        .surface_overlays
        .layers
        .iter()
        .filter(|layer| {
            layer.enabled
                && match &layer.scope {
                    SurfaceScope::Definition { definition_id } => {
                        *definition_id == occurrence.definition_id
                    }
                    SurfaceScope::Occurrence {
                        occurrence_id,
                        expected_definition_id,
                    } => {
                        *occurrence_id == occurrence.occurrence_id
                            && *expected_definition_id == occurrence.definition_id
                    }
                }
        })
        .map(|layer| &layer.face_id)
        .collect::<BTreeSet<_>>();
    if candidates.is_empty() {
        return Ok(result);
    }
    for face in native.faces() {
        check_cancel(cancel)?;
        if !candidates.contains(&face.face_id) {
            continue;
        }
        let Some(SurfaceFinish::VerticalRibs {
            depth_mm,
            pitch_mm,
            edge_margin_mm,
            phase_mm,
        }) = input.surface_overlays.effective_finish(
            &occurrence.definition_id,
            occurrence.occurrence_id,
            &face.face_id,
        )
        else {
            continue;
        };
        let width = input.intent.recipe.bead_width_mm;
        if *pitch_mm < 4.0 * width
            || *edge_margin_mm < 2.0 * width
            || *depth_mm < 4.0 * SURFACE_TOLERANCE_MM
            || *depth_mm > 0.5 * width
        {
            return Err(unsupported(
                &face.face_id,
                "ribs require pitch >= 4 bead widths, protected edge margin >= 2 bead widths, depth >= 4 finish approximation bounds and depth <= half a bead width",
            ));
        }
        let vertical = rotate(&occurrence.pose, [0.0, 0.0, 1.0]);
        if vertical[0] != 0.0
            || vertical[1] != 0.0
            || (vertical[2].abs() - 1.0).abs() > 8.0 * f64::EPSILON
        {
            return Err(unsupported(
                &face.face_id,
                "vertical ribs require source local Z to map to positive or negative build Z without tilt",
            ));
        }
        if (source_bounds.min[2] - input.intent.printer.build_envelope.min[2]).abs()
            > super::LAYER_QUANTIZATION_MM
        {
            return Err(unsupported(
                &face.face_id,
                "vertical ribs require a bed-anchored native extrusion",
            ));
        }
        let cert = native
            .vertical_wall_face_with_cancel(&face.face_id, cancel)
            .map_err(|e| {
                error(
                    match e.code {
                        GeometryErrorCode::Cancelled => ManufacturingErrorCode::Cancelled,
                        GeometryErrorCode::ResourceLimit => ManufacturingErrorCode::ResourceLimit,
                        _ => ManufacturingErrorCode::UnsupportedGeometry,
                    },
                    format!("surface face {}: {e}", face.face_id.as_str()),
                )
            })?;
        // Restrict the whole source to invariant extrusion carriers. Intermediate
        // horizontal shelves and tilted cylinders cannot inherit a face certificate.
        if result.is_empty() {
            for carrier in native.faces() {
                check_cancel(cancel)?;
                let invariant = match &carrier.carrier {
                    FaceCarrier::Plane { origin_mm, normal } => {
                        if normal[2] == 0.0 {
                            true
                        } else {
                            normal[0] == 0.0
                                && normal[1] == 0.0
                                && ((origin_mm[2] - cert.min_z_mm).abs()
                                    <= super::LAYER_QUANTIZATION_MM
                                    || (origin_mm[2] - cert.max_z_mm).abs()
                                        <= super::LAYER_QUANTIZATION_MM)
                        }
                    }
                    FaceCarrier::Cylinder { axis_direction, .. } => {
                        axis_direction[0] == 0.0 && axis_direction[1] == 0.0
                    }
                };
                if !invariant {
                    return Err(unsupported(
                        &face.face_id,
                        "vertical ribs require an invariant native extrusion with no intermediate shelf or sloped carrier",
                    ));
                }
            }
        }
        let active = cert.width_mm - 2.0 * edge_margin_mm;
        if !active.is_finite() || active < *pitch_mm {
            return Err(unsupported(
                &face.face_id,
                "selected face is too narrow for the protected corner bands and one full rib pitch",
            ));
        }
        // Chord error <=0.00099 mm plus native-frame agreement/directed rounding
        // <=0.00001 mm retains the fixed total 0.001 mm construction bound.
        let k = std::f64::consts::TAU / pitch_mm;
        let ramp = pitch_mm / 2.0;
        let t = std::f64::consts::PI / ramp;
        let curvature = depth_mm * (0.5 * k * k + k * t + 0.5 * t * t);
        let chord_budget = SURFACE_TOLERANCE_MM - SURFACE_ROUNDING_MM;
        let ramp_count = (ramp * (curvature / (8.0 * chord_budget)).sqrt())
            .ceil()
            .max(4.0);
        // Away from the two taper ramps, amplitude is constant. Its second
        // derivative bound is four times smaller; do not oversample the whole
        // face using the worst taper curvature.
        let interior_count =
            ((active - 2.0 * ramp) * (0.5 * depth_mm * k * k / (8.0 * chord_budget)).sqrt()).ceil();
        let count = 2.0 * ramp_count + interior_count;
        if !count.is_finite() || count > (MAX_DEPOSITION_SEGMENTS as usize / 4) as f64 {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "surface profile sampling budget exceeded",
            ));
        }
        samples = samples
            .checked_add(count as usize + 9)
            .filter(|v| *v <= sample_limit)
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "combined surface profile sampling budget exceeded",
                )
            })?;
        // Reserve half the frame budget for floating arithmetic, independently
        // of observed sample agreement. Comparing two equally rounded points
        // cannot certify a profile on an excessively large source/pose frame.
        let scale = cert
            .origin_mm
            .into_iter()
            .chain(occurrence.pose.translation_mm)
            .chain([cert.width_mm, cert.min_z_mm, cert.max_z_mm, *depth_mm])
            .fold(1.0f64, |scale, value| scale.max(value.abs()));
        if 4096.0 * f64::EPSILON * scale > SURFACE_ROUNDING_MM / 2.0 {
            return Err(unsupported(
                &face.face_id,
                "source/pose arithmetic cannot fit the reserved profile construction bound",
            ));
        }
        let origin = forward(&occurrence.pose, cert.origin_mm);
        let horizontal = rotate(&occurrence.pose, cert.horizontal_direction);
        let normal = rotate(&occurrence.pose, cert.outward_normal);
        let z0 = forward(
            &occurrence.pose,
            [cert.origin_mm[0], cert.origin_mm[1], cert.min_z_mm],
        )[2];
        let z1 = forward(
            &occurrence.pose,
            [cert.origin_mm[0], cert.origin_mm[1], cert.max_z_mm],
        )[2];
        let mut field = RibField {
            face_id: face.face_id.clone(),
            bounds: AabbMm {
                min: [f64::INFINITY; 3],
                max: [f64::NEG_INFINITY; 3],
            },
            origin,
            horizontal,
            normal,
            width: cert.width_mm,
            depth: *depth_mm,
            pitch: *pitch_mm,
            margin: *edge_margin_mm,
            phase: *phase_mm,
            overlap: width / 4.0,
            samples: count as usize,
            ramp_samples: ramp_count as usize,
            interior_samples: interior_count as usize,
            peak_mm: 0.0,
        };
        for index in 0..=field.samples {
            check_cancel(cancel)?;
            let s = field.sample_parameter(index);
            field.peak_mm = field.peak_mm.max(field.displacement(s));
        }
        for s in [field.margin, field.width - field.margin] {
            for distance in [0.0, field.depth] {
                let p = field.point(s, distance);
                for (a, coordinate) in [p.x, p.y].into_iter().enumerate() {
                    field.bounds.min[a] = field.bounds.min[a].min(coordinate);
                    field.bounds.max[a] = field.bounds.max[a].max(coordinate);
                }
            }
        }
        // Every actual chain/frame-adjusted sample stays within this reserved
        // construction radius of the declared field. Include it in nominal
        // printer/component/other-occurrence exclusion, not physical proof.
        for axis in 0..2 {
            field.bounds.min[axis] -= SURFACE_ROUNDING_MM;
            field.bounds.max[axis] += SURFACE_ROUNDING_MM;
        }
        field.bounds.min[2] = z0.min(z1);
        field.bounds.max[2] = z0.max(z1);
        result.push(field);
    }
    Ok(result)
}

impl RibField {
    pub(super) fn sample_points(&self) -> usize {
        self.samples + 9
    }
    fn sample_parameter(&self, index: usize) -> f64 {
        if index == self.samples {
            return self.width - self.margin;
        }
        let ramp = self.pitch / 2.0;
        if index <= self.ramp_samples {
            self.margin + ramp * index as f64 / self.ramp_samples as f64
        } else if index <= self.ramp_samples + self.interior_samples {
            self.margin
                + ramp
                + (self.width - 2.0 * self.margin - 2.0 * ramp) * (index - self.ramp_samples) as f64
                    / self.interior_samples as f64
        } else {
            self.width - self.margin - ramp
                + ramp * (index - self.ramp_samples - self.interior_samples) as f64
                    / self.ramp_samples as f64
        }
    }
    fn point(&self, s: f64, d: f64) -> Coord<f64> {
        Coord {
            x: self.origin[0] + s * self.horizontal[0] + d * self.normal[0],
            y: self.origin[1] + s * self.horizontal[1] + d * self.normal[1],
        }
    }
    fn displacement(&self, s: f64) -> f64 {
        let edge = (s - self.margin).min(self.width - self.margin - s);
        if edge <= 0.0 {
            return 0.0;
        }
        let taper = if edge >= self.pitch / 2.0 {
            1.0
        } else {
            0.5 * (1.0 - (std::f64::consts::TAU * edge / self.pitch).cos())
        };
        self.depth
            * 0.5
            * (1.0 - (std::f64::consts::TAU * (s - self.phase) / self.pitch).cos())
            * taper
    }
}

/// D = M union A, constructed over an attributed native sampled-face chain.
pub(super) fn realize(
    source: MultiPolygon<f64>,
    fields: &[RibField],
    sample: f64,
    retained: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    splice::realize(source, fields, sample, retained, work, cancel)
}

/// The first ordinary wall must retain a resolvable rib peak after erosion.
/// This bounded offset check does not claim exact swept-bead surface physics.
pub(super) fn certify_first_wall(
    centers: &MultiPolygon<f64>,
    fields: &[RibField],
    sample: f64,
    bead: f64,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    for field in fields {
        if sample <= field.bounds.min[2] || sample >= field.bounds.max[2] {
            continue;
        }
        check_cancel(cancel)?;
        let peak = field.peak_mm;
        let error_bound = SURFACE_TOLERANCE_MM + super::OFFSET_TOLERANCE_MM;
        if peak <= 2.0 * error_bound {
            return Err(unsupported(
                &field.face_id,
                "rib depth is not resolvable through the declared finish and offset approximation budgets",
            ));
        }
        let mut realized_peak = f64::NEG_INFINITY;
        for polygon in &centers.0 {
            for point in &polygon.exterior().0 {
                check_cancel(cancel)?;
                let dx = point.x - field.origin[0];
                let dy = point.y - field.origin[1];
                let s = dx * field.horizontal[0] + dy * field.horizontal[1];
                let distance = dx * field.normal[0] + dy * field.normal[1];
                if s >= field.margin
                    && s <= field.width - field.margin
                    && distance >= -bead
                    && distance <= field.depth + error_bound
                {
                    realized_peak = realized_peak.max(distance);
                }
            }
        }
        if realized_peak < peak - bead / 2.0 - error_bound
            || realized_peak > peak - bead / 2.0 + error_bound
        {
            return Err(unsupported(
                &field.face_id,
                "requested rib peak bounds did not survive the ordinary first-wall offset",
            ));
        }
    }
    Ok(())
}

pub(super) fn expanded(bounds: AabbMm, fields: &[RibField]) -> AabbMm {
    let mut result = bounds;
    for field in fields {
        for a in 0..3 {
            result.min[a] = result.min[a].min(field.bounds.min[a]);
            result.max[a] = result.max[a].max(field.bounds.max[a]);
        }
    }
    result
}

pub(super) fn intersects(a: AabbMm, b: AabbMm) -> bool {
    // Closed XY exclusion prevents same-slab ring contact; Z contact alone is
    // admitted because adjacent source slabs never share an interior sample.
    a.min[2] < b.max[2]
        && b.min[2] < a.max[2]
        && (0..2).all(|axis| a.min[axis] <= b.max[axis] && b.min[axis] <= a.max[axis])
}

/// Fixed component exclusion is conservative metadata, not a swept-tool proof.
pub(super) fn component_bounds(component: &MachineComponent) -> AabbMm {
    let local = match &component.shape {
        MachineComponentShape::Box { bounds_mm } => *bounds_mm,
        MachineComponentShape::Cylinder {
            radius_mm,
            height_mm,
        } => AabbMm {
            min: [-radius_mm, -radius_mm, 0.0],
            max: [*radius_mm, *radius_mm, *height_mm],
        },
    };
    let mut result = AabbMm {
        min: [f64::INFINITY; 3],
        max: [f64::NEG_INFINITY; 3],
    };
    for mask in 0..8 {
        let p = forward(
            &component.pose,
            std::array::from_fn(|a| {
                if mask & (1 << a) == 0 {
                    local.min[a]
                } else {
                    local.max[a]
                }
            }),
        );
        for a in 0..3 {
            result.min[a] = result.min[a].min(p[a]);
            result.max[a] = result.max[a].max(p[a]);
        }
    }
    result
}

pub(super) fn prime_intersects(bounds: AabbMm, job: &SingleExtruderJob, bead: f64) -> bool {
    let padding = bead / 2.0 + 2.0 * super::LAYER_QUANTIZATION_MM;
    let mut enter = 0.0_f64;
    let mut exit = 1.0_f64;
    for axis in 0..2 {
        let lo = bounds.min[axis] - padding;
        let hi = bounds.max[axis] + padding;
        let delta = job.prime_end_mm[axis] - job.prime_start_mm[axis];
        if delta == 0.0 {
            if job.prime_start_mm[axis] < lo || job.prime_start_mm[axis] > hi {
                return false;
            }
        } else {
            let a = (lo - job.prime_start_mm[axis]) / delta;
            let b = (hi - job.prime_start_mm[axis]) / delta;
            enter = enter.max(a.min(b));
            exit = exit.min(a.max(b));
            if enter > exit {
                return false;
            }
        }
    }
    true
}

fn unsupported(face_id: &FaceId, message: &str) -> ManufacturingError {
    error(
        ManufacturingErrorCode::UnsupportedGeometry,
        format!("surface face {}: {message}", face_id.as_str()),
    )
}
