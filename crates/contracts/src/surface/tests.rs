// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::*;
fn layer(id: u32, scope: SurfaceScope, finish: SurfaceFinish) -> SurfaceLayer {
    SurfaceLayer {
        layer_id: SurfaceLayerId::new(id).unwrap(),
        enabled: true,
        scope,
        face_id: FaceId::from_step_entity(7).unwrap(),
        finish,
    }
}
fn ribs() -> SurfaceFinish {
    SurfaceFinish::VerticalRibs {
        depth_mm: 0.1,
        pitch_mm: 2.0,
        edge_margin_mm: 0.8,
        phase_mm: 0.0,
    }
}
#[test]
fn ordered_scopes_smooth_and_disabled_reveal_lower_fills() {
    let definition_id = DefinitionId::from_source_sha256(&[3; 32]);
    let occurrence_id = OccurrenceId::new(1).unwrap();
    let definition = SurfaceScope::Definition {
        definition_id: definition_id.clone(),
    };
    let occurrence = SurfaceScope::Occurrence {
        occurrence_id,
        expected_definition_id: definition_id.clone(),
    };
    let mut doc = SurfaceOverlays {
        schema_version: SURFACE_SCHEMA_VERSION,
        images: vec![],
        layers: vec![
            layer(1, occurrence.clone(), SurfaceFinish::Smooth),
            layer(2, definition.clone(), ribs()),
        ],
    };
    let face = FaceId::from_step_entity(7).unwrap();
    assert_eq!(
        doc.effective_layer(&definition_id, occurrence_id, &face)
            .map(|layer| &layer.finish),
        Some(&SurfaceFinish::Smooth)
    );
    doc.layers[0].enabled = false;
    assert_eq!(
        doc.effective_layer(&definition_id, occurrence_id, &face)
            .map(|layer| &layer.finish),
        Some(&ribs())
    );
    doc.layers.push(layer(3, definition, SurfaceFinish::Smooth));
    assert_eq!(
        doc.effective_layer(&definition_id, occurrence_id, &face)
            .map(|layer| &layer.finish),
        Some(&SurfaceFinish::Smooth)
    );
    doc.layers.pop();
    assert_eq!(
        doc.effective_layer(&definition_id, occurrence_id, &face)
            .map(|layer| &layer.finish),
        Some(&ribs())
    );
    let other = OccurrenceId::new(2).unwrap();
    assert_eq!(
        doc.effective_layer(&definition_id, other, &face)
            .map(|layer| &layer.finish),
        Some(&ribs())
    );
    assert_eq!(
        doc.effective_layer(&definition_id, other, &FaceId::from_step_entity(8).unwrap()),
        None
    );
    let wire: crate::rpc::SurfaceOverlays = doc.clone().try_into().unwrap();
    assert_eq!(SurfaceOverlays::try_from(wire).unwrap(), doc);
    assert_eq!(
        serde_json::from_slice::<SurfaceOverlays>(&serde_json::to_vec(&doc).unwrap()).unwrap(),
        doc
    );
}
#[test]
fn checked_ids_parameters_presence_and_allocation_bounds_reject_invalid_documents() {
    assert!(SurfaceLayerId::new(0).is_err());
    assert!(serde_json::from_str::<SurfaceLayerId>("0").is_err());
    for finish in [
        SurfaceFinish::VerticalRibs {
            depth_mm: f64::NAN,
            pitch_mm: 2.0,
            edge_margin_mm: 1.0,
            phase_mm: 0.0,
        },
        SurfaceFinish::VerticalRibs {
            depth_mm: 0.1,
            pitch_mm: 0.0,
            edge_margin_mm: 1.0,
            phase_mm: 0.0,
        },
        SurfaceFinish::VerticalRibs {
            depth_mm: 0.1,
            pitch_mm: 2.0,
            edge_margin_mm: -1.0,
            phase_mm: 0.0,
        },
        SurfaceFinish::VerticalRibs {
            depth_mm: 0.1,
            pitch_mm: 2.0,
            edge_margin_mm: 1.0,
            phase_mm: 2.0,
        },
    ] {
        assert!(finish.validate().is_err());
        assert!(crate::rpc::SurfaceFinish::try_from(finish).is_err());
    }
    assert!(SurfaceFinish::try_from(crate::rpc::SurfaceFinish { kind: None }).is_err());
    assert!(SurfaceScope::try_from(crate::rpc::SurfaceScope { kind: None }).is_err());
    assert!(
        serde_json::from_str::<SurfaceOverlays>(r#"{"schema_version":0,"layers":[]}"#).is_err()
    );
    assert!(serde_json::from_str::<SurfaceOverlays>(r#"{"schema_version":2}"#).is_err());
    assert!(
        serde_json::from_str::<SurfaceOverlays>(r#"{"schema_version":1,"layers":[]}"#).is_err()
    );
    assert!(
        SurfaceOverlays::try_from(crate::rpc::SurfaceOverlays {
            schema_version: 1,
            images: vec![],
            layers: vec![],
        })
        .is_err()
    );
    let scope = SurfaceScope::Definition {
        definition_id: DefinitionId::from_source_sha256(&[1; 32]),
    };
    let sample = layer(1, scope, ribs());
    let mut doc = SurfaceOverlays {
        schema_version: SURFACE_SCHEMA_VERSION,
        images: vec![],
        layers: vec![sample.clone(), sample.clone()],
    };
    assert!(doc.validate().is_err());
    doc.layers = Vec::with_capacity(MAX_SURFACE_LAYERS + 1);
    assert_eq!(
        doc.validate().unwrap_err().code,
        ProjectErrorCode::ResourceLimit
    );
    let repeated = vec![serde_json::to_value(sample).unwrap(); MAX_SURFACE_LAYERS + 1];
    let error = serde_json::from_value::<SurfaceOverlays>(
        serde_json::json!({"schema_version":SURFACE_SCHEMA_VERSION,"images":[],"layers":repeated}),
    )
    .unwrap_err();
    assert!(error.is_data());
    let mut face = String::with_capacity(MAX_SURFACE_DOCUMENT_BYTES + 1);
    face.push_str("step:7");
    let mut inflated = layer(
        1,
        SurfaceScope::Definition {
            definition_id: DefinitionId::from_source_sha256(&[1; 32]),
        },
        ribs(),
    );
    inflated.face_id = FaceId::parse(face).unwrap();
    doc.layers = vec![inflated];
    assert_eq!(
        doc.validate().unwrap_err().code,
        ProjectErrorCode::ResourceLimit
    );
}

#[test]
fn cylindrical_ribs_check_integer_count_depth_phase_and_typed_roundtrip() {
    for count in [2, 256] {
        for phase in [0.0, 0.999999] {
            let finish = SurfaceFinish::CylindricalRibs {
                depth_mm: 0.1,
                rib_count: count,
                phase_fraction: phase,
            };
            let wire = crate::rpc::SurfaceFinish::try_from(finish.clone()).unwrap();
            assert_eq!(SurfaceFinish::try_from(wire).unwrap(), finish);
            let json = serde_json::to_string(&finish).unwrap();
            assert_eq!(
                serde_json::from_str::<SurfaceFinish>(&json).unwrap(),
                finish
            );
        }
    }
    for (depth, count, phase) in [
        (f64::NAN, 16, 0.0),
        (f64::INFINITY, 16, 0.0),
        (0.0, 16, 0.0),
        (-0.1, 16, 0.0),
        (0.1, 0, 0.0),
        (0.1, 1, 0.0),
        (0.1, 257, 0.0),
        (0.1, u32::MAX, 0.0),
        (0.1, 16, -0.1),
        (0.1, 16, 1.0),
        (0.1, 16, f64::NAN),
        (0.1, 16, f64::INFINITY),
    ] {
        let finish = SurfaceFinish::CylindricalRibs {
            depth_mm: depth,
            rib_count: count,
            phase_fraction: phase,
        };
        assert!(finish.validate().is_err());
        assert!(crate::rpc::SurfaceFinish::try_from(finish).is_err());
        let wire = crate::rpc::SurfaceFinish {
            kind: Some(crate::rpc::surface_finish::Kind::CylindricalRibs(
                crate::rpc::SurfaceCylindricalRibs {
                    depth_mm: depth,
                    rib_count: count,
                    phase_fraction: phase,
                },
            )),
        };
        assert!(SurfaceFinish::try_from(wire).is_err());
    }
    for json in [
        r#"{"kind":"cylindrical_ribs","depth_mm":0.1,"rib_count":2.5,"phase_fraction":0}"#,
        r#"{"kind":"cylindrical_ribs","depth_mm":0.1,"rib_count":16}"#,
        r#"{"kind":"cylindrical_ribs","depth_mm":0.1,"rib_count":16,"phase_fraction":0,"radius_mm":5}"#,
    ] {
        assert!(serde_json::from_str::<SurfaceFinish>(json).is_err());
    }
}

fn image_fixture() -> (SurfaceImageRecord, Vec<u8>) {
    let bytes = [b"P5\n2 2\n255\n".as_slice(), &[0, 64, 128, 255]].concat();
    (
        SurfaceImageRecord {
            image_hash: SourceHash::from_bytes(&bytes),
            width: 2,
            height: 2,
            byte_count: bytes.len() as u64,
            source_hash: SourceHash::from_bytes(b"encoded source"),
            source_name: "height.png".into(),
        },
        bytes,
    )
}
fn image_parameters() -> ImageReliefParameters {
    ImageReliefParameters {
        depth_mm: 1.2,
        u_repeats: 3,
        u_phase_fraction: 0.125,
        z_period_mm: 2.7,
        z_phase_mm: 0.4,
        edge_margin_mm: 0.8,
        vertical_error_mm: 0.025,
    }
}
#[test]
fn canonical_image_hash_includes_dimensions_and_borrows_exact_pixels() {
    let (record, bytes) = image_fixture();
    assert_eq!(
        record.image_hash.as_str(),
        "72da4da49413259d1c478e37535cf866b981511b8142d25ce5482f4104e45a6f"
    );
    let view = record.validate_content(&bytes).unwrap();
    assert_eq!((view.width, view.height), (2, 2));
    assert_eq!(view.pixels, &[0, 64, 128, 255]);
    assert_eq!(view.canonical_bytes.as_ptr(), bytes.as_ptr());
    assert_eq!(view.pixels.as_ptr(), bytes[11..].as_ptr());
    let pixels = vec![73u8; 6];
    let a = [b"P5\n2 3\n255\n".as_slice(), &pixels].concat();
    let b = [b"P5\n3 2\n255\n".as_slice(), &pixels].concat();
    assert_ne!(SourceHash::from_bytes(&a), SourceHash::from_bytes(&b));
    let mut provenance = record.clone();
    provenance.source_hash = SourceHash::from_bytes(b"different PNG color/alpha encoding");
    provenance.source_name = "normalized-equivalent.pgm".into();
    assert_eq!(
        provenance.validate_content(&bytes).unwrap().pixels,
        view.pixels
    );
    assert_eq!(provenance.image_hash, record.image_hash); // Encoded provenance is not canonical content identity.
}
#[test]
fn canonical_image_refuses_noncanonical_headers_trailing_bytes_and_corruption() {
    let (record, bytes) = image_fixture();
    for mut malformed in [
        [b"P5\n02 2\n255\n".as_slice(), &[0, 64, 128, 255]].concat(),
        [b"P5\n2 2\n254\n".as_slice(), &[0, 64, 128, 255]].concat(),
        [b"P5\n#comment\n2 2\n255\n".as_slice(), &[0, 64, 128, 255]].concat(),
        bytes[..bytes.len() - 1].to_vec(),
        [bytes.as_slice(), &[0]].concat(),
    ] {
        assert!(record.validate_content(&malformed).is_err());
        malformed[0] ^= 1;
        assert!(record.validate_content(&malformed).is_err());
    }
    for header in [b"P2\n2 2\n255\n", b"P5\n2 2\n254\n", b"P5\n2\t2\n255\n"] {
        let malformed = [header.as_slice(), &[0, 64, 128, 255]].concat();
        let mut rehashed = record.clone();
        rehashed.image_hash = SourceHash::from_bytes(&malformed);
        assert!(rehashed.validate_content(&malformed).is_err()); // Header canonicality is independent of hash correctness.
    }
    let mut invalid_metadata = serde_json::to_value(&record).unwrap();
    invalid_metadata["height"] = serde_json::json!(257);
    assert!(serde_json::from_value::<SurfaceImageRecord>(invalid_metadata).is_err());
    let mut changed = bytes.clone();
    *changed.last_mut().unwrap() ^= 1;
    assert_eq!(
        record.validate_content(&changed).unwrap_err().code,
        ProjectErrorCode::CorruptAsset
    );
    let mut wrong = record.clone();
    wrong.width = 3;
    assert!(wrong.validate_content(&bytes).is_err());
    for size in [0, 1, 257, u32::MAX] {
        let mut invalid = record.clone();
        invalid.height = size;
        assert!(invalid.validate().is_err());
    }
    let mut invalid = record.clone();
    invalid.byte_count = u64::MAX;
    assert!(invalid.validate().is_err());
    let mut invalid = record.clone();
    invalid.source_name = "é".repeat(129);
    assert!(invalid.validate().is_err());
    let mut invalid = record;
    invalid.source_name = "height\n.png".into();
    assert!(invalid.validate().is_err());
}
#[test]
fn image_metadata_is_exact_sorted_used_authority_even_for_disabled_layers() {
    let (record, _) = image_fixture();
    let mut doc = SurfaceOverlays {
        schema_version: SURFACE_SCHEMA_VERSION,
        images: vec![record.clone()],
        layers: vec![layer(
            1,
            SurfaceScope::Definition {
                definition_id: DefinitionId::from_source_sha256(&[3; 32]),
            },
            SurfaceFinish::ImageRelief {
                image_hash: record.image_hash.clone(),
                parameters: image_parameters(),
            },
        )],
    };
    doc.layers[0].enabled = false;
    let mut inflated = doc.clone();
    let mut hash_storage = String::with_capacity(MAX_SURFACE_DOCUMENT_BYTES + 1);
    hash_storage.push_str(record.source_hash.as_str());
    inflated.images[0].source_hash = SourceHash::parse(hash_storage).unwrap();
    assert_eq!(
        inflated.validate().unwrap_err().code,
        ProjectErrorCode::ResourceLimit
    );
    doc.validate().unwrap();
    assert_eq!(
        SurfaceOverlays::try_from(crate::rpc::SurfaceOverlays::try_from(doc.clone()).unwrap())
            .unwrap(),
        doc
    );
    let mut missing = doc.clone();
    missing.images.clear();
    assert!(missing.validate().is_err());
    let mut orphan = doc.clone();
    orphan.layers.clear();
    assert!(orphan.validate().is_err());
    let mut duplicate = doc.clone();
    duplicate.images.push(record.clone());
    assert!(duplicate.validate().is_err());
    let mut unknown = doc.clone();
    unknown.layers[0].finish = SurfaceFinish::ImageRelief {
        image_hash: SourceHash::from_bytes(b"unknown"),
        parameters: image_parameters(),
    };
    assert!(unknown.validate().is_err());
    let mut second = record;
    second.image_hash = SourceHash::from_bytes(b"other canonical image");
    doc.images.push(second.clone());
    doc.layers.push(layer(
        2,
        doc.layers[0].scope.clone(),
        SurfaceFinish::ImageRelief {
            image_hash: second.image_hash,
            parameters: image_parameters(),
        },
    ));
    doc.images.sort_by(|a, b| a.image_hash.cmp(&b.image_hash));
    doc.validate().unwrap();
    doc.images.reverse();
    assert!(doc.validate().is_err());
    assert!(
        serde_json::from_str::<SurfaceOverlays>(r#"{"schema_version":3,"layers":[]}"#).is_err()
    );
    assert!(
        serde_json::from_str::<SurfaceOverlays>(r#"{"schema_version":2,"images":[],"layers":[]}"#)
            .is_err()
    );
}
#[test]
fn image_content_table_is_bounded_and_exact_not_a_metadata_substitute() {
    let (record, bytes) = image_fixture();
    let content = SurfaceImageContent {
        image_hash: record.image_hash.clone(),
        bytes: bytes.into(),
    };
    validate_image_contents(
        std::slice::from_ref(&record),
        std::slice::from_ref(&content),
    )
    .unwrap();
    assert!(validate_image_contents(std::slice::from_ref(&record), &[]).is_err());
    assert!(validate_image_contents(&[], std::slice::from_ref(&content)).is_err());
    let mut mismatch = content.clone();
    mismatch.image_hash = SourceHash::from_bytes(b"other");
    assert!(validate_image_contents(std::slice::from_ref(&record), &[mismatch]).is_err());
    let mut corrupt = content.clone();
    std::sync::Arc::make_mut(&mut corrupt.bytes)[11] ^= 1;
    assert!(validate_image_contents(std::slice::from_ref(&record), &[corrupt]).is_err());
    let mut oversized = serde_json::to_value(&content).unwrap();
    oversized["bytes"] = serde_json::json!(vec![0u8; MAX_SURFACE_IMAGE_BYTES + 1]);
    assert!(serde_json::from_value::<SurfaceImageContent>(oversized).is_err());
    let mut inflated = content;
    std::sync::Arc::make_mut(&mut inflated.bytes).reserve_exact(MAX_SURFACE_IMAGE_BYTES + 1);
    assert_eq!(
        validate_image_contents(std::slice::from_ref(&record), &[inflated])
            .unwrap_err()
            .code,
        ProjectErrorCode::ResourceLimit
    );
    #[derive(Deserialize)]
    struct Table {
        #[serde(deserialize_with = "bounded_image_contents")]
        images: Vec<SurfaceImageContent>,
    }
    let maximum = SurfaceImageContent {
        image_hash: SourceHash::from_bytes(b"maximum"),
        bytes: vec![0; MAX_SURFACE_IMAGE_BYTES].into(),
    };
    let encoded = serde_json::to_string(&maximum).unwrap();
    let full = format!(
        "{{\"images\":[{}]}}",
        vec![encoded; MAX_SURFACE_IMAGES].join(",")
    );
    assert!(serde_json::from_str::<Table>(&full).is_err()); // Sixteen maximal PGMs exceed 1MiB, even though each fits.
    let empty: Table = serde_json::from_str(r#"{"images":[]}"#).unwrap();
    assert!(empty.images.is_empty());
}
#[test]
fn image_mapping_has_explicit_units_phases_and_no_legacy_defaults() {
    let parameters = image_parameters();
    parameters.validate().unwrap();
    assert_eq!(
        ImageReliefParameters::try_from(
            crate::rpc::ImageReliefParameters::try_from(parameters.clone()).unwrap()
        )
        .unwrap(),
        parameters
    );
    for field in [
        "depth_mm",
        "u_repeats",
        "u_phase_fraction",
        "z_period_mm",
        "z_phase_mm",
        "edge_margin_mm",
        "vertical_error_mm",
    ] {
        let mut value = serde_json::to_value(&parameters).unwrap();
        value.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<ImageReliefParameters>(value).is_err());
    }
    for value in [f64::NAN, f64::INFINITY, 0.0, -1.0] {
        let mut p = parameters.clone();
        p.depth_mm = value;
        assert!(p.validate().is_err());
        let mut p = parameters.clone();
        p.z_period_mm = value;
        assert!(p.validate().is_err());
        let mut p = parameters.clone();
        p.vertical_error_mm = value;
        assert!(p.validate().is_err());
    }
    for count in [0, 257, u32::MAX] {
        let mut p = parameters.clone();
        p.u_repeats = count;
        assert!(p.validate().is_err());
    }
    for phase in [-0.1, 1.0, f64::NAN] {
        let mut p = parameters.clone();
        p.u_phase_fraction = phase;
        assert!(p.validate().is_err());
    }
    let mut p = parameters.clone();
    p.z_phase_mm = p.z_period_mm;
    assert!(p.validate().is_err());
    let mut p = parameters.clone();
    p.edge_margin_mm = -0.1;
    assert!(p.validate().is_err());
    let mut p = parameters;
    p.edge_margin_mm = 0.0;
    p.depth_mm = 10.0;
    p.validate().unwrap(); // Carrier and support admission, not half-bead rib restrictions, own these values.
}
