// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::*;
use crate::{appearance::AppearanceColor, rpc};
fn filament(name: &str, rgb: [f64; 3]) -> FilamentRevision {
    let definition = FilamentDefinition {
        declaration: Default::default(),
        catalog_origin: None,
        name: name.into(),
        material: None,
        color_name: None,
        appearance: Some(DeclaredAppearance {
            color: AppearanceColor::Srgb { channels: rgb },
            source: DefinitionSource::Operator,
        }),
        profiles: vec![],
        source: DefinitionSource::Operator,
    };
    FilamentRevision {
        reference: FilamentRef {
            id: FilamentId(uuid::Uuid::new_v4().to_string()),
            revision: definition.content_hash().unwrap(),
        },
        definition,
    }
}
fn recipe(count: usize) -> RecipeTransfer {
    RecipeTransfer::from_declarations(
        RecipeId(uuid::Uuid::new_v4().to_string()),
        "Reusable dusk".into(),
        (0..count)
            .map(|i| filament(&format!("Filament {i}"), [i as f64 / 3.0, 0.3, 0.7]))
            .collect(),
        AppearanceColor::Srgb { channels: [0.4; 3] },
        vec![0.2; count],
    )
    .unwrap()
}
#[test]
fn complete_recipe_roundtrips_typed_wire_for_two_and_three_ingredients() {
    for count in [2, 3] {
        let transfer = recipe(count);
        let wire = rpc::LibraryRecipeTransfer::try_from(transfer.clone()).unwrap();
        assert_eq!(RecipeTransfer::try_from(wire).unwrap(), transfer);
        let request = LibraryWrite {
            library_id: None,
            request_id: uuid::Uuid::new_v4().to_string(),
            base_revision: 0,
            mutation: LibraryMutation::PutRecipe {
                transfer,
                expected: None,
            },
        };
        assert_eq!(
            LibraryWrite::try_from(rpc::LibraryWriteRequest::try_from(request.clone()).unwrap())
                .unwrap(),
            request
        );
    }
}
#[test]
fn recipe_closure_refuses_missing_foreign_duplicate_and_modified_dependencies() {
    let valid = recipe(2);
    let mut missing = valid.clone();
    missing.filaments.pop();
    assert!(missing.validate().is_err());
    let mut foreign = valid.clone();
    foreign.filaments[1] = filament("Other", [0.0; 3]);
    assert!(foreign.validate().is_err());
    let mut duplicate = valid.clone();
    duplicate.filaments[1] = duplicate.filaments[0].clone();
    assert!(duplicate.validate().is_err());
    let mut modified = valid.clone();
    modified.filaments[0].definition.name = "Changed without revision".into();
    assert!(modified.validate().is_err());
    let mut no_evidence = valid;
    no_evidence.recipe.definition.palette.rows.pop();
    assert!(no_evidence.validate().is_err());
}
#[test]
fn recipe_merge_retains_exact_historical_ingredients_without_changing_current_variants() {
    let transfer = recipe(2);
    let old = transfer.filaments[0].clone();
    let mut newer = old.clone();
    newer.definition.name = "New current variant".into();
    newer.reference.revision = newer.definition.content_hash().unwrap();
    let collection = MaterialCollection {
        entries: vec![FilamentEntry {
            current: newer.reference.clone(),
            archived: true,
            origin: None,
        }],
        revisions: vec![newer.clone()],
        recipes: vec![],
    };
    let collection = collection.with_recipe(&transfer, None).unwrap();
    assert_eq!(collection.entries[0].current, newer.reference);
    assert!(collection.entries[0].archived);
    assert_eq!(
        collection
            .recipe_transfer(&transfer.recipe.reference())
            .unwrap(),
        transfer
    );
    assert_eq!(
        collection
            .clone()
            .with_recipe(&transfer, None)
            .unwrap_err()
            .code,
        LibraryErrorCode::StaleRevision
    );
    let expected = transfer.recipe.revision.clone();
    let mut changed = transfer.clone();
    changed.recipe.definition.name = "Renamed recipe".into();
    changed.recipe.revision = changed.recipe.definition.content_hash().unwrap();
    let updated = collection.with_recipe(&changed, Some(&expected)).unwrap();
    assert!(
        updated
            .recipe_transfer(&transfer.recipe.reference())
            .is_err()
    );
    assert_eq!(
        updated
            .recipe_transfer(&changed.recipe.reference())
            .unwrap(),
        changed
    );
}
#[test]
fn unknown_declarations_stay_unknown_and_reuse_rebases_all_physical_origins() {
    let mut transfer = recipe(3);
    transfer.filaments[0].definition.appearance = None;
    transfer.filaments[0].reference.revision =
        transfer.filaments[0].definition.content_hash().unwrap();
    let transfer = RecipeTransfer::from_declarations(
        transfer.recipe.id,
        "Unknown optics".into(),
        transfer.filaments,
        AppearanceColor::Srgb { channels: [0.2; 3] },
        vec![0.1, 0.2, 0.3],
    )
    .unwrap();
    assert!(matches!(
        transfer.recipe.definition.palette.rows[0].evidence,
        crate::appearance::AppearanceEvidence::Unknown { .. }
    ));
    let context = RecipeStudyContext {
        patch: crate::optical::OpticalPatch {
            origin_mm: [2.0, 3.0, 4.0],
            normal_xy: [1.0, 0.0],
            width_mm: 7.0,
            height_mm: 8.0,
        },
        cycle_origin_z_mm: 0.03,
    };
    let request = context.request(&transfer.recipe.definition).unwrap();
    assert_eq!(request.patch, context.patch);
    assert_eq!(request.recipe.origin_z_mm, 0.03);
    assert_eq!(request.search.origin_z_mm, 0.03);
    assert_eq!(transfer.recipe.definition.request.recipe.origin_z_mm, 0.0);
}

#[test]
fn legitimate_large_dependency_closure_refuses_without_truncating_profiles() {
    let selection: crate::manufacturing::MaterialProfileSelection = serde_json::from_slice(
        include_bytes!("../../../../fixtures/manufacturing/material-profile-a.json"),
    )
    .unwrap();
    let mut filaments = recipe(3).filaments;
    for filament in &mut filaments {
        filament.definition.material = Some(selection.profile.material.clone());
        filament.definition.profiles = (0..16)
            .map(|i| {
                let mut profile = selection.profile.clone();
                profile.id = format!("bounded-profile-{i}");
                profile
            })
            .collect();
        filament.reference.revision = filament.definition.content_hash().unwrap();
        filament.validate().unwrap();
    }
    let error = RecipeTransfer::from_declarations(
        RecipeId(uuid::Uuid::new_v4().to_string()),
        "Full profile closure".into(),
        filaments,
        AppearanceColor::Srgb { channels: [0.3; 3] },
        vec![0.2; 3],
    )
    .unwrap_err();
    assert!(error.contains("48 KiB"), "{error}");
}
