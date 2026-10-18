// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;

fn intent() -> ManufacturingIntent {
    ManufacturingIntent {
        printer: PrinterSpecification {
            schema_version: MANUFACTURING_SCHEMA_VERSION,
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
            workflow: JobWorkflow::DiagnosticBodyOnly,
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
            native_shape_error_mm: 0.02,
            bead_width_mm: 0.45,
            perimeter_count: 2,
            infill_fraction: 1.0,
            top_skin_thickness_mm: 0.6,
            bottom_skin_thickness_mm: 0.6,
            print_speed_mm_s: 30.0,
            travel_speed_mm_s: 100.0,
            flow_multiplier: 1.0,
            process: ProcessPolicy {
                first_layer_speed_factor: 1.0,
                first_layer_flow_factor: 1.0,
                fan_first_layers: 0,
                fan_after_first_layers: 0,
                bridge_speed_factor: 0.5,
                bridge_flow_factor: 1.0,
                bridge_fan_pwm: 255,
                max_bridge_span_mm: 5.0,
                retraction: RetractionPolicy::Disabled,
            },
            supports: SupportPolicy::Disabled,
            seam_policy: SeamPolicy::Nearest,
            rectilinear_axis: RectilinearAxisPolicy::AlternatingXY,
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
fn coverage() -> PlanarRegionCoverage {
    PlanarRegionCoverage {
        section_area_mm2: 50.0,
        source_section_area_mm2: 50.0,
        surface_added_area_mm2: 0.0,
        wall_area_mm2: 50.0,
        bottom_skin_area_mm2: 0.0,
        top_skin_area_mm2: 0.0,
        solid_interior_area_mm2: 0.0,
        sparse_interior_area_mm2: 0.0,
        intentional_void_area_mm2: 0.0,
        protected_hole_area_mm2: 0.0,
        nominal_deposition_volume_mm3: 0.0,
        support_area_mm2: 0.0,
        support_nominal_deposition_volume_mm3: 0.0,
        requested_infill_fraction: 1.0,
    }
}
fn native_geometry() -> NativeGeometryCoverageSummary {
    NativeGeometryCoverageSummary {
        requested_shape_error_mm: 0.02,
        achieved_shape_error_mm: 0.001,
        height_analysis_count: 1,
        slab_count: 1,
        certified_layer_count: 1,
        event_count: 0,
        geometry_work: 1,
        evidence: PreflightEvidence::Certified,
        source_feature_coverage: NativeFeatureCoverageSummary {
            disjoint_source_layer_count: 1,
            invariant_source_union_layer_count: 0,
            native_interface_complex_layer_count: 0,
            owned_critical_slab_count: 0,
        },
        skin_coverage: SkinCoverageScope::ScheduledMaterialDistance,
        support_clearance: SupportClearanceScope::ContinuousTargetOuterOccupancy,
    }
}
#[test]
fn native_geometry_coverage_requires_complete_certified_bounded_policy() {
    let summary = native_geometry();
    summary.validate().unwrap();
    let value = serde_json::to_value(&summary).unwrap();
    assert_eq!(value["skin_coverage"], "scheduled_material_distance");
    assert_eq!(
        value["support_clearance"],
        "continuous_target_outer_occupancy"
    );
    assert_eq!(
        value["source_feature_coverage"]["disjoint_source_layer_count"],
        1
    );
    for (disjoint, invariant, interface) in [(3, 0, 0), (0, 3, 0), (0, 0, 3), (1, 1, 1)] {
        let mut expected = summary.clone();
        expected.certified_layer_count = 3;
        expected.slab_count = 3;
        expected.source_feature_coverage = NativeFeatureCoverageSummary {
            disjoint_source_layer_count: disjoint,
            invariant_source_union_layer_count: invariant,
            native_interface_complex_layer_count: interface,
            owned_critical_slab_count: 1,
        };
        expected.validate().unwrap();
        let encoded = serde_json::to_value(&expected).unwrap();
        assert_eq!(
            serde_json::from_value::<NativeGeometryCoverageSummary>(encoded).unwrap(),
            expected
        );
    }
    for (disjoint, invariant, interface, endpoints) in [
        (0, 0, 0, 0),
        (1, 1, 0, 0),
        (1, 0, 1, 0),
        (1, 0, 0, 2),
        (u32::MAX, 0, 1, 0),
    ] {
        let mut invalid = summary.clone();
        invalid.source_feature_coverage = NativeFeatureCoverageSummary {
            disjoint_source_layer_count: disjoint,
            invariant_source_union_layer_count: invariant,
            native_interface_complex_layer_count: interface,
            owned_critical_slab_count: endpoints,
        };
        assert!(invalid.validate().is_err());
    }
    let mut omitted_occurrence_capture = summary.clone();
    omitted_occurrence_capture.height_analysis_count = 2;
    assert!(omitted_occurrence_capture.validate().is_err());
    for field in [
        "native_interface_complex_layer_count",
        "owned_critical_slab_count",
    ] {
        let mut missing = value.clone();
        missing["source_feature_coverage"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(serde_json::from_value::<NativeGeometryCoverageSummary>(missing).is_err());
    }
    let mut old_scope = value.clone();
    old_scope["source_feature_coverage"] = serde_json::json!("disjoint_placed_sources");
    assert!(serde_json::from_value::<NativeGeometryCoverageSummary>(old_scope).is_err());
    for field in value.as_object().unwrap().keys() {
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(
            serde_json::from_value::<NativeGeometryCoverageSummary>(missing).is_err(),
            "{field}"
        );
    }
    for (field, invalid) in [
        ("requested_shape_error_mm", serde_json::json!(0.0)),
        ("achieved_shape_error_mm", serde_json::json!(-0.001)),
        ("achieved_shape_error_mm", serde_json::json!(0.021)),
        ("height_analysis_count", serde_json::json!(0)),
        (
            "height_analysis_count",
            serde_json::json!(MAX_OCCURRENCES + 1),
        ),
        ("slab_count", serde_json::json!(0)),
        (
            "slab_count",
            serde_json::json!(MAX_PRINT_LAYERS * MAX_OCCURRENCES + 1),
        ),
        ("certified_layer_count", serde_json::json!(0)),
        (
            "certified_layer_count",
            serde_json::json!(MAX_PRINT_LAYERS + 1),
        ),
        ("event_count", serde_json::json!(4097)),
        ("geometry_work", serde_json::json!(0)),
    ] {
        let mut changed = value.clone();
        changed[field] = invalid;
        let invalid: NativeGeometryCoverageSummary = serde_json::from_value(changed).unwrap();
        assert!(invalid.validate().is_err(), "{field}");
    }
    for evidence in [
        PreflightEvidence::NotEvaluated,
        PreflightEvidence::Unsupported,
        PreflightEvidence::Indeterminate,
        PreflightEvidence::Failed,
    ] {
        let mut invalid = summary.clone();
        invalid.evidence = evidence;
        assert!(invalid.validate().is_err());
    }
    for achieved in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut invalid = summary.clone();
        invalid.achieved_shape_error_mm = achieved;
        assert!(invalid.validate().is_err());
    }
    for field in [
        "evidence",
        "skin_coverage",
        "support_clearance",
        "source_feature_coverage",
    ] {
        let mut unknown = value.clone();
        unknown[field] = serde_json::json!("unspecified");
        assert!(serde_json::from_value::<NativeGeometryCoverageSummary>(unknown).is_err());
    }
    let mut exact = summary;
    exact.achieved_shape_error_mm = 0.0;
    exact.validate().unwrap();
}

#[test]
fn native_coverage_counts_accepted_source_slabs_not_reused_layers() {
    let mut summary = native_geometry();
    summary.height_analysis_count = 2;
    summary.slab_count = 6;
    summary.certified_layer_count = 40;
    summary.source_feature_coverage = NativeFeatureCoverageSummary {
        disjoint_source_layer_count: 10,
        invariant_source_union_layer_count: 20,
        native_interface_complex_layer_count: 10,
        owned_critical_slab_count: 2,
    };
    summary.validate().unwrap();
    let decoded: NativeGeometryCoverageSummary =
        serde_json::from_slice(&serde_json::to_vec(&summary).unwrap()).unwrap();
    assert_eq!(decoded, summary);
    decoded.validate().unwrap();

    let mut missing_source = summary.clone();
    missing_source.slab_count = 1;
    missing_source
        .source_feature_coverage
        .owned_critical_slab_count = 0;
    assert!(missing_source.validate().is_err());

    let mut extra_captures = summary.clone();
    extra_captures.slab_count =
        extra_captures.height_analysis_count * extra_captures.certified_layer_count + 1;
    assert!(extra_captures.validate().is_err());

    let mut extra_ownership = summary.clone();
    extra_ownership
        .source_feature_coverage
        .owned_critical_slab_count = summary.slab_count + 1;
    assert!(extra_ownership.validate().is_err());

    let mut uncovered_layer = summary;
    uncovered_layer.certified_layer_count += 1;
    assert!(uncovered_layer.validate().is_err());
}

#[test]
fn accepted_coverage_binds_recipe_occurrences_schedule_and_visible_summary() {
    let captured = bundle();
    assert_eq!(
        captured.summary().native_geometry,
        captured.plan.native_geometry
    );
    let mut missing = serde_json::to_value(&captured.plan).unwrap();
    missing.as_object_mut().unwrap().remove("native_geometry");
    assert!(serde_json::from_value::<NormalizedPrintPlan>(missing).is_err());
    let mut missing = serde_json::to_value(captured.summary()).unwrap();
    missing.as_object_mut().unwrap().remove("native_geometry");
    assert!(serde_json::from_value::<ManufacturingSummary>(missing).is_err());
    let mut mismatch = captured.clone();
    mismatch.plan.native_geometry.requested_shape_error_mm = 0.03;
    assert!(mismatch.validate().is_err());
    let mut mismatch = captured.clone();
    mismatch.plan.native_geometry.height_analysis_count = 2;
    assert!(mismatch.validate().is_err());
    let mut mismatch = captured.clone();
    mismatch.plan.native_geometry.certified_layer_count = 2;
    assert!(mismatch.validate().is_err());
    let mut summary = captured.summary();
    summary.native_geometry.certified_layer_count = 2;
    assert!(summary.validate().is_err());
    let invariant = segmented_plan(&[&[1], &[1]]);
    assert_eq!(invariant.native_geometry.slab_count, 2);
    assert_eq!(invariant.native_geometry.certified_layer_count, 2);
    invariant.validate(&intent()).unwrap();
}

fn plan() -> NormalizedPrintPlan {
    NormalizedPrintPlan {
        schema_version: MANUFACTURING_SCHEMA_VERSION,
        sampling_tolerance_mm: 0.005,
        surface_tolerance_mm: 0.001,
        offset_tolerance_mm: 0.005,
        layer_quantization_mm: 0.000001,
        surface_resolution: vec![],
        native_geometry: native_geometry(),
        layers: vec![PrintLayer {
            index: 0,
            z_mm: 5.2,
            height_mm: 0.2,
            section_z_mm: 5.1,
            region_coverage: coverage(),
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
    let input_hash = input_fingerprint(
        &project_id,
        &definitions,
        &occurrences,
        &intent,
        &SurfaceOverlays::empty(),
    )
    .unwrap();
    ManufacturingBundle {
        schema_version: MANUFACTURING_SCHEMA_VERSION,
        provenance: ManufacturingProvenance {
            project_id,
            input_revision: ProjectRevision(9),
            input_hash,
            definitions,
            occurrences,
            intent,
            surface_overlays: SurfaceOverlays::empty(),
            surface_images: vec![],
            kernel: KernelIdentity {
                name: "native".into(),
                version: "1".into(),
                revision: "pinned".into(),
            },
            planner_version: "11".into(),
            emitter_version: "4".into(),
            verifier_version: "4".into(),
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
            lifecycle_motion_count: 0,
            priming_filament_length_mm: 0.0,
            support_deposition_segments: 0,
            support_deposited_volume_mm3: 0.0,
            support_filament_length_mm: 0.0,
            retraction_count: 0,
            retracted_filament_mm: 0.0,
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
        &format!("\"schema_version\":{MANUFACTURING_SCHEMA_VERSION}"),
        &format!("\"schema_version\":{MANUFACTURING_SCHEMA_VERSION},\"schema_version\":{MANUFACTURING_SCHEMA_VERSION}"),
        1,
    );
    assert!(decode_intent(duplicate.as_bytes()).is_err());
}

#[test]
fn native_shape_error_is_required_positive_and_persisted_as_semantic_intent() {
    let intent = intent();
    let mut missing = serde_json::to_value(&intent).unwrap();
    missing["recipe"]
        .as_object_mut()
        .unwrap()
        .remove("native_shape_error_mm");
    assert!(decode_intent(&serde_json::to_vec(&missing).unwrap()).is_err());
    for value in [0.0, -0.001, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut invalid = intent.clone();
        invalid.recipe.native_shape_error_mm = value;
        assert!(invalid.validate().is_err(), "{value}");
    }
    let mut old = intent.clone();
    old.printer.schema_version = MANUFACTURING_SCHEMA_VERSION - 1;
    assert!(old.validate().is_err());
    let captured = bundle();
    let bytes = serde_json::to_vec(&captured).unwrap();
    let restored = decode_bundle(&bytes).unwrap();
    assert_eq!(restored.provenance.intent, intent);
    assert_eq!(
        restored.provenance.intent.recipe.native_shape_error_mm,
        0.02
    );
    let mut changed = restored.provenance.clone();
    changed.intent.recipe.native_shape_error_mm = 0.01;
    let changed_hash = input_fingerprint(
        &changed.project_id,
        &changed.definitions,
        &changed.occurrences,
        &changed.intent,
        &changed.surface_overlays,
    )
    .unwrap();
    assert_ne!(changed_hash, restored.provenance.input_hash);
    assert!(
        changed.validate().is_err(),
        "captured hash must bind the native shape policy"
    );
    changed.input_hash = changed_hash;
    changed.validate().unwrap();
    let mut missing = serde_json::to_value(&captured).unwrap();
    missing["provenance"]["intent"]["recipe"]
        .as_object_mut()
        .unwrap()
        .remove("native_shape_error_mm");
    assert!(decode_bundle(&serde_json::to_vec(&missing).unwrap()).is_err());
    let mut old = serde_json::to_value(&captured).unwrap();
    old["schema_version"] = serde_json::json!(MANUFACTURING_SCHEMA_VERSION - 1);
    assert!(decode_bundle(&serde_json::to_vec(&old).unwrap()).is_err());
}

#[test]
fn seam_and_axis_json_are_explicit_strict_and_preserve_unit_directions() {
    let original = intent();
    let value = serde_json::to_value(&original).unwrap();
    assert_eq!(
        value["recipe"]["seam_policy"],
        serde_json::json!({"kind":"nearest"})
    );
    assert_eq!(value["recipe"]["rectilinear_axis"], "alternating_xy");
    for (axis, name) in [
        (RectilinearAxisPolicy::X, "x"),
        (RectilinearAxisPolicy::Y, "y"),
        (RectilinearAxisPolicy::AlternatingXY, "alternating_xy"),
        (RectilinearAxisPolicy::AlternatingYX, "alternating_yx"),
    ] {
        let mut authored = original.clone();
        authored.recipe.rectilinear_axis = axis;
        authored.recipe.seam_policy = SeamPolicy::PreferredBuildDirection { xy: [0.6, -0.8] };
        let json = serde_json::to_value(&authored).unwrap();
        assert_eq!(json["recipe"]["rectilinear_axis"], name);
        assert_eq!(
            json["recipe"]["seam_policy"],
            serde_json::json!({"kind":"preferred_build_direction","xy":[0.6,-0.8]})
        );
        assert_eq!(
            decode_intent(&serde_json::to_vec(&authored).unwrap()).unwrap(),
            authored
        );
    }
    for policy in [
        serde_json::json!({}),
        serde_json::json!({"kind":"unknown"}),
        serde_json::json!({"kind":"nearest","xy":[1.0,0.0]}),
        serde_json::json!({"kind":"preferred_build_direction"}),
        serde_json::json!({"kind":"preferred_build_direction","xy":[1.0]}),
        serde_json::json!({"kind":"preferred_build_direction","xy":[1.0,0.0,0.0]}),
        serde_json::json!({"kind":"preferred_build_direction","xy":[1.0,0.0],"frame":"camera"}),
    ] {
        let mut invalid = value.clone();
        invalid["recipe"]["seam_policy"] = policy;
        assert!(decode_intent(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
    for policy in [
        serde_json::json!("alternating_x_y"),
        serde_json::json!("unknown"),
        serde_json::Value::Null,
    ] {
        let mut invalid = value.clone();
        invalid["recipe"]["rectilinear_axis"] = policy;
        assert!(decode_intent(&serde_json::to_vec(&invalid).unwrap()).is_err());
    }
    for xy in [
        [0.0, 0.0],
        [2.0, 0.0],
        [f64::MIN_POSITIVE, 0.0],
        [f64::MAX, f64::MAX],
        [f64::NAN, 1.0],
        [1.0, f64::INFINITY],
        [f64::NEG_INFINITY, 0.0],
        [1.0 + 2.0 * crate::geometry::POSE_NORM_TOLERANCE, 0.0],
    ] {
        let mut invalid = original.clone();
        invalid.recipe.seam_policy = SeamPolicy::PreferredBuildDirection { xy };
        assert!(invalid.validate().is_err(), "{xy:?}");
    }
    let xy = [1.0 + 0.5 * crate::geometry::POSE_NORM_TOLERANCE, 0.0];
    let mut admitted = original;
    admitted.recipe.seam_policy = SeamPolicy::PreferredBuildDirection { xy };
    admitted.validate().unwrap();
    assert_eq!(
        decode_intent(&serde_json::to_vec(&admitted).unwrap()).unwrap(),
        admitted
    );
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
    changed.validate().unwrap();
    changed.recipe.infill_fraction = 0.0;
    assert!(changed.validate().is_err());
    changed = valid.clone();
    changed.printer.capabilities.extruders = 2;
    assert_eq!(
        changed.validate().unwrap_err().code,
        ManufacturingErrorCode::UnsupportedCapability
    );
    changed = valid.clone();
    changed.printer.capabilities.generated_supports = true;
    changed.validate().unwrap();
    changed.printer.capabilities.generated_supports = false;
    changed.recipe.supports = SupportPolicy::Breakaway {
        density: 0.25,
        xy_clearance_mm: 0.5,
        z_gap_mm: 0.2,
        interface_thickness_mm: 0.4,
    };
    assert!(changed.validate().is_err());
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
fn ordinary_deposition_rejects_orthogonal_and_diagonal_segments_for_each_axis_policy() {
    for (policy, fixed_axes) in [
        (RectilinearAxisPolicy::X, [1, 1]),
        (RectilinearAxisPolicy::Y, [0, 0]),
        (RectilinearAxisPolicy::AlternatingXY, [1, 0]),
        (RectilinearAxisPolicy::AlternatingYX, [0, 1]),
    ] {
        let mut data = intent();
        data.recipe.rectilinear_axis = policy;
        for kind in [
            DepositionKind::BottomSkin,
            DepositionKind::TopSkin,
            DepositionKind::SolidFill,
            DepositionKind::SparseFill,
        ] {
            data.recipe.infill_fraction = if kind == DepositionKind::SparseFill {
                0.25
            } else {
                1.0
            };
            let mut accepted = segmented_plan(&[&[2], &[2]]);
            for (layer, fixed_axis) in accepted.layers.iter_mut().zip(fixed_axes) {
                let coverage = &mut layer.region_coverage;
                coverage.wall_area_mm2 = 0.0;
                coverage.requested_infill_fraction = data.recipe.infill_fraction;
                match kind {
                    DepositionKind::BottomSkin => coverage.bottom_skin_area_mm2 = 50.0,
                    DepositionKind::TopSkin => coverage.top_skin_area_mm2 = 50.0,
                    DepositionKind::SolidFill => coverage.solid_interior_area_mm2 = 50.0,
                    DepositionKind::SparseFill => {
                        coverage.sparse_interior_area_mm2 = 50.0;
                        coverage.intentional_void_area_mm2 = 37.5;
                    }
                    _ => unreachable!(),
                }
                let along = 1 - fixed_axis;
                let path = &mut layer.paths[0];
                path.kind = kind;
                for (index, point) in path.points_mm.iter_mut().enumerate() {
                    point[fixed_axis] = 10.0;
                    point[along] = 10.0 + index as f64;
                }
            }
            accepted.validate(&data).unwrap();
            for (index, fixed_axis) in fixed_axes.into_iter().enumerate() {
                let mut orthogonal = accepted.clone();
                for point in &mut orthogonal.layers[index].paths[0].points_mm {
                    point.swap(0, 1);
                }
                let error = orthogonal.validate(&data).unwrap_err();
                assert_eq!(error.code, ManufacturingErrorCode::InvalidSpecification);
                let mut diagonal = accepted.clone();
                diagonal.layers[index].paths[0].points_mm[2][fixed_axis] += 1.0;
                assert_eq!(
                    diagonal.validate(&data).unwrap_err().code,
                    ManufacturingErrorCode::InvalidSpecification
                );
            }
        }
    }
}

#[test]
fn ordinary_axis_uses_exact_xyz6_projection_without_geometric_epsilon() {
    let mut data = intent();
    data.recipe.rectilinear_axis = RectilinearAxisPolicy::X;
    let mut accepted = segmented_plan(&[&[1]]);
    accepted.layers[0].paths[0].points_mm[0][1] = 10.0000004;
    accepted.layers[0].paths[0].points_mm[1][1] = 10.00000049;
    accepted.validate(&data).unwrap();
    let mut changed = accepted.clone();
    changed.layers[0].paths[0].points_mm[1][1] = 10.00000051;
    assert_eq!(
        changed.validate(&data).unwrap_err().message,
        "ordinary deposition segment violates the authored rectilinear axis"
    );
    accepted.layers[0].paths[0].points_mm[0][1] = -0.0;
    accepted.layers[0].paths[0].points_mm[1][1] = 0.0;
    accepted.validate(&data).unwrap();
}

#[test]
fn ordinary_axis_uses_scheduled_index_and_excludes_bridge_support_and_perimeters() {
    let mut data = intent();
    data.printer.capabilities.generated_supports = true;
    data.recipe.supports = SupportPolicy::Breakaway {
        density: 0.5,
        xy_clearance_mm: 0.3,
        z_gap_mm: 0.2,
        interface_thickness_mm: 0.2,
    };
    let mut scheduled = segmented_plan(&[&[1], &[1]]);
    let first = &mut scheduled.layers[0];
    first.paths[0].kind = DepositionKind::SupportInterface;
    first.region_coverage.section_area_mm2 = 0.0;
    first.region_coverage.source_section_area_mm2 = 0.0;
    first.region_coverage.wall_area_mm2 = 0.0;
    first.region_coverage.support_area_mm2 = 1.0;
    first.region_coverage.support_nominal_deposition_volume_mm3 = 0.1;
    scheduled.validate(&data).unwrap();
    scheduled.layers[1].paths[0].points_mm[1].swap(0, 1);
    assert_eq!(
        scheduled.validate(&data).unwrap_err().message,
        "ordinary deposition segment violates the authored rectilinear axis"
    );
    for kind in [
        DepositionKind::Bridge,
        DepositionKind::Support,
        DepositionKind::SupportInterface,
    ] {
        let mut accepted = plan();
        accepted.layers[0].paths.push(DepositionPath {
            kind,
            points_mm: vec![[30.0, 30.0, 5.2], [31.0, 31.0, 5.2]],
        });
        if matches!(
            kind,
            DepositionKind::Support | DepositionKind::SupportInterface
        ) {
            accepted.layers[0].region_coverage.support_area_mm2 = 1.0;
            accepted.layers[0]
                .region_coverage
                .support_nominal_deposition_volume_mm3 = 0.1;
        }
        accepted.validate(&data).unwrap();
    }
}

#[test]
fn durable_bundle_rejects_rehashed_axis_mismatch_before_stored_verification() {
    let mut captured = bundle();
    captured.plan = segmented_plan(&[&[3]]);
    decode_bundle(&serde_json::to_vec(&captured).unwrap()).unwrap();
    captured.provenance.intent.recipe.rectilinear_axis = RectilinearAxisPolicy::Y;
    let provenance = &mut captured.provenance;
    provenance.input_hash = input_fingerprint(
        &provenance.project_id,
        &provenance.definitions,
        &provenance.occurrences,
        &provenance.intent,
        &provenance.surface_overlays,
    )
    .unwrap();
    provenance.validate().unwrap();
    captured.verification.verified = true;
    let error = decode_bundle(&serde_json::to_vec(&captured).unwrap()).unwrap_err();
    assert_eq!(error.code, ManufacturingErrorCode::InvalidSpecification);
}

#[test]
fn fingerprint_is_sorted_and_sensitive_to_authoritative_semantics() {
    let (project, definitions, mut occurrences) = inputs();
    let intent = intent();
    let mut second = occurrences[0].clone();
    second.occurrence_id = OccurrenceId::new(2).unwrap();
    second.pose.translation_mm[0] = 10.0;
    occurrences.push(second);
    let original = input_fingerprint(
        &project,
        &definitions,
        &occurrences,
        &intent,
        &SurfaceOverlays::empty(),
    )
    .unwrap();
    occurrences.reverse();
    assert_eq!(
        original,
        input_fingerprint(
            &project,
            &definitions,
            &occurrences,
            &intent,
            &SurfaceOverlays::empty()
        )
        .unwrap()
    );
    occurrences[0].pose.translation_mm[0] += 1.0;
    assert_ne!(
        original,
        input_fingerprint(
            &project,
            &definitions,
            &occurrences,
            &intent,
            &SurfaceOverlays::empty()
        )
        .unwrap()
    );
    occurrences[0].pose.translation_mm[0] -= 1.0;
    let mut renamed = definitions.clone();
    renamed[0].provenance.source_name = "retained-label.step".into();
    assert_ne!(
        original,
        input_fingerprint(
            &project,
            &renamed,
            &occurrences,
            &intent,
            &SurfaceOverlays::empty()
        )
        .unwrap()
    );
    let mut units = definitions.clone();
    units[0].provenance.source_unit = SourceUnit::Inch;
    assert_ne!(
        original,
        input_fingerprint(
            &project,
            &units,
            &occurrences,
            &intent,
            &SurfaceOverlays::empty()
        )
        .unwrap()
    );
    let mut changed_intent = intent.clone();
    changed_intent.recipe.flow_multiplier = 1.01;
    assert_ne!(
        original,
        input_fingerprint(
            &project,
            &definitions,
            &occurrences,
            &changed_intent,
            &SurfaceOverlays::empty()
        )
        .unwrap()
    );
    let mut invalid = definitions.clone();
    invalid[0].provenance.source_hash = SourceHash::from_bytes(b"foreign");
    assert!(
        input_fingerprint(
            &project,
            &invalid,
            &occurrences,
            &intent,
            &SurfaceOverlays::empty()
        )
        .is_err()
    );
    occurrences.push(occurrences[0].clone());
    assert!(
        input_fingerprint(
            &project,
            &definitions,
            &occurrences,
            &intent,
            &SurfaceOverlays::empty()
        )
        .is_err()
    );
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
        format_version: crate::project::PROJECT_FORMAT_VERSION,
        project_id: p.project_id.clone(),
        revision: ProjectRevision(10),
        units: ProjectUnits::Millimetres,
        frame: ProjectFrame::RightHanded,
        next_occurrence: 2,
        next_surface_layer: 1,
        surface_overlays: SurfaceOverlays::empty(),
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
fn native_failure_categories_preserve_specific_serialized_diagnostics() {
    for (code, name) in [
        (
            ManufacturingErrorCode::NativeKernelFailure,
            "native_kernel_failure",
        ),
        (
            ManufacturingErrorCode::NativeConditioningFailure,
            "native_conditioning_failure",
        ),
        (
            ManufacturingErrorCode::NativeAnalysisIndeterminate,
            "native_analysis_indeterminate",
        ),
        (
            ManufacturingErrorCode::NativeSingularGeometry,
            "native_singular_geometry",
        ),
        (
            ManufacturingErrorCode::NativeShapeErrorExceeded,
            "native_shape_error_exceeded",
        ),
    ] {
        let error = ManufacturingError::new(code, "Native geometry query diagnostic");
        let value = serde_json::to_value(&error).unwrap();
        assert_eq!(value["code"], name);
        assert_eq!(
            serde_json::from_value::<ManufacturingError>(value).unwrap(),
            error
        );
    }
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
                height_mm: 0.2,
                section_z_mm: 5.0 + (index as f64 + 0.5) * 0.2,
                region_coverage: coverage(),
                paths: counts
                    .iter()
                    .map(|count| DepositionPath {
                        kind: DepositionKind::SolidFill,
                        points_mm: (0..=*count)
                            .map(|point| {
                                let delta = (point % 2) as f64;
                                if index.is_multiple_of(2) {
                                    [10.0 + delta, 10.0, z_mm]
                                } else {
                                    [10.0, 10.0 + delta, z_mm]
                                }
                            })
                            .collect(),
                    })
                    .collect(),
            }
        })
        .collect();
    result.native_geometry.certified_layer_count = result.layers.len() as u32;
    result.native_geometry.slab_count = result.layers.len() as u32;
    result
        .native_geometry
        .source_feature_coverage
        .disjoint_source_layer_count = result.layers.len() as u32;
    result
}

#[test]
fn missing_object_deposition_is_rejected_at_whole_plan_admission() {
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
        let decoded: NormalizedPrintPlan = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            decoded.validate(&intent()).unwrap_err().code,
            ManufacturingErrorCode::InvalidSpecification
        );
        let mut artifact = bundle();
        artifact.plan = empty_layer;
        artifact.verification.deposition_segments = 2;
        assert!(decode_bundle(&serde_json::to_vec(&artifact).unwrap()).is_err());
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
            r#"{{"schema_version":{MANUFACTURING_SCHEMA_VERSION},"layers":[{{"index":0,"z_mm":5.2,"section_z_mm":5.1,"paths":[{{"kind":"solid_fill","points_mm":{points}}},INVALID]}}],"sampling_tolerance_mm":0.005,"surface_tolerance_mm":0.001,"offset_tolerance_mm":0.005,"layer_quantization_mm":0.000001}}"#
        );
        let error = serde_json::from_str::<NormalizedPrintPlan>(&json).unwrap_err();
        assert!(error.is_data(), "{error:?}");
    }
}

#[test]
fn nested_plan_fields_are_order_independent_but_strictly_required_and_unique() {
    let expected = segmented_plan(&[&[1]]);
    let coverage = serde_json::to_string(&expected.layers[0].region_coverage).unwrap();
    let native_geometry = serde_json::to_string(&expected.native_geometry).unwrap();
    let points = r#"[[10,10,5.2],[11,10,5.2]]"#;
    let path = format!(r#"{{"points_mm":{points},"kind":"solid_fill"}}"#);
    let layer = format!(
        r#"{{"region_coverage":{coverage},"paths":[{path}],"section_z_mm":5.1,"height_mm":0.2,"z_mm":5.2,"index":0}}"#
    );
    let wrap = |layer: &str| {
        format!(
            r#"{{"native_geometry":{native_geometry},"surface_resolution":[],"layers":[{layer}],"layer_quantization_mm":0.000001,"offset_tolerance_mm":0.005,"surface_tolerance_mm":0.001,"sampling_tolerance_mm":0.005,"schema_version":{MANUFACTURING_SCHEMA_VERSION}}}"#
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
        let changed = format!(
            r#"{{"region_coverage":{coverage},"paths":[{duplicate}],"section_z_mm":5.1,"height_mm":0.2,"z_mm":5.2,"index":0}}"#
        );
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
        r#""height_mm":0.2"#,
        &format!(r#""paths":[{path}]"#),
        &format!(r#""region_coverage":{coverage}"#),
    ] {
        let duplicate = format!(
            r#"{{{field},"region_coverage":{coverage},"paths":[{path}],"section_z_mm":5.1,"height_mm":0.2,"z_mm":5.2,"index":0}}"#
        );
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
        let changed_layer = format!(
            r#"{{"region_coverage":{coverage},"paths":[{changed_path}],"section_z_mm":5.1,"height_mm":0.2,"z_mm":5.2,"index":0}}"#
        );
        assert!(
            serde_json::from_str::<NormalizedPrintPlan>(&wrap(&changed_layer))
                .unwrap_err()
                .is_data()
        );
    }
    for changed_layer in [
        format!(
            r#"{{"region_coverage":{coverage},"paths":[{path}],"section_z_mm":5.1,"height_mm":0.2,"z_mm":5.2,"index":0,"extra":0}}"#
        ),
        format!(
            r#"{{"region_coverage":{coverage},"paths":[{path}],"section_z_mm":5.1,"height_mm":0.2,"z_mm":5.2}}"#
        ),
        format!(
            r#"{{"region_coverage":{coverage},"paths":[{path}],"section_z_mm":5.1,"height_mm":0.2,"index":0}}"#
        ),
        format!(
            r#"{{"region_coverage":{coverage},"paths":[{path}],"height_mm":0.2,"z_mm":5.2,"index":0}}"#
        ),
        format!(
            r#"{{"region_coverage":{coverage},"height_mm":0.2,"section_z_mm":5.1,"z_mm":5.2,"index":0}}"#
        ),
        format!(r#"{{"paths":[{path}],"height_mm":0.2,"section_z_mm":5.1,"z_mm":5.2,"index":0}}"#),
        format!(
            r#"{{"region_coverage":{coverage},"paths":[{path}],"section_z_mm":5.1,"z_mm":5.2,"index":0}}"#
        ),
    ] {
        assert!(
            serde_json::from_str::<NormalizedPrintPlan>(&wrap(&changed_layer))
                .unwrap_err()
                .is_data()
        );
    }
}

#[test]
fn recipe_policies_are_required_without_defaults() {
    let original = serde_json::to_value(intent()).unwrap();
    for field in ["process", "supports", "seam_policy", "rectilinear_axis"] {
        let mut missing = original.clone();
        missing["recipe"].as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<ManufacturingIntent>(missing).is_err());
    }
    for field in [
        "first_layer_speed_factor",
        "first_layer_flow_factor",
        "fan_first_layers",
        "fan_after_first_layers",
        "bridge_speed_factor",
        "bridge_flow_factor",
        "bridge_fan_pwm",
        "max_bridge_span_mm",
        "retraction",
    ] {
        let mut missing = original.clone();
        missing["recipe"]["process"]
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(
            serde_json::from_value::<ManufacturingIntent>(missing).is_err(),
            "{field}"
        );
    }
    let mut missing = serde_json::to_value(plan()).unwrap();
    missing["layers"][0]
        .as_object_mut()
        .unwrap()
        .remove("height_mm");
    assert!(serde_json::from_value::<NormalizedPrintPlan>(missing).is_err());
    let mut old = original;
    old["printer"]["schema_version"] = 2.into();
    assert!(
        serde_json::from_value::<ManufacturingIntent>(old)
            .unwrap()
            .validate()
            .is_err()
    );
}

#[test]
fn process_policy_rejects_numeric_flow_feed_and_serialization_excess() {
    for case in 0..9 {
        let mut data = intent();
        match case {
            0 => data.recipe.process.first_layer_speed_factor = 0.0,
            1 => data.recipe.process.first_layer_speed_factor = 1.1,
            2 => data.recipe.process.bridge_speed_factor = f64::NAN,
            3 => data.recipe.process.first_layer_flow_factor = f64::INFINITY,
            4 => data.recipe.process.bridge_flow_factor = 1000.0,
            5 => data.recipe.process.max_bridge_span_mm = -1.0,
            6 => data.recipe.process.first_layer_speed_factor = 1e-30,
            7 => {
                data.recipe.process.retraction = RetractionPolicy::Enabled {
                    distance_mm: 1e-12,
                    speed_mm_s: 10.0,
                    minimum_travel_mm: 1.0,
                    z_hop_mm: 0.0,
                }
            }
            _ => {
                data.recipe.process.retraction = RetractionPolicy::Enabled {
                    distance_mm: 1.0,
                    speed_mm_s: 151.0,
                    minimum_travel_mm: 1.0,
                    z_hop_mm: 0.0,
                }
            }
        }
        assert!(data.validate().is_err(), "case {case}");
    }
}

#[test]
fn true_layer_height_and_support_ownership_are_not_nominal_grid_aliases() {
    let data = intent();
    let mut scheduled = plan();
    let layer = &mut scheduled.layers[0];
    layer.height_mm = 0.15;
    layer.z_mm = 5.15;
    layer.section_z_mm = 5.075;
    for path in &mut layer.paths {
        for point in &mut path.points_mm {
            point[2] = 5.15;
        }
    }
    scheduled.validate(&data).unwrap();
    scheduled.layers[0].height_mm = 0.2;
    assert!(scheduled.validate(&data).is_err());
    let mut support = data;
    support.printer.capabilities.generated_supports = true;
    support.recipe.supports = SupportPolicy::Breakaway {
        density: 0.5,
        xy_clearance_mm: 0.4,
        z_gap_mm: 0.15,
        interface_thickness_mm: 0.2,
    };
    let mut mixed = plan();
    mixed.layers[0].paths.push(DepositionPath {
        kind: DepositionKind::Support,
        points_mm: vec![[30.0, 30.0, 5.2], [31.0, 30.0, 5.2]],
    });
    mixed.layers[0].region_coverage.support_area_mm2 = 1.0;
    mixed.layers[0]
        .region_coverage
        .support_nominal_deposition_volume_mm3 = 0.1;
    mixed.validate(&support).unwrap();
    mixed.layers[0].region_coverage.section_area_mm2 += 1.0;
    assert!(
        mixed.validate(&support).is_err(),
        "support cannot enter object partition"
    );
}

#[test]
fn preflight_admission_is_bounded_and_never_claims_physical_authority() {
    let check = PreflightCheck {
        code: "native_analysis_required".into(),
        severity: PreflightSeverity::Info,
        evidence: PreflightEvidence::NotEvaluated,
        required: false,
        message: "Native compile analysis required; physical qualification absent".into(),
        occurrence_id: None,
        face_id: None,
    };
    let mut report = PreflightReport {
        input_revision: ProjectRevision(9),
        disposition: PreflightDisposition::Admitted,
        software_only: true,
        checks: vec![check.clone()],
    };
    report.validate().unwrap();
    report.software_only = false;
    assert!(report.validate().is_err());
    report.software_only = true;
    let mut value = serde_json::to_value(&report).unwrap();
    value["checks"] = serde_json::to_value(vec![check; 257]).unwrap();
    assert!(serde_json::from_value::<PreflightReport>(value).is_err());
    report.checks.clear();
    assert!(report.validate().is_err());
}

#[test]
fn preflight_disposition_follows_required_evidence_with_known_failure_priority() {
    use PreflightDisposition::{Admitted, Refused, Unresolved};
    use PreflightEvidence::{Certified, Failed, Indeterminate, NotEvaluated, Unsupported};
    let check = PreflightCheck {
        code: "geometry".into(),
        severity: PreflightSeverity::Info,
        evidence: Certified,
        required: true,
        message: "Software geometry evidence".into(),
        occurrence_id: None,
        face_id: None,
    };
    let states = [Certified, Failed, Indeterminate, NotEvaluated, Unsupported];
    for first in states {
        for second in states {
            for first_required in [false, true] {
                for second_required in [false, true] {
                    let mut checks = vec![check.clone(), check.clone()];
                    checks[0].evidence = first;
                    checks[0].required = first_required;
                    checks[1].evidence = second;
                    checks[1].required = second_required;
                    let required: Vec<_> = checks.iter().filter(|check| check.required).collect();
                    let expected = if required
                        .iter()
                        .any(|check| matches!(check.evidence, Failed | Unsupported))
                    {
                        Refused
                    } else if required
                        .iter()
                        .any(|check| matches!(check.evidence, NotEvaluated | Indeterminate))
                    {
                        Unresolved
                    } else {
                        Admitted
                    };
                    for disposition in [Admitted, Refused, Unresolved] {
                        let report = PreflightReport {
                            input_revision: ProjectRevision(9),
                            disposition,
                            software_only: true,
                            checks: checks.clone(),
                        };
                        assert_eq!(
                            report.validate().is_ok(),
                            disposition == expected,
                            "{first:?}/{first_required}, {second:?}/{second_required}, {disposition:?}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn preflight_json_requires_explicit_evidence_policy_and_rejects_eligibility_alias() {
    let report = PreflightReport {
        input_revision: ProjectRevision(9),
        disposition: PreflightDisposition::Unresolved,
        software_only: true,
        checks: vec![PreflightCheck {
            code: "height_analysis".into(),
            severity: PreflightSeverity::Warning,
            evidence: PreflightEvidence::Indeterminate,
            required: true,
            message: "Height analysis has unresolved obligations".into(),
            occurrence_id: None,
            face_id: None,
        }],
    };
    let value = serde_json::to_value(&report).unwrap();
    assert_eq!(value["disposition"], "unresolved");
    assert_eq!(value["checks"][0]["evidence"], "indeterminate");
    assert_eq!(
        serde_json::from_value::<PreflightReport>(value.clone()).unwrap(),
        report
    );
    for field in ["evidence", "required"] {
        let mut missing = value.clone();
        missing["checks"][0].as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<PreflightReport>(missing).is_err());
    }
    let mut missing = value.clone();
    missing.as_object_mut().unwrap().remove("disposition");
    assert!(serde_json::from_value::<PreflightReport>(missing).is_err());
    let mut legacy = value.clone();
    legacy["eligible"] = serde_json::json!(false);
    assert!(serde_json::from_value::<PreflightReport>(legacy).is_err());
    for invalid in ["unspecified", "unknown", "passed"] {
        let mut changed = value.clone();
        changed["checks"][0]["evidence"] = serde_json::json!(invalid);
        assert!(serde_json::from_value::<PreflightReport>(changed).is_err());
        let mut changed = value.clone();
        changed["disposition"] = serde_json::json!(invalid);
        assert!(serde_json::from_value::<PreflightReport>(changed).is_err());
    }
}

#[test]
fn durable_contact_gaps_enforce_minimum_and_real_interface_context() {
    let mut data = intent();
    data.printer.capabilities.generated_supports = true;
    data.recipe.layer_height_mm = 0.25;
    data.recipe.supports = SupportPolicy::Breakaway {
        density: 0.5,
        xy_clearance_mm: 0.3,
        z_gap_mm: 0.25,
        interface_thickness_mm: 0.2,
    };
    let mut scheduled = plan();
    scheduled.layers[0].z_mm = 5.25;
    scheduled.layers[0].height_mm = 0.25;
    scheduled.layers[0].section_z_mm = 5.125;
    for path in &mut scheduled.layers[0].paths {
        for point in &mut path.points_mm {
            point[2] = 5.25;
        }
    }
    let mut object = scheduled.layers[0].clone();
    object.index = 2;
    object.z_mm = 5.75;
    object.section_z_mm = 5.625;
    for path in &mut object.paths {
        for point in &mut path.points_mm {
            point[2] = 5.75;
        }
    }
    let mut zero = coverage();
    zero.section_area_mm2 = 0.0;
    zero.source_section_area_mm2 = 0.0;
    zero.wall_area_mm2 = 0.0;
    let support = &mut scheduled.layers[0];
    support.region_coverage = zero.clone();
    support.region_coverage.support_area_mm2 = 50.0;
    support
        .region_coverage
        .support_nominal_deposition_volume_mm3 = 1.0;
    for path in &mut support.paths {
        path.kind = DepositionKind::SupportInterface;
    }
    scheduled.layers.push(PrintLayer {
        index: 1,
        z_mm: 5.5,
        height_mm: 0.25,
        section_z_mm: 5.375,
        region_coverage: zero,
        paths: Vec::new(),
    });
    scheduled.layers.push(object);
    scheduled.native_geometry.certified_layer_count = scheduled.layers.len() as u32;
    scheduled.native_geometry.slab_count = scheduled.layers.len() as u32;
    scheduled
        .native_geometry
        .source_feature_coverage
        .disjoint_source_layer_count = scheduled.layers.len() as u32;
    let bytes = serde_json::to_vec(&scheduled).unwrap();
    let admitted: NormalizedPrintPlan = serde_json::from_slice(&bytes).unwrap();
    admitted.validate(&data).unwrap();
    let mut without_interface = admitted.clone();
    for path in &mut without_interface.layers[0].paths {
        path.kind = DepositionKind::Support;
    }
    assert!(without_interface.validate(&data).is_err());
    data.recipe.supports = SupportPolicy::Disabled;
    assert!(admitted.validate(&data).is_err());
    data.recipe.supports = SupportPolicy::Breakaway {
        density: 0.5,
        xy_clearance_mm: 0.3,
        z_gap_mm: 0.2,
        interface_thickness_mm: 0.2,
    };
    admitted.validate(&data).unwrap();
    if let SupportPolicy::Breakaway { z_gap_mm, .. } = &mut data.recipe.supports {
        *z_gap_mm = 0.3;
    }
    assert!(admitted.validate(&data).is_err());
}

#[test]
fn surface_fingerprint_preserves_order_disabled_layers_and_smooth_authority() {
    use crate::surface::*;
    let (project, definitions, occurrences) = inputs();
    let intent = intent();
    let scope = SurfaceScope::Definition {
        definition_id: definitions[0].definition_id.clone(),
    };
    let mut doc = SurfaceOverlays {
        schema_version: SURFACE_SCHEMA_VERSION,
        images: vec![],
        layers: vec![
            SurfaceLayer {
                layer_id: SurfaceLayerId::new(1).unwrap(),
                enabled: false,
                scope: scope.clone(),
                face_id: FaceId::from_step_entity(7).unwrap(),
                finish: SurfaceFinish::Smooth,
            },
            SurfaceLayer {
                layer_id: SurfaceLayerId::new(2).unwrap(),
                enabled: true,
                scope,
                face_id: FaceId::from_step_entity(7).unwrap(),
                finish: SurfaceFinish::VerticalRibs {
                    depth_mm: 0.1,
                    pitch_mm: 2.0,
                    edge_margin_mm: 0.8,
                    phase_mm: 0.0,
                },
            },
        ],
    };
    let hash = input_fingerprint(&project, &definitions, &occurrences, &intent, &doc).unwrap();
    doc.layers.reverse();
    assert_ne!(
        hash,
        input_fingerprint(&project, &definitions, &occurrences, &intent, &doc).unwrap()
    );
    doc.layers.reverse();
    doc.layers[0].enabled = true;
    assert_ne!(
        hash,
        input_fingerprint(&project, &definitions, &occurrences, &intent, &doc).unwrap()
    );
    doc.layers[0].enabled = false;
    let mut cylindrical_hashes = std::collections::BTreeSet::new();
    for finish in [
        SurfaceFinish::CylindricalRibs {
            depth_mm: 0.1,
            rib_count: 16,
            phase_fraction: 0.0,
        },
        SurfaceFinish::CylindricalRibs {
            depth_mm: 0.2,
            rib_count: 16,
            phase_fraction: 0.0,
        },
        SurfaceFinish::CylindricalRibs {
            depth_mm: 0.1,
            rib_count: 17,
            phase_fraction: 0.0,
        },
        SurfaceFinish::CylindricalRibs {
            depth_mm: 0.1,
            rib_count: 16,
            phase_fraction: 0.25,
        },
    ] {
        doc.layers[1].finish = finish;
        let current =
            input_fingerprint(&project, &definitions, &occurrences, &intent, &doc).unwrap();
        assert_ne!(hash, current);
        assert!(
            cylindrical_hashes.insert(current.as_str().to_owned()),
            "depth/count/phase must each change the cylindrical fingerprint"
        );
        doc.layers[0].finish = doc.layers[1].finish.clone();
        assert_ne!(
            current,
            input_fingerprint(&project, &definitions, &occurrences, &intent, &doc).unwrap()
        );
        doc.layers[0].finish = SurfaceFinish::Smooth;
    }
    doc.layers.remove(0);
    assert_ne!(
        hash,
        input_fingerprint(&project, &definitions, &occurrences, &intent, &doc).unwrap()
    );
}
#[test]
fn realized_material_area_requires_exact_source_plus_addition_and_plan_bound_presence() {
    let intent = intent();
    let mut area = coverage();
    area.source_section_area_mm2 = 49.0;
    area.surface_added_area_mm2 = 1.0;
    area.validate(&intent.recipe).unwrap();
    area.surface_added_area_mm2 = 2.0;
    assert!(area.validate(&intent.recipe).is_err());
    area.surface_added_area_mm2 = -1.0;
    assert!(area.validate(&intent.recipe).is_err());
    let mut old = serde_json::to_value(plan()).unwrap();
    old.as_object_mut().unwrap().remove("surface_tolerance_mm");
    assert!(serde_json::from_value::<NormalizedPrintPlan>(old).is_err());
    let mut weak_bound = plan();
    weak_bound.surface_tolerance_mm = 0.002;
    assert!(weak_bound.validate(&intent).is_err());
    let mut old_profile = intent;
    old_profile.printer.schema_version = 4;
    assert!(old_profile.validate().is_err());
}

fn image_bundle() -> ManufacturingBundle {
    use crate::surface::*;
    let mut bundle = bundle();
    let bytes: Arc<Vec<u8>> = [b"P5\n2 2\n255\n".as_slice(), &[0, 64, 128, 255]]
        .concat()
        .into();
    let image = SurfaceImageRecord {
        image_hash: SourceHash::from_bytes(&bytes),
        width: 2,
        height: 2,
        byte_count: bytes.len() as u64,
        source_hash: SourceHash::from_bytes(b"encoded png"),
        source_name: "height.png".into(),
    };
    let parameters = ImageReliefParameters {
        depth_mm: 1.0,
        u_repeats: 3,
        u_phase_fraction: 0.125,
        z_period_mm: 2.0,
        z_phase_mm: 0.25,
        edge_margin_mm: 0.8,
        vertical_error_mm: 0.025,
    };
    bundle.provenance.surface_overlays = SurfaceOverlays {
        schema_version: SURFACE_SCHEMA_VERSION,
        images: vec![image.clone()],
        layers: vec![SurfaceLayer {
            layer_id: SurfaceLayerId::new(1).unwrap(),
            enabled: true,
            scope: SurfaceScope::Definition {
                definition_id: bundle.provenance.definitions[0].definition_id.clone(),
            },
            face_id: FaceId::from_step_entity(7).unwrap(),
            finish: SurfaceFinish::ImageRelief {
                image_hash: image.image_hash.clone(),
                parameters,
            },
        }],
    };
    bundle.provenance.surface_images = vec![SurfaceImageContent {
        image_hash: image.image_hash.clone(),
        bytes,
    }];
    bundle.provenance.input_hash = input_fingerprint(
        &bundle.provenance.project_id,
        &bundle.provenance.definitions,
        &bundle.provenance.occurrences,
        &bundle.provenance.intent,
        &bundle.provenance.surface_overlays,
    )
    .unwrap();
    bundle.plan.surface_resolution = vec![SurfaceResolution {
        surface_layer_id: SurfaceLayerId::new(1).unwrap(),
        occurrence_id: bundle.provenance.occurrences[0].occurrence_id,
        face_id: FaceId::from_step_entity(7).unwrap(),
        image_hash: image.image_hash,
        layer_index: 0,
        source_z_mm: 5.1,
        evaluated_witnesses: 2,
        resolved: 0,
        ambiguous: 1,
        below_nominal_resolution: 1,
        unavailable: 0,
        coverage_complete: true,
        representatives_truncated: false,
        max_residual_lower_mm: 0.2,
        max_residual_upper_mm: 0.3,
        representatives: vec![
            SurfaceFeatureResolution {
                source_u: 0.25,
                source_z_mm: 5.1,
                cell: 0,
                repeat: 0,
                classification: SurfaceFeatureClass::Ambiguous,
                residual_lower_mm: 0.0,
                residual_upper_mm: 0.1,
            },
            SurfaceFeatureResolution {
                source_u: 0.75,
                source_z_mm: 5.1,
                cell: 1,
                repeat: 2,
                classification: SurfaceFeatureClass::BelowNominalResolution,
                residual_lower_mm: 0.2,
                residual_upper_mm: 0.3,
            },
        ],
    }];
    bundle
}
#[test]
fn image_bundle_is_self_contained_and_inspection_never_carries_pixels() {
    let bundle = image_bundle();
    bundle.validate().unwrap();
    let cloned = bundle.clone();
    assert!(Arc::ptr_eq(
        &bundle.provenance.surface_images[0].bytes,
        &cloned.provenance.surface_images[0].bytes
    ));
    let decoded = decode_bundle(&serde_json::to_vec(&bundle).unwrap()).unwrap();
    assert_eq!(decoded, bundle);
    assert_eq!(
        bundle.summary().surface_resolution,
        SurfaceResolutionTotals {
            evaluated_witnesses: 2,
            resolved: 0,
            ambiguous: 1,
            below_nominal_resolution: 1,
            unavailable: 0,
            coverage_complete: true
        }
    );
    let inspection = ManufacturingInspectionProvenance::from(&bundle.provenance);
    let value = serde_json::to_value(&inspection).unwrap();
    assert!(value.get("surface_images").is_none());
    assert_eq!(
        value["surface_overlays"]["images"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        value["surface_overlays"]["images"][0]
            .get("bytes")
            .is_none()
    );
    let wire: crate::rpc::ManufacturingInspectionProvenance =
        inspection.clone().try_into().unwrap();
    assert_eq!(
        ManufacturingInspectionProvenance::try_from(wire).unwrap(),
        inspection
    );
    let mut missing = bundle.clone();
    missing.provenance.surface_images.clear();
    assert!(missing.validate().is_err());
    let mut corrupt = bundle.clone();
    Arc::make_mut(&mut corrupt.provenance.surface_images[0].bytes)[11] ^= 1;
    assert!(corrupt.validate().is_err());
    let mut unknown = bundle.clone();
    unknown.provenance.surface_images[0].image_hash = SourceHash::from_bytes(b"unknown");
    assert!(unknown.validate().is_err());
    let mut old = serde_json::to_value(&bundle).unwrap();
    old["schema_version"] = serde_json::json!(4);
    old["provenance"]
        .as_object_mut()
        .unwrap()
        .remove("surface_images");
    old["plan"]
        .as_object_mut()
        .unwrap()
        .remove("surface_resolution");
    assert_eq!(
        decode_bundle(&serde_json::to_vec(&old).unwrap())
            .unwrap_err()
            .code,
        ManufacturingErrorCode::InvalidSpecification
    );
    let mut oversized = serde_json::to_value(&bundle.provenance).unwrap();
    oversized["surface_images"] = serde_json::json!(vec![
        serde_json::to_value(
            &bundle.provenance.surface_images[0]
        )
        .unwrap();
        crate::surface::MAX_SURFACE_IMAGES + 1
    ]);
    assert!(serde_json::from_value::<ManufacturingProvenance>(oversized).is_err());
}
#[test]
fn image_fingerprint_binds_every_mapping_and_first_admission_provenance_even_disabled() {
    use crate::surface::SurfaceFinish;
    let bundle = image_bundle();
    let original = bundle.provenance.input_hash.clone();
    let fingerprint = |overlays: &SurfaceOverlays| {
        input_fingerprint(
            &bundle.provenance.project_id,
            &bundle.provenance.definitions,
            &bundle.provenance.occurrences,
            &bundle.provenance.intent,
            overlays,
        )
        .unwrap()
    };
    let mut disabled = bundle.provenance.surface_overlays.clone();
    disabled.layers[0].enabled = false;
    assert_ne!(fingerprint(&disabled), original);
    let disabled_hash = fingerprint(&disabled);
    for field in 0..7 {
        let mut changed = disabled.clone();
        let SurfaceFinish::ImageRelief { parameters, .. } = &mut changed.layers[0].finish else {
            unreachable!()
        };
        match field {
            0 => parameters.depth_mm += 0.1,
            1 => parameters.u_repeats += 1,
            2 => parameters.u_phase_fraction += 0.01,
            3 => parameters.z_period_mm += 0.1,
            4 => parameters.z_phase_mm += 0.1,
            5 => parameters.edge_margin_mm += 0.1,
            _ => parameters.vertical_error_mm += 0.001,
        }
        assert_ne!(fingerprint(&changed), disabled_hash);
    }
    let mut changed = disabled.clone();
    changed.images[0].source_hash = SourceHash::from_bytes(b"different encoded alpha provenance");
    assert_ne!(fingerprint(&changed), disabled_hash);
    let mut changed = disabled;
    changed.images[0].source_name = "renamed.png".into();
    assert_ne!(fingerprint(&changed), disabled_hash);
}
#[test]
fn resolution_validation_is_strict_and_advisory_attribution_cannot_drift() {
    let bundle = image_bundle();
    let record = &bundle.plan.surface_resolution[0];
    record.validate().unwrap();
    for field in 0..10 {
        let mut changed = record.clone();
        match field {
            0 => changed.evaluated_witnesses += 1,
            1 => changed.resolved = u32::MAX,
            2 => changed.max_residual_lower_mm = -0.1,
            3 => changed.max_residual_upper_mm = f64::NAN,
            4 => changed.representatives_truncated = true,
            5 => changed.representatives[0].source_z_mm += 0.1,
            6 => changed.representatives[0].classification = SurfaceFeatureClass::Resolved,
            7 => changed.representatives[1].residual_lower_mm = 0.0,
            8 => changed.representatives[0].source_u = 1.0,
            _ => changed.representatives = vec![changed.representatives[0].clone(); 9],
        }
        assert!(changed.validate().is_err());
    }
    for field in 0..7 {
        let mut changed = bundle.clone();
        let record = &mut changed.plan.surface_resolution[0];
        match field {
            0 => record.layer_index = 1,
            1 => record.occurrence_id = OccurrenceId::new(2).unwrap(),
            2 => record.surface_layer_id = crate::surface::SurfaceLayerId::new(2).unwrap(),
            3 => record.face_id = FaceId::from_step_entity(8).unwrap(),
            4 => record.image_hash = SourceHash::from_bytes(b"other image"),
            5 => record.representatives[0].cell = 2,
            _ => record.representatives[0].repeat = 3,
        }
        assert!(changed.validate().is_err());
    }
    let mut duplicate = bundle.clone();
    duplicate.plan.surface_resolution.push(record.clone());
    assert!(duplicate.validate().is_err());
    let mut blank = record.clone();
    blank.evaluated_witnesses = 0;
    blank.ambiguous = 0;
    blank.below_nominal_resolution = 0;
    blank.max_residual_lower_mm = 0.0;
    blank.max_residual_upper_mm = 0.0;
    blank.representatives.clear();
    blank.validate().unwrap();
    let mut unavailable = blank;
    unavailable.evaluated_witnesses = 1;
    unavailable.unavailable = 1;
    unavailable.representatives_truncated = true;
    assert!(unavailable.validate().is_err());
    unavailable.coverage_complete = false;
    unavailable.validate().unwrap();
    let mut totals = SurfaceResolutionTotals::from_records(&[unavailable]);
    totals.validate().unwrap();
    totals.evaluated_witnesses += 1;
    assert!(totals.validate().is_err());
    assert_eq!(
        self::bundle().summary().surface_resolution,
        SurfaceResolutionTotals::empty()
    );
}

#[test]
fn xyz6_gap_refuses_unbounded_outward_result_despite_finite_head_and_compensation() {
    // The exact difference is below -MAX, although rounded subtraction and
    // every TwoDiff intermediate are finite. Outward evidence must refuse.
    let above = -f64::MAX;
    let below = 1.0;
    assert!((above - below).is_finite());
    let failure = program_coordinates::enclosed_vertical_gap_mm(above, below).unwrap_err();
    assert_eq!(
        failure.code,
        ManufacturingErrorCode::ProgramCoordinateConditioningFailure
    );
}

#[test]
fn xyz6_gap_rejects_nonfinite_compensation_despite_finite_subtraction_head() {
    let below = f64::MAX;
    let above = 1.5 * (f64::MAX - f64::MAX.next_down());
    let head = above - below;
    assert!(head.is_finite());
    assert!(!(above - head).is_finite());
    let failure = program_coordinates::enclosed_vertical_gap_mm(above, below).unwrap_err();
    assert_eq!(
        failure.code,
        ManufacturingErrorCode::ProgramCoordinateConditioningFailure
    );
}
