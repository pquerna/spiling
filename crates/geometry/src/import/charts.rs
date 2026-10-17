// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Structural source charts, before conic curves acquire rational parameters.
//! These formulas use the complete line/circle coefficients and their bounded
//! oriented intervals. No nearest-point projection or sampled inverse is used.
use super::{BoundaryCurveMm, boundary, error, invalid};
use monstertruck_io::step::load::step_geometry::{
    Curve3D, ElementarySurface, Surface, SweepSurface,
};
use monstertruck_modeling::{Curve2D, Line, Processor, RevolutionSurface, base::*};
use spiling_contracts::geometry::{GeometryError, GeometryErrorCode};
use std::f64::consts::{PI, TAU};

const RESIDUAL: f64 = 1e-7;

fn point(p: [f64; 3]) -> Point3 {
    Point3::new(p[0], p[1], p[2])
}
fn vector(p: [f64; 3]) -> Vector3 {
    Vector3::new(p[0], p[1], p[2])
}
fn line(a: (f64, f64), b: (f64, f64), swapped: bool) -> Curve2D {
    let p = |(u, v)| {
        if swapped {
            Point2::new(v, u)
        } else {
            Point2::new(u, v)
        }
    };
    Curve2D::Line(Line(p(a), p(b)))
}
fn regular(v: Vector3) -> Result<Vector3, GeometryError> {
    if !v.magnitude2().is_finite() || v.magnitude2() <= 1e-20 {
        return Err(error(
            GeometryErrorCode::SingularGeometry,
            "singular source trim chart basis",
        ));
    }
    Ok(v.normalize())
}
fn inverse(matrix: &Matrix4) -> Result<Matrix4, GeometryError> {
    matrix
        .invert()
        .ok_or_else(|| invalid("singular source chart placement"))
}
fn circle_local(
    witness: &BoundaryCurveMm,
    matrix: &Matrix4,
) -> Option<(Point3, Vector3, Vector3, f64, f64)> {
    let BoundaryCurveMm::CircleArc {
        center_mm,
        u_mm,
        v_mm,
        start_angle,
        end_angle,
    } = witness
    else {
        return None;
    };
    Some((
        matrix.transform_point(point(*center_mm)),
        matrix.transform_vector(vector(*u_mm)),
        matrix.transform_vector(vector(*v_mm)),
        *start_angle,
        *end_angle,
    ))
}
fn angular(
    u: Vector3,
    v: Vector3,
    x: Vector3,
    y: Vector3,
    a: f64,
    b: f64,
) -> Result<(f64, f64), GeometryError> {
    let radius = u.magnitude();
    if u.dot(x).hypot(u.dot(y)) <= RESIDUAL
        || u.cross(v).dot(x.cross(y)).abs() <= RESIDUAL * radius * radius
    {
        return Err(error(
            GeometryErrorCode::ParameterCorrespondenceUnavailable,
            "circle has no regular angular chart",
        ));
    }
    let phase = u.dot(y).atan2(u.dot(x));
    let direction = u.cross(v).dot(x.cross(y)).signum();
    Ok((phase + direction * a, phase + direction * b))
}

fn revolution(
    curve: &Curve3D,
    processor: &Processor<RevolutionSurface<Line<Point3>>, Matrix4>,
) -> Result<Option<Curve2D>, GeometryError> {
    let witness = match boundary(curve, 1.0) {
        Ok(w) => w,
        Err(c) if c.code == GeometryErrorCode::UnsupportedGeometry => return Ok(None),
        Err(c) => return Err(c),
    };
    revolution_witness(&witness, processor)
}
fn revolution_witness(
    witness: &BoundaryCurveMm,
    processor: &Processor<RevolutionSurface<Line<Point3>>, Matrix4>,
) -> Result<Option<Curve2D>, GeometryError> {
    let inv = inverse(processor.transform())?;
    let rev = processor.entity();
    let profile = rev.entity_curve();
    let axis = regular(rev.axis())?;
    let origin = rev.origin();
    let d0 = profile.0 - origin;
    let axial0 = d0.dot(axis);
    let radial0 = d0 - axial0 * axis;
    let x = regular(radial0)?;
    let y = axis.cross(x);
    let delta = profile.1 - profile.0;
    let axial_delta = delta.dot(axis);
    let radial_delta = delta.dot(x);
    if (delta - axial_delta * axis - radial_delta * x).magnitude() > RESIDUAL {
        return Ok(None);
    }
    let profile_parameter = |p: Point3| -> Result<f64, GeometryError> {
        let d = p - profile.0;
        if axial_delta.abs() > RESIDUAL {
            Ok(d.dot(axis) / axial_delta)
        } else if radial_delta.abs() > RESIDUAL {
            Ok(d.dot(x) / radial_delta)
        } else {
            Err(error(
                GeometryErrorCode::SingularGeometry,
                "collapsed revolution source profile",
            ))
        }
    };
    let swapped = !processor.orientation();
    if let Some((center, u, v, a, b)) = circle_local(witness, &inv) {
        let d = center - origin;
        if (d - d.dot(axis) * axis).magnitude() > RESIDUAL
            || u.dot(axis).abs() > RESIDUAL
            || v.dot(axis).abs() > RESIDUAL
        {
            return Ok(None);
        }
        let radius = u.magnitude();
        let projected = origin + d.dot(axis) * axis + radius * x;
        let t = profile_parameter(projected)?;
        if (profile.subs(t) - projected).magnitude() > RESIDUAL {
            return Err(invalid(
                "source rim disagrees with revolution profile coefficients",
            ));
        }
        let (a, b) = angular(u, v, x, y, a, b)?;
        return Ok(Some(line((t, a), (t, b), swapped)));
    }
    if let BoundaryCurveMm::Line { start_mm, end_mm } = witness {
        let a = inv.transform_point(point(*start_mm));
        let b = inv.transform_point(point(*end_mm));
        let da = a - origin;
        let db = b - origin;
        let ra = da - da.dot(axis) * axis;
        let rb = db - db.dot(axis) * axis;
        if ra.magnitude() <= RESIDUAL || rb.magnitude() <= RESIDUAL {
            return Ok(None);
        }
        if ra.cross(rb).magnitude() > RESIDUAL * ra.magnitude().max(rb.magnitude())
            || ra.dot(rb) <= 0.0
        {
            return Ok(None);
        }
        let angle = ra.dot(y).atan2(ra.dot(x));
        let rotate_back = Matrix3::from_axis_angle(axis, Rad(-angle));
        let pa = origin + rotate_back * da;
        let pb = origin + rotate_back * db;
        let ta = profile_parameter(pa)?;
        let tb = profile_parameter(pb)?;
        if (profile.subs(ta) - pa).magnitude() > RESIDUAL
            || (profile.subs(tb) - pb).magnitude() > RESIDUAL
        {
            return Err(invalid(
                "source generator disagrees with revolution profile coefficients",
            ));
        }
        return Ok(Some(line((ta, angle), (tb, angle), swapped)));
    }
    Ok(None)
}

fn sphere(
    curve: &Curve3D,
    processor: &monstertruck_io::step::load::step_geometry::SphericalSurface,
) -> Result<Option<Curve2D>, GeometryError> {
    let witness = match boundary(curve, 1.0) {
        Ok(w) => w,
        Err(c) if c.code == GeometryErrorCode::UnsupportedGeometry => return Ok(None),
        Err(c) => return Err(c),
    };
    sphere_witness(&witness, processor)
}
fn sphere_witness(
    witness: &BoundaryCurveMm,
    processor: &monstertruck_io::step::load::step_geometry::SphericalSurface,
) -> Result<Option<Curve2D>, GeometryError> {
    let inv = inverse(processor.transform())?;
    let Some((center, u, v, a, b)) = circle_local(witness, &inv) else {
        return Ok(None);
    };
    let sphere = processor.entity().0;
    let d = center - sphere.center();
    let radius = sphere.radius();
    let normal = regular(u.cross(v))?;
    let swapped = !processor.orientation();
    if normal.cross(Vector3::unit_z()).magnitude() <= 1e-10 && d.x.hypot(d.y) <= RESIDUAL {
        if (d.z.abs().hypot(u.magnitude()) - radius).abs() > RESIDUAL {
            return Err(invalid("latitude circle disagrees with source sphere"));
        }
        let latitude = (d.z / radius).asin();
        let (a, b) = angular(u, v, Vector3::unit_x(), Vector3::unit_y(), a, b)?;
        return Ok(Some(line((a, latitude), (b, latitude), swapped)));
    }
    if d.magnitude() > RESIDUAL
        || normal.z.abs() > 1e-10
        || (u.magnitude() - radius).abs() > RESIDUAL
    {
        return Ok(None);
    }
    let radial = if u.x.hypot(u.y) > RESIDUAL {
        Vector3::new(u.x, u.y, 0.0)
    } else {
        Vector3::new(v.x, v.y, 0.0)
    };
    let radial = regular(radial)?;
    let longitude = radial.y.atan2(radial.x);
    let phase = u.z.atan2(u.dot(radial));
    let direction = (u.dot(radial) * v.z - u.z * v.dot(radial)).signum();
    let mut la = phase + direction * a;
    let mut lb = phase + direction * b;
    let branch = ((la + lb) * 0.5 / TAU).round() * TAU;
    la -= branch;
    lb -= branch;
    if la.min(lb) < -PI / 2.0 || la.max(lb) > PI / 2.0 {
        // Endpoints may belong to the original analytic pole stratum. This is
        // only a proposal: coefficient tubes and the whole shared-vertex pole
        // completion still qualify both incident uses. An interior crossing
        // cannot be made regular by selecting another longitude.
        let shift = if (la + lb) * 0.5 >= 0.0 { PI } else { -PI };
        la -= shift;
        lb -= shift;
        if la.min(lb) < -PI / 2.0 || la.max(lb) > PI / 2.0 {
            return Err(error(
                GeometryErrorCode::SingularGeometry,
                "meridian trim crosses an unresolved spherical pole chart",
            ));
        }
        let longitude = longitude + PI;
        return Ok(Some(line((longitude, -la), (longitude, -lb), swapped)));
    }
    Ok(Some(line((longitude, la), (longitude, lb), swapped)))
}

fn torus(
    curve: &Curve3D,
    processor: &monstertruck_io::step::load::step_geometry::ToroidalSurface,
) -> Result<Option<Curve2D>, GeometryError> {
    let witness = match boundary(curve, 1.0) {
        Ok(w) => w,
        Err(c) if c.code == GeometryErrorCode::UnsupportedGeometry => return Ok(None),
        Err(c) => return Err(c),
    };
    torus_witness(&witness, processor)
}
fn torus_witness(
    witness: &BoundaryCurveMm,
    processor: &monstertruck_io::step::load::step_geometry::ToroidalSurface,
) -> Result<Option<Curve2D>, GeometryError> {
    let inv = inverse(processor.transform())?;
    let Some((center, u, v, a, b)) = circle_local(witness, &inv) else {
        return Ok(None);
    };
    let torus = processor.entity();
    let d = center - torus.center();
    if torus.large_radius() <= torus.small_radius() {
        return Err(error(
            GeometryErrorCode::SingularGeometry,
            "source torus has no regular ring chart",
        ));
    }
    let normal = regular(u.cross(v))?;
    let swapped = !processor.orientation();
    if normal.cross(Vector3::unit_z()).magnitude() <= 1e-10 && d.x.hypot(d.y) <= RESIDUAL {
        let radius = u.magnitude();
        if ((radius - torus.large_radius()).hypot(d.z) - torus.small_radius()).abs() > RESIDUAL {
            return Err(invalid("latitude circle disagrees with source torus"));
        }
        let minor = d.z.atan2(radius - torus.large_radius());
        let (a, b) = angular(u, v, Vector3::unit_x(), Vector3::unit_y(), a, b)?;
        return Ok(Some(line((a, minor), (b, minor), swapped)));
    }
    if d.z.abs() > RESIDUAL
        || (d.x.hypot(d.y) - torus.large_radius()).abs() > RESIDUAL
        || (u.magnitude() - torus.small_radius()).abs() > RESIDUAL
        || normal.z.abs() > 1e-10
    {
        return Ok(None);
    }
    let radial = regular(Vector3::new(d.x, d.y, 0.0))?;
    if normal.dot(radial).abs() > 1e-10 {
        return Ok(None);
    }
    let longitude = radial.y.atan2(radial.x);
    let (a, b) = angular(u, v, radial, Vector3::unit_z(), a, b)?;
    Ok(Some(line((longitude, a), (longitude, b), swapped)))
}

fn extrusion(
    curve: &Curve3D,
    source: &monstertruck_io::step::load::step_geometry::StepExtrusionSurface,
) -> Result<Option<Curve2D>, GeometryError> {
    let direction = source.extruding_vector();
    let norm = direction.magnitude2();
    if !norm.is_finite() || norm <= 1e-20 {
        return Err(error(
            GeometryErrorCode::SingularGeometry,
            "collapsed source extrusion direction",
        ));
    }
    let profile = source.entity_curve().as_ref();
    let translated = match (profile, curve) {
        (Curve3D::BsplineCurve(a), Curve3D::BsplineCurve(b))
            if a.knot_vector() == b.knot_vector()
                && a.control_points().len() == b.control_points().len() =>
        {
            let delta = b.control_points()[0] - a.control_points()[0];
            let offset = delta.dot(direction) / norm;
            a.control_points()
                .iter()
                .zip(b.control_points())
                .all(|(a, b)| (*b - *a - offset * direction).magnitude() <= RESIDUAL)
                .then_some(offset)
        }
        (Curve3D::Line(a), Curve3D::Line(b)) => {
            let offset = (b.0 - a.0).dot(direction) / norm;
            ((b.0 - a.0 - offset * direction).magnitude() <= RESIDUAL
                && (b.1 - a.1 - offset * direction).magnitude() <= RESIDUAL)
                .then_some(offset)
        }
        (Curve3D::NurbsCurve(a), Curve3D::NurbsCurve(b))
            if a.knot_vector() == b.knot_vector()
                && a.control_points().len() == b.control_points().len() =>
        {
            let first_a = a.control_points()[0];
            let first_b = b.control_points()[0];
            if first_a.w == 0.0 || first_b.w != first_a.w {
                return Ok(None);
            }
            let offset = (first_b.truncate() / first_b.w - first_a.truncate() / first_a.w)
                .dot(direction)
                / norm;
            a.control_points()
                .iter()
                .zip(b.control_points())
                .all(|(a, b)| {
                    a.w == b.w
                        && (b.truncate() - a.truncate() - offset * a.w * direction).magnitude()
                            <= RESIDUAL * a.w.abs()
                })
                .then_some(offset)
        }
        _ => None,
    };
    let (a, b) = profile.range_tuple();
    if let Some(offset) = translated {
        return Ok(Some(line((a, offset), (b, offset), false)));
    }
    let Curve3D::Line(edge) = curve else {
        return Ok(None);
    };
    for u in [a, b] {
        let base = profile.subs(u);
        let va = (edge.0 - base).dot(direction) / norm;
        let vb = (edge.1 - base).dot(direction) / norm;
        if (edge.0 - base - va * direction).magnitude() <= RESIDUAL
            && (edge.1 - base - vb * direction).magnitude() <= RESIDUAL
        {
            return Ok(Some(line((u, va), (u, vb), false)));
        }
    }
    Ok(None)
}
/// An affine plane inverse supplies a proposal, not exact carrier membership.
/// Exported LINE coefficients can have a small nonzero normal residual, which
/// correctly prevents the native *exact* projection receipt. The caller must
/// still prove the complete original expression's physical correspondence.
fn planar_line_witness(
    witness: &BoundaryCurveMm,
    plane: &monstertruck_modeling::Plane,
) -> Option<Curve2D> {
    let BoundaryCurveMm::Line { start_mm, end_mm } = witness else {
        return None;
    };
    let u = plane.axis_u();
    let v = plane.axis_v();
    let inverse = Matrix3::from_cols(u, v, u.cross(v)).invert()?;
    let a = inverse * (point(*start_mm) - plane.origin());
    let b = inverse * (point(*end_mm) - plane.origin());
    if [a.x, a.y, b.x, b.y]
        .into_iter()
        .any(|coefficient| !coefficient.is_finite())
    {
        return None;
    }
    Some(line((a.x, a.y), (b.x, b.y), false))
}

pub(super) fn source_chart(
    curve: &Curve3D,
    surface: &Surface,
) -> Result<Option<Curve2D>, GeometryError> {
    // SURFACE_CURVE evaluates its unchanged 3D leader. Other associations may
    // have no narrow analytic metadata (for example an affine NURBS floor);
    // they cannot prevent a proposal on this particular original carrier.
    // The caller still proves the complete original expression, including its
    // wrappers, against the selected carrier before this proposal is usable.
    let mut curve = curve;
    for depth in 0..=32 {
        match curve {
            Curve3D::SurfaceCurve(source) if depth < 32 => curve = source.leader(),
            Curve3D::SurfaceCurve(_) => {
                return Err(error(
                    GeometryErrorCode::ResourceLimit,
                    "source chart association nesting exceeds source limit",
                ));
            }
            _ => break,
        }
    }
    match surface {
        Surface::ElementarySurface(ElementarySurface::Plane(plane)) => {
            let witness = match boundary(curve, 1.0) {
                Ok(witness) => witness,
                Err(cause) if cause.code == GeometryErrorCode::UnsupportedGeometry => {
                    return Ok(None);
                }
                Err(cause) => return Err(cause),
            };
            Ok(planar_line_witness(&witness, plane))
        }
        Surface::ElementarySurface(
            ElementarySurface::CylindricalSurface(s) | ElementarySurface::ConicalSurface(s),
        ) => revolution(curve, s),
        Surface::ElementarySurface(ElementarySurface::Sphere(s)) => sphere(curve, s),
        Surface::ElementarySurface(ElementarySurface::ToroidalSurface(s)) => torus(curve, s),
        Surface::SweepSurface(SweepSurface::RevolutionSurface(s)) => {
            let Curve3D::Line(profile) = s.entity().entity_curve().as_ref() else {
                return Ok(None);
            };
            let processor =
                s.map_ref(|r| RevolutionSurface::by_revolution(*profile, r.origin(), r.axis()));
            revolution(curve, &processor)
        }
        Surface::SweepSurface(SweepSurface::ExtrusionSurface(s)) => extrusion(curve, s),
        _ => Ok(None),
    }
}

/// A coefficient-derived guide proposes an ordinary UV expression. Callers
/// must prove it against the retained actual implicit branch before admission.
pub(super) fn guide_chart(
    witness: &BoundaryCurveMm,
    surface: &Surface,
) -> Result<Option<Curve2D>, GeometryError> {
    match surface {
        Surface::ElementarySurface(ElementarySurface::Plane(plane)) => {
            Ok(planar_line_witness(witness, plane))
        }
        Surface::ElementarySurface(
            ElementarySurface::CylindricalSurface(s) | ElementarySurface::ConicalSurface(s),
        ) => revolution_witness(witness, s),
        Surface::ElementarySurface(ElementarySurface::Sphere(s)) => sphere_witness(witness, s),
        Surface::ElementarySurface(ElementarySurface::ToroidalSurface(s)) => {
            torus_witness(witness, s)
        }
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spherical_meridian_proposals_include_only_original_pole_endpoints() {
        let radius = 2.;
        let mut carrier = Processor::new(monstertruck_io::step::load::step_geometry::Sphere(
            monstertruck_modeling::Sphere::new(Point3::origin(), radius),
        ));
        let arc = |start_angle, end_angle| BoundaryCurveMm::CircleArc {
            center_mm: [0.; 3],
            u_mm: [radius, 0., 0.],
            v_mm: [0., 0., radius],
            start_angle,
            end_angle,
        };
        let half_pi = PI / 2.;
        for inverted in [false, true] {
            if inverted {
                carrier.invert();
            }
            for (a, b) in [(-half_pi, 0.), (0., half_pi), (half_pi, 0.), (0., -half_pi)] {
                let proposal = sphere_witness(&arc(a, b), &carrier).unwrap().unwrap();
                let start = if inverted {
                    Point2::new(a, 0.)
                } else {
                    Point2::new(0., a)
                };
                let end = if inverted {
                    Point2::new(b, 0.)
                } else {
                    Point2::new(0., b)
                };
                assert_eq!(proposal.subs(0.), start);
                assert_eq!(proposal.subs(1.), end);
            }
            for (a, b) in [(0., PI), (-half_pi - 1e-4, 0.), (0., half_pi + 1e-4)] {
                assert_eq!(
                    sphere_witness(&arc(a, b), &carrier).unwrap_err().code,
                    GeometryErrorCode::SingularGeometry,
                    "an interior or outside pole crossing is not an endpoint stratum",
                );
            }
        }
    }
}
