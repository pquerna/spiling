// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Retained study application prepares one immutable, snapshot-bound intent edit.
use super::*;

#[derive(Debug, Clone)]
pub struct OpticalConstructionCandidate {
    pub(super) captured: Arc<Snapshot>,
    pub(super) captured_revision: ProjectRevision,
    pub(super) intent: Arc<ManufacturingIntent>,
    construction_index: u16,
}
impl OpticalConstructionCandidate {
    pub fn construction_index(&self) -> u16 {
        self.construction_index
    }
}

pub fn prepare_optical_construction(
    captured: Arc<Snapshot>,
    captured_revision: ProjectRevision,
    application: ApplyOpticalConstruction,
    cancelled: &dyn Fn() -> bool,
) -> Result<OpticalConstructionCandidate, ProjectError> {
    let check_cancel = || {
        if cancelled() {
            Err(error(
                ProjectErrorCode::Cancelled,
                "optical construction application cancelled",
            ))
        } else {
            Ok(())
        }
    };
    check_cancel()?;
    application.validate().map_err(manufacturing_error)?;
    let study = captured
        .appearance_study
        .optical
        .as_ref()
        .ok_or_else(|| invalid("no retained optical study"))?;
    let palette = captured
        .appearance_study
        .palette
        .as_ref()
        .ok_or_else(|| invalid("no retained optical palette"))?;
    let report = study
        .report
        .as_ref()
        .ok_or_else(|| invalid("no accepted optical report"))?;
    study.matches_report(palette, report)?;
    let record = study
        .report_record
        .as_ref()
        .ok_or_else(|| invalid("no accepted optical report record"))?;
    if record.hash != application.report_hash
        || !record.complete
        || !report.is_complete()
        || study.selection != Some(application.selection)
        || !report.has_selection(&application.selection)
    {
        return Err(invalid(
            "application must select the exact accepted complete optical report and retained selection",
        ));
    }
    if study.request.materials.len() != 2 {
        return Err(invalid(
            "surface routing requires exactly two explicitly bound study materials",
        ));
    }
    let recipe = match application.selection {
        OpticalSelection::Requested => study.request.recipe.clone(),
        OpticalSelection::Candidate { index } => study.request.candidate(index)?,
    };
    let palette_asset = captured
        .appearance_palettes
        .get(&palette.canonical_hash)
        .ok_or_else(|| {
            error(
                ProjectErrorCode::MissingAsset,
                "retained optical palette is missing",
            )
        })?;
    let mut intent = captured
        .manufacturing_intent
        .as_ref()
        .ok_or_else(|| {
            invalid("apply requires an existing source-bound manufacturing construction")
        })?
        .as_ref()
        .clone();
    let JobWorkflow::FixedDualMarlin { job } = &mut intent.printer.workflow else {
        return Err(invalid(
            "optical construction requires the fixed-dual manufacturing route",
        ));
    };
    let construction = job
        .surface_constructions
        .get(usize::from(application.construction_index))
        .ok_or_else(|| invalid("selected surface construction is absent"))?;
    let occurrence = captured
        .occurrences
        .get(&construction.occurrence_id)
        .ok_or_else(|| invalid("surface construction occurrence is absent"))?;
    let source = captured
        .sources
        .get(&construction.definition_id)
        .ok_or_else(|| invalid("surface construction source is absent"))?;
    if occurrence.definition_id != construction.definition_id
        || source.record.provenance.source_hash != construction.source_hash
    {
        return Err(invalid(
            "surface construction does not bind its immutable source/occurrence",
        ));
    }
    let mut materials = Vec::new();
    for (index, binding) in application.bindings.iter().enumerate() {
        check_cancel()?;
        if binding.evidence_key != study.request.materials[index]
            || binding.evidence_hash != report.material_evidence_hashes[index]
        {
            return Err(invalid(
                "explicit evidence keys/hashes must match retained study material order",
            ));
        }
        let row = palette_asset
            .document()
            .rows
            .iter()
            .find(|row| row.key == binding.evidence_key)
            .ok_or_else(|| invalid("applied evidence is absent from retained palette"))?;
        let loaded = &job.tools[usize::from(
            job.tool_for_material(&binding.loaded_material_id)
                .map_err(manufacturing_error)?,
        )]
        .material;
        if loaded.declaration_hash != binding.loaded_declaration_hash
            || row.evidence_hash()? != binding.evidence_hash
        {
            return Err(invalid(
                "explicit loaded-material declaration or optical evidence changed",
            ));
        }
        materials.push(AppliedOpticalMaterial {
            evidence: row.clone(),
            evidence_hash: binding.evidence_hash.clone(),
            loaded_material: loaded.clone(),
        });
    }
    let materials: [AppliedOpticalMaterial; 2] = materials
        .try_into()
        .map_err(|_| invalid("optical application requires two captured materials"))?;
    let material_ids = materials.each_ref().map(|m| m.loaded_material.id.clone());
    let origin = OpticalConstructionOrigin {
        report: record.clone(),
        palette_hash: palette.canonical_hash.clone(),
        selection: application.selection,
        model: study.request.model,
        observation: study.request.observation,
        target: study.request.target,
        requested_patch: study.request.patch,
        recipe: recipe.clone(),
        materials,
        applied_pose: occurrence.pose,
    };
    let construction = &mut job.surface_constructions[usize::from(application.construction_index)];
    construction.shell = OccurrenceMaterial::LayerCycle {
        recipe,
        material_ids,
    };
    construction.optical_origin = Some(origin);
    intent.validate().map_err(manufacturing_error)?;
    check_cancel()?;
    Ok(OpticalConstructionCandidate {
        captured,
        captured_revision,
        intent: Arc::new(intent),
        construction_index: application.construction_index,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{optical_tests::optical_project, tests::commit_appearance};

    fn prepared_project() -> (Project, ApplyOpticalConstruction) {
        let (mut project, _, report) = optical_project();
        let snapshot = project.snapshot();
        let occurrence = snapshot.occurrences.values().next().unwrap();
        let source = &snapshot.sources[&occurrence.definition_id];
        let mut intent: ManufacturingIntent = serde_json::from_str(include_str!(
            "../../../fixtures/manufacturing/surface-color-box.intent.json"
        ))
        .unwrap();
        let JobWorkflow::FixedDualMarlin { job } = &mut intent.printer.workflow else {
            unreachable!()
        };
        job.assignments[0].occurrence_id = occurrence.occurrence_id;
        let construction = &mut job.surface_constructions[0];
        construction.occurrence_id = occurrence.occurrence_id;
        construction.definition_id = occurrence.definition_id.clone();
        construction.source_hash = source.record.provenance.source_hash.clone();
        let bindings = std::array::from_fn(|i| OpticalMaterialBinding {
            evidence_key: report.request.materials[i].clone(),
            evidence_hash: report.material_evidence_hashes[i].clone(),
            loaded_material_id: job.tools[i].material.id.clone(),
            loaded_declaration_hash: job.tools[i].material.declaration_hash.clone(),
        });
        commit_appearance(
            &mut project,
            Edit::SetManufacturingIntent {
                intent: Arc::new(intent),
            },
        );
        (
            project,
            ApplyOpticalConstruction {
                construction_index: 0,
                report_hash: report.canonical_record().unwrap().hash,
                selection: OpticalSelection::Requested,
                bindings,
            },
        )
    }

    #[test]
    fn retained_application_is_atomic_revision_owned_and_undoable_with_typed_evidence_closure() {
        let (mut project, application) = prepared_project();
        let before = project.snapshot();
        let revision = project.info().revision;
        let candidate =
            prepare_optical_construction(before.clone(), revision, application.clone(), &|| false)
                .unwrap();
        assert!(Arc::ptr_eq(&project.snapshot(), &before));
        let edit = project
            .prepare(Edit::ApplyOpticalConstruction { candidate })
            .unwrap();
        assert_eq!(project.info().revision, revision);
        project.commit(edit).unwrap();
        assert_eq!(project.info().revision.0, revision.0 + 1);
        let intent = project.snapshot().manufacturing_intent.clone().unwrap();
        let JobWorkflow::FixedDualMarlin { job } = &intent.printer.workflow else {
            unreachable!()
        };
        let origin = job.surface_constructions[0]
            .optical_origin
            .as_ref()
            .unwrap();
        assert_eq!(origin.report.hash, application.report_hash);
        assert_eq!(
            origin.materials[0].evidence_hash,
            application.bindings[0].evidence_hash
        );
        assert_eq!(origin.materials[1].loaded_material, job.tools[1].material);
        let wire =
            spiling_contracts::rpc::OpticalConstructionOrigin::try_from(origin.clone()).unwrap();
        assert_eq!(OpticalConstructionOrigin::try_from(wire).unwrap(), *origin);
        let manifest = project.capture().manifest;
        let encoded = serde_json::to_vec(&manifest).unwrap();
        let decoded: ProjectManifest = serde_json::from_slice(&encoded).unwrap();
        decoded.validate().unwrap();
        project.undo().unwrap();
        assert!(Arc::ptr_eq(
            project.snapshot().manufacturing_intent.as_ref().unwrap(),
            before.manufacturing_intent.as_ref().unwrap()
        ));
        project.redo().unwrap();
        assert!(Arc::ptr_eq(
            project.snapshot().manufacturing_intent.as_ref().unwrap(),
            &intent
        ));
    }

    #[test]
    fn retained_candidate_uses_its_selected_recipe_and_explicit_material_order() {
        let (mut project, mut application) = prepared_project();
        let report = project
            .snapshot()
            .appearance_study
            .optical
            .as_ref()
            .unwrap()
            .report
            .as_ref()
            .unwrap()
            .clone();
        let index = report.alternatives[0];
        application.selection = OpticalSelection::Candidate { index };
        commit_appearance(
            &mut project,
            Edit::AcceptOpticalReport {
                report: report.clone(),
                selection: Some(application.selection),
            },
        );
        let expected = report.request.candidate(index).unwrap();
        let candidate = prepare_optical_construction(
            project.snapshot(),
            project.info().revision,
            application,
            &|| false,
        )
        .unwrap();
        let edit = project
            .prepare(Edit::ApplyOpticalConstruction { candidate })
            .unwrap();
        project.commit(edit).unwrap();
        let snapshot = project.snapshot();
        let JobWorkflow::FixedDualMarlin { job } = &snapshot
            .manufacturing_intent
            .as_ref()
            .unwrap()
            .printer
            .workflow
        else {
            unreachable!()
        };
        assert_eq!(
            job.surface_constructions[0]
                .optical_origin
                .as_ref()
                .unwrap()
                .recipe,
            expected
        );
        assert!(
            matches!(&job.surface_constructions[0].shell,OccurrenceMaterial::LayerCycle {recipe,..} if recipe==&expected)
        );
    }

    #[test]
    fn application_rejects_stale_even_after_identical_undo_and_preserves_state_on_cancellation_or_bad_bindings()
     {
        let (mut project, application) = prepared_project();
        let before = project.snapshot();
        let revision = project.info().revision;
        assert_eq!(
            prepare_optical_construction(before.clone(), revision, application.clone(), &|| true)
                .unwrap_err()
                .code,
            ProjectErrorCode::Cancelled
        );
        for mutation in 0..5 {
            let mut wrong = application.clone();
            match mutation {
                0 => wrong.report_hash = SourceHash::from_bytes(b"other report"),
                1 => wrong.bindings[0].evidence_hash = SourceHash::from_bytes(b"other evidence"),
                2 => {
                    wrong.bindings[0].loaded_declaration_hash =
                        SourceHash::from_bytes(b"other loaded material")
                }
                3 => wrong.bindings.swap(0, 1),
                _ => wrong.selection = OpticalSelection::Candidate { index: 0 },
            }
            assert!(
                prepare_optical_construction(before.clone(), revision, wrong, &|| false).is_err()
            );
            assert!(Arc::ptr_eq(&project.snapshot(), &before));
        }
        let candidate =
            prepare_optical_construction(before.clone(), revision, application, &|| false).unwrap();
        let occurrence = before.occurrences.values().next().unwrap();
        let mut pose = occurrence.pose;
        pose.translation_mm[0] += 1.0;
        commit_appearance(
            &mut project,
            Edit::SetPose {
                occurrence_id: occurrence.occurrence_id,
                pose,
            },
        );
        project.undo().unwrap();
        assert!(Arc::ptr_eq(&project.snapshot(), &before));
        assert_eq!(
            project
                .prepare(Edit::ApplyOpticalConstruction { candidate })
                .unwrap_err()
                .code,
            ProjectErrorCode::StaleRevision
        );
    }
}
