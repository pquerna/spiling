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
    pub pinned: Vec<OccurrenceId>,
    pub spacing_mm: f64,
    pub style: LayoutStyle,
}
impl LayoutRequest {
    pub fn validate(&self) -> Result<(), GeometryError> {
        if self.quantity == 0
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
    pub fn validate(&self) -> Result<(), GeometryError> {
        self.request.validate()?;
        if !self.placements.is_empty() {
            validate_layout_placements(&self.placements)?;
        }
        self.verification
            .validate()
            .map_err(|e| GeometryError::new(GeometryErrorCode::InvalidGeometry, e.message))?;
        if !self.verification.verified
            || self.placed_count == 0
            || self.placed_count > self.requested_count
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
