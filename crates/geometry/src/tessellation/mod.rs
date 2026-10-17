// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

mod boundary;
mod coverage;
mod sphere;
use crate::{MeshArtifact, check_cancel, kernel::Definition};
use monstertruck_meshing::prelude::{
    PolygonMesh, TessellationOptions, TessellationPrimitiveMode, TessellationPrimitiveOptions,
    trimmed_cshell_triangulation_with,
};
use monstertruck_modeling::{Curve, Curve2D, ParameterCurve, ParametricSurface3D, Point3, Surface};
use monstertruck_topology::{
    compress::{CompressedEdgeUse, CompressedTrimmedFace, CompressedTrimmedShell},
    trimmed::{TrimmedFace, TrimmedShell},
};
use spiling_contracts::display::{MeshBudget, MeshChunkBuilder};
use spiling_contracts::geometry::{
    AnalyticSurfaceDescription, DisplayProfile, FaceInfo, GeometryError, GeometryErrorCode,
    MAX_NATIVE_FACES, MESH_CARRIER_TOLERANCE_MM, NORMAL_NORM_TOLERANCE, TorusKind,
};
use std::{collections::HashMap, sync::atomic::AtomicBool};

pub(crate) fn tessellate(
    definition: &Definition,
    profile: DisplayProfile,
    cancel: &AtomicBool,
    budget: MeshBudget,
) -> Result<MeshArtifact, GeometryError> {
    check_cancel(cancel)?;
    budget.validate()?;
    let face_count = definition.faces.len();
    if face_count == 0 || face_count > MAX_NATIVE_FACES as usize {
        return Err(resource("native face count exceeds display support limits"));
    }
    if definition.solid.face_iter().count() != face_count
        || definition
            .trimmed_solid
            .boundaries()
            .iter()
            .map(|shell| shell.faces().len())
            .sum::<usize>()
            != face_count
    {
        return Err(invalid("native face slots disagree with source-face table"));
    }
    let mut builder =
        MeshChunkBuilder::with_budget(definition.id.clone(), face_count as u32, profile, budget)?;
    let mut work = crate::GeometryWorkBudget::new(20_000_000);
    let options = TessellationOptions {
        // Leave room for independently certified source trim displacement;
        // search-mesh tolerance never supplies either proof.
        tolerance: MESH_CARRIER_TOLERANCE_MM * 0.25,
        search_trials: 2,
        primitive: TessellationPrimitiveOptions {
            mode: TessellationPrimitiveMode::Triangles,
            ..Default::default()
        },
    };
    // A one-face shell preserves the public mesher's boundary/trim path without
    // materializing native polygon data for the rest of the solid.
    let trimmed_faces = definition
        .trimmed_solid
        .boundaries()
        .iter()
        .flat_map(|shell| shell.faces());
    for (ordinal, ((face, info), trimmed)) in definition
        .solid
        .face_iter()
        .zip(&definition.faces)
        .zip(trimmed_faces)
        .enumerate()
    {
        check_cancel(cancel)?;
        info.validate()?;
        match &info.geometry.analytic {
            Some(AnalyticSurfaceDescription::Torus { torus_kind, .. })
                if torus_kind != &TorusKind::RegularRing =>
            {
                return Err(singular(
                    "native non-ring torus has no certified regular display domain",
                ));
            }
            _ => {}
        }
        if trimmed.face().id() != face.id() || trimmed.face().orientation() != face.orientation() {
            return Err(invalid(
                "retained native trim charts changed source face identity/orientation",
            ));
        }
        let mut native_work = monstertruck_modeling::ValidatedGeometryBudget::new(
            usize::try_from(work.remaining()).unwrap_or(usize::MAX),
        );
        let snapshot_result = (|| {
            let mut bytes = face.with_surface(|surface| {
                monstertruck_modeling::surface_owned_bytes(
                    surface,
                    coverage::PROOF_BYTES,
                    &mut native_work,
                    cancel,
                )
            })?;
            for edge in face.edge_iter() {
                bytes = bytes.saturating_add(edge.with_curve(|curve| {
                    monstertruck_modeling::curve_owned_bytes(
                        curve,
                        coverage::PROOF_BYTES.saturating_sub(bytes),
                        &mut native_work,
                        cancel,
                    )
                })?);
            }
            Ok::<_, monstertruck_modeling::ValidatedGeometryError>(bytes)
        })();
        crate::parametric::debit_native_work(&mut work, native_work.consumed(), cancel)?;
        let snapshot_bytes = snapshot_result.map_err(crate::parametric::native_error)?;
        let wire_count = trimmed.trims().len();
        let coedge_count = trimmed.trims().iter().map(Vec::len).sum::<usize>();
        // Only references are copied. Precharge both old/new metadata buffers
        // while wrapping an original per-use UV curve in its borrowed carrier.
        let snapshot_metadata_bytes = coedge_count
            .saturating_mul(
                std::mem::size_of::<Option<&Curve2D>>()
                    .saturating_add(std::mem::size_of::<CompressedEdgeUse<&Curve2D>>())
                    .saturating_add(std::mem::size_of::<
                        CompressedEdgeUse<ParameterCurve<&Curve2D, &Surface>>,
                    >()),
            )
            .saturating_add(wire_count.saturating_mul(3 * std::mem::size_of::<Vec<()>>()))
            .saturating_add(3 * std::mem::size_of::<CompressedTrimmedFace<Surface, &Curve2D>>());
        coverage::check_memory(
            snapshot_bytes
                .saturating_add(builder.retained_bytes())
                .saturating_add(snapshot_metadata_bytes),
        )?;
        let mapping_work = coedge_count.saturating_add(wire_count).saturating_add(1) as u64;
        work.charge(mapping_work, cancel)?;
        let borrowed = TrimmedFace::try_new(
            face.clone(),
            trimmed
                .trims()
                .iter()
                .map(|wire| wire.iter().map(Option::as_ref).collect())
                .collect(),
        )
        .map_err(|_| invalid("original native trim layout changed at compression"))?;
        let shell: TrimmedShell<Point3, Curve, Surface, &Curve2D> =
            std::iter::once(borrowed).collect();
        let mut compressed = CompressedTrimmedShell::from(&shell);
        drop(shell);
        if compressed.faces.len() != 1 || compressed.faces[0].orientation != face.orientation() {
            return Err(invalid(
                "native face compression changed face slots or orientation",
            ));
        }
        let mut candidate_options = options;
        let mut refinements = 0;
        loop {
            work.charge(1, cancel)?;
            work.charge(mapping_work.saturating_mul(2), cancel)?;
            // The source carrier is borrowed only during the native candidate
            // call. Release its guard before Root trim/domain preparation.
            let (mut meshed, restored) = face.with_surface(|surface| {
                let lifted =
                    map_snapshot_trims(compressed, |curve| ParameterCurve::new(curve, surface));
                let meshed = trimmed_cshell_triangulation_with(&lifted, candidate_options);
                let restored = map_snapshot_trims(lifted, |trim| trim.decompose().0);
                (meshed, restored)
            });
            compressed = restored;
            check_cancel(cancel)?;
            if meshed.faces.len() != 1 || meshed.faces[0].orientation != face.orientation() {
                return Err(invalid(
                    "strict meshing changed native face slots or orientation",
                ));
            }
            let mut polygon = meshed.faces[0]
                .surface
                .take()
                .ok_or_else(|| invalid("strict meshing dropped a native face"))?;
            drop(meshed);
            let extra_bytes = snapshot_bytes
                .saturating_add(builder.retained_bytes())
                .saturating_add(snapshot_metadata_bytes)
                .saturating_add(compressed.vertices.capacity().saturating_mul(24))
                .saturating_add(
                    compressed
                        .edges
                        .capacity()
                        .saturating_mul(compressed.edges.first().map_or(0, std::mem::size_of_val)),
                );
            let sphere_chart = sphere::SphereChart::prepare(
                definition,
                ordinal,
                &polygon,
                extra_bytes,
                &mut work,
                cancel,
            )?;
            if let Some(chart) = &sphere_chart {
                chart.canonicalize_poles(&mut polygon, &mut work, cancel)?;
            }
            let boundary_face = if sphere_chart.is_none() {
                let prepared = boundary::BoundaryFace::prepare(
                    definition,
                    ordinal,
                    &polygon,
                    extra_bytes,
                    &mut work,
                    cancel,
                )?;
                prepared.canonicalize(&mut polygon, &mut work, cancel)?;
                Some(prepared)
            } else {
                None
            };
            let data = match audit_face(
                &FaceAuditInput {
                    polygon: &polygon,
                    surface: &compressed.faces[0].surface,
                    native_orientation: face.orientation(),
                    info,
                    budget: builder.remaining_budget(),
                    sphere_chart: sphere_chart.as_ref(),
                    boundary_face: boundary_face.as_ref(),
                    extra_retained_bytes: extra_bytes
                        .saturating_add(
                            sphere_chart
                                .as_ref()
                                .map_or(0, |chart| chart.native_retained_bytes()),
                        )
                        .saturating_add(
                            boundary_face
                                .as_ref()
                                .map_or(0, |face| face.native_retained_bytes()),
                        ),
                },
                &mut work,
                cancel,
            ) {
                Ok(data) => data,
                Err(cause)
                    if cause.code == GeometryErrorCode::ShapeErrorExceeded && refinements < 8 =>
                {
                    // Candidate generation is not proof. Refine only after a
                    // completed whole-triangle audit rejects its approximation;
                    // every attempt shares this job's unchanged work/byte caps.
                    work.charge(
                        (polygon.positions().len() + polygon.tri_faces().len()) as u64,
                        cancel,
                    )?;
                    refinements += 1;
                    candidate_options.tolerance *= 0.25;
                    continue;
                }
                Err(cause) => return Err(cause),
            };
            let trim_error = match &sphere_chart {
                Some(chart) => chart.certify(
                    definition,
                    ordinal,
                    &polygon,
                    data.retained_bytes(),
                    &mut work,
                    cancel,
                )?,
                None => boundary_face
                    .as_ref()
                    .ok_or_else(|| invalid("native UV proof was not prepared"))?
                    .certify(
                        definition,
                        ordinal,
                        &polygon,
                        data.retained_bytes(),
                        &mut work,
                        cancel,
                    )?,
            };
            if crate::parametric::up(data.carrier_deviation_mm + trim_error)
                > MESH_CARRIER_TOLERANCE_MM
            {
                if refinements < 8 {
                    work.charge(
                        (polygon.positions().len() + polygon.tri_faces().len()) as u64,
                        cancel,
                    )?;
                    refinements += 1;
                    candidate_options.tolerance *= 0.25;
                    continue;
                }
                return Err(GeometryError::new(
                    GeometryErrorCode::ShapeErrorExceeded,
                    format!(
                        "native carrier deviation {} mm plus whole-domain trim displacement {trim_error} mm exceeds {MESH_CARRIER_TOLERANCE_MM} mm on source face {ordinal}",
                        data.carrier_deviation_mm
                    ),
                ));
            }
            builder.push_face(
                ordinal as u32,
                &data.positions,
                &data.normals,
                &data.triangles,
                data.carrier_deviation_mm,
                trim_error,
            )?;
            // polygon and indexed adapter data are dropped at this face boundary.
            break;
        }
    }
    check_cancel(cancel)?;
    builder.finish()
}

/// Moves native geometry unchanged while adapting borrowed per-use trim views.
/// Callers precharge both metadata layouts; no carrier/UV geometry is cloned.
fn map_snapshot_trims<T, U>(
    snapshot: CompressedTrimmedShell<Point3, Curve, Surface, T>,
    mut map: impl FnMut(T) -> U,
) -> CompressedTrimmedShell<Point3, Curve, Surface, U> {
    CompressedTrimmedShell {
        vertices: snapshot.vertices,
        edges: snapshot.edges,
        faces: snapshot
            .faces
            .into_iter()
            .map(|face| CompressedTrimmedFace {
                boundaries: face
                    .boundaries
                    .into_iter()
                    .map(|wire| {
                        wire.into_iter()
                            .map(|edge| CompressedEdgeUse {
                                index: edge.index,
                                orientation: edge.orientation,
                                trim_curve: edge.trim_curve.map(&mut map),
                            })
                            .collect()
                    })
                    .collect(),
                orientation: face.orientation,
                surface: face.surface,
            })
            .collect(),
    }
}

struct FaceMesh {
    positions: Vec<[f64; 3]>,
    normals: Vec<[f64; 3]>,
    triangles: Vec<[u32; 3]>,
    carrier_deviation_mm: f64,
}
impl FaceMesh {
    fn retained_bytes(&self) -> usize {
        coverage::vector_bytes(&self.positions)
            .saturating_add(coverage::vector_bytes(&self.normals))
            .saturating_add(coverage::vector_bytes(&self.triangles))
    }
}

struct FaceAuditInput<'a, 'source> {
    polygon: &'a PolygonMesh,
    surface: &'a Surface,
    native_orientation: bool,
    info: &'a FaceInfo,
    budget: MeshBudget,
    sphere_chart: Option<&'a sphere::SphereChart<'source>>,
    boundary_face: Option<&'a boundary::BoundaryFace<'source>>,
    extra_retained_bytes: usize,
}

fn audit_face(
    input: &FaceAuditInput<'_, '_>,
    work: &mut crate::GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<FaceMesh, GeometryError> {
    let &FaceAuditInput {
        polygon,
        surface,
        native_orientation,
        info,
        budget,
        sphere_chart,
        boundary_face,
        extra_retained_bytes,
    } = input;
    if polygon.tri_faces().is_empty()
        || polygon.positions().is_empty()
        || !polygon.quad_faces().is_empty()
        || !polygon.other_faces().is_empty()
    {
        return Err(invalid(
            "strict meshing returned an empty or non-triangle face",
        ));
    }
    if polygon.positions().len() > budget.vertices as usize
        || polygon.normals().len() > budget.vertices as usize
        || polygon.uv_coords().len() > budget.vertices as usize
        || polygon.tri_faces().len() > budget.triangles as usize
    {
        return Err(resource(
            "per-face native tessellation exceeds count limits",
        ));
    }
    if polygon.positions().iter().any(|p| !finite([p.x, p.y, p.z]))
        || (boundary_face.is_none()
            && polygon.normals().iter().any(|n| {
                !finite([n.x, n.y, n.z])
                    || (length([n.x, n.y, n.z]) - 1.0).abs() > NORMAL_NORM_TOLERANCE
            }))
        || polygon
            .uv_coords()
            .iter()
            .any(|uv| !uv.x.is_finite() || !uv.y.is_finite())
    {
        return Err(invalid(
            "native tessellation contains nonfinite attributes or nonunit normals",
        ));
    }
    let occupied = coverage::proof_memory_bound(polygon)?.saturating_add(extra_retained_bytes);
    coverage::check_memory(occupied.saturating_add(polygon.tri_faces().len().saturating_mul(12)))?;
    work.charge(1, cancel)?;
    let mut result = FaceMesh {
        positions: Vec::new(),
        normals: Vec::new(),
        triangles: Vec::with_capacity(polygon.tri_faces().len()),
        carrier_deviation_mm: 0.0,
    };
    // Exact equal position/normal values share a vertex, including identical UV
    // seam attributes. A UV-only boundary must not duplicate display vertices.
    let mut vertices: HashMap<[u64; 6], u32> = HashMap::new();
    let mut corners: HashMap<(usize, Option<usize>, Option<usize>), u32> = HashMap::new();
    let joint_count = boundary_face.map_or(0, |face| face.joint_count());
    let occupied = occupied.saturating_add(joint_count);
    coverage::check_memory(occupied.saturating_add(result.retained_bytes()))?;
    work.charge(1, cancel)?;
    let mut omitted = vec![0_u8; joint_count];
    for (triangle_index, triangle) in polygon.tri_faces().iter().enumerate() {
        if triangle_index % 1024 == 0 {
            check_cancel(cancel)?;
        }
        let mut points = [[0.0; 3]; 3];
        let mut normals = [[0.0; 3]; 3];
        let mut indices = [0; 3];
        let mut parameters = [[0.0; 2]; 3];
        for (slot, vertex) in triangle.iter().enumerate() {
            let point = polygon
                .positions()
                .get(vertex.pos)
                .ok_or_else(|| invalid("native mesh position index out of range"))?;
            points[slot] = [point.x, point.y, point.z];
            let uv = vertex
                .uv
                .and_then(|index| polygon.uv_coords().get(index))
                .ok_or_else(|| invalid("native mesh parameter index out of range"))?;
            parameters[slot] = [uv.x, uv.y];
        }
        let joint = boundary_face
            .map(|face| face.triangle_joint(&parameters, work, cancel))
            .transpose()?
            .flatten();
        let mut boundary_normals = None;
        let mut boundary_normal = [false; 3];
        let mut boundary_error = None;
        if let Some(joint) = joint {
            let face =
                boundary_face.ok_or_else(|| invalid("native boundary authority disappeared"))?;
            match crate::parametric::triangle_boundary_error_bound(
                surface,
                &parameters,
                &points,
                face.vertex(joint),
                face.joint_kind(joint),
                work,
                cancel,
            )? {
                crate::parametric::BoundaryTriangleProof::VertexTube { error_mm } => {
                    if length(cross(sub(points[1], points[0]), sub(points[2], points[0]))) != 0.
                        || !error_mm.is_finite()
                        || error_mm < 0.
                        || error_mm > MESH_CARRIER_TOLERANCE_MM
                    {
                        return Err(GeometryError::new(
                            GeometryErrorCode::ShapeErrorExceeded,
                            "native omitted pole patch exceeds its original vertex tube",
                        ));
                    }
                    omitted[joint] = 1;
                    result.carrier_deviation_mm = result.carrier_deviation_mm.max(error_mm);
                    continue;
                }
                crate::parametric::BoundaryTriangleProof::NativeTriangle {
                    error_mm,
                    normals,
                    boundary_normal: mask,
                } => {
                    if mask.iter().enumerate().any(|(slot, &boundary)| {
                        boundary && !face.supports_normal(joint, parameters[slot])
                    }) {
                        return Err(invalid(
                            "one-sided normal has no original native boundary witness",
                        ));
                    }
                    boundary_error = Some(error_mm);
                    boundary_normals = Some(normals);
                    boundary_normal = mask;
                }
            }
        }
        for (slot, vertex) in triangle.iter().enumerate() {
            let corner = (vertex.pos, vertex.nor, vertex.uv);
            work.charge(1, cancel)?;
            if let Some(&index) = corners.get(&corner).filter(|_| !boundary_normal[slot]) {
                indices[slot] = index;
                points[slot] = result.positions[index as usize];
                normals[slot] = result.normals[index as usize];
                continue;
            }
            let grow = |length: usize, capacity: usize| {
                if length == capacity {
                    capacity.saturating_mul(2).max(8)
                } else {
                    capacity
                }
            };
            let maps =
                coverage::map_bytes::<[u64; 6], u32>(grow(vertices.len(), vertices.capacity()))
                    .saturating_add(coverage::map_bytes::<
                        (usize, Option<usize>, Option<usize>),
                        u32,
                    >(if boundary_normal[slot] {
                        corners.capacity()
                    } else {
                        grow(corners.len(), corners.capacity())
                    }));
            let vectors = grow(result.positions.len(), result.positions.capacity())
                .saturating_mul(24)
                .saturating_add(
                    grow(result.normals.len(), result.normals.capacity()).saturating_mul(24),
                )
                .saturating_add(result.triangles.capacity().saturating_mul(12));
            coverage::check_memory(occupied.saturating_add(maps).saturating_add(vectors))?;
            if corners.len() == budget.vertices as usize {
                return Err(resource(
                    "per-face native attribute combination budget exceeded",
                ));
            }
            let point = points[slot];
            let analytic = if boundary_normal[slot] {
                unit(
                    boundary_normals
                        .as_ref()
                        .ok_or_else(|| invalid("one-sided native normal proof absent"))?[slot],
                )?
            } else {
                let uv = parameters[slot];
                let analytic = surface.normal(uv[0], uv[1]);
                let analytic = unit([analytic.x, analytic.y, analytic.z])?;
                let normal = vertex
                    .nor
                    .and_then(|index| polygon.normals().get(index))
                    .ok_or_else(|| invalid("native mesh normal index absent or out of range"))?;
                let normal = [normal.x, normal.y, normal.z];
                if !finite(normal)
                    || (length(normal) - 1.).abs() > NORMAL_NORM_TOLERANCE
                    || length(sub(normal, analytic)) > NORMAL_NORM_TOLERANCE
                {
                    return Err(invalid(
                        "native mesh normal disagrees with regular surface orientation",
                    ));
                }
                analytic
            };
            let native_normal = if native_orientation {
                analytic
            } else {
                scale(analytic, -1.)
            };
            let normal = if boundary_normal[slot] {
                // Actual regular-interior representative, not an undefined
                // analytic pole normal or a claim of a unique tangent limit.
                native_normal
            } else {
                let carrier_normal = match &info.geometry.analytic {
                    Some(carrier) => carrier_normal(carrier, point)?,
                    None => analytic,
                };
                if info.orientation {
                    carrier_normal
                } else {
                    scale(carrier_normal, -1.)
                }
            };
            if !boundary_normal[slot] && length(sub(native_normal, normal)) > NORMAL_NORM_TOLERANCE
            {
                return Err(invalid(
                    "native oriented normal disagrees with cached carrier orientation",
                ));
            }
            // Cached orientation is relative to the canonical carrier; topology
            // orientation is separately applied to native triangle winding.
            points[slot] = point;
            normals[slot] = normal;
            let key = std::array::from_fn(|axis| {
                canonical_bits(if axis < 3 {
                    point[axis]
                } else {
                    normal[axis - 3]
                })
            });
            indices[slot] = if let Some(&index) = vertices.get(&key) {
                index
            } else {
                if result.positions.len() == budget.vertices as usize {
                    return Err(resource("per-face remapped vertex budget exceeded"));
                }
                let index = result.positions.len() as u32;
                vertices.insert(key, index);
                result.positions.push(point);
                result.normals.push(normal);
                index
            };
            if !boundary_normal[slot] {
                corners.insert(corner, indices[slot]);
            }
        }
        if !native_orientation {
            indices.swap(1, 2);
            points.swap(1, 2);
            normals.swap(1, 2);
            parameters.swap(1, 2);
        }
        let geometric_normal = cross(sub(points[1], points[0]), sub(points[2], points[0]));
        if !finite(geometric_normal)
            || length(geometric_normal) == 0.0
            || normals
                .iter()
                .any(|&normal| dot(geometric_normal, normal) <= 0.0)
        {
            return Err(invalid(
                "native mesh triangle is degenerate or incorrectly oriented",
            ));
        }
        let deviation = if let Some(chart) = sphere_chart {
            work.charge(1, cancel)?;
            chart.triangle_error(points)?
        } else if let Some(error) = boundary_error {
            error
        } else {
            // Every physical triangle point and the complete native UV-triangle
            // image are paired by the same barycentric weights, in both
            // directions. Trim coverage and packing error remain separate
            // gates; an infinite-analytic-carrier bound cannot strengthen the
            // reverse source-patch obligation.
            crate::parametric::triangle_error_bound(surface, &parameters, &points, work, cancel)?
        };
        if !deviation.is_finite() || deviation < 0. || deviation > MESH_CARRIER_TOLERANCE_MM {
            return Err(GeometryError::new(
                GeometryErrorCode::ShapeErrorExceeded,
                format!(
                    "native whole-triangle deviation {deviation} mm exceeds {MESH_CARRIER_TOLERANCE_MM} mm on source face {:?}",
                    info.face_id
                ),
            ));
        }
        result.carrier_deviation_mm = result.carrier_deviation_mm.max(deviation);
        result.triangles.push(indices);
    }
    if result.triangles.is_empty() {
        return Err(invalid(
            "native boundary quotient cannot drop a complete source face",
        ));
    }
    if let Some(face) = boundary_face
        && omitted.iter().enumerate().any(|(joint, &used)| {
            used != 0
                && !result
                    .positions
                    .iter()
                    .any(|point| *point == face.vertex(joint))
        })
    {
        return Err(invalid(
            "omitted native pole patch has no surviving original vertex anchor",
        ));
    }
    Ok(result)
}

fn carrier_normal(
    carrier: &AnalyticSurfaceDescription,
    point: [f64; 3],
) -> Result<[f64; 3], GeometryError> {
    match carrier {
        AnalyticSurfaceDescription::Plane { normal, .. } => Ok(*normal),
        AnalyticSurfaceDescription::Cylinder {
            axis_origin_mm,
            axis_direction,
            ..
        } => unit(project_radial(sub(point, *axis_origin_mm), *axis_direction)),
        AnalyticSurfaceDescription::Cone {
            frame,
            semi_angle_radians,
            ..
        } => {
            let radial = unit(project_radial(sub(point, frame.origin_mm), frame.z_axis))?;
            unit(sub(radial, scale(frame.z_axis, semi_angle_radians.tan())))
        }
        AnalyticSurfaceDescription::Sphere { frame, .. } => unit(sub(point, frame.origin_mm)),
        AnalyticSurfaceDescription::Torus {
            frame,
            major_radius_mm,
            torus_kind,
            ..
        } => {
            if *torus_kind != TorusKind::RegularRing {
                return Err(singular("singular/non-ring torus display is not certified"));
            }
            let relative = sub(point, frame.origin_mm);
            let radial = unit(project_radial(relative, frame.z_axis))?;
            unit(sub(relative, scale(radial, *major_radius_mm)))
        }
    }
}

fn project_radial(point: [f64; 3], axis: [f64; 3]) -> [f64; 3] {
    sub(point, scale(axis, dot(point, axis)))
}
fn unit(vector: [f64; 3]) -> Result<[f64; 3], GeometryError> {
    let norm = length(vector);
    if !finite(vector) || !norm.is_finite() {
        return Err(conditioning("nonfinite native normal enclosure"));
    }
    if norm == 0.0 {
        return Err(singular("native surface normal vanishes"));
    }
    Ok(scale(vector, 1.0 / norm))
}
fn canonical_bits(value: f64) -> u64 {
    if value == 0.0 { 0 } else { value.to_bits() }
}
fn finite(vector: [f64; 3]) -> bool {
    vector.into_iter().all(f64::is_finite)
}
#[cfg(test)]
fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] + b[i])
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}
fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    a.map(|v| v * s)
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn length(a: [f64; 3]) -> f64 {
    a[0].hypot(a[1]).hypot(a[2])
}
fn invalid(message: &str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::InvalidGeometry, message)
}
fn resource(message: &str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::ResourceLimit, message)
}
fn singular(message: &str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::SingularGeometry, message)
}
fn conditioning(message: &str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::ConditioningFailure, message)
}

#[cfg(test)]
mod tests;
