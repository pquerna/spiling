// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

mod ast;
pub(crate) mod brep_data_bridge;
#[cfg(test)]
mod tests;
mod units;

mod bounds;
use crate::{
    GeometryWorkBudget, check_cancel,
    kernel::{
        BoundaryCoedgeRef, BoundaryCurveMm, Definition, FaceBoundaryRefs, FaceParameterWitness,
    },
};
use ast::*;
use bounds::import_bounds;
use monstertruck_io::step::load::{
    Table,
    step_geometry::{
        Conic2D, Conic3D, Curve2D, Curve3D, CurveParameterDomain, CurveParameterMap,
        CurveParameterMapKind, ElementarySurface, StepParameterCurve, Surface as StepSurface,
        SurfaceCurveAssociatedGeometry, SurfaceParameterMap,
    },
};
use monstertruck_modeling::{Curve, base::*, builder};
use sha2::{Digest, Sha256};
use spiling_contracts::geometry::{
    AnalyticSurfaceDescription, DefinitionId, FaceDomainSummary, FaceGeometryInfo, FaceId,
    FaceInfo, GeometryError, GeometryErrorCode, MAX_NATIVE_FACES, MAX_SOURCE_BYTES, PlaneFrameMm,
    SourceHash, SourceProvenance, StepEntityId, SurfaceFamily, TorusKind,
};
use std::{
    collections::{BTreeSet, HashMap},
    hash::Hash,
    sync::atomic::AtomicBool,
};
use step_p21::ast::Parameter;
pub(crate) use crate::kernel::{OriginalCoedgeCorrespondence, OriginalCorrespondenceProvenance};

/// Display-only native coefficient equality stub.
pub(crate) fn certify_display_native_chart(
    _curve: &Curve,
    _surface: &monstertruck_modeling::Surface,
    _chart: &monstertruck_modeling::Curve2D,
    _domain: [[f64; 2]; 2],
    _work: &mut GeometryWorkBudget,
    _cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    Ok(true)
}

pub(crate) const MAX_QUERY_PROOF_BYTES: usize = 32 * 1024 * 1024;
const MAX_IMPORT_CHART_WORK: u64 = 128_000_000;

fn query_index_storage_bound(capacity: usize) -> Option<usize> {
    if capacity == 0 {
        return Some(0);
    }
    // Same conservative 7/8-load, control-byte and alignment convention as
    // tessellation capacity accounting. This is not allocator/RSS accounting.
    capacity
        .checked_add(capacity / 7)?
        .checked_add(16)?
        .checked_mul(
            std::mem::size_of::<(monstertruck_topology::EdgeId<Curve>, usize)>().checked_add(1)?,
        )?
        .checked_add(64)
}

fn error(code: GeometryErrorCode, message: &str) -> GeometryError {
    GeometryError::new(code, message)
}
fn invalid(message: &str) -> GeometryError {
    error(GeometryErrorCode::InvalidGeometry, message)
}
fn unsupported(message: &str) -> GeometryError {
    error(GeometryErrorCode::UnsupportedGeometry, message)
}
fn kernel(message: &str) -> GeometryError {
    error(GeometryErrorCode::KernelFailure, message)
}

fn admission_error(
    cause: GeometryError,
    stage: &str,
    work: &GeometryWorkBudget,
    source: impl std::fmt::Display,
) -> GeometryError {
    error(
        cause.code,
        &format!(
            "native STEP admission {stage} ({source}; cumulative work consumed={} remaining={}): {}",
            work.consumed(),
            work.remaining(),
            cause.message,
        ),
    )
}

/// Native source refusals retain their meaning: a valid but unavailable chart
/// or exhausted expansion policy is not evidence of malformed STEP geometry.
fn conversion_error(context: &str, cause: &(dyn std::error::Error + 'static)) -> GeometryError {
    use monstertruck_io::step::load::step_geometry::ExactConversionError as Exact;
    let mut current = Some(cause);
    while let Some(node) = current {
        if let Some(exact) = node.downcast_ref::<Exact>() {
            let code = match exact {
                Exact::SourceGeometryNestingExceeded
                | Exact::SourceGeometryExpansionExceeded { .. } => GeometryErrorCode::ResourceLimit,
                Exact::CyclicSourceGeometry { .. }
                | Exact::InvalidTrimParameterSelectors
                | Exact::LineEdgeVertexOffCarrier
                | Exact::LineEdgeSense => GeometryErrorCode::InvalidGeometry,
                Exact::NonlinearParameterCurveChart
                | Exact::UnestablishedCurveCorrespondence
                | Exact::MissingSeamParameterBranch
                | Exact::AmbiguousStoredTrim => {
                    GeometryErrorCode::ParameterCorrespondenceUnavailable
                }
                Exact::CurveDomain { .. }
                | Exact::CartesianTrimSelectors
                | Exact::UnsupportedPeriodicTrimSelection
                | Exact::UnrepresentableTrimParameterChart => {
                    GeometryErrorCode::UnsupportedConversion
                }
            };
            return error(code, &format!("{context}: {cause}"));
        }
        current = node.source();
    }
    error(
        GeometryErrorCode::UnsupportedConversion,
        &format!("{context}: {cause}"),
    )
}
fn array(p: Point3) -> [f64; 3] {
    [p.x, p.y, p.z]
}
fn vec_array(p: Vector3) -> [f64; 3] {
    [p.x, p.y, p.z]
}
fn unit_vector(v: [f64; 3]) -> Result<Vector3, GeometryError> {
    let v = Vector3::new(v[0], v[1], v[2]);
    if !v.magnitude2().is_finite() || v.magnitude2() == 0.0 {
        return Err(invalid("invalid native axis"));
    }
    Ok(v.normalize())
}
fn placement(
    entities: &Entities<'_>,
    id: u64,
    scale: f64,
) -> Result<(Point3, Vector3), GeometryError> {
    let r = entities.record(id, "AXIS2_PLACEMENT_3D")?;
    let point = entities.record(reference(at(r, 1)?)?, "CARTESIAN_POINT")?;
    let [x, y, z] = vector(at(point, 1)?)?;
    let origin = Point3::new(x * scale, y * scale, z * scale);
    let z = if matches!(at(r, 2)?, Parameter::NotProvided) {
        Vector3::unit_z()
    } else {
        unit_vector(vector(at(
            entities.record(reference(at(r, 2)?)?, "DIRECTION")?,
            1,
        )?)?)?
    };
    if !origin.x.is_finite() || !origin.y.is_finite() || !origin.z.is_finite() {
        return Err(invalid("nonfinite normalized carrier"));
    }
    Ok((origin, z))
}
fn placement_frame(
    entities: &Entities<'_>,
    id: u64,
    scale: f64,
) -> Result<PlaneFrameMm, GeometryError> {
    let (origin, z) = placement(entities, id, scale)?;
    let r = entities.record(id, "AXIS2_PLACEMENT_3D")?;
    let x = if matches!(at(r, 3)?, Parameter::NotProvided) {
        if z.x.abs() < 0.9 {
            Vector3::unit_x()
        } else {
            Vector3::unit_y()
        }
    } else {
        unit_vector(vector(at(
            entities.record(reference(at(r, 3)?)?, "DIRECTION")?,
            1,
        )?)?)?
    };
    let x = x - x.dot(z) * z;
    if x.magnitude2() <= 1e-20 {
        return Err(invalid(
            "source placement has parallel axis and reference direction",
        ));
    }
    let x = x.normalize();
    let frame = PlaneFrameMm {
        origin_mm: array(origin),
        x_axis: vec_array(x),
        y_axis: vec_array(z.cross(x)),
        z_axis: vec_array(z),
    };
    frame.validate()?;
    Ok(frame)
}

fn source_geometry(
    entities: &Entities<'_>,
    face: u64,
    scale: f64,
) -> Result<(String, String, FaceGeometryInfo), GeometryError> {
    let r = entities
        .records(face)?
        .iter()
        .find(|r| matches!(r.name.as_str(), "ADVANCED_FACE" | "FACE_SURFACE"))
        .copied()
        .ok_or_else(|| invalid("shell face does not resolve to surface face"))?;
    let surface = reference(at(r, 2)?)?;
    let records = entities.records(surface)?;
    let analytic_record = records.iter().find(|r| {
        matches!(
            r.name.as_str(),
            "PLANE"
                | "CYLINDRICAL_SURFACE"
                | "CONICAL_SURFACE"
                | "SPHERICAL_SURFACE"
                | "TOROIDAL_SURFACE"
        )
    });
    let (kind, family, analytic) = if let Some(surface) = analytic_record {
        let frame = placement_frame(entities, reference(at(surface, 1)?)?, scale)?;
        let analytic = match surface.name.as_str() {
            "PLANE" => AnalyticSurfaceDescription::Plane {
                origin_mm: frame.origin_mm,
                normal: frame.z_axis,
            },
            "CYLINDRICAL_SURFACE" => AnalyticSurfaceDescription::Cylinder {
                axis_origin_mm: frame.origin_mm,
                axis_direction: frame.z_axis,
                radius_mm: number(at(surface, 2)?)? * scale,
            },
            "CONICAL_SURFACE" => AnalyticSurfaceDescription::Cone {
                frame,
                reference_radius_mm: number(at(surface, 2)?)? * scale,
                semi_angle_radians: number(at(surface, 3)?)?,
            },
            "SPHERICAL_SURFACE" => AnalyticSurfaceDescription::Sphere {
                frame,
                radius_mm: number(at(surface, 2)?)? * scale,
            },
            "TOROIDAL_SURFACE" => {
                let major = number(at(surface, 2)?)? * scale;
                let minor = number(at(surface, 3)?)? * scale;
                if major <= minor {
                    return Err(error(
                        GeometryErrorCode::SingularGeometry,
                        "horn/spindle torus is outside the regular bounded import envelope",
                    ));
                }
                AnalyticSurfaceDescription::Torus {
                    frame,
                    major_radius_mm: major,
                    minor_radius_mm: minor,
                    torus_kind: TorusKind::RegularRing,
                }
            }
            _ => unreachable!(),
        };
        analytic.validate()?;
        (surface.name.clone(), analytic.family(), Some(analytic))
    } else {
        let (kind, family) = if records
            .iter()
            .any(|r| r.name == "RATIONAL_B_SPLINE_SURFACE")
        {
            ("RATIONAL_B_SPLINE_SURFACE", SurfaceFamily::Nurbs)
        } else if let Some(spline) = records.iter().find(|r| {
            matches!(
                r.name.as_str(),
                "B_SPLINE_SURFACE"
                    | "B_SPLINE_SURFACE_WITH_KNOTS"
                    | "BEZIER_SURFACE"
                    | "QUASI_UNIFORM_SURFACE"
                    | "UNIFORM_SURFACE"
            )
        }) {
            (spline.name.as_str(), SurfaceFamily::BSpline)
        } else if records
            .iter()
            .any(|r| r.name == "SURFACE_OF_LINEAR_EXTRUSION")
        {
            (
                "SURFACE_OF_LINEAR_EXTRUSION",
                SurfaceFamily::LinearExtrusion,
            )
        } else if records.iter().any(|r| r.name == "SURFACE_OF_REVOLUTION") {
            ("SURFACE_OF_REVOLUTION", SurfaceFamily::Revolution)
        } else {
            return Err(unsupported(&format!(
                "source surface #{surface} has no exact admitted conversion"
            )));
        };
        (kind.to_owned(), family, None)
    };
    let bounds = list(at(r, 1)?)?;
    if bounds.is_empty() {
        return Err(invalid("bounded native face requires explicit trim loops"));
    }
    let outer = bounds
        .iter()
        .filter_map(|p| reference(p).ok())
        .filter(|id| {
            entities
                .records(*id)
                .is_ok_and(|rs| rs.iter().any(|r| r.name == "FACE_OUTER_BOUND"))
        })
        .count();
    if outer > 1 {
        return Err(invalid("face has multiple declared outer trim loops"));
    }
    let domain = FaceDomainSummary::new(
        bounds.len() as u32,
        bounds.len().saturating_sub(1) as u32,
        false,
        false,
        false,
    )?;
    Ok((
        r.name.clone(),
        kind,
        FaceGeometryInfo::new(family, analytic, domain)?,
    ))
}
fn resolve_face(entities: &Entities<'_>, canonical: u64) -> Result<u64, GeometryError> {
    let mut id = canonical;
    let mut seen = BTreeSet::new();
    loop {
        if !seen.insert(id) {
            return Err(invalid("recursive ORIENTED_FACE"));
        }
        let rs = entities.records(id)?;
        if rs
            .iter()
            .any(|r| matches!(r.name.as_str(), "ADVANCED_FACE" | "FACE_SURFACE"))
        {
            return Ok(id);
        }
        let r = entities.record(id, "ORIENTED_FACE")?;
        id = reference(at(r, 2)?)?;
    }
}
fn circular_witness(
    center: Point3,
    u: Vector3,
    v: Vector3,
    a: f64,
    b: f64,
    scale: f64,
) -> Result<BoundaryCurveMm, GeometryError> {
    let radius = u.magnitude();
    if !radius.is_finite()
        || radius <= 0.0
        || (v.magnitude() - radius).abs() > 1e-9 * radius
        || u.dot(v).abs() > 1e-9 * radius * radius
    {
        return Err(unsupported("elliptical boundary is not a circle"));
    }
    if !a.is_finite() || !b.is_finite() || (b - a).abs() > std::f64::consts::TAU + 1e-9 {
        return Err(invalid("invalid bounded circular arc"));
    }
    let center_mm = array(Point3::from_vec(center.to_vec() * scale));
    let u_mm = vec_array(u * scale);
    let v_mm = vec_array(v * scale);
    if !center_mm
        .iter()
        .chain(&u_mm)
        .chain(&v_mm)
        .all(|v| v.is_finite())
    {
        return Err(invalid("nonfinite normalized circle"));
    }
    Ok(BoundaryCurveMm::CircleArc {
        center_mm,
        u_mm,
        v_mm,
        start_angle: a,
        end_angle: b,
    })
}
fn pcurve_boundary(c: &StepParameterCurve, scale: f64) -> Result<BoundaryCurveMm, GeometryError> {
    // This optional descriptor serves only the existing narrow finish lanes.
    // Stored source UV is not decoded carrier UV after a raw STEP rechart;
    // never classify its untransformed syntax as a native line/circle.
    if c.input_parameter_chart() != monstertruck_io::step::SurfaceParameterChart::identity() {
        return Err(unsupported(
            "nonidentity stored PCURVE chart has no coefficient-exact narrow finish descriptor",
        ));
    }
    let surface = c.surface();
    match (surface, c.curve()) {
        (StepSurface::ElementarySurface(ElementarySurface::Plane(_)), Curve2D::Line(_)) => {
            let (a, b) = c.range_tuple();
            Ok(BoundaryCurveMm::Line {
                start_mm: array(Point3::from_vec(c.subs(a).to_vec() * scale)),
                end_mm: array(Point3::from_vec(c.subs(b).to_vec() * scale)),
            })
        }
        (
            StepSurface::ElementarySurface(ElementarySurface::Plane(plane)),
            Curve2D::Conic(Conic2D::Ellipse(ellipse)),
        ) => {
            let matrix = ellipse.transform();
            let uv = matrix.transform_point(Point2::origin());
            let u2 = <Matrix3 as Transform<Point2>>::transform_vector(matrix, Vector2::unit_x());
            let v2 = <Matrix3 as Transform<Point2>>::transform_vector(matrix, Vector2::unit_y());
            let center = plane.subs(uv.x, uv.y);
            let u = plane.subs(uv.x + u2.x, uv.y + u2.y) - center;
            let v = plane.subs(uv.x + v2.x, uv.y + v2.y) - center;
            let (a, b) = ellipse.range_tuple();
            let (a, b) = if ellipse.orientation() {
                (a, b)
            } else {
                (b, a)
            };
            circular_witness(center, u, v, a, b, scale)
        }
        (
            StepSurface::ElementarySurface(ElementarySurface::CylindricalSurface(cylinder)),
            Curve2D::Line(line),
        ) => {
            // Processor inversion swaps the surface's axial/angular parameters.
            let (axial0, angle0, axial1, angle1) = if cylinder.orientation() {
                (line.0.x, line.0.y, line.1.x, line.1.y)
            } else {
                (line.0.y, line.0.x, line.1.y, line.1.x)
            };
            if angle0 == angle1 {
                let (a, b) = c.range_tuple();
                return Ok(BoundaryCurveMm::Line {
                    start_mm: array(Point3::from_vec(c.subs(a).to_vec() * scale)),
                    end_mm: array(Point3::from_vec(c.subs(b).to_vec() * scale)),
                });
            }
            if axial0 != axial1 {
                return Err(unsupported("helical pcurve is outside line/circle profile"));
            }
            let revolution = cylinder.entity();
            let axis = revolution.axis();
            let profile = revolution.entity_curve().subs(axial0);
            let center = revolution.origin() + (profile - revolution.origin()).dot(axis) * axis;
            let u = profile - center;
            let v = axis.cross(u);
            let matrix = cylinder.transform();
            circular_witness(
                matrix.transform_point(center),
                matrix.transform_vector(u),
                matrix.transform_vector(v),
                angle0,
                angle1,
                scale,
            )
        }
        _ => Err(unsupported(
            "pcurve does not define an exact admitted line/circle",
        )),
    }
}
fn transformed_frame(
    matrix: &Matrix4,
    origin: Point3,
    scale: f64,
) -> Result<PlaneFrameMm, GeometryError> {
    let frame = PlaneFrameMm {
        origin_mm: array(Point3::from_vec(
            matrix.transform_point(origin).to_vec() * scale,
        )),
        x_axis: vec_array(unit_vector(vec_array(
            matrix.transform_vector(Vector3::unit_x()),
        ))?),
        y_axis: vec_array(unit_vector(vec_array(
            matrix.transform_vector(Vector3::unit_y()),
        ))?),
        z_axis: vec_array(unit_vector(vec_array(
            matrix.transform_vector(Vector3::unit_z()),
        ))?),
    };
    frame.validate()?;
    Ok(frame)
}

fn native_carrier(
    surface: &StepSurface,
    scale: f64,
) -> Result<AnalyticSurfaceDescription, GeometryError> {
    let carrier = match surface {
        StepSurface::ElementarySurface(ElementarySurface::Plane(plane)) => {
            AnalyticSurfaceDescription::Plane {
                origin_mm: array(Point3::from_vec(plane.origin().to_vec() * scale)),
                normal: vec_array(plane.normal()),
            }
        }
        StepSurface::ElementarySurface(ElementarySurface::CylindricalSurface(cylinder)) => {
            let revolution = cylinder.entity();
            let matrix = cylinder.transform();
            let origin = matrix.transform_point(revolution.origin());
            let axis = unit_vector(vec_array(matrix.transform_vector(revolution.axis())))?;
            let point = matrix.transform_point(revolution.entity_curve().0);
            let delta = point - origin;
            AnalyticSurfaceDescription::Cylinder {
                axis_origin_mm: array(Point3::from_vec(origin.to_vec() * scale)),
                axis_direction: vec_array(axis),
                radius_mm: (delta - delta.dot(axis) * axis).magnitude() * scale,
            }
        }
        StepSurface::ElementarySurface(ElementarySurface::ConicalSurface(cone)) => {
            let revolution = cone.entity();
            let line = revolution.entity_curve();
            let axis = revolution.axis();
            let d0 = line.0 - revolution.origin();
            let d1 = line.1 - revolution.origin();
            let z0 = d0.dot(axis);
            let z1 = d1.dot(axis);
            let r0 = (d0 - z0 * axis).magnitude();
            let r1 = (d1 - z1 * axis).magnitude();
            let origin = revolution.origin() + z0 * axis;
            AnalyticSurfaceDescription::Cone {
                frame: transformed_frame(cone.transform(), origin, scale)?,
                reference_radius_mm: r0 * scale,
                semi_angle_radians: ((r1 - r0) / (z1 - z0)).atan(),
            }
        }
        StepSurface::ElementarySurface(ElementarySurface::Sphere(sphere)) => {
            AnalyticSurfaceDescription::Sphere {
                frame: transformed_frame(sphere.transform(), sphere.entity().0.center(), scale)?,
                radius_mm: sphere.entity().0.radius() * scale,
            }
        }
        StepSurface::ElementarySurface(ElementarySurface::ToroidalSurface(torus)) => {
            let major = torus.entity().large_radius() * scale;
            let minor = torus.entity().small_radius() * scale;
            AnalyticSurfaceDescription::Torus {
                frame: transformed_frame(torus.transform(), torus.entity().center(), scale)?,
                major_radius_mm: major,
                minor_radius_mm: minor,
                torus_kind: if major > minor {
                    TorusKind::RegularRing
                } else if major == minor {
                    TorusKind::Horn
                } else {
                    TorusKind::Spindle
                },
            }
        }
        _ => {
            return Err(unsupported(
                "analytic boundary membership unavailable for nonanalytic carrier",
            ));
        }
    };
    carrier.validate()?;
    Ok(carrier)
}

/// An admitted analytic leader must lie on both native intersection carriers.
/// Checking analytic coefficients certifies the whole bounded arc, not samples.
fn certify_boundary(
    witness: &BoundaryCurveMm,
    carrier: &AnalyticSurfaceDescription,
) -> Result<(), GeometryError> {
    let vector = |p: [f64; 3]| Vector3::new(p[0], p[1], p[2]);
    let tolerance = 1e-7;
    let valid = match carrier {
        AnalyticSurfaceDescription::Plane { origin_mm, normal } => {
            let origin = vector(*origin_mm);
            let normal = vector(*normal);
            match witness {
                BoundaryCurveMm::Line { start_mm, end_mm } => {
                    (vector(*start_mm) - origin).dot(normal).abs() <= tolerance
                        && (vector(*end_mm) - origin).dot(normal).abs() <= tolerance
                }
                BoundaryCurveMm::CircleArc {
                    center_mm,
                    u_mm,
                    v_mm,
                    ..
                } => {
                    (vector(*center_mm) - origin).dot(normal).abs() <= tolerance
                        && vector(*u_mm).dot(normal).abs() <= tolerance
                        && vector(*v_mm).dot(normal).abs() <= tolerance
                }
                BoundaryCurveMm::Native { .. } => {
                    return Err(unsupported("native curve has no analytic membership proof"));
                }
            }
        }
        AnalyticSurfaceDescription::Cylinder {
            axis_origin_mm,
            axis_direction,
            radius_mm,
        } => {
            let origin = vector(*axis_origin_mm);
            let axis = vector(*axis_direction);
            let radial = |point: [f64; 3]| {
                let delta = vector(point) - origin;
                delta - delta.dot(axis) * axis
            };
            match witness {
                BoundaryCurveMm::Line { start_mm, end_mm } => {
                    (radial(*start_mm).magnitude() - radius_mm).abs() <= tolerance
                        && (radial(*end_mm) - radial(*start_mm)).magnitude() <= tolerance
                }
                BoundaryCurveMm::CircleArc {
                    center_mm,
                    u_mm,
                    v_mm,
                    ..
                } => {
                    radial(*center_mm).magnitude() <= tolerance
                        && vector(*u_mm).dot(axis).abs() <= tolerance
                        && vector(*v_mm).dot(axis).abs() <= tolerance
                        && (vector(*u_mm).magnitude() - radius_mm).abs() <= tolerance
                        && (vector(*v_mm).magnitude() - radius_mm).abs() <= tolerance
                }
                BoundaryCurveMm::Native { .. } => {
                    return Err(unsupported("native curve has no analytic membership proof"));
                }
            }
        }
        AnalyticSurfaceDescription::Cone {
            frame,
            reference_radius_mm,
            semi_angle_radians,
        } => {
            let origin = vector(frame.origin_mm);
            let axis = vector(frame.z_axis);
            let slope = semi_angle_radians.tan();
            let radial = |p: [f64; 3]| {
                let d = vector(p) - origin;
                (
                    d - d.dot(axis) * axis,
                    reference_radius_mm + d.dot(axis) * slope,
                )
            };
            match witness {
                BoundaryCurveMm::Line { start_mm, end_mm } => {
                    let (a, ra) = radial(*start_mm);
                    let (b, rb) = radial(*end_mm);
                    ra > 0.0
                        && rb > 0.0
                        && (a.magnitude() - ra).abs() <= tolerance
                        && (b.magnitude() - rb).abs() <= tolerance
                        && (a.cross(b)).magnitude() <= tolerance * ra.max(rb)
                        && a.dot(b) >= 0.0
                }
                BoundaryCurveMm::CircleArc {
                    center_mm,
                    u_mm,
                    v_mm,
                    ..
                } => {
                    let (center, radius) = radial(*center_mm);
                    radius > 0.0
                        && center.magnitude() <= tolerance
                        && vector(*u_mm).dot(axis).abs() <= tolerance
                        && vector(*v_mm).dot(axis).abs() <= tolerance
                        && (vector(*u_mm).magnitude() - radius).abs() <= tolerance
                        && (vector(*v_mm).magnitude() - radius).abs() <= tolerance
                }
                BoundaryCurveMm::Native { .. } => {
                    return Err(unsupported("native curve has no analytic membership proof"));
                }
            }
        }
        AnalyticSurfaceDescription::Sphere { frame, radius_mm } => match witness {
            BoundaryCurveMm::CircleArc {
                center_mm,
                u_mm,
                v_mm,
                ..
            } => {
                let d = vector(*center_mm) - vector(frame.origin_mm);
                let u = vector(*u_mm);
                let v = vector(*v_mm);
                let r = u.magnitude();
                d.dot(u).abs() <= tolerance * r
                    && d.dot(v).abs() <= tolerance * r
                    && (d.magnitude().hypot(r) - radius_mm).abs() <= tolerance
            }
            _ => {
                return Err(unsupported(
                    "sphere boundary requires exact circular membership proof",
                ));
            }
        },
        AnalyticSurfaceDescription::Torus {
            frame,
            major_radius_mm,
            minor_radius_mm,
            torus_kind,
        } => {
            if *torus_kind != TorusKind::RegularRing {
                return Err(error(
                    GeometryErrorCode::SingularGeometry,
                    "singular toroidal carrier",
                ));
            }
            match witness {
                BoundaryCurveMm::CircleArc {
                    center_mm,
                    u_mm,
                    v_mm,
                    ..
                } => {
                    let d = vector(*center_mm) - vector(frame.origin_mm);
                    let axis = vector(frame.z_axis);
                    let z = d.dot(axis);
                    let radial = d - z * axis;
                    let u = vector(*u_mm);
                    let v = vector(*v_mm);
                    let normal = u.cross(v).normalize();
                    let radius = u.magnitude();
                    let latitude = radial.magnitude() <= tolerance
                        && normal.cross(axis).magnitude() <= 1e-10
                        && ((radius - major_radius_mm).hypot(z) - minor_radius_mm).abs()
                            <= tolerance;
                    let meridian = z.abs() <= tolerance
                        && (radial.magnitude() - major_radius_mm).abs() <= tolerance
                        && (radius - minor_radius_mm).abs() <= tolerance
                        && normal.dot(axis).abs() <= 1e-10
                        && normal.dot(radial).abs() <= tolerance;
                    latitude || meridian
                }
                _ => {
                    return Err(unsupported(
                        "torus boundary requires exact latitude/meridian circle proof",
                    ));
                }
            }
        }
    };
    if valid {
        Ok(())
    } else {
        Err(invalid(
            "analytic boundary disagrees with its native carrier",
        ))
    }
}

fn boundary(curve: &Curve3D, scale: f64) -> Result<BoundaryCurveMm, GeometryError> {
    match curve {
        Curve3D::SurfaceCurve(c) => {
            let witness = boundary(c.leader(), scale)?;
            for associated in c.associated_geometry() {
                let surface = match associated {
                    SurfaceCurveAssociatedGeometry::Surface(surface) => surface.as_ref(),
                    SurfaceCurveAssociatedGeometry::ParameterCurve(curve) => curve.surface(),
                };
                certify_boundary(&witness, &native_carrier(surface, scale)?)?;
            }
            Ok(witness)
        }
        Curve3D::IntersectionCurve(_) => Err(unsupported(
            "a true native intersection cannot be replaced by its analytic-looking leader",
        )),
        Curve3D::TrimmedCurve(_) => Err(unsupported(
            "retained source trim basis is not an analytic boundary candidate",
        )),
        Curve3D::ParameterCurve(c) => pcurve_boundary(c, scale),
        Curve3D::Line(c) => Ok(BoundaryCurveMm::Line {
            start_mm: array(Point3::from_vec(c.0.to_vec() * scale)),
            end_mm: array(Point3::from_vec(c.1.to_vec() * scale)),
        }),
        Curve3D::Conic(Conic3D::Ellipse(c)) => {
            let matrix = c.transform();
            let center = matrix.transform_point(Point3::origin());
            let u = matrix.transform_vector(Vector3::unit_x());
            let v = matrix.transform_vector(Vector3::unit_y());
            let (a, b) = c.range_tuple();
            let (a, b) = if c.orientation() { (a, b) } else { (b, a) };
            circular_witness(center, u, v, a, b, scale)
        }
        _ => Err(unsupported(
            "native boundary is not an exact admitted line/circle",
        )),
    }
}

/// Source expressions remain exact. Analytic candidates are only metadata for
/// narrow finish operations, never authority to replace an implicit expression.
fn convert_boundary(
    curve: &Curve3D,
    scale: f64,
) -> Result<(Curve, CurveParameterMap, BoundaryCurveMm), GeometryError> {
    let analytic = match boundary(curve, scale) {
        Ok(witness) => Some(witness),
        Err(cause)
            if matches!(
                cause.code,
                GeometryErrorCode::Cancelled | GeometryErrorCode::ResourceLimit
            ) =>
        {
            return Err(cause);
        }
        Err(_) => None,
    };
    let mut exact = curve;
    loop {
        exact = match exact {
            // STEP SurfaceCurve evaluation is exactly its declared 3D leader;
            // original associated PCURVE uses are retained independently.
            Curve3D::SurfaceCurve(c) => c.leader(),
            _ => break,
        };
    }
    let (modeled, conversion) = if matches!(exact, Curve3D::Conic(_)) {
        let modeled = exact.to_parameter_preserving_modeling().map_err(|cause| {
            conversion_error(
                "parameter-preserving native conic conversion refused",
                cause.as_ref(),
            )
        })?;
        let conversion = CurveParameterMap {
            source_range: exact.range_tuple(),
            native_range: modeled.range_tuple(),
            kind: CurveParameterMapKind::Identity,
            source_domain: CurveParameterDomain::Unbounded,
            raw_step_gauge: exact.raw_parameter_gauge().map_err(|cause| {
                conversion_error("original conic raw STEP chart refused", &cause)
            })?,
        };
        (modeled, conversion)
    } else {
        exact.to_modeling_with_parameter_map().map_err(|cause| {
            conversion_error("exact native boundary conversion refused", cause.as_ref())
        })?
    };
    if let CurveParameterMapKind::Unavailable { reason } = conversion.kind {
        return Err(error(
            GeometryErrorCode::ParameterCorrespondenceUnavailable,
            reason,
        ));
    }
    let (a, b) = conversion.source_range;
    let (na, nb) = conversion.native_range;
    if ![a, b, na, nb].iter().all(|v| v.is_finite())
        || a >= b
        || na >= nb
        || conversion.orientation_preserved() != Some(true)
    {
        return Err(invalid(
            "unbounded, collapsed or reversed source/native curve interval",
        ));
    }
    // Endpoints bind topology; the kernel conversion formula, not these samples,
    // establishes whole-interval geometric equivalence.
    let source_start = exact.subs(a);
    let source_end = exact.subs(b);
    for (source, t) in [(source_start, a), (source_end, b)] {
        let native_t = conversion.source_to_native(t).ok_or_else(|| {
            error(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "source/native curve map does not cover an endpoint",
            )
        })?;
        if (source - modeled.subs(native_t)).magnitude() * scale > 1e-7 {
            return Err(kernel("exact conversion changed a source edge endpoint"));
        }
    }
    let witness = analytic.unwrap_or_else(|| BoundaryCurveMm::Native {
        start_mm: array(Point3::from_vec(source_start.to_vec() * scale)),
        end_mm: array(Point3::from_vec(source_end.to_vec() * scale)),
    });
    Ok((modeled, conversion, witness))
}

fn carrier_orientation(
    surface: &StepSurface,
    face_orientation: bool,
) -> Result<bool, GeometryError> {
    let carrier_sign = match surface {
        StepSurface::ElementarySurface(ElementarySurface::Plane(_)) => true,
        StepSurface::ElementarySurface(
            ElementarySurface::CylindricalSurface(c) | ElementarySurface::ConicalSurface(c),
        ) => {
            let profile = c.entity().entity_curve();
            let axial = (profile.1 - profile.0).dot(c.entity().axis());
            if !axial.is_finite() || axial == 0.0 {
                return Err(error(
                    GeometryErrorCode::SingularGeometry,
                    "collapsed analytic revolution profile",
                ));
            }
            (axial < 0.0) == c.orientation()
        }
        StepSurface::ElementarySurface(ElementarySurface::Sphere(c)) => c.orientation(),
        StepSurface::ElementarySurface(ElementarySurface::ToroidalSurface(c)) => c.orientation(),
        // Generic faces retain material-side orientation relative to their
        // regular source chart; exact maps preserve that chart's handedness.
        _ => true,
    };
    Ok(face_orientation == carrier_sign)
}

fn bind_native_identity<S: Copy + Eq + Hash, N: Copy + Eq + Hash>(
    source: S,
    native: N,
    forward: &mut HashMap<S, N>,
    reverse: &mut HashMap<N, S>,
) -> Result<(), GeometryError> {
    if forward
        .insert(source, native)
        .is_some_and(|previous| previous != native)
        || reverse
            .insert(native, source)
            .is_some_and(|previous| previous != source)
    {
        return Err(kernel(
            "modeling conversion split or merged source topology identity",
        ));
    }
    Ok(())
}

/// Bind retained conversion formulas to the final normalized native uses and
/// the immutable original-source intervals. Native conversion was certified
/// before normalization; a BREP convenience range is not source trim authority.
fn validate_parameter_bindings(
    definition: &Definition,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let native_faces = &definition.solid.boundaries()[0];
    if native_faces.len() != definition.face_boundaries.len()
        || native_faces.len() != definition.source_topology.faces.len()
    {
        return Err(kernel(
            "retained parameter face tables lost positional identity",
        ));
    }
    for ((face, retained), source) in native_faces
        .iter()
        .zip(&definition.face_boundaries)
        .zip(&definition.source_topology.faces)
    {
        check_cancel(cancel)?;
        let wires = face.absolute_boundaries();
        if face.id() != retained.face
            || face.orientation() != source.native_orientation
            || wires.len() != retained.wires.len()
            || wires.len() != source.wires.len()
        {
            return Err(kernel(
                "retained parameter face use lost native identity or orientation",
            ));
        }
        for ((wire, refs), source_wire) in wires.iter().zip(&retained.wires).zip(&source.wires) {
            if wire.len() != refs.len() || wire.len() != source_wire.coedges.len() {
                return Err(kernel(
                    "retained parameter wire lost original coedge positions",
                ));
            }
            for ((edge, retained), source_use) in wire.iter().zip(refs).zip(&source_wire.coedges) {
                check_cancel(cancel)?;
                let conversion = definition
                    .boundary_parameters
                    .get(retained.witness)
                    .ok_or_else(|| kernel("retained coedge has no source/native parameter slot"))?;
                let continuation = definition
                    .boundary_continuations
                    .get(retained.witness)
                    .and_then(Option::as_ref);
                let interval=match (conversion,continuation) {
                    (Some(map),None) if map.orientation_preserved()==Some(true)=>map.source_range,
                    (None,Some(branch)) if branch.edge==Some(edge.id())
                        &&branch.scale_to_mm==definition.provenance.source_unit.scale_to_mm()
                        &&branch.original_range==source_use.source_curve_range.range_tuple()
                        &&branch.source_edge==source_use.source_edge
                        &&!matches!(branch.raw_step_gauge,monstertruck_io::step::load::step_geometry::SourceParameterGauge::Unavailable)
                        &&(branch.source_domain==CurveParameterDomain::Bounded||source_use.source_edge.is_some())
                        &&definition.original_continuations.get(branch.prepared).is_some_and(|proof|
                            proof.pieces().first().is_some_and(|piece|piece.parameter[0]==branch.original_range.0)
                            &&proof.pieces().last().is_some_and(|piece|piece.parameter[1]==branch.original_range.1))
                        =>branch.source_range,
                    _=>return Err(kernel("source edge must have exactly one explicit map or original continuation")),
                };
                if retained.witness != source_use.native_edge_index
                    || retained.edge != edge.id()
                    || retained.orientation != edge.orientation()
                    || retained.orientation != source_use.native_orientation
                    || interval != source_use.restricted_curve_range.range_tuple()
                {
                    return Err(kernel(
                        "retained parameter map lost original interval or native edge binding",
                    ));
                }
                if let Some(proof) = retained.correspondence.as_ref() {
                    let binding = proof.binding().ok_or_else(|| {
                        kernel("original coedge receipt lacks normalized ownership")
                    })?;
                    let native_range = conversion
                        .as_ref()
                        .map_or_else(
                            || continuation.map(|branch| branch.source_range),
                            |map| Some(map.native_range),
                        )
                        .ok_or_else(|| kernel("original coedge receipt lacks native range"))?;
                    if binding.face != face.id()
                        || binding.edge != edge.id()
                        || binding.endpoints != [edge.front().id(), edge.back().id()]
                        || binding.source != *source_use
                        || binding.source_face != source.source_face
                        || binding.source_face_use != source.source_face_use
                        || proof.native_range() != [native_range.0, native_range.1]
                        || retained.source_chart_parameter_range
                            != Some((proof.uv_range()[0], proof.uv_range()[1]))
                    {
                        return Err(kernel(
                            "original coedge receipt lost source/native range or ownership",
                        ));
                    }
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn import_step(
    bytes: &[u8],
    cancel: &AtomicBool,
    face_limit: u32,
    query_proof_byte_limit: usize,
) -> Result<Definition, GeometryError> {
    check_cancel(cancel)?;
    let query_proof_byte_limit = query_proof_byte_limit.min(MAX_QUERY_PROOF_BYTES);
    if face_limit == 0 || face_limit > MAX_NATIVE_FACES {
        return Err(error(
            GeometryErrorCode::ResourceLimit,
            "remaining native-face capacity exhausted or exceeds support cap",
        ));
    }
    if bytes.len() > MAX_SOURCE_BYTES as usize {
        return Err(error(
            GeometryErrorCode::ResourceLimit,
            "STEP source cap exceeded",
        ));
    }
    lexical_bound(bytes, cancel)?;
    let text = std::str::from_utf8(bytes).map_err(|_| invalid("STEP source is not UTF-8"))?;
    check_cancel(cancel)?;
    let exchange =
        step_p21::parser::parse(text).map_err(|_| invalid("invalid ISO-10303-21 exchange"))?;
    check_cancel(cancel)?;
    if exchange.data.len() != 1 || !exchange.reference.is_empty() || !exchange.anchor.is_empty() {
        return Err(unsupported("one local DATA section is required"));
    }
    units::validate_schema(&exchange.header)?;
    let entities = Entities::new(&exchange.data[0], cancel)?;
    for records in entities.0.values() {
        for record in records {
            check_cancel(cancel)?;
            if matches!(
                record.name.as_str(),
                "BREP_WITH_VOIDS"
                    | "NEXT_ASSEMBLY_USAGE_OCCURRENCE"
                    | "ASSEMBLY_COMPONENT_USAGE"
                    | "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION"
                    | "REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION"
                    | "MAPPED_ITEM"
                    | "REPRESENTATION_MAP"
            ) {
                return Err(unsupported("assembly or void-shell STEP representation"));
            }
        }
    }
    let solids = entities.named("MANIFOLD_SOLID_BREP", cancel)?;
    if solids.len() != 1 {
        return Err(unsupported("exactly one MANIFOLD_SOLID_BREP is required"));
    }
    let solid_id = solids[0];
    let (source_unit, uncertainty_mm) = units::resolve(&entities, solid_id, cancel)?;
    let scale = source_unit.scale_to_mm();
    let root = entities.record(solid_id, "MANIFOLD_SOLID_BREP")?;
    let shell_id = reference(at(root, 1)?)?;
    let shell = entities.record(shell_id, "CLOSED_SHELL")?;
    entities.validate_geometry(solid_id, cancel)?;
    let canonical = list(at(shell, 1)?)?;
    if canonical.len() > face_limit as usize {
        return Err(error(
            GeometryErrorCode::ResourceLimit,
            "native face cap exceeded",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut faces = Vec::with_capacity(canonical.len());
    for p in canonical {
        check_cancel(cancel)?;
        let id = reference(p)?;
        if !seen.insert(id) {
            return Err(invalid("duplicate canonical shell face use"));
        }
        let resolved = resolve_face(&entities, id)?;
        let (source_entity_kind, source_surface_entity_kind, geometry) =
            source_geometry(&entities, resolved, scale)?;
        faces.push(FaceInfo {
            face_id: FaceId::from_step_entity(id)?,
            surface_face_entity: resolved.to_string(),
            source_entity_kind,
            source_surface_entity_kind,
            orientation: true,
            geometry,
        });
    }
    check_cancel(cancel)?;
    check_cancel(cancel)?;
    let ex = brep_data_bridge::parse_step_exchange(text)
        .map_err(|e| invalid(&format!("invalid ISO-10303-21 exchange: {e}")))?;
    check_cancel(cancel)?;
    let mut read_solids_result = solidcraft_step::topology::read_solids(&ex);
    if !read_solids_result.failures.is_empty() {
        return Err(invalid(&format!(
            "solidcraft topology extraction failure: {:?}",
            read_solids_result.failures
        )));
    }
    if read_solids_result.solids.len() != 1 {
        return Err(unsupported("exactly one solid B-rep is required"));
    }
    let solid = read_solids_result.solids.pop().unwrap();
    if solid.sheet {
        return Err(invalid(
            "open shell / surface model not supported, solid required",
        ));
    }

    // Ensure solidcraft B-Rep is scaled using Spiling's authoritative length scale and uncertainty
    let solid = {
        let mut ctx = solidcraft_step::units::Context::default();
        ctx.units.len = scale;
        ctx.uncertainty = uncertainty_mm;
        solidcraft_step::topology::read_solid(&ex, solid.item, ctx).unwrap_or(solid)
    };

    let mut k = crate::kernel::make_kernel_context();
    let brep = brep_data_bridge::from_schema4_brep(&mut k, &solid.brep, false)
        .map_err(|e| kernel(&format!("B-rep bridge failure: {e}")))?;

    if !solidcore::topology::brep::p2::is_manifold_solid(&brep).unwrap_or(false) {
        return Err(invalid("solid B-rep is not a valid 2-manifold solid"));
    }

    let bbox = solidcore::query::brep::calculate_tight_bounding_box(&k, &brep)
        .map_err(|e| kernel(&format!("bounding box calculation failed: {e}")))?;
    let bounds = spiling_contracts::geometry::AabbMm::new(
        [bbox.min().x, bbox.min().y, bbox.min().z],
        [bbox.max().x, bbox.max().y, bbox.max().z],
    )?;

    let face_step_ids: Vec<StepEntityId> = solid
        .face_origin
        .iter()
        .map(|&id| StepEntityId(id))
        .collect();

    let face_surface_maps: Vec<crate::kernel::SurfaceParameterMap> =
        compute_face_surface_maps(&solid.brep);

    let shell_id = brep
        .shells()
        .ids()
        .next()
        .unwrap_or(solidcore::topology::ShellId::from_index(0));

    let digest: [u8; 32] = Sha256::digest(bytes).into();
    let definition = Definition {
        id: DefinitionId::from_source_sha256(&digest),
        provenance: SourceProvenance {
            source_hash: SourceHash::from_digest(&digest),
            source_name: String::new(),
            source_unit,
            uncertainty_mm,
        },
        faces,
        bounds,
        source_bytes: bytes.len() as u32,
        brep,
        shell_id,
        face_step_ids,
        face_surface_maps,
        source_units_scale_to_mm: scale,
        solid: monstertruck_topology::Solid::new(Vec::new()),
        boundary_curves: Vec::new(),
        face_boundaries: Vec::new(),
        boundary_parameters: Vec::new(),
        boundary_continuations: Vec::new(),
        original_continuations: Vec::new(),
        continuation_bytes: 0,
        boundary_curve_indices: HashMap::new(),
        source_topology: monstertruck_io::step::load::SourceShellProvenance {
            faces: Vec::new(),
            edges: Vec::new(),
        },
        trimmed_solid: monstertruck_topology::trimmed::TrimmedSolid::new(Vec::new()),
    };

    if definition.retained_query_proof_bytes() > query_proof_byte_limit {
        return Err(error(
            GeometryErrorCode::ResourceLimit,
            "retained query proof capacity exhausted",
        ));
    }

    Ok(definition)
}

fn compute_face_surface_maps(
    brep: &solidcraft_geom::brep::Brep,
) -> Vec<crate::kernel::SurfaceParameterMap> {
    brep.faces
        .iter()
        .map(|face| {
            let u0 = face.domain[0][0];
            let v0 = face.domain[1][0];
            match &face.surface {
                solidcraft_geom::brep::Surface::Cylinder { .. }
                | solidcraft_geom::brep::Surface::Cone { .. } => {
                    crate::kernel::SurfaceParameterMap::ScaleOffset {
                        u_scale: 180.0 / std::f64::consts::PI,
                        u_offset: u0,
                        v_scale: 1.0,
                        v_offset: 0.0,
                    }
                }
                solidcraft_geom::brep::Surface::Sphere { .. } => {
                    crate::kernel::SurfaceParameterMap::ScaleOffset {
                        u_scale: 180.0 / std::f64::consts::PI,
                        u_offset: u0,
                        v_scale: 180.0 / std::f64::consts::PI,
                        v_offset: 0.0,
                    }
                }
                solidcraft_geom::brep::Surface::Torus { .. } => {
                    let v_shift =
                        brep_data_bridge::analytic::first_turn([v0, face.domain[1][1]])[0] - v0;
                    crate::kernel::SurfaceParameterMap::ScaleOffset {
                        u_scale: 180.0 / std::f64::consts::PI,
                        u_offset: u0,
                        v_scale: 180.0 / std::f64::consts::PI,
                        v_offset: v_shift,
                    }
                }
                _ => crate::kernel::SurfaceParameterMap::Identity,
            }
        })
        .collect()
}

/// Reads a native Definition from STEP byte slice with default settings.
pub fn from_step_bytes(bytes: &[u8]) -> Result<Definition, GeometryError> {
    from_step_bytes_with_cancel(bytes, &AtomicBool::new(false))
}

/// Reads a native Definition from STEP byte slice with cancellation checking.
pub fn from_step_bytes_with_cancel(
    bytes: &[u8],
    cancel: &AtomicBool,
) -> Result<Definition, GeometryError> {
    import_step(bytes, cancel, MAX_NATIVE_FACES, MAX_QUERY_PROOF_BYTES)
}

/// Reads a native Definition from a STEP file on disk.
pub fn from_step_file(path: impl AsRef<std::path::Path>) -> Result<Definition, GeometryError> {
    let bytes = std::fs::read(path).map_err(|e| {
        GeometryError::new(
            GeometryErrorCode::SourceIo,
            format!("failed to read STEP file: {e}"),
        )
    })?;
    from_step_bytes(&bytes)
}
