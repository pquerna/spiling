// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;

fn intent() -> ManufacturingIntent {
    ManufacturingIntent {
        printer: PrinterSpecification {
            schema_version: 1,
            id: "software-cartesian".into(),
            revision: "1".into(),
            name: "Software only".into(),
            coordinate_frame: PrinterCoordinateFrame::RightHandedMillimetres,
            build_envelope: AabbMm {
                min: [0.0, 0.0, 5.0],
                max: [200.0, 200.0, 205.0],
            },
            components: vec![
                MachineComponent {
                    id: "bed".into(),
                    role: MachineComponentRole::Bed,
                    pose: RigidPoseMm::IDENTITY,
                    shape: MachineComponentShape::Box {
                        bounds_mm: AabbMm {
                            min: [0.0, 0.0, 0.0],
                            max: [200.0, 200.0, 5.0],
                        },
                    },
                },
                MachineComponent {
                    id: "head".into(),
                    role: MachineComponentRole::Toolhead,
                    pose: RigidPoseMm::IDENTITY,
                    shape: MachineComponentShape::Cylinder {
                        radius_mm: 2.0,
                        height_mm: 10.0,
                    },
                },
            ],
            capabilities: PrinterCapabilities {
                motion_space: MotionSpace::CartesianXyz,
                extruders: 1,
                output_dialect: OutputDialect::CartesianAbsoluteGcodeV1,
                generated_supports: false,
            },
            nozzle_diameter_mm: 0.4,
            max_feed_mm_s: 150.0,
            max_volumetric_flow_mm3_s: 15.0,
        },
        recipe: PlanarPrintRecipe {
            id: "solid".into(),
            revision: "1".into(),
            material_id: "nominal".into(),
            material_revision: "1".into(),
            filament_diameter_mm: 1.75,
            layer_height_mm: 0.2,
            bead_width_mm: 0.45,
            perimeter_count: 2,
            infill_fraction: 1.0,
            print_speed_mm_s: 30.0,
            travel_speed_mm_s: 100.0,
            flow_multiplier: 1.0,
        },
    }
}
fn inputs() -> (ProjectId, Vec<StoredDefinition>, Vec<OccurrenceRecord>) {
    let hash = SourceHash::from_bytes(b"authoritative source bytes");
    let digest: [u8; 32] = Sha256::digest(b"authoritative source bytes").into();
    let id = DefinitionId::from_source_sha256(&digest);
    (
        ProjectId::parse("8b4894f6-76b5-453c-a431-34a47f5d6c1f").unwrap(),
        vec![StoredDefinition {
            definition_id: id.clone(),
            provenance: SourceProvenance {
                source_hash: hash,
                source_name: "part.step".into(),
                source_unit: SourceUnit::Millimetre,
                uncertainty_mm: Some(0.001),
            },
        }],
        vec![OccurrenceRecord {
            occurrence_id: OccurrenceId::new(1).unwrap(),
            definition_id: id,
            pose: RigidPoseMm::IDENTITY,
        }],
    )
}
fn plan() -> NormalizedPrintPlan {
    NormalizedPrintPlan {
        schema_version: 1,
        sampling_tolerance_mm: 0.005,
        offset_tolerance_mm: 0.005,
        layer_quantization_mm: 0.000001,
        layers: vec![PrintLayer {
            index: 0,
            z_mm: 5.2,
            section_z_mm: 5.1,
            paths: vec![DepositionPath {
                kind: DepositionKind::Perimeter,
                points_mm: vec![
                    [10.0, 10.0, 5.2],
                    [20.0, 10.0, 5.2],
                    [20.0, 20.0, 5.2],
                    [10.0, 10.0, 5.2],
                ],
            }],
        }],
    }
}
fn bundle() -> ManufacturingBundle {
    let (project_id, definitions, occurrences) = inputs();
    let intent = intent();
    let input_hash = input_fingerprint(&project_id, &definitions, &occurrences, &intent).unwrap();
    ManufacturingBundle {
        schema_version: 1,
        provenance: ManufacturingProvenance {
            project_id,
            input_revision: ProjectRevision(9),
            input_hash,
            definitions,
            occurrences,
            intent,
            kernel: KernelIdentity {
                name: "native".into(),
                version: "1".into(),
                revision: "pinned".into(),
            },
            planner_version: "1".into(),
            emitter_version: "1".into(),
            verifier_version: "1".into(),
        },
        plan: plan(),
        program: "; arbitrary bytes do not imply replay success\n".into(),
        verification: VerificationReport {
            verified: false,
            coverage: vec!["schema".into()],
            limitations: vec!["no physics".into()],
            deposition_segments: 3,
            travel_segments: 1,
            deposited_volume_mm3: 1.0,
            filament_length_mm: 0.4,
            max_position_error_mm: 0.0,
            max_extrusion_error_mm: 0.0,
        },
    }
}

#[test]
fn strict_authoring_json_requires_every_explicit_profile_and_recipe_field() {
    let value = serde_json::to_value(intent()).unwrap();
    assert_eq!(
        value["printer"]["coordinate_frame"],
        "right_handed_millimetres"
    );
    assert_eq!(value["printer"]["components"][0]["shape"]["kind"], "box");
    assert_eq!(
        value["printer"]["components"][1]["shape"]["kind"],
        "cylinder"
    );
    for scope in ["printer", "recipe"] {
        for field in value[scope].as_object().unwrap().keys() {
            let mut changed = value.clone();
            changed[scope].as_object_mut().unwrap().remove(field);
            assert!(
                serde_json::from_value::<ManufacturingIntent>(changed).is_err(),
                "missing {scope}.{field}"
            );
        }
        let mut changed = value.clone();
        changed[scope]["unknown"] = serde_json::json!(true);
        assert!(serde_json::from_value::<ManufacturingIntent>(changed).is_err());
    }
    let bytes = serde_json::to_vec(&intent()).unwrap();
    assert_eq!(decode_intent(&bytes).unwrap(), intent());
    let mut duplicate = String::from_utf8(bytes).unwrap();
    duplicate = duplicate.replacen(
        "\"schema_version\":1",
        "\"schema_version\":1,\"schema_version\":1",
        1,
    );
    assert!(decode_intent(duplicate.as_bytes()).is_err());
}

#[test]
fn profiles_reject_nonfinite_ranges_capabilities_flow_and_duplicate_components() {
    let valid = intent();
    valid.validate().unwrap();
    let mut changed = valid.clone();
    changed.recipe.layer_height_mm = f64::NAN;
    assert!(changed.validate().is_err());
    changed = valid.clone();
    changed.recipe.flow_multiplier = f64::INFINITY;
    assert!(changed.validate().is_err());
    changed = valid.clone();
    changed.recipe.perimeter_count = 0;
    assert!(changed.validate().is_err());
    changed = valid.clone();
    changed.recipe.layer_height_mm = changed.recipe.bead_width_mm + 0.1;
    assert!(changed.validate().is_err());
    changed = valid.clone();
    changed.recipe.infill_fraction = 0.5;
    assert_eq!(
        changed.validate().unwrap_err().code,
        ManufacturingErrorCode::UnsupportedCapability
    );
    changed = valid.clone();
    changed.printer.capabilities.extruders = 2;
    assert_eq!(
        changed.validate().unwrap_err().code,
        ManufacturingErrorCode::UnsupportedCapability
    );
    changed = valid.clone();
    changed.printer.capabilities.generated_supports = true;
    assert_eq!(
        changed.validate().unwrap_err().code,
        ManufacturingErrorCode::UnsupportedCapability
    );
    changed = valid.clone();
    changed.recipe.travel_speed_mm_s = 151.0;
    assert!(changed.validate().is_err());
    changed = valid.clone();
    changed.printer.max_volumetric_flow_mm3_s = 0.001;
    assert!(changed.validate().is_err());
    changed = valid.clone();
    changed
        .printer
        .components
        .push(changed.printer.components[0].clone());
    assert!(changed.validate().is_err());
    changed = valid;
    changed.printer.components[1].shape = MachineComponentShape::Cylinder {
        radius_mm: -1.0,
        height_mm: 10.0,
    };
    assert!(changed.validate().is_err());
}

#[test]
fn fixed_layer_paths_reject_implicit_closure_zero_nonfinite_and_out_of_frame_moves() {
    let intent = intent();
    let valid = plan();
    valid.validate(&intent).unwrap();
    let mut changed = valid.clone();
    changed.layers[0].paths[0].points_mm.pop();
    assert!(changed.validate(&intent).is_err());
    changed = valid.clone();
    changed.layers[0].paths[0].points_mm[1] = changed.layers[0].paths[0].points_mm[0];
    assert!(changed.validate(&intent).is_err());
    changed = valid.clone();
    changed.layers[0].paths[0].points_mm[1][0] = f64::INFINITY;
    assert!(changed.validate(&intent).is_err());
    changed = valid.clone();
    changed.layers[0].paths[0].points_mm[1][2] += 0.01;
    assert!(changed.validate(&intent).is_err());
    changed = valid.clone();
    changed.layers[0].paths[0].points_mm[1][0] = 201.0;
    assert!(changed.validate(&intent).is_err());
    changed = valid.clone();
    changed.layers[0].index = 1;
    assert!(changed.validate(&intent).is_err());
    changed = valid.clone();
    changed.layers[0].section_z_mm = changed.layers[0].z_mm;
    assert!(changed.validate(&intent).is_err());
    changed = valid;
    changed.layers[0].z_mm -= 5.0;
    assert!(changed.validate(&intent).is_err());
}

#[test]
fn fingerprint_is_sorted_and_sensitive_to_authoritative_semantics() {
    let (project, definitions, mut occurrences) = inputs();
    let intent = intent();
    let mut second = occurrences[0].clone();
    second.occurrence_id = OccurrenceId::new(2).unwrap();
    second.pose.translation_mm[0] = 10.0;
    occurrences.push(second);
    let original = input_fingerprint(&project, &definitions, &occurrences, &intent).unwrap();
    occurrences.reverse();
    assert_eq!(
        original,
        input_fingerprint(&project, &definitions, &occurrences, &intent).unwrap()
    );
    occurrences[0].pose.translation_mm[0] += 1.0;
    assert_ne!(
        original,
        input_fingerprint(&project, &definitions, &occurrences, &intent).unwrap()
    );
    occurrences[0].pose.translation_mm[0] -= 1.0;
    let mut renamed = definitions.clone();
    renamed[0].provenance.source_name = "retained-label.step".into();
    assert_ne!(
        original,
        input_fingerprint(&project, &renamed, &occurrences, &intent).unwrap()
    );
    let mut units = definitions.clone();
    units[0].provenance.source_unit = SourceUnit::Inch;
    assert_ne!(
        original,
        input_fingerprint(&project, &units, &occurrences, &intent).unwrap()
    );
    let mut changed_intent = intent.clone();
    changed_intent.recipe.flow_multiplier = 1.01;
    assert_ne!(
        original,
        input_fingerprint(&project, &definitions, &occurrences, &changed_intent).unwrap()
    );
    let mut invalid = definitions.clone();
    invalid[0].provenance.source_hash = SourceHash::from_bytes(b"foreign");
    assert!(input_fingerprint(&project, &invalid, &occurrences, &intent).is_err());
    occurrences.push(occurrences[0].clone());
    assert!(input_fingerprint(&project, &definitions, &occurrences, &intent).is_err());
}

#[test]
fn bundle_schema_validation_never_confers_replay_trust_and_rejects_tampered_records() {
    let valid = bundle();
    valid.validate().unwrap();
    assert!(!valid.verification.verified);
    let bytes = serde_json::to_vec(&valid).unwrap();
    assert_eq!(decode_bundle(&bytes).unwrap(), valid);
    let mut record = ManufacturingArtifactRecord {
        hash: SourceHash::from_bytes(&bytes),
        input_hash: valid.provenance.input_hash.clone(),
        byte_count: bytes.len() as u32,
        summary: valid.summary(),
    };
    valid.validate_record(&record).unwrap();
    let mut later_revision = valid.clone();
    later_revision.provenance.input_revision = ProjectRevision(100);
    later_revision.validate().unwrap();
    assert_eq!(
        later_revision.provenance.input_hash,
        valid.provenance.input_hash
    );
    record.summary.software_only = false;
    assert!(valid.validate_record(&record).is_err());
    record.summary = valid.summary();
    record.input_hash = SourceHash::from_bytes(b"old inputs");
    assert_eq!(
        valid.validate_record(&record).unwrap_err().code,
        ManufacturingErrorCode::CorruptArtifact
    );
    let mut changed = valid;
    changed.provenance.intent.recipe.print_speed_mm_s += 1.0;
    assert_eq!(
        changed.validate().unwrap_err().code,
        ManufacturingErrorCode::CorruptArtifact
    );
}

#[test]
fn manifest_artifact_must_reference_current_explicit_intent_and_input_hash() {
    let bundle = bundle();
    let p = &bundle.provenance;
    let bytes = serde_json::to_vec(&bundle).unwrap();
    let mut manifest = ProjectManifest {
        format_version: 2,
        project_id: p.project_id.clone(),
        revision: ProjectRevision(10),
        units: ProjectUnits::Millimetres,
        frame: ProjectFrame::RightHanded,
        next_occurrence: 2,
        definitions: p.definitions.clone(),
        occurrences: p.occurrences.clone(),
        manufacturing_intent: Some((p.intent.clone()).into()),
        manufacturing_artifact: Some(ManufacturingArtifactRecord {
            hash: SourceHash::from_bytes(&bytes),
            input_hash: p.input_hash.clone(),
            byte_count: bytes.len() as u32,
            summary: bundle.summary(),
        }),
    };
    manifest.validate().unwrap();
    Arc::make_mut(manifest.manufacturing_intent.as_mut().unwrap())
        .recipe
        .print_speed_mm_s += 1.0;
    assert!(manifest.validate().is_err());
    manifest.manufacturing_intent = None;
    assert!(manifest.validate().is_err());
}

#[test]
fn resource_and_utf8_error_boundaries_are_enforced() {
    let text = "界".repeat(MAX_MANUFACTURING_ERROR_BYTES);
    let bounded = ManufacturingError::new(ManufacturingErrorCode::Io, &text);
    assert!(bounded.message.len() <= MAX_MANUFACTURING_ERROR_BYTES);
    assert!(text.starts_with(&bounded.message));
    assert!(
        serde_json::from_value::<ManufacturingError>(
            serde_json::json!({"code":"io","message":text})
        )
        .is_err()
    );
    assert_eq!(
        decode_intent(&vec![b' '; MAX_PROFILE_JSON_BYTES as usize + 1])
            .unwrap_err()
            .code,
        ManufacturingErrorCode::ResourceLimit
    );
    assert_eq!(
        decode_bundle(&vec![b' '; MAX_MANUFACTURING_BUNDLE_BYTES as usize + 1])
            .unwrap_err()
            .code,
        ManufacturingErrorCode::ResourceLimit
    );
    let mut too_many = plan();
    too_many.layers = vec![too_many.layers[0].clone(); MAX_PRINT_LAYERS as usize + 1];
    assert_eq!(
        too_many.validate(&intent()).unwrap_err().code,
        ManufacturingErrorCode::ResourceLimit
    );
    let mut components = intent();
    components.printer.components =
        vec![components.printer.components[0].clone(); MAX_MACHINE_COMPONENTS as usize + 1];
    assert_eq!(
        components.validate().unwrap_err().code,
        ManufacturingErrorCode::ResourceLimit
    );
    let mut segments = plan();
    segments.layers[0].paths[0].kind = DepositionKind::SolidFill;
    segments.layers[0].paths[0].points_mm =
        vec![[10.0, 10.0, 5.2]; MAX_DEPOSITION_SEGMENTS as usize + 2];
    assert_eq!(
        segments.validate(&intent()).unwrap_err().code,
        ManufacturingErrorCode::ResourceLimit
    );
}

#[test]
fn untrusted_json_collections_are_rejected_before_unbounded_growth() {
    let mut value = serde_json::to_value(intent()).unwrap();
    let component = value["printer"]["components"][0].clone();
    value["printer"]["components"] =
        serde_json::Value::Array(vec![component; MAX_MACHINE_COMPONENTS as usize + 1]);
    assert!(serde_json::from_value::<ManufacturingIntent>(value).is_err());
    let mut value = serde_json::to_value(bundle()).unwrap();
    let layer = value["plan"]["layers"][0].clone();
    value["plan"]["layers"] = serde_json::Value::Array(vec![layer; MAX_PRINT_LAYERS as usize + 1]);
    assert!(serde_json::from_value::<ManufacturingBundle>(value).is_err());
}

fn segmented_plan(segment_counts: &[&[usize]]) -> NormalizedPrintPlan {
    let mut result = plan();
    result.layers = segment_counts
        .iter()
        .enumerate()
        .map(|(index, counts)| {
            let z_mm = 5.0 + (index + 1) as f64 * 0.2;
            PrintLayer {
                index: index as u32,
                z_mm,
                section_z_mm: 5.0 + (index as f64 + 0.5) * 0.2,
                paths: counts
                    .iter()
                    .map(|count| DepositionPath {
                        kind: DepositionKind::SolidFill,
                        points_mm: (0..=*count)
                            .map(|point| [10.0 + (point % 2) as f64, 10.0, z_mm])
                            .collect(),
                    })
                    .collect(),
            }
        })
        .collect();
    result
}

#[test]
fn every_fixed_height_layer_requires_deposition_in_typed_and_decoded_plans() {
    let valid = segmented_plan(&[&[1], &[1], &[1]]);
    valid.validate(&intent()).unwrap();
    for index in 0..valid.layers.len() {
        let mut empty_layer = valid.clone();
        empty_layer.layers[index].paths.clear();
        assert_eq!(
            empty_layer.validate(&intent()).unwrap_err().code,
            ManufacturingErrorCode::InvalidSpecification
        );
        let bytes = serde_json::to_vec(&empty_layer).unwrap();
        assert!(
            serde_json::from_slice::<NormalizedPrintPlan>(&bytes)
                .unwrap_err()
                .is_data()
        );
        let mut artifact = bundle();
        artifact.plan = empty_layer;
        artifact.verification.deposition_segments = 2;
        assert_eq!(
            decode_bundle(&serde_json::to_vec(&artifact).unwrap())
                .unwrap_err()
                .code,
            ManufacturingErrorCode::CorruptArtifact
        );
    }
}

#[test]
fn plan_json_admits_exact_cumulative_segment_budget_across_paths_and_layers() {
    for counts in [
        vec![vec![50_000, 50_000]],
        vec![vec![25_000, 25_000], vec![25_000, 25_000]],
        vec![vec![100_000]],
    ] {
        let slices: Vec<&[usize]> = counts.iter().map(Vec::as_slice).collect();
        let expected = segmented_plan(&slices);
        let decoded: NormalizedPrintPlan =
            serde_json::from_slice(&serde_json::to_vec(&expected).unwrap()).unwrap();
        assert_eq!(decoded, expected);
        decoded.validate(&intent()).unwrap();
        assert_eq!(
            decoded
                .layers
                .iter()
                .flat_map(|layer| &layer.paths)
                .map(|path| path.points_mm.len() - 1)
                .sum::<usize>(),
            100_000
        );
        let mut artifact = bundle();
        artifact.plan = expected;
        artifact.verification.deposition_segments = 100_000;
        let decoded = decode_bundle(&serde_json::to_vec(&artifact).unwrap()).unwrap();
        assert_eq!(decoded, artifact);
        assert_eq!(decoded.summary().deposition_segments, 100_000);
    }
}

#[test]
fn plan_json_rejects_cumulative_segment_excess_across_paths_and_layers() {
    for counts in [
        vec![vec![50_000, 50_001]],
        vec![vec![25_000, 25_000], vec![25_000, 25_001]],
        vec![vec![100_001]],
    ] {
        let slices: Vec<&[usize]> = counts.iter().map(Vec::as_slice).collect();
        let exceeded = segmented_plan(&slices);
        assert_eq!(
            exceeded.validate(&intent()).unwrap_err().code,
            ManufacturingErrorCode::ResourceLimit
        );
        let bytes = serde_json::to_vec(&exceeded).unwrap();
        assert!(
            serde_json::from_slice::<NormalizedPrintPlan>(&bytes)
                .unwrap_err()
                .is_data()
        );
        let mut artifact = bundle();
        artifact.plan = exceeded;
        assert_eq!(
            decode_bundle(&serde_json::to_vec(&artifact).unwrap())
                .unwrap_err()
                .code,
            ManufacturingErrorCode::CorruptArtifact
        );
    }
}

#[test]
fn cumulative_segment_excess_is_rejected_before_decoding_later_points() {
    for counts in [vec![vec![99_999, 2]], vec![vec![99_999], vec![2]]] {
        let slices: Vec<&[usize]> = counts.iter().map(Vec::as_slice).collect();
        let mut json = serde_json::to_string(&segmented_plan(&slices)).unwrap();
        // An invalid token follows the point exceeding the aggregate limit.
        // Schema admission must fail before the JSON parser reaches that token.
        let final_points_close = json.rfind("]]}").unwrap() + 1;
        json.insert_str(final_points_close, ",INVALID");
        let error = serde_json::from_str::<NormalizedPrintPlan>(&json).unwrap_err();
        assert!(error.is_data(), "{error:?}");
    }
}

#[test]
fn empty_and_single_point_paths_cannot_bypass_cumulative_admission() {
    for points in ["[]", "[[10,10,5.2]]"] {
        let json = format!(
            r#"{{"schema_version":1,"layers":[{{"index":0,"z_mm":5.2,"section_z_mm":5.1,"paths":[{{"kind":"solid_fill","points_mm":{points}}},INVALID]}}],"sampling_tolerance_mm":0.005,"offset_tolerance_mm":0.005,"layer_quantization_mm":0.000001}}"#
        );
        let error = serde_json::from_str::<NormalizedPrintPlan>(&json).unwrap_err();
        assert!(error.is_data(), "{error:?}");
    }
}

#[test]
fn nested_plan_fields_are_order_independent_but_strictly_required_and_unique() {
    let expected = segmented_plan(&[&[1]]);
    let points = r#"[[10,10,5.2],[11,10,5.2]]"#;
    let path = format!(r#"{{"points_mm":{points},"kind":"solid_fill"}}"#);
    let layer = format!(r#"{{"paths":[{path}],"section_z_mm":5.1,"z_mm":5.2,"index":0}}"#);
    let wrap = |layer: &str| {
        format!(
            r#"{{"layers":[{layer}],"layer_quantization_mm":0.000001,"offset_tolerance_mm":0.005,"sampling_tolerance_mm":0.005,"schema_version":1}}"#
        )
    };
    assert_eq!(
        serde_json::from_str::<NormalizedPrintPlan>(&wrap(&layer)).unwrap(),
        expected
    );
    for field in [
        r#""kind":"solid_fill""#.to_string(),
        format!(r#""points_mm":{points}"#),
    ] {
        let duplicate = format!(r#"{{{field},"points_mm":{points},"kind":"solid_fill"}}"#);
        let changed =
            format!(r#"{{"paths":[{duplicate}],"section_z_mm":5.1,"z_mm":5.2,"index":0}}"#);
        assert!(
            serde_json::from_str::<NormalizedPrintPlan>(&wrap(&changed))
                .unwrap_err()
                .is_data()
        );
    }
    for field in [
        r#""index":0"#,
        r#""z_mm":5.2"#,
        r#""section_z_mm":5.1"#,
        &format!(r#""paths":[{path}]"#),
    ] {
        let duplicate =
            format!(r#"{{{field},"paths":[{path}],"section_z_mm":5.1,"z_mm":5.2,"index":0}}"#);
        assert!(
            serde_json::from_str::<NormalizedPrintPlan>(&wrap(&duplicate))
                .unwrap_err()
                .is_data()
        );
    }
    for changed_path in [
        format!(r#"{{"kind":"solid_fill","points_mm":{points},"extra":0}}"#),
        format!(r#"{{"points_mm":{points}}}"#),
        r#"{"kind":"solid_fill"}"#.to_string(),
    ] {
        let changed_layer =
            format!(r#"{{"paths":[{changed_path}],"section_z_mm":5.1,"z_mm":5.2,"index":0}}"#);
        assert!(
            serde_json::from_str::<NormalizedPrintPlan>(&wrap(&changed_layer))
                .unwrap_err()
                .is_data()
        );
    }
    for changed_layer in [
        format!(r#"{{"paths":[{path}],"section_z_mm":5.1,"z_mm":5.2,"index":0,"extra":0}}"#),
        format!(r#"{{"paths":[{path}],"section_z_mm":5.1,"z_mm":5.2}}"#),
        format!(r#"{{"paths":[{path}],"section_z_mm":5.1,"index":0}}"#),
        format!(r#"{{"paths":[{path}],"z_mm":5.2,"index":0}}"#),
        r#"{"section_z_mm":5.1,"z_mm":5.2,"index":0}"#.to_string(),
    ] {
        assert!(
            serde_json::from_str::<NormalizedPrintPlan>(&wrap(&changed_layer))
                .unwrap_err()
                .is_data()
        );
    }
}
