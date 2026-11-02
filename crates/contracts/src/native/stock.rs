// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::{request_id, required, session_parent};
use crate::{
    geometry::{SessionId, SourceHash},
    project::{ProjectCommand, ProjectRevision},
    rpc,
    stock::*,
};
impl TryFrom<rpc::StockQuery> for StockQuery {
    type Error = String;
    fn try_from(v: rpc::StockQuery) -> Result<Self, String> {
        let scope = match rpc::StockScope::try_from(v.scope).map_err(|_| "invalid stock scope")? {
            rpc::StockScope::StockProject => StockScope::Project,
            rpc::StockScope::StockMyLibrary => StockScope::MyLibrary,
            rpc::StockScope::StockBoth => StockScope::Both,
            _ => return Err("missing stock scope".into()),
        };
        let query = Self {
            scope,
            on_hand_only: v.on_hand_only,
            target: {
                let target = required(v.target, "stock target")?;
                crate::optical::OpticalTarget {
                    color: required(target.color, "stock target color")?.try_into()?,
                    delta_e00_tolerance: target.delta_e00_tolerance,
                    lightness_tolerance: target.lightness_tolerance,
                }
            },
            patch: required(v.patch, "stock patch")?.try_into()?,
            slot_height_mm: v.slot_height_mm,
            max_evaluations: v.max_evaluations,
        };
        query.validate()?;
        Ok(query)
    }
}
impl TryFrom<StockQuery> for rpc::StockQuery {
    type Error = String;
    fn try_from(v: StockQuery) -> Result<Self, String> {
        v.validate()?;
        Ok(Self {
            scope: match v.scope {
                StockScope::Project => rpc::StockScope::StockProject,
                StockScope::MyLibrary => rpc::StockScope::StockMyLibrary,
                StockScope::Both => rpc::StockScope::StockBoth,
            } as i32,
            on_hand_only: v.on_hand_only,
            target: Some(rpc::OpticalTarget {
                color: Some(v.target.color.try_into()?),
                delta_e00_tolerance: v.target.delta_e00_tolerance,
                lightness_tolerance: v.target.lightness_tolerance,
            }),
            patch: Some(v.patch.try_into()?),
            slot_height_mm: v.slot_height_mm,
            max_evaluations: v.max_evaluations,
        })
    }
}
impl TryFrom<rpc::MatchStockRequest> for ProjectCommand {
    type Error = String;
    fn try_from(v: rpc::MatchStockRequest) -> Result<Self, String> {
        request_id(&v.request_id)?;
        let session_id: SessionId = session_parent(&v.parent)?;
        Ok(Self::MatchStock {
            session_id,
            base_revision: ProjectRevision(v.base_revision),
            query: required(v.query, "stock query")?.try_into()?,
        })
    }
}
impl TryFrom<rpc::StockMatchRecord> for StockMatchRecord {
    type Error = String;
    fn try_from(v: rpc::StockMatchRecord) -> Result<Self, String> {
        let r = Self {
            hash: SourceHash::parse(v.hash).map_err(|e| e.to_string())?,
            byte_count: v.byte_count,
            input_hash: SourceHash::parse(v.input_hash).map_err(|e| e.to_string())?,
            producer: v.producer,
            complete: v.complete,
        };
        r.validate()?;
        Ok(r)
    }
}
impl TryFrom<StockMatchRecord> for rpc::StockMatchRecord {
    type Error = String;
    fn try_from(v: StockMatchRecord) -> Result<Self, String> {
        v.validate()?;
        Ok(Self {
            hash: v.hash.as_str().into(),
            byte_count: v.byte_count,
            input_hash: v.input_hash.as_str().into(),
            producer: v.producer,
            complete: v.complete,
        })
    }
}
pub fn validate_report_resource(
    record: &StockMatchRecord,
    resource: &crate::ArtifactView,
) -> Result<(), String> {
    record.validate()?;
    let size = super::validate_artifact_view(resource, MAX_STOCK_REPORT_BYTES as u64)?;
    if record.hash != SourceHash::parse(resource.sha256.clone()).map_err(|e| e.to_string())?
        || record.byte_count != size
        || resource.name.strip_prefix("artifacts/") != Some(record.hash.as_str())
        || resource.media_type != STOCK_REPORT_MIME
    {
        return Err("stock report resource differs".into());
    }
    Ok(())
}
