// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
//! Immutable bundled declarations and bounded text search, independent of personal storage.
use serde::Deserialize;
use sha2::{Digest, Sha256};
use spiling_contracts::{catalog::*, geometry::SourceHash, library::FilamentDeclaration};
use std::sync::atomic::{AtomicBool, Ordering};
const HUEFORGE: &[u8] =
    include_bytes!("../../../data/material-catalogs/filament-library-hueforge.json");
const OPENPRINTTAG: &[u8] =
    include_bytes!("../../../data/material-catalogs/filament-library-openprinttag.json");
const PRINTERS: &[u8] = include_bytes!("../../../crates/contracts/data/printer-references.json");
const HARDWARE: &[u8] = include_bytes!("../../../crates/contracts/data/hardware-catalog.json");
const MANIFEST: &[u8] = include_bytes!("../../../data/material-catalogs/manifest.json");
#[derive(Deserialize)]
struct Manifest {
    revision: String,
    catalogs: Vec<ManifestCatalog>,
}
#[derive(Deserialize)]
struct ManifestCatalog {
    source: String,
    sha256: String,
    bytes: usize,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Upstream {
    version: u32,
    source: String,
    #[serde(rename = "sourceUrl")]
    source_url: String,
    #[serde(rename = "lastSynced")]
    _last_synced: String,
    #[serde(rename = "entryCount")]
    entry_count: usize,
    entries: Vec<UpstreamRow>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpstreamRow {
    id: String,
    brand: String,
    material: String,
    name: String,
    hex: String,
    td: Option<f64>,
    #[serde(rename = "searchText")]
    _search_text: String,
    #[serde(rename = "finish")]
    _finish: Option<String>,
}
pub(super) struct CatalogIndex {
    snapshot: SourceHash,
    rows: Vec<Indexed>,
}
struct Indexed {
    record: CatalogRecord,
    text: String,
    name: String,
    tokens: Vec<String>,
}
pub(super) fn normalize(s: &str) -> String {
    s.chars()
        .flat_map(char::to_lowercase)
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
impl CatalogIndex {
    pub fn bundled() -> Result<Self, String> {
        let manifest: Manifest = serde_json::from_slice(MANIFEST).map_err(|e| e.to_string())?;
        let snapshot = SourceHash::from_bytes(
            &serde_json::to_vec(&[
                SourceHash::from_bytes(MANIFEST),
                SourceHash::from_bytes(PRINTERS),
                SourceHash::from_bytes(HARDWARE),
            ])
            .map_err(|e| e.to_string())?,
        );
        let mut rows = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for (source, bytes) in [("hueforge", HUEFORGE), ("openprinttag", OPENPRINTTAG)] {
            let pin = manifest
                .catalogs
                .iter()
                .find(|c| c.source == source)
                .ok_or("missing bundled catalog provenance")?;
            if bytes.len() > 8 * 1024 * 1024
                || bytes.len() != pin.bytes
                || format!("{:x}", Sha256::digest(bytes)) != pin.sha256
            {
                return Err("bundled catalog bytes differ from provenance".into());
            }
            let data: Upstream = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
            if data.version != 1
                || data.source != source
                || data.entries.len() != data.entry_count
                || data.entry_count > 32768
            {
                return Err("bundled catalog schema/count differs".into());
            }
            for r in data.entries {
                if !seen.insert((source.to_owned(), r.id.clone())) {
                    return Err("duplicate bundled reference identity".into());
                }
                let rgb = spiling_contracts::color_naming::parse_srgb8_hex(&r.hex)?;
                let text = normalize(&format!("{} {} {} {}", r.brand, r.material, r.name, r.hex));
                let item = CatalogItem::Filament {
                    name: r.name.clone(),
                    declaration: FilamentDeclaration {
                        brand: Some(r.brand),
                        material_label: Some(r.material),
                        diameter_mm: None,
                        hueforge_td_mm: r.td,
                    },
                    srgb: rgb,
                };
                let record = CatalogRecord {
                    origin: CatalogOrigin {
                        provider: format!("prusa-{source}"),
                        id: r.id,
                        revision: manifest.revision.clone(),
                        snapshot: snapshot.clone(),
                        content: item.content_hash()?,
                        url: data.source_url.clone(),
                    },
                    item,
                };
                record.validate()?;
                rows.push(Indexed {
                    record,
                    name: normalize(&r.name),
                    tokens: text.split_whitespace().map(str::to_owned).collect(),
                    text,
                });
            }
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct PrinterRow {
            id: String,
            item: CatalogItem,
        }
        let mut printers: Vec<PrinterRow> =
            serde_json::from_slice(PRINTERS).map_err(|e| e.to_string())?;
        if PRINTERS.len() > 48 * 1024 || printers.len() > 128 {
            return Err("printer reference source exceeds budget".into());
        }
        let author_url = "https://github.com/pquerna/spiling/blob/main/crates/contracts/data/printer-references.ts";
        let revision = SourceHash::from_bytes(PRINTERS);
        for row in printers.drain(..) {
            if row.item.kind() != CatalogKind::Printer
                || !seen.insert(("spiling-printers".into(), row.id.clone()))
            {
                return Err("invalid printer reference identity".into());
            }
            rows.push(index_printer(
                row.id,
                row.item,
                "spiling-printers",
                revision.as_str(),
                author_url,
                &snapshot,
            )?);
        }
        let revision = SourceHash::from_bytes(HARDWARE);
        for model in spiling_contracts::hardware::builtin_catalog()?.models {
            let item=CatalogItem::Printer {manufacturer:model.vendor,model:model.model,aliases:vec![],volume_mm:model.declared_volume_mm.map(|b|std::array::from_fn(|a|b.max[a]-b.min[a])),source_url:"https://github.com/pquerna/spiling/blob/main/crates/contracts/data/hardware-catalog.ts".into()};
            rows.push(index_printer(model.id,item,"spiling-setups",revision.as_str(),"https://github.com/pquerna/spiling/blob/main/crates/contracts/data/hardware-catalog.ts",&snapshot)?);
        }
        Ok(Self { snapshot, rows })
    }
    pub fn get(&self, key: &CatalogKey) -> Result<CatalogRecord, String> {
        key.validate()?;
        self.rows.iter().find(|row| {
            row.record.item.kind() == key.kind
                && row.record.origin.provider == key.provider
                && row.record.origin.id == key.id
        }).map(|row| row.record.clone()).ok_or_else(|| {
            "This source record is absent from the bundled catalog; keep the captured declaration".into()
        })
    }
    pub fn search(&self, q: &CatalogQuery, cancel: &AtomicBool) -> Result<CatalogPage, String> {
        q.validate()?;
        if q.snapshot.as_ref().is_some_and(|s| s != &self.snapshot) {
            return Err("catalog changed; start a new search".into());
        }
        let normalized = normalize(&q.query);
        let needles: Vec<_> = normalized.split_whitespace().collect();
        let mut found = Vec::new();
        for (i, row) in self.rows.iter().enumerate() {
            if i % 128 == 0 && cancel.load(Ordering::Acquire) {
                return Err("catalog search cancelled".into());
            }
            if row.record.item.kind() != q.kind {
                continue;
            }
            let rank = if normalized.is_empty() || row.name == normalized {
                Some(0)
            } else if row.text.contains(&normalized) {
                Some(1)
            } else if needles
                .iter()
                .all(|n| row.tokens.iter().any(|t| t.starts_with(n)))
            {
                Some(2)
            } else if needles
                .iter()
                .all(|n| row.tokens.iter().any(|t| n.len() > 3 && one_edit(n, t)))
            {
                Some(3)
            } else {
                None
            };
            if let Some(rank) = rank {
                found.push((rank, i));
            }
        }
        found.sort_by(|(a, ai), (b, bi)| {
            a.cmp(b)
                .then_with(|| self.rows[*ai].text.cmp(&self.rows[*bi].text))
                .then_with(|| {
                    self.rows[*ai]
                        .record
                        .origin
                        .provider
                        .cmp(&self.rows[*bi].record.origin.provider)
                })
                .then_with(|| {
                    self.rows[*ai]
                        .record
                        .origin
                        .id
                        .cmp(&self.rows[*bi].record.origin.id)
                })
        });
        if q.offset as usize > found.len() {
            return Err("catalog offset outside captured search".into());
        }
        let total = found.len() as u32;
        let entries: Vec<_> = found
            .into_iter()
            .skip(q.offset as usize)
            .take(CATALOG_PAGE_SIZE)
            .map(|(_, i)| self.rows[i].record.clone())
            .collect();
        let end = q.offset + entries.len() as u32;
        let result = CatalogPage {
            snapshot: self.snapshot.clone(),
            query: q.clone(),
            entries,
            total,
            next: (end < total).then_some(end),
        };
        result.validate()?;
        Ok(result)
    }
}
fn index_printer(
    id: String,
    item: CatalogItem,
    provider: &str,
    revision: &str,
    url: &str,
    snapshot: &SourceHash,
) -> Result<Indexed, String> {
    let CatalogItem::Printer {
        manufacturer,
        model,
        aliases,
        ..
    } = &item
    else {
        return Err("expected printer declaration".into());
    };
    let text = normalize(&format!("{manufacturer} {model} {}", aliases.join(" ")));
    let name = normalize(model);
    let record = CatalogRecord {
        origin: CatalogOrigin {
            provider: provider.into(),
            id,
            revision: revision.into(),
            snapshot: snapshot.clone(),
            content: item.content_hash()?,
            url: url.into(),
        },
        item,
    };
    record.validate()?;
    Ok(Indexed {
        record,
        tokens: text.split_whitespace().map(str::to_owned).collect(),
        text,
        name,
    })
}
fn one_edit(a: &str, b: &str) -> bool {
    let al = a.chars().count();
    let bl = b.chars().count();
    if al.abs_diff(bl) > 1 {
        return false;
    }
    let mut a = a.chars().peekable();
    let mut b = b.chars().peekable();
    let mut edits = 0;
    while let (Some(x), Some(y)) = (a.peek(), b.peek()) {
        if x == y {
            a.next();
            b.next();
        } else {
            edits += 1;
            if edits > 1 {
                return false;
            }
            match al.cmp(&bl) {
                std::cmp::Ordering::Less => {
                    b.next();
                }
                std::cmp::Ordering::Greater => {
                    a.next();
                }
                std::cmp::Ordering::Equal => {
                    a.next();
                    b.next();
                }
            }
        }
    }
    edits + usize::from(a.peek().is_some() || b.peek().is_some()) <= 1
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_snapshots_keep_sources_td_unknowns_and_captured_pages() {
        let i = CatalogIndex::bundled().unwrap();
        let cancel = AtomicBool::new(false);
        let mut q = CatalogQuery {
            kind: CatalogKind::Filament,
            query: "prusament pla".into(),
            offset: 0,
            snapshot: None,
        };
        let p = i.search(&q, &cancel).unwrap();
        assert!(p.total > 20);
        assert_eq!(p.entries.len(), 20);
        for r in &p.entries {
            let CatalogItem::Filament { declaration, .. } = &r.item else {
                panic!()
            };
            assert!(
                declaration
                    .brand
                    .as_ref()
                    .unwrap()
                    .to_lowercase()
                    .contains("prusament")
            );
            assert!(declaration.diameter_mm.is_none());
            if r.origin.provider == "prusa-openprinttag" {
                assert!(declaration.hueforge_td_mm.is_none());
            }
        }
        q.offset = p.next.unwrap();
        assert!(i.search(&q, &cancel).is_err());
        q.snapshot = Some(p.snapshot);
        let next = i.search(&q, &cancel).unwrap();
        assert!(
            p.entries
                .iter()
                .all(|a| !next.entries.iter().any(|b| a.origin == b.origin))
        );
        cancel.store(true, Ordering::Release);
        assert!(i.search(&q, &cancel).is_err());
    }
}
