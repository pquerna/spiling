// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
//! Bounded saved-stock decisions. Inventory, estimates and project application stay distinct.
use crate::{appearance::*, geometry::SourceHash, library::*, optical::*};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

pub const STOCK_PRODUCER: &str = "spiling-stock-d65-prusa-v7-runs4-1";
pub const STOCK_REPORT_MIME: &str = "application/x-spiling-stock-match+json";
pub const MAX_STOCK_REPORT_BYTES: usize = 1024 * 1024;
pub const MAX_STOCK_CANDIDATES: usize = 512;
pub const MAX_STOCK_RESULTS: usize = 16;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StockScope {
    Project,
    MyLibrary,
    Both,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StockQuery {
    pub scope: StockScope,
    pub on_hand_only: bool,
    pub target: OpticalTarget,
    pub patch: OpticalPatch,
    pub slot_height_mm: f64,
    pub max_evaluations: u32,
}
impl StockQuery {
    pub fn validate(&self) -> Result<(), String> {
        self.target.color.validate().map_err(|e| e.to_string())?;
        if !self.target.delta_e00_tolerance.is_finite()
            || self.target.delta_e00_tolerance < 0.0
            || self
                .target
                .lightness_tolerance
                .is_some_and(|v| !v.is_finite() || v < 0.0)
        {
            return Err("target tolerances must be finite and nonnegative".into());
        }
        self.patch.validate().map_err(|e| e.to_string())?;
        if !self.slot_height_mm.is_finite()
            || !(0.01..=1.0).contains(&self.slot_height_mm)
            || !(1..=65536).contains(&self.max_evaluations)
        {
            return Err("stock search needs a 0.01–1 mm slot and 1–65536 evaluations".into());
        }
        Ok(())
    }
    /// A disclosed finite grid: two contiguous runs, 1–4 uniform slots each,
    /// with either material first. No ranking-dependent hidden shortlist.
    pub fn patterns(&self) -> Vec<OpticalPattern> {
        let mut patterns = Vec::with_capacity(32);
        for a in 1..=4 {
            for b in 1..=4 {
                for reverse in [false, true] {
                    let mut materials = vec![0; a];
                    materials.extend(vec![1; b]);
                    if reverse {
                        materials.reverse();
                    }
                    patterns.push(OpticalPattern { materials });
                }
            }
        }
        patterns
    }
    pub fn request(&self, materials: Vec<String>, recipe: LayerCycle) -> OpticalRequest {
        OpticalRequest {
            model: OpticalModel::PrusaV7_007bb922,
            materials,
            target: self.target,
            observation: OpticalObservation {
                illuminant: AppearanceIlluminant::D65,
                observer: AppearanceObserver::Cie1931TwoDegree,
                view: OpticalView::NormalToPatch,
            },
            patch: self.patch,
            recipe,
            search: OpticalSearchSpace {
                patterns: self.patterns(),
                slot_heights_mm: vec![self.slot_height_mm],
                phase_slots: vec![0],
                origin_z_mm: self.patch.origin_mm[2],
            },
            limits: SearchLimits {
                max_visited: 32,
                max_alternatives: 1,
            },
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StockCandidate {
    pub filament: FilamentRef,
    pub name: String,
    #[serde(deserialize_with = "required_option")]
    pub appearance: Option<DeclaredAppearance>,
    #[serde(deserialize_with = "required_option")]
    pub hueforge_td_mm: Option<f64>,
    pub in_project: bool,
    pub in_library: bool,
    pub on_hand_spools: u32,
    pub profile_count: u32,
}
impl StockCandidate {
    pub fn validate(&self) -> Result<(), String> {
        self.filament.validate()?;
        if self.name.is_empty()
            || self.name.len() > 256
            || self.name.chars().any(char::is_control)
            || (!self.in_project && !self.in_library)
            || self.on_hand_spools > 512
            || self.profile_count > 16
            || self
                .hueforge_td_mm
                .is_some_and(|v| !v.is_finite() || v <= 0.0)
        {
            return Err("invalid stock candidate".into());
        }
        if let Some(a) = &self.appearance {
            a.color.validate().map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    pub fn palette_row(&self, key: String) -> PaletteRow {
        PaletteRow {
            key,
            material_id: self.filament.id.0.clone(),
            evidence_id: "declared".into(),
            name: self.name.clone(),
            evidence: match self.appearance.as_ref().map(|a| a.color) {
                Some(AppearanceColor::Srgb { channels }) => {
                    AppearanceEvidence::CatalogEstimate { srgb: channels }
                }
                _ => AppearanceEvidence::Unknown {
                    reason: "No declared sRGB reference estimate".into(),
                },
            },
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StockSingle {
    pub candidate: u16,
    pub delta_e00: f64,
    pub within_tolerance: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StockPair {
    pub candidates: [u16; 2],
    pub cycle: LayerCycle,
    pub assessment: OpticalAssessment,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StockSearchStatus {
    Complete,
    WorkLimit,
    Cancelled,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StockMatchReport {
    pub producer: String,
    pub input_hash: SourceHash,
    pub query: StockQuery,
    #[serde(deserialize_with = "required_option")]
    pub library: Option<LibraryIdentity>,
    pub candidates: Vec<StockCandidate>,
    pub singles: Vec<StockSingle>,
    pub pairs: Vec<StockPair>,
    pub unassessed_singles: u32,
    pub unassessed_pairs: u32,
    pub total_pairs: u32,
    pub visited_pairs: u32,
    pub evaluations: u32,
    pub status: StockSearchStatus,
}
impl StockMatchReport {
    pub fn input_hash(
        query: &StockQuery,
        library: &Option<LibraryIdentity>,
        candidates: &[StockCandidate],
    ) -> Result<SourceHash, String> {
        let bytes = serde_json::to_vec(&(STOCK_PRODUCER, query, library, candidates))
            .map_err(|e| e.to_string())?;
        Ok(SourceHash::from_bytes(&bytes))
    }
    pub fn validate(&self) -> Result<(), String> {
        self.query.validate()?;
        if let Some(library) = &self.library {
            library.validate()?;
        }
        if self.producer != STOCK_PRODUCER
            || self.candidates.len() > MAX_STOCK_CANDIDATES
            || self.input_hash != Self::input_hash(&self.query, &self.library, &self.candidates)?
            || self.singles.len() > MAX_STOCK_RESULTS
            || self.pairs.len() > MAX_STOCK_RESULTS
            || self.visited_pairs > self.total_pairs
            || self.unassessed_pairs > self.visited_pairs
            || self.unassessed_singles as usize > self.candidates.len()
            || self.evaluations > self.query.max_evaluations
            || (self.status == StockSearchStatus::Complete
                && self.visited_pairs != self.total_pairs)
        {
            return Err("stock report identity, bounds or completeness differs".into());
        }
        let mut refs = std::collections::BTreeSet::new();
        for candidate in &self.candidates {
            candidate.validate()?;
            if !refs.insert(&candidate.filament) {
                return Err("duplicate stock ingredient".into());
            }
        }
        let expected_pairs: u32 = (0..self.candidates.len())
            .map(|a| {
                ((a + 1)..self.candidates.len())
                    .filter(|&b| self.candidates[a].filament.id != self.candidates[b].filament.id)
                    .count() as u32
            })
            .sum();
        if self.total_pairs != expected_pairs
            || self.evaluations < self.visited_pairs * 32
            || self.pairs.len() as u32 > self.visited_pairs - self.unassessed_pairs
            || self.singles.len() + self.unassessed_singles as usize > self.candidates.len()
            || (self.status != StockSearchStatus::Cancelled
                && self.evaluations != self.visited_pairs * 32)
        {
            return Err("stock search coverage differs from its disclosed pair grid".into());
        }
        let valid = |n: u16| usize::from(n) < self.candidates.len();
        let mut singles = std::collections::BTreeSet::new();
        for single in &self.singles {
            if !singles.insert(single.candidate)
                || !valid(single.candidate)
                || !single.delta_e00.is_finite()
                || single.delta_e00 < 0.0
            {
                return Err("invalid stock single result".into());
            }
        }
        let mut pairs = std::collections::BTreeSet::new();
        for pair in &self.pairs {
            if !pairs.insert(pair.candidates) {
                return Err("duplicate ranked stock pair".into());
            }
            let request = self.query.request(
                vec!["a/declared".into(), "b/declared".into()],
                pair.cycle.clone(),
            );
            request.validate().map_err(|e| e.to_string())?;
            crate::optical::validate_assessment(
                &pair.assessment,
                &pair.cycle.slots,
                usize::from(pair.cycle.phase_slot),
                pair.cycle.origin_z_mm,
                &self.query.patch,
                &request.observation,
                &self.query.target,
            )
            .map_err(|e| e.to_string())?;
            if pair.cycle.phase_slot != 0
                || pair.cycle.origin_z_mm != self.query.patch.origin_mm[2]
                || pair
                    .cycle
                    .slots
                    .iter()
                    .any(|s| s.height_mm != self.query.slot_height_mm)
                || !self.query.patterns().iter().any(|p| {
                    p.materials
                        .iter()
                        .copied()
                        .eq(pair.cycle.slots.iter().map(|s| s.material))
                })
            {
                return Err("stock result cycle is outside the disclosed grid".into());
            }

            if !pair.candidates.iter().all(|&n| valid(n))
                || pair.candidates[0] >= pair.candidates[1]
                || self.candidates[usize::from(pair.candidates[0])].filament.id
                    == self.candidates[usize::from(pair.candidates[1])].filament.id
            {
                return Err("invalid stock pair ingredients".into());
            }
            pair.cycle
                .validate(2, &self.query.patch)
                .map_err(|e| e.to_string())?;
            let OpticalAssessment::Estimate {
                delta_e00,
                predicted_lab,
                ..
            } = &pair.assessment
            else {
                return Err("ranked stock pair lacks an estimate".into());
            };
            if !delta_e00.is_finite()
                || *delta_e00 < 0.0
                || predicted_lab.iter().any(|v| !v.is_finite())
            {
                return Err("invalid stock pair estimate".into());
            }
        }
        Ok(())
    }
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_STOCK_REPORT_BYTES {
            return Err("stock report exceeds 1 MiB".into());
        }
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_STOCK_REPORT_BYTES {
            return Err("stock report exceeds 1 MiB".into());
        }
        let report: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        report.validate()?;
        Ok(report)
    }
    /// Builds an ordinary portable recipe from exactly the reviewed ingredient bytes.
    pub fn recipe(
        &self,
        index: usize,
        id: RecipeId,
        name: String,
        filaments: Vec<FilamentRevision>,
    ) -> Result<RecipeTransfer, String> {
        self.validate()?;
        let pair = self
            .pairs
            .get(index)
            .ok_or("selected stock recipe is absent")?;
        if filaments.len() != 2
            || filaments
                .iter()
                .zip(pair.candidates)
                .any(|(f, n)| f.reference != self.candidates[usize::from(n)].filament)
        {
            return Err("selected stock ingredient revisions differ".into());
        }
        let mut transfer = RecipeTransfer::from_declarations(
            id,
            name,
            filaments,
            self.query.target.color,
            vec![self.query.slot_height_mm; 2],
        )?;
        transfer.recipe.definition.request = self.query.request(
            transfer.recipe.definition.request.materials.clone(),
            pair.cycle.clone(),
        );
        transfer.recipe.revision = transfer.recipe.definition.content_hash()?;
        transfer.validate()?;
        Ok(transfer)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StockMatchRecord {
    pub hash: SourceHash,
    pub byte_count: u64,
    pub input_hash: SourceHash,
    pub producer: String,
    pub complete: bool,
}
impl StockMatchRecord {
    pub fn validate(&self) -> Result<(), String> {
        if self.producer != STOCK_PRODUCER
            || self.byte_count == 0
            || self.byte_count > MAX_STOCK_REPORT_BYTES as u64
        {
            return Err("invalid stock report record".into());
        }
        Ok(())
    }
    pub fn validate_bytes(&self, bytes: &[u8]) -> Result<StockMatchReport, String> {
        self.validate()?;
        if bytes.len() as u64 != self.byte_count || SourceHash::from_bytes(bytes) != self.hash {
            return Err("stock report bytes differ".into());
        }
        let report = StockMatchReport::decode(bytes)?;
        if report.input_hash != self.input_hash
            || self.complete != (report.status == StockSearchStatus::Complete)
        {
            return Err("stock report inputs/completion differ".into());
        }
        Ok(report)
    }
}

fn required_option<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StockChoice {
    Single(usize),
    Pair(usize),
}
