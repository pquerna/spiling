// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::*;
use std::sync::atomic::AtomicBool;
fn ingredients(project: &mut Project, keys: &[String]) -> Vec<RecipeIngredient> {
    keys.iter()
        .enumerate()
        .map(|(i, key)| {
            let definition = FilamentDefinition {
                declaration: Default::default(),
                catalog_origin: None,
                name: format!("Recipe ingredient {i}"),
                material: None,
                color_name: None,
                appearance: None,
                profiles: vec![],
                source: DefinitionSource::Operator,
            };
            let revision = FilamentRevision {
                reference: FilamentRef {
                    id: FilamentId(uuid::Uuid::new_v4().to_string()),
                    revision: definition.content_hash().unwrap(),
                },
                definition,
            };
            let ingredient = RecipeIngredient {
                filament: revision.reference.clone(),
                palette_key: key.clone(),
            };
            let edit = project
                .prepare(Edit::PutFilament {
                    revision,
                    origin: None,
                    expected: None,
                })
                .unwrap();
            project.commit(edit).unwrap();
            ingredient
        })
        .collect()
}
#[test]
fn capture_uses_selected_candidate_and_reuse_clears_reports_preserving_manufacturing() {
    let (mut project, asset, _) = crate::optical_tests::optical_project();
    let mut request = project
        .appearance_study()
        .optical
        .as_ref()
        .unwrap()
        .request
        .as_ref()
        .clone();
    request.search.phase_slots = vec![1];
    crate::tests::commit_appearance(
        &mut project,
        Edit::SetOpticalStudy {
            request: request.clone(),
        },
    );
    let report = crate::optical_tests::optical_report(&asset, Arc::new(request.clone()));
    crate::tests::commit_appearance(
        &mut project,
        Edit::AcceptOpticalReport {
            report,
            selection: Some(OpticalSelection::Candidate { index: 0 }),
        },
    );
    let ingredients = ingredients(&mut project, &request.materials);
    let id = RecipeId(uuid::Uuid::new_v4().to_string());
    let before = project.snapshot();
    let edit = project
        .prepare(Edit::CaptureRecipe {
            id: id.clone(),
            name: "Candidate recipe".into(),
            ingredients,
            expected: None,
        })
        .unwrap();
    project.commit(edit).unwrap();
    let collection = project
        .snapshot()
        .materials
        .as_ref()
        .unwrap()
        .document()
        .unwrap();
    let recipe = &collection.recipes[0];
    assert_eq!(recipe.id, id);
    assert_eq!(
        recipe.definition.request.recipe,
        request.candidate(0).unwrap()
    );
    assert_ne!(recipe.definition.request.recipe, request.recipe);
    assert_eq!(
        recipe.definition.palette.source_revision,
        asset.document().source_revision
    );
    assert!(Arc::ptr_eq(
        project.snapshot().manufacturing_artifact.as_ref().unwrap(),
        before.manufacturing_artifact.as_ref().unwrap()
    ));
    let context = RecipeStudyContext {
        patch: OpticalPatch {
            origin_mm: [0.0, 0.0, 1.0],
            normal_xy: [1.0, 0.0],
            width_mm: 2.0,
            height_mm: 3.0,
        },
        cycle_origin_z_mm: 0.03,
    };
    let edit = project
        .prepare(Edit::LoadRecipeStudy {
            reference: recipe.reference(),
            context: context.clone(),
        })
        .unwrap();
    project.commit(edit).unwrap();
    let reloaded = project.appearance_study();
    let study = reloaded.optical.as_ref().unwrap();
    assert_eq!(study.request.patch, context.patch);
    assert!(study.report.is_none() && study.report_record.is_none() && study.selection.is_none());
    assert!(reloaded.report.is_none() && reloaded.selection.is_none());
    assert!(Arc::ptr_eq(
        project.snapshot().manufacturing_artifact.as_ref().unwrap(),
        before.manufacturing_artifact.as_ref().unwrap()
    ));
    project.undo().unwrap();
    assert_eq!(
        project
            .appearance_study()
            .optical
            .as_ref()
            .unwrap()
            .selection,
        Some(OpticalSelection::Candidate { index: 0 })
    );
    project.redo().unwrap();
    assert!(
        project
            .appearance_study()
            .optical
            .as_ref()
            .unwrap()
            .selection
            .is_none()
    );
}
#[test]
fn complete_project_recipe_survives_history_save_and_standalone_readonly_reopen() {
    let mut project = Project::new();
    let _ = ingredients(
        &mut project,
        &[
            "a/declared".into(),
            "b/declared".into(),
            "c/declared".into(),
        ],
    );
    let filaments = project
        .snapshot()
        .materials
        .as_ref()
        .unwrap()
        .document()
        .unwrap()
        .revisions;
    let transfer = RecipeTransfer::from_declarations(
        RecipeId(uuid::Uuid::new_v4().to_string()),
        "Three material study".into(),
        filaments,
        AppearanceColor::Srgb { channels: [0.3; 3] },
        vec![0.2; 3],
    )
    .unwrap();
    let edit = project
        .prepare(Edit::PutRecipe {
            transfer: transfer.clone(),
            expected: None,
        })
        .unwrap();
    project.commit(edit).unwrap();
    project.undo().unwrap();
    assert!(
        project
            .snapshot()
            .materials
            .as_ref()
            .unwrap()
            .document()
            .unwrap()
            .recipes
            .is_empty()
    );
    project.redo().unwrap();
    let base = project.info().revision;
    assert_eq!(
        project
            .prepare(Edit::PutRecipe {
                transfer: transfer.clone(),
                expected: None
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::StaleRevision
    );
    assert_eq!(project.info().revision, base);
    let root = std::env::temp_dir().join(format!("spiling-recipe-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("Standalone.spil");
    let saved = storage::save(
        project.capture(),
        None,
        Some(path.clone()),
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap();
    project.mark_saved(saved).unwrap();
    drop(project);
    let mut project =
        Project::from_loaded(storage::open(&path, true, false, &AtomicBool::new(false)).unwrap())
            .unwrap();
    assert_eq!(
        project
            .snapshot()
            .materials
            .as_ref()
            .unwrap()
            .document()
            .unwrap()
            .recipe_transfer(&transfer.recipe.reference())
            .unwrap(),
        transfer
    );
    assert_eq!(
        project
            .prepare(Edit::PutRecipe {
                transfer,
                expected: None
            })
            .unwrap_err()
            .code,
        ProjectErrorCode::ReadOnly
    );
    assert!(!project.info().dirty);
    assert!(project.undo().is_err());
    drop(project);
    std::fs::remove_dir_all(root).unwrap();
}
