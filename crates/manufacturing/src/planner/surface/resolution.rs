// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Advisory nominal-bead diagnostics over final D and actual associated walls.
use super::*;
use geo::Polygon;

struct FeatureWitness {
    point: Coord<f64>,
    source_u: f64,
    source_z: f64,
    cell: u32,
    repeat: u32,
    native_edge_parameter: Option<f64>,
}
fn segment_distance(p: Coord<f64>, a: Coord<f64>, b: Coord<f64>) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let length = dx * dx + dy * dy;
    let t = if length == 0.0 {
        0.0
    } else {
        ((p.x - a.x) * dx + (p.y - a.y) * dy) / length
    }
    .clamp(0.0, 1.0);
    (p.x - (a.x + t * dx)).hypot(p.y - (a.y + t * dy))
}
pub(super) fn owning_material<'a>(
    material: &'a MultiPolygon<f64>,
    field: &PreparedFinish,
    cancel: &AtomicBool,
) -> Result<Option<&'a Polygon<f64>>, ManufacturingError> {
    let mut owner = None;
    for polygon in &material.0 {
        check_cancel(cancel)?;
        let matches = match &field.shape {
            PreparedShape::Planar(f) => {
                let endpoints = [f.point(0.0, 0.0), f.point(f.width, 0.0)];
                let mut found = [false; 2];
                for point in &polygon.exterior().0 {
                    check_cancel(cancel)?;
                    for (index, endpoint) in endpoints.iter().enumerate() {
                        found[index] |= (point.x - endpoint.x).hypot(point.y - endpoint.y)
                            <= SURFACE_ROUNDING_MM / 2.0;
                    }
                }
                found[0] && found[1]
            }
            PreparedShape::Cylindrical(f) => f.owns_realized(polygon, cancel)?,
        };
        if matches {
            if owner.is_some() {
                return Ok(None);
            }
            owner = Some(polygon);
        }
    }
    Ok(owner)
}

pub(super) fn owning_boundary(
    material: &MultiPolygon<f64>,
    field: &PreparedFinish,
    cancel: &AtomicBool,
) -> Result<Option<u32>, ManufacturingError> {
    let Some(owner) = owning_material(material, field, cancel)? else {
        return Ok(None);
    };
    let mut boundary = 0u32;
    for polygon in &material.0 {
        if std::ptr::eq(polygon, owner) {
            return Ok(Some(boundary));
        }
        boundary = boundary
            .checked_add(1 + polygon.interiors().len() as u32)
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "finish boundary correspondence overflow",
                )
            })?;
    }
    Ok(None)
}
fn residual(distance: f64, bead: f64, uncertainty: f64) -> (SurfaceFeatureClass, f64, f64) {
    let low = (distance - bead / 2.0 - uncertainty).max(0.0);
    let high = (distance - bead / 2.0 + uncertainty).max(0.0);
    (
        if high == 0.0 {
            SurfaceFeatureClass::Resolved
        } else if low > 0.0 {
            SurfaceFeatureClass::BelowNominalResolution
        } else {
            SurfaceFeatureClass::Ambiguous
        },
        low,
        high,
    )
}
fn emit(
    record: &mut SurfaceResolution,
    w: FeatureWitness,
    wall: Option<(&super::super::walls::WallPlan, u32)>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    check_cancel(cancel)?;
    let (classification, lower, upper) =
        if let Some((wall, boundary)) = wall.filter(|_| w.native_edge_parameter.is_some()) {
            let cost = wall.correspondence.len();
            if work
                .checked_add(cost)
                .is_none_or(|n| n > super::super::MAX_BOOLEAN_PAIRS)
            {
                record.coverage_complete = false;
                return Ok(false);
            }
            proof::charge(work, cost)?;
            let mut distance = f64::INFINITY;
            wall.visit_boundary_edges(boundary, |edge| {
                check_cancel(cancel)?;
                let a = Coord {
                    x: edge.points_mm[0][0],
                    y: edge.points_mm[0][1],
                };
                let b = Coord {
                    x: edge.points_mm[1][0],
                    y: edge.points_mm[1][1],
                };
                distance = distance.min(segment_distance(w.point, a, b) - edge.width_mm * 0.5);
                Ok(())
            })?;
            let scale = w.point.x.abs().max(w.point.y.abs()).max(distance).max(1.0);
            let uncertainty = SURFACE_TOLERANCE_MM
                + super::super::OFFSET_TOLERANCE_MM
                + 4096.0 * f64::EPSILON * scale;
            if !distance.is_finite() {
                (SurfaceFeatureClass::Unavailable, 0.0, 0.0)
            } else {
                residual(distance, 0.0, uncertainty)
            }
        } else {
            (SurfaceFeatureClass::Unavailable, 0.0, 0.0)
        };
    record.evaluated_witnesses += 1;
    match classification {
        SurfaceFeatureClass::Resolved => record.resolved += 1,
        SurfaceFeatureClass::Ambiguous => record.ambiguous += 1,
        SurfaceFeatureClass::BelowNominalResolution => record.below_nominal_resolution += 1,
        SurfaceFeatureClass::Unavailable => {
            record.unavailable += 1;
            record.coverage_complete = false;
        }
    }
    record.max_residual_lower_mm = record.max_residual_lower_mm.max(lower);
    record.max_residual_upper_mm = record.max_residual_upper_mm.max(upper);
    if record.representatives.len() < 8 {
        record.representatives.push(SurfaceFeatureResolution {
            source_u: w.source_u,
            source_z_mm: w.source_z,
            cell: w.cell,
            repeat: w.repeat,
            classification,
            residual_lower_mm: lower,
            residual_upper_mm: upper,
        });
    } else {
        record.representatives_truncated = true;
    }
    Ok(true)
}
pub(in crate::planner) struct ResolutionGeometry<'a> {
    pub wall: &'a super::super::walls::WallPlan,
    pub material: &'a MultiPolygon<f64>,
    pub native: &'a MultiPolygon<f64>,
}

pub(in crate::planner) fn collect_resolution(
    geometry: ResolutionGeometry<'_>,
    fields: &[PreparedFinish],
    sample: f64,
    layer_index: u32,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Vec<SurfaceResolution>, ManufacturingError> {
    let ResolutionGeometry {
        wall: wall_plan,
        material,
        native,
    } = geometry;
    let mut records = Vec::new();
    for field in fields {
        let Some(image) = field.image() else { continue };
        check_cancel(cancel)?;
        let source_z = image.source_z(sample);
        let mut record = SurfaceResolution {
            surface_layer_id: image.layer_id,
            occurrence_id: image.occurrence_id,
            face_id: field.face_id().clone(),
            image_hash: image.image.record().image_hash.clone(),
            layer_index,
            source_z_mm: source_z,
            evaluated_witnesses: 0,
            resolved: 0,
            ambiguous: 0,
            below_nominal_resolution: 0,
            unavailable: 0,
            coverage_complete: true,
            representatives_truncated: false,
            max_residual_lower_mm: 0.0,
            max_residual_upper_mm: 0.0,
            representatives: Vec::with_capacity(8),
        };
        if image.zero_row(sample) {
            records.push(record);
            continue;
        }
        let scan = region::point_count(material)
            .checked_add(1)
            .and_then(|n| {
                n.checked_mul(
                    wall_plan
                        .correspondence
                        .len()
                        .saturating_add(region::point_count(native))
                        .saturating_add(1),
                )
            })
            .and_then(|n| n.checked_mul(4));
        if scan
            .and_then(|n| work.checked_add(n))
            .is_none_or(|n| n > super::super::MAX_BOOLEAN_PAIRS)
        {
            record.coverage_complete = false;
            records.push(record);
            continue;
        }
        proof::charge(work, scan.expect("admitted diagnostic scan"))?;
        let owner = owning_material(material, field, cancel)?;
        let native_owner = owning_material(native, field, cancel)?;
        let wall = owning_boundary(material, field, cancel)?.map(|boundary| (wall_plan, boundary));
        if let Some(owner) = owner {
            'edges: for edge in owner.exterior().0.windows(2) {
                check_cancel(cancel)?;
                // Production D already contains every field/native/taper break,
                // strict piece extrema to chord allowance and plateau nodes.
                // Interior chord midpoint has <=0.001mm lift uncertainty.
                for fraction in [0.0, 0.5] {
                    let point = Coord {
                        x: edge[0].x + (edge[1].x - edge[0].x) * fraction,
                        y: edge[0].y + (edge[1].y - edge[0].y) * fraction,
                    };
                    let source_u = match &field.shape {
                        PreparedShape::Planar(f) => {
                            let s = (point.x - f.origin[0]) * f.horizontal[0]
                                + (point.y - f.origin[1]) * f.horizontal[1];
                            let d = (point.x - f.origin[0]) * f.normal[0]
                                + (point.y - f.origin[1]) * f.normal[1];
                            if s < f.margin
                                || s > f.width - f.margin
                                || d < -SURFACE_ROUNDING_MM
                                || d > f.depth + SURFACE_ROUNDING_MM
                            {
                                continue;
                            }
                            (s - f.margin) / (f.width - 2.0 * f.margin)
                        }
                        PreparedShape::Cylindrical(f) => f.source_u(point),
                    };
                    let (cell, repeat) = image.field()?.cell(source_u);
                    let lifted = if let Some(native_owner) = native_owner {
                        match &field.shape {
                            PreparedShape::Planar(f) => {
                                splice::witness(f, native_owner.exterior(), point, sample, cancel)?
                            }
                            PreparedShape::Cylindrical(f) => cylindrical::witness(
                                f,
                                native_owner.exterior(),
                                point,
                                sample,
                                cancel,
                            )?,
                        }
                    } else {
                        None
                    };
                    let (point, native_edge_parameter) =
                        lifted.map_or((point, None), |(point, parameter)| (point, Some(parameter)));
                    if !emit(
                        &mut record,
                        FeatureWitness {
                            point,
                            source_u: source_u.rem_euclid(1.0),
                            source_z,
                            cell,
                            repeat,
                            native_edge_parameter,
                        },
                        wall,
                        work,
                        cancel,
                    )? {
                        break 'edges;
                    }
                }
            }
        } else {
            let (cell, repeat) = image.field()?.cell(0.0);
            emit(
                &mut record,
                FeatureWitness {
                    point: Coord { x: 0.0, y: 0.0 },
                    source_u: 0.0,
                    source_z,
                    cell,
                    repeat,
                    native_edge_parameter: None,
                },
                None,
                work,
                cancel,
            )?;
        }
        records.push(record);
    }
    Ok(records)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_residual_interval_distinguishes_uncertainty_from_lost_feature() {
        let (class, low, high) = residual(0.205, 0.4, 0.006);
        assert!(matches!(class, SurfaceFeatureClass::Ambiguous));
        assert_eq!(low, 0.0);
        assert!(high > 0.01);
        let (class, low, high) = residual(0.22, 0.4, 0.006);
        assert!(matches!(class, SurfaceFeatureClass::BelowNominalResolution));
        assert!(low > 0.013 && high > low);
        assert_eq!(
            segment_distance(
                Coord { x: 1.0, y: 1.0 },
                Coord { x: 0.0, y: 0.0 },
                Coord { x: 2.0, y: 0.0 }
            ),
            1.0
        );
    }
}
