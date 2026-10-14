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
        layers: vec![
            layer(1, occurrence.clone(), SurfaceFinish::Smooth),
            layer(2, definition.clone(), ribs()),
        ],
    };
    let face = FaceId::from_step_entity(7).unwrap();
    assert_eq!(
        doc.effective_finish(&definition_id, occurrence_id, &face),
        Some(&SurfaceFinish::Smooth)
    );
    doc.layers[0].enabled = false;
    assert_eq!(
        doc.effective_finish(&definition_id, occurrence_id, &face),
        Some(&ribs())
    );
    doc.layers.push(layer(3, definition, SurfaceFinish::Smooth));
    assert_eq!(
        doc.effective_finish(&definition_id, occurrence_id, &face),
        Some(&SurfaceFinish::Smooth)
    );
    doc.layers.pop();
    assert_eq!(
        doc.effective_finish(&definition_id, occurrence_id, &face),
        Some(&ribs())
    );
    let other = OccurrenceId::new(2).unwrap();
    assert_eq!(
        doc.effective_finish(&definition_id, other, &face),
        Some(&ribs())
    );
    assert_eq!(
        doc.effective_finish(&definition_id, other, &FaceId::from_step_entity(8).unwrap()),
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
        serde_json::json!({"schema_version":SURFACE_SCHEMA_VERSION,"layers":repeated}),
    )
    .unwrap_err();
    assert!(error.to_string().contains("surface layer budget"));
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
