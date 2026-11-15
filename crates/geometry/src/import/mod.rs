// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

mod source;

mod ast;
mod prepared;
pub mod structure;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod topology_tests;
mod units;
pub use prepared::PreparedStepSource;

use crate::{check_cancel, kernel::Definition};
use ast::*;
use sha2::{Digest, Sha256};
use solidcore::math::{Point3, Vec3};
use spiling_contracts::geometry::{
    AnalyticSurfaceDescription, DefinitionId, FaceDomainSummary, FaceGeometryInfo, FaceId,
    FaceInfo, GeometryError, GeometryErrorCode, MAX_NATIVE_FACES, MAX_SOURCE_BYTES, PlaneFrameMm,
    SourceHash, SourceProvenance, SourceSolidId, StepEntityId, SurfaceFamily, TorusKind,
};
use std::{collections::BTreeSet, sync::atomic::AtomicBool};
use step_p21::ast::Parameter;

pub(crate) use crate::compute::MAX_QUERY_PROOF_BYTES;
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
fn vec_array(p: Vec3) -> [f64; 3] {
    [p.x, p.y, p.z]
}
fn unit_vector(v: [f64; 3]) -> Result<Vec3, GeometryError> {
    let scale = v.into_iter().map(f64::abs).fold(0.0, f64::max);
    if !v.into_iter().all(f64::is_finite) || scale == 0.0 {
        return Err(invalid("invalid native axis"));
    }
    let scaled = Vec3::new(v[0] / scale, v[1] / scale, v[2] / scale);
    Ok(scaled / scaled.length())
}
/// Every original LINE edge curve passes through its edge's vertices within the import
/// tolerance. The reader reads a straight edge whose vertices lie off its LINE as the segment
/// between them, however far; that is a different edge from the one the file declares, so past
/// the tolerance the import refuses rather than invent it.
fn lines_through_their_vertices(
    original: &crate::kernel::source::OriginalSourceTopology,
    tolerance: f64,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    use solidcore_step::geom::Curve;
    for coedge in &original.coedges {
        check_cancel(cancel)?;
        let mut curve = coedge.spatial_curve.as_ref();
        for _ in 0..32 {
            curve = match curve {
                Curve::SurfaceCurve { curve_3d, .. } => curve_3d,
                Curve::Trimmed { basis, .. } => basis,
                _ => break,
            };
        }
        let Curve::Line {
            origin, direction, ..
        } = curve
        else {
            continue;
        };
        let length = direction.iter().map(|d| d * d).sum::<f64>().sqrt();
        if !(length.is_finite() && length > 0.0) {
            return Err(invalid("STEP LINE without a direction"));
        }
        let d = direction.map(|c| c / length);
        for p in coedge.vertex_points_mm {
            let w = [p[0] - origin[0], p[1] - origin[1], p[2] - origin[2]];
            let along = w[0] * d[0] + w[1] * d[1] + w[2] * d[2];
            let off = [
                w[0] - along * d[0],
                w[1] - along * d[1],
                w[2] - along * d[2],
            ];
            let gap = off.iter().map(|c| c * c).sum::<f64>().sqrt();
            if gap.is_nan() || gap > tolerance {
                return Err(invalid(&format!(
                    "STEP edge #{}: a vertex lies {gap:.6} mm off its LINE, past the {tolerance} mm import tolerance",
                    coedge.edge_curve_id.0
                )));
            }
        }
    }
    Ok(())
}

/// Each note of the STEP reader (a construct it read another way: a twin edge made one, a seam
/// added, a cut laid again, ...) as a reported finding with its count, never silent.
fn reader_findings(notes: &[String]) -> Vec<spiling_contracts::geometry::ImportFinding> {
    use spiling_contracts::geometry::{
        ImportFinding, ImportFindingCode, ImportFindingKind, MAX_DISPLAY_LABEL_BYTES,
    };
    let limit = MAX_DISPLAY_LABEL_BYTES as usize;
    notes
        .iter()
        .map(|note| {
            let count = note
                .split_whitespace()
                .next()
                .and_then(|n| n.parse::<u32>().ok())
                .filter(|n| *n > 0)
                .unwrap_or(1);
            let mut end = note.len().min(limit);
            while !note.is_char_boundary(end) {
                end -= 1;
            }
            ImportFinding {
                kind: ImportFindingKind::Changed,
                code: ImportFindingCode::SourceReading,
                count,
                label: "STEP reading".into(),
                detail: note[..end].to_string(),
                bounds_mm: None,
                displacement_mm: None,
            }
        })
        .collect()
}

/// The periods of a native face surface that closes on itself across a direction (its two
/// boundary lines one, sampled): the direction's natural width; `None` where it does not close.
fn surface_periods(
    k: &solidcore::kernel::Kernel,
    surface: &solidcore::surface::surface::Surface,
) -> Result<[Option<f64>; 2], solidcore::SolidError> {
    let natural = surface.natural_uv_domain_with_nurb(k)?;
    let closes = |along_u: bool| -> bool {
        let (length, other) = if along_u {
            (
                natural.u_max() - natural.u_min(),
                [natural.v_min(), natural.v_max()],
            )
        } else {
            (
                natural.v_max() - natural.v_min(),
                [natural.u_min(), natural.u_max()],
            )
        };
        if !(length.is_finite() && length > 0.0 && other[0].is_finite() && other[1].is_finite()) {
            return false;
        }
        (0..=4).all(|i| {
            let w = other[0] + (other[1] - other[0]) * f64::from(i) / 4.0;
            let (a, z) = if along_u {
                (
                    solidcore::math::Point2::new(natural.u_min(), w),
                    solidcore::math::Point2::new(natural.u_max(), w),
                )
            } else {
                (
                    solidcore::math::Point2::new(w, natural.v_min()),
                    solidcore::math::Point2::new(w, natural.v_max()),
                )
            };
            match (surface.evaluate_point(k, a), surface.evaluate_point(k, z)) {
                (Ok(p), Ok(q)) => (p - q).length() <= 1e-9 * (1.0 + p.length().max(q.length())),
                _ => false,
            }
        })
    };
    Ok([
        closes(true).then(|| natural.u_max() - natural.u_min()),
        closes(false).then(|| natural.v_max() - natural.v_min()),
    ])
}

/// The reader's refusal: a construct it does not read yet is unsupported, anything else is
/// an invalid source.
fn read_refusal(message: &str) -> GeometryError {
    if message.contains("not supported yet") {
        unsupported(message)
    } else {
        invalid(message)
    }
}

fn placement(
    entities: &Entities<'_>,
    id: u64,
    scale: f64,
) -> Result<(Point3, Vec3), GeometryError> {
    let r = entities.record(id, "AXIS2_PLACEMENT_3D")?;
    let point = entities.record(reference(at(r, 1)?)?, "CARTESIAN_POINT")?;
    let [x, y, z] = vector(at(point, 1)?)?;
    let origin = Point3::new(x * scale, y * scale, z * scale);
    let z = if matches!(at(r, 2)?, Parameter::NotProvided) {
        Vec3::new(0.0, 0.0, 1.0)
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
            Vec3::new(1.0, 0.0, 0.0)
        } else {
            Vec3::new(0.0, 1.0, 0.0)
        }
    } else {
        unit_vector(vector(at(
            entities.record(reference(at(r, 3)?)?, "DIRECTION")?,
            1,
        )?)?)?
    };
    let x = x - x.dot(z) * z;
    if x.length_squared() <= 1e-20 {
        return Err(invalid(
            "source placement has parallel axis and reference direction",
        ));
    }
    let x = unit_vector(vec_array(x))?;
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
    radians_per_angle_unit: f64,
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
                | "DEGENERATE_TOROIDAL_SURFACE"
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
                semi_angle_radians: number(at(surface, 3)?)? * radians_per_angle_unit,
            },
            "SPHERICAL_SURFACE" => AnalyticSurfaceDescription::Sphere {
                frame,
                radius_mm: number(at(surface, 2)?)? * scale,
            },
            "TOROIDAL_SURFACE" | "DEGENERATE_TOROIDAL_SURFACE" => {
                let major = number(at(surface, 2)?)? * scale;
                let minor = number(at(surface, 3)?)? * scale;
                // OCCT's Geom_ToroidalSurface admits any positive radii; horn
                // (major == minor) and spindle (major < minor) tori are
                // ordinary CAD fillet carriers whose trimmed patch is what matters.
                // The STEP reader refuses faces on a spindle's inner (lemon)
                // sheet, whose side STEP and kernel normals disagree on.
                if surface.name == "DEGENERATE_TOROIDAL_SURFACE"
                    && !source::source_bool(at(surface, 4)?)?
                {
                    return Err(unsupported(
                        "degenerate torus inner (lemon) sheet has no admitted face side",
                    ));
                }
                let torus_kind = if major > minor {
                    TorusKind::RegularRing
                } else if major == minor {
                    TorusKind::Horn
                } else {
                    TorusKind::Spindle
                };
                AnalyticSurfaceDescription::Torus {
                    frame,
                    major_radius_mm: major,
                    minor_radius_mm: minor,
                    torus_kind,
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
fn resolve_face(entities: &Entities<'_>, canonical: u64) -> Result<(u64, bool), GeometryError> {
    let mut id = canonical;
    let mut forward = true;
    let mut seen = BTreeSet::new();
    loop {
        if !seen.insert(id) {
            return Err(invalid("recursive ORIENTED_FACE"));
        }
        let records = entities.records(id)?;
        if let Some(face) = records
            .iter()
            .copied()
            .find(|record| matches!(record.name.as_str(), "ADVANCED_FACE" | "FACE_SURFACE"))
        {
            return Ok((id, forward == source::source_bool(at(face, 3)?)?));
        }
        let oriented = entities.record(id, "ORIENTED_FACE")?;
        forward = forward == source::source_bool(at(oriented, 3)?)?;
        id = reference(at(oriented, 2)?)?;
    }
}
/// One closed shell of a solid: the CLOSED_SHELL listing its faces and
/// whether the solid uses it reversed (ORIENTED_CLOSED_SHELL `.F.`).
struct SolidShell {
    faces: u64,
    reversed: bool,
}

/// The shells bounding a solid item, outer shell first: a MANIFOLD_SOLID_BREP's
/// one shell, or a BREP_WITH_VOIDS's outer shell and then each void shell in
/// declared order (ISO 10303-42). Material is inside the outer shell and
/// outside every void; each shell keeps its declared orientation.
fn solid_shells(entities: &Entities<'_>, solid: u64) -> Result<Vec<SolidShell>, GeometryError> {
    let shell = |id: u64| -> Result<SolidShell, GeometryError> {
        let records = entities.records(id)?;
        if records.iter().any(|r| r.name == "CLOSED_SHELL") {
            return Ok(SolidShell {
                faces: id,
                reversed: false,
            });
        }
        let oriented = entities.record(id, "ORIENTED_CLOSED_SHELL")?;
        let faces = reference(at(oriented, 2)?)?;
        entities
            .record(faces, "CLOSED_SHELL")
            .map_err(|_| unsupported("oriented shell element is not a CLOSED_SHELL"))?;
        Ok(SolidShell {
            faces,
            reversed: !source::source_bool(at(oriented, 3)?)?,
        })
    };
    let records = entities.records(solid)?;
    if let Some(root) = records.iter().find(|r| r.name == "BREP_WITH_VOIDS") {
        let mut shells = vec![shell(reference(at(root, 1)?)?)?];
        let voids = list(at(root, 2)?)?;
        if voids.is_empty() {
            return Err(invalid("BREP_WITH_VOIDS declares no void shell"));
        }
        for void in voids {
            let void = reference(void)?;
            entities
                .record(void, "ORIENTED_CLOSED_SHELL")
                .map_err(|_| invalid("BREP_WITH_VOIDS void is not an oriented closed shell"))?;
            shells.push(shell(void)?);
        }
        let mut seen = BTreeSet::new();
        if !shells.iter().all(|s| seen.insert(s.faces)) {
            return Err(invalid("solid uses one closed shell twice"));
        }
        return Ok(shells);
    }
    let root = entities.record(solid, "MANIFOLD_SOLID_BREP")?;
    Ok(vec![shell(reference(at(root, 1)?)?)?])
}

/// The STEP solid items Spiling imports: MANIFOLD_SOLID_BREP and its
/// BREP_WITH_VOIDS subtype, in entity order.
pub(super) const SOLID_ITEMS: [&str; 2] = ["MANIFOLD_SOLID_BREP", "BREP_WITH_VOIDS"];

pub(crate) fn import_step(
    bytes: &[u8],
    selected: Option<&SourceSolidId>,
    cancel: &AtomicBool,
    face_limit: u32,
    query_proof_byte_limit: usize,
) -> Result<Definition, GeometryError> {
    check_cancel(cancel)?;
    validate_face_limit(face_limit)?;
    PreparedStepSource::new(bytes, query_proof_byte_limit, cancel)?.read_solid(
        selected,
        cancel,
        face_limit,
        query_proof_byte_limit,
    )
}

fn validate_face_limit(face_limit: u32) -> Result<(), GeometryError> {
    if face_limit == 0 {
        return Err(error(
            GeometryErrorCode::ResourceLimit,
            "remaining native-face capacity exhausted",
        ));
    }
    Ok(())
}

fn import_prepared(
    prepared: &PreparedStepSource<'_>,
    selected: Option<&SourceSolidId>,
    cancel: &AtomicBool,
    face_limit: u32,
    query_proof_byte_limit: usize,
) -> Result<Definition, GeometryError> {
    check_cancel(cancel)?;
    validate_face_limit(face_limit)?;
    let bytes = prepared.bytes;
    let text = std::str::from_utf8(bytes).map_err(|_| invalid("STEP source is not UTF-8"))?;
    let entities = Entities::new(&prepared.data, cancel)?;
    // Product structure must be coherent even when one solid is imported; its
    // placements belong to the caller's occurrences, not to this definition.
    let bodies = &prepared.bodies;
    let solid_id = match selected {
        None if bodies.len() == 1 => bodies[0].solid.step_entity(),
        None if bodies.is_empty() => {
            return Err(unsupported("source contains no placed MANIFOLD_SOLID_BREP"));
        }
        None => {
            return Err(unsupported(
                "source places several solids; import each one by its solid ID",
            ));
        }
        Some(selected) => {
            if bodies.len() < 2 || !bodies.iter().any(|body| &body.solid == selected) {
                return Err(invalid(
                    "solid selector must name one of several placed source solids",
                ));
            }
            selected.step_entity()
        }
    };
    let units::ContextUnits {
        length: source_unit,
        radians_per_angle_unit,
        uncertainty_mm,
    } = units::resolve(&entities, solid_id, cancel)?;
    let scale = source_unit.scale_to_mm();
    let shells = solid_shells(&entities, solid_id)?;
    entities.validate_geometry(solid_id, cancel)?;
    let mut canonical = Vec::new();
    for shell in &shells {
        for face in list(at(entities.record(shell.faces, "CLOSED_SHELL")?, 1)?)? {
            canonical.push((reference(face)?, shell.reversed));
        }
    }
    if canonical.len() > face_limit as usize {
        return Err(error(
            GeometryErrorCode::ResourceLimit,
            "native face cap exceeded",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut faces = Vec::with_capacity(canonical.len());
    for (id, reversed) in canonical {
        check_cancel(cancel)?;
        if !seen.insert(id) {
            return Err(invalid("duplicate canonical shell face use"));
        }
        let (resolved, orientation) = resolve_face(&entities, id)?;
        // A reversed (ORIENTED_CLOSED_SHELL .F.) shell turns every face use.
        let orientation = orientation != reversed;
        let (source_entity_kind, source_surface_entity_kind, geometry) =
            source_geometry(&entities, resolved, scale, radians_per_angle_unit)?;
        faces.push(FaceInfo {
            face_id: FaceId::from_step_entity(id)?,
            surface_face_entity: resolved.to_string(),
            source_entity_kind,
            source_surface_entity_kind,
            orientation,
            geometry,
        });
    }
    check_cancel(cancel)?;
    let ex = solidcore_step::p21::parse(text)
        .map_err(|e| invalid(&format!("invalid ISO-10303-21 exchange: {e}")))?;
    check_cancel(cancel)?;
    // The exact single root was validated above. Read it once, directly in
    // authoritative mm units, rather than building an unscaled BREP first.
    let mut context = solidcore_step::units::Context::default();
    context.units.len = scale;
    context.units.ang = radians_per_angle_unit;
    context.uncertainty = uncertainty_mm;
    let import_tolerance = solidcore_step::topology::tolerance(&context);
    let solid = solidcore_step::topology::read_solid_with(
        &ex,
        solid_id,
        context,
        &solidcore_ops::trims::KernelTrims,
    )
    .map_err(|e| read_refusal(&e))?;
    check_cancel(cancel)?;
    if solid.sheet {
        return Err(invalid(
            "open shell / surface model not supported, solid required",
        ));
    }
    let mut source_reader = source::reader(&ex, scale, radians_per_angle_unit);

    let mut k = crate::kernel::make_kernel_context();
    // Trusted: the reader's record goes to the kernel as it is, never healed (no silent repair);
    // the record order then always stands.
    let (mut brep, order) = solidcore_convert::brep::to_kernel(&mut k, &solid.brep, true)
        .map_err(|e| kernel(&format!("B-rep conversion failure: {e}")))?;

    if !solidcore::topology::brep::p2::is_manifold_solid(&brep).unwrap_or(false) {
        return Err(invalid("solid B-rep is not a valid 2-manifold solid"));
    }

    let bounds = native_enclosure_bounds(&brep, cancel)?;

    // Record face `i` is kernel face `order.faces[i]`, the reader's `face_origin[i]`.
    let native_faces = order.faces.clone();
    if native_faces.len() != solid.face_origin.len() || native_faces.len() != faces.len() {
        return Err(invalid(
            "native face inventory changed the original face-use census",
        ));
    }
    let mut source_faces = std::collections::BTreeMap::new();
    for (native, source) in native_faces.into_iter().zip(&solid.face_origin) {
        if source_faces.insert(*source, native).is_some() {
            return Err(invalid("native face provenance is ambiguous"));
        }
    }
    let mut face_ids = Vec::with_capacity(faces.len());
    let mut face_step_ids = Vec::with_capacity(faces.len());
    for info in &faces {
        let source = info
            .surface_face_entity
            .parse::<u64>()
            .map_err(|_| invalid("original face provenance is not a STEP entity"))?;
        let native = source_faces
            .remove(&source)
            .ok_or_else(|| invalid("native face lost its original source identity"))?;
        face_ids.push(native);
        face_step_ids.push(StepEntityId(info.face_id.step_entity()));
    }
    if !source_faces.is_empty() {
        return Err(invalid(
            "native face inventory includes an unbound source face",
        ));
    }
    prepare_loop_edge_cache(&mut brep, &face_ids, query_proof_byte_limit, cancel)?;

    let shell_id = brep
        .shells()
        .ids()
        .next()
        .ok_or_else(|| invalid("native solid has no shell"))?;
    let original_source =
        source::capture(&entities, &mut source_reader, &faces, &face_ids, cancel)?;
    lines_through_their_vertices(&original_source, import_tolerance, cancel)?;

    let digest: [u8; 32] = Sha256::digest(bytes).into();
    let definition = Definition {
        id: DefinitionId::for_source(&digest, selected),
        provenance: SourceProvenance {
            normalized: None,
            source_hash: SourceHash::from_digest(&digest),
            source_name: String::new(),
            source_unit,
            uncertainty_mm,
            source_solid: selected.cloned(),
            findings: reader_findings(&solid.notes),
        },
        faces,
        bounds,
        source_bytes: bytes.len() as u32,
        brep,
        shell_id,
        face_ids,
        face_step_ids,
        original_source,
        source_units_scale_to_mm: scale,
        record_bytes: crate::kernel::record_bytes::of(&solid.brep),
        record: std::sync::Arc::new(solid.brep),
        record_order: order,
    };

    if definition.retained_query_proof_bytes() > query_proof_byte_limit {
        return Err(error(
            GeometryErrorCode::ResourceLimit,
            "retained query proof capacity exhausted",
        ));
    }

    Ok(definition)
}

/// Materialize the decoder's optional loop cache once, from the authoritative
/// reciprocal edge-use cycles. Consumers still independently check incidence;
/// this cache supplies ordered indexing, not source correspondence or material.
fn prepare_loop_edge_cache(
    brep: &mut solidcore::topology::Brep,
    faces: &[solidcore::FaceId],
    byte_limit: usize,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    use solidcore::nurbs::b_spline_surface_n_lib::p2::OrientType;
    use solidcore::topology::{EdgeuseOwner, LoopEdge, LoopuseStart};
    fn native(cause: solidcore::SolidError) -> GeometryError {
        kernel(&format!("native loop inventory: {cause}"))
    }
    let mut retained = 0usize;
    for &face_id in faces {
        check_cancel(cancel)?;
        let face = brep.face(face_id).map_err(native)?;
        let owner = face
            .faceuse
            .ok_or_else(|| invalid("native face has no loop owner"))?;
        let loop_count = face.loops.len();
        let faceuse = brep.faceuse(owner).map_err(native)?;
        if faceuse.face != Some(face_id) || faceuse.loopuses.len() != loop_count {
            return Err(invalid("native face and faceuse loop inventories disagree"));
        }
        for wire in 0..loop_count {
            let loop_id = brep.face(face_id).map_err(native)?.loops[wire];
            let loopuse_id = brep.faceuse(owner).map_err(native)?.loopuses[wire];
            let record = brep.loop_(loop_id).map_err(native)?;
            let loopuse = brep.loopuse(loopuse_id).map_err(native)?;
            if record.face != face_id
                || loopuse.loop_ != Some(loop_id)
                || loopuse.faceuse != Some(owner)
            {
                return Err(invalid("native loop belongs to a different face or use"));
            }
            let first = match loopuse.start {
                LoopuseStart::Edgeuse(Some(first)) => first,
                LoopuseStart::Vertexuse(Some(_)) if record.edges.is_empty() => continue,
                _ => {
                    return Err(invalid(
                        "native loop has no complete edge-use or vertex-use owner",
                    ));
                }
            };
            // Count without allocating an intermediate edge-use vector. A
            // finite arena bound also rejects a circuit that never returns.
            let mut count = 0usize;
            let mut current = first;
            loop {
                check_cancel(cancel)?;
                if count == brep.edgeuses().len() {
                    return Err(invalid("native loop edge-use circuit does not close"));
                }
                let use_ = brep.edgeuse(current).map_err(native)?;
                if use_.owner != EdgeuseOwner::Loopuse(Some(loopuse_id)) {
                    return Err(invalid("native loop edge-use circuit crosses owners"));
                }
                let next = use_
                    .ccw
                    .ok_or_else(|| invalid("native loop has no next edge use"))?;
                if brep.edgeuse(next).map_err(native)?.cw != Some(current) {
                    return Err(invalid("native loop edge-use links are not reciprocal"));
                }
                count += 1;
                current = next;
                if current == first {
                    break;
                }
            }
            let bytes = count
                .checked_mul(std::mem::size_of::<LoopEdge>())
                .and_then(|bytes| retained.checked_add(bytes))
                .ok_or_else(|| {
                    error(
                        GeometryErrorCode::ResourceLimit,
                        "native loop inventory capacity overflow",
                    )
                })?;
            if bytes > byte_limit {
                return Err(error(
                    GeometryErrorCode::ResourceLimit,
                    "native loop inventory allowance exhausted",
                ));
            }
            let mut entries = Vec::new();
            entries.try_reserve_exact(count).map_err(|_| {
                error(
                    GeometryErrorCode::ResourceLimit,
                    "native loop inventory allocation refused",
                )
            })?;
            retained = retained
                .checked_add(
                    entries
                        .capacity()
                        .saturating_mul(std::mem::size_of::<LoopEdge>()),
                )
                .ok_or_else(|| {
                    error(
                        GeometryErrorCode::ResourceLimit,
                        "native loop inventory capacity overflow",
                    )
                })?;
            if retained > byte_limit {
                return Err(error(
                    GeometryErrorCode::ResourceLimit,
                    "native loop inventory retained allowance exhausted",
                ));
            }
            current = first;
            for _ in 0..count {
                check_cancel(cancel)?;
                let use_ = brep.edgeuse(current).map_err(native)?;
                let edge = use_
                    .edge
                    .ok_or_else(|| invalid("native loop edge use has no edge"))?;
                brep.edge(edge).map_err(native)?;
                let forward = match use_.orientation {
                    OrientType::Same => true,
                    OrientType::Opposite => false,
                    _ => return Err(invalid("native loop edge use has unknown orientation")),
                };
                entries.push(LoopEdge { edge, forward });
                current = use_
                    .ccw
                    .ok_or_else(|| invalid("native loop has no next edge use"))?;
            }
            brep.loop_mut(loop_id).map_err(native)?.edges = entries;
        }
    }
    Ok(())
}

/// Read captured STEP bytes under the caller's remaining face and source-proof limits.
pub fn from_step_bytes_with_limits(
    bytes: &[u8],
    cancel: &AtomicBool,
    face_limit: u32,
    query_proof_byte_limit: usize,
) -> Result<Definition, GeometryError> {
    import_step(bytes, None, cancel, face_limit, query_proof_byte_limit)
}

/// Read one selected solid of a source that places several, under the
/// caller's limits. `None` requires a source that places exactly one solid.
pub fn from_step_solid_with_limits(
    bytes: &[u8],
    solid: Option<&SourceSolidId>,
    cancel: &AtomicBool,
    face_limit: u32,
    query_proof_byte_limit: usize,
) -> Result<Definition, GeometryError> {
    import_step(bytes, solid, cancel, face_limit, query_proof_byte_limit)
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
    import_step(bytes, None, cancel, MAX_NATIVE_FACES, MAX_QUERY_PROOF_BYTES)
}

/// Reads one selected solid with default limits.
pub fn from_step_solid_with_cancel(
    bytes: &[u8],
    solid: &SourceSolidId,
    cancel: &AtomicBool,
) -> Result<Definition, GeometryError> {
    import_step(
        bytes,
        Some(solid),
        cancel,
        MAX_NATIVE_FACES,
        MAX_QUERY_PROOF_BYTES,
    )
}

/// Reads a native Definition from a STEP file on disk.
pub fn from_step_file(path: impl AsRef<std::path::Path>) -> Result<Definition, GeometryError> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).map_err(|cause| {
        error(
            GeometryErrorCode::SourceIo,
            &format!("failed to open STEP file: {cause}"),
        )
    })?;
    let length = file
        .metadata()
        .map_err(|cause| error(GeometryErrorCode::SourceIo, &cause.to_string()))?
        .len();
    if length > u64::from(MAX_SOURCE_BYTES) {
        return Err(error(
            GeometryErrorCode::ResourceLimit,
            "STEP source exceeds immutable object representation",
        ));
    }
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(length as usize).map_err(|_| {
        error(
            GeometryErrorCode::ResourceLimit,
            "cannot allocate captured STEP source",
        )
    })?;
    // A changed source cannot force geometric Vec growth beyond its captured length.
    (&mut file)
        .take(length)
        .read_to_end(&mut bytes)
        .map_err(|cause| {
            error(
                GeometryErrorCode::SourceIo,
                &format!("failed to read STEP file: {cause}"),
            )
        })?;
    let mut extra = [0u8; 1];
    if bytes.len() as u64 != length
        || file
            .read(&mut extra)
            .map_err(|cause| error(GeometryErrorCode::SourceIo, &cause.to_string()))?
            != 0
    {
        return Err(error(
            GeometryErrorCode::SourceChanged,
            "STEP source changed during capture",
        ));
    }
    from_step_bytes(&bytes)
}

/// A complete native enclosure for scene fitting and coarse exclusion, not a
/// tight measurement or material receipt. Evaluate whole retained carrier/edge
/// restrictions; never substitute a vertex hull or a failed tight-box result.
/// The indices of the B-spline basis functions of `degree` over `knots` (`count` of them) that
/// are nonzero somewhere on `[a, b]`: support `[k_i, k_{i+degree+1}]` meeting it. `None` when the
/// range leaves the knot domain (no hull covers it) or is not ordered.
fn active_controls(
    knots: &[f64],
    degree: usize,
    count: usize,
    [a, b]: [f64; 2],
) -> Option<std::ops::Range<usize>> {
    let (start, end) = (*knots.get(degree)?, *knots.get(count)?);
    if !(a <= b && start <= a && b <= end) || knots.len() != count + degree + 1 {
        return None;
    }
    let first = (0..count).find(|&i| knots.get(i + degree + 1).is_some_and(|&k| k >= a))?;
    let last = (0..count)
        .rev()
        .find(|&i| knots.get(i).is_some_and(|&k| k <= b))?;
    (first <= last).then_some(first..last + 1)
}

/// The box of Euclidean control points (homogeneous / weight), widened by their division's
/// rounding. A rational net qualifies only with every weight finite and positive: each point
/// is then a convex combination of them. `None` when no point qualifies.
fn control_hull<'a>(
    points: impl Iterator<Item = &'a solidcore::nurbs::math::CPoint>,
    rational: bool,
) -> Option<[[f64; 2]; 3]> {
    let mut hull = [[f64::INFINITY, f64::NEG_INFINITY]; 3];
    let mut any = false;
    for p in points {
        let w = if rational { p.w } else { 1.0 };
        if !(w.is_finite() && w > 0.0) {
            return None;
        }
        for (axis, value) in [p.x, p.y, p.z].into_iter().enumerate() {
            let x = value / w;
            if !x.is_finite() {
                return None;
            }
            let slack = 4.0 * f64::EPSILON * x.abs() + f64::MIN_POSITIVE;
            hull[axis][0] = hull[axis][0].min(x - slack);
            hull[axis][1] = hull[axis][1].max(x + slack);
        }
        any = true;
    }
    any.then_some(hull)
}

/// Whether a face's carrier is a plane (unwrapping curve-bounded bases).
fn is_plane(surface: &solidcore::surface::surface::Surface) -> bool {
    use solidcore::surface::surface::SurfaceKind;
    let mut base = surface;
    for _ in 0..32 {
        match base.kind() {
            SurfaceKind::CurveBounded(b) => base = b.basis(),
            SurfaceKind::Plane(_) => return true,
            _ => return false,
        }
    }
    false
}

/// The control hull of a face's NURBS on `chart`, where the face's parameters are that NURBS's:
/// a spline, or a kernel cylinder, cone, sphere or torus on its form (not one under a STEP
/// wrapper, whose chart is the original's).
fn surface_hull(
    surface: &solidcore::surface::surface::Surface,
    chart: [[f64; 2]; 2],
) -> Result<Option<[[f64; 2]; 3]>, GeometryError> {
    use solidcore::surface::surface::SurfaceKind;
    let mut base = surface;
    for _ in 0..32 {
        match base.kind() {
            SurfaceKind::CurveBounded(b) => base = b.basis(),
            SurfaceKind::BSpline(_)
            | SurfaceKind::Cone(_)
            | SurfaceKind::Sphere(_)
            | SurfaceKind::Torus(_) => break,
            _ => return Ok(None),
        }
    }
    let Some(net) = crate::parametric::analytic_eval::native_surface_nurbs(base)? else {
        return Ok(None);
    };
    let (Some(rows), Some(columns)) = (
        active_controls(net.u_knots(), net.p(), net.n() + 1, chart[0]),
        active_controls(net.v_knots(), net.q(), net.m() + 1, chart[1]),
    ) else {
        return Ok(None);
    };
    let mut points = Vec::with_capacity(rows.len() * columns.len());
    for i in rows {
        for j in columns.clone() {
            points.push(
                net.cpt(i, j)
                    .map_err(|e| kernel(&format!("native face net: {e}")))?,
            );
        }
    }
    Ok(control_hull(points.iter(), net.is_rational()))
}

/// An interval enclosure intersected with a control hull, both sound, so the tighter of the
/// two on each side. A hull that misses the enclosure (rounding only) leaves the enclosure.
fn within_hull(
    position: [crate::parametric::Interval; 3],
    hull: Option<[[f64; 2]; 3]>,
) -> Result<[crate::parametric::Interval; 3], GeometryError> {
    let Some(hull) = hull else {
        return Ok(position);
    };
    let mut out = position;
    for axis in 0..3 {
        let (lo, hi) = (
            position[axis].lo.max(hull[axis][0]),
            position[axis].hi.min(hull[axis][1]),
        );
        if lo <= hi {
            out[axis] = crate::parametric::Interval::new(lo, hi)?;
        }
    }
    Ok(out)
}

/// Import bounds are tight to this (mm): a piece reaching at most this far past the bounds so
/// far is taken whole, rather than split further.
const BOUNDS_SLACK_MM: f64 = 1e-3;

/// The running box, grown only by sound enclosures.
struct Box3 {
    minimum: [f64; 3],
    maximum: [f64; 3],
}

impl Box3 {
    fn include(&mut self, position: [crate::parametric::Interval; 3]) -> Result<(), GeometryError> {
        for axis in 0..3 {
            let span = crate::parametric::Interval::new(position[axis].lo, position[axis].hi)?;
            self.minimum[axis] = self.minimum[axis].min(span.lo);
            self.maximum[axis] = self.maximum[axis].max(span.hi);
        }
        Ok(())
    }
    /// How far `position` reaches past the box on any side (0 inside it).
    fn excess(&self, position: &[crate::parametric::Interval; 3]) -> f64 {
        (0..3)
            .map(|axis| {
                (self.minimum[axis] - position[axis].lo)
                    .max(position[axis].hi - self.maximum[axis])
                    .max(0.0)
            })
            .fold(0.0, f64::max)
    }
}

/// Grow `bounds` by the image of `domain` (one or two parameter ranges): a piece whose sound
/// enclosure (`enclose`) lies inside the bounds so far adds nothing; one reaching past them by at
/// most [`BOUNDS_SLACK_MM`], or too small to split, is taken whole; any other is halved in each
/// range. Vertices and edges seed the bounds, so only pieces holding a body extreme split.
fn refine_bounds<const N: usize>(
    bounds: &mut Box3,
    domain: [[f64; 2]; N],
    work: &mut crate::GeometryWorkBudget,
    cancel: &AtomicBool,
    mut enclose: impl FnMut(
        [[f64; 2]; N],
        &mut crate::GeometryWorkBudget,
    ) -> Result<[crate::parametric::Interval; 3], GeometryError>,
) -> Result<(), GeometryError> {
    let mut stack = vec![domain];
    while let Some(piece) = stack.pop() {
        work.charge(1, cancel)?;
        let position = enclose(piece, work)?;
        let excess = bounds.excess(&position);
        if excess == 0.0 {
            continue;
        }
        let splittable = piece.iter().all(|[a, b]| {
            let m = 0.5 * (a + b);
            a < &m && &m < b
        });
        if excess <= BOUNDS_SLACK_MM || !splittable {
            bounds.include(position)?;
            continue;
        }
        let halves = piece.map(|[a, b]| {
            let m = 0.5 * (a + b);
            [[a, m], [m, b]]
        });
        for k in 0..(1usize << N) {
            stack.push(std::array::from_fn(|d| halves[d][(k >> d) & 1]));
        }
    }
    Ok(())
}

fn native_enclosure_bounds(
    brep: &solidcore::topology::Brep,
    cancel: &AtomicBool,
) -> Result<spiling_contracts::geometry::AabbMm, GeometryError> {
    use crate::parametric::{self, CurveEvaluator, Interval};
    let mut work = crate::GeometryWorkBudget::query();
    let kernel = crate::kernel::make_kernel_context();
    let mut bounds = Box3 {
        minimum: [f64::INFINITY; 3],
        maximum: [f64::NEG_INFINITY; 3],
    };
    for (_, vertex) in brep.vertices().iter() {
        work.charge(1, cancel)?;
        bounds.include([
            Interval::new(vertex.point.x, vertex.point.x)?,
            Interval::new(vertex.point.y, vertex.point.y)?,
            Interval::new(vertex.point.z, vertex.point.z)?,
        ])?;
    }
    for (edge_id, edge) in brep.edges().iter() {
        work.charge(1, cancel)?;
        let Some(curve) = &edge.curve else {
            return Err(invalid("native edge has no coefficient curve"));
        };
        // A spline edge lies in the hull of its control points active on a piece of it.
        let net = match curve.kind() {
            solidcore::curve::curve::CurveKind::BSpline(b) => b.gw_nurb(),
            _ => None,
        };
        let domain = [[edge.interval.min(), edge.interval.max()]];
        refine_bounds(&mut bounds, domain, &mut work, cancel, |[range], work| {
            let before = work.consumed();
            let position = curve.tensor(range, 0, work, cancel).map_err(|failure| {
                work.annotate_error(
                    failure,
                    before,
                    format_args!("native import edge enclosure; edge={edge_id:?}"),
                )
            })?[0];
            let hull = net.and_then(|n| {
                let rows = active_controls(n.knots(), n.degree(), n.cpts().len(), range)?;
                control_hull(n.cpts().get(rows)?.iter(), n.is_rational())
            });
            within_hull(position, hull)
        })?;
    }
    for (face_id, face) in brep.faces().iter() {
        work.charge(1, cancel)?;
        let surface = face
            .surface
            .as_ref()
            .ok_or_else(|| invalid("native face has no coefficient carrier"))?;
        // A plane face's coordinates are linear on it, so their extremes lie on its boundary,
        // whose edges are enclosed above; its parameter box reaches past its trims.
        if is_plane(surface) {
            continue;
        }
        let low = face.uv_domain.min();
        let high = face.uv_domain.max();
        let mut chart = [[low.x, high.x], [low.y, high.y]];
        // A face whose trims wind past one period of a closed direction (a
        // thread flank's helical band) lies within one whole turn there.
        let natural = surface
            .natural_uv_domain_with_nurb(&kernel)
            .map_err(|cause| self::kernel(&format!("native face chart: {cause}")))?;
        let natural = [
            [natural.u_min(), natural.u_max()],
            [natural.v_min(), natural.v_max()],
        ];
        let beyond = [0, 1].map(|d| chart[d][0] < natural[d][0] || chart[d][1] > natural[d][1]);
        if beyond.contains(&true) {
            let periods = surface_periods(&kernel, surface)
                .map_err(|cause| self::kernel(&format!("native face chart: {cause}")))?;
            for d in 0..2 {
                if beyond[d] && periods[d].is_some() {
                    chart[d] = natural[d];
                }
            }
        }
        // A spline, or a kernel cylinder/cone/sphere/torus on its NURBS form (whose parameters
        // the face's chart is in), lies in the hull of its control points active on a piece.
        refine_bounds(&mut bounds, chart, &mut work, cancel, |piece, work| {
            let before = work.consumed();
            let position = parametric::surface_position_enclosure(surface, piece, work, cancel)
                .map_err(|failure| {
                    work.annotate_error(
                        failure,
                        before,
                        format_args!("native import face enclosure; face={face_id:?}"),
                    )
                })?;
            within_hull(position, surface_hull(surface, piece)?)
        })?;
    }
    spiling_contracts::geometry::AabbMm::new(bounds.minimum, bounds.maximum)
}
