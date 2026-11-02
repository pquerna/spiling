// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::required;
use crate::{catalog::*, geometry::SourceHash, library::FilamentDeclaration, rpc};
fn hash(s: String) -> Result<SourceHash, String> {
    SourceHash::parse(s).map_err(|e| e.to_string())
}
impl TryFrom<rpc::FilamentDeclaration> for FilamentDeclaration {
    type Error = String;
    fn try_from(v: rpc::FilamentDeclaration) -> Result<Self, String> {
        let d = Self {
            brand: v.brand,
            material_label: v.material_label,
            diameter_mm: v.diameter_mm,
            hueforge_td_mm: v.hueforge_td_mm,
        };
        d.validate()?;
        Ok(d)
    }
}
impl TryFrom<FilamentDeclaration> for rpc::FilamentDeclaration {
    type Error = String;
    fn try_from(v: FilamentDeclaration) -> Result<Self, String> {
        v.validate()?;
        Ok(Self {
            brand: v.brand,
            material_label: v.material_label,
            diameter_mm: v.diameter_mm,
            hueforge_td_mm: v.hueforge_td_mm,
        })
    }
}
impl TryFrom<rpc::CatalogRecord> for CatalogRecord {
    type Error = String;
    fn try_from(v: rpc::CatalogRecord) -> Result<Self, String> {
        let o = required(v.origin, "catalog origin")?;
        let item = match required(v.item, "catalog item")? {
            rpc::catalog_record::Item::Filament(f) => CatalogItem::Filament {
                name: f.name,
                declaration: required(f.declaration, "filament declaration")?.try_into()?,
                srgb: f
                    .srgb
                    .try_into()
                    .map_err(|_| "RGB must have three channels")?,
            },
            rpc::catalog_record::Item::Printer(p) => CatalogItem::Printer {
                manufacturer: p.manufacturer,
                model: p.model,
                aliases: p.aliases,
                volume_mm: p.volume_mm.map(|v| [v.x, v.y, v.z]),
                source_url: p.source_url,
            },
        };
        let record = Self {
            origin: CatalogOrigin {
                provider: o.provider,
                id: o.id,
                revision: o.revision,
                snapshot: hash(o.snapshot)?,
                content: hash(o.content)?,
                url: o.url,
            },
            item,
        };
        record.validate()?;
        Ok(record)
    }
}
impl TryFrom<CatalogRecord> for rpc::CatalogRecord {
    type Error = String;
    fn try_from(v: CatalogRecord) -> Result<Self, String> {
        v.validate()?;
        let o = v.origin;
        Ok(Self {
            origin: Some(rpc::CatalogOrigin {
                provider: o.provider,
                id: o.id,
                revision: o.revision,
                snapshot: o.snapshot.as_str().into(),
                content: o.content.as_str().into(),
                url: o.url,
            }),
            item: Some(match v.item {
                CatalogItem::Filament {
                    name,
                    declaration,
                    srgb,
                } => rpc::catalog_record::Item::Filament(rpc::CatalogFilament {
                    name,
                    declaration: Some(declaration.try_into()?),
                    srgb: srgb.to_vec(),
                }),
                CatalogItem::Printer {
                    manufacturer,
                    model,
                    aliases,
                    volume_mm,
                    source_url,
                } => rpc::catalog_record::Item::Printer(rpc::CatalogPrinter {
                    manufacturer,
                    model,
                    aliases,
                    volume_mm: volume_mm.map(|v| rpc::Vec3 {
                        x: v[0],
                        y: v[1],
                        z: v[2],
                    }),
                    source_url,
                }),
            }),
        })
    }
}
impl TryFrom<rpc::CatalogQuery> for CatalogQuery {
    type Error = String;
    fn try_from(v: rpc::CatalogQuery) -> Result<Self, String> {
        let q = Self {
            kind: match rpc::CatalogKind::try_from(v.kind).map_err(|_| "unknown catalog kind")? {
                rpc::CatalogKind::CatalogFilament => CatalogKind::Filament,
                rpc::CatalogKind::CatalogPrinter => CatalogKind::Printer,
                _ => return Err("catalog kind is required".into()),
            },
            query: v.query,
            offset: v.offset,
            snapshot: v.snapshot.map(hash).transpose()?,
        };
        q.validate()?;
        Ok(q)
    }
}
impl TryFrom<CatalogQuery> for rpc::CatalogQuery {
    type Error = String;
    fn try_from(v: CatalogQuery) -> Result<Self, String> {
        v.validate()?;
        Ok(Self {
            kind: match v.kind {
                CatalogKind::Filament => rpc::CatalogKind::CatalogFilament as i32,
                CatalogKind::Printer => rpc::CatalogKind::CatalogPrinter as i32,
            },
            query: v.query,
            offset: v.offset,
            snapshot: v.snapshot.map(|s| s.as_str().into()),
        })
    }
}
impl TryFrom<rpc::CatalogPage> for CatalogPage {
    type Error = String;
    fn try_from(v: rpc::CatalogPage) -> Result<Self, String> {
        if v.entries.len() > CATALOG_PAGE_SIZE {
            return Err("catalog page exceeds limit".into());
        }
        let p = Self {
            snapshot: hash(v.snapshot)?,
            query: required(v.query, "catalog query")?.try_into()?,
            entries: v
                .entries
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            total: v.total,
            next: v.next,
        };
        p.validate()?;
        Ok(p)
    }
}
impl TryFrom<CatalogPage> for rpc::CatalogPage {
    type Error = String;
    fn try_from(v: CatalogPage) -> Result<Self, String> {
        v.validate()?;
        Ok(Self {
            snapshot: v.snapshot.as_str().into(),
            query: Some(v.query.try_into()?),
            entries: v
                .entries
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            total: v.total,
            next: v.next,
        })
    }
}

impl TryFrom<rpc::CatalogKey> for CatalogKey {
    type Error = String;
    fn try_from(v: rpc::CatalogKey) -> Result<Self, String> {
        let kind = match rpc::CatalogKind::try_from(v.kind).map_err(|_| "invalid catalog kind")? {
            rpc::CatalogKind::CatalogFilament => CatalogKind::Filament,
            rpc::CatalogKind::CatalogPrinter => CatalogKind::Printer,
            _ => return Err("missing catalog kind".into()),
        };
        let key = Self {
            kind,
            provider: v.provider,
            id: v.id,
        };
        key.validate()?;
        Ok(key)
    }
}
impl TryFrom<CatalogKey> for rpc::CatalogKey {
    type Error = String;
    fn try_from(v: CatalogKey) -> Result<Self, String> {
        v.validate()?;
        Ok(Self {
            kind: match v.kind {
                CatalogKind::Filament => rpc::CatalogKind::CatalogFilament as i32,
                CatalogKind::Printer => rpc::CatalogKind::CatalogPrinter as i32,
            },
            provider: v.provider,
            id: v.id,
        })
    }
}
