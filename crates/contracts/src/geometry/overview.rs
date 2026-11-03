// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
//! Bounded source context for navigation; never constructed-material or packing evidence.
use super::*;
use crate::project::{MAX_PRINT_LAYOUTS, PrintLayoutId, ProjectId, ProjectRevision};

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PrintLayoutOverview {
    pub session_id: SessionId,
    pub scene_revision: SceneRevision,
    pub project_id: ProjectId,
    pub project_revision: ProjectRevision,
    pub layouts: Vec<LayoutOverview>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LayoutOverview {
    pub layout_id: PrintLayoutId,
    pub build_area_mm: Option<AabbMm>,
    pub captured_plate: bool,
    pub parts: Vec<LayoutPartBounds>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LayoutPartBounds {
    pub occurrence_id: OccurrenceId,
    pub source_bounds_mm: AabbMm,
}
impl PrintLayoutOverview {
    pub fn validate(&self) -> Result<(), GeometryError> {
        let mut layouts = std::collections::BTreeSet::new();
        let mut parts = std::collections::BTreeSet::new();
        if self.layouts.is_empty() || self.layouts.len() > MAX_PRINT_LAYOUTS {
            return Err(GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "invalid overview plate count",
            ));
        }
        for layout in &self.layouts {
            if !layouts.insert(&layout.layout_id) {
                return Err(GeometryError::new(
                    GeometryErrorCode::InvalidGeometry,
                    "duplicate overview plate",
                ));
            }
            if let Some(bounds) = layout.build_area_mm {
                bounds.validate()?;
                if (0..2).any(|a| bounds.min[a] >= bounds.max[a]) || bounds.min[2] != bounds.max[2]
                {
                    return Err(GeometryError::new(
                        GeometryErrorCode::InvalidGeometry,
                        "invalid overview build area",
                    ));
                }
            } else if layout.captured_plate {
                return Err(GeometryError::new(
                    GeometryErrorCode::InvalidGeometry,
                    "captured plate has no build area",
                ));
            }
            for part in &layout.parts {
                part.source_bounds_mm.validate()?;
                if !parts.insert(part.occurrence_id) {
                    return Err(GeometryError::new(
                        GeometryErrorCode::InvalidGeometry,
                        "duplicate overview part",
                    ));
                }
            }
        }
        if parts.len() > MAX_OCCURRENCES as usize {
            return Err(GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "overview exceeds project part limit",
            ));
        }
        Ok(())
    }
}
