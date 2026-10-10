// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

mod ast;
#[cfg(test)]
mod tests;
mod units;

use crate::{
    check_cancel,
    kernel::{BoundaryCurveMm, Definition},
};
use ast::*;
use monstertruck_io::step::load::{
    Table,
    step_geometry::{
        Conic2D, Conic3D, Curve2D, Curve3D, ElementarySurface, StepParameterCurve,
        Surface as StepSurface, SurfaceCurveAssociatedGeometry,
    },
};
use monstertruck_modeling::{
    Curve, Surface, base::*, bounding::certified_solid_bounding_box, builder,
};
use sha2::{Digest, Sha256};
use spiling_contracts::geometry::{
    AabbMm, DefinitionId, FaceCarrier, FaceId, FaceInfo, GeometryError, GeometryErrorCode,
    MAX_NATIVE_FACES, MAX_SOURCE_BYTES, SourceHash, SourceProvenance,
};
use std::{collections::BTreeSet, sync::atomic::AtomicBool};
use step_p21::ast::Parameter;

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
fn source_carrier(
    entities: &Entities<'_>,
    face: u64,
    scale: f64,
) -> Result<(String, FaceCarrier), GeometryError> {
    let rs = entities.records(face)?;
    let r = rs
        .iter()
        .find(|r| matches!(r.name.as_str(), "ADVANCED_FACE" | "FACE_SURFACE"))
        .ok_or_else(|| invalid("shell face does not resolve to surface face"))?;
    let surface = reference(at(r, 2)?)?;
    let rs = entities.records(surface)?;
    let r = rs.first().ok_or_else(|| invalid("missing face carrier"))?;
    let (origin, axis) = placement(entities, reference(at(r, 1)?)?, scale)?;
    let carrier = match r.name.as_str() {
        "PLANE" => FaceCarrier::Plane {
            origin_mm: array(origin),
            normal: vec_array(axis),
        },
        "CYLINDRICAL_SURFACE" => FaceCarrier::Cylinder {
            axis_origin_mm: array(origin),
            axis_direction: vec_array(axis),
            radius_mm: number(at(r, 2)?)? * scale,
        },
        _ => {
            return Err(unsupported(
                "only planar/cylindrical face carriers are admitted",
            ));
        }
    };
    carrier.validate()?;
    let kind = entities
        .records(face)?
        .iter()
        .find(|r| matches!(r.name.as_str(), "ADVANCED_FACE" | "FACE_SURFACE"))
        .unwrap()
        .name
        .clone();
    Ok((kind, carrier))
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
    let surface = c.surface().as_ref();
    match (surface, c.curve().as_ref()) {
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
fn native_carrier(surface: &StepSurface, scale: f64) -> Result<FaceCarrier, GeometryError> {
    let carrier = match surface {
        StepSurface::ElementarySurface(ElementarySurface::Plane(plane)) => FaceCarrier::Plane {
            origin_mm: array(Point3::from_vec(plane.origin().to_vec() * scale)),
            normal: vec_array(plane.normal()),
        },
        StepSurface::ElementarySurface(ElementarySurface::CylindricalSurface(cylinder)) => {
            let revolution = cylinder.entity();
            let matrix = cylinder.transform();
            let origin = matrix.transform_point(revolution.origin());
            let axis = unit_vector(vec_array(matrix.transform_vector(revolution.axis())))?;
            let point = matrix.transform_point(revolution.entity_curve().0);
            let delta = point - origin;
            FaceCarrier::Cylinder {
                axis_origin_mm: array(Point3::from_vec(origin.to_vec() * scale)),
                axis_direction: vec_array(axis),
                radius_mm: (delta - delta.dot(axis) * axis).magnitude() * scale,
            }
        }
        _ => {
            return Err(unsupported(
                "intersection boundary has an unsupported carrier",
            ));
        }
    };
    carrier.validate()?;
    Ok(carrier)
}

/// An admitted analytic leader must lie on both native intersection carriers.
/// Checking analytic coefficients certifies the whole bounded arc, not samples.
fn certify_boundary(witness: &BoundaryCurveMm, carrier: &FaceCarrier) -> Result<(), GeometryError> {
    let vector = |p: [f64; 3]| Vector3::new(p[0], p[1], p[2]);
    let tolerance = 1e-7;
    let valid = match carrier {
        FaceCarrier::Plane { origin_mm, normal } => {
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
            }
        }
        FaceCarrier::Cylinder {
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
                    SurfaceCurveAssociatedGeometry::ParameterCurve(curve) => {
                        curve.surface().as_ref()
                    }
                };
                certify_boundary(&witness, &native_carrier(surface, scale)?)?;
            }
            Ok(witness)
        }
        Curve3D::IntersectionCurve(c) => {
            let witness = boundary(c.leader(), scale)?;
            certify_boundary(&witness, &native_carrier(c.surface0(), scale)?)?;
            certify_boundary(&witness, &native_carrier(c.surface1(), scale)?)?;
            Ok(witness)
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

/// The loader wraps writer-declared exact lines/circles in intersection objects.
/// `boundary` proved their leaders lie on every carrier before extraction.
/// Keep that exact analytic representation so public certified bounds apply;
/// this removes a representation wrapper, not geometry or source identity.
fn modeling_curve(curve: &Curve3D) -> Option<Curve> {
    match curve {
        Curve3D::IntersectionCurve(curve) => modeling_curve(curve.leader()),
        Curve3D::SurfaceCurve(curve) => modeling_curve(curve.leader()),
        curve => Curve::try_from(curve).ok(),
    }
}

pub(crate) fn import_step(
    bytes: &[u8],
    cancel: &AtomicBool,
    face_limit: u32,
) -> Result<Definition, GeometryError> {
    check_cancel(cancel)?;
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
        let (source_entity_kind, carrier) = source_carrier(&entities, resolved, scale)?;
        faces.push(FaceInfo {
            face_id: FaceId::from_step_entity(id)?,
            surface_face_entity: resolved.to_string(),
            source_entity_kind,
            orientation: true,
            carrier,
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
    let (compressed, reports) = table
        .to_compressed_solid_reported(step_solid)
        .map_err(|_| invalid("STEP solid conversion failed"))?;
    check_cancel(cancel)?;
    if compressed.boundaries.len() != 1 || reports.len() != 1 || !reports[0].is_lossless() {
        return Err(invalid("reachable STEP conversion loss or extra shell"));
    }
    let shell = &compressed.boundaries[0];
    if shell.faces.len() != faces.len() {
        return Err(kernel("positional source-face conversion mismatch"));
    }
    let mut boundary_curves = Vec::with_capacity(shell.edges.len() + shell.vertices.len());
    for edge in &shell.edges {
        check_cancel(cancel)?;
        boundary_curves.push(boundary(&edge.curve, scale)?);
    }
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
    // Retain the source positional surface/flag for assertions across extraction.
    let slots = shell
        .faces
        .iter()
        .map(|f| (f.surface.clone(), f.orientation))
        .collect::<Vec<_>>();
    check_cancel(cancel)?;
    let native = monstertruck_topology::Solid::extract(compressed)
        .map_err(|_| invalid("solid is not a connected closed manifold"))?;
    check_cancel(cancel)?;
    for shell in native.boundaries() {
        shell
            .check_solid_boundary()
            .map_err(|_| invalid("invalid native solid boundary"))?;
    }
    if native.boundaries()[0].len() != faces.len() {
        return Err(kernel("face extraction changed positional count"));
    }
    for (face, (surface, orientation)) in native.boundaries()[0].iter().zip(&slots) {
        if face.surface() != *surface || face.orientation() != *orientation {
            return Err(kernel("face extraction changed positional carrier"));
        }
    }
    check_cancel(cancel)?;
    let mapped = native
        .try_mapped(
            |p| Some(*p),
            modeling_curve,
            |surface| Surface::try_from(surface).ok(),
        )
        .ok_or_else(|| unsupported("native STEP-to-modeling conversion failed"))?;
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
    if !solid.is_geometric_consistent() {
        return Err(invalid(
            "normalized native solid is geometrically inconsistent",
        ));
    }
    for ((face, info), (source_surface, source_orientation)) in
        solid.boundaries()[0].iter().zip(&mut faces).zip(&slots)
    {
        check_cancel(cancel)?;
        // Verify both source and modeled carrier direction. Cylinder orientation is relative
        // to outward radial normal, not the internal revolution parameter handedness.
        let source_point = source_surface.subs(0.37, 0.73);
        let point = Point3::from_vec(source_point.to_vec() * scale);
        let surface = face.surface();
        let modeled_point = surface.subs(0.37, 0.73);
        if (modeled_point - point).magnitude() > 1e-7 {
            return Err(kernel(
                "modeling conversion changed positional carrier parameters",
            ));
        }
        let normal = surface.normal(0.37, 0.73) * if face.orientation() { 1.0 } else { -1.0 };
        let source_normal =
            source_surface.normal(0.37, 0.73) * if *source_orientation { 1.0 } else { -1.0 };
        if !normal.magnitude2().is_finite()
            || normal.magnitude2() == 0.0
            || normal.normalize().dot(source_normal.normalize()) < 1.0 - 1e-10
        {
            return Err(kernel(
                "modeling conversion changed native face orientation",
            ));
        }
        let canonical = match &info.carrier {
            FaceCarrier::Plane { origin_mm, normal } => {
                let origin = Point3::new(origin_mm[0], origin_mm[1], origin_mm[2]);
                let n = unit_vector(*normal)?;
                if (point - origin).dot(n).abs() > 1e-7 {
                    return Err(kernel("source plane carrier mismatch"));
                }
                n
            }
            FaceCarrier::Cylinder {
                axis_origin_mm,
                axis_direction,
                radius_mm,
            } => {
                let origin = Point3::new(axis_origin_mm[0], axis_origin_mm[1], axis_origin_mm[2]);
                let axis = unit_vector(*axis_direction)?;
                let d = point - origin;
                let radial = d - d.dot(axis) * axis;
                if (radial.magnitude() - radius_mm).abs() > 1e-7 {
                    return Err(kernel("source cylinder carrier mismatch"));
                }
                radial.normalize()
            }
        };
        let dot = normal.normalize().dot(canonical);
        if (dot.abs() - 1.0).abs() > 1e-10 {
            return Err(kernel(
                "native face normal is not analytical carrier normal",
            ));
        }
        info.orientation = dot > 0.0;
        info.validate()?;
    }
    check_cancel(cancel)?;
    let bounds = certified_solid_bounding_box(&solid)
        .ok_or_else(|| invalid("native solid has no certified bounds"))?;
    let bounds = AabbMm {
        min: array(bounds.min()),
        max: array(bounds.max()),
    };
    bounds.validate()?;
    check_cancel(cancel)?;
    let digest: [u8; 32] = Sha256::digest(bytes).into();
    Ok(Definition {
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
    })
}
