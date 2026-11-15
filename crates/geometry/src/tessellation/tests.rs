// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_contracts::display::{ValidatedMeshChunk, validate_mesh_chunk, validate_mesh_manifest};
use spiling_contracts::geometry::{
    ArtifactId, MAX_GEOMETRY_CHUNK_BYTES, MESH_QUANTIZATION_TOLERANCE_MM,
    MESH_SURFACE_TOLERANCE_MM, MeshChunkMetadata, SessionId,
};
use std::path::Path;

fn fixture(name: &str) -> Definition {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/geometry")
        .join(name);
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("original fixture {}: {error}", path.display()));
    crate::from_step_bytes_with_cancel(&bytes, &AtomicBool::new(false)).unwrap()
}

/// The definition's planned candidates at `chord`.
fn planned(definition: &Definition, chord: f64) -> candidate::Candidates {
    candidate::plan(
        definition,
        MeshTolerance::display(chord),
        PROOF_BYTES,
        &mut GeometryWorkBudget::query(),
        &AtomicBool::new(false),
    )
    .unwrap()
}

#[test]
fn candidate_parameter_roundoff_never_changes_source_or_physical_vertices() {
    let definition = fixture("independent-exporters/occt-spline-housing-mm.step");
    let candidate::Candidates {
        edges, mut faces, ..
    } = planned(&definition, MESH_CARRIER_TOLERANCE_MM * 0.25);
    let face = definition.face_ids()[1];
    let mut candidate = faces.swap_remove(1);
    let domain = definition
        .face_surface(face)
        .unwrap()
        .natural_uv_domain_if_built();
    let source = definition.provenance().clone();
    let positions = candidate.positions.clone();
    candidate.uvs[0].y = crate::parametric::up(crate::parametric::up(domain.v_max()));
    let mut work = GeometryWorkBudget::query();
    let cancel = AtomicBool::new(false);
    prepare_candidate_parameters(&definition, &mut candidate, &mut work, &cancel).unwrap();
    assert_eq!(candidate.uvs[0].y, domain.v_max());
    assert_eq!(candidate.positions, positions);
    assert_eq!(definition.provenance(), &source);
    let outside = domain.v_max() + 0.01;
    candidate.uvs[0].y = outside;
    prepare_candidate_parameters(&definition, &mut candidate, &mut work, &cancel).unwrap();
    assert_eq!(candidate.uvs[0].y, outside);
    assert!(
        audit::face(
            &definition,
            1,
            &candidate,
            &edges,
            MeshBudget::FROZEN,
            0,
            &mut work,
            &cancel,
        )
        .is_err()
    );
}

#[test]
fn refinement_round_batches_audits_and_requires_a_fresh_complete_pass() {
    let mut requested = Vec::new();
    let mut round = RefinementRound::new(0);
    let mut packed = Vec::new();
    for ordinal in 0..16 {
        let result = if ordinal == 1 || ordinal == 12 {
            Err(shape(format!("independent audit face {ordinal}")))
        } else {
            Ok(ordinal)
        };
        if let Some(data) = round
            .audit(result, |_| {
                requested.push(ordinal);
                Ok(())
            })
            .unwrap()
        {
            packed.push(data);
        }
    }
    assert_eq!(packed, [0]);
    assert_eq!(
        round.finish().unwrap_err(),
        shape("independent audit face 1")
    );
    // Every failing audit of the round is collected; the next round plans the body finer.
    assert_eq!(requested, [1, 12]);
    let mut fresh = RefinementRound::new(1);
    let complete: Vec<_> = (0..16)
        .map(|ordinal| {
            fresh
                .audit(Ok(ordinal), |_| panic!("successful audits never refine"))
                .unwrap()
                .unwrap()
        })
        .collect();
    fresh.finish().unwrap();
    assert_eq!(complete, (0..16).collect::<Vec<_>>());
}

#[test]
fn refinement_round_preserves_retry_priority_and_terminal_operational_failures() {
    let first = shape("first independent audit");
    let mut round = RefinementRound::new(0);
    round.audit::<()>(Err(first.clone()), |_| Ok(())).unwrap();
    for code in [
        GeometryErrorCode::InvalidGeometry,
        GeometryErrorCode::AnalysisIndeterminate,
    ] {
        let later = GeometryError::new(code, "later candidate or audit failure");
        assert_eq!(round.failure(later.clone()), first);
        assert_eq!(
            round
                .audit::<()>(Err(later.clone()), |_| panic!("not a refinement request"))
                .unwrap_err(),
            first
        );
        assert_eq!(RefinementRound::new(0).failure(later.clone()), later);
    }
    for code in [
        GeometryErrorCode::ResourceLimit,
        GeometryErrorCode::Cancelled,
    ] {
        let terminal = GeometryError::new(code, "operational failure");
        assert_eq!(round.failure(terminal.clone()), terminal);
        assert_eq!(
            round
                .audit::<()>(Err(terminal.clone()), |_| panic!(
                    "not a refinement request"
                ))
                .unwrap_err(),
            terminal
        );
    }
    assert_eq!(
        RefinementRound::new(8)
            .audit::<()>(Err(first.clone()), |_| panic!("last round must not retry"))
            .unwrap_err(),
        first
    );
}

#[test]
fn refinement_exhaustion_retains_source_attempt_and_original_audit_context() {
    let definition = fixture("box-mm.step");
    let cancel = AtomicBool::new(false);
    let mut exhausted = GeometryWorkBudget::new(0);
    let failure = candidate::plan(
        &definition,
        MeshTolerance::display(MESH_CARRIER_TOLERANCE_MM * 0.25 * 0.25),
        PROOF_BYTES,
        &mut exhausted,
        &cancel,
    )
    .err()
    .unwrap();
    let scoped = display_context(
        failure,
        &definition,
        None,
        1,
        exhausted.consumed(),
        Some("original independent audit"),
    );
    assert_eq!(scoped.code, GeometryErrorCode::ResourceLimit);
    assert!(scoped.message.contains(definition.id().as_str()));
    assert!(scoped.message.contains("attempt=1 work=0"));
    assert!(
        scoped
            .message
            .contains("first refinement: original independent audit")
    );

    let mut first_refinement = None;
    let failure = RefinementRound::new(0)
        .audit::<()>(Err(shape("original independent audit")), |cause| {
            first_refinement.get_or_insert_with(|| cause.message.clone());
            exhausted.charge(1, &cancel)
        })
        .unwrap_err();
    assert_eq!(failure.code, GeometryErrorCode::ResourceLimit);
    assert_eq!(
        first_refinement.as_deref(),
        Some("original independent audit")
    );
}

fn metadata(mesh: &MeshArtifact) -> Vec<MeshChunkMetadata> {
    let session = SessionId::new();
    let artifact = ArtifactId::new(1).unwrap();
    mesh.chunks
        .iter()
        .map(|chunk| {
            chunk
                .descriptor
                .clone()
                .into_metadata(session.clone(), artifact)
        })
        .collect()
}
fn decoded<'a>(bytes: &'a [u8], descriptor: &MeshChunkMetadata) -> ValidatedMeshChunk<'a> {
    validate_mesh_chunk(
        bytes,
        descriptor,
        &descriptor.session_id,
        descriptor.artifact_id,
        &descriptor.definition_id,
        descriptor.chunk_index,
    )
    .unwrap()
}
fn point(mesh: &ValidatedMeshChunk<'_>, index: u32) -> [f64; 3] {
    let p = mesh.positions.get(index as usize).unwrap();
    std::array::from_fn(|a| mesh.local_origin_mm[a] + f64::from(p[a]))
}
fn audit_encoded(definition: &Definition, encoded: &MeshArtifact) {
    let descriptors = metadata(encoded);
    validate_mesh_manifest(&descriptors).unwrap();
    let mut seen = vec![false; definition.face_ids().len()];
    let mut previous = 0;
    let mut triangles = 0;
    for (chunk, descriptor) in encoded.chunks.iter().zip(&descriptors) {
        assert_eq!(&descriptor.definition_id, definition.id());
        assert_eq!(descriptor.face_count as usize, definition.face_ids().len());
        assert!(chunk.bytes.len() <= MAX_GEOMETRY_CHUNK_BYTES as usize);
        assert!(
            descriptor.carrier_deviation_mm + descriptor.trim_deviation_mm
                <= MESH_CARRIER_TOLERANCE_MM
        );
        assert!(descriptor.quantization_error_mm <= MESH_QUANTIZATION_TOLERANCE_MM);
        assert!(
            descriptor.carrier_deviation_mm
                + descriptor.trim_deviation_mm
                + descriptor.quantization_error_mm
                <= MESH_SURFACE_TOLERANCE_MM
        );
        let view = decoded(&chunk.bytes, descriptor);
        triangles += view.face_ordinals.len() as u32;
        for (triangle_index, ordinal) in view.face_ordinals.iter().enumerate() {
            assert!(ordinal >= previous);
            previous = ordinal;
            seen[ordinal as usize] = true;
            let native = definition.face_ids()[ordinal as usize];
            assert_eq!(definition.face_ordinal(native), Some(ordinal as usize));
            let points: [[f64; 3]; 3] = std::array::from_fn(|i| {
                point(&view, view.indices.get(3 * triangle_index + i).unwrap())
            });
            let a = audit::sub(points[1], points[0]);
            let b = audit::sub(points[2], points[0]);
            let cross = [
                a[1] * b[2] - a[2] * b[1],
                a[2] * b[0] - a[0] * b[2],
                a[0] * b[1] - a[1] * b[0],
            ];
            for slot in 0..3 {
                let index = view.indices.get(3 * triangle_index + slot).unwrap();
                let normal = view.normals.get(index as usize).unwrap();
                assert!((0..3).map(|i| cross[i] * f64::from(normal[i])).sum::<f64>() > 0.0);
            }
        }
    }
    assert!(
        seen.into_iter().all(|v| v),
        "every original source-face ordinal must be published"
    );
    assert_eq!(triangles, encoded.triangle_count);
    assert!(triangles > 0);
}

#[test]
fn caller_work_and_progress_cover_the_complete_display_without_resetting() {
    let definition = fixture("box-mm.step");
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::query();
    work.charge(11, &cancel).unwrap();
    let mut updates = Vec::new();
    crate::tessellate_with_work(
        &definition,
        DisplayProfile::MeshTrimmedMm005V2,
        &cancel,
        MeshBudget::FROZEN,
        &mut work,
        &mut |update| updates.push(update),
    )
    .unwrap();
    assert_eq!(updates[0].stage, DisplayStage::PreparingEdges);
    // The caller's work carries on: never reset, and the plan's own work adds to it.
    assert!(updates[0].consumed_work >= 11);
    assert!(updates[1].consumed_work > 11);
    assert!(updates.windows(2).all(|pair| {
        pair[0].consumed_work <= pair[1].consumed_work
            && pair[0].completed_faces <= pair[1].completed_faces
    }));
    let completed: Vec<_> = updates
        .iter()
        .filter(|update| update.stage == DisplayStage::FaceComplete)
        .map(|update| update.completed_faces)
        .collect();
    assert_eq!(completed, vec![1, 2, 3, 4, 5, 6]);
    let last = updates.last().unwrap();
    assert_eq!(last.stage, DisplayStage::Complete);
    assert_eq!(last.total_faces, 6);
    assert_eq!(last.completed_faces, last.total_faces);
    assert_eq!(last.consumed_work, work.consumed());
    let mut limited = GeometryWorkBudget::new(work.consumed() - 12);
    let mut stages = Vec::new();
    let failure = crate::tessellate_with_work(
        &definition,
        DisplayProfile::MeshTrimmedMm005V2,
        &cancel,
        MeshBudget::FROZEN,
        &mut limited,
        &mut |update| stages.push(update.stage),
    )
    .unwrap_err();
    assert_eq!(failure.code, GeometryErrorCode::ResourceLimit);
    assert!(!stages.contains(&DisplayStage::Complete));
    let mut stages = Vec::new();
    let failure = crate::tessellate_with_work(
        &definition,
        DisplayProfile::MeshTrimmedMm005V2,
        &cancel,
        MeshBudget::FROZEN,
        &mut GeometryWorkBudget::query(),
        &mut |update| {
            stages.push(update.stage);
            if update.stage == DisplayStage::AuditingFace {
                cancel.store(true, std::sync::atomic::Ordering::Release);
            }
        },
    )
    .unwrap_err();
    assert_eq!(failure.code, GeometryErrorCode::Cancelled);
    assert!(stages.contains(&DisplayStage::AuditingFace));
    assert!(!stages.contains(&DisplayStage::Complete));
}

#[test]
fn original_parts_preserve_all_source_faces_analytic_normals_and_profile_precision() {
    for name in [
        "box-mm.step",
        "box-inch.step",
        "cylinder.step",
        "through-hole.step",
        "independent-exporters/occt-box-mm.step",
        "independent-exporters/occt-cylinder-mm.step",
        "independent-exporters/occt-through-hole-mm.step",
        "independent-exporters/occt-rounded-housing-mm.step",
        "independent-exporters/occt-tapered-fitting-mm.step",
        "independent-exporters/occt-ribbed-bracket-mm.step",
    ] {
        let definition = fixture(name);
        let mesh = tessellate(
            &definition,
            DisplayProfile::MeshTrimmedMm005V2,
            &AtomicBool::new(false),
            MeshBudget::FROZEN,
        )
        .unwrap_or_else(|cause| panic!("{name}: {cause:?}"));
        audit_encoded(&definition, &mesh);
    }
}

#[test]
fn unchanged_independent_fitting_display_preserves_physical_taper_and_bore() {
    let definition = fixture("independent-exporters/occt-tapered-fitting-mm.step");
    let mesh = tessellate(
        &definition,
        DisplayProfile::MeshTrimmedMm005V2,
        &AtomicBool::new(false),
        MeshBudget::FROZEN,
    )
    .unwrap();
    audit_encoded(&definition, &mesh);
    for (chunk, descriptor) in mesh.chunks.iter().zip(metadata(&mesh)) {
        let view = decoded(&chunk.bytes, &descriptor);
        for triangle in 0..view.face_ordinals.len() {
            let points: [[f64; 3]; 3] =
                std::array::from_fn(|i| point(&view, view.indices.get(3 * triangle + i).unwrap()));
            for weights in [
                [1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 1.0],
                [0.5, 0.5, 0.0],
                [0.0, 0.5, 0.5],
                [0.5, 0.0, 0.5],
                [1.0 / 3.0; 3],
            ] {
                let p: [f64; 3] =
                    std::array::from_fn(|axis| (0..3).map(|i| weights[i] * points[i][axis]).sum());
                let ordinal = view.face_ordinals.get(triangle).unwrap();
                use spiling_contracts::geometry::AnalyticSurfaceDescription as Carrier;
                let distance = match definition.faces()[ordinal as usize]
                    .geometry
                    .analytic
                    .as_ref()
                    .unwrap()
                {
                    Carrier::Plane { origin_mm, .. } => (p[2] - origin_mm[2]).abs(),
                    Carrier::Cylinder { radius_mm, .. } => {
                        assert!((*radius_mm - 2.0).abs() < 1e-10);
                        (p[0].hypot(p[1]) - 2.0).abs()
                    }
                    Carrier::Cone { .. } => {
                        (p[0].hypot(p[1]) - (6.0 - p[2] / 6.0)).abs()
                            / (1.0_f64 + 1.0 / 36.0).sqrt()
                    }
                    _ => panic!("unexpected unchanged fitting carrier"),
                };
                assert!(
                    distance
                        <= descriptor.carrier_deviation_mm
                            + descriptor.quantization_error_mm
                            + 1e-10
                );
            }
            let [a, b, c] = points;
            // Complete projected-triangle distance: a radial hole cannot be
            // checked at vertices alone. The origin must remain outside every
            // triangle, with all three edges clear of the physical radius-two bore.
            let orient = |a: [f64; 3], b: [f64; 3]| a[0] * b[1] - a[1] * b[0];
            let signs = [orient(a, b), orient(b, c), orient(c, a)];
            assert!(!(signs.iter().all(|s| *s > 0.0) || signs.iter().all(|s| *s < 0.0)));
            for (a, b) in [(a, b), (b, c), (c, a)] {
                let d = [b[0] - a[0], b[1] - a[1]];
                let length = d[0] * d[0] + d[1] * d[1];
                let t = if length > 0.0 {
                    (-(a[0] * d[0] + a[1] * d[1]) / length).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                assert!(
                    (a[0] + t * d[0]).hypot(a[1] + t * d[1]) >= 2.0 - MESH_SURFACE_TOLERANCE_MM
                );
            }
        }
    }
}

#[test]
fn complete_native_trim_proof_rejects_missing_and_duplicate_on_carrier_triangles() {
    let definition = fixture("box-mm.step");
    let candidate::Candidates {
        edges, mut faces, ..
    } = planned(&definition, MESH_CARRIER_TOLERANCE_MM * 0.25);
    let original = faces.swap_remove(0);
    let outside = edges.retained_bytes() + original.retained_bytes();
    {
        audit::face(
            &definition,
            0,
            &original,
            &edges,
            MeshBudget::FROZEN,
            outside,
            &mut GeometryWorkBudget::new(20_000_000),
            &AtomicBool::new(false),
        )
        .expect("a complete independently qualified candidate is published on its own audit");
    }
    for duplicate in [false, true] {
        let mut candidate = original.clone();
        if duplicate {
            candidate.triangles.push(candidate.triangles[0]);
        } else {
            candidate.triangles.remove(0);
        }
        let error = audit::face(
            &definition,
            0,
            &candidate,
            &edges,
            MeshBudget::FROZEN,
            outside,
            &mut GeometryWorkBudget::new(20_000_000),
            &AtomicBool::new(false),
        )
        .err()
        .expect("invalid triangle chain must refuse");
        assert_eq!(error.code, GeometryErrorCode::InvalidGeometry);
    }
}

#[test]
fn housing_pole_lune_uses_coefficient_normals_and_refuses_unproved_aliases() {
    let definition = fixture("independent-exporters/occt-rounded-housing-mm.step");
    let ordinal = definition
        .faces()
        .iter()
        .position(|f| f.face_id.as_str() == "step:461")
        .unwrap();
    // The sphere's form chart puts its south pole on a whole domain row (the old degree chart's
    // v = -90); rows a ninth of the span above it are regular (the old v > -80).
    let domain = definition
        .brep()
        .face(definition.face_ids()[ordinal])
        .unwrap()
        .uv_domain;
    let pole_v = domain.min().y;
    let regular_v = pole_v + (domain.max().y - domain.min().y) / 9.0;
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(20_000_000);
    let mut attempt = 0;
    // Like production, this consumer acquires a certified candidate rather
    // than treating an incidental first grid as a required approximation.
    // Failed native generation and proof work share the same allowance.
    let (edges, candidate, audited, pole, shared) = loop {
        let candidate::Candidates {
            edges, mut faces, ..
        } = candidate::plan(
            &definition,
            MeshTolerance::display(MESH_CARRIER_TOLERANCE_MM * 0.25 * 0.25_f64.powi(attempt)),
            PROOF_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let outside = edges.retained_bytes();
        let mut candidate = faces.swap_remove(ordinal);
        let pole = candidate.uvs.iter().position(|q| q.y == pole_v).unwrap();
        let shared = candidate.shared[pole].unwrap();
        // Candidate fallback normals are neither required nor published at a
        // proved singularity. Publish the same-law outward field instead.
        for (i, owner) in candidate.shared.iter().enumerate() {
            if *owner == Some(shared) {
                candidate.normals[i] = solidcore::Vec3::new(1.0, 0.0, 0.0);
            }
        }
        match audit::face(
            &definition,
            ordinal,
            &candidate,
            &edges,
            MeshBudget::FROZEN,
            outside,
            &mut work,
            &cancel,
        ) {
            Ok(audited) => break (edges, candidate, audited, pole, shared),
            Err(cause) if cause.code == GeometryErrorCode::ShapeErrorExceeded && attempt < 8 => {
                attempt += 1;
            }
            Err(cause) => panic!("bounded housing pole display audit: {cause:?}"),
        }
    };
    assert!(
        crate::parametric::up(audited.carrier_error + audited.trim_error)
            <= MESH_CARRIER_TOLERANCE_MM,
        "regular patches, complete singular fans and source trim share the frozen native allowance",
    );
    for (i, owner) in candidate.shared.iter().enumerate() {
        if *owner == Some(shared) {
            assert!(audited.normals[i][0].abs() < 1e-10);
            assert!(audited.normals[i][1].abs() < 1e-10);
            assert!(audited.normals[i][2] < -0.999999);
        }
    }
    for mutation in 0..3 {
        let mut altered = candidate.clone();
        match mutation {
            0 => altered.uvs[pole].y = crate::parametric::up(pole_v),
            1 => {
                let regular = altered.uvs.iter().position(|q| q.y > regular_v).unwrap();
                altered.shared[regular] = Some(shared);
            }
            _ => {
                let regular = altered
                    .triangles
                    .iter()
                    .position(|t| {
                        t.iter()
                            .all(|i| altered.shared[*i as usize] != Some(shared))
                    })
                    .unwrap();
                altered.triangles.remove(regular);
            }
        }
        let error = audit::face(
            &definition,
            ordinal,
            &altered,
            &edges,
            MeshBudget::FROZEN,
            0,
            &mut GeometryWorkBudget::new(20_000_000),
            &cancel,
        )
        .err()
        .expect("a near pole, false shared alias or unowned hole must refuse");
        assert_eq!(error.code, GeometryErrorCode::InvalidGeometry);
    }
}

#[test]
fn face_slot_and_resource_refusals_never_return_partial_artifacts() {
    let definition = fixture("box-mm.step");
    let error = tessellate(
        &definition,
        DisplayProfile::MeshTrimmedMm005V2,
        &AtomicBool::new(false),
        MeshBudget {
            vertices: 1,
            triangles: 1,
            bytes: 152,
        },
    )
    .unwrap_err();
    assert!(matches!(
        error.code,
        GeometryErrorCode::ResourceLimit
            | GeometryErrorCode::ShapeErrorExceeded
            | GeometryErrorCode::InvalidGeometry
    ));
    assert!(
        tessellate(
            &definition,
            DisplayProfile::MeshTrimmedMm005V2,
            &AtomicBool::new(true),
            MeshBudget::FROZEN
        )
        .is_err()
    );
    assert!(
        tessellate(
            &definition,
            DisplayProfile::MeshTrimmedMm005V2,
            &AtomicBool::new(false),
            MeshBudget::FROZEN
        )
        .is_ok()
    );
}

#[test]
fn native_ruled_triangle_retains_full_height_under_display_carrier_bound() {
    let definition = fixture("independent-exporters/occt-spline-housing-mm.step");
    let ordinal = definition
        .faces()
        .iter()
        .position(|f| f.face_id.as_str() == "step:137")
        .unwrap();
    let cancel = AtomicBool::new(false);
    let mut work = GeometryWorkBudget::new(20_000_000);
    let candidate::Candidates {
        edges, mut faces, ..
    } = planned(&definition, MESH_CARRIER_TOLERANCE_MM * 0.25);
    let candidate = faces.swap_remove(ordinal);
    drop(edges);
    // A whole-height triangle on the ruled face's own chart: two neighbouring rulings, from one
    // end of the face to the other. The planner may split it; the proof must bound it whole.
    let surface = definition.face_surface(candidate.face).unwrap();
    let natural = surface.natural_uv_domain_if_built();
    let bounds = [
        [natural.u_min(), natural.u_max()],
        [natural.v_min(), natural.v_max()],
    ];
    let at = |q: [f64; 2], work: &mut GeometryWorkBudget| {
        crate::parametric::surface_enclosure(surface, [[q[0], q[0]], [q[1], q[1]]], work, &cancel)
            .unwrap()
            .position
            .map(crate::parametric::Interval::mid)
    };
    let corner_z = |q: [f64; 2], work: &mut GeometryWorkBudget| at(q, work)[2];
    let rise = [
        (corner_z([bounds[0][1], bounds[1][0]], &mut work)
            - corner_z([bounds[0][0], bounds[1][0]], &mut work))
        .abs(),
        (corner_z([bounds[0][0], bounds[1][1]], &mut work)
            - corner_z([bounds[0][0], bounds[1][0]], &mut work))
        .abs(),
    ];
    let height_axis = usize::from(rise[1] > rise[0]);
    let across = 1 - height_axis;
    let first = audit::triangle_uv(&candidate, candidate.triangles[0]).unwrap();
    let mut rulings: Vec<f64> = first.iter().map(|q| q[across]).collect();
    rulings.sort_by(f64::total_cmp);
    rulings.dedup();
    let corner = |ruling: f64, end: usize| {
        let mut q = [0.0; 2];
        q[across] = ruling;
        q[height_axis] = bounds[height_axis][end];
        q
    };
    let uv = [
        corner(rulings[0], 0),
        corner(rulings[1], 0),
        corner(rulings[1], 1),
    ];
    let points = uv.map(|q| at(q, &mut work));
    let triangle = [0_u32, 1, 2];
    let height = points
        .iter()
        .map(|p| p[2])
        .fold(f64::NEG_INFINITY, f64::max)
        - points.iter().map(|p| p[2]).fold(f64::INFINITY, f64::min);
    assert!(
        (height - 6.0).abs() < 1e-12,
        "the ruled triangle spans the face's full height: {points:?}"
    );
    let domain = std::array::from_fn(|axis| {
        [
            uv.iter().map(|p| p[axis]).fold(f64::INFINITY, f64::min),
            uv.iter().map(|p| p[axis]).fold(f64::NEG_INFINITY, f64::max),
        ]
    });
    let uvs: Vec<_> = uv
        .iter()
        .map(|q| solidcore::Point2::new(q[0], q[1]))
        .collect();
    let mut proof = crate::parametric::DisplayTriangleProof::with_mesh_inventory(
        surface,
        &uvs,
        vec![0],
        vec![crate::parametric::DisplayMeshBox::new(
            crate::parametric::DisplayMeshBox::key_for(domain),
        )],
        PROOF_BYTES - candidate.retained_bytes(),
        &mut work,
        &cancel,
    )
    .unwrap();
    let error = proof
        .indexed_triangle_error_bound(
            &uv,
            &points,
            0,
            &triangle.map(|i| i as usize),
            MESH_CARRIER_TOLERANCE_MM,
            &mut work,
            &cancel,
        )
        .unwrap();
    assert!(
        error <= MESH_CARRIER_TOLERANCE_MM,
        "native full-height carrier error {error}"
    );
}
