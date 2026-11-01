// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
//! Transactional recipe snapshots and fresh study admission; no filesystem or manufacturing routing.
use super::*;
use spiling_contracts::library::*;
impl Project {
    pub(crate) fn store_recipe(
        &self,
        snapshot: &mut Snapshot,
        transfer: &spiling_contracts::library::RecipeTransfer,
        expected: Option<&SourceHash>,
    ) -> Result<(), ProjectError> {
        let collection = snapshot
            .materials
            .as_ref()
            .map(|a| a.document())
            .transpose()?
            .unwrap_or_default();
        let mut collection = collection.with_recipe(transfer, expected).map_err(|e| {
            error(
                if e.code == spiling_contracts::library::LibraryErrorCode::StaleRevision {
                    ProjectErrorCode::StaleRevision
                } else {
                    ProjectErrorCode::InvalidProject
                },
                e.message,
            )
        })?;
        materials::prune_revisions(&mut collection, []);
        snapshot.materials = Some(Arc::new(materials::MaterialAsset::new(&collection)?));
        Ok(())
    }
    pub(crate) fn capture_recipe(
        &self,
        snapshot: &mut Snapshot,
        id: RecipeId,
        name: String,
        ingredients: Vec<RecipeIngredient>,
        expected: Option<SourceHash>,
    ) -> Result<(), ProjectError> {
        let study = snapshot
            .appearance_study
            .optical
            .as_ref()
            .ok_or_else(|| invalid("no optical study to capture"))?;
        let palette = snapshot
            .appearance_study
            .palette
            .as_ref()
            .ok_or_else(|| invalid("no retained palette"))?;
        let report = study
            .report
            .as_ref()
            .ok_or_else(|| invalid("assess and retain a selection before saving its recipe"))?;
        study.matches_report(palette, report)?;
        let selection = study
            .selection
            .ok_or_else(|| invalid("choose and retain an assessed selection before saving"))?;
        if !report.is_complete() || !report.has_selection(&selection) {
            return Err(invalid(
                "recipe requires a retained complete assessed selection",
            ));
        }
        let mut request = study.request.as_ref().clone();
        request.recipe = match selection {
            OpticalSelection::Requested => request.recipe.clone(),
            OpticalSelection::Candidate { index } => request.candidate(index)?,
        };
        let mut palette = snapshot
            .appearance_palettes
            .get(&palette.canonical_hash)
            .ok_or_else(|| invalid("missing palette"))?
            .document()
            .clone();
        palette
            .rows
            .retain(|row| request.materials.contains(&row.key));
        let collection = snapshot
            .materials
            .as_ref()
            .map(|a| a.document())
            .transpose()?
            .unwrap_or_default();
        let filaments = ingredients
            .iter()
            .map(|i| {
                collection
                    .revisions
                    .iter()
                    .find(|r| r.reference == i.filament)
                    .cloned()
                    .ok_or_else(|| {
                        invalid("recipe ingredient must be an exact project filament snapshot")
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let definition = spiling_contracts::library::RecipeDefinition {
            name,
            ingredients,
            palette,
            request,
        };
        let recipe = spiling_contracts::library::RecipeEntry {
            id,
            revision: definition.content_hash().map_err(invalid)?,
            definition,
        };
        self.store_recipe(
            snapshot,
            &spiling_contracts::library::RecipeTransfer { recipe, filaments },
            expected.as_ref(),
        )?;
        Ok(())
    }
    pub(crate) fn load_recipe_study(
        &self,
        snapshot: &mut Snapshot,
        reference: RecipeRef,
        context: RecipeStudyContext,
    ) -> Result<(), ProjectError> {
        let collection = snapshot
            .materials
            .as_ref()
            .ok_or_else(|| invalid("no project recipes"))?
            .document()?;
        let transfer = collection.recipe_transfer(&reference).map_err(invalid)?;
        let request = context
            .request(&transfer.recipe.definition)
            .map_err(invalid)?;
        let document = transfer.recipe.definition.palette;
        let canonical = document.canonical_bytes()?;
        let record = PaletteRecord::new(&document, &canonical, &canonical, document.name.clone())?;
        let asset = Arc::new(PaletteAsset::from_document(
            record,
            canonical.clone(),
            canonical,
            document,
        )?);
        request.validate_palette(asset.prepared())?;
        self.install_palette(snapshot, asset)?;
        Arc::make_mut(&mut snapshot.appearance_study).optical = Some(OpticalStudy::new(request));
        Ok(())
    }
}
#[cfg(test)]
mod tests;
