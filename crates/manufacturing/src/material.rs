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
                0 => selected.profile.printer_revision = "wrong".into(),
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
