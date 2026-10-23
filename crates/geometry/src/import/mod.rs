// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

mod ast;
#[cfg(test)]
mod tests;
mod units;

mod bounds;
mod chart_proof;
mod charts;
mod harmonic;
mod implicit;
mod master;
mod source_conic;
mod source_trim;
mod trim_certificate;
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
    SourceHash, SourceProvenance, SurfaceFamily, TorusKind,
};
use std::{
    collections::{BTreeSet, HashMap},
    hash::Hash,
    sync::atomic::AtomicBool,
};
use step_p21::ast::Parameter;
pub(crate) use trim_certificate::{OriginalCoedgeCorrespondence, OriginalCorrespondenceProvenance};

/// Display-only native coefficient equality; this does not bind an original
/// source PCURVE or make an unproved coedge available to operation preparers.
pub(crate) fn certify_display_native_chart(
    curve: &Curve,
    surface: &monstertruck_modeling::Surface,
    chart: &monstertruck_modeling::Curve2D,
    domain: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    source_trim::certify_display_native_chart(curve, surface, chart, domain, work, cancel)
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
        Curve3D::TrimmedCurve(_) => {
            source_conic::nominal_boundary(curve, scale)?.ok_or_else(|| {
                unsupported("retained source trim basis is not an analytic boundary candidate")
            })
        }
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
    let table = Table::from_data_section(&exchange.data[0]);
    check_cancel(cancel)?;
    if !table.entity_report.is_empty() {
        return Err(invalid("STEP entity construction refused source records"));
    }
    let step_solid = table
        .manifold_solid_brep
        .get(&solid_id)
        .ok_or_else(|| invalid("solid entity was not converted"))?;
    let (mut compact_trims, reports) = table
        .to_compressed_trimmed_solid_reported(step_solid)
        .map_err(|cause| conversion_error("STEP solid conversion refused", &cause))?;
    check_cancel(cancel)?;
    if compact_trims.boundaries.len() != 1
        || reports.len() != 1
        || !reports[0].is_lossless()
        || reports[0].source_shells().len() != 1
    {
        return Err(invalid(
            "reachable STEP conversion loss, missing original lineage or extra shell",
        ));
    }
    let source_topology = &reports[0].source_shells()[0];
    if source_topology.faces.len() != faces.len() {
        return Err(kernel(
            "original source-face lineage count disagrees with native conversion",
        ));
    }
    let mut declared_trims = Vec::with_capacity(faces.len());
    let compact_shell = &mut compact_trims.boundaries[0];
    let source_edges = &compact_shell.edges;
    let mut original_edge_ranges = vec![None; source_edges.len()];
    let mut original_edge_ids = vec![None; source_edges.len()];
    for ((source, info), lineage) in compact_shell
        .faces
        .iter_mut()
        .zip(&faces)
        .zip(&source_topology.faces)
    {
        check_cancel(cancel)?;
        if source.boundaries.len() != lineage.wires.len() {
            return Err(kernel(
                "original source wire lineage count disagrees with conversion",
            ));
        }
        let mut wires = Vec::with_capacity(source.boundaries.len());
        for (wire, source_wire) in source.boundaries.iter_mut().zip(&lineage.wires) {
            let mut trims = Vec::with_capacity(wire.len());
            if wire.len() != source_wire.coedges.len() {
                return Err(kernel(
                    "original source coedge lineage count disagrees with conversion",
                ));
            }
            for (edge_use, source_use) in wire.iter_mut().zip(&source_wire.coedges) {
                check_cancel(cancel)?;
                if edge_use.index != source_use.native_edge_index
                    || edge_use.orientation != source_use.native_orientation
                    || source_edges
                        .get(edge_use.index)
                        .map(|edge| edge.curve.range_tuple())
                        != Some(source_use.restricted_curve_range.range_tuple())
                {
                    return Err(kernel(
                        "original source coedge index, sense or trim interval lost during conversion",
                    ));
                }
                let original_range = source_use.source_curve_range.range_tuple();
                if original_edge_ranges[edge_use.index]
                    .replace(original_range)
                    .is_some_and(|previous| previous != original_range)
                {
                    return Err(kernel(
                        "original edge interval disagrees between native coedge uses",
                    ));
                }
                if let Some(source_id) = source_use.source_edge
                    && original_edge_ids[edge_use.index]
                        .replace(source_id)
                        .is_some_and(|previous| previous != source_id)
                {
                    return Err(kernel(
                        "native edge disagrees between original EDGE_CURVE identities",
                    ));
                }
                let mut trim = edge_use.trim_curve.take();
                if !edge_use.orientation
                    && let Some(curve) = &mut trim
                {
                    curve.invert();
                }
                trims.push(trim);
            }
            wires.push(trims);
        }
        if lineage
            .source_face_use
            .map(FaceId::from_step_entity)
            .transpose()?
            .as_ref()
            != Some(&info.face_id)
            || lineage.source_face.map(|id| id.to_string()).as_ref()
                != Some(&info.surface_face_entity)
            || lineage.native_orientation != source.orientation
        {
            return Err(kernel(
                "native face use lost original STEP entity or orientation lineage",
            ));
        }
        declared_trims.push(wires);
    }
    let compressed = compact_trims.erase_trims();
    let shell = &compressed.boundaries[0];
    if shell.faces.len() != faces.len() {
        return Err(kernel("positional source-face conversion mismatch"));
    }
    let mut boundary_curves = Vec::with_capacity(shell.edges.len() + shell.vertices.len());
    let mut boundary_parameters = Vec::with_capacity(shell.edges.len());
    let mut chart_work = GeometryWorkBudget::new(MAX_IMPORT_CHART_WORK);
    let mut chart_diagnostic = trim_certificate::CorrespondenceDiagnostic::default();
    // Fixed failure-only work partition; no per-query trace or success output.
    let mut preparation_work = 0u64;
    let mut source_coedge_work = 0u64;
    let mut periodic_wire_work = 0u64;
    let mut continuation_owners = HashMap::new();
    for (index, edge) in shell.edges.iter().enumerate() {
        if implicit::is_original_implicit(&edge.curve, &mut chart_work, cancel)? {
            let source_id = original_edge_ids[index];
            let key = (source_id, if source_id.is_some() { 0 } else { index });
            let range = original_edge_ranges[index]
                .ok_or_else(|| kernel("implicit edge lacks original source range"))?;
            let next = continuation_owners.len();
            let (_, previous) = *continuation_owners.entry(key).or_insert((next, range));
            if previous != range {
                return Err(kernel(
                    "one original EDGE_CURVE has inconsistent whole continuation ranges",
                ));
            }
        }
    }
    let continuation_slots = if continuation_owners.is_empty() {
        0
    } else {
        shell.edges.len()
    };
    let reserved_index_bytes = if continuation_slots == 0 {
        0
    } else {
        query_index_storage_bound(
            shell
                .edges
                .len()
                .checked_mul(2)
                .and_then(|n| n.checked_add(8))
                .ok_or_else(|| {
                    error(
                        GeometryErrorCode::ResourceLimit,
                        "original edge index capacity overflow",
                    )
                })?,
        )
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "original edge index capacity overflow",
            )
        })?
    };
    let mut continuation_bytes = continuation_slots
        .checked_mul(std::mem::size_of::<
            Option<crate::kernel::OriginalBoundaryContinuation>,
        >())
        .and_then(|slots| {
            continuation_owners
                .len()
                .checked_mul(std::mem::size_of::<
                    monstertruck_modeling::PreparedValidatedCurve<'static>,
                >())
                .and_then(|proofs| slots.checked_add(proofs))
        })
        .and_then(|bytes| bytes.checked_add(reserved_index_bytes))
        .filter(|bytes| *bytes <= query_proof_byte_limit)
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "whole original continuation owner capacity exhausted",
            )
        })?;
    let mut boundary_continuations = Vec::with_capacity(continuation_slots);
    let mut original_continuations = Vec::with_capacity(continuation_owners.len());
    // One import-only whole-original master receipt per precharged owner.
    // Owners never reallocate (implicit::prepare refuses capacity growth), so
    // exact branch bindings remain live/stable through all native children.
    let master_slot_size = std::mem::size_of::<Option<
        trim_certificate::SelectedMasterReceipt,
    >>();
    let master_slot_bytes = continuation_owners.len().checked_mul(master_slot_size)
        .ok_or_else(|| error(
            GeometryErrorCode::ResourceLimit, "selected master receipt capacity overflow",
        ))?;
    if master_slot_bytes > query_proof_byte_limit - continuation_bytes {
        return Err(error(
            GeometryErrorCode::ResourceLimit, "selected master receipt capacity exhausted",
        ));
    }
    chart_work.charge(continuation_owners.len() as u64, cancel)?;
    let mut selected_master_receipts: Vec<Option<trim_certificate::SelectedMasterReceipt>> =
        vec![None; continuation_owners.len()];
    let master_slot_bytes = selected_master_receipts.capacity().checked_mul(master_slot_size)
        .ok_or_else(|| error(
            GeometryErrorCode::ResourceLimit, "selected master receipt capacity overflow",
        ))?;
    continuation_bytes = continuation_bytes.checked_add(master_slot_bytes)
        .filter(|bytes| *bytes <= query_proof_byte_limit)
        .ok_or_else(|| error(
            GeometryErrorCode::ResourceLimit, "selected master receipt capacity exhausted",
        ))?;
    let mut modeled_edges = Vec::with_capacity(shell.edges.len());
    for (edge_index, edge) in shell.edges.iter().enumerate() {
        // Candidate decomposition has a hard 32-wrapper structural bound.
        // Debit its maximum traversal against the same job as chart proofs.
        chart_work.charge(33, cancel)?;
        let original_range = original_edge_ranges[edge_index]
            .ok_or_else(|| kernel("native edge has no original whole-source interval binding"))?;
        let source_id = original_edge_ids[edge_index];
        let key = (source_id, if source_id.is_some() { 0 } else { edge_index });
        if let Some(&(owner, _)) = continuation_owners.get(&key) {
            let shared = (owner < original_continuations.len()).then_some(owner);
            let preparation_started = chart_work.consumed();
            let prepared = implicit::prepare(
                implicit::ContinuationInput {
                    source: &edge.curve,
                    whole: original_range,
                    shared,
                    scale,
                },
                &mut original_continuations,
                &mut continuation_bytes,
                query_proof_byte_limit,
                &mut chart_work,
                cancel,
            );
            preparation_work += chart_work.consumed() - preparation_started;
            let (curve, mut branch, witness) = prepared
            .map_err(|cause| {
                admission_error(
                    cause,
                    "original-continuation",
                    &chart_work,
                    format_args!("source edge {source_id:?}, native edge {edge_index}"),
                )
            })?;
            branch.source_edge = source_id;
            let original = implicit::original_source(&edge.curve, original_range)?;
            let original_native = implicit::original_native_curve(&curve, original_range)?;
            master::validate_selected_master(
                original,
                Some(original_range),
                Some((original_native, &original_continuations[branch.prepared])),
                &mut selected_master_receipts[branch.prepared],
                scale,
                &mut chart_work,
                cancel,
            )
            .map_err(|cause| {
                admission_error(
                    cause,
                    "implicit-selected-master",
                    &chart_work,
                    format_args!("source edge {source_id:?}, native edge {edge_index}"),
                )
            })?;
            boundary_curves.push(witness);
            boundary_parameters.push(None);
            boundary_continuations.push(Some(branch));
            modeled_edges.push(monstertruck_topology::compress::CompressedEdge {
                vertices: edge.vertices,
                curve,
            });
            continue;
        }
        master::validate_selected_master(
            &edge.curve, Some(original_range), None, &mut None, scale, &mut chart_work, cancel,
        )
        .map_err(|cause| {
            admission_error(
                cause,
                "selected-master",
                &chart_work,
                format_args!("source edge {source_id:?}, native edge {edge_index}"),
            )
        })?;
        let (curve, parameter, witness) = convert_boundary(&edge.curve, scale)?;
        boundary_curves.push(witness);
        boundary_parameters.push(Some(parameter));
        if continuation_slots != 0 {
            boundary_continuations.push(None);
        }
        modeled_edges.push(monstertruck_topology::compress::CompressedEdge {
            vertices: edge.vertices,
            curve,
        });
    }
    let edge_phase_work = chart_work.consumed();
    drop((continuation_owners, original_edge_ids, original_edge_ranges));
    for point in &shell.vertices {
        let p = array(Point3::from_vec(point.to_vec() * scale));
        if !p.iter().all(|v| v.is_finite()) {
            return Err(invalid("nonfinite normalized native vertex"));
        }
        boundary_curves.push(BoundaryCurveMm::Line {
            start_mm: p,
            end_mm: p,
        });
    }
    // Preserve original source face/wire/coedge attribution before modeled
    // topology construction. Shared geometry stays in the one boundary witness
    // store. Periodic subdivision is already complete here.
    let mut coedge_count = 0usize;
    let mut wire_count = 0usize;
    let proof_cap = 2 * spiling_contracts::geometry::MAX_STEP_ENTITIES as usize;
    for face in &shell.faces {
        check_cancel(cancel)?;
        wire_count = wire_count
            .checked_add(face.boundaries.len())
            .ok_or_else(|| kernel("native trim reference count overflow"))?;
        for wire in &face.boundaries {
            check_cancel(cancel)?;
            coedge_count = coedge_count
                .checked_add(wire.len())
                .ok_or_else(|| kernel("native coedge reference count overflow"))?;
        }
        if coedge_count > proof_cap || wire_count > proof_cap {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "native analytic trim reference cap exceeded",
            ));
        }
    }
    // Charge complete fixed records at their source owner, including alignment
    // and the inline receipt. A second record-sized reserve bounds temporary
    // metadata coexistence; only the final source-owned records remain charged.
    let correspondence_bytes = coedge_count
        .checked_mul(std::mem::size_of::<BoundaryCoedgeRef>())
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "original coedge receipt capacity overflow",
            )
        })?;
    if continuation_bytes
        .checked_add(correspondence_bytes.saturating_mul(2))
        .is_none_or(|bytes| bytes > query_proof_byte_limit)
    {
        return Err(error(
            GeometryErrorCode::ResourceLimit,
            "original coedge receipt owner capacity exhausted",
        ));
    }
    continuation_bytes += correspondence_bytes;
    let mut trim_slots = Vec::with_capacity(shell.faces.len());
    for face in &shell.faces {
        check_cancel(cancel)?;
        let mut wires = Vec::with_capacity(face.boundaries.len());
        for wire in &face.boundaries {
            let mut coedges = Vec::with_capacity(wire.len());
            for coedge in wire {
                check_cancel(cancel)?;
                let endpoints = shell
                    .edges
                    .get(coedge.index)
                    .ok_or_else(|| kernel("source coedge refers to a missing edge"))?
                    .vertices;
                coedges.push((*coedge, endpoints));
            }
            wires.push(coedges);
        }
        trim_slots.push(wires);
    }
    let mut face_parameters = Vec::with_capacity(shell.faces.len());
    let mut modeled_faces = Vec::with_capacity(shell.faces.len());
    let mut source_trims = Vec::with_capacity(shell.faces.len());
    let mut source_trim_metadata = Vec::with_capacity(shell.faces.len());
    for (face_index, ((source, info), declared)) in shell
        .faces
        .iter()
        .zip(&mut faces)
        .zip(declared_trims)
        .enumerate()
    {
        check_cancel(cancel)?;
        let face_started_work = chart_work.consumed();
        let (surface, conversion) =
            source
                .surface
                .to_modeling_with_parameter_map()
                .map_err(|cause| {
                    conversion_error(
                        "exact native source surface conversion refused",
                        cause.as_ref(),
                    )
                })?;
        if let SurfaceParameterMap::Unavailable { reason } = conversion {
            return Err(error(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                reason,
            ));
        }
        info.orientation = carrier_orientation(&source.surface, source.orientation)?;
        info.geometry.domain.periodic_u = surface.period_u().is_some();
        info.geometry.domain.periodic_v = surface.period_v().is_some();
        info.geometry.domain.trim_loop_count = source.boundaries.len() as u32;
        info.geometry.domain.inner_loop_count = source.boundaries.len().saturating_sub(1) as u32;
        info.validate()?;
        let native_uv_domain = source_trim::intrinsic_domain(&source.surface, &surface, conversion);
        // Count actual face occurrences, including different original bounds.
        // Coincident carrier coefficients never establish a seam association.
        let mut face_edge_uses = HashMap::<usize, u8>::new();
        for coedge in source.boundaries.iter().flatten() {
            chart_work.charge(1, cancel)?;
            let count = face_edge_uses.entry(coedge.index).or_default();
            *count = count.saturating_add(1);
        }
        let mut trim_wires = Vec::with_capacity(source.boundaries.len());
        let mut metadata_wires = Vec::with_capacity(source.boundaries.len());
        if declared.len() != source.boundaries.len() {
            return Err(kernel(
                "original PCURVE wire count changed during source conversion",
            ));
        }
        for (wire_index, (wire, declared_wire)) in
            source.boundaries.iter().zip(declared).enumerate()
        {
            if wire.len() != declared_wire.len() {
                return Err(kernel(
                    "original PCURVE coedge count changed during source conversion",
                ));
            }
            let mut witnesses = Vec::with_capacity(wire.len());
            let mut unavailable_seam_binding = false;
            for (coedge_index, (coedge, declared_trim)) in
                wire.iter().zip(&declared_wire).enumerate()
            {
                check_cancel(cancel)?;
                let missing_required_branch = declared_trim.is_none()
                    && source_trim::requires_bound_seam(
                        &shell.edges[coedge.index].curve,
                        face_edge_uses[&coedge.index] > 1,
                        &mut chart_work, cancel,
                    )?;
                unavailable_seam_binding |= missing_required_branch;
                let coedge_started = chart_work.consumed();
                let retained = if missing_required_branch {
                    // Preserve 3D inspection, never synthesize the absent
                    // original seam branch through native or neighbor UV.
                    Ok(source_trim::SourceTrim::retained(None))
                } else {
                    source_trim::retain_with_continuation(
                    source_trim::TrimInput {
                        source_curve: &shell.edges[coedge.index].curve,
                        native_curve: &modeled_edges[coedge.index].curve,
                        surface: source_trim::SurfaceInput {
                            source_surface: &source.surface,
                            native_surface: &surface,
                            map: conversion,
                            scale,
                        },
                        declared: declared_trim.as_ref(),
                    },
                    boundary_continuations
                        .get(coedge.index)
                        .and_then(Option::as_ref)
                        .map(|branch| &original_continuations[branch.prepared]),
                    boundary_continuations
                        .get(coedge.index)
                        .and_then(Option::as_ref)
                        .and_then(|branch| selected_master_receipts[branch.prepared].as_ref()),
                    &mut chart_work,
                    cancel,
                    )
                };
                source_coedge_work += chart_work.consumed() - coedge_started;
                let witness = retained
                .map_err(|cause| {
                    error(cause.code, &format!(
                        "source-chart f={} s#{} w={wire_index} c={coedge_index} e={}/orig{:?};work={}/{} fs={face_started_work} fw={} prep={preparation_work} edge={edge_phase_work} init={source_coedge_work} per={periodic_wire_work} oth={}: {}",
                        info.face_id.as_str(), info.surface_face_entity, coedge.index,
                        source_topology.faces[face_index].wires[wire_index].coedges
                            [coedge_index].source_edge,
                        chart_work.consumed(), chart_work.remaining(),
                        chart_work.consumed() - face_started_work,
                        chart_work.consumed() - edge_phase_work - source_coedge_work - periodic_wire_work,
                        cause.message,
                    ))
                })?;
                if boundary_continuations
                    .get(coedge.index)
                    .and_then(Option::as_ref)
                    .is_some()
                    && !witness.certificate.is_certified()
                    && !missing_required_branch
                {
                    return Err(error(
                        GeometryErrorCode::ParameterCorrespondenceUnavailable,
                        "original implicit coedge lacks whole retained-branch UV correspondence",
                    ));
                }
                witnesses.push(witness);
            }
            if !unavailable_seam_binding {
            let periodic_started = chart_work.consumed();
            let periodic_result = source_trim::bind_periodic_wire(
                &mut witnesses,
                |index| {
                    let coedge = wire[index];
                    source_trim::PeriodicUse {
                        face: face_index,
                        wire: wire_index,
                        coedge: index,
                        source_edge: source_topology.faces[face_index].wires[wire_index].coedges
                            [index]
                            .source_edge,
                        native_edge: coedge.index,
                        source: &shell.edges[coedge.index].curve,
                        native: &modeled_edges[coedge.index].curve,
                        declared: declared_wire[index].as_ref(),
                        retained: boundary_continuations
                            .get(coedge.index)
                            .and_then(Option::as_ref)
                            .map(|branch| &original_continuations[branch.prepared]),
                        master: boundary_continuations
                            .get(coedge.index)
                            .and_then(Option::as_ref)
                            .and_then(|branch| selected_master_receipts[branch.prepared].as_ref()),
                        orientation: coedge.orientation,
                        vertices: shell.edges[coedge.index].vertices,
                        front_vertex: shell.vertices[if coedge.orientation {
                            shell.edges[coedge.index].vertices.0
                        } else {
                            shell.edges[coedge.index].vertices.1
                        }],
                    }
                },
                source_trim::SurfaceInput {
                    source_surface: &source.surface,
                    native_surface: &surface,
                    map: conversion,
                    scale,
                },
                &mut chart_work,
                cancel,
                &mut chart_diagnostic,
            );
            periodic_wire_work += chart_work.consumed() - periodic_started;
            periodic_result
            .map_err(|cause| {
                // Failure-only signatures: the saved whole master is not a
                // child receipt. Keep its paid slot alive while collecting
                // actual affine anchors/ranges and wrapper frames below.
                let active = chart_diagnostic.active_source;
                let branch = active.and_then(|use_| boundary_continuations
                    .get(use_.native_edge).and_then(Option::as_ref));
                let master = branch.and_then(|branch|
                    selected_master_receipts[branch.prepared].as_ref().map(|receipt| receipt.whole()));
                let child = active.and_then(|use_| witnesses.get(use_.coedge))
                    .and_then(|witness| witness.correspondence.as_ref());
                let signature = |proof: &OriginalCoedgeCorrespondence| {
                    let slope = proof.uv_parameter_derivative();
                    (proof.native_range(), proof.uv_range(), [slope.lo, slope.hi],
                        proof.margin_uv(), proof.position_error_mm())
                };
                let contained = master.zip(child).map(|(whole, child)| {
                    let whole = whole.native_range();
                    let child = child.native_range();
                    whole[0] <= child[0] && child[1] <= whole[1]
                });
                let root = branch.map(|branch| {
                    let owner = &original_continuations[branch.prepared];
                    let mut leaf = owner;
                    let mut depth = 0;
                    while depth < 32 {
                        let Some(basis) = leaf.mapped_basis() else { break };
                        leaf = basis;
                        depth += 1;
                    }
                    (owner.range(), leaf.range(), depth)
                });
                let native_outer = active.and_then(|use_| {
                    match &modeled_edges[use_.native_edge].curve {
                        Curve::TrimmedCurve(wrapper) => Some((
                            wrapper.entity().range_tuple(), wrapper.orientation(),
                            *wrapper.transform() == Matrix4::identity(),
                        )),
                        _ => None,
                    }
                });
                let uv_outer = active.and_then(|use_| witnesses.get(use_.coedge))
                    .and_then(|witness| witness.curve.as_ref())
                    .and_then(|chart| match chart {
                        monstertruck_modeling::Curve2D::TrimmedCurve(wrapper) => Some((
                            wrapper.entity().range_tuple(), wrapper.orientation(),
                            *wrapper.transform() == Matrix3::identity(),
                        )),
                        _ => None,
                    });
                let current_joint = chart_diagnostic.retained_joint.as_ref().map(|joint| (
                    joint.phase, chart_work.consumed().checked_sub(joint.phase_started_work),
                    joint.initial_achieved_native, joint.metric.derived_limit,
                    joint.refined_achieved_native, joint.final_margin_uv,
                ));
                let (current, wrappers) = cause.message.split_once(" | wrappers:")
                    .unwrap_or((&cause.message, ""));
                error(cause.code, &format!(
                    "span={:?};current-joint(phase,work,base,goal,refined,proposed_uv)={current_joint:?} eval={:?} use={:?}; face={} source=#{} wire={wire_index} face-start-work={face_started_work} current-face-work={} work-partition preparation={preparation_work} edge-phase={edge_phase_work} initial-coedges={source_coedge_work} periodic={periodic_wire_work} current-periodic={} other={}; old-master={:?} old-witness-child={:?} contained={contained:?} root={root:?} native_outer={native_outer:?} old-witness-uv_outer={uv_outer:?}: {current}; {chart_diagnostic} | wrappers:{wrappers}",
                    chart_diagnostic.failed_span,
                    chart_diagnostic.evaluation,
                    active.map(|use_| (use_.coedge, use_.native_edge, use_.source_edge, use_.shift)),
                    info.face_id.as_str(), info.surface_face_entity,
                    chart_work.consumed() - face_started_work,
                    chart_work.consumed() - periodic_started,
                    chart_work.consumed() - edge_phase_work - source_coedge_work - periodic_wire_work,
                    master.map(signature), child.map(signature),
                ))
            })?;
            }
            let mut metadata = Vec::with_capacity(wire.len());
            let trims = witnesses
                .into_iter()
                .map(|witness| {
                    let parameter_range = witness.curve.as_ref().map(|curve| curve.range_tuple());
                    metadata.push((
                        witness.margin_uv,
                        witness.error_mm,
                        parameter_range,
                        witness.certificate,
                        witness.declared_source,
                        witness.correspondence,
                    ));
                    witness.curve
                })
                .collect();
            trim_wires.push(trims);
            metadata_wires.push(metadata);
        }
        source_trim_metadata.push(metadata_wires);
        source_trims.push(trim_wires);
        face_parameters.push(FaceParameterWitness {
            conversion,
            native_uv_domain,
        });
        modeled_faces.push(monstertruck_topology::compress::CompressedFace {
            boundaries: source.boundaries.clone(),
            orientation: source.orientation,
            surface,
        });
    }
    drop(selected_master_receipts);
    continuation_bytes -= master_slot_bytes;
    // Move compressed topology once; exact conversion does not clone an entire
    // source BREP or infer correspondence from equal source/native UV samples.
    let source_edge_count = modeled_edges.len();
    let mut boundaries = compressed.boundaries;
    let source = boundaries
        .pop()
        .ok_or_else(|| invalid("missing converted shell"))?;
    let mapped =
        monstertruck_topology::Solid::extract(monstertruck_topology::compress::CompressedSolid {
            boundaries: vec![monstertruck_topology::compress::CompressedShell {
                vertices: source.vertices,
                edges: modeled_edges,
                faces: modeled_faces,
                vertex_stable_ids: source.vertex_stable_ids,
                edge_stable_ids: source.edge_stable_ids,
                face_stable_ids: source.face_stable_ids,
            }],
            id_allocator: compressed.id_allocator,
            attributes: compressed.attributes,
        })
        .map_err(|cause| invalid(&format!("native solid extraction failed: {cause}")))?;
    check_cancel(cancel)?;
    let solid = builder::transformed(&mapped, Matrix4::from_scale(scale));
    check_cancel(cancel)?;
    if solid.boundaries().len() != 1 || solid.boundaries()[0].len() != faces.len() {
        return Err(kernel("modeling conversion changed positional faces"));
    }
    for shell in solid.boundaries() {
        shell
            .check_solid_boundary()
            .map_err(|_| invalid("normalized solid is not closed manifold"))?;
    }
    let mut native_edges = HashMap::new();
    if continuation_slots != 0 {
        native_edges.try_reserve(continuation_slots).map_err(|_| {
            error(
                GeometryErrorCode::ResourceLimit,
                "native edge index allocation failed",
            )
        })?;
        let actual_index = query_index_storage_bound(native_edges.capacity()).ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "native edge index capacity overflow",
            )
        })?;
        if actual_index > reserved_index_bytes {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "native edge index exceeds precharged storage bound",
            ));
        }
        continuation_bytes = continuation_bytes - reserved_index_bytes + actual_index;
        for (face, slots) in solid.boundaries()[0].iter().zip(&trim_slots) {
            for (wire, slots) in face.absolute_boundaries().iter().zip(slots) {
                for (edge, (slot, _)) in wire.iter().zip(slots) {
                    check_cancel(cancel)?;
                    if native_edges
                        .insert(edge.id(), slot.index)
                        .is_some_and(|index| index != slot.index)
                    {
                        return Err(kernel(
                            "original native boundary index changed during normalization",
                        ));
                    }
                    if let Some(branch) = boundary_continuations
                        .get_mut(slot.index)
                        .and_then(Option::as_mut)
                    {
                        if branch.edge.is_some_and(|id| id != edge.id()) {
                            return Err(kernel("original continuation split native edge identity"));
                        }
                        branch.edge = Some(edge.id());
                    }
                }
            }
        }
    }
    let (bounds, bound_refinement) = import_bounds(
        bounds::ImportBoundsInput {
            solid: &solid,
            faces: &faces,
            source_trims: &source_trims,
            parameters: &face_parameters,
            continuations: &boundary_continuations,
            proofs: &original_continuations,
            edge_count: source_edge_count,
            source_slots: &trim_slots,
        },
        cancel,
    )?;
    let mut face_boundaries = Vec::with_capacity(faces.len());
    let mut source_edges = HashMap::new();
    let mut source_vertices = HashMap::new();
    let mut native_vertices = HashMap::new();
    let mut trimmed_faces = Vec::with_capacity(faces.len());
    for (face_index, (((face, slots), parameter), trims)) in solid.boundaries()[0]
        .iter()
        .zip(&trim_slots)
        .zip(face_parameters)
        .zip(source_trims)
        .enumerate()
    {
        check_cancel(cancel)?;
        let wires = face.absolute_boundaries();
        if wires.len() != slots.len() {
            return Err(kernel("modeling conversion changed positional trim wires"));
        }
        let mut retained = Vec::with_capacity(wires.len());
        for (wire_index, (wire, slots)) in wires.iter().zip(slots).enumerate() {
            check_cancel(cancel)?;
            if wire.len() != slots.len() {
                return Err(kernel("modeling conversion changed positional coedges"));
            }
            let mut refs = Vec::with_capacity(wire.len());
            for (coedge_index, (edge, (slot, source_endpoints))) in
                wire.iter().zip(slots).enumerate()
            {
                check_cancel(cancel)?;
                let witness = boundary_curves
                    .get(slot.index)
                    .ok_or_else(|| kernel("native coedge witness is missing"))?;
                let (mut a, mut b) = witness.endpoints();
                if !slot.orientation {
                    std::mem::swap(&mut a, &mut b);
                }
                let front: [f64; 3] = edge.front().point().into();
                let back: [f64; 3] = edge.back().point().into();
                if let Some(branch) = boundary_continuations
                    .get(slot.index)
                    .and_then(Option::as_ref)
                {
                    let prepared = crate::parametric::PreparedCurveEnclosure::borrowed_restricted(
                        &original_continuations[branch.prepared],
                        scale,
                        branch.source_range,
                    );
                    let range = branch.source_range;
                    let parameters = if slot.orientation {
                        [range.0, range.1]
                    } else {
                        [range.1, range.0]
                    };
                    for (parameter, vertex) in parameters.into_iter().zip([front, back]) {
                        let point =
                            prepared.endpoint_position(parameter, &mut chart_work, cancel)?;
                        let residual =
                            (0..3).fold(crate::parametric::Interval::point(0.0), |sum, axis| {
                                sum.add(
                                    point[axis]
                                        .sub(crate::parametric::Interval::point(vertex[axis]))
                                        .square(),
                                )
                            });
                        if residual.hi > 1.0e-14 {
                            return Err(invalid(
                                "original implicit branch endpoint does not bind its native source vertex",
                            ));
                        }
                    }
                } else {
                    let consistent = edge.is_geometric_consistent();
                    let included = consistent.then(|| {
                        face.with_surface(|surface| edge.with_curve(|curve| surface.include(curve)))
                    });
                    if included != Some(true) {
                        let endpoints = edge.with_curve(|curve| {
                            let range = curve.range_tuple();
                            (
                                range,
                                array(curve.subs(range.0)),
                                array(curve.subs(range.1)),
                            )
                        });
                        return Err(invalid(&format!(
                            "normalized explicit native coedge is geometrically inconsistent: face {} wire {wire_index} coedge {coedge_index} native edge {} orientation={} edge_consistent={consistent} carrier_includes={included:?} native_curve={endpoints:?} topology_endpoints={front:?}/{back:?}",
                            faces[face_index].face_id.as_str(), slot.index, edge.orientation(),
                        )));
                    }
                }
                let distance =
                    |p: [f64; 3], q: [f64; 3]| (p[0] - q[0]).hypot(p[1] - q[1]).hypot(p[2] - q[2]);
                if edge.orientation() != slot.orientation
                    || !a.iter().chain(&b).all(|v| v.is_finite())
                    || distance(a, front) > 1e-7
                    || distance(b, back) > 1e-7
                {
                    return Err(kernel(
                        "modeling conversion changed analytic coedge endpoints",
                    ));
                }
                bind_native_identity(slot.index, edge.id(), &mut source_edges, &mut native_edges)?;
                let (source_front, source_back) = if slot.orientation {
                    *source_endpoints
                } else {
                    (source_endpoints.1, source_endpoints.0)
                };
                bind_native_identity(
                    source_front,
                    edge.front().id(),
                    &mut source_vertices,
                    &mut native_vertices,
                )?;
                bind_native_identity(
                    source_back,
                    edge.back().id(),
                    &mut source_vertices,
                    &mut native_vertices,
                )?;
                let metadata = source_trim_metadata[face_index][wire_index][coedge_index];
                // Normalization can change coefficients AND chart ranges.
                // Recompute retained base/tube position on these actual final
                // operands and independently prove expanded-tube tangents;
                // no scaled pre-normalization residual supplies authority.
                let correspondence = if let Some(previous) = metadata.5 {
                    let chart = trims[wire_index][coedge_index].as_ref().ok_or_else(|| {
                        kernel("completed original correspondence lost its UV coefficients")
                    })?;
                    let mut proof = face.with_surface(|surface| edge.with_curve(|curve| {
                        let continuation = boundary_continuations
                            .get(slot.index).and_then(Option::as_ref);
                        if previous.provenance() == OriginalCorrespondenceProvenance::ExactCoefficients
                            && continuation.is_none()
                            && metadata.0 == [0.0; 2]
                            && source_trim::native_chart(
                                curve, surface, Some(chart), parameter.native_uv_domain,
                                &mut chart_work, cancel,
                            )?.is_some()
                        {
                            return trim_certificate::OriginalCoedgeCorrespondence::exact_coefficients(
                                curve, chart, &mut chart_work, cancel,
                            ).map(Some);
                        }
                        let input = trim_certificate::Correspondence {
                            curve, surface, chart,
                            maximum_error_mm: 1.0e-7,
                            margin_uv: metadata.0,
                        };
                        let proof = if let Some(branch) = continuation {
                            trim_certificate::certify_normalized_retained_correspondence(
                                input, (&original_continuations[branch.prepared], scale),
                                Some(&previous),
                                &mut chart_work, cancel, None,
                            )?
                        } else {
                            trim_certificate::certify_declared_coedge_correspondence(
                                input, &mut chart_work, cancel, None,
                            )?
                        };
                        Ok(proof)
                    })).map_err(|cause| admission_error(
                        cause,
                        "normalized-correspondence",
                        &chart_work,
                        format_args!(
                            "face {} (source #{}), wire {wire_index}, coedge {coedge_index}, source edge {:?}, native edge {}",
                            faces[face_index].face_id.as_str(),
                            faces[face_index].surface_face_entity,
                            source_topology.faces[face_index].wires[wire_index].coedges
                                [coedge_index].source_edge,
                            slot.index,
                        ),
                    ))?;
                    if previous.provenance() == OriginalCorrespondenceProvenance::CompletedPoleGraph
                        && let Some(receipt) = proof.as_mut()
                    {
                        receipt.complete_pole_graph();
                    }
                    proof
                } else {
                    None
                };
                let source_chart_error_mm = correspondence.as_ref().map_or(metadata.1, |proof| {
                    metadata.1.max(proof.position_error_mm())
                });
                let correspondence = correspondence.map(|proof| {
                    proof.bind(crate::kernel::OriginalCoedgeBinding {
                        face: face.id(),
                        edge: edge.id(),
                        endpoints: [edge.front().id(), edge.back().id()],
                        owner: [face_index, wire_index, coedge_index],
                        source_face: source_topology.faces[face_index].source_face,
                        source_face_use: source_topology.faces[face_index].source_face_use,
                        source: source_topology.faces[face_index].wires[wire_index].coedges
                            [coedge_index],
                    })
                });
                refs.push(BoundaryCoedgeRef {
                    witness: slot.index,
                    orientation: slot.orientation,
                    edge: edge.id(),
                    correspondence,
                    source_chart_margin_uv: source_trim_metadata[face_index][wire_index]
                        [coedge_index]
                        .0,
                    source_chart_error_mm,
                    source_chart_parameter_range: source_trim_metadata[face_index][wire_index]
                        [coedge_index]
                        .2,
                    source_chart_certificate: source_trim_metadata[face_index][wire_index]
                        [coedge_index]
                        .3,
                    source_chart_declared: source_trim_metadata[face_index][wire_index]
                        [coedge_index]
                        .4,
                });
            }
            retained.push(refs);
        }
        face_boundaries.push(FaceBoundaryRefs {
            face: face.id(),
            wires: retained,
            parameter,
        });
        trimmed_faces.push(
            monstertruck_topology::trimmed::TrimmedFace::try_new(face.clone(), trims).map_err(
                |cause| {
                    kernel(&format!(
                        "retained exact native trim topology mismatch: {cause}"
                    ))
                },
            )?,
        );
    }
    drop((trim_slots, source_edges, source_vertices, native_vertices));
    let trimmed_shell: monstertruck_topology::trimmed::TrimmedShell<_, _, _, _> =
        trimmed_faces.into_iter().collect();
    let trimmed_solid = std::iter::once(trimmed_shell).collect();
    check_cancel(cancel)?;
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    let mut source_shells = reports
        .into_iter()
        .next()
        .ok_or_else(|| kernel("missing source report"))?
        .into_source_shells();
    let source_topology = source_shells
        .pop()
        .ok_or_else(|| kernel("missing original source topology"))?;
    let boundary_curve_indices = if original_continuations.is_empty() {
        HashMap::new()
    } else {
        native_edges
    };
    let mut definition = Definition {
        solid,
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
        boundary_curves,
        face_boundaries,
        boundary_parameters,
        boundary_continuations,
        original_continuations,
        continuation_bytes,
        boundary_curve_indices,
        source_topology,
        trimmed_solid,
    };
    validate_parameter_bindings(&definition, cancel)?;
    bounds::tighten_import_bounds(&mut definition, bound_refinement, cancel)?;
    Ok(definition)
}
