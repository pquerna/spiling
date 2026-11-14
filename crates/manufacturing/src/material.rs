// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Deterministic process resolution before native planning. Catalogs are inert.
use spiling_contracts::manufacturing::*;
mod review;
pub use review::review_materials;

pub fn resolve_material_profile(
    current: &ManufacturingIntent,
    selection: MaterialProfileSelection,
) -> Result<ManufacturingIntent, ManufacturingError> {
    current.validate()?;
    selection.validate()?;
    let mut resolved = current.clone();
    let profile = &selection.profile;
    resolved.recipe.material_id.clone_from(&profile.material.id);
    resolved
        .recipe
        .material_revision
        .clone_from(&profile.material.revision);
    resolved.recipe.filament_diameter_mm = profile.material.filament_diameter_mm;
    resolved.recipe.print_speed_mm_s = selection.print_speed_mm_s();
    resolved.recipe.flow_multiplier = selection.flow_multiplier();
    resolved.recipe.process = selection.process().clone();
    let JobWorkflow::CompleteMarlin { job } = &mut resolved.printer.workflow else {
        return Err(ManufacturingError::new(
            ManufacturingErrorCode::InvalidSpecification,
            "material profiles currently require a complete single-extruder Marlin job",
        ));
    };
    job.nozzle_temperature_c = selection.nozzle_temperature_c();
    job.bed_temperature_c = selection.bed_temperature_c();
    resolved.material_profile = Some(ResolvedMaterialProfile {
        resolver_version: MATERIAL_RESOLVER_VERSION,
        selection,
    });
    resolved.validate()?;
    Ok(resolved)
}

/// Resolve one explicitly addressed loaded tool; preserve all other tool captures.
pub fn resolve_tool_material_profile(
    current: &ManufacturingIntent,
    tool_id: &str,
    selection: MaterialProfileSelection,
) -> Result<ManufacturingIntent, ManufacturingError> {
    current.validate()?;
    selection.validate()?;
    let mut resolved = current.clone();
    let JobWorkflow::FixedDualMarlin { job } = &mut resolved.printer.workflow else {
        return Err(ManufacturingError::new(
            ManufacturingErrorCode::InvalidSpecification,
            "an addressed tool profile requires a fixed-dual job",
        ));
    };
    let tool = job
        .tools
        .iter_mut()
        .find(|tool| tool.id == tool_id)
        .ok_or_else(|| {
            ManufacturingError::new(
                ManufacturingErrorCode::InvalidSpecification,
                "unknown loaded tool profile target",
            )
        })?;
    // A process profile is not permission to silently replace a loaded formulation
    // or installed nozzle. Contract validation binds both to the existing capture.
    tool.nozzle_temperature_c = selection.nozzle_temperature_c();
    tool.material_profile = Some(ResolvedMaterialProfile {
        resolver_version: MATERIAL_RESOLVER_VERSION,
        selection,
    });
    resolved.validate()?;
    Ok(resolved)
}

/// Create the first complete, explicit recipe from an operator-selected setup
/// and material. This is creation, never a fallback for omitted wire fields or
/// an overwrite of an existing recipe. Captured values survive saved return.
pub fn create_material_intent(
    hardware: std::sync::Arc<spiling_contracts::hardware::ProjectHardwareCapture>,
    selection: MaterialProfileSelection,
) -> Result<ManufacturingIntent, ManufacturingError> {
    hardware
        .validate()
        .map_err(|e| ManufacturingError::new(ManufacturingErrorCode::InvalidSpecification, e))?;
    selection.validate()?;
    let mut printer = hardware.setup.printer.clone();
    let JobWorkflow::CompleteMarlin { job } = &mut printer.workflow else {
        return Err(ManufacturingError::new(
            ManufacturingErrorCode::UnsupportedCapability,
            "standard material settings require an explicitly selected complete single-tool Marlin setup",
        ));
    };
    job.nozzle_temperature_c = selection.nozzle_temperature_c();
    job.bed_temperature_c = selection.bed_temperature_c();
    let limits = printer.deposition_limits;
    let profile_limits = selection.profile.deposition_limits;
    let min_width = limits
        .min_bead_width_mm
        .max(profile_limits.min_bead_width_mm);
    let max_width = limits
        .max_bead_width_mm
        .min(profile_limits.max_bead_width_mm);
    let min_height = limits
        .min_bead_height_mm
        .max(profile_limits.min_bead_height_mm);
    let max_height = limits
        .max_bead_height_mm
        .min(profile_limits.max_bead_height_mm);
    if min_width > max_width || min_height > max_height || min_height > max_width {
        return Err(ManufacturingError::new(
            ManufacturingErrorCode::InvalidSpecification,
            "selected setup and material have no common bead envelope",
        ));
    }
    let width = (printer.nozzle_diameter_mm * 1.125)
        .max(min_height)
        .clamp(min_width, max_width);
    let height = 0.2_f64.clamp(min_height, max_height.min(width));
    let material = &selection.profile.material;
    let mut intent = ManufacturingIntent {
        hardware: Some(hardware),
        material_profile: Some(ResolvedMaterialProfile {
            resolver_version: MATERIAL_RESOLVER_VERSION,
            selection: selection.clone(),
        }),
        recipe: PlanarPrintRecipe {
            id: "spiling.standard-planar".into(),
            revision: "1".into(),
            material_id: material.id.clone(),
            material_revision: material.revision.clone(),
            filament_diameter_mm: material.filament_diameter_mm,
            layer_height_mm: height,
            layer_height_policy: LayerHeightPolicy::Preferred,
            native_shape_error_mm: 0.01,
            bead_width_mm: width,
            sparse_infill: SparseInfillPolicy::Fine,
            perimeter_count: 3,
            infill_fraction: 0.15,
            top_skin_thickness_mm: 0.8_f64.max(height),
            bottom_skin_thickness_mm: 0.8_f64.max(height),
            print_speed_mm_s: selection.print_speed_mm_s(),
            travel_speed_mm_s: printer.max_travel_feed_mm_s,
            flow_multiplier: selection.flow_multiplier(),
            process: selection.process().clone(),
            supports: SupportPolicy::Disabled,
            seam_policy: SeamPolicy::Nearest,
            rectilinear_axis: RectilinearAxisPolicy::AlternatingXY,
            top_skin_axis: RectilinearAxisPolicy::AlternatingXY,
        },
        printer,
    };
    intent.recipe.supports = intent.automatic_support_policy()?;
    intent.validate()?;
    Ok(intent)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn intent() -> ManufacturingIntent {
        decode_intent(include_bytes!(
            "../../../fixtures/manufacturing/complete-sparse.intent.json"
        ))
        .unwrap()
    }
    fn selection() -> MaterialProfileSelection {
        serde_json::from_slice(include_bytes!(
            "../../../fixtures/manufacturing/material-profile-a.json"
        ))
        .unwrap()
    }
    #[test]
    fn resolution_preserves_hardware_and_captures_overrides_and_identity() {
        let original = intent();
        let mut selected = selection();
        selected.overrides.nozzle_temperature_c = Some(210.0);
        selected.overrides.print_speed_mm_s = Some(25.0);
        let resolved = resolve_material_profile(&original, selected.clone()).unwrap();
        assert_eq!(
            resolved.printer.max_volumetric_flow_mm3_s,
            original.printer.max_volumetric_flow_mm3_s
        );
        assert_eq!(resolved.effective_max_volumetric_flow_mm3_s(), 6.0);
        assert_eq!(resolved.recipe.print_speed_mm_s, 25.0);
        assert_eq!(
            resolved.material_profile.as_ref().unwrap().selection,
            selected
        );
        let JobWorkflow::CompleteMarlin { job } = &resolved.printer.workflow else {
            panic!("complete job")
        };
        assert_eq!(job.nozzle_temperature_c, 210.0);
        assert_eq!(
            decode_intent(&serde_json::to_vec(&resolved).unwrap()).unwrap(),
            resolved
        );
        let mut disagreement = resolved.clone();
        disagreement.recipe.print_speed_mm_s = 26.0;
        assert!(
            disagreement
                .validate()
                .unwrap_err()
                .message
                .contains("disagrees")
        );
        disagreement = resolved.clone();
        disagreement
            .material_profile
            .as_mut()
            .unwrap()
            .resolver_version += 1;
        assert!(disagreement.validate().is_err());
        assert_eq!(
            resolve_material_profile(&resolved, selected).unwrap(),
            resolved
        );
        assert_eq!(original.material_profile, None);
    }
    #[test]
    fn per_tool_resolution_keeps_other_capture_and_rejects_shared_conflicts() {
        let current = decode_intent(include_bytes!(
            "../../../fixtures/manufacturing/fixed-dual-profiles.intent.json"
        ))
        .unwrap();
        let JobWorkflow::FixedDualMarlin { job } = &current.printer.workflow else {
            unreachable!()
        };
        let mut selected = job.tools[0]
            .material_profile
            .as_ref()
            .unwrap()
            .selection
            .clone();
        selected.overrides.print_speed_mm_s = Some(27.0);
        let result =
            resolve_tool_material_profile(&current, &job.tools[0].id, selected.clone()).unwrap();
        let JobWorkflow::FixedDualMarlin { job: changed } = &result.printer.workflow else {
            unreachable!()
        };
        assert_eq!(job.tools[1], changed.tools[1]);
        assert_eq!(changed.tools[0].print_speed_mm_s(&result.recipe), 27.0);
        for case in 0..6 {
            let mut invalid = selected.clone();
            match case {
                0 => invalid.overrides.bed_temperature_c = Some(70.0),
                1 => invalid.profile.material.id = "different-product".into(),
                2 => invalid.profile.nozzle_diameter_mm = 0.6,
                3 => {
                    invalid.profile.cooling_configuration_id = "other-fan".into();
                    invalid.setup.cooling_configuration_id = "other-fan".into();
                }
                4 => invalid.overrides.nozzle_temperature_c = Some(300.0),
                _ => invalid.profile.process.max_bridge_span_mm += 1.0,
            }
            assert!(
                resolve_tool_material_profile(&current, &job.tools[0].id, invalid).is_err(),
                "case{case}"
            );
        }
        assert!(resolve_tool_material_profile(&current, "missing-tool", selected).is_err());
        assert!(current.validate().is_ok());
    }
    #[test]
    fn incompatible_profiles_and_unallowed_limits_refuse_without_clamping() {
        let original = intent();
        for index in 0..10 {
            let mut selected = selection();
            match index {
                0 => selected.profile.nozzle_diameter_mm = 0.8,
                1 => selected.profile.nozzle_diameter_mm = 0.6,
                2 => selected.setup.cooling_configuration_id = "wrong".into(),
                3 => selected.profile.material.requires_abrasion_resistant_nozzle = true,
                4 => selected.profile.max_volumetric_flow_mm3_s = 0.01,
                5 => selected.overrides.nozzle_temperature_c = Some(260.0),
                6 => selected.profile.deposition_limits.max_bead_width_mm = 0.4,
                7 => selected.overrides.flow_multiplier = Some(f64::NAN),
                8 => selected.setup.hotend_id = "changed-hotend".into(),
                _ => selected.setup.nozzle_material = "changed-nozzle-material".into(),
            }
            assert!(
                resolve_material_profile(&original, selected).is_err(),
                "case {index}"
            );
        }
        assert!(original.validate().is_ok());
        assert_eq!(original.material_profile, None);
    }
}

#[cfg(test)]
mod creation_tests {
    use super::*;
    use spiling_contracts::hardware::*;
    use std::sync::Arc;

    #[test]
    fn first_material_creates_portable_defaults_and_keeps_installed_limits_authoritative() {
        let catalog = builtin_catalog().unwrap();
        let hardware = Arc::new(ProjectHardwareCapture {
            setup: catalog
                .setups
                .into_iter()
                .find(|s| matches!(s.printer.workflow, JobWorkflow::CompleteMarlin { .. }))
                .unwrap(),
            plate: catalog.plates[0].clone(),
            face: "textured".into(),
            accessories: Vec::new(),
        });
        let mut selection: MaterialProfileSelection = serde_json::from_slice(include_bytes!(
            "../../../fixtures/manufacturing/material-profile-a.json"
        ))
        .unwrap();
        selection.overrides.nozzle_temperature_c = Some(215.0);
        let intent = create_material_intent(hardware.clone(), selection.clone()).unwrap();
        assert!(matches!(
            intent.recipe.supports,
            SupportPolicy::Breakaway { .. }
        ));
        assert_eq!(intent.hardware, Some(hardware.clone()));
        intent.validate().unwrap();
        let restored = decode_intent(&serde_json::to_vec(&intent).unwrap()).unwrap();
        assert_eq!(restored, intent);
        let mut forged = intent.clone();
        forged.printer.max_print_feed_mm_s *= 0.9;
        assert!(
            forged.validate().is_err(),
            "profile thermal resolution cannot change installed capabilities"
        );
        let mut forged = intent.clone();
        if let JobWorkflow::CompleteMarlin { job } = &mut forged.printer.workflow {
            job.nozzle_temperature_c += 1.0;
        }
        assert!(
            forged.validate().is_err(),
            "thermal target requires captured material authority"
        );
        let mut explicit_off = intent.clone();
        explicit_off.recipe.supports = SupportPolicy::Disabled;
        let resolved = resolve_material_profile(&explicit_off, selection).unwrap();
        assert_eq!(
            resolved.recipe.supports,
            SupportPolicy::Disabled,
            "profile changes cannot re-enable authored Off"
        );
        // Profiles are keyed by compatibility, not printer identity: another
        // printer with the same nozzle keeps the profile; another nozzle refuses it.
        let selection = intent.material_profile.unwrap().selection;
        let mut renamed = hardware.as_ref().clone();
        renamed.setup.printer.id.push_str("-other");
        create_material_intent(Arc::new(renamed), selection.clone()).unwrap();
        let mut wrong = hardware.as_ref().clone();
        wrong.setup.printer.nozzle_diameter_mm = 0.6;
        assert!(create_material_intent(Arc::new(wrong), selection).is_err());
    }
}
