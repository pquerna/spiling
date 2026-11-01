// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
//! Checked, typed recipe and evidence-closure adapters.
use super::required;
use crate::{appearance::PaletteDocument, geometry::SourceHash, library::*, rpc};
fn hash(v: String) -> Result<SourceHash, String> {
    SourceHash::parse(v).map_err(|e| e.to_string())
}
impl TryFrom<rpc::AppearancePaletteDocument> for PaletteDocument {
    type Error = String;
    fn try_from(v: rpc::AppearancePaletteDocument) -> Result<Self, String> {
        let value = Self {
            name: v.name,
            source_url: v.source_url,
            source_revision: v.source_revision,
            license: v.license,
            conversion: v.conversion,
            rows: v
                .rows
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        };
        value.validate().map_err(|e| e.to_string())?;
        Ok(value)
    }
}
impl TryFrom<PaletteDocument> for rpc::AppearancePaletteDocument {
    type Error = String;
    fn try_from(v: PaletteDocument) -> Result<Self, String> {
        v.validate().map_err(|e| e.to_string())?;
        let value = Self {
            name: v.name,
            source_url: v.source_url,
            source_revision: v.source_revision,
            license: v.license,
            conversion: v.conversion,
            rows: v
                .rows
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        };
        Ok(value)
    }
}
impl TryFrom<rpc::LibraryRecipeRef> for RecipeRef {
    type Error = String;
    fn try_from(v: rpc::LibraryRecipeRef) -> Result<Self, String> {
        let value = Self {
            id: RecipeId(v.id),
            revision: hash(v.revision)?,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<RecipeRef> for rpc::LibraryRecipeRef {
    type Error = String;
    fn try_from(v: RecipeRef) -> Result<Self, String> {
        v.validate()?;
        let value = Self {
            id: v.id.0,
            revision: v.revision.as_str().into(),
        };
        Ok(value)
    }
}
impl TryFrom<rpc::LibraryRecipeIngredient> for RecipeIngredient {
    type Error = String;
    fn try_from(v: rpc::LibraryRecipeIngredient) -> Result<Self, String> {
        let value = Self {
            filament: required(v.filament, "ingredient filament")?.try_into()?,
            palette_key: v.palette_key,
        };
        value.filament.validate()?;
        Ok(value)
    }
}
impl TryFrom<RecipeIngredient> for rpc::LibraryRecipeIngredient {
    type Error = String;
    fn try_from(v: RecipeIngredient) -> Result<Self, String> {
        v.filament.validate()?;
        let value = Self {
            filament: Some(v.filament.try_into()?),
            palette_key: v.palette_key,
        };
        Ok(value)
    }
}
impl TryFrom<rpc::LibraryRecipeDefinition> for RecipeDefinition {
    type Error = String;
    fn try_from(v: rpc::LibraryRecipeDefinition) -> Result<Self, String> {
        let value = Self {
            name: v.name,
            ingredients: v
                .ingredients
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            palette: required(v.palette, "recipe palette")?.try_into()?,
            request: required(v.request, "recipe request")?.try_into()?,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<RecipeDefinition> for rpc::LibraryRecipeDefinition {
    type Error = String;
    fn try_from(v: RecipeDefinition) -> Result<Self, String> {
        v.validate()?;
        let value = Self {
            name: v.name,
            ingredients: v
                .ingredients
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            palette: Some(v.palette.try_into()?),
            request: Some(v.request.try_into()?),
        };
        Ok(value)
    }
}
impl TryFrom<rpc::LibraryRecipeEntry> for RecipeEntry {
    type Error = String;
    fn try_from(v: rpc::LibraryRecipeEntry) -> Result<Self, String> {
        let value = Self {
            id: RecipeId(v.id),
            revision: hash(v.revision)?,
            definition: required(v.definition, "recipe definition")?.try_into()?,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<RecipeEntry> for rpc::LibraryRecipeEntry {
    type Error = String;
    fn try_from(v: RecipeEntry) -> Result<Self, String> {
        v.validate()?;
        let value = Self {
            id: v.id.0,
            revision: v.revision.as_str().into(),
            definition: Some(v.definition.try_into()?),
        };
        Ok(value)
    }
}
impl TryFrom<rpc::LibraryRecipeTransfer> for RecipeTransfer {
    type Error = String;
    fn try_from(v: rpc::LibraryRecipeTransfer) -> Result<Self, String> {
        let value = Self {
            recipe: required(v.recipe, "recipe entry")?.try_into()?,
            filaments: v
                .filaments
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<RecipeTransfer> for rpc::LibraryRecipeTransfer {
    type Error = String;
    fn try_from(v: RecipeTransfer) -> Result<Self, String> {
        v.validate()?;
        let value = Self {
            recipe: Some(v.recipe.try_into()?),
            filaments: v
                .filaments
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
        };
        Ok(value)
    }
}
impl TryFrom<rpc::LibraryRecipeSummary> for RecipeSummary {
    type Error = String;
    fn try_from(v: rpc::LibraryRecipeSummary) -> Result<Self, String> {
        let value = Self {
            reference: required(v.reference, "recipe reference")?.try_into()?,
            name: v.name,
            ingredient_count: v.ingredient_count,
            target: required(v.target, "recipe target")?.try_into()?,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<RecipeSummary> for rpc::LibraryRecipeSummary {
    type Error = String;
    fn try_from(v: RecipeSummary) -> Result<Self, String> {
        v.validate()?;
        let value = Self {
            reference: Some(v.reference.try_into()?),
            name: v.name,
            ingredient_count: v.ingredient_count,
            target: Some(v.target.try_into()?),
        };
        Ok(value)
    }
}
impl TryFrom<rpc::LibraryListRecipesRequest> for LibraryRecipeQuery {
    type Error = String;
    fn try_from(v: rpc::LibraryListRecipesRequest) -> Result<Self, String> {
        let value = Self {
            expected: v.expected.map(TryInto::try_into).transpose()?,
            query: v.query,
            offset: v.offset,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<LibraryRecipeQuery> for rpc::LibraryListRecipesRequest {
    type Error = String;
    fn try_from(v: LibraryRecipeQuery) -> Result<Self, String> {
        v.validate()?;
        let value = Self {
            expected: v.expected.map(TryInto::try_into).transpose()?,
            query: v.query,
            offset: v.offset,
        };
        Ok(value)
    }
}
impl TryFrom<rpc::LibraryRecipePage> for LibraryRecipePage {
    type Error = String;
    fn try_from(v: rpc::LibraryRecipePage) -> Result<Self, String> {
        let value = Self {
            identity: v.identity.map(TryInto::try_into).transpose()?,
            entries: v
                .entries
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            total_matches: v.total_matches,
            next_offset: v.next_offset,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<LibraryRecipePage> for rpc::LibraryRecipePage {
    type Error = String;
    fn try_from(v: LibraryRecipePage) -> Result<Self, String> {
        v.validate()?;
        let value = Self {
            identity: v.identity.map(TryInto::try_into).transpose()?,
            entries: v
                .entries
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            total_matches: v.total_matches,
            next_offset: v.next_offset,
        };
        Ok(value)
    }
}
impl TryFrom<rpc::RecipeStudyContext> for RecipeStudyContext {
    type Error = String;
    fn try_from(v: rpc::RecipeStudyContext) -> Result<Self, String> {
        let value = Self {
            patch: required(v.patch, "recipe patch")?.try_into()?,
            cycle_origin_z_mm: v.cycle_origin_z_mm,
        };
        value.validate()?;
        Ok(value)
    }
}
impl TryFrom<RecipeStudyContext> for rpc::RecipeStudyContext {
    type Error = String;
    fn try_from(v: RecipeStudyContext) -> Result<Self, String> {
        v.validate()?;
        let value = Self {
            patch: Some(v.patch.try_into()?),
            cycle_origin_z_mm: v.cycle_origin_z_mm,
        };
        Ok(value)
    }
}

impl TryFrom<rpc::OpticalPatch> for crate::optical::OpticalPatch {
    type Error = String;
    fn try_from(v: rpc::OpticalPatch) -> Result<Self, String> {
        let o = required(v.origin_mm, "patch origin")?;
        let value = Self {
            origin_mm: [o.x, o.y, o.z],
            normal_xy: [v.normal_x, v.normal_y],
            width_mm: v.width_mm,
            height_mm: v.height_mm,
        };
        value.validate().map_err(|e| e.to_string())?;
        Ok(value)
    }
}
impl TryFrom<crate::optical::OpticalPatch> for rpc::OpticalPatch {
    type Error = String;
    fn try_from(v: crate::optical::OpticalPatch) -> Result<Self, String> {
        v.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            origin_mm: Some(rpc::Vec3 {
                x: v.origin_mm[0],
                y: v.origin_mm[1],
                z: v.origin_mm[2],
            }),
            normal_x: v.normal_xy[0],
            normal_y: v.normal_xy[1],
            width_mm: v.width_mm,
            height_mm: v.height_mm,
        })
    }
}
