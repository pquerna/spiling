// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Transient authoring of one source-bound functional region in the full intent.
use super::*;
use spiling_contracts::manufacturing::*;

fn request_for_face(
    job: &FixedDualJob,
    face: &FaceInspection,
) -> Result<FunctionalMaterialConstruction, &'static str> {
    if job.assignments.len() != 1 || !job.surface_constructions.is_empty() {
        return Err("Use one uniformly assigned part with no optical construction");
    }
    if face.face.geometry.family != SurfaceFamily::Plane
        || !matches!(
            face.face.geometry.analytic,
            Some(AnalyticSurfaceDescription::Plane { .. })
        )
    {
        return Err("Select a native planar face; the engine must also admit its full-height trim");
    }
    let assignment = &job.assignments[0];
    let OccurrenceMaterial::Uniform { material_id: base } = &assignment.material else {
        return Err(
            "A functional region requires a uniform base assignment, without an optical cycle",
        );
    };
    if assignment.occurrence_id != face.reference.occurrence_id {
        return Err("The selected face must belong to the assigned part");
    }
    if job.tools[0].material.id == job.tools[1].material.id
        || !job.tools.iter().any(|t| &t.material.id == base)
    {
        return Err("Declare two distinct loaded materials and assign one as the base");
    }
    let region = job.tools.iter().find(|t| &t.material.id != base).unwrap();
    Ok(FunctionalMaterialConstruction {
        interface_policy: FunctionalInterfacePolicy::DeclaredSameFamily,
        occurrence_id: face.reference.occurrence_id,
        definition_id: face.reference.definition_id.clone(),
        source_hash: face.provenance.source_hash.clone(),
        face_id: face.reference.face_id.clone(),
        material_id: region.material.id.clone(),
        base_material_id: base.clone(),
        minimum_pure_depth_mm: 5.0,
        join: PlanarTKeyJoin {
            reach_into_region_mm: 1.6,
            reach_into_base_mm: 1.6,
            pitch_mm: 6.4,
            stem_width_mm: 1.6,
            head_width_mm: 4.8,
            head_depth_mm: 1.2,
            edge_margin_mm: 1.2,
            end_collar_height_mm: 1.2,
            collar_head_width_mm: 3.2,
        },
    })
}

fn bind_region(job: &mut FixedDualJob, region_tool: usize) {
    let Some(request) = &mut job.functional_construction else {
        return;
    };
    if region_tool > 1 || job.assignments.len() != 1 {
        return;
    }
    request.material_id = job.tools[region_tool].material.id.clone();
    request.base_material_id = job.tools[1 - region_tool].material.id.clone();
    job.assignments[0].material = OccurrenceMaterial::Uniform {
        material_id: request.base_material_id.clone(),
    };
}

fn remove_region(job: &mut FixedDualJob) {
    if let Some(request) = job.functional_construction.take()
        && let [assignment] = job.assignments.as_mut_slice()
        && assignment.occurrence_id == request.occurrence_id
    {
        assignment.material = OccurrenceMaterial::Uniform {
            material_id: request.base_material_id,
        };
    }
}

impl Workbench {
    fn functional_face(&self) -> Result<FaceInspection, &'static str> {
        if !self.display_current() {
            return Err("Wait for the current native source display");
        }
        let scene = self.state.display.as_ref().ok_or("Load one native part")?;
        if scene.occurrences.len() != 1 {
            return Err(
                "Functional regions currently require exactly one native part on this plate",
            );
        }
        let selected = self
            .selected
            .as_ref()
            .ok_or("Select a full-height planar source face")?;
        let inspection = self
            .state
            .inspection
            .as_ref()
            .filter(|i| &i.reference == selected)
            .ok_or("Wait for native inspection of the selected face")?;
        let occurrence = &scene.occurrences[0];
        let definition = scene
            .definition(&occurrence.definition_id)
            .ok_or("Source definition is unavailable")?;
        selected
            .validate_current(
                &scene.summary,
                occurrence,
                &definition.record,
                &definition.faces,
            )
            .map_err(|_| "Select the face again in the current source revision")?;
        if inspection.provenance.source_hash != definition.record.provenance.source_hash {
            return Err("The inspected face does not match the current source identity");
        }
        Ok((**inspection).clone())
    }

    pub(super) fn functional_material_ui(&mut self, ui: &mut egui::Ui, writable: bool) {
        ui::group(ui, "Material region");
        ui.weak(
            "One planar face on solid rectangular stock. Physical retention remains unqualified.",
        );
        ui::help(
            ui,
            "functional-region-scope",
            "Scope and qualification",
            "Requires one upright rectangular native part, a full-height axis-aligned face and no optical construction. The engine independently admits the geometry and complete keys. Software replay does not establish adhesion, physical strength or printability.",
        );
        let face = self.functional_face();
        let Some(project) = self.state.project.as_ref() else {
            return;
        };
        let profile_pending = self.material.has_changes(
            &project.project_id,
            self.state.manufacturing.intent.as_deref(),
        );
        let Some(draft) = self
            .manufacturing
            .draft
            .as_mut()
            .filter(|d| d.project == project.project_id)
        else {
            ui.label("Load an explicit manufacturing intent with a fixed-dual setup and two declared loaded materials.");
            return;
        };
        let JobWorkflow::FixedDualMarlin { job } = &mut draft.value.printer.workflow else {
            ui.label("Functional regions require an explicit fixed-dual setup and two declared loaded materials.");
            return;
        };
        let conflict = draft.revision != project.revision;
        let disabled = if !writable {
            Some("Read-only project or an operation is pending")
        } else if conflict {
            Some("Project changed. Discard the settings draft and reload current values")
        } else if profile_pending {
            Some("Apply or discard the material profile draft first")
        } else {
            None
        };
        if let Some(reason) = disabled {
            ui.label(reason);
        }
        let proposed = face
            .as_ref()
            .map_err(|e| *e)
            .and_then(|face| request_for_face(job, face));
        if job.functional_construction.is_none() {
            ui.label("No functional material region requested · the part retains its assigned base material");
            let reason = disabled.or(proposed.as_ref().err().copied());
            if ui
                .add_enabled(
                    reason.is_none(),
                    egui::Button::new("Stage selected face assignment"),
                )
                .clicked()
            {
                job.functional_construction = Some(
                    proposed
                        .clone()
                        .expect("current native face and explicit materials"),
                );
            }
            if let Some(reason) = proposed.as_ref().err() {
                ui.weak(*reason);
            }
        }
        let mut remove = false;
        if job.functional_construction.is_some() {
            ui.add_enabled_ui(disabled.is_none(), |ui| {
                let request = job.functional_construction.as_mut().unwrap();
                ui.label(format!("Authored request · part {} · face {}", request.occurrence_id.get(), request.face_id.as_str()));
                ui::number(ui, "Minimum pure region depth (mm)", &mut request.minimum_pure_depth_mm, 0.1);
                ui.weak("Pure material excludes the join band; never reduced automatically.");
                let mut region_tool = job.tools.iter().position(|t| t.material.id == request.material_id);
                let previous_tool = region_tool;
                egui::ComboBox::from_id_salt("functional-region-material")
                    .selected_text(format!("Region · {}", request.material_id))
                    .show_ui(ui, |ui| {
                        for (index, tool) in job.tools.iter().enumerate() {
                            ui.selectable_value(&mut region_tool, Some(index), format!("{} · {} · T{index}", tool.material.id, tool.material.polymer_family));
                        }
                    });
                if region_tool != previous_tool && let Some(index) = region_tool { bind_region(job, index); }
                let request = job.functional_construction.as_mut().unwrap();
                ui.label(format!("Base · {}", request.base_material_id));
                egui::ComboBox::from_id_salt("functional-interface-policy")
                    .selected_text(match request.interface_policy {
                        FunctionalInterfacePolicy::DeclaredSameFamily => "Declared same family",
                        FunctionalInterfacePolicy::MechanicalCapture => "Mechanical capture",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut request.interface_policy, FunctionalInterfacePolicy::DeclaredSameFamily, "Declared same family");
                        ui.selectable_value(&mut request.interface_policy, FunctionalInterfacePolicy::MechanicalCapture, "Mechanical capture");
                    });
                ui.weak(match request.interface_policy {
                    FunctionalInterfacePolicy::DeclaredSameFamily => "Requires equal declared families; not an adhesion measurement.",
                    FunctionalInterfacePolicy::MechanicalCapture => "Permits different families for this construction; no adhesion or physical retention credit.",
                });
                egui::CollapsingHeader::new("Joins · 1 requested interface")
                    .id_salt("functional-t-key-join").default_open(true).show(ui, |ui| {
                        ui.label("Strategy · captive T-keys");
                        ui::number(ui, "Reach into region (mm)", &mut request.join.reach_into_region_mm, 0.1);
                        ui::number(ui, "Reach into base (mm)", &mut request.join.reach_into_base_mm, 0.1);
                        ui.collapsing("Advanced T-key dimensions", |ui| {
                            for (label, value) in [
                                ("Key pitch (mm)", &mut request.join.pitch_mm),
                                ("Stem width (mm)", &mut request.join.stem_width_mm),
                                ("Middle head width (mm)", &mut request.join.head_width_mm),
                                ("Head depth (mm)", &mut request.join.head_depth_mm),
                                ("Edge margin (mm)", &mut request.join.edge_margin_mm),
                                ("End collar height (mm)", &mut request.join.end_collar_height_mm),
                                ("Collar head width (mm)", &mut request.join.collar_head_width_mm),
                            ] { ui::number(ui, label, value, 0.1); }
                        });
                        ui.weak("Requested dimensions. Fresh replay supplies the realized join in Review.");
                    });
                remove = ui.button("Remove region · restore base").clicked();
            });
            if remove {
                remove_region(job);
            }
            if let Some(request) = &job.functional_construction
                && let Err(error) = request.validate(job)
            {
                ui.colored_label(ui.visuals().warn_fg_color, error.to_string());
            }
            if job.tools[0].material.polymer_family != job.tools[1].material.polymer_family
                && job.functional_construction.as_ref().is_some_and(|request| {
                    request.interface_policy == FunctionalInterfacePolicy::DeclaredSameFamily
                })
            {
                ui.colored_label(ui.visuals().warn_fg_color, "Declared same polymer family refuses these different family declarations. Mechanical capture requires an explicit policy choice and complete native/replay admission.");
            }
        }
        let can_focus = face.is_ok() && self.source_display_visible();
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(can_focus, egui::Button::new("Focus selected face"))
                .clicked()
            {
                self.focus_selection();
            }
            if ui
                .add_enabled(can_focus, egui::Button::new("Reveal selected face"))
                .clicked()
            {
                self.reveal_selection();
            }
        });
        ui.separator();
    }
}

#[cfg(test)]
mod tests {
    use super::super::manufacturing::ManufacturingUi;
    use super::*;
    use spiling_contracts::project::{PrintLayoutId, ProjectId, ProjectRevision};
    use std::sync::Arc;

    fn intent() -> ManufacturingIntent {
        let mut value: ManufacturingIntent = serde_json::from_slice(include_bytes!(
            "../../../../fixtures/manufacturing/fixed-dual.intent.json"
        ))
        .unwrap();
        job(&mut value).assignments.truncate(1);
        value
    }
    fn job(intent: &mut ManufacturingIntent) -> &mut FixedDualJob {
        let JobWorkflow::FixedDualMarlin { job } = &mut intent.printer.workflow else {
            panic!("fixture route");
        };
        job
    }
    fn face(job: &FixedDualJob) -> FaceInspection {
        let face_id = FaceId::from_step_entity(1).unwrap();
        FaceInspection {
            reference: FaceRef {
                session_id: SessionId::new(),
                scene_revision: SceneRevision(4),
                occurrence_id: job.assignments[0].occurrence_id,
                definition_id: DefinitionId::from_source_sha256(&[2; 32]),
                face_id: face_id.clone(),
            },
            face: FaceInfo {
                face_id,
                surface_face_entity: "1".into(),
                source_entity_kind: "ADVANCED_FACE".into(),
                source_surface_entity_kind: "PLANE".into(),
                orientation: true,
                geometry: FaceGeometryInfo::new(
                    SurfaceFamily::Plane,
                    Some(AnalyticSurfaceDescription::Plane {
                        origin_mm: [0.0; 3],
                        normal: [1.0, 0.0, 0.0],
                    }),
                    FaceDomainSummary::new(1, 0, false, false, false).unwrap(),
                )
                .unwrap(),
            },
            pose: RigidPoseMm::IDENTITY,
            provenance: SourceProvenance {
                source_hash: SourceHash::from_bytes(b"native source"),
                source_name: "part.step".into(),
                source_unit: SourceUnit::Millimetre,
                uncertainty_mm: None,
            },
        }
    }
    #[test]
    fn region_and_joins_share_one_revision_bound_full_intent_transaction() {
        let current = Arc::new(intent());
        let project = ProjectId::new();
        let session = SessionId::new();
        let mut ui = ManufacturingUi::default();
        ui.sync_intent(&project, ProjectRevision(8), Some(current.clone()), None);
        let draft = ui.draft.as_mut().unwrap();
        let job = job(&mut draft.value);
        let inspected = face(job);
        let request = request_for_face(job, &inspected).unwrap();
        assert_eq!(request.source_hash, inspected.provenance.source_hash);
        assert_eq!(request.definition_id, inspected.reference.definition_id);
        assert_eq!(request.face_id, inspected.reference.face_id);
        assert_eq!(request.minimum_pure_depth_mm, 5.0);
        assert_eq!(
            request.interface_policy,
            FunctionalInterfacePolicy::DeclaredSameFamily
        );
        job.functional_construction = Some(request);
        bind_region(job, 0);
        let request = job.functional_construction.as_mut().unwrap();
        request.interface_policy = FunctionalInterfacePolicy::MechanicalCapture;
        request.minimum_pure_depth_mm = 6.0;
        request.join.reach_into_region_mm = 3.0;
        request.join.reach_into_base_mm = 4.0;
        request.join.end_collar_height_mm = 1.5;
        // Settings edits join the same transaction; other complete declarations survive.
        draft.value.recipe.top_skin_thickness_mm += 0.2;
        let expected = draft.value.clone();
        let command = draft
            .apply_command(PrintLayoutId::new(), session.clone(), ProjectRevision(8))
            .unwrap();
        let ManufacturingCommand::SetIntent {
            session_id,
            base_revision,
            intent: submitted,
            ..
        } = command
        else {
            panic!("ordinary full-intent command");
        };
        assert_eq!(session_id, session);
        assert_eq!(base_revision, ProjectRevision(8));
        assert_eq!(*submitted, expected);
        assert_eq!(submitted.recipe.process, current.recipe.process);
        assert!(ui.has_intent_draft(&project, Some(&current)));
        // Rejection and navigation/render refreshes preserve the complete request.
        ui.sync_intent(&project, ProjectRevision(8), Some(current.clone()), None);
        assert_eq!(ui.draft.as_ref().unwrap().value, expected);
        // A different transaction must not rebase or erase edited dimensions.
        ui.sync_intent(&project, ProjectRevision(9), Some(current), None);
        let draft = ui.draft.as_mut().unwrap();
        assert_eq!(draft.revision, ProjectRevision(8));
        assert!(
            draft
                .apply_command(PrintLayoutId::new(), session, ProjectRevision(9))
                .is_err()
        );
        assert_eq!(draft.value, expected);
        // Only the exact accepted value clears the unapplied/Slice guard.
        assert!(!ui.has_intent_draft(&project, Some(&submitted)));
        ui.sync_intent(&project, ProjectRevision(9), Some(submitted), None);
        assert_eq!(ui.draft.as_ref().unwrap().revision, ProjectRevision(9));
    }
    #[test]
    fn removal_restores_explicit_base_without_rewriting_loaded_declarations() {
        let mut value = intent();
        let original = value.clone();
        let job = job(&mut value);
        job.functional_construction = Some(request_for_face(job, &face(job)).unwrap());
        bind_region(job, 0);
        let base = job
            .functional_construction
            .as_ref()
            .unwrap()
            .base_material_id
            .clone();
        remove_region(job);
        assert!(job.functional_construction.is_none());
        assert_eq!(
            job.assignments[0].material,
            OccurrenceMaterial::Uniform { material_id: base }
        );
        let JobWorkflow::FixedDualMarlin { job: before } = &original.printer.workflow else {
            unreachable!()
        };
        assert_eq!(job.tools, before.tools);
        assert_eq!(value.recipe, original.recipe);
    }
    #[test]
    fn incompatible_polymers_remain_a_rejected_retained_draft() {
        let mut value = intent();
        let job = job(&mut value);
        job.functional_construction = Some(request_for_face(job, &face(job)).unwrap());
        job.tools[0].material.polymer_family = "TPU".into();
        job.tools[1].material.polymer_family = "PLA".into();
        let project = ProjectId::new();
        let mut ui = ManufacturingUi::default();
        ui.sync_intent(&project, ProjectRevision(2), Some(Arc::new(intent())), None);
        ui.draft.as_mut().unwrap().value = value.clone();
        let error = ui
            .draft
            .as_mut()
            .unwrap()
            .apply_command(PrintLayoutId::new(), SessionId::new(), ProjectRevision(2))
            .unwrap_err();
        assert!(error.contains("common polymer"), "{error}");
        assert_eq!(ui.draft.as_ref().unwrap().value, value);
    }
    #[test]
    fn mixed_families_require_an_explicit_policy_edit_before_apply() {
        let mut value = intent();
        let job = job(&mut value);
        job.tools[0].material.polymer_family = "TPU".into();
        job.tools[1].material.polymer_family = "PLA".into();
        let request = request_for_face(job, &face(job)).unwrap();
        assert_eq!(
            request.interface_policy,
            FunctionalInterfacePolicy::DeclaredSameFamily
        );
        job.functional_construction = Some(request);
        assert!(value.validate().is_err());
        let JobWorkflow::FixedDualMarlin { job } = &mut value.printer.workflow else {
            unreachable!()
        };
        job.functional_construction
            .as_mut()
            .unwrap()
            .interface_policy = FunctionalInterfacePolicy::MechanicalCapture;
        let project = ProjectId::new();
        let mut ui = ManufacturingUi::default();
        ui.sync_intent(&project, ProjectRevision(2), Some(Arc::new(intent())), None);
        let draft = ui.draft.as_mut().unwrap();
        draft.value = value.clone();
        let command = draft
            .apply_command(PrintLayoutId::new(), SessionId::new(), ProjectRevision(2))
            .unwrap();
        let ManufacturingCommand::SetIntent {
            intent: submitted,
            base_revision,
            ..
        } = command
        else {
            panic!("ordinary full-intent command")
        };
        assert_eq!(base_revision, ProjectRevision(2));
        assert_eq!(*submitted, value);
        assert_eq!(draft.value, value);
    }
    #[test]
    fn source_target_and_route_require_explicit_eligible_bindings() {
        let mut value = intent();
        let job = job(&mut value);
        let mut inspected = face(job);
        inspected.reference.occurrence_id = OccurrenceId::new(999).unwrap();
        assert!(request_for_face(job, &inspected).is_err());
        inspected = face(job);
        inspected.face.geometry.analytic = None;
        assert!(request_for_face(job, &inspected).is_err());
        let inspected = face(job);
        job.assignments[0].material = OccurrenceMaterial::LayerCycle {
            recipe: spiling_contracts::optical::LayerCycle {
                slots: vec![],
                phase_slot: 0,
                origin_z_mm: 0.0,
            },
            material_ids: [
                job.tools[0].material.id.clone(),
                job.tools[1].material.id.clone(),
            ],
        };
        assert!(request_for_face(job, &inspected).is_err());
    }

    #[test]
    fn a_project_without_an_intent_cannot_inherit_another_projects_region_draft() {
        let mut value = intent();
        let job = job(&mut value);
        job.functional_construction = Some(request_for_face(job, &face(job)).unwrap());
        let mut ui = ManufacturingUi::default();
        ui.sync_intent(
            &ProjectId::new(),
            ProjectRevision(2),
            Some(Arc::new(value)),
            None,
        );
        assert!(ui.draft.is_some());
        ui.sync_intent(&ProjectId::new(), ProjectRevision(2), None, None);
        assert!(ui.draft.is_none());
    }
}
