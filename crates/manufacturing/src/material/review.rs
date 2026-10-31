// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::resolve_material_profile;
use spiling_contracts::{manufacturing::*, project::ProjectRevision};

/// Review declarations against one captured input. No mutation, geometry queries or clamping.
pub fn review_materials(
    current: &ManufacturingIntent,
    revision: ProjectRevision,
    request: MaterialReviewRequest,
) -> Result<MaterialReviewReport, ManufacturingError> {
    current.validate()?;
    request.validate()?;
    let mut selections = if let Some(catalog) = request.catalog {
        catalog
            .profiles
            .into_iter()
            .map(|profile| MaterialProfileSelection {
                profile,
                setup: catalog.setup.clone(),
                overrides: MaterialProfileOverrides {
                    nozzle_temperature_c: None,
                    bed_temperature_c: None,
                    print_speed_mm_s: None,
                    flow_multiplier: None,
                    process: None,
                },
            })
            .collect::<Vec<_>>()
    } else {
        vec![
            current
                .material_profile
                .as_ref()
                .ok_or_else(|| {
                    ManufacturingError::new(
                        ManufacturingErrorCode::NoIntent,
                        "select a material profile before inspecting captured material settings",
                    )
                })?
                .selection
                .clone(),
        ]
    };
    let catalog_profiles = selections.len() as u32;
    selections.sort_by(|a, b| {
        (&a.profile.id, &a.profile.revision).cmp(&(&b.profile.id, &b.profile.revision))
    });
    let terms = request
        .query
        .split_whitespace()
        .map(str::to_lowercase)
        .collect::<Vec<_>>();
    selections.retain(|s| {
        let p = &s.profile;
        let filler = match p.material.filler.as_ref().map(|f| f.kind) {
            Some(MaterialFillerKind::CarbonFiber) => "carbon_fiber cf",
            Some(MaterialFillerKind::GlassFiber) => "glass_fiber gf",
            None => "",
        };
        let text = format!(
            "{} {} {} {} {} {} {}",
            p.id,
            p.revision,
            p.material.id,
            p.material.manufacturer,
            p.material.product,
            p.material.polymer_family,
            filler
        )
        .to_lowercase();
        terms.iter().all(|term| text.contains(term))
    });
    let matching_profiles = selections.len() as u32;
    let candidates = selections
        .into_iter()
        .skip(request.offset as usize)
        .take(MATERIAL_REVIEW_PAGE_SIZE)
        .map(|selection| review_candidate(current, selection))
        .collect();
    let report = MaterialReviewReport {
        input_revision: revision,
        query: request.query,
        offset: request.offset,
        catalog_profiles,
        matching_profiles,
        candidates,
    };
    report.validate()?;
    Ok(report)
}

fn review_candidate(
    current: &ManufacturingIntent,
    selection: MaterialProfileSelection,
) -> MaterialCandidateReview {
    let mut issues = Vec::new();
    let mut issue = |code, message: &str| {
        issues.push(MaterialCompatibilityIssue {
            code,
            message: message.into(),
        })
    };
    let p = &selection.profile;
    if p.printer_id != current.printer.id || p.printer_revision != current.printer.revision {
        issue(
            MaterialCompatibilityCode::PrinterMismatch,
            "profile requires a different printer ID or revision",
        );
    }
    if p.nozzle_diameter_mm != current.printer.nozzle_diameter_mm {
        issue(
            MaterialCompatibilityCode::NozzleMismatch,
            "profile nozzle diameter differs from the selected printer nozzle",
        );
    }
    if p.hotend_id != selection.setup.hotend_id
        || p.nozzle_material != selection.setup.nozzle_material
        || p.cooling_configuration_id != selection.setup.cooling_configuration_id
    {
        issue(
            MaterialCompatibilityCode::SetupMismatch,
            "profile hotend, nozzle material or cooling differs from the declared installed setup",
        );
    }
    if p.material.requires_abrasion_resistant_nozzle && !selection.setup.abrasion_resistant_nozzle {
        issue(
            MaterialCompatibilityCode::AbrasionRequirement,
            "material requires a declared abrasion-resistant nozzle",
        );
    }
    let resolved = if issues.is_empty() {
        match resolve_material_profile(current, selection.clone()) {
            Ok(value) => Some(value),
            Err(error) => {
                issues.push(MaterialCompatibilityIssue {
                    code: MaterialCompatibilityCode::ProcessConstraints,
                    message: error.message,
                });
                None
            }
        }
    } else {
        None
    };
    let settings = resolved
        .as_ref()
        .map(|proposed| {
            MaterialSettingField::ALL
                .into_iter()
                .map(|field| MaterialSettingReview {
                    field,
                    current: value(current, field),
                    proposed: value(proposed, field),
                    current_origin: origin(current, field),
                    proposed_origin: origin(proposed, field),
                })
                .collect()
        })
        .unwrap_or_default();
    MaterialCandidateReview {
        selection,
        input_changed: resolved.as_ref().map(|v| v != current),
        issues,
        settings,
    }
}

fn value(intent: &ManufacturingIntent, field: MaterialSettingField) -> Option<f64> {
    use MaterialSettingField::*;
    let r = &intent.recipe;
    let p = &r.process;
    let limits = intent.effective_deposition_limits();
    Some(match field {
        NozzleTemperatureC | BedTemperatureC => {
            let JobWorkflow::CompleteMarlin { job } = &intent.printer.workflow else {
                return None;
            };
            if field == NozzleTemperatureC {
                job.nozzle_temperature_c
            } else {
                job.bed_temperature_c
            }
        }
        PrintSpeedMmS => r.print_speed_mm_s,
        FilamentDiameterMm => r.filament_diameter_mm,
        FlowMultiplier => r.flow_multiplier,
        MaxVolumetricFlowMm3S => intent.effective_max_volumetric_flow_mm3_s(),
        FirstLayerSpeedFactor => p.first_layer_speed_factor,
        FirstLayerFlowFactor => p.first_layer_flow_factor,
        FanFirstLayers => p.fan_first_layers as f64,
        FanAfterFirstLayers => p.fan_after_first_layers as f64,
        BridgeSpeedFactor => p.bridge_speed_factor,
        BridgeFlowFactor => p.bridge_flow_factor,
        BridgeFanPwm => p.bridge_fan_pwm as f64,
        MaxBridgeSpanMm => p.max_bridge_span_mm,
        MinBeadWidthMm => limits.min_bead_width_mm,
        MaxBeadWidthMm => limits.max_bead_width_mm,
        MinBeadHeightMm => limits.min_bead_height_mm,
        MaxBeadHeightMm => limits.max_bead_height_mm,
        RetractionDistanceMm
        | RetractionSpeedMmS
        | RetractionMinimumTravelMm
        | RetractionZHopMm => {
            let RetractionPolicy::Enabled {
                distance_mm,
                speed_mm_s,
                minimum_travel_mm,
                z_hop_mm,
            } = p.retraction
            else {
                return None;
            };
            match field {
                RetractionDistanceMm => distance_mm,
                RetractionSpeedMmS => speed_mm_s,
                RetractionMinimumTravelMm => minimum_travel_mm,
                _ => z_hop_mm,
            }
        }
    })
}

fn origin(intent: &ManufacturingIntent, field: MaterialSettingField) -> MaterialValueOrigin {
    use MaterialSettingField::*;
    use MaterialValueOrigin::*;
    let is_limit = matches!(
        field,
        MaxVolumetricFlowMm3S | MinBeadWidthMm | MaxBeadWidthMm | MinBeadHeightMm | MaxBeadHeightMm
    );
    let Some(profile) = &intent.material_profile else {
        return if is_limit {
            MachineLimit
        } else {
            CurrentIntent
        };
    };
    let s = &profile.selection;
    if is_limit {
        let machine = &intent.printer.deposition_limits;
        let material = &s.profile.deposition_limits;
        let (a, b, minimum) = match field {
            MaxVolumetricFlowMm3S => (
                intent.printer.max_volumetric_flow_mm3_s,
                s.profile.max_volumetric_flow_mm3_s,
                true,
            ),
            MinBeadWidthMm => (machine.min_bead_width_mm, material.min_bead_width_mm, false),
            MaxBeadWidthMm => (machine.max_bead_width_mm, material.max_bead_width_mm, true),
            MinBeadHeightMm => (
                machine.min_bead_height_mm,
                material.min_bead_height_mm,
                false,
            ),
            _ => (
                machine.max_bead_height_mm,
                material.max_bead_height_mm,
                true,
            ),
        };
        return if a == b {
            SharedLimit
        } else if (a < b) == minimum {
            MachineLimit
        } else {
            MaterialLimit
        };
    }
    let overridden = match field {
        NozzleTemperatureC => s.overrides.nozzle_temperature_c.is_some(),
        BedTemperatureC => s.overrides.bed_temperature_c.is_some(),
        PrintSpeedMmS => s.overrides.print_speed_mm_s.is_some(),
        FlowMultiplier => s.overrides.flow_multiplier.is_some(),
        FilamentDiameterMm => false,
        _ => s.overrides.process.is_some(),
    };
    if overridden {
        ExplicitOverride
    } else {
        SelectedProfile
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn intent() -> ManufacturingIntent {
        decode_intent(include_bytes!(
            "../../../../fixtures/manufacturing/complete-sparse.intent.json"
        ))
        .unwrap()
    }
    fn selection() -> MaterialProfileSelection {
        serde_json::from_slice(include_bytes!(
            "../../../../fixtures/manufacturing/material-profile-a.json"
        ))
        .unwrap()
    }
    fn request(profiles: Vec<MaterialProcessProfile>) -> MaterialReviewRequest {
        MaterialReviewRequest {
            catalog: Some(MaterialCatalog {
                setup: selection().setup,
                profiles,
            }),
            query: String::new(),
            offset: 0,
        }
    }
    fn row(c: &MaterialCandidateReview, field: MaterialSettingField) -> &MaterialSettingReview {
        c.settings.iter().find(|s| s.field == field).unwrap()
    }
    #[test]
    fn catalog_reports_effective_intersection_without_mutation_or_clamping() {
        let original = intent();
        let mut profile = selection().profile;
        profile.deposition_limits.max_bead_width_mm =
            original.printer.deposition_limits.max_bead_width_mm * 2.0;
        let report =
            review_materials(&original, ProjectRevision(4), request(vec![profile])).unwrap();
        let c = &report.candidates[0];
        assert!(c.issues.is_empty());
        let flow = row(c, MaterialSettingField::MaxVolumetricFlowMm3S);
        assert_eq!(flow.proposed, Some(6.0));
        assert_eq!(flow.proposed_origin, MaterialValueOrigin::MaterialLimit);
        let width = row(c, MaterialSettingField::MaxBeadWidthMm);
        assert_eq!(
            width.proposed,
            Some(original.printer.deposition_limits.max_bead_width_mm)
        );
        assert_eq!(width.proposed_origin, MaterialValueOrigin::MachineLimit);
        assert_eq!(original, intent());
        assert_eq!(report.input_revision, ProjectRevision(4));
    }
    #[test]
    fn declared_incompatibilities_are_visible_without_proposed_process() {
        let mut wrong = selection().profile;
        wrong.printer_revision = "other".into();
        wrong.nozzle_diameter_mm *= 2.0;
        wrong.material.requires_abrasion_resistant_nozzle = true;
        let mut req = request(vec![wrong]);
        req.catalog
            .as_mut()
            .unwrap()
            .setup
            .abrasion_resistant_nozzle = false;
        let c = review_materials(&intent(), ProjectRevision(1), req)
            .unwrap()
            .candidates
            .remove(0);
        assert!(
            c.issues
                .iter()
                .any(|i| i.code == MaterialCompatibilityCode::PrinterMismatch)
        );
        assert!(
            c.issues
                .iter()
                .any(|i| i.code == MaterialCompatibilityCode::NozzleMismatch)
        );
        assert!(
            c.issues
                .iter()
                .any(|i| i.code == MaterialCompatibilityCode::AbrasionRequirement)
        );
        assert_eq!(c.input_changed, None);
        assert!(c.settings.is_empty());
        assert!(resolve_material_profile(&intent(), c.selection).is_err());
    }
    #[test]
    fn impossible_flow_is_reported_instead_of_silently_reducing_speed() {
        let mut profile = selection().profile;
        profile.max_volumetric_flow_mm3_s = 0.001;
        let c = review_materials(&intent(), ProjectRevision(1), request(vec![profile]))
            .unwrap()
            .candidates
            .remove(0);
        assert_eq!(
            c.issues[0].code,
            MaterialCompatibilityCode::ProcessConstraints
        );
        assert!(c.settings.is_empty());
    }
    #[test]
    fn current_inspection_retains_explicit_overrides_and_is_a_noop() {
        let mut selected = selection();
        selected.overrides.nozzle_temperature_c = Some(210.0);
        selected.overrides.process = Some(selected.profile.process.clone());
        let current = resolve_material_profile(&intent(), selected).unwrap();
        let report = review_materials(
            &current,
            ProjectRevision(7),
            MaterialReviewRequest {
                catalog: None,
                query: String::new(),
                offset: 0,
            },
        )
        .unwrap();
        let c = &report.candidates[0];
        assert_eq!(c.input_changed, Some(false));
        assert!(c.settings.iter().all(|s| s.current == s.proposed));
        assert_eq!(
            row(c, MaterialSettingField::NozzleTemperatureC).proposed_origin,
            MaterialValueOrigin::ExplicitOverride
        );
        assert_eq!(
            row(c, MaterialSettingField::BridgeSpeedFactor).proposed_origin,
            MaterialValueOrigin::ExplicitOverride
        );
    }
    #[test]
    fn review_reports_reject_corrupted_effective_fields_and_incomplete_retraction() {
        let report = review_materials(
            &intent(),
            ProjectRevision(1),
            request(vec![selection().profile]),
        )
        .unwrap();
        let mutations = [
            (MaterialSettingField::PrintSpeedMmS, Some(0.0)),
            (MaterialSettingField::FanAfterFirstLayers, Some(256.0)),
            (MaterialSettingField::BridgeSpeedFactor, Some(2.0)),
            (MaterialSettingField::NozzleTemperatureC, Some(210.5)),
            (MaterialSettingField::MaxBridgeSpanMm, Some(f64::NAN)),
            (MaterialSettingField::FlowMultiplier, None),
        ];
        for (field, value) in mutations {
            let mut bad = report.clone();
            bad.candidates[0]
                .settings
                .iter_mut()
                .find(|s| s.field == field)
                .unwrap()
                .proposed = value;
            assert!(bad.validate().is_err(), "accepted corrupted {field:?}");
        }
        let mut bad = report.clone();
        bad.candidates[0].settings.swap(0, 1);
        assert!(bad.validate().is_err());
        let mut bad = report;
        let row = bad.candidates[0]
            .settings
            .iter_mut()
            .find(|s| s.field == MaterialSettingField::RetractionDistanceMm)
            .unwrap();
        row.proposed = if row.proposed.is_some() {
            None
        } else {
            Some(1.0)
        };
        assert!(bad.validate().is_err());
    }

    #[test]
    fn family_and_display_metadata_do_not_select_process_settings() {
        let current = resolve_material_profile(&intent(), selection()).unwrap();
        let mut profile = selection().profile;
        profile.material.polymer_family = "PCTG".into();
        profile.material.product = "Different label".into();
        let c = review_materials(&current, ProjectRevision(1), request(vec![profile]))
            .unwrap()
            .candidates
            .remove(0);
        assert_eq!(c.input_changed, Some(true));
        assert!(c.settings.iter().all(|s| s.current == s.proposed));
    }
    #[test]
    fn search_and_pages_are_deterministic_and_never_hide_the_match_count() {
        let profiles = (0..7)
            .rev()
            .map(|i| {
                let mut p = selection().profile;
                p.id = format!("profile-{i}");
                p.material.polymer_family = "PCTG".into();
                p
            })
            .collect();
        let mut req = request(profiles);
        req.query = "pctg SYNTHETIC".into();
        let a = review_materials(&intent(), ProjectRevision(1), req.clone()).unwrap();
        req.offset = 4;
        let b = review_materials(&intent(), ProjectRevision(1), req).unwrap();
        assert_eq!(a.matching_profiles, 7);
        assert_eq!(a.candidates.len(), 4);
        assert_eq!(a.candidates[0].selection.profile.id, "profile-0");
        assert_eq!(b.candidates.len(), 3);
        assert_eq!(b.candidates[0].selection.profile.id, "profile-4");
        let wire = spiling_contracts::rpc::ManufacturingReply::try_from(
            ManufacturingResponse::MaterialReview { report: a.clone() },
        )
        .unwrap();
        assert_eq!(
            ManufacturingResponse::try_from(wire).unwrap(),
            ManufacturingResponse::MaterialReview { report: a }
        );
    }
}
