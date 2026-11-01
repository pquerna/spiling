// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Portable material content and personal physical inventory are separate authorities.
use crate::{
    appearance::{AppearanceColor, PaletteDocument},
    geometry::SourceHash,
    manufacturing::{MaterialDefinition, MaterialProcessProfile},
    optical::OpticalRequest,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use ts_rs::TS;

mod recipes;
pub use recipes::*;
#[cfg(test)]
mod recipe_tests;

pub const LIBRARY_SCHEMA: u32 = 1;
pub const MAX_FILAMENTS: usize = 256;
pub const MAX_FILAMENT_REVISIONS: usize = 1024;
pub const MAX_SPOOLS: usize = 512;
pub const MAX_RECIPES: usize = 128;
pub const MAX_PROFILES_PER_FILAMENT: usize = 16;
pub const MAX_MATERIAL_COLLECTION_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_LIBRARY_RECEIPTS: usize = 64;
pub const MAX_FILAMENT_BYTES: usize = 32 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
pub struct LibraryId(pub String);
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
pub struct FilamentId(pub String);
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
pub struct SpoolId(pub String);
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
pub struct RecipeId(pub String);

pub fn validate_library_uuid(value: &str) -> Result<(), String> {
    let parsed = uuid::Uuid::parse_str(value).map_err(|_| "invalid library UUID")?;
    if parsed.is_nil() || parsed.to_string() != value {
        return Err("library identity requires a canonical nonzero UUID".into());
    }
    Ok(())
}
fn text(value: &str, max: usize) -> Result<(), String> {
    if value.is_empty()
        || value.len() > max
        || value.trim() != value
        || value.chars().any(char::is_control)
    {
        return Err("library text must be nonempty bounded canonical text".into());
    }
    Ok(())
}
fn required_option<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(deserializer)
}
fn optional_text(value: &Option<String>, max: usize) -> Result<(), String> {
    value.as_deref().map_or(Ok(()), |value| text(value, max))
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DefinitionSource {
    Operator,
    Imported { name: String, sha256: SourceHash },
}
impl DefinitionSource {
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::Operator => Ok(()),
            Self::Imported { name, .. } => text(name, 256),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DeclaredAppearance {
    pub color: AppearanceColor,
    pub source: DefinitionSource,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FilamentDefinition {
    pub name: String,
    #[serde(deserialize_with = "required_option")]
    pub material: Option<MaterialDefinition>,
    #[serde(deserialize_with = "required_option")]
    pub color_name: Option<String>,
    #[serde(deserialize_with = "required_option")]
    pub appearance: Option<DeclaredAppearance>,
    pub profiles: Vec<MaterialProcessProfile>,
    pub source: DefinitionSource,
}
impl FilamentDefinition {
    pub fn validate(&self) -> Result<(), String> {
        text(&self.name, 256)?;
        optional_text(&self.color_name, 128)?;
        self.source.validate()?;
        if let Some(material) = &self.material {
            material.validate().map_err(|e| e.to_string())?;
        }
        if let Some(appearance) = &self.appearance {
            appearance.color.validate().map_err(|e| e.to_string())?;
            appearance.source.validate()?;
        }
        if self.profiles.len() > MAX_PROFILES_PER_FILAMENT {
            return Err("too many process profiles for one filament".into());
        }
        let mut identities = BTreeSet::new();
        for profile in &self.profiles {
            profile.validate().map_err(|e| e.to_string())?;
            if self.material.as_ref() != Some(&profile.material)
                || !identities.insert((&profile.id, &profile.revision))
            {
                return Err("profile identity or captured material disagrees with filament".into());
            }
        }
        Ok(())
    }
    /// A profile/name/source update does not change the physical variant represented by owned stock.
    pub fn same_stock_variant(&self, other: &Self) -> bool {
        let material_ok = match (&self.material, &other.material) {
            (Some(a), Some(b)) => {
                a.id == b.id
                    && a.manufacturer == b.manufacturer
                    && a.product == b.product
                    && a.polymer_family == b.polymer_family
                    && a.filler == b.filler
                    && a.filament_diameter_mm == b.filament_diameter_mm
            }
            (None, Some(_)) | (None, None) => true,
            (Some(_), None) => false,
        };
        material_ok
            && match (&self.appearance, &other.appearance) {
                (Some(a), Some(b)) => a.color == b.color,
                (None, _) => true,
                (Some(_), None) => false,
            }
    }
    pub fn content_hash(&self) -> Result<SourceHash, String> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_FILAMENT_BYTES {
            return Err("filament definition exceeds 32 KiB".into());
        }
        Ok(SourceHash::from_bytes(&bytes))
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq, PartialOrd, Ord)]
#[serde(deny_unknown_fields)]
pub struct FilamentRef {
    pub id: FilamentId,
    pub revision: SourceHash,
}
impl FilamentRef {
    pub fn validate(&self) -> Result<(), String> {
        validate_library_uuid(&self.id.0)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FilamentRevision {
    pub reference: FilamentRef,
    pub definition: FilamentDefinition,
}
impl FilamentRevision {
    pub fn validate(&self) -> Result<(), String> {
        self.reference.validate()?;
        if self.definition.content_hash()? != self.reference.revision {
            return Err("filament content differs from its immutable revision".into());
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LibraryOrigin {
    pub library_id: LibraryId,
    pub filament: FilamentRef,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FilamentEntry {
    pub current: FilamentRef,
    pub archived: bool,
    #[serde(deserialize_with = "required_option")]
    pub origin: Option<LibraryOrigin>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RecipeIngredient {
    pub filament: FilamentRef,
    pub palette_key: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RecipeDefinition {
    pub name: String,
    pub ingredients: Vec<RecipeIngredient>,
    pub palette: PaletteDocument,
    pub request: OpticalRequest,
}
impl RecipeDefinition {
    pub fn validate(&self) -> Result<(), String> {
        text(&self.name, 256)?;
        self.palette.validate().map_err(|e| e.to_string())?;
        self.request.validate().map_err(|e| e.to_string())?;
        if self.ingredients.len() != self.request.materials.len()
            || self.palette.rows.len() != self.ingredients.len()
        {
            return Err("recipe ingredient count differs from optical request".into());
        }
        let mut keys = BTreeSet::new();
        let mut filaments = BTreeSet::new();
        for ingredient in &self.ingredients {
            ingredient.filament.validate()?;
            text(&ingredient.palette_key, 64)?;
            if !self.request.materials.contains(&ingredient.palette_key)
                || !self
                    .palette
                    .rows
                    .iter()
                    .any(|row| row.key == ingredient.palette_key)
                || !keys.insert(&ingredient.palette_key)
                || !filaments.insert(&ingredient.filament.id)
            {
                return Err(
                    "recipe ingredient is missing, repeated or foreign to its palette/request"
                        .into(),
                );
            }
        }
        Ok(())
    }
    pub fn content_hash(&self) -> Result<SourceHash, String> {
        self.validate()?;
        Ok(SourceHash::from_bytes(
            &serde_json::to_vec(self).map_err(|e| e.to_string())?,
        ))
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RecipeEntry {
    pub id: RecipeId,
    pub revision: SourceHash,
    pub definition: RecipeDefinition,
}

/// Exact reusable content; this document contains no personal stock or machine paths.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaterialCollection {
    pub entries: Vec<FilamentEntry>,
    pub revisions: Vec<FilamentRevision>,
    pub recipes: Vec<RecipeEntry>,
}
impl MaterialCollection {
    pub fn resolve(&self, reference: &FilamentRef) -> Option<&FilamentDefinition> {
        self.revisions
            .iter()
            .find(|v| &v.reference == reference)
            .map(|v| &v.definition)
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.entries.len() > MAX_FILAMENTS
            || self.revisions.len() > MAX_FILAMENT_REVISIONS
            || self.recipes.len() > MAX_RECIPES
        {
            return Err("material collection count limit".into());
        }
        let mut revisions = BTreeSet::new();
        for revision in &self.revisions {
            revision.validate()?;
            if !revisions.insert(&revision.reference) {
                return Err("duplicate filament revision".into());
            }
        }
        let mut entries = BTreeSet::new();
        for entry in &self.entries {
            entry.current.validate()?;
            if !entries.insert(&entry.current.id) || self.resolve(&entry.current).is_none() {
                return Err("duplicate or unresolved active filament identity".into());
            }
            if let Some(origin) = &entry.origin {
                validate_library_uuid(&origin.library_id.0)?;
                origin.filament.validate()?;
            }
        }
        for revision in &self.revisions {
            if !entries.contains(&revision.reference.id) {
                return Err("filament revision lacks its owning entry".into());
            }
        }
        let mut recipe_ids = BTreeSet::new();
        for recipe in &self.recipes {
            validate_library_uuid(&recipe.id.0)?;
            recipe.definition.validate()?;
            if !recipe_ids.insert(&recipe.id)
                || recipe.revision != recipe.definition.content_hash()?
            {
                return Err("duplicate recipe or wrong recipe content revision".into());
            }
            for ingredient in &recipe.definition.ingredients {
                if self.resolve(&ingredient.filament).is_none() {
                    return Err("recipe ingredient revision is not captured".into());
                }
            }
        }
        Ok(())
    }
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_MATERIAL_COLLECTION_BYTES {
            return Err("material collection byte limit".into());
        }
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_MATERIAL_COLLECTION_BYTES {
            return Err("material collection byte limit".into());
        }
        let value: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        value.validate()?;
        Ok(value)
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StockState {
    OnHand,
    Ordered,
    Empty,
    Archived,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuantitySource {
    Entered,
    Weighed,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StockQuantity {
    pub remaining_g: f64,
    pub source: QuantitySource,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SpoolRecord {
    pub id: SpoolId,
    pub filament: FilamentRef,
    pub name: String,
    pub state: StockState,
    #[serde(deserialize_with = "required_option")]
    pub quantity: Option<StockQuantity>,
    #[serde(deserialize_with = "required_option")]
    pub lot: Option<String>,
    #[serde(deserialize_with = "required_option")]
    pub location: Option<String>,
}
impl SpoolRecord {
    pub fn validate(&self) -> Result<(), String> {
        validate_library_uuid(&self.id.0)?;
        self.filament.validate()?;
        text(&self.name, 256)?;
        optional_text(&self.lot, 128)?;
        optional_text(&self.location, 256)?;
        if self.quantity.as_ref().is_some_and(|q| {
            !q.remaining_g.is_finite() || q.remaining_g < 0.0 || q.remaining_g > 1_000_000.0
        }) {
            return Err("stock quantity requires finite nonnegative grams up to 1000000".into());
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LibraryReceipt {
    pub request_id: String,
    pub request_hash: SourceHash,
    pub revision: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LibraryDocument {
    pub schema: u32,
    pub id: LibraryId,
    pub revision: u64,
    pub materials: MaterialCollection,
    pub spools: Vec<SpoolRecord>,
    pub receipts: Vec<LibraryReceipt>,
}
impl LibraryDocument {
    pub fn validate(&self) -> Result<(), String> {
        validate_library_uuid(&self.id.0)?;
        if self.schema != LIBRARY_SCHEMA
            || self.revision > i64::MAX as u64
            || self.spools.len() > MAX_SPOOLS
            || self.receipts.len() > MAX_LIBRARY_RECEIPTS
        {
            return Err("library schema, revision or count limit".into());
        }
        self.materials.validate()?;
        let mut spools = BTreeSet::new();
        for spool in &self.spools {
            spool.validate()?;
            if !spools.insert(&spool.id) || self.materials.resolve(&spool.filament).is_none() {
                return Err("duplicate spool or missing pinned filament revision".into());
            }
        }
        let mut requests = BTreeSet::new();
        for receipt in &self.receipts {
            validate_library_uuid(&receipt.request_id)?;
            if !requests.insert(&receipt.request_id)
                || receipt.revision == 0
                || receipt.revision > self.revision
            {
                return Err("duplicate or invalid library publication receipt".into());
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LibraryMutation {
    PutFilament {
        id: FilamentId,
        #[serde(deserialize_with = "required_option")]
        expected: Option<SourceHash>,
        definition: FilamentDefinition,
    },
    ArchiveFilament {
        filament: FilamentRef,
        archived: bool,
    },
    PutSpool {
        spool: SpoolRecord,
    },
    RemoveSpool {
        id: SpoolId,
    },
    PutRecipe {
        transfer: RecipeTransfer,
        #[serde(deserialize_with = "required_option")]
        expected: Option<SourceHash>,
    },
    RemoveRecipe {
        id: RecipeId,
        expected: SourceHash,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LibraryWrite {
    #[serde(deserialize_with = "required_option")]
    pub library_id: Option<LibraryId>,
    pub request_id: String,
    pub base_revision: u64,
    pub mutation: LibraryMutation,
}
impl LibraryWrite {
    pub fn validate(&self) -> Result<(), String> {
        validate_library_uuid(&self.request_id)?;
        if let Some(id) = &self.library_id {
            validate_library_uuid(&id.0)?;
        }
        if self.base_revision > i64::MAX as u64 {
            return Err("library revision exhausted".into());
        }
        match &self.mutation {
            LibraryMutation::PutFilament { id, definition, .. } => {
                validate_library_uuid(&id.0)?;
                definition.content_hash()?;
            }
            LibraryMutation::ArchiveFilament { filament, .. } => filament.validate()?,
            LibraryMutation::PutSpool { spool } => spool.validate()?,
            LibraryMutation::RemoveSpool { id } => validate_library_uuid(&id.0)?,
            LibraryMutation::PutRecipe { transfer, .. } => transfer.validate()?,
            LibraryMutation::RemoveRecipe { id, .. } => validate_library_uuid(&id.0)?,
        }
        Ok(())
    }
    pub fn digest(&self) -> Result<SourceHash, String> {
        self.validate()?;
        Ok(SourceHash::from_bytes(
            &serde_json::to_vec(self).map_err(|e| e.to_string())?,
        ))
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LibraryErrorCode {
    InvalidData,
    NotFound,
    StaleRevision,
    Locked,
    ReadOnly,
    ResourceLimit,
    Cancelled,
    Io,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq, thiserror::Error)]
#[error("{code:?}: {message}")]
#[serde(deny_unknown_fields)]
pub struct LibraryError {
    pub code: LibraryErrorCode,
    pub message: String,
}
impl LibraryError {
    pub fn new(code: LibraryErrorCode, message: impl Into<String>) -> Self {
        let mut message = message.into();
        if message.len() > 1024 {
            let mut end = 1024;
            while !message.is_char_boundary(end) {
                end -= 1;
            }
            message.truncate(end);
        }
        Self { code, message }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LibraryIdentity {
    pub id: LibraryId,
    pub revision: u64,
}
impl LibraryIdentity {
    pub fn validate(&self) -> Result<(), String> {
        validate_library_uuid(&self.id.0)?;
        if self.revision == 0 || self.revision > i64::MAX as u64 {
            return Err("invalid published library revision".into());
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LibraryStatus {
    pub location: crate::geometry::NativePath,
    #[serde(deserialize_with = "required_option")]
    pub identity: Option<LibraryIdentity>,
    pub filament_count: u32,
    pub spool_count: u32,
    pub recipe_count: u32,
}
impl LibraryStatus {
    pub fn validate(&self) -> Result<(), String> {
        self.location.validate().map_err(|e| e.to_string())?;
        if let Some(identity) = &self.identity {
            identity.validate()?;
        }
        if self.filament_count as usize > MAX_FILAMENTS
            || self.spool_count as usize > MAX_SPOOLS
            || self.recipe_count as usize > MAX_RECIPES
            || (self.identity.is_none()
                && (self.filament_count != 0 || self.spool_count != 0 || self.recipe_count != 0))
        {
            return Err("invalid library status counts".into());
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LibraryFilamentSummary {
    pub entry: FilamentEntry,
    pub name: String,
    #[serde(deserialize_with = "required_option")]
    pub material: Option<MaterialDefinition>,
    #[serde(deserialize_with = "required_option")]
    pub color_name: Option<String>,
    #[serde(deserialize_with = "required_option")]
    pub appearance: Option<DeclaredAppearance>,
    pub profile_count: u32,
    pub on_hand_spools: u32,
    pub earlier_spools: u32,
}
impl LibraryFilamentSummary {
    pub fn validate(&self) -> Result<(), String> {
        self.entry.current.validate()?;
        text(&self.name, 256)?;
        optional_text(&self.color_name, 128)?;
        if let Some(material) = &self.material {
            material.validate().map_err(|e| e.to_string())?;
        }
        if let Some(appearance) = &self.appearance {
            appearance.color.validate().map_err(|e| e.to_string())?;
            appearance.source.validate()?;
        }
        if self.profile_count as usize > MAX_PROFILES_PER_FILAMENT
            || self.on_hand_spools as usize > MAX_SPOOLS
            || self.earlier_spools as usize > MAX_SPOOLS
        {
            return Err("filament summary count limit".into());
        }
        if let Some(origin) = &self.entry.origin {
            validate_library_uuid(&origin.library_id.0)?;
            origin.filament.validate()?;
        }
        Ok(())
    }
}
pub const MAX_LIBRARY_PAGE: u32 = 20;
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LibraryFilamentPage {
    #[serde(deserialize_with = "required_option")]
    pub identity: Option<LibraryIdentity>,
    pub entries: Vec<LibraryFilamentSummary>,
    pub total_matches: u32,
    #[serde(deserialize_with = "required_option")]
    pub next_offset: Option<u32>,
}
impl LibraryFilamentPage {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(id) = &self.identity {
            id.validate()?;
        }
        if self.entries.len() > MAX_LIBRARY_PAGE as usize
            || self.total_matches as usize > MAX_FILAMENTS
            || self.entries.len() > self.total_matches as usize
            || self
                .next_offset
                .is_some_and(|n| n == 0 || n >= self.total_matches)
            || (self.identity.is_none()
                && (self.total_matches != 0
                    || !self.entries.is_empty()
                    || self.next_offset.is_some()))
        {
            return Err("invalid library page bounds".into());
        }
        let mut ids = BTreeSet::new();
        for entry in &self.entries {
            entry.validate()?;
            if !ids.insert(&entry.entry.current.id) {
                return Err("duplicate library page entry".into());
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LibrarySpoolPage {
    #[serde(deserialize_with = "required_option")]
    pub identity: Option<LibraryIdentity>,
    pub spools: Vec<SpoolRecord>,
    pub total_matches: u32,
    #[serde(deserialize_with = "required_option")]
    pub next_offset: Option<u32>,
}
impl LibrarySpoolPage {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(id) = &self.identity {
            id.validate()?;
        }
        if self.spools.len() > MAX_LIBRARY_PAGE as usize
            || self.total_matches as usize > MAX_SPOOLS
            || self.spools.len() > self.total_matches as usize
            || self
                .next_offset
                .is_some_and(|n| n == 0 || n >= self.total_matches)
            || (self.identity.is_none()
                && (self.total_matches != 0
                    || !self.spools.is_empty()
                    || self.next_offset.is_some()))
        {
            return Err("invalid spool page bounds".into());
        }
        let mut ids = BTreeSet::new();
        for spool in &self.spools {
            spool.validate()?;
            if !ids.insert(&spool.id) {
                return Err("duplicate spool page entry".into());
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LibraryPublication {
    Confirmed,
    Unconfirmed,
    Unknown,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LibraryWriteReceipt {
    pub identity: LibraryIdentity,
    pub request_id: String,
    pub publication: LibraryPublication,
    pub replayed: bool,
    #[serde(deserialize_with = "required_option")]
    pub message: Option<String>,
}
impl LibraryWriteReceipt {
    pub fn validate(&self) -> Result<(), String> {
        self.identity.validate()?;
        validate_library_uuid(&self.request_id)?;
        if self.message.as_ref().is_some_and(|s| s.len() > 1024)
            || (self.publication == LibraryPublication::Confirmed && self.message.is_some())
        {
            return Err("invalid library publication receipt".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LibraryFilamentQuery {
    #[serde(deserialize_with = "required_option")]
    pub expected: Option<LibraryIdentity>,
    pub query: String,
    pub offset: u32,
    pub page_size: u32,
    pub on_hand_only: bool,
    pub include_archived: bool,
}
impl LibraryFilamentQuery {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(expected) = &self.expected {
            expected.validate()?;
        }
        if self.query.len() > 128
            || self.query.chars().any(char::is_control)
            || self.page_size > MAX_LIBRARY_PAGE
            || self.offset as usize > MAX_FILAMENTS
            || (self.offset != 0 && self.expected.is_none())
        {
            return Err("invalid bounded filament query".into());
        }
        Ok(())
    }
    pub fn limit(&self) -> usize {
        if self.page_size == 0 {
            MAX_LIBRARY_PAGE as usize
        } else {
            self.page_size as usize
        }
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LibrarySpoolQuery {
    #[serde(deserialize_with = "required_option")]
    pub expected: Option<LibraryIdentity>,
    #[serde(deserialize_with = "required_option")]
    pub filament: Option<FilamentRef>,
    pub offset: u32,
    pub page_size: u32,
}
impl LibrarySpoolQuery {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(expected) = &self.expected {
            expected.validate()?;
        }
        if let Some(filament) = &self.filament {
            filament.validate()?;
        }
        if self.page_size > MAX_LIBRARY_PAGE
            || self.offset as usize > MAX_SPOOLS
            || (self.offset != 0 && self.expected.is_none())
        {
            return Err("invalid bounded spool query".into());
        }
        Ok(())
    }
    pub fn limit(&self) -> usize {
        if self.page_size == 0 {
            MAX_LIBRARY_PAGE as usize
        } else {
            self.page_size as usize
        }
    }
}

/// Small project root descriptor; complete reusable content lives in one exact PTAC object.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MaterialCollectionRecord {
    pub hash: SourceHash,
    pub byte_count: u64,
    pub filament_count: u32,
    pub recipe_count: u32,
}
impl MaterialCollectionRecord {
    pub fn validate(&self) -> Result<(), String> {
        if self.byte_count == 0
            || self.byte_count > MAX_MATERIAL_COLLECTION_BYTES as u64
            || self.filament_count > MAX_FILAMENTS as u32
            || self.recipe_count > MAX_RECIPES as u32
        {
            return Err("invalid project material collection descriptor".into());
        }
        Ok(())
    }
}
