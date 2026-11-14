// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
//! Offline reference declarations. Neither catalog presence nor inventory grants process authority.
use crate::{geometry::SourceHash, library::FilamentDeclaration};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;

pub const CATALOG_PAGE_SIZE: usize = 20;
pub const MAX_CATALOG_RECORD_BYTES: usize = 2048;
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CatalogKind {
    Filament,
    Printer,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CatalogKey {
    pub kind: CatalogKind,
    pub provider: String,
    pub id: String,
}
impl CatalogKey {
    pub fn validate(&self) -> Result<(), String> {
        if !text(&self.provider, 64) || !text(&self.id, 256) {
            return Err("invalid catalog key".into());
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CatalogOrigin {
    pub provider: String,
    pub id: String,
    pub revision: String,
    pub snapshot: SourceHash,
    pub content: SourceHash,
    pub url: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CatalogItem {
    Filament {
        name: String,
        declaration: FilamentDeclaration,
        srgb: [u8; 3],
    },
    Printer {
        manufacturer: String,
        model: String,
        aliases: Vec<String>,
        #[serde(deserialize_with = "required_option")]
        volume_mm: Option<[f64; 3]>,
        source_url: String,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CatalogRecord {
    pub origin: CatalogOrigin,
    pub item: CatalogItem,
}
fn text(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.trim() == value
        && !value.chars().any(char::is_control)
}
impl CatalogItem {
    pub fn kind(&self) -> CatalogKind {
        match self {
            Self::Filament { .. } => CatalogKind::Filament,
            Self::Printer { .. } => CatalogKind::Printer,
        }
    }
    pub fn content_hash(&self) -> Result<SourceHash, String> {
        SourceHash::parse(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(self).map_err(|e| e.to_string())?)
        ))
        .map_err(|e| e.to_string())
    }
}
impl CatalogRecord {
    pub fn key(&self) -> CatalogKey {
        CatalogKey {
            kind: self.item.kind(),
            provider: self.origin.provider.clone(),
            id: self.origin.id.clone(),
        }
    }
    /// An explicit proposal. Saving still requires normal library/project CAS admission.
    pub fn filament_update(
        &self,
        current: &crate::library::FilamentDefinition,
    ) -> Result<crate::library::FilamentDefinition, String> {
        current.validate()?;
        if current
            .catalog_origin
            .as_ref()
            .is_none_or(|r| r.key() != self.key())
        {
            return Err("catalog update belongs to another product".into());
        }
        let original = current
            .catalog_origin
            .as_ref()
            .ok_or("missing captured origin")?
            .filament_definition()?;
        let source = self.filament_definition()?;
        let mut proposed = current.clone();
        fn update<T: PartialEq>(value: &mut T, baseline: &T, next: T) {
            if value == baseline {
                *value = next;
            }
        }
        update(&mut proposed.name, &original.name, source.name);
        update(
            &mut proposed.color_name,
            &original.color_name,
            source.color_name,
        );
        update(
            &mut proposed.appearance,
            &original.appearance,
            source.appearance,
        );
        update(&mut proposed.source, &original.source, source.source);
        update(
            &mut proposed.declaration.brand,
            &original.declaration.brand,
            source.declaration.brand,
        );
        update(
            &mut proposed.declaration.material_label,
            &original.declaration.material_label,
            source.declaration.material_label,
        );
        update(
            &mut proposed.declaration.diameter_mm,
            &original.declaration.diameter_mm,
            source.declaration.diameter_mm,
        );
        update(
            &mut proposed.declaration.hueforge_td_mm,
            &original.declaration.hueforge_td_mm,
            source.declaration.hueforge_td_mm,
        );
        proposed.catalog_origin = Some(Box::new(self.clone()));
        proposed.validate()?;
        Ok(proposed)
    }

    pub fn filament_definition(&self) -> Result<crate::library::FilamentDefinition, String> {
        self.validate()?;
        let CatalogItem::Filament {
            name,
            declaration,
            srgb,
        } = &self.item
        else {
            return Err("select a filament reference".into());
        };
        let source = crate::library::DefinitionSource::Imported {
            name: self.origin.provider.clone(),
            sha256: self.origin.content.clone(),
        };
        // A recognised polymer family and a declared diameter make the
        // catalog filament usable: it carries a material identity and the
        // family's generic profiles. Nothing is assumed when either is absent.
        let family = declaration
            .material_label
            .as_deref()
            .and_then(crate::manufacturing::generic_polymer_family);
        let material = family
            .zip(declaration.diameter_mm)
            .map(
                |(family, diameter)| crate::manufacturing::MaterialDefinition {
                    id: format!("catalog.{}", self.origin.id),
                    revision: self.origin.revision.clone(),
                    manufacturer: declaration
                        .brand
                        .clone()
                        .unwrap_or_else(|| self.origin.provider.clone()),
                    product: name.clone(),
                    polymer_family: family.into(),
                    filler: None,
                    filament_diameter_mm: diameter,
                    requires_abrasion_resistant_nozzle: false,
                },
            );
        let profiles = match &material {
            Some(material) => crate::manufacturing::generic_profiles_for(material)?,
            None => Vec::new(),
        };
        let definition = crate::library::FilamentDefinition {
            name: format!("{} {}", declaration.brand.as_deref().unwrap_or(""), name)
                .trim()
                .into(),
            declaration: declaration.clone(),
            catalog_origin: Some(Box::new(self.clone())),
            material,
            color_name: Some(name.clone()),
            appearance: Some(crate::library::DeclaredAppearance {
                color: crate::appearance::AppearanceColor::Srgb {
                    channels: srgb.map(|v| f64::from(v) / 255.0),
                },
                source: source.clone(),
            }),
            profiles,
            source,
        };
        definition.validate()?;
        Ok(definition)
    }

    pub fn validate(&self) -> Result<(), String> {
        if !text(&self.origin.provider, 64)
            || !text(&self.origin.id, 256)
            || !text(&self.origin.revision, 128)
            || !text(&self.origin.url, 1024)
            || !self.origin.url.starts_with("https://")
        {
            return Err("invalid catalog origin".into());
        }
        match &self.item {
            CatalogItem::Filament {
                name, declaration, ..
            } => {
                if !text(name, 256) {
                    return Err("invalid catalog filament name".into());
                }
                declaration.validate()?;
            }
            CatalogItem::Printer {
                manufacturer,
                model,
                aliases,
                volume_mm,
                source_url,
            } => {
                if !text(manufacturer, 256)
                    || !text(model, 256)
                    || aliases.len() > 16
                    || aliases.iter().any(|a| !text(a, 256))
                    || !text(source_url, 1024)
                    || !source_url.starts_with("https://")
                    || volume_mm.is_some_and(|v| v.iter().any(|x| !x.is_finite() || *x <= 0.0))
                {
                    return Err("invalid printer reference declaration".into());
                }
            }
        }
        if self.item.content_hash()? != self.origin.content
            || serde_json::to_vec(self).map_err(|e| e.to_string())?.len() > MAX_CATALOG_RECORD_BYTES
        {
            return Err("catalog record content hash or size differs".into());
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CatalogQuery {
    pub kind: CatalogKind,
    pub query: String,
    pub offset: u32,
    #[serde(deserialize_with = "required_option")]
    pub snapshot: Option<SourceHash>,
}
impl CatalogQuery {
    pub fn validate(&self) -> Result<(), String> {
        if self.query.len() > 256
            || self.query.chars().any(char::is_control)
            || self.offset > 65536
            || (self.offset > 0 && self.snapshot.is_none())
        {
            return Err("invalid catalog query or uncaptured continuation".into());
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CatalogPage {
    pub snapshot: SourceHash,
    pub query: CatalogQuery,
    pub entries: Vec<CatalogRecord>,
    pub total: u32,
    #[serde(deserialize_with = "required_option")]
    pub next: Option<u32>,
}
impl CatalogPage {
    pub fn validate(&self) -> Result<(), String> {
        self.query.validate()?;
        let end = self
            .query
            .offset
            .checked_add(self.entries.len() as u32)
            .ok_or("catalog page overflow")?;
        if self.entries.len() > CATALOG_PAGE_SIZE
            || end > self.total
            || self.next != (end < self.total).then_some(end)
            || (end < self.total && self.entries.is_empty())
            || self
                .query
                .snapshot
                .as_ref()
                .is_some_and(|s| s != &self.snapshot)
        {
            return Err("catalog page/query mismatch".into());
        }
        let mut ids = std::collections::BTreeSet::new();
        for row in &self.entries {
            row.validate()?;
            if row.item.kind() != self.query.kind
                || row.origin.snapshot != self.snapshot
                || !ids.insert((&row.origin.provider, &row.origin.id))
            {
                return Err("catalog page identity differs".into());
            }
        }
        Ok(())
    }
}

fn required_option<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    Option::deserialize(d)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn record() -> CatalogRecord {
        let item = CatalogItem::Filament {
            name: "Ocean".into(),
            declaration: FilamentDeclaration {
                brand: Some("Example".into()),
                material_label: Some("PLA".into()),
                diameter_mm: None,
                hueforge_td_mm: Some(2.0),
            },
            srgb: [0, 128, 180],
        };
        CatalogRecord {
            origin: CatalogOrigin {
                provider: "example".into(),
                id: "ocean-pla".into(),
                revision: "source-1".into(),
                snapshot: SourceHash::from_bytes(b"snapshot"),
                content: item.content_hash().unwrap(),
                url: "https://example.org/catalog".into(),
            },
            item,
        }
    }
    #[test]
    fn declarations_preserve_unknowns_and_refuse_changed_provenance() {
        let mut row = record();
        row.validate().unwrap();
        let d = row.filament_definition().unwrap();
        assert_eq!(d.declaration.diameter_mm, None);
        assert!(d.material.is_none() && d.profiles.is_empty());
        assert_eq!(d.catalog_origin, Some(Box::new(row.clone())));
        if let CatalogItem::Filament { srgb, .. } = &mut row.item {
            *srgb = [0, 0, 0];
        }
        assert!(row.validate().is_err());
        let mut json = serde_json::to_value(&d).unwrap();
        json["declaration"]
            .as_object_mut()
            .unwrap()
            .remove("diameter_mm");
        assert!(serde_json::from_value::<crate::library::FilamentDefinition>(json).is_err());
    }
    #[test]
    fn a_declared_polymer_family_derives_generic_profiles() {
        let mut row = record();
        if let CatalogItem::Filament { declaration, .. } = &mut row.item {
            declaration.diameter_mm = Some(1.75);
            declaration.material_label = Some("Silk PLA+".into());
        }
        row.origin.content = row.item.content_hash().unwrap();
        let d = row.filament_definition().unwrap();
        let material = d.material.as_ref().unwrap();
        assert_eq!(material.polymer_family, "PLA");
        assert_eq!(material.id, "catalog.ocean-pla");
        assert_eq!(d.profiles.len(), 5, "one generic profile per nozzle class");
        assert!(d.profiles.iter().all(|p| &p.material == material));
        assert!(d.profiles.iter().any(|p| p.nozzle_diameter_mm == 0.4));
        if let CatalogItem::Filament { declaration, .. } = &mut row.item {
            declaration.material_label = Some("Nylon".into());
        }
        row.origin.content = row.item.content_hash().unwrap();
        let d = row.filament_definition().unwrap();
        assert!(d.material.is_none() && d.profiles.is_empty());
    }
    #[test]
    fn pages_refuse_duplicate_source_identities_and_stale_continuations() {
        let row = record();
        let mut p = CatalogPage {
            snapshot: row.origin.snapshot.clone(),
            query: CatalogQuery {
                kind: CatalogKind::Filament,
                query: String::new(),
                offset: 0,
                snapshot: None,
            },
            entries: vec![row.clone()],
            total: 2,
            next: Some(1),
        };
        p.validate().unwrap();
        p.entries.push(row);
        p.next = None;
        assert!(p.validate().is_err());
        p.entries.pop();
        p.next = Some(1);
        p.query.snapshot = Some(SourceHash::from_bytes(b"another snapshot"));
        assert!(p.validate().is_err());
    }
    #[test]
    fn reviewed_update_preserves_local_values_and_rejects_another_product() {
        let before = record();
        let mut local = before.filament_definition().unwrap();
        local.name = "My workshop blue".into();
        local.declaration.diameter_mm = Some(1.75);
        local.source = crate::library::DefinitionSource::Operator;
        let mut next = before.clone();
        if let CatalogItem::Filament {
            declaration, srgb, ..
        } = &mut next.item
        {
            declaration.hueforge_td_mm = Some(3.0);
            *srgb = [20, 40, 60];
        }
        next.origin.content = next.item.content_hash().unwrap();
        next.origin.revision = "source-2".into();
        let proposed = next.filament_update(&local).unwrap();
        assert_eq!(proposed.name, local.name);
        assert_eq!(proposed.declaration.diameter_mm, Some(1.75));
        assert_eq!(proposed.declaration.hueforge_td_mm, Some(3.0));
        assert_eq!(
            proposed.appearance,
            next.filament_definition().unwrap().appearance
        );
        assert_eq!(proposed.source, local.source);
        assert_eq!(proposed.material, local.material);
        assert_eq!(proposed.profiles, local.profiles);
        assert_eq!(proposed.catalog_origin, Some(Box::new(next.clone())));
        next.origin.provider = "another-provider".into();
        assert!(next.filament_update(&local).is_err());
        next.origin.provider = before.origin.provider;
        next.origin.id = "another-product".into();
        assert!(next.filament_update(&local).is_err());
    }
}
