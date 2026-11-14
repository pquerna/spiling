// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
//! Paged source context for navigation; never constructed-material or packing evidence.
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
    pub definitions: Vec<DefinitionRecord>,
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
    pub definition_id: DefinitionId,
    pub pose: RigidPoseMm,
}
/// First reply of a paged overview: plate context and record counts. The
/// definitions and parts follow in [`LayoutOverviewPageKind`] pages read at the
/// same scene/project revisions, so no control frame carries the whole project.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LayoutOverviewSummary {
    pub session_id: SessionId,
    pub scene_revision: SceneRevision,
    pub project_id: ProjectId,
    pub project_revision: ProjectRevision,
    pub layouts: Vec<LayoutOverviewHeader>,
    pub definition_count: u32,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LayoutOverviewHeader {
    pub layout_id: PrintLayoutId,
    pub build_area_mm: Option<AabbMm>,
    pub captured_plate: bool,
    pub part_count: u32,
}
/// One part row of the flattened, layout-ordered part pages.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LayoutOverviewPart {
    /// Index into [`LayoutOverviewSummary::layouts`].
    pub layout_index: u32,
    pub part: LayoutPartBounds,
}
fn validate_plate(
    build_area_mm: Option<AabbMm>,
    captured_plate: bool,
) -> Result<(), GeometryError> {
    if let Some(bounds) = build_area_mm {
        bounds.validate()?;
        if (0..2).any(|a| bounds.min[a] >= bounds.max[a]) || bounds.min[2] != bounds.max[2] {
            return Err(invalid("invalid overview build area"));
        }
    } else if captured_plate {
        return Err(invalid("captured plate has no build area"));
    }
    Ok(())
}
fn validate_layout_count(count: usize) -> Result<(), GeometryError> {
    if count == 0 || count > MAX_PRINT_LAYOUTS {
        return Err(GeometryError::new(
            GeometryErrorCode::ResourceLimit,
            "invalid overview plate count",
        ));
    }
    Ok(())
}
impl LayoutOverviewSummary {
    pub fn validate(&self) -> Result<(), GeometryError> {
        validate_layout_count(self.layouts.len())?;
        let mut layouts = std::collections::BTreeSet::new();
        let mut parts = 0u32;
        for layout in &self.layouts {
            if !layouts.insert(&layout.layout_id) {
                return Err(invalid("duplicate overview plate"));
            }
            validate_plate(layout.build_area_mm, layout.captured_plate)?;
            parts = parts
                .checked_add(layout.part_count)
                .ok_or_else(|| invalid("overview part count exceeds its u32 width"))?;
        }
        if self.definition_count > parts {
            return Err(invalid("unreferenced overview definition"));
        }
        Ok(())
    }
    pub fn part_count(&self) -> u32 {
        self.layouts.iter().map(|l| l.part_count).sum()
    }
}
impl LayoutOverviewPart {
    pub fn validate(&self) -> Result<(), GeometryError> {
        self.part.source_bounds_mm.validate()?;
        self.part.pose.validate()
    }
}

impl PrintLayoutOverview {
    /// Assemble every page read at the summary's revisions and check it whole.
    pub fn assemble(
        summary: LayoutOverviewSummary,
        definitions: Vec<DefinitionRecord>,
        parts: Vec<LayoutOverviewPart>,
    ) -> Result<Self, GeometryError> {
        summary.validate()?;
        if definitions.len() != summary.definition_count as usize
            || parts.len() != summary.part_count() as usize
        {
            return Err(invalid("overview pages do not match the summary counts"));
        }
        let mut layouts: Vec<LayoutOverview> = summary
            .layouts
            .iter()
            .map(|l| LayoutOverview {
                layout_id: l.layout_id.clone(),
                build_area_mm: l.build_area_mm,
                captured_plate: l.captured_plate,
                parts: Vec::with_capacity(l.part_count as usize),
            })
            .collect();
        let mut previous = 0;
        for row in parts {
            let index = row.layout_index as usize;
            if index < previous || index >= layouts.len() {
                return Err(invalid("overview part pages are not in plate order"));
            }
            previous = index;
            layouts[index].parts.push(row.part);
        }
        if layouts
            .iter()
            .zip(&summary.layouts)
            .any(|(a, b)| a.parts.len() != b.part_count as usize)
        {
            return Err(invalid("overview part pages do not match plate counts"));
        }
        let overview = Self {
            session_id: summary.session_id,
            scene_revision: summary.scene_revision,
            project_id: summary.project_id,
            project_revision: summary.project_revision,
            layouts,
            definitions,
        };
        overview.validate()?;
        Ok(overview)
    }

    /// Display identity excludes metadata-only project revisions.
    pub fn same_geometry(&self, other: &Self) -> bool {
        self.session_id == other.session_id
            && self.project_id == other.project_id
            && self.definitions == other.definitions
            && self.layouts == other.layouts
    }

    pub fn validate(&self) -> Result<(), GeometryError> {
        let mut layouts = std::collections::BTreeSet::new();
        let mut definitions = std::collections::BTreeSet::new();
        for definition in &self.definitions {
            definition.validate()?;
            if !definitions.insert(&definition.definition_id) {
                return Err(invalid("duplicate overview definition"));
            }
        }
        let mut used = std::collections::BTreeSet::new();
        let mut parts = std::collections::BTreeSet::new();
        validate_layout_count(self.layouts.len())?;
        for layout in &self.layouts {
            if !layouts.insert(&layout.layout_id) {
                return Err(invalid("duplicate overview plate"));
            }
            validate_plate(layout.build_area_mm, layout.captured_plate)?;
            for part in &layout.parts {
                part.source_bounds_mm.validate()?;
                part.pose.validate()?;
                if !definitions.contains(&part.definition_id) {
                    return Err(invalid("overview part references an absent definition"));
                }
                used.insert(&part.definition_id);
                if !parts.insert(part.occurrence_id) {
                    return Err(invalid("duplicate overview part"));
                }
            }
        }
        if used != definitions {
            return Err(invalid("unreferenced overview definition"));
        }
        if u32::try_from(parts.len()).is_err() || u32::try_from(self.definitions.len()).is_err() {
            return Err(GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "overview counts exceed their u32 width",
            ));
        }
        Ok(())
    }
}
