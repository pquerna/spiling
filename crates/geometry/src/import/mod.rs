// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

mod source;

mod ast;
pub(crate) mod brep_data_bridge;
#[cfg(test)]
mod tests;
mod units;

use crate::{check_cancel, kernel::Definition};
use ast::*;
use sha2::{Digest, Sha256};
use solidcore::math::{Point3, Vec3};
use spiling_contracts::geometry::{
    AnalyticSurfaceDescription, DefinitionId, FaceDomainSummary, FaceGeometryInfo, FaceId,
    FaceInfo, GeometryError, GeometryErrorCode, MAX_NATIVE_FACES, MAX_SOURCE_BYTES, PlaneFrameMm,
    SourceHash, SourceProvenance, StepEntityId, SurfaceFamily, TorusKind,
};
use std::{collections::BTreeSet, sync::atomic::AtomicBool};
use step_p21::ast::Parameter;

pub(crate) const MAX_QUERY_PROOF_BYTES: usize = 32 * 1024 * 1024;
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
                    | "ORIENTED_CLOSED_SHELL"
                    | "NEXT_ASSEMBLY_USAGE_OCCURRENCE"
                    | "ASSEMBLY_COMPONENT_USAGE"
                    | "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION"
                    | "REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION"
                    | "MAPPED_ITEM"
                    | "REPRESENTATION_MAP"
            ) {
                return Err(unsupported(
                    "assembly, void-shell or oriented-shell STEP representation",
                ));
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
        let (resolved, orientation) = resolve_face(&entities, id)?;
        let (source_entity_kind, source_surface_entity_kind, geometry) =
            source_geometry(&entities, resolved, scale)?;
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
    let ex = brep_data_bridge::parse_step_exchange(text)
        .map_err(|e| invalid(&format!("invalid ISO-10303-21 exchange: {e}")))?;
    check_cancel(cancel)?;
    // The exact single root was validated above. Decode it once, directly in
    // authoritative mm units, rather than building an unscaled BREP first.
    let mut context = solidcraft_step::units::Context::default();
    context.units.len = scale;
    context.uncertainty = uncertainty_mm;
    let solid = solidcraft_step::topology::read_solid(&ex, solid_id, context).map_err(|cause| {
        invalid(&format!(
            "normalized STEP topology extraction failed: {cause:?}"
        ))
    })?;
    check_cancel(cancel)?;
    if solid.sheet {
        return Err(invalid(
            "open shell / surface model not supported, solid required",
        ));
    }

    let mut k = crate::kernel::make_kernel_context();
    let mut brep = brep_data_bridge::from_schema4_brep(&mut k, &solid.brep, false)
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

    let native_faces = brep.faces().ids().collect::<Vec<_>>();
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
        face_ids.push(
            source_faces
                .remove(&source)
                .ok_or_else(|| invalid("native face lost its original source identity"))?,
        );
        face_step_ids.push(StepEntityId(info.face_id.step_entity()));
    }
    if !source_faces.is_empty() {
        return Err(invalid(
            "native face inventory includes an unbound source face",
        ));
    }
    prepare_loop_edge_cache(&mut brep, &face_ids, query_proof_byte_limit, cancel)?;

    let face_surface_maps: Vec<crate::kernel::SurfaceParameterMap> =
        compute_face_surface_maps(&solid.brep);

    let shell_id = brep
        .shells()
        .ids()
        .next()
        .ok_or_else(|| invalid("native solid has no shell"))?;
    let original_source = source::capture(&entities, &ex, &faces, &face_ids, scale, cancel)?;

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
        face_ids,
        face_step_ids,
        face_surface_maps,
        original_source,
        source_units_scale_to_mm: scale,
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

/// Read captured STEP bytes under the caller's remaining face and source-proof limits.
pub fn from_step_bytes_with_limits(
    bytes: &[u8],
    cancel: &AtomicBool,
    face_limit: u32,
    query_proof_byte_limit: usize,
) -> Result<Definition, GeometryError> {
    import_step(bytes, cancel, face_limit, query_proof_byte_limit)
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
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| {
            file.take(u64::from(MAX_SOURCE_BYTES) + 1)
                .read_to_end(&mut bytes)
        })
        .map_err(|cause| {
            GeometryError::new(
                GeometryErrorCode::SourceIo,
                format!("failed to read STEP file: {cause}"),
            )
        })?;
    from_step_bytes(&bytes)
}
