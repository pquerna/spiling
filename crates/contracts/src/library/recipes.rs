// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
//! Bounded complete recipe closures, independent of physical stock and application.
use super::*;
use crate::{appearance::AppearanceColor, optical::OpticalPatch};

/// Complete transfers use the bounded control path; oversized definitions refuse explicitly.
pub const MAX_RECIPE_TRANSFER_BYTES: usize = 48 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RecipeRef {
    pub id: RecipeId,
    pub revision: SourceHash,
}
impl RecipeRef {
    pub fn validate(&self) -> Result<(), String> {
        validate_library_uuid(&self.id.0)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RecipeTransfer {
    pub recipe: RecipeEntry,
    pub filaments: Vec<FilamentRevision>,
}
impl RecipeEntry {
    pub fn reference(&self) -> RecipeRef {
        RecipeRef {
            id: self.id.clone(),
            revision: self.revision.clone(),
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        validate_library_uuid(&self.id.0)?;
        if self.revision != self.definition.content_hash()? {
            return Err("recipe content differs from its immutable revision".into());
        }
        Ok(())
    }
}
impl RecipeTransfer {
    pub fn validate(&self) -> Result<(), String> {
        self.recipe.validate()?;
        let ingredients = &self.recipe.definition.ingredients;
        if self.filaments.len() != ingredients.len() {
            return Err("recipe transfer requires exactly its ingredient snapshots".into());
        }
        let mut refs = BTreeSet::new();
        for filament in &self.filaments {
            filament.validate()?;
            if !refs.insert(&filament.reference)
                || !ingredients.iter().any(|i| i.filament == filament.reference)
            {
                return Err("recipe transfer has a duplicate or unrelated ingredient".into());
            }
        }
        if serde_json::to_vec(self).map_err(|e| e.to_string())?.len() > MAX_RECIPE_TRANSFER_BYTES {
            return Err("complete recipe transfer exceeds 48 KiB; reduce captured profiles/search data explicitly".into());
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RecipeSummary {
    pub reference: RecipeRef,
    pub name: String,
    pub ingredient_count: u32,
    pub target: AppearanceColor,
}
impl RecipeSummary {
    pub fn validate(&self) -> Result<(), String> {
        self.reference.validate()?;
        text(&self.name, 256)?;
        if !(2..=3).contains(&self.ingredient_count) {
            return Err("recipe needs two or three ingredients".into());
        }
        self.target.validate().map_err(|e| e.to_string())
    }
}
impl From<&RecipeEntry> for RecipeSummary {
    fn from(recipe: &RecipeEntry) -> Self {
        Self {
            reference: recipe.reference(),
            name: recipe.definition.name.clone(),
            ingredient_count: recipe.definition.ingredients.len() as u32,
            target: recipe.definition.request.target.color,
        }
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LibraryRecipeQuery {
    pub expected: Option<LibraryIdentity>,
    pub query: String,
    pub offset: u32,
}
impl LibraryRecipeQuery {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(id) = &self.expected {
            id.validate()?;
        }
        if self.query.len() > 256
            || self.query.chars().any(char::is_control)
            || self.offset as usize > MAX_RECIPES
        {
            return Err("recipe query exceeds bounds".into());
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LibraryRecipePage {
    pub identity: Option<LibraryIdentity>,
    pub entries: Vec<RecipeSummary>,
    pub total_matches: u32,
    pub next_offset: Option<u32>,
}
impl LibraryRecipePage {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(id) = &self.identity {
            id.validate()?;
        }
        if self.identity.is_none() && (self.total_matches != 0 || self.next_offset.is_some()) {
            return Err("recipe page without library identity".into());
        }
        validate_recipe_page(&self.entries, self.total_matches, self.next_offset)
    }
}
pub(crate) fn validate_recipe_page(
    entries: &[RecipeSummary],
    total: u32,
    next: Option<u32>,
) -> Result<(), String> {
    if entries.len() > MAX_LIBRARY_PAGE as usize
        || total as usize > MAX_RECIPES
        || entries.len() > total as usize
        || next.is_some_and(|n| n == 0 || n >= total)
    {
        return Err("invalid bounded recipe page".into());
    }
    let mut ids = BTreeSet::new();
    for entry in entries {
        entry.validate()?;
        if !ids.insert(&entry.reference.id) {
            return Err("duplicate recipe page entry".into());
        }
    }
    Ok(())
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RecipeStudyContext {
    pub patch: OpticalPatch,
    pub cycle_origin_z_mm: f64,
}
impl RecipeStudyContext {
    pub fn validate(&self) -> Result<(), String> {
        self.patch.validate().map_err(|e| e.to_string())?;
        if !self.cycle_origin_z_mm.is_finite() {
            return Err("nonfinite cycle origin".into());
        }
        Ok(())
    }
    pub fn request(&self, recipe: &RecipeDefinition) -> Result<OpticalRequest, String> {
        let mut request = recipe.request.clone();
        request.patch = self.patch;
        request.recipe.origin_z_mm = self.cycle_origin_z_mm;
        request.search.origin_z_mm = self.cycle_origin_z_mm;
        request.validate().map_err(|e| e.to_string())?;
        Ok(request)
    }
}
impl MaterialCollection {
    pub fn recipe_transfer(&self, reference: &RecipeRef) -> Result<RecipeTransfer, String> {
        reference.validate()?;
        let recipe = self
            .recipes
            .iter()
            .find(|r| r.reference() == *reference)
            .ok_or("recipe revision is absent; refresh before reuse")?
            .clone();
        let filaments = recipe
            .definition
            .ingredients
            .iter()
            .map(|i| {
                self.revisions
                    .iter()
                    .find(|r| r.reference == i.filament)
                    .cloned()
                    .ok_or_else(|| "recipe dependency is missing".into())
            })
            .collect::<Result<Vec<_>, String>>()?;
        let transfer = RecipeTransfer { recipe, filaments };
        transfer.validate()?;
        Ok(transfer)
    }
    /// Consumes a candidate collection; errors never expose a partially merged result.
    pub fn with_recipe(
        mut self,
        transfer: &RecipeTransfer,
        expected: Option<&SourceHash>,
    ) -> Result<Self, LibraryError> {
        let invalid = |e| LibraryError::new(LibraryErrorCode::InvalidData, e);
        transfer.validate().map_err(invalid)?;
        let current = self.recipes.iter().position(|r| r.id == transfer.recipe.id);
        if current.map(|i| &self.recipes[i].revision) != expected {
            return Err(LibraryError::new(
                LibraryErrorCode::StaleRevision,
                "recipe changed; review the retained draft",
            ));
        }
        for filament in &transfer.filaments {
            if let Some(existing) = self
                .revisions
                .iter()
                .find(|r| r.reference == filament.reference)
            {
                if existing != filament {
                    return Err(invalid("conflicting immutable ingredient revision".into()));
                }
            } else {
                self.revisions.push(filament.clone());
            }
            // Capturing historical ingredients never replaces an existing current variant or stock.
            if !self
                .entries
                .iter()
                .any(|e| e.current.id == filament.reference.id)
            {
                self.entries.push(FilamentEntry {
                    current: filament.reference.clone(),
                    archived: false,
                    origin: None,
                });
            }
        }
        if let Some(index) = current {
            self.recipes[index] = transfer.recipe.clone();
        } else {
            self.recipes.push(transfer.recipe.clone());
        }
        self.canonical_bytes().map_err(invalid)?;
        Ok(self)
    }
}

impl RecipeTransfer {
    /// An explicit experiment from declared sRGB; never promotes a declaration to measured optics.
    pub fn from_declarations(
        id: RecipeId,
        name: String,
        filaments: Vec<FilamentRevision>,
        target: AppearanceColor,
        heights_mm: Vec<f64>,
    ) -> Result<Self, String> {
        use crate::{appearance::*, optical::*};
        if !(2..=3).contains(&filaments.len()) || heights_mm.len() != filaments.len() {
            return Err(
                "choose two or three distinct filaments and one physical slot height each".into(),
            );
        }
        for filament in &filaments {
            filament.validate()?;
        }
        let ingredients: Vec<_> = filaments
            .iter()
            .map(|f| RecipeIngredient {
                filament: f.reference.clone(),
                palette_key: format!("{}/declared", f.reference.id.0),
            })
            .collect();
        let rows = filaments.iter().zip(&ingredients).map(|(f, i)| PaletteRow {
            key: i.palette_key.clone(), material_id: f.reference.id.0.clone(), evidence_id: "declared".into(), name: f.definition.name.clone(),
            evidence: match f.definition.appearance.as_ref().map(|a| a.color) {
                Some(AppearanceColor::Srgb { channels }) => AppearanceEvidence::CatalogEstimate { srgb: channels },
                _ => AppearanceEvidence::Unknown { reason: "No declared sRGB estimate; appearance declaration is not a calibrated optical measurement".into() },
            },
        }).collect();
        let palette = PaletteDocument {
            name: name.clone(),
            source_url: "https://github.com/pquerna/spiling/blob/main/docs/protocol/library.md#reusable-recipes".into(),
            source_revision: SourceHash::from_bytes(
                &serde_json::to_vec(&filaments).map_err(|e| e.to_string())?,
            )
            .as_str()
            .into(),
            license: "Operator and imported declarations; source rights retained".into(),
            conversion: PALETTE_CONVERSION.into(),
            rows,
        };
        let pattern: Vec<_> = (0..filaments.len() as u8).collect();
        let mut reverse = pattern.clone();
        reverse.reverse();
        let request = OpticalRequest {
            model: OpticalModel::PrusaV7_007bb922,
            materials: ingredients.iter().map(|i| i.palette_key.clone()).collect(),
            target: OpticalTarget {
                color: target,
                delta_e00_tolerance: 2.0,
                lightness_tolerance: None,
            },
            observation: OpticalObservation {
                illuminant: AppearanceIlluminant::D65,
                observer: AppearanceObserver::Cie1931TwoDegree,
                view: OpticalView::NormalToPatch,
            },
            patch: OpticalPatch {
                origin_mm: [0.0; 3],
                normal_xy: [0.0, -1.0],
                width_mm: 10.0,
                height_mm: 10.0,
            },
            recipe: LayerCycle {
                slots: pattern
                    .iter()
                    .zip(heights_mm)
                    .map(|(&material, height_mm)| OpticalSlot {
                        material,
                        height_mm,
                    })
                    .collect(),
                phase_slot: 0,
                origin_z_mm: 0.0,
            },
            search: OpticalSearchSpace {
                patterns: vec![
                    OpticalPattern { materials: pattern },
                    OpticalPattern { materials: reverse },
                ],
                slot_heights_mm: vec![0.1, 0.2, 0.3],
                phase_slots: (0..filaments.len() as u16).collect(),
                origin_z_mm: 0.0,
            },
            limits: SearchLimits {
                max_visited: 1024,
                max_alternatives: 16,
            },
        };
        let definition = RecipeDefinition {
            name,
            ingredients,
            palette,
            request,
        };
        let value = Self {
            recipe: RecipeEntry {
                id,
                revision: definition.content_hash()?,
                definition,
            },
            filaments,
        };
        value.validate()?;
        Ok(value)
    }
}
