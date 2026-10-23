// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

mod boundary;
mod coverage;
mod display_trim;
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
            .saturating_add(3 * std::mem::size_of::<CompressedTrimmedFace<Surface, &Curve2D>>())
            // One bounded last shape refusal plus its replacement coexist.
            .saturating_add(2 * (std::mem::size_of::<GeometryError>()
                + spiling_contracts::geometry::MAX_ERROR_MESSAGE_BYTES as usize));
        coverage::check_memory(
            snapshot_bytes
                .saturating_add(builder.retained_bytes())
                .saturating_add(snapshot_metadata_bytes),
        )?;
        let display_face = display_trim::DisplayFace::prepare(
            definition,
            ordinal,
            trimmed,
            snapshot_bytes
                .saturating_add(builder.retained_bytes())
                .saturating_add(snapshot_metadata_bytes),
            &mut work,
            cancel,
        ).map_err(|cause| display_failure(
            cause, definition, ordinal, "display chart construction", &work,
        ))?;
        let display_bytes = display_face.as_ref().map_or(0, |face| face.retained_bytes());
        let mapping_work = coedge_count.saturating_add(wire_count).saturating_add(1) as u64;
        work.charge(mapping_work, cancel)?;
        let borrowed = TrimmedFace::try_new(
            face.clone(),
            match &display_face {
                Some(display) => display.face().trims().iter().map(|wire| {
                    wire.iter().map(|trim| trim.as_ref().map(
                        monstertruck_modeling::NativeTrimCurve::native_uv_curve,
                    )).collect()
                }).collect(),
                None => trimmed.trims().iter()
                    .map(|wire| wire.iter().map(Option::as_ref).collect())
                    .collect(),
            },
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
        let mut last_shape_error: Option<GeometryError> = None;
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
                .saturating_add(display_bytes)
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
            ).map_err(|cause| display_failure(
                cause, definition, ordinal, "spherical chart preparation", &work,
            ))?;
            if let Some(chart) = &sphere_chart {
                chart.canonicalize_poles(&mut polygon, &mut work, cancel)?;
            }
            let boundary_face = if sphere_chart.is_none() {
                let prepared = boundary::BoundaryFace::prepare(
                    definition,
                    ordinal,
                    display_face.as_ref(),
                    &polygon,
                    extra_bytes,
                    &mut work,
                    cancel,
                ).map_err(|cause| display_failure(
                    cause, definition, ordinal,
                    format_args!(
                        "boundary trim-domain preparation candidate={refinements} last_shape_error={}",
                        last_shape_error.as_ref().map_or("none", |error| error.message.as_str()),
                    ),
                    &work,
                ))?;
                prepared.canonicalize(&mut polygon, &mut work, cancel).map_err(|cause| {
                    display_failure(cause, definition, ordinal, "natural-boundary canonicalization", &work)
                })?;
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
            ).map_err(|cause| {
                let cause = if cause.code != GeometryErrorCode::ShapeErrorExceeded
                    && let Some(previous) = &last_shape_error
                {
                    GeometryError::new(cause.code, format!(
                        "{}; last_shape_error={}", cause.message, previous.message,
                    ))
                } else {
                    cause
                };
                display_failure(
                    cause, definition, ordinal,
                    format_args!(
                        "whole-triangle audit candidate={refinements} triangles={}",
                        polygon.tri_faces().len(),
                    ),
                    &work,
                )
            }) {
                Ok(data) => data,
                Err(cause)
                    if cause.code == GeometryErrorCode::ShapeErrorExceeded && refinements < 8 =>
                {
                    // Candidate generation is not proof. Refine only after a
                    // completed whole-triangle audit rejects its approximation;
                    // every attempt shares this job's unchanged work/byte caps.
                    last_shape_error = Some(cause);
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
                ),
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
                    ),
            }.map_err(|cause| display_failure(
                cause, definition, ordinal,
                format_args!(
                    "complete trim/triangle-chain coverage candidate={refinements} triangles={}",
                    polygon.tri_faces().len(),
                ),
                &work,
            ))?;
            if crate::parametric::up(data.carrier_deviation_mm + trim_error)
                > MESH_CARRIER_TOLERANCE_MM
            {
                if refinements < 8 {
                    last_shape_error = Some(GeometryError::new(
                        GeometryErrorCode::ShapeErrorExceeded,
                        format!(
                            "candidate={refinements}: carrier={} mm plus complete trim displacement={trim_error} mm exceeds {MESH_CARRIER_TOLERANCE_MM} mm",
                            data.carrier_deviation_mm,
                        ),
                    ));
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

fn display_failure(
    cause: GeometryError,
    definition: &Definition,
    ordinal: usize,
    stage: impl std::fmt::Display,
    work: &crate::GeometryWorkBudget,
) -> GeometryError {
    GeometryError::new(cause.code, format!(
        "native display {stage}: source={} face={ordinal} source_face={} cumulative_work={} remaining_work={}: {}",
        definition.id().as_str(), definition.faces[ordinal].face_id.as_str(),
        work.consumed(), work.remaining(), cause.message,
    ))
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

/// Build exact source-UV box IDs without retaining discovery storage in audit.
fn build_display_box_inventory(
    polygon: &PolygonMesh,
    native_orientation: bool,
    outside_bytes: usize,
    work: &mut crate::GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(Vec<u32>, Vec<crate::parametric::DisplayMeshBox>), GeometryError> {
    let mut triangles = Vec::<u32>::new();
    coverage::check_memory(outside_bytes.saturating_add(
        polygon.tri_faces().len().saturating_mul(std::mem::size_of::<u32>()),
    ))?;
    work.charge(1, cancel)?;
    triangles.try_reserve_exact(polygon.tri_faces().len())
        .map_err(|_| resource("display triangle-box index allocation failed"))?;
    let inventory_base = outside_bytes.saturating_add(
        triangles.capacity().saturating_mul(std::mem::size_of::<u32>()),
    );
    let mut ids = HashMap::<[u64; 4], u32>::new();
    let discovery_base = inventory_base.saturating_add(std::mem::size_of_val(&ids));
    coverage::check_memory(discovery_base)?;
    for triangle in polygon.tri_faces() {
        work.charge(8, cancel)?;
        let mut uv = [[0.0; 2]; 3];
        for (vertex, item) in triangle.iter().enumerate() {
            let point = item.uv.and_then(|index| polygon.uv_coords().get(index))
                .ok_or_else(|| invalid("box inventory has absent/out-of-range native UV"))?;
            uv[vertex] = [point.x, point.y];
        }
        if !native_orientation {
            uv.swap(1, 2);
        }
        let domain: [[f64; 2]; 2] = std::array::from_fn(|axis| [
            uv.iter().map(|point| point[axis]).fold(f64::INFINITY, f64::min),
            uv.iter().map(|point| point[axis]).fold(f64::NEG_INFINITY, f64::max),
        ]);
        let key = [
            domain[0][0].to_bits(), domain[0][1].to_bits(),
            domain[1][0].to_bits(), domain[1][1].to_bits(),
        ];
        let index = if let Some(index) = ids.get(&key) {
            *index
        } else {
            let index = u32::try_from(ids.len())
                .map_err(|_| resource("display UV-box index representation exhausted"))?;
            if ids.len() == ids.capacity() {
                coverage::check_memory(discovery_base
                    .saturating_add(coverage::map_bytes::<[u64; 4], u32>(ids.capacity()))
                    .saturating_add(coverage::map_bytes::<[u64; 4], u32>(
                        ids.capacity().saturating_mul(2).max(8),
                    )))?;
                work.charge(1, cancel)?;
                ids.try_reserve(1).map_err(|_| resource("display box discovery allocation failed"))?;
                coverage::check_memory(discovery_base.saturating_add(
                    coverage::map_bytes::<[u64; 4], u32>(ids.capacity()),
                ))?;
            }
            ids.insert(key, index);
            index
        };
        triangles.push(index);
    }
    // Only the exact discovered cardinality determines the large proof table.
    let count = ids.len();
    let mut keys = Vec::<Option<[u64; 4]>>::new();
    let key_unit = std::mem::size_of::<Option<[u64; 4]>>();
    let key_header = std::mem::size_of_val(&keys);
    coverage::check_memory(discovery_base
        .saturating_add(coverage::map_bytes::<[u64; 4], u32>(ids.capacity()))
        .saturating_add(key_header)
        .saturating_add(count.saturating_mul(key_unit)))?;
    work.charge(count as u64 + 1, cancel)?;
    keys.try_reserve_exact(count).map_err(|_| resource("display box-key workspace allocation failed"))?;
    let key_bytes = key_header.saturating_add(keys.capacity().saturating_mul(key_unit));
    coverage::check_memory(discovery_base
        .saturating_add(coverage::map_bytes::<[u64; 4], u32>(ids.capacity()))
        .saturating_add(key_bytes))?;
    for index in 0..count {
        if index % 1024 == 0 {
            check_cancel(cancel)?;
        }
        keys.push(None);
    }
    for (key, &index) in &ids {
        work.charge(1, cancel)?;
        *keys.get_mut(index as usize).ok_or_else(|| invalid("display box inventory index changed"))? =
            Some(*key);
    }
    drop(ids);
    let mut boxes = Vec::<crate::parametric::DisplayMeshBox>::new();
    let proof_unit = std::mem::size_of::<crate::parametric::DisplayMeshBox>();
    let construction_base = inventory_base.saturating_add(key_bytes);
    coverage::check_memory(construction_base.saturating_add(count.saturating_mul(proof_unit)))?;
    work.charge(1, cancel)?;
    boxes.try_reserve_exact(count).map_err(|_| resource("display exact box-proof allocation failed"))?;
    coverage::check_memory(construction_base.saturating_add(
        boxes.capacity().saturating_mul(proof_unit),
    ))?;
    for key in keys {
        work.charge(1, cancel)?;
        boxes.push(crate::parametric::DisplayMeshBox::new(
            key.ok_or_else(|| invalid("display box key inventory is incomplete"))?,
        ));
    }
    // Both discovery map and key workspace are gone before audit/output growth.
    Ok((triangles, boxes))
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
    let audit_base = coverage::proof_memory_bound(polygon)?.saturating_add(extra_retained_bytes);
    let proof_uvs = if sphere_chart.is_none() { polygon.uv_coords().as_slice() } else { &[] };
    let proposed_proof_bytes = crate::parametric::DisplayTriangleProof::base_retained_bytes_for_vertices(
        proof_uvs.len(),
    )?;
    coverage::check_memory(audit_base.saturating_add(proposed_proof_bytes)
        .saturating_add(polygon.tri_faces().len().saturating_mul(12)))?;
    let (triangle_boxes, mesh_boxes) = if sphere_chart.is_none() {
        build_display_box_inventory(
            polygon, native_orientation, audit_base.saturating_add(proposed_proof_bytes), work, cancel,
        )?
    } else {
        (Vec::new(), Vec::new())
    };
    let mut triangle_proof = crate::parametric::DisplayTriangleProof::with_mesh_inventory(
        surface, proof_uvs, triangle_boxes, mesh_boxes, work, cancel,
    )?;
    let occupied = audit_base.saturating_add(triangle_proof.retained_bytes());
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
        let mut parameter_indices = [0_usize; 3];
        for (slot, vertex) in triangle.iter().enumerate() {
            let point = polygon
                .positions()
                .get(vertex.pos)
                .ok_or_else(|| invalid("native mesh position index out of range"))?;
            points[slot] = [point.x, point.y, point.z];
            let parameter_index = vertex.uv
                .ok_or_else(|| invalid("native mesh parameter index is absent"))?;
            let uv = polygon.uv_coords().get(parameter_index)
                .ok_or_else(|| invalid("native mesh parameter index out of range"))?;
            parameters[slot] = [uv.x, uv.y];
            parameter_indices[slot] = parameter_index;
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
            parameter_indices.swap(1, 2);
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
            triangle_proof.indexed_triangle_error_bound(
                &parameters, &points, triangle_index, &parameter_indices, work, cancel,
            ).map_err(|cause| {
                let uv_box: [[f64; 2]; 2] = std::array::from_fn(|axis| [
                    parameters.iter().map(|point| point[axis]).fold(f64::INFINITY, f64::min),
                    parameters.iter().map(|point| point[axis]).fold(f64::NEG_INFINITY, f64::max),
                ]);
                GeometryError::new(cause.code, format!(
                    "triangle={triangle_index} uv_box={uv_box:?}: {}", cause.message,
                ))
            })?
        };
        if !deviation.is_finite() || deviation < 0. || deviation > MESH_CARRIER_TOLERANCE_MM {
            work.charge(1, cancel)?;
            return Err(GeometryError::new(
                GeometryErrorCode::ShapeErrorExceeded,
                format!(
                    "native whole-triangle deviation {deviation} mm exceeds {MESH_CARRIER_TOLERANCE_MM} mm on source face {:?}; triangle_uv={parameters:?} smooth_span={:?}",
                    info.face_id, triangle_proof.smooth_span_for_triangle(triangle_index, &parameters),
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
