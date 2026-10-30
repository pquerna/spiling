// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Bounded manufacturing construction, not an edited BREP or physical certificate.
use super::{forward, region, rotate};
use crate::{CompilerInput, check_cancel, error};
use geo::{Coord, MultiPolygon};
use spiling_contracts::geometry::{
    AabbMm, AnalyticSurfaceDescription, FaceId, GeometryErrorCode, OccurrenceRecord, SurfaceFamily,
};
use spiling_contracts::manufacturing::program_coordinates::project_xyz6_segment_bead_height_mm;
use spiling_contracts::manufacturing::*;
use spiling_contracts::surface::{SurfaceFinish, SurfaceScope};
use std::{
    collections::BTreeSet,
    sync::{Arc, atomic::AtomicBool},
};

pub(super) const SURFACE_TOLERANCE_MM: f64 = 0.001;
const SURFACE_ROUNDING_MM: f64 = 0.00001;
mod cylindrical;
mod image;
mod profile;
mod proof;
mod resolution;
mod splice;
use image::ImageMapping;
pub(super) use resolution::{ResolutionGeometry, collect_resolution};
#[cfg(test)]
mod tests;
use profile::PeriodicProfile;

/// Requested occupied band together with the original placed-native Z scope.
pub(super) struct OccupiedHeightScope {
    pub(super) requested: [f64; 2],
    pub(super) native: [f64; 2],
}

/// The planner sees only identity, bounds and accounting; carrier data stays here.
pub(super) struct PreparedFinish {
    shape: PreparedShape,
}
enum PreparedShape {
    Planar(RibField),
    Cylindrical(cylindrical::CylinderField),
}
impl PreparedFinish {
    pub(super) fn is_image(&self) -> bool {
        match &self.shape {
            PreparedShape::Planar(field) => field.image.is_some(),
            PreparedShape::Cylindrical(field) => field.image.is_some(),
        }
    }
    pub(super) fn sample_points(&self) -> usize {
        match &self.shape {
            PreparedShape::Planar(field) => field.sample_points(),
            PreparedShape::Cylindrical(field) => field.samples,
        }
    }
    pub(super) fn face_id(&self) -> &FaceId {
        match &self.shape {
            PreparedShape::Planar(field) => &field.face_id,
            PreparedShape::Cylindrical(field) => &field.face_id,
        }
    }
    fn bounds(&self) -> AabbMm {
        match &self.shape {
            PreparedShape::Planar(field) => field.bounds,
            PreparedShape::Cylindrical(field) => field.bounds,
        }
    }
    pub(super) fn certificate_description(&self) -> &'static str {
        match &self.shape {
            PreparedShape::Planar(_) => "rectangular supporting-face",
            PreparedShape::Cylindrical(_) => "full-turn exterior cylindrical-face",
        }
    }
    /// Conservative continuous-target outer occupancy from the certified finish
    /// construction box. A box is not contact, deposited material or inner coverage.
    pub(super) fn include_occupied_envelope(
        &self,
        occupied: &mut Arc<super::ContinuousOccupied>,
        heights: &OccupiedHeightScope,
        schedule: &[super::LayerElevation],
        retained_points: usize,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        let requested = [
            heights.requested[0].max(heights.native[0]),
            heights.requested[1].min(heights.native[1]),
        ];
        if requested[0] > requested[1] {
            return Ok(());
        }
        let Some(image) = self.image() else {
            return self.include_occupied_piece(
                occupied,
                heights,
                None,
                retained_points,
                work,
                cancel,
            );
        };
        let whole = image.occupied_interval_bounds(requested, work, cancel)?;
        if whole.maximum == 0.0 && whole.dz == 0.0 {
            return Ok(());
        }
        if whole.dz == 0.0 || requested[0] == requested[1] {
            return self.include_occupied_piece(
                occupied,
                heights,
                Some(whole.maximum),
                retained_points,
                work,
                cancel,
            );
        }
        // Use exactly the final capsule check's actual XYZ6 upper enclosure.
        // The next representable cut leaves every closed C piece present, but
        // keeps a higher slab's peak off the preceding contact sheet.
        let mut low = 0;
        let mut high = schedule.len();
        while low < high {
            check_cancel(cancel)?;
            super::boolean::charge(work, 1)?;
            let middle = low + (high - low) / 2;
            let layer = &schedule[middle];
            let cut =
                project_xyz6_segment_bead_height_mm(layer.z, layer.z, layer.height)?[1].next_up();
            if cut <= requested[0] {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        let mut start = requested[0];
        for layer in &schedule[low..] {
            check_cancel(cancel)?;
            super::boolean::charge(work, 1)?;
            let cut =
                project_xyz6_segment_bead_height_mm(layer.z, layer.z, layer.height)?[1].next_up();
            let end = cut.min(requested[1]);
            if end > start {
                let interval = image.occupied_interval_bounds([start, end], work, cancel)?;
                if interval.maximum != 0.0 || interval.dz != 0.0 {
                    self.include_occupied_piece(
                        occupied,
                        &OccupiedHeightScope {
                            requested: [start, end],
                            native: heights.native,
                        },
                        Some(interval.maximum),
                        retained_points,
                        work,
                        cancel,
                    )?;
                }
                start = end;
            }
            if start == requested[1] {
                return Ok(());
            }
        }
        let interval = image.occupied_interval_bounds([start, requested[1]], work, cancel)?;
        if interval.maximum != 0.0 || interval.dz != 0.0 {
            self.include_occupied_piece(
                occupied,
                &OccupiedHeightScope {
                    requested: [start, requested[1]],
                    native: heights.native,
                },
                Some(interval.maximum),
                retained_points,
                work,
                cancel,
            )?;
        }
        Ok(())
    }

    fn include_occupied_piece(
        &self,
        occupied: &mut Arc<super::ContinuousOccupied>,
        heights: &OccupiedHeightScope,
        maximum: Option<f64>,
        retained_points: usize,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        let [lower_z, upper_z] = heights.requested;
        let native_height = heights.native;
        check_cancel(cancel)?;
        let mut bounds = self.bounds();
        if upper_z < native_height[0] || lower_z > native_height[1] {
            return Ok(());
        }
        if let (Some(image), Some(maximum)) = (self.image(), maximum)
            && (0.0..1.0).contains(&maximum)
        {
            let depth = (maximum * image.parameters.depth_mm)
                .next_up()
                .min(image.parameters.depth_mm);
            let height = [bounds.min[2], bounds.max[2]];
            bounds = match &self.shape {
                PreparedShape::Planar(field) => field.construction_bounds(depth, height)?,
                PreparedShape::Cylindrical(field) => {
                    cylindrical::construction_bounds(field.source_bounds_xy, depth, height)?
                }
            };
        }
        retained_points
            .checked_add(occupied.point_count)
            .and_then(|n| n.checked_add(5))
            .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "continuous finish occupied-envelope staging budget exceeded",
                )
            })?;
        *work = work
            .checked_add(5)
            .filter(|n| *n <= super::MAX_BOOLEAN_PAIRS)
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "continuous finish envelope work budget exceeded",
                )
            })?;
        let mut envelope = geo::Polygon::new(
            geo::LineString::from(vec![
                (bounds.min[0], bounds.min[1]),
                (bounds.max[0], bounds.min[1]),
                (bounds.max[0], bounds.max[1]),
                (bounds.min[0], bounds.max[1]),
                (bounds.min[0], bounds.min[1]),
            ]),
            vec![],
        );
        // Whole-native eligibility proves exact zero actual world-Z from
        // every local horizontal/normal displacement, using the actual matrix.
        // The captured placed-native outer bracket therefore encloses the
        // finish's Z as well; nominal rounded frame endpoints do not.
        super::region::canonicalize_polygon(&mut envelope);
        Arc::make_mut(occupied).push(
            envelope,
            [lower_z.max(native_height[0]), upper_z.min(native_height[1])],
            None,
        )?;
        Ok(())
    }
}

struct RibField {
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
    image: Option<ImageMapping>,
}

/// Validate even disabled/shadowed targets: saved native identity remains authority.
pub(super) fn validate_targets(
    input: &CompilerInput<'_>,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
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

pub(super) fn admit_extrusion(
    input: &CompilerInput<'_>,
    occurrence: &OccurrenceRecord,
    source_bounds: AabbMm,
    face_id: &FaceId,
    min_z: f64,
    max_z: f64,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let vertical = rotate(&occurrence.pose, [0.0, 0.0, 1.0]);
    if !vertical.iter().all(|v| v.is_finite())
        || vertical[0] != 0.0
        || vertical[1] != 0.0
        || (vertical[2].abs() - 1.0).abs() > 8.0 * f64::EPSILON
    {
        return Err(unsupported(
            face_id,
            "vertical ribs require source local Z to map to positive or negative build Z without tilt",
        ));
    }
    if [
        rotate(&occurrence.pose, [1.0, 0.0, 0.0]),
        rotate(&occurrence.pose, [0.0, 1.0, 0.0]),
    ]
    .iter()
    .any(|axis| axis[2] != 0.0)
    {
        return Err(unsupported(
            face_id,
            "native extrusion finish requires exactly zero actual build-Z from horizontal or normal displacement",
        ));
    }
    if (source_bounds.min[2] - input.intent.printer.build_envelope.min[2]).abs()
        > super::LAYER_QUANTIZATION_MM
    {
        return Err(unsupported(
            face_id,
            "vertical ribs require a bed-anchored native extrusion",
        ));
    }
    for carrier in input.definitions[&occurrence.definition_id].faces() {
        check_cancel(cancel)?;
        let invariant = match (&carrier.geometry.family, &carrier.geometry.analytic) {
            (
                SurfaceFamily::Plane,
                Some(AnalyticSurfaceDescription::Plane { origin_mm, normal }),
            ) => {
                normal[2] == 0.0
                    || (normal[0] == 0.0
                        && normal[1] == 0.0
                        && ((origin_mm[2] - min_z).abs() <= super::LAYER_QUANTIZATION_MM
                            || (origin_mm[2] - max_z).abs() <= super::LAYER_QUANTIZATION_MM))
            }
            (
                SurfaceFamily::Cylinder,
                Some(AnalyticSurfaceDescription::Cylinder { axis_direction, .. }),
            ) => axis_direction[0] == 0.0 && axis_direction[1] == 0.0,
            _ => false,
        };
        if !invariant {
            return Err(unsupported(
                face_id,
                "vertical ribs require an invariant native extrusion with no intermediate shelf or sloped carrier",
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
) -> Result<Vec<PreparedFinish>, ManufacturingError> {
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
        let layer = input.surface_overlays.effective_layer(
            &occurrence.definition_id,
            occurrence.occurrence_id,
            &face.face_id,
        );
        let finish = layer.map(|layer| &layer.finish);
        // Native family is a dispatch restriction, never a finish certificate.
        // An optional analytic description on another family must not broaden
        // the existing exact planar/full-turn-cylinder finish lanes.
        let supported_family = match finish {
            None | Some(SurfaceFinish::Smooth) => true,
            Some(SurfaceFinish::VerticalRibs { .. }) => {
                face.geometry.family == SurfaceFamily::Plane
            }
            Some(SurfaceFinish::CylindricalRibs { .. }) => {
                face.geometry.family == SurfaceFamily::Cylinder
            }
            Some(SurfaceFinish::ImageRelief { .. }) => matches!(
                face.geometry.family,
                SurfaceFamily::Plane | SurfaceFamily::Cylinder
            ),
        };
        if !supported_family {
            return Err(unsupported(
                &face.face_id,
                "effective finish has no realization for this native source family; generic material admission does not authorize a planar or cylindrical finish",
            ));
        }
        if let Some(SurfaceFinish::ImageRelief { .. }) = finish {
            if matches!(
                face.geometry.analytic,
                Some(AnalyticSurfaceDescription::Cylinder { .. })
            ) {
                let field = cylindrical::metadata(
                    input,
                    occurrence,
                    source_bounds,
                    layer.expect("effective layer"),
                    sample_limit.saturating_sub(samples),
                    cancel,
                )?;
                samples += field.samples;
                result.push(PreparedFinish {
                    shape: PreparedShape::Cylindrical(field),
                });
                continue;
            }
            if !matches!(
                face.geometry.analytic,
                Some(AnalyticSurfaceDescription::Plane { .. })
            ) {
                return Err(unsupported(
                    &face.face_id,
                    "image relief requires an exact certified planar or cylindrical supporting face; this source family has no finish realization",
                ));
            }
        }
        if let Some(SurfaceFinish::CylindricalRibs { .. }) = finish {
            let field = cylindrical::metadata(
                input,
                occurrence,
                source_bounds,
                layer.expect("effective layer"),
                sample_limit.saturating_sub(samples),
                cancel,
            )?;
            samples += field.samples;
            result.push(PreparedFinish {
                shape: PreparedShape::Cylindrical(field),
            });
            continue;
        }
        let (depth_mm, pitch_mm, edge_margin_mm, phase_mm, image_parameters) = match finish {
            Some(SurfaceFinish::VerticalRibs {
                depth_mm,
                pitch_mm,
                edge_margin_mm,
                phase_mm,
            }) => (*depth_mm, *pitch_mm, *edge_margin_mm, *phase_mm, None),
            Some(SurfaceFinish::ImageRelief {
                image_hash,
                parameters,
            }) => (
                parameters.depth_mm,
                2.0 * parameters.edge_margin_mm,
                parameters.edge_margin_mm,
                0.0,
                Some((image_hash, parameters)),
            ),
            _ => continue,
        };
        let (depth_mm, pitch_mm, edge_margin_mm, phase_mm) =
            (&depth_mm, &pitch_mm, &edge_margin_mm, &phase_mm);
        let width = input.intent.recipe.bead_width_mm;
        if *edge_margin_mm < 2.0 * width
            || (image_parameters.is_none()
                && (*pitch_mm < 4.0 * width
                    || *depth_mm < 4.0 * SURFACE_TOLERANCE_MM
                    || *depth_mm > 0.5 * width))
        {
            return Err(unsupported(
                &face.face_id,
                "ribs require process-admitted pitch/depth; all planar fields require protected edge margin >= two beads",
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
        admit_extrusion(
            input,
            occurrence,
            source_bounds,
            &face.face_id,
            cert.min_z_mm,
            cert.max_z_mm,
            cancel,
        )?;
        let image = image_parameters
            .map(|(hash, parameters)| {
                ImageMapping::new(
                    input,
                    occurrence,
                    hash,
                    parameters,
                    layer.expect("effective layer").layer_id,
                    cert.min_z_mm,
                    cert.max_z_mm,
                )
            })
            .transpose()?;
        if image.is_some() && cert.width_mm <= 4.0 * edge_margin_mm {
            return Err(unsupported(
                &face.face_id,
                "planar image width must exceed four protected margins",
            ));
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
        let profile =
            PeriodicProfile::new(*depth_mm, k, std::f64::consts::TAU * phase_mm / pitch_mm);
        let ramp = pitch_mm / 2.0;
        let t = std::f64::consts::PI / ramp;
        let curvature = if let Some(image) = &image {
            2.0 * image.global_du() / active * t + 0.5 * image.parameters.depth_mm * t * t
        } else {
            profile.second_derivative_bound()
                + 2.0 * profile.first_derivative_bound() * t
                + 0.5 * profile.maximum() * t * t
        };
        let chord_budget = SURFACE_TOLERANCE_MM - SURFACE_ROUNDING_MM;
        let ramp_count = (ramp * (curvature / (8.0 * chord_budget)).sqrt())
            .ceil()
            .max(4.0);
        // Away from the two taper ramps, amplitude is constant. Its second
        // derivative bound is four times smaller; do not oversample the whole
        // face using the worst taper curvature.
        let interior_count = if image.is_some() {
            1.0
        } else {
            ((active - 2.0 * ramp)
                * (profile.second_derivative_bound() / (8.0 * chord_budget)).sqrt())
            .ceil()
        };
        let count = 2.0 * ramp_count + interior_count;
        let image_events = image.as_ref().map_or(0, |i| {
            i.image.record().width as usize * i.parameters.u_repeats as usize + 2
        });
        if !count.is_finite()
            || count + 3.0 * image_events as f64 > (MAX_DEPOSITION_SEGMENTS as usize / 4) as f64
        {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "surface profile sampling budget exceeded",
            ));
        }
        samples = samples
            .checked_add(count as usize + 3 * image_events + 9)
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
        let sensitivity = image.as_ref().map_or(1.0, |image| {
            1.0 + image.global_du() / active
                + std::f64::consts::PI * image.parameters.depth_mm / (2.0 * edge_margin_mm)
        });
        if 4096.0 * f64::EPSILON * scale * sensitivity
            > if image.is_some() {
                SURFACE_ROUNDING_MM * 3.0 / 8.0
            } else {
                SURFACE_ROUNDING_MM / 2.0
            }
        {
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
            image,
        };
        for index in 0..=field.samples {
            check_cancel(cancel)?;
            let s = field.sample_parameter(index);
            field.peak_mm = field.peak_mm.max(field.displacement(s, (z0 + z1) / 2.0));
        }
        field.bounds = field.construction_bounds(field.depth, [z0.min(z1), z0.max(z1)])?;
        result.push(PreparedFinish {
            shape: PreparedShape::Planar(field),
        });
    }
    Ok(result)
}

impl RibField {
    fn construction_bounds(
        &self,
        depth: f64,
        height: [f64; 2],
    ) -> Result<AabbMm, ManufacturingError> {
        let mut bounds = AabbMm {
            min: [f64::INFINITY, f64::INFINITY, height[0]],
            max: [f64::NEG_INFINITY, f64::NEG_INFINITY, height[1]],
        };
        for s in [self.margin, self.width - self.margin] {
            for distance in [0.0, depth] {
                let point = self.point(s, distance);
                for (axis, coordinate) in [point.x, point.y].into_iter().enumerate() {
                    if !coordinate.is_finite() {
                        return Err(unsupported(
                            &self.face_id,
                            "nonfinite planar construction envelope",
                        ));
                    }
                    bounds.min[axis] = bounds.min[axis].min(coordinate);
                    bounds.max[axis] = bounds.max[axis].max(coordinate);
                }
            }
        }
        // Global metadata and interval envelopes retain the SAME admitted
        // source/frame/phase construction reserve.
        for axis in 0..2 {
            bounds.min[axis] -= SURFACE_ROUNDING_MM;
            bounds.max[axis] += SURFACE_ROUNDING_MM;
        }
        Ok(bounds)
    }
    pub(super) fn sample_points(&self) -> usize {
        self.samples
            + 9
            + self.image.as_ref().map_or(0, |i| {
                3 * (i.image.record().width as usize * i.parameters.u_repeats as usize + 2)
            })
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
    fn image_piece_derivative(&self, s: f64, lo: f64, hi: f64, sample: f64) -> f64 {
        let image = self.image.as_ref().expect("image piece");
        let a = image.value(
            (lo - self.margin) / (self.width - 2.0 * self.margin),
            sample,
        );
        let b = image.value(
            (hi - self.margin) / (self.width - 2.0 * self.margin),
            sample,
        );
        let slope = (b - a) / (hi - lo);
        let h = a + slope * (s - lo);
        let edge = (s - self.margin).min(self.width - self.margin - s);
        if edge <= 0.0 {
            return 0.0;
        }
        if edge >= self.margin {
            return slope;
        }
        let k = std::f64::consts::PI / self.margin;
        let sign = if s < self.width / 2.0 { 1.0 } else { -1.0 };
        slope * (1.0 - (k * edge).cos()) / 2.0 + h * sign * k * (k * edge).sin() / 2.0
    }
    fn displacement(&self, s: f64, sample: f64) -> f64 {
        let edge = (s - self.margin).min(self.width - self.margin - s);
        if edge <= 0.0 {
            return 0.0;
        }
        if let Some(image) = &self.image {
            let taper = if edge >= self.margin {
                1.0
            } else {
                (1.0 - (std::f64::consts::PI * edge / self.margin).cos()) / 2.0
            };
            return image.value((s - self.margin) / (self.width - 2.0 * self.margin), sample)
                * taper;
        }
        let taper = if edge >= self.pitch / 2.0 {
            1.0
        } else {
            PeriodicProfile::new(1.0, std::f64::consts::TAU / self.pitch, 0.0).value(edge)
        };
        PeriodicProfile::new(
            self.depth,
            std::f64::consts::TAU / self.pitch,
            std::f64::consts::TAU * self.phase / self.pitch,
        )
        .value(s)
            * taper
    }
}

#[derive(Clone, Copy)]
enum RealizationPolicy {
    Requested,
    Required,
}
impl RealizationPolicy {
    fn includes(self, field: &PreparedFinish) -> bool {
        matches!(self, Self::Requested) || !field.is_image()
    }
}

/// D = M union A, constructed over an attributed native sampled-face chain.
pub(super) fn realize(
    source: MultiPolygon<f64>,
    fields: &[PreparedFinish],
    sample: f64,
    retained: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    realize_with_policy(
        source,
        fields,
        sample,
        retained,
        work,
        cancel,
        RealizationPolicy::Requested,
    )
}

/// C = native M plus every non-image addition; image displacement alone is advisory.
pub(super) fn realize_required(
    source: MultiPolygon<f64>,
    fields: &[PreparedFinish],
    sample: f64,
    retained: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    realize_with_policy(
        source,
        fields,
        sample,
        retained,
        work,
        cancel,
        RealizationPolicy::Required,
    )
}

fn realize_with_policy(
    source: MultiPolygon<f64>,
    fields: &[PreparedFinish],
    sample: f64,
    retained: usize,
    work: &mut usize,
    cancel: &AtomicBool,
    policy: RealizationPolicy,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    let mut material = splice::realize(source, fields, sample, retained, work, cancel, policy)?;
    for field in fields.iter().filter(|field| policy.includes(field)) {
        if let PreparedShape::Cylindrical(field) = &field.shape {
            material = cylindrical::realize(material, field, sample, retained, work, cancel)?;
        }
    }
    Ok(material)
}

/// Required ribs consume the actual boundary-associated outer bead edges.
/// Nominal widths are software command associations, not measured bead physics.
pub(super) fn certify_first_wall(
    wall: &super::walls::WallPlan,
    material: &MultiPolygon<f64>,
    fields: &[PreparedFinish],
    sample: f64,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    let strict_fields = fields
        .iter()
        .filter(|field| field.image().is_none())
        .count();
    if strict_fields == 0 {
        return Ok(());
    }
    let comparisons = wall
        .paths
        .iter()
        .map(|path| path.points_mm.len())
        .sum::<usize>()
        .checked_mul(strict_fields)
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "first-wall measurement work overflow",
            )
        })?;
    proof::charge(&mut 0, comparisons)?;
    for field in fields {
        if field.image().is_some() {
            continue;
        } // Image feature loss is reported by attributed resolution witnesses.
        let boundary = resolution::owning_boundary(material, field, cancel)?.ok_or_else(|| {
            unsupported(
                field.face_id(),
                "required finish lost actual material boundary correspondence",
            )
        })?;
        let field = match &field.shape {
            PreparedShape::Planar(field) => field,
            PreparedShape::Cylindrical(field) => {
                cylindrical::certify_first_wall(wall, boundary, field, sample, cancel)?;
                continue;
            }
        };
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
        wall.visit_boundary_edges(boundary, |edge| {
            for point in edge.points_mm {
                check_cancel(cancel)?;
                let dx = point[0] - field.origin[0];
                let dy = point[1] - field.origin[1];
                let s = dx * field.horizontal[0] + dy * field.horizontal[1];
                let distance = dx * field.normal[0] + dy * field.normal[1];
                if s >= field.margin
                    && s <= field.width - field.margin
                    && distance >= -edge.width_mm
                    && distance <= field.depth + error_bound
                {
                    realized_peak = realized_peak.max(distance + edge.width_mm * 0.5);
                }
            }
            Ok(())
        })?;
        if realized_peak < peak - error_bound || realized_peak > peak + error_bound {
            return Err(unsupported(
                &field.face_id,
                "requested rib peak bounds did not survive the ordinary first-wall offset",
            ));
        }
    }
    Ok(())
}

/// Enclose final D using native M's captured placed support and XY additions.
/// Whole-native finish admission proves exact zero world-Z displacement, so D
/// has M's vertical support. Field construction endpoints are independently
/// rounded carrier enclosures, not new material heights or contact witnesses.
pub(super) fn expanded(bounds: AabbMm, fields: &[PreparedFinish]) -> AabbMm {
    let mut result = bounds;
    for field in fields {
        let bounds = field.bounds();
        for a in 0..2 {
            result.min[a] = result.min[a].min(bounds.min[a]);
            result.max[a] = result.max[a].max(bounds.max[a]);
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
        for (a, coordinate) in p.into_iter().enumerate() {
            result.min[a] = result.min[a].min(coordinate);
            result.max[a] = result.max[a].max(coordinate);
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
