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
    let selection = fit_selection(selection, &resolved.printer)?;
    let profile = &selection.profile;
    resolved.recipe.material_id.clone_from(&profile.material.id);
    resolved
        .recipe
        .material_revision
        .clone_from(&profile.material.revision);
    resolved.recipe.filament_diameter_mm = profile.material.filament_diameter_mm;
    resolved.recipe.print_speed_mm_s = selection.print_speed_mm_s();
    resolved.recipe.flow_multiplier = selection.flow_multiplier();
    resolved.recipe.process = selection.process();
    let JobWorkflow::CompleteMarlin { job } = &mut resolved.printer.workflow else {
        return Err(ManufacturingError::new(
            ManufacturingErrorCode::InvalidSpecification,
            "material profiles currently require a complete single-extruder Marlin job",
        ));
    };
    apply_thermal(job, &selection);
    resolved.material_profile = Some(ResolvedMaterialProfile {
        resolver_version: MATERIAL_RESOLVER_VERSION,
        selection,
    });
    resolved.validate()?;
    Ok(resolved)
}

/// Explicit overrides a printer needs to accept a profile: the material
/// print speed within the printer's print feed, and an off bed on a machine
/// without a heated bed (refused when the material needs one).
fn fit_selection(
    mut selection: MaterialProfileSelection,
    printer: &PrinterSpecification,
) -> Result<MaterialProfileSelection, ManufacturingError> {
    if selection.print_speed_mm_s() > printer.max_print_feed_mm_s {
        selection.overrides.print_speed_mm_s = Some(printer.max_print_feed_mm_s);
    }
    // Temperatures beyond the machine's declared limits clamp to them.
    if let JobWorkflow::CompleteMarlin { job } = &printer.workflow {
        let nozzle = |t: f64| {
            t.clamp(
                job.minimum_extrusion_temperature_c,
                job.maximum_nozzle_temperature_c,
            )
        };
        let o = &mut selection.overrides;
        let (n, f) = (
            selection.profile.nozzle_temperature_c,
            selection.profile.first_layer_nozzle_temperature_c,
        );
        if o.nozzle_temperature_c.is_none() && nozzle(n) != n {
            o.nozzle_temperature_c = Some(nozzle(n));
        }
        if o.first_layer_nozzle_temperature_c.is_none() && nozzle(f) != f {
            o.first_layer_nozzle_temperature_c = Some(nozzle(f));
        }
        let max_bed = job.maximum_bed_temperature_c;
        let (b, fb) = (
            selection.profile.bed_temperature_c,
            selection.profile.first_layer_bed_temperature_c,
        );
        if max_bed > 0.0 && o.bed_temperature_c.is_none() && b > max_bed {
            o.bed_temperature_c = Some(max_bed);
        }
        if max_bed > 0.0 && o.first_layer_bed_temperature_c.is_none() && fb > max_bed {
            o.first_layer_bed_temperature_c = Some(max_bed);
        }
    }
    if let JobWorkflow::CompleteMarlin { job } = &printer.workflow
        && job.maximum_bed_temperature_c == 0.0
    {
        if selection.profile.requires_heated_bed {
            return Err(ManufacturingError::new(
                ManufacturingErrorCode::UnsupportedCapability,
                "the selected material requires a heated bed and the printer declares none",
            ));
        }
        selection.overrides.bed_temperature_c = Some(0.0);
        selection.overrides.first_layer_bed_temperature_c = Some(0.0);
    }
    selection.validate()?;
    Ok(selection)
}

/// Job temperatures and the lifecycle's standby nozzle from a selection.
fn apply_thermal(job: &mut SingleExtruderJob, selection: &MaterialProfileSelection) {
    job.nozzle_temperature_c = selection.nozzle_temperature_c();
    job.bed_temperature_c = selection.bed_temperature_c();
    job.first_layer_nozzle_temperature_c = selection.first_layer_nozzle_temperature_c();
    job.first_layer_bed_temperature_c = selection.first_layer_bed_temperature_c();
    let standby = selection.profile.standby_nozzle_temperature_c;
    for step in &mut job.lifecycle.start {
        if let LifecycleStep::NozzleTarget { standby_c } | LifecycleStep::NozzleWait { standby_c } =
            step
            && standby_c.is_some()
        {
            *standby_c = Some(standby);
        }
    }
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

fn r6(v: f64) -> f64 {
    (v * 1_000_000.0).round() / 1_000_000.0
}

/// Create the first complete, explicit recipe from an operator-selected setup
/// and material at standard quality. This is creation, never a fallback for
/// omitted wire fields or an overwrite of an existing recipe.
pub fn create_material_intent(
    hardware: std::sync::Arc<spiling_contracts::hardware::ProjectHardwareCapture>,
    selection: MaterialProfileSelection,
) -> Result<ManufacturingIntent, ManufacturingError> {
    create_default_intent(
        hardware,
        selection,
        QualityPreset::Standard,
        SupportChoice::Automatic,
    )
}

/// The defaults: printer + material + quality. Role speeds come from the
/// machine description (a declared software setup inherits the generic FFF
/// family), each within the printer's print feed; the volumetric flow and
/// minimum layer time slow them where needed and the verifier reports it.
/// Two walls, 15 % sparse infill, top skin max(0.7 mm, 5 layers), bottom
/// max(0.5 mm, 3 layers) and, when requested, the printer's automatic
/// supports. The recipe identity names the quality preset. Captured values
/// survive saved return.
pub fn create_default_intent(
    hardware: std::sync::Arc<spiling_contracts::hardware::ProjectHardwareCapture>,
    selection: MaterialProfileSelection,
    quality: QualityPreset,
    supports: SupportChoice,
) -> Result<ManufacturingIntent, ManufacturingError> {
    let invalid =
        |e: String| ManufacturingError::new(ManufacturingErrorCode::InvalidSpecification, e);
    hardware.validate().map_err(invalid)?;
    selection.validate()?;
    let mut printer = hardware.setup.printer.clone();
    let selection = fit_selection(selection, &printer)?;
    let JobWorkflow::CompleteMarlin { job } = &mut printer.workflow else {
        return Err(ManufacturingError::new(
            ManufacturingErrorCode::UnsupportedCapability,
            "standard material settings require an explicitly selected complete single-tool Marlin setup",
        ));
    };
    apply_thermal(job, &selection);
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
    let nozzle = printer.nozzle_diameter_mm;
    let width = quality
        .bead_width_mm(nozzle)
        .max(min_height)
        .clamp(min_width, max_width);
    let height = quality
        .layer_height_mm(nozzle)
        .clamp(min_height, max_height.min(width));
    let first = quality
        .first_layer_height_mm(nozzle)
        .clamp(min_height, max_height.min(width));
    let mut speeds = match &hardware.setup.machine {
        Some(machine) => machine.role_speeds(),
        None => spiling_contracts::hardware::generic_role_speeds().map_err(invalid)?,
    };
    for speed in [
        &mut speeds.first_layer_mm_s,
        &mut speeds.first_layer_infill_mm_s,
        &mut speeds.outer_wall_mm_s,
        &mut speeds.inner_wall_mm_s,
        &mut speeds.sparse_infill_mm_s,
        &mut speeds.solid_infill_mm_s,
        &mut speeds.top_skin_mm_s,
        &mut speeds.support_mm_s,
        &mut speeds.bridge_mm_s,
    ] {
        *speed = speed.min(printer.max_print_feed_mm_s);
    }
    let material = &selection.profile.material;
    let mut intent = ManufacturingIntent {
        hardware: Some(hardware),
        material_profile: Some(ResolvedMaterialProfile {
            resolver_version: MATERIAL_RESOLVER_VERSION,
            selection: selection.clone(),
        }),
        recipe: PlanarPrintRecipe {
            id: quality.recipe_id(),
            revision: "1".into(),
            material_id: material.id.clone(),
            material_revision: material.revision.clone(),
            filament_diameter_mm: material.filament_diameter_mm,
            layer_height_mm: height,
            layer_height_policy: LayerHeightPolicy::Preferred,
            bead_width_mm: width,
            sparse_infill: SparseInfillPolicy::Fine,
            perimeter_count: 2,
            infill_fraction: 0.15,
            top_skin_thickness_mm: r6(0.7_f64.max(5.0 * height)),
            bottom_skin_thickness_mm: r6(0.5_f64.max(3.0 * height)),
            print_speed_mm_s: selection.print_speed_mm_s(),
            travel_speed_mm_s: printer.max_travel_feed_mm_s,
            flow_multiplier: selection.flow_multiplier(),
            process: selection.process(),
            supports: SupportPolicy::Disabled,
            seam_policy: SeamPolicy::Nearest,
            rectilinear_axis: RectilinearAxisPolicy::AlternatingXY,
            top_skin_axis: RectilinearAxisPolicy::AlternatingXY,
            role_speeds: Some(speeds),
            first_layer_height_mm: Some(first),
        },
        printer,
    };
    if supports == SupportChoice::Automatic {
        intent.recipe.supports = intent.automatic_support_policy()?;
    }
    intent.validate()?;
    Ok(intent)
}

/// The one-step defaults: the material choice resolved against the captured
/// printer, then [`create_default_intent`]. `filament` returns the project's
/// captured filament revision for a reference.
pub fn create_requested_intent(
    hardware: std::sync::Arc<spiling_contracts::hardware::ProjectHardwareCapture>,
    request: &DefaultIntentRequest,
    filament: impl FnOnce(
        &spiling_contracts::library::FilamentRef,
    )
        -> Result<spiling_contracts::library::FilamentDefinition, ManufacturingError>,
) -> Result<ManufacturingIntent, ManufacturingError> {
    request.validate()?;
    let selection = match &request.material {
        DefaultMaterialChoice::Generic { polymer_family } => {
            generic_selection(&hardware.setup, polymer_family)?
        }
        DefaultMaterialChoice::ProjectFilament {
            reference,
            profile_id,
        } => filament_selection(
            &hardware.setup,
            &filament(reference)?,
            profile_id.as_deref(),
        )?,
        DefaultMaterialChoice::Profile { selection } => (**selection).clone(),
    };
    create_default_intent(hardware, selection, request.quality, request.supports)
}

/// The profile of a captured filament for a setup's nozzle (exactly one, or
/// the named one), with the tool setup the profile is keyed to and the
/// machine's extruder drive, as [`generic_selection`] does for generics.
pub fn filament_selection(
    setup: &spiling_contracts::hardware::HardwareSetupDefinition,
    filament: &spiling_contracts::library::FilamentDefinition,
    profile_id: Option<&str>,
) -> Result<MaterialProfileSelection, ManufacturingError> {
    let invalid =
        |e: String| ManufacturingError::new(ManufacturingErrorCode::InvalidSpecification, e);
    let nozzle = setup.printer.nozzle_diameter_mm;
    let candidates: Vec<&MaterialProcessProfile> = filament
        .profiles
        .iter()
        .filter(|p| profile_id.is_none_or(|id| p.id == id))
        .filter(|p| p.nozzle_diameter_mm == nozzle)
        .collect();
    let profile = match candidates.as_slice() {
        [one] => (*one).clone(),
        [] => {
            return Err(invalid(format!(
                "filament {:?} has no profile{} for a {nozzle} mm nozzle (profiles: {})",
                filament.name,
                profile_id.map_or(String::new(), |id| format!(" {id}")),
                if filament.profiles.is_empty() {
                    "none".into()
                } else {
                    filament
                        .profiles
                        .iter()
                        .map(|p| format!("{} @ {} mm", p.id, p.nozzle_diameter_mm))
                        .collect::<Vec<_>>()
                        .join(", ")
                }
            )));
        }
        many => {
            return Err(invalid(format!(
                "filament {:?} has several {nozzle} mm profiles; choose one of {}",
                filament.name,
                many.iter()
                    .map(|p| p.id.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
    };
    tool_selection(setup, profile)
}

fn tool_selection(
    setup: &spiling_contracts::hardware::HardwareSetupDefinition,
    profile: MaterialProcessProfile,
) -> Result<MaterialProfileSelection, ManufacturingError> {
    let selection = MaterialProfileSelection {
        setup: MaterialToolSetup {
            hotend_id: profile.hotend_id.clone(),
            nozzle_material: profile.nozzle_material.clone(),
            abrasion_resistant_nozzle: false,
            cooling_configuration_id: profile.cooling_configuration_id.clone(),
            extruder_drive: setup
                .machine
                .as_ref()
                .map_or(ExtruderDrive::DirectDrive, |m| m.extruder_drive),
        },
        profile,
        overrides: MaterialProfileOverrides {
            nozzle_temperature_c: None,
            bed_temperature_c: None,
            print_speed_mm_s: None,
            flow_multiplier: None,
            process: None,
            first_layer_nozzle_temperature_c: None,
            first_layer_bed_temperature_c: None,
        },
    };
    selection.validate()?;
    Ok(selection)
}

/// The generic profile of a polymer family for a setup's nozzle, with the
/// setup's installed tool: the generic brass hotend and part fan the generic
/// profiles are keyed to, and the machine's extruder drive (direct drive for a
/// declared software setup without machine facts).
pub fn generic_selection(
    setup: &spiling_contracts::hardware::HardwareSetupDefinition,
    polymer_family: &str,
) -> Result<MaterialProfileSelection, ManufacturingError> {
    let invalid =
        |e: String| ManufacturingError::new(ManufacturingErrorCode::InvalidSpecification, e);
    let family = generic_polymer_family(polymer_family).ok_or_else(|| {
        invalid(format!(
            "no generic profile covers material {polymer_family:?}"
        ))
    })?;
    let nozzle = setup.printer.nozzle_diameter_mm;
    let profile = generic_material_profiles()
        .map_err(invalid)?
        .into_iter()
        .find(|p| p.material.polymer_family == family && p.nozzle_diameter_mm == nozzle)
        .ok_or_else(|| {
            invalid(format!(
                "no generic {family} profile for a {nozzle} mm nozzle"
            ))
        })?;
    tool_selection(setup, profile)
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

    #[test]
    fn one_step_defaults_cover_every_shipped_single_tool_setup_and_record_the_choice() {
        let catalog = builtin_catalog().unwrap();
        let mut created = 0;
        for setup in &catalog.setups {
            let hardware = Arc::new(catalog.stock_capture(setup).unwrap());
            for family in spiling_contracts::manufacturing::GENERIC_FAMILIES {
                for quality in QualityPreset::ALL {
                    for supports in [SupportChoice::Automatic, SupportChoice::Off] {
                        let request = DefaultIntentRequest {
                            material: DefaultMaterialChoice::Generic {
                                polymer_family: family.to_lowercase(),
                            },
                            quality,
                            supports,
                        };
                        let result = create_requested_intent(hardware.clone(), &request, |_| {
                            unreachable!("generic choices need no project filament")
                        });
                        if !matches!(setup.printer.workflow, JobWorkflow::CompleteMarlin { .. }) {
                            assert_eq!(
                                result.unwrap_err().code,
                                ManufacturingErrorCode::UnsupportedCapability
                            );
                            continue;
                        }
                        let intent = result.unwrap();
                        intent.validate().unwrap();
                        created += 1;
                        assert_eq!(intent.hardware.as_deref(), Some(hardware.as_ref()));
                        assert_eq!(
                            QualityPreset::from_recipe_id(&intent.recipe.id),
                            Some(quality)
                        );
                        let profile = &intent.material_profile.as_ref().unwrap().selection.profile;
                        assert_eq!(profile.material.polymer_family, family);
                        assert_eq!(
                            matches!(intent.recipe.supports, SupportPolicy::Disabled),
                            supports == SupportChoice::Off
                        );
                        assert_eq!(
                            intent.recipe.layer_height_mm,
                            quality.layer_height_mm(setup.printer.nozzle_diameter_mm)
                        );
                    }
                }
            }
        }
        // MK4S, both Bambu, Ender-3 V3 SE, generic Klipper/Marlin, the Marlin reference.
        assert_eq!(created, 7 * 5 * 3 * 2);
    }
    #[test]
    fn project_filaments_choose_their_nozzle_profile_or_explain_why_not() {
        let catalog = builtin_catalog().unwrap();
        let setup = catalog.find_setup("prusa.mk4s.04").unwrap();
        let material = MaterialDefinition {
            id: "catalog.example".into(),
            revision: "1".into(),
            manufacturer: "Example".into(),
            product: "Example PETG".into(),
            polymer_family: "PETG".into(),
            filler: None,
            filament_diameter_mm: 1.75,
            requires_abrasion_resistant_nozzle: false,
        };
        let profiles = generic_profiles_for(&material).unwrap();
        use spiling_contracts::library::*;
        let mut filament = FilamentDefinition {
            declaration: FilamentDeclaration {
                brand: Some("Example".into()),
                material_label: Some("PETG".into()),
                diameter_mm: Some(1.75),
                hueforge_td_mm: None,
            },
            catalog_origin: None,
            name: "Example PETG".into(),
            material: Some(material),
            color_name: None,
            appearance: None,
            profiles: Vec::new(),
            source: DefinitionSource::Operator,
        };
        filament.profiles = profiles.clone();
        let selection = filament_selection(setup, &filament, None).unwrap();
        assert_eq!(selection.profile.nozzle_diameter_mm, 0.4);
        assert_eq!(selection.profile.material.id, "catalog.example");
        let mut doubled = filament.clone();
        let mut twin = profiles[1].clone();
        twin.id.push_str(".twin");
        doubled.profiles.push(twin.clone());
        let error = filament_selection(setup, &doubled, None).unwrap_err();
        assert!(error.message.contains("choose one of"), "{}", error.message);
        assert_eq!(
            filament_selection(setup, &doubled, Some(&twin.id))
                .unwrap()
                .profile
                .id,
            twin.id
        );
        filament.profiles.retain(|p| p.nozzle_diameter_mm != 0.4);
        let error = filament_selection(setup, &filament, None).unwrap_err();
        assert!(error.message.contains("0.4 mm nozzle"), "{}", error.message);
    }
}
