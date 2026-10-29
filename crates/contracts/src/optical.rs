// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Inert, bounded headless optical studies. Estimates are not routed print instructions.
//! Physical arithmetic admits at most 2^52 periods in either the origin-relative
//! offset or patch height, with period/minimum-slot ratio at most 2^52. Collapsed
//! build/relative/periodic intervals and unrepresentable wrapped phases refuse.
//! These are f64 representability boundaries, not printer/minimum-layer policies.
use crate::appearance::{
    AppearanceColor, AppearanceIlluminant, AppearanceObserver, AppearanceOutcome, AppearanceStatus,
    MAX_APPEARANCE_ALTERNATIVES, MAX_APPEARANCE_REPORT_BYTES, PaletteRecord, PreparedPalette,
    SearchLimits, bounded_sequence, encode_bounded, hash_json, preflight_appearance_json,
    required_option, text64,
};
use crate::geometry::SourceHash;
use crate::project::{ProjectError, ProjectErrorCode};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use ts_rs::TS;

pub const OPTICAL_PRODUCER: &str = "spiling-optical-prusa-v7-007bb922-cycle1";
pub const OPTICAL_REPORT_MIME: &str = "application/x-spiling-optical-report+json";
pub const MAX_OPTICAL_REQUEST_BYTES: usize = 64 * 1024;
pub const MAX_OPTICAL_CANDIDATES: usize = 1024;
const MAX_EXACT_PERIODS: f64 = 4_503_599_627_370_496.0;

fn invalid(message: &str) -> ProjectError {
    ProjectError::new(ProjectErrorCode::InvalidProject, message)
}
fn resource(message: &str) -> ProjectError {
    ProjectError::new(ProjectErrorCode::ResourceLimit, message)
}
fn positive(value: f64) -> bool {
    value.is_finite() && value > 0.0
}
fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 128.0 * f64::EPSILON * a.abs().max(b.abs()).max(f64::MIN_POSITIVE)
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpticalModel {
    PrusaV7_007bb922,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpticalView {
    NormalToPatch,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpticalEvidence {
    ReferenceEstimate,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpticalExecution {
    StudyOnlyNotRouted,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OpticalObservation {
    pub illuminant: AppearanceIlluminant,
    pub observer: AppearanceObserver,
    pub view: OpticalView,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OpticalPatch {
    pub origin_mm: [f64; 3],
    pub normal_xy: [f64; 2],
    pub width_mm: f64,
    pub height_mm: f64,
}
impl OpticalPatch {
    pub fn validate(&self) -> Result<(), ProjectError> {
        if self
            .origin_mm
            .iter()
            .chain(self.normal_xy.iter())
            .any(|v| !v.is_finite())
            || !positive(self.width_mm)
            || !positive(self.height_mm)
            || (self.normal_xy[0].hypot(self.normal_xy[1]) - 1.0).abs() > 1e-12
            || !(self.origin_mm[2] + self.height_mm).is_finite()
            || self.origin_mm[2] + self.height_mm <= self.origin_mm[2]
        {
            return Err(invalid(
                "optical patch requires finite representable upright dimensions and a unit XY normal",
            ));
        }
        // The finite width lies along the horizontal tangent to the normal.
        for (origin, tangent) in self.origin_mm[..2]
            .iter()
            .zip([-self.normal_xy[1], self.normal_xy[0]])
        {
            let extent = tangent * self.width_mm;
            if !extent.is_finite()
                || !(origin + extent).is_finite()
                || (extent != 0.0 && origin + extent == *origin)
            {
                return Err(invalid(
                    "optical patch width is not representable in the build frame",
                ));
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OpticalTarget {
    pub color: AppearanceColor,
    pub delta_e00_tolerance: f64,
    #[serde(deserialize_with = "required_option")]
    pub lightness_tolerance: Option<f64>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OpticalSlot {
    pub material: u8,
    pub height_mm: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LayerCycle {
    #[serde(deserialize_with = "slots64")]
    pub slots: Vec<OpticalSlot>,
    pub phase_slot: u16,
    pub origin_z_mm: f64,
}
impl LayerCycle {
    /// Checks actual unequal slots before periodic arithmetic. Out-of-range
    /// search phases are filtered separately.
    pub fn validate(&self, materials: usize, patch: &OpticalPatch) -> Result<(), ProjectError> {
        validate_slots(
            &self.slots,
            self.phase_slot as usize,
            self.origin_z_mm,
            materials,
            patch,
        )
    }
}
fn validate_slots(
    slots: &[OpticalSlot],
    phase: usize,
    origin: f64,
    materials: usize,
    patch: &OpticalPatch,
) -> Result<(), ProjectError> {
    patch.validate()?;
    if slots.is_empty() || slots.len() > 64 || phase >= slots.len() || !origin.is_finite() {
        return Err(invalid(
            "optical cycle requires 1..64 slots and a valid explicit phase/origin",
        ));
    }
    let mut period = 0.0;
    let mut minimum = f64::INFINITY;
    for i in 0..slots.len() {
        let slot = &slots[(phase + i) % slots.len()];
        if slot.material as usize >= materials || !positive(slot.height_mm) {
            return Err(invalid(
                "optical slot has an invalid material or nonpositive/nonfinite height",
            ));
        }
        let next = period + slot.height_mm;
        if !next.is_finite() || next <= period {
            return Err(invalid(
                "optical cycle loses a physical slot in f64 arithmetic",
            ));
        }
        period = next;
        minimum = minimum.min(slot.height_mm);
    }
    let offset = patch.origin_mm[2] - origin;
    let end = offset + patch.height_mm;
    if !offset.is_finite()
        || !end.is_finite()
        || end <= offset
        || !positive(period)
        || period / minimum > MAX_EXACT_PERIODS
        || !(offset / period).is_finite()
        || (offset / period).abs() > MAX_EXACT_PERIODS
        || !(patch.height_mm / period).is_finite()
        || patch.height_mm / period > MAX_EXACT_PERIODS
    {
        return Err(invalid(
            "optical cycle/window exceeds the documented 2^52-period representability boundary",
        ));
    }
    let periodic_phase = offset.rem_euclid(period);
    if periodic_phase >= period
        || (patch.height_mm < period && periodic_phase + patch.height_mm == periodic_phase)
    {
        return Err(invalid(
            "optical origin-relative phase/window is not representable inside its period",
        ));
    }
    Ok(())
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OpticalPattern {
    #[serde(deserialize_with = "indices64")]
    pub materials: Vec<u8>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OpticalSearchSpace {
    #[serde(deserialize_with = "patterns64")]
    pub patterns: Vec<OpticalPattern>,
    #[serde(deserialize_with = "heights16")]
    pub slot_heights_mm: Vec<f64>,
    #[serde(deserialize_with = "phases64")]
    pub phase_slots: Vec<u16>,
    pub origin_z_mm: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OpticalRequest {
    pub model: OpticalModel,
    #[serde(deserialize_with = "keys3")]
    pub materials: Vec<String>,
    pub target: OpticalTarget,
    pub observation: OpticalObservation,
    pub patch: OpticalPatch,
    pub recipe: LayerCycle,
    pub search: OpticalSearchSpace,
    pub limits: SearchLimits,
}
impl OpticalRequest {
    pub fn validate(&self) -> Result<(), ProjectError> {
        if !(2..=3).contains(&self.materials.len()) {
            return Err(invalid(
                "optical request needs two or three independent palette materials",
            ));
        }
        for (i, key) in self.materials.iter().enumerate() {
            if key.is_empty()
                || key.len() > 64
                || key.trim() != key
                || key.chars().any(char::is_control)
                || self.materials[..i].contains(key)
            {
                return Err(invalid(
                    "optical material keys must be distinct bounded canonical text",
                ));
            }
        }
        self.target.color.validate()?;
        if !self.target.delta_e00_tolerance.is_finite()
            || self.target.delta_e00_tolerance < 0.0
            || self
                .target
                .lightness_tolerance
                .is_some_and(|v| !v.is_finite() || v < 0.0)
        {
            return Err(invalid("optical tolerances must be finite and nonnegative"));
        }
        self.recipe.validate(self.materials.len(), &self.patch)?;
        self.limits.validate()?;
        if self.limits.max_visited as usize > MAX_OPTICAL_CANDIDATES {
            return Err(resource("optical visited limit exceeds 1024"));
        }
        let search = &self.search;
        if search.patterns.is_empty()
            || search.patterns.len() > 64
            || search.slot_heights_mm.is_empty()
            || search.slot_heights_mm.len() > 16
            || search.phase_slots.is_empty()
            || search.phase_slots.len() > 64
            || !search.origin_z_mm.is_finite()
        {
            return Err(resource(
                "optical grid cardinality/origin exceeds supported bounds",
            ));
        }
        for (i, pattern) in search.patterns.iter().enumerate() {
            if pattern.materials.is_empty()
                || pattern.materials.len() > 64
                || pattern
                    .materials
                    .iter()
                    .any(|&m| m as usize >= self.materials.len())
                || search.patterns[..i].contains(pattern)
            {
                return Err(invalid(
                    "optical patterns must be distinct nonempty ordered material-index sequences",
                ));
            }
        }
        self.candidate_count()?;
        let mut slots = [OpticalSlot {
            material: 0,
            height_mm: 0.0,
        }; 64];
        for (i, &height) in search.slot_heights_mm.iter().enumerate() {
            if !positive(height) || search.slot_heights_mm[..i].contains(&height) {
                return Err(invalid(
                    "optical grid slot heights must be positive finite and unique",
                ));
            }
            // Validate physical arithmetic once per pattern/height; a uniform
            // pattern's period is independent of the visited phase.
            for pattern in &search.patterns {
                for (slot, &material) in slots.iter_mut().zip(&pattern.materials) {
                    *slot = OpticalSlot {
                        material,
                        height_mm: height,
                    };
                }
                validate_slots(
                    &slots[..pattern.materials.len()],
                    0,
                    search.origin_z_mm,
                    self.materials.len(),
                    &self.patch,
                )?;
            }
        }
        for (i, &phase) in search.phase_slots.iter().enumerate() {
            if phase >= 64 || search.phase_slots[..i].contains(&phase) {
                return Err(invalid(
                    "optical phases must be unique explicit slot indices below 64",
                ));
            }
        }
        hash_json(self, MAX_OPTICAL_REQUEST_BYTES)?;
        Ok(())
    }
    pub fn validate_palette(&self, palette: &PreparedPalette) -> Result<(), ProjectError> {
        self.validate()?;
        let rows = &palette.document().rows;
        let mut ids: [Option<&str>; 3] = [None; 3];
        for (i, key) in self.materials.iter().enumerate() {
            let index = rows.binary_search_by(|row| row.key.cmp(key)).map_err(|_| {
                invalid("optical material key does not exist in the prepared palette")
            })?;
            let id = rows[index].material_id.as_str();
            if ids[..i].contains(&Some(id)) {
                return Err(invalid(
                    "optical pool cannot count two evidence rows for the same material as independent materials",
                ));
            }
            ids[i] = Some(id);
        }
        Ok(())
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, ProjectError> {
        preflight_appearance_json(bytes, MAX_OPTICAL_REQUEST_BYTES)?;
        let request: Self = serde_json::from_slice(bytes)
            .map_err(|_| invalid("invalid bounded optical request JSON"))?;
        request.validate()?;
        Ok(request)
    }
    pub fn encode(&self) -> Result<Vec<u8>, ProjectError> {
        self.validate()?;
        encode_bounded(self, MAX_OPTICAL_REQUEST_BYTES)
    }
    pub fn candidate_count(&self) -> Result<u32, ProjectError> {
        if !(1..=64).contains(&self.search.patterns.len())
            || !(1..=16).contains(&self.search.slot_heights_mm.len())
            || !(1..=64).contains(&self.search.phase_slots.len())
        {
            return Err(resource(
                "optical grid dimensions exceed their declared bounds",
            ));
        }
        let count = self
            .search
            .patterns
            .len()
            .checked_mul(self.search.slot_heights_mm.len())
            .and_then(|v| v.checked_mul(self.search.phase_slots.len()))
            .filter(|&v| v > 0 && v <= MAX_OPTICAL_CANDIDATES)
            .ok_or_else(|| resource("optical declared grid must contain 1..1024 candidates"))?;
        Ok(count as u32)
    }
    /// Pattern outermost, height next, phase innermost. Invalid pattern-specific
    /// phases intentionally survive construction, so the visit can be filtered.
    pub fn candidate(&self, index: u32) -> Result<LayerCycle, ProjectError> {
        self.validate()?;
        if index >= self.candidate_count()? {
            return Err(invalid(
                "optical candidate index is outside the declared grid",
            ));
        }
        let phases = self.search.phase_slots.len();
        let heights = self.search.slot_heights_mm.len();
        let phase = index as usize % phases;
        let height = index as usize / phases % heights;
        let pattern = index as usize / phases / heights;
        Ok(LayerCycle {
            slots: self.search.patterns[pattern]
                .materials
                .iter()
                .map(|&material| OpticalSlot {
                    material,
                    height_mm: self.search.slot_heights_mm[height],
                })
                .collect(),
            phase_slot: self.search.phase_slots[phase],
            origin_z_mm: self.search.origin_z_mm,
        })
    }
    pub fn allocated_bytes(&self) -> usize {
        self.materials.capacity() * std::mem::size_of::<String>()
            + self.materials.iter().map(String::capacity).sum::<usize>()
            + self.recipe.slots.capacity() * std::mem::size_of::<OpticalSlot>()
            + self.search.patterns.capacity() * std::mem::size_of::<OpticalPattern>()
            + self
                .search
                .patterns
                .iter()
                .map(|p| p.materials.capacity())
                .sum::<usize>()
            + self.search.slot_heights_mm.capacity() * std::mem::size_of::<f64>()
            + self.search.phase_slots.capacity() * std::mem::size_of::<u16>()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CycleExposure {
    pub period_mm: f64,
    pub phase_at_patch_mm: f64,
    pub complete_periods: u64,
    pub exposed_height_mm: [f64; 3],
    pub fractions: [f64; 3],
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpticalUnknownReason {
    MissingEvidence,
    UnsupportedConditions,
    UnsupportedColorDomain,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OpticalAssessment {
    FilteredPhase,
    Unassessed {
        reason: OpticalUnknownReason,
        exposure: CycleExposure,
    },
    Estimate {
        exposure: CycleExposure,
        predicted_lab: [f64; 3],
        display_srgb8: [u8; 3],
        delta_e00: f64,
        delta_lightness: f64,
        within_tolerance: bool,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OpticalVisit {
    pub index: u32,
    pub assessment: OpticalAssessment,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OpticalSelection {
    Requested,
    Candidate { index: u32 },
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OpticalReport {
    #[serde(deserialize_with = "text64")]
    pub producer: String,
    #[serde(deserialize_with = "hash64")]
    pub input_hash: SourceHash,
    #[serde(deserialize_with = "hash64")]
    pub palette_hash: SourceHash,
    pub request: Arc<OpticalRequest>,
    #[serde(deserialize_with = "hashes3")]
    pub material_evidence_hashes: Vec<SourceHash>,
    pub evidence: OpticalEvidence,
    pub execution: OpticalExecution,
    pub requested: OpticalAssessment,
    pub total_candidates: u32,
    #[serde(deserialize_with = "visits1024")]
    pub visits: Vec<OpticalVisit>,
    #[serde(deserialize_with = "alternatives256")]
    pub alternatives: Vec<u32>,
    pub alternatives_truncated: bool,
    pub status: AppearanceStatus,
}

pub fn optical_input_hash(
    palette_hash: &SourceHash,
    request: &OpticalRequest,
) -> Result<SourceHash, ProjectError> {
    request.validate()?;
    Ok(hash_json(
        &(OPTICAL_PRODUCER, palette_hash, request),
        MAX_OPTICAL_REQUEST_BYTES + 256,
    )?
    .0)
}

fn validate_assessment(
    assessment: &OpticalAssessment,
    slots: &[OpticalSlot],
    phase: usize,
    origin: f64,
    request: &OpticalRequest,
) -> Result<(), ProjectError> {
    if phase >= slots.len() {
        return if matches!(assessment, OpticalAssessment::FilteredPhase) {
            Ok(())
        } else {
            Err(invalid("invalid grid phase must be a filtered visit"))
        };
    }
    let exposure = match assessment {
        OpticalAssessment::FilteredPhase => {
            return Err(invalid(
                "valid cycle cannot have a filtered phase assessment",
            ));
        }
        OpticalAssessment::Unassessed { exposure, .. } => exposure,
        OpticalAssessment::Estimate {
            exposure,
            predicted_lab,
            delta_e00,
            delta_lightness,
            within_tolerance,
            ..
        } => {
            if request.observation.illuminant != AppearanceIlluminant::D65
                || request.observation.observer != AppearanceObserver::Cie1931TwoDegree
                || predicted_lab.iter().any(|v| !v.is_finite())
                || !delta_e00.is_finite()
                || *delta_e00 < 0.0
                || !delta_lightness.is_finite()
                || *delta_lightness < 0.0
                || *within_tolerance
                    != (*delta_e00 <= request.target.delta_e00_tolerance
                        && request
                            .target
                            .lightness_tolerance
                            .is_none_or(|max| *delta_lightness <= max))
            {
                return Err(invalid(
                    "optical estimate is nonfinite or inconsistent with target tolerances",
                ));
            }
            exposure
        }
    };
    let mut period = 0.0;
    let mut present = [false; 3];
    for i in 0..slots.len() {
        let slot = slots[(phase + i) % slots.len()];
        period += slot.height_mm;
        present[slot.material as usize] = true;
    }
    let expected_phase = (request.patch.origin_mm[2] - origin).rem_euclid(period);
    let remainder = request.patch.height_mm % period;
    let complete = ((request.patch.height_mm - remainder) / period).round() as u64;
    if !positive(exposure.period_mm)
        || !close(exposure.period_mm, period)
        || !exposure.phase_at_patch_mm.is_finite()
        || exposure.phase_at_patch_mm < 0.0
        || exposure.phase_at_patch_mm >= exposure.period_mm
        || !close(exposure.phase_at_patch_mm, expected_phase)
        || exposure.complete_periods != complete
        || exposure
            .exposed_height_mm
            .iter()
            .chain(exposure.fractions.iter())
            .any(|v| !v.is_finite() || *v < 0.0)
        || !close(
            exposure.exposed_height_mm.iter().sum(),
            request.patch.height_mm,
        )
        || !close(exposure.fractions.iter().sum(), 1.0)
        || (0..3).any(|i| {
            (!present[i] && (exposure.exposed_height_mm[i] != 0.0 || exposure.fractions[i] != 0.0))
                || exposure.fractions[i] > 1.0
                || !close(
                    exposure.fractions[i],
                    exposure.exposed_height_mm[i] / request.patch.height_mm,
                )
        })
    {
        return Err(invalid(
            "optical exposure is nonfinite or inconsistent with its physical cycle/window",
        ));
    }
    Ok(())
}
impl OpticalReport {
    pub fn is_complete(&self) -> bool {
        matches!(self.status, AppearanceStatus::Complete { .. })
    }
    pub fn has_selection(&self, selection: &OpticalSelection) -> bool {
        match selection {
            OpticalSelection::Requested => {
                matches!(self.requested, OpticalAssessment::Estimate { .. })
            }
            OpticalSelection::Candidate { index } => {
                self.alternatives.contains(index)
                    && self
                        .visits
                        .get(*index as usize)
                        .is_some_and(|v| matches!(v.assessment, OpticalAssessment::Estimate { .. }))
            }
        }
    }
    pub fn validate(&self) -> Result<(), ProjectError> {
        self.request.validate()?;
        if self.producer != OPTICAL_PRODUCER
            || self.input_hash != optical_input_hash(&self.palette_hash, &self.request)?
            || self.material_evidence_hashes.len() != self.request.materials.len()
            || self.total_candidates != self.request.candidate_count()?
        {
            return Err(invalid(
                "optical report producer/palette/request/evidence identity mismatch",
            ));
        }
        if self.visits.len() > self.total_candidates as usize
            || self.visits.len() > self.request.limits.max_visited as usize
            || self.alternatives.len() > self.request.limits.max_alternatives as usize
        {
            return Err(resource(
                "optical report visited/alternative bounds exceeded",
            ));
        }
        validate_assessment(
            &self.requested,
            &self.request.recipe.slots,
            self.request.recipe.phase_slot as usize,
            self.request.recipe.origin_z_mm,
            &self.request,
        )?;
        let mut ranked = [(0.0, 0u32); MAX_OPTICAL_CANDIDATES];
        let mut assessed_count = 0;
        let mut slots = [OpticalSlot {
            material: 0,
            height_mm: 0.0,
        }; 64];
        let mut any_match = false;
        let mut unknown = false;
        for (index, visit) in self.visits.iter().enumerate() {
            if visit.index != index as u32 {
                return Err(invalid(
                    "optical visits must be the sequential declared-grid prefix",
                ));
            }
            let phases = self.request.search.phase_slots.len();
            let heights = self.request.search.slot_heights_mm.len();
            let pattern = &self.request.search.patterns[index / phases / heights].materials;
            let height = self.request.search.slot_heights_mm[index / phases % heights];
            let phase = self.request.search.phase_slots[index % phases] as usize;
            for (slot, &material) in slots.iter_mut().zip(pattern) {
                *slot = OpticalSlot {
                    material,
                    height_mm: height,
                };
            }
            validate_assessment(
                &visit.assessment,
                &slots[..pattern.len()],
                phase,
                self.request.search.origin_z_mm,
                &self.request,
            )?;
            match visit.assessment {
                OpticalAssessment::Estimate {
                    delta_e00,
                    within_tolerance,
                    ..
                } => {
                    ranked[assessed_count] = (delta_e00, visit.index);
                    assessed_count += 1;
                    any_match |= within_tolerance;
                }
                OpticalAssessment::Unassessed { .. } => unknown = true,
                OpticalAssessment::FilteredPhase => {}
            }
        }
        let ranked = &mut ranked[..assessed_count];
        ranked.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        let retained = ranked
            .len()
            .min(self.request.limits.max_alternatives as usize);
        if self.alternatives.len() != retained
            || self
                .alternatives
                .iter()
                .zip(ranked.iter())
                .any(|(a, b)| *a != b.1)
            || self.alternatives_truncated != (ranked.len() > retained)
        {
            return Err(invalid(
                "optical alternatives must be the deterministic assessed top-K, with honest truncation",
            ));
        }
        match self.status {
            AppearanceStatus::Complete { outcome } => {
                let expected = if any_match {
                    AppearanceOutcome::AssessedMatch
                } else if ranked.is_empty() || unknown {
                    AppearanceOutcome::NoAssessedMatch
                } else {
                    AppearanceOutcome::AssessedMismatch
                };
                if self.visits.len() != self.total_candidates as usize || outcome != expected {
                    return Err(invalid(
                        "complete optical report has incomplete enumeration or inconsistent outcome",
                    ));
                }
            }
            AppearanceStatus::WorkLimit
                if self.visits.len() != self.request.limits.max_visited as usize
                    || self.visits.len() == self.total_candidates as usize =>
            {
                return Err(invalid(
                    "work-limited optical report has inconsistent prefix length",
                ));
            }
            AppearanceStatus::Cancelled if self.visits.len() == self.total_candidates as usize => {
                return Err(invalid(
                    "cancelled optical report already exhausted its grid",
                ));
            }
            _ => {}
        }
        hash_json(self, MAX_APPEARANCE_REPORT_BYTES)?;
        Ok(())
    }
    pub fn encode(&self) -> Result<Vec<u8>, ProjectError> {
        self.validate()?;
        encode_bounded(self, MAX_APPEARANCE_REPORT_BYTES)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, ProjectError> {
        preflight_appearance_json(bytes, MAX_APPEARANCE_REPORT_BYTES)?;
        let report: Self = serde_json::from_slice(bytes)
            .map_err(|_| invalid("invalid bounded optical report JSON"))?;
        report.validate()?;
        Ok(report)
    }
    pub fn canonical_record(&self) -> Result<OpticalReportRecord, ProjectError> {
        self.validate()?;
        let (hash, count) = hash_json(self, MAX_APPEARANCE_REPORT_BYTES)?;
        Ok(OpticalReportRecord {
            hash,
            byte_count: count as u64,
            input_hash: self.input_hash.clone(),
            producer: self.producer.clone(),
            complete: self.is_complete(),
        })
    }
    pub fn matches_inputs(
        &self,
        palette: &PaletteRecord,
        request: &OpticalRequest,
    ) -> Result<(), ProjectError> {
        self.validate()?;
        palette.validate()?;
        request.validate()?;
        if self.palette_hash != palette.canonical_hash || self.request.as_ref() != request {
            return Err(invalid(
                "optical report does not bind the active palette and exact ordered request",
            ));
        }
        Ok(())
    }
    /// Excludes the shared request allocation; its owner deduplicates Arc pointers.
    pub fn allocated_bytes(&self) -> usize {
        self.producer.capacity()
            + self.input_hash.allocated_bytes()
            + self.palette_hash.allocated_bytes()
            + self.material_evidence_hashes.capacity() * std::mem::size_of::<SourceHash>()
            + self
                .material_evidence_hashes
                .iter()
                .map(SourceHash::allocated_bytes)
                .sum::<usize>()
            + self.visits.capacity() * std::mem::size_of::<OpticalVisit>()
            + self.alternatives.capacity() * std::mem::size_of::<u32>()
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OpticalReportRecord {
    #[serde(deserialize_with = "hash64")]
    pub hash: SourceHash,
    pub byte_count: u64,
    #[serde(deserialize_with = "hash64")]
    pub input_hash: SourceHash,
    #[serde(deserialize_with = "text64")]
    pub producer: String,
    pub complete: bool,
}
impl OpticalReportRecord {
    pub fn validate(&self) -> Result<(), ProjectError> {
        if self.byte_count == 0 || self.byte_count > MAX_APPEARANCE_REPORT_BYTES as u64 {
            return Err(resource(
                "optical report byte count exceeds supported bounds",
            ));
        }
        if self.producer != OPTICAL_PRODUCER {
            return Err(invalid("unsupported optical report producer"));
        }
        Ok(())
    }
    pub fn validate_bytes(&self, bytes: &[u8]) -> Result<OpticalReport, ProjectError> {
        self.validate()?;
        if bytes.len() as u64 != self.byte_count || !self.hash.matches_bytes(bytes) {
            return Err(ProjectError::new(
                ProjectErrorCode::CorruptAsset,
                "optical report content hash/length mismatch",
            ));
        }
        let report = OpticalReport::decode(bytes)?;
        if report.canonical_record()? != *self {
            return Err(invalid(
                "optical descriptor does not bind canonical report bytes",
            ));
        }
        Ok(report)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OpticalStudy {
    pub request: Arc<OpticalRequest>,
    #[serde(deserialize_with = "required_option")]
    pub report: Option<Arc<OpticalReport>>,
    #[serde(deserialize_with = "required_option")]
    pub report_record: Option<OpticalReportRecord>,
    #[serde(deserialize_with = "required_option")]
    pub selection: Option<OpticalSelection>,
}
impl OpticalStudy {
    pub fn new(request: OpticalRequest) -> Self {
        Self {
            request: Arc::new(request),
            report: None,
            report_record: None,
            selection: None,
        }
    }
    pub fn validate(&self, palette: &PaletteRecord) -> Result<(), ProjectError> {
        validate_study_info(
            &self.request,
            self.report_record.as_ref(),
            self.selection,
            palette,
        )?;
        if let Some(report) = &self.report {
            self.matches_report(palette, report)?;
            if !report.is_complete() || Some(report.canonical_record()?) != self.report_record {
                return Err(invalid(
                    "accepted optical study requires a complete report and exact cached record",
                ));
            }
        } else if self.report_record.is_some() {
            return Err(invalid("optical report record lacks its accepted report"));
        }
        if self.selection.as_ref().is_some_and(|selection| {
            !self
                .report
                .as_ref()
                .is_some_and(|report| report.has_selection(selection))
        }) {
            return Err(invalid(
                "optical selection is not a requested estimate or retained assessed candidate",
            ));
        }
        Ok(())
    }
    pub fn matches_report(
        &self,
        palette: &PaletteRecord,
        report: &OpticalReport,
    ) -> Result<(), ProjectError> {
        report.matches_inputs(palette, &self.request)
    }
    pub fn info(&self) -> OpticalStudyInfo {
        OpticalStudyInfo {
            request: self.request.clone(),
            report: self.report_record.clone(),
            selection: self.selection,
        }
    }
    /// Excludes both shared request and report allocations.
    pub fn allocated_bytes(&self) -> usize {
        self.report_record.as_ref().map_or(0, |record| {
            record.hash.allocated_bytes()
                + record.input_hash.allocated_bytes()
                + record.producer.capacity()
        })
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OpticalStudyInfo {
    pub request: Arc<OpticalRequest>,
    #[serde(deserialize_with = "required_option")]
    pub report: Option<OpticalReportRecord>,
    #[serde(deserialize_with = "required_option")]
    pub selection: Option<OpticalSelection>,
}
impl OpticalStudyInfo {
    pub fn validate(&self, palette: &PaletteRecord) -> Result<(), ProjectError> {
        validate_study_info(&self.request, self.report.as_ref(), self.selection, palette)
    }
}
fn validate_study_info(
    request: &OpticalRequest,
    report: Option<&OpticalReportRecord>,
    selection: Option<OpticalSelection>,
    palette: &PaletteRecord,
) -> Result<(), ProjectError> {
    palette.validate()?;
    request.validate()?;
    if let Some(record) = report {
        record.validate()?;
        if !record.complete
            || record.input_hash != optical_input_hash(&palette.canonical_hash, request)?
        {
            return Err(invalid(
                "optical info record does not bind complete active inputs",
            ));
        }
    }
    if let Some(selection) = selection {
        if report.is_none() {
            return Err(invalid("optical selection info lacks an accepted report"));
        }
        if let OpticalSelection::Candidate { index } = selection {
            if index >= request.candidate_count()? {
                return Err(invalid("optical selection info index is outside the grid"));
            }
            let phases = request.search.phase_slots.len();
            let heights = request.search.slot_heights_mm.len();
            let pattern = &request.search.patterns[index as usize / phases / heights];
            if request.search.phase_slots[index as usize % phases] as usize
                >= pattern.materials.len()
            {
                return Err(invalid("optical selection info refers to a filtered phase"));
            }
        }
    }
    Ok(())
}

fn slots64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<OpticalSlot>, D::Error> {
    bounded_sequence::<D, OpticalSlot, 64>(d)
}
fn indices64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
    bounded_sequence::<D, u8, 64>(d)
}
fn patterns64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<OpticalPattern>, D::Error> {
    bounded_sequence::<D, OpticalPattern, 64>(d)
}
fn heights16<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<f64>, D::Error> {
    bounded_sequence::<D, f64, 16>(d)
}
fn phases64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<u16>, D::Error> {
    bounded_sequence::<D, u16, 64>(d)
}
fn hash64<'de, D: serde::Deserializer<'de>>(d: D) -> Result<SourceHash, D::Error> {
    SourceHash::parse(text64(d)?)
        .map_err(|_| serde::de::Error::custom("optical hash must be lowercase SHA-256 hex"))
}
fn hashes3<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<SourceHash>, D::Error> {
    #[derive(Deserialize)]
    #[serde(transparent)]
    struct Hash(#[serde(deserialize_with = "hash64")] SourceHash);
    bounded_sequence::<D, Hash, 3>(d).map(|hashes| hashes.into_iter().map(|hash| hash.0).collect())
}
fn visits1024<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<OpticalVisit>, D::Error> {
    bounded_sequence::<D, OpticalVisit, MAX_OPTICAL_CANDIDATES>(d)
}
fn alternatives256<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<u32>, D::Error> {
    bounded_sequence::<D, u32, MAX_APPEARANCE_ALTERNATIVES>(d)
}
fn keys3<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(transparent)]
    struct Key(#[serde(deserialize_with = "text64")] String);
    bounded_sequence::<D, Key, 3>(d).map(|keys| keys.into_iter().map(|key| key.0).collect())
}
