// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Revision-bound layout proposals; only Core can apply their occurrence edits.
use super::*;
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LayoutObjective {
    Arrange,
    Copies,
    Fill,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LayoutStyle {
    Balanced,
    Dense,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LayoutRequest {
    pub objective: LayoutObjective,
    pub template: Option<OccurrenceId>,
    /// Total wanted quantity of the template, including the original. Fill uses this as its ceiling.
    pub quantity: u32,
    pub use_additional_plates: bool,
    pub pinned: Vec<OccurrenceId>,
    pub spacing_mm: f64,
    pub style: LayoutStyle,
}
impl LayoutRequest {
    pub fn validate(&self) -> Result<(), GeometryError> {
        if (self.use_additional_plates && self.objective != LayoutObjective::Copies)
            || self.quantity == 0
            || self.quantity > MAX_OCCURRENCES
            || !self.spacing_mm.is_finite()
            || !(0.0..=100.0).contains(&self.spacing_mm)
            || self.pinned.len() > MAX_OCCURRENCES as usize
            || self.pinned.iter().copied().collect::<BTreeSet<_>>().len() != self.pinned.len()
            || (self.objective == LayoutObjective::Arrange) != self.template.is_none()
        {
            return Err(GeometryError::new(
                GeometryErrorCode::InvalidGeometry,
                "Invalid layout quantity, template, pins or spacing",
            ));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LayoutPlacement {
    pub source: OccurrenceId,
    pub copy: bool,
    pub pose: RigidPoseMm,
}
pub fn validate_layout_placements(placements: &[LayoutPlacement]) -> Result<(), GeometryError> {
    let mut moved = BTreeSet::new();
    if placements.is_empty() || placements.len() > MAX_OCCURRENCES as usize {
        return Err(GeometryError::new(
            GeometryErrorCode::ResourceLimit,
            "Layout edit exceeds the occurrence budget",
        ));
    }
    for p in placements {
        p.pose.validate()?;
        if !p.copy && !moved.insert(p.source) {
            return Err(GeometryError::new(
                GeometryErrorCode::InvalidGeometry,
                "Layout moves the same occurrence twice",
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LayoutLimit {
    RequestedQuantity,
    BuildArea,
    SearchBudget,
    ManufacturingBudget,
    PlateBudget,
    OccurrenceBudget,
}
/// One proposed new job. Its copies are relative to the selected original, never another trial.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AdditionalPlateProposal {
    pub placements: Vec<LayoutPlacement>,
    pub footprints_mm: Vec<AabbMm>,
    pub candidates_examined: u32,
    pub manufacturing_attempts: u32,
    pub verification: crate::manufacturing::VerificationReport,
}
impl AdditionalPlateProposal {
    pub fn validate(&self) -> Result<(), GeometryError> {
        validate_layout_placements(&self.placements)?;
        self.verification
            .validate()
            .map_err(|e| GeometryError::new(GeometryErrorCode::InvalidGeometry, e.message))?;
        if self.placements.iter().any(|p| !p.copy)
            || self.placements.len() != self.footprints_mm.len()
            || !self.verification.verified
            || self.candidates_examined > 2_000_000
            || !(1..=4).contains(&self.manufacturing_attempts)
        {
            return Err(GeometryError::new(
                GeometryErrorCode::InvalidGeometry,
                "Invalid additional plate proposal",
            ));
        }
        for b in &self.footprints_mm {
            b.validate()?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LayoutProposal {
    pub id: SourceHash,
    pub session_id: SessionId,
    pub project_id: crate::project::ProjectId,
    pub project_revision: crate::project::ProjectRevision,
    pub scene_revision: SceneRevision,
    pub request: LayoutRequest,
    pub placements: Vec<LayoutPlacement>,
    pub additional_plates: Vec<AdditionalPlateProposal>,
    pub footprints_mm: Vec<AabbMm>,
    pub requested_count: u32,
    pub placed_count: u32,
    /// Number of finite placement candidates examined. This never proves a global optimum.
    pub candidates_examined: u32,
    pub manufacturing_attempts: u32,
    pub limited_by: LayoutLimit,
    pub verification: crate::manufacturing::VerificationReport,
}
impl LayoutProposal {
    pub fn total_placed(&self) -> u32 {
        self.placed_count.saturating_add(
            self.additional_plates
                .iter()
                .map(|p| p.placements.len() as u32)
                .sum::<u32>(),
        )
    }
    pub fn copied_count(&self) -> usize {
        self.placements.iter().filter(|p| p.copy).count()
            + self
                .additional_plates
                .iter()
                .map(|p| p.placements.len())
                .sum::<usize>()
    }
    pub fn validate(&self) -> Result<(), GeometryError> {
        self.request.validate()?;
        if !self.placements.is_empty() {
            validate_layout_placements(&self.placements)?;
        }
        self.verification
            .validate()
            .map_err(|e| GeometryError::new(GeometryErrorCode::InvalidGeometry, e.message))?;
        if self.additional_plates.len() >= crate::project::MAX_PRINT_LAYOUTS
            || (!self.additional_plates.is_empty() && !self.request.use_additional_plates)
        {
            return Err(GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "Additional plate limit exceeded",
            ));
        }
        let mut candidates = self.candidates_examined as u64;
        for plate in &self.additional_plates {
            plate.validate()?;
            candidates += plate.candidates_examined as u64;
            if plate
                .placements
                .iter()
                .any(|p| Some(p.source) != self.request.template)
            {
                return Err(GeometryError::new(
                    GeometryErrorCode::InvalidGeometry,
                    "Additional plate uses a different template",
                ));
            }
        }
        let original_count =
            self.placements.iter().filter(|p| !p.copy).count() + self.request.pinned.len();
        let expected = if self.request.objective == LayoutObjective::Arrange {
            original_count
        } else {
            original_count + self.request.quantity as usize - 1
        };
        if self.placements.len() + self.request.pinned.len() != self.placed_count as usize
            || expected != self.requested_count as usize
            || self.placements.iter().any(|p| {
                (p.copy && Some(p.source) != self.request.template)
                    || (!p.copy && self.request.pinned.contains(&p.source))
            })
            || candidates > 2_000_000
            || !self.verification.verified
            || self.placed_count == 0
            || self.total_placed() > self.requested_count
            || self.requested_count > MAX_OCCURRENCES
            || self.footprints_mm.len() != self.placed_count as usize
            || self.candidates_examined > 2_000_000
            || !(1..=4).contains(&self.manufacturing_attempts)
        {
            return Err(GeometryError::new(
                GeometryErrorCode::InvalidGeometry,
                "Layout result exceeds its captured quantity or verification budget",
            ));
        }
        for b in &self.footprints_mm {
            b.validate()?;
        }
        Ok(())
    }
}
