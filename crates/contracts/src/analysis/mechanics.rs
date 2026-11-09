// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::manufacturing::PlanarPrintRecipe;

pub const ANALYSIS_PRODUCER: &str = "assumed-printed-hex27-2";
pub const MAX_MECHANICAL_ELEMENTS: usize = 768;
pub const MAX_MECHANICAL_DOFS: usize = 16_000;
pub const MAX_MECHANICAL_FIELD_BYTES: usize = 4 * 1024 * 1024;
pub const ANALYSIS_FIELDS_MIME: &str = "application/x-spiling-analysis-fields";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisModelScope {
    PlannedAssumedTiedMacro,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AnalysisAccuracyEvidence {
    Unqualified,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AssumedPrintedLaws {
    pub name: String,
    pub justification: String,
    pub compatible_recipe: PlanarPrintRecipe,
    pub wall_mpa: [[f64; 6]; 6],
    pub skin_mpa: [[f64; 6]; 6],
    pub effective_core_mpa: [[f64; 6]; 6],
}
impl AssumedPrintedLaws {
    pub fn validate(&self) -> Result<(), ProjectError> {
        self.compatible_recipe
            .validate()
            .map_err(|error| invalid(&error.to_string()))?;
        if self.name.is_empty() || self.name.len() > 120 || self.name.chars().any(char::is_control)
        {
            return Err(invalid(
                "assumed mechanical laws need a bounded explicit name",
            ));
        }
        if self.justification.trim().is_empty()
            || self.justification.len() > 2048
            || self
                .justification
                .chars()
                .any(|character| character.is_control() && character != '\n')
        {
            return Err(invalid(
                "assumed laws require an explicit bounded compatibility/model justification",
            ));
        }
        for tensor in [&self.wall_mpa, &self.skin_mpa, &self.effective_core_mpa] {
            if tensor
                .iter()
                .flatten()
                .any(|value| !value.is_finite() || value.abs() > 1e8)
            {
                return Err(invalid(
                    "assumed engineering tensor entries must be finite and bounded",
                ));
            }
            let mut lower = [[0.0_f64; 6]; 6];
            for row in 0..6 {
                for column in 0..=row {
                    let value = tensor[row][column];
                    if !value.is_finite()
                        || value.abs() > 1e8
                        || (value - tensor[column][row]).abs() > 1e-10 * value.abs().max(1.0)
                    {
                        return Err(invalid(
                            "assumed engineering tensors must be finite and symmetric",
                        ));
                    }
                    let pivot = value
                        - (0..column)
                            .map(|inner| lower[row][inner] * lower[column][inner])
                            .sum::<f64>();
                    lower[row][column] = if row == column {
                        if pivot <= 0.0 {
                            return Err(invalid(
                                "assumed engineering tensor is not positive definite",
                            ));
                        }
                        pivot.sqrt()
                    } else {
                        pivot / lower[column][column]
                    };
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AnalysisRunSettings {
    pub recipe: PlanarPrintRecipe,
    pub laws: AssumedPrintedLaws,
    pub maximum_span_mm: f64,
    pub maximum_seconds: u32,
}
impl AnalysisRunSettings {
    pub fn validate(&self) -> Result<(), ProjectError> {
        self.recipe
            .validate()
            .map_err(|error| invalid(&error.to_string()))?;
        self.laws.validate()?;
        if self.recipe != self.laws.compatible_recipe {
            return Err(invalid(
                "changed recipe needs an explicitly compatible assumed law, not recipe-hash scaling",
            ));
        }
        if !self.maximum_span_mm.is_finite()
            || !(0.25..=32.0).contains(&self.maximum_span_mm)
            || !(1..=60).contains(&self.maximum_seconds)
            || self.recipe.infill_fraction <= 0.0
        {
            return Err(invalid(
                "mechanical span must be 0.25..32 mm, deadline 1..60 s, and assumed core fraction positive",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AnalysisCosts {
    pub native_capture_ms: f64,
    pub construction_ms: f64,
    pub operator_setup_ms: f64,
    pub rhs_ms: f64,
    pub solve_ms: f64,
    pub assessment_ms: f64,
    pub fields_ms: f64,
    pub operator_reused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AnalysisNumericalAssessment {
    pub free_residual_n: f64,
    pub rhs_norm_n: f64,
    pub force_imbalance_n: f64,
    pub moment_imbalance_n_mm: f64,
    pub work_energy_relative_error: f64,
    pub patch_force_error_n: f64,
    pub patch_moment_error_n_mm: f64,
    pub rigid_constraint_ranks: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AnalysisRunSummary {
    pub project_id: ProjectId,
    pub kernel_revision: String,
    pub native_sample_hash: [u8; 32],
    pub model_scope: AnalysisModelScope,
    pub accuracy_evidence: AnalysisAccuracyEvidence,
    pub producer: String,
    pub case: AnalysisCaseRecord,
    pub captured_revision: ProjectRevision,
    pub input_hash: SourceHash,
    pub operator_hash: SourceHash,
    pub source_hash: SourceHash,
    pub pose: RigidPoseMm,
    pub settings: AnalysisRunSettings,
    pub native_shape_error_mm: f64,
    pub elements: u32,
    pub dofs: u32,
    pub components: u32,
    pub region_volume_mm3: [f64; 3],
    pub nominal_material_volume_mm3: f64,
    pub region_energy_n_mm: [f64; 3],
    pub strain_energy_n_mm: f64,
    pub maximum_displacement_mm: f64,
    pub maximum_macro_von_mises_mpa: f64,
    pub maximum_macro_layer_normal_mpa: f64,
    pub concentration_radius_mm: f64,
    pub assessment: AnalysisNumericalAssessment,
    pub costs: AnalysisCosts,
}

impl AnalysisRunSummary {
    pub fn expected_operator_hash(&self) -> Result<SourceHash, ProjectError> {
        let key = SourceHash::from_bytes(
            &serde_json::to_vec(&(
                ANALYSIS_PRODUCER,
                &self.kernel_revision,
                &self.case.case.definition_id,
                &self.case.case.source_hash,
                self.pose,
                &self.case.case.restraints,
                &self.settings,
            ))
            .map_err(|error| invalid(&error.to_string()))?,
        );
        Ok(SourceHash::from_bytes(
            &serde_json::to_vec(&(&key, self.native_sample_hash))
                .map_err(|error| invalid(&error.to_string()))?,
        ))
    }
    pub fn expected_input_hash(&self) -> Result<SourceHash, ProjectError> {
        Ok(SourceHash::from_bytes(
            &serde_json::to_vec(&(
                ANALYSIS_PRODUCER,
                &self.operator_hash,
                &self.case,
                self.captured_revision,
            ))
            .map_err(|error| invalid(&error.to_string()))?,
        ))
    }
    pub fn validate(&self) -> Result<(), ProjectError> {
        self.settings.validate()?;
        self.case.case.validate()?;
        self.pose
            .validate()
            .map_err(|error| invalid(&error.to_string()))?;
        if self.producer != ANALYSIS_PRODUCER
            || self.case.case.loads.is_empty()
            || self.case.case.restraints.is_empty()
            || self.kernel_revision.is_empty()
            || self.kernel_revision.len() > 80
            || self.operator_hash != self.expected_operator_hash()?
            || self.input_hash != self.expected_input_hash()?
            || self.source_hash != self.case.case.source_hash
            || self.case.edited_revision.0 > self.captured_revision.0
            || self.elements == 0
            || self.elements as usize > MAX_MECHANICAL_ELEMENTS
            || self.dofs == 0
            || self.dofs as usize > MAX_MECHANICAL_DOFS
            || !self.dofs.is_multiple_of(3)
            || self.components == 0
            || self.components > self.elements
            || self.assessment.rigid_constraint_ranks.len() != self.components as usize
            || self
                .assessment
                .rigid_constraint_ranks
                .iter()
                .any(|rank| *rank != 6)
        {
            return Err(invalid(
                "analysis summary has invalid producer, capture, sizes or restraint ranks",
            ));
        }
        let values = [
            self.native_shape_error_mm,
            self.nominal_material_volume_mm3,
            self.strain_energy_n_mm,
            self.maximum_displacement_mm,
            self.maximum_macro_von_mises_mpa,
            self.maximum_macro_layer_normal_mpa,
            self.concentration_radius_mm,
            self.assessment.free_residual_n,
            self.assessment.rhs_norm_n,
            self.assessment.force_imbalance_n,
            self.assessment.moment_imbalance_n_mm,
            self.assessment.work_energy_relative_error,
            self.assessment.patch_force_error_n,
            self.assessment.patch_moment_error_n_mm,
            self.costs.native_capture_ms,
            self.costs.construction_ms,
            self.costs.operator_setup_ms,
            self.costs.rhs_ms,
            self.costs.solve_ms,
            self.costs.assessment_ms,
            self.costs.fields_ms,
        ];
        if values
            .into_iter()
            .chain(self.region_volume_mm3)
            .chain(self.region_energy_n_mm)
            .any(|value| !value.is_finite() || value < 0.0)
            || self.strain_energy_n_mm <= 0.0
            || self.nominal_material_volume_mm3 <= 0.0
            || self.concentration_radius_mm <= 0.0
        {
            return Err(invalid(
                "analysis summary contains invalid dimensional values",
            ));
        }
        if self.costs.operator_reused
            && (self.costs.construction_ms != 0.0
                || self.costs.operator_setup_ms != 0.0
                || self.costs.native_capture_ms != 0.0)
        {
            return Err(invalid(
                "warm operator reuse must not report newly performed setup",
            ));
        }
        Ok(())
    }
}
