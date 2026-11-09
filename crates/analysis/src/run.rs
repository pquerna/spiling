// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::{Budget, PreparedOperator, native, prepare, solve};
use spiling_contracts::{
    analysis::{AnalysisCaseRecord, fields::*, mechanics::*},
    geometry::{DefinitionId, RigidPoseMm, SourceHash},
    project::ProjectRevision,
};
use spiling_geometry::Definition;
use std::{sync::atomic::AtomicBool, time::Instant};

pub struct CachedOperator {
    native_sample_hash: [u8; 32],
    key: SourceHash,
    hash: SourceHash,
    operator: PreparedOperator,
    shape_error_mm: f64,
}
pub struct RunCapture {
    pub project_id: spiling_contracts::project::ProjectId,
    pub case: AnalysisCaseRecord,
    pub revision: ProjectRevision,
    pub pose: RigidPoseMm,
    pub settings: AnalysisRunSettings,
}
pub struct RunProduct {
    pub summary: AnalysisRunSummary,
    pub bytes: Vec<u8>,
}
pub fn execute(
    definition: &Definition,
    capture: &RunCapture,
    cache: &mut Option<CachedOperator>,
    cancel: &AtomicBool,
) -> Result<RunProduct, String> {
    let budget = Budget::new(cancel, capture.settings.maximum_seconds);
    capture
        .settings
        .validate()
        .map_err(|error| error.to_string())?;
    if definition.id() != &capture.case.case.definition_id
        || definition.provenance().source_hash != capture.case.case.source_hash
    {
        return Err("StaleSource: native source differs from captured analysis case".into());
    }
    let key = SourceHash::from_bytes(
        &serde_json::to_vec(&(
            ANALYSIS_PRODUCER,
            spiling_geometry::KERNEL_REVISION,
            definition.id(),
            &capture.case.case.source_hash,
            capture.pose,
            &capture.case.case.restraints,
            &capture.settings,
        ))
        .map_err(|error| error.to_string())?,
    );
    let reused = cache.as_ref().is_some_and(|cached| cached.key == key);
    let mut native_capture_ms = 0.0;
    if !reused {
        *cache = None;
        let started = Instant::now();
        let prism = native::capture(definition, capture.case.case.occurrence_id, cancel)?;
        native_capture_ms = started.elapsed().as_secs_f64() * 1000.0;
        budget.check()?;
        let operator = prepare(
            &prism,
            &capture.settings,
            capture.pose,
            &capture.case.case,
            &budget,
        )?;
        let hash = SourceHash::from_bytes(
            &serde_json::to_vec(&(&key, prism.native_sample_hash))
                .map_err(|error| error.to_string())?,
        );
        *cache = Some(CachedOperator {
            native_sample_hash: prism.native_sample_hash,
            key,
            hash,
            operator,
            shape_error_mm: prism.shape_error_mm,
        });
    }
    let cached = cache.as_ref().unwrap();
    let operator = &cached.operator;
    let solution = solve(operator, &capture.case.case, &capture.settings, &budget)?;
    let input_hash = SourceHash::from_bytes(
        &serde_json::to_vec(&(
            ANALYSIS_PRODUCER,
            &cached.hash,
            &capture.case,
            capture.revision,
        ))
        .map_err(|error| error.to_string())?,
    );
    let von_mises = |stress: [f64; 6]| {
        (0.5 * ((stress[0] - stress[1]).powi(2)
            + (stress[1] - stress[2]).powi(2)
            + (stress[2] - stress[0]).powi(2))
            + 3.0 * (stress[3].powi(2) + stress[4].powi(2) + stress[5].powi(2)))
        .sqrt()
    };
    let costs = AnalysisCosts {
        native_capture_ms,
        construction_ms: if reused {
            0.0
        } else {
            operator.construction_ms
        },
        operator_setup_ms: if reused { 0.0 } else { operator.setup_ms },
        rhs_ms: solution.rhs_ms,
        solve_ms: solution.solve_ms,
        assessment_ms: solution.assessment_ms,
        fields_ms: solution.fields_ms,
        operator_reused: reused,
    };
    let summary = AnalysisRunSummary {
        project_id: capture.project_id.clone(),
        kernel_revision: spiling_geometry::KERNEL_REVISION.into(),
        native_sample_hash: cached.native_sample_hash,
        model_scope: AnalysisModelScope::PlannedAssumedTiedMacro,
        accuracy_evidence: AnalysisAccuracyEvidence::Unqualified,
        producer: ANALYSIS_PRODUCER.into(),
        case: capture.case.clone(),
        captured_revision: capture.revision,
        input_hash,
        operator_hash: cached.hash.clone(),
        source_hash: capture.case.case.source_hash.clone(),
        pose: capture.pose,
        settings: capture.settings.clone(),
        native_shape_error_mm: cached.shape_error_mm,
        elements: operator.mesh.elements.len() as u32,
        dofs: (operator.mesh.nodes.len() * 3) as u32,
        components: operator.mesh.components as u32,
        region_volume_mm3: solution.volumes,
        nominal_material_volume_mm3: solution.volumes[0]
            + solution.volumes[1]
            + solution.volumes[2] * capture.settings.recipe.infill_fraction,
        region_energy_n_mm: solution.energy,
        strain_energy_n_mm: solution.energy.iter().sum(),
        maximum_displacement_mm: solution
            .displacement_mm
            .iter()
            .map(|value| value.iter().map(|entry| entry * entry).sum::<f64>().sqrt())
            .fold(0.0, f64::max),
        maximum_macro_von_mises_mpa: solution
            .fields
            .iter()
            .map(|field| von_mises(field.stress_mpa))
            .fold(0.0, f64::max),
        maximum_macro_layer_normal_mpa: solution
            .fields
            .iter()
            .map(|field| field.stress_mpa[operator.mesh.layer_axis].abs())
            .fold(0.0, f64::max),
        concentration_radius_mm: 3.0 * capture.settings.recipe.bead_width_mm,
        assessment: solution.assessment.clone(),
        costs,
    };
    let nodes = operator
        .mesh
        .nodes
        .iter()
        .enumerate()
        .map(|(index, &position_mm)| AnalysisNode {
            position_mm,
            displacement_mm: solution.displacement_mm[index],
            applied_force_n: solution.applied_force_n[index],
        })
        .collect::<Vec<_>>();
    let elements = operator
        .mesh
        .elements
        .iter()
        .zip(&solution.fields)
        .map(|(hex, field)| AnalysisElement {
            nodes: hex.nodes.map(|node| node as u32),
            center_mm: field.center_mm,
            size_mm: hex.size,
            role: field.role,
            volume_mm3: field.volume_mm3,
            stress_mpa: field.stress_mpa,
            energy_n_mm: field.energy_n_mm,
            concentration: field.concentration,
        })
        .collect::<Vec<_>>();
    let patches = solution
        .patches
        .iter()
        .map(|patch| AnalysisPatchSample {
            point_mm: patch.point_mm,
            area_mm2: patch.area_mm2,
            traction_mpa: patch.traction_mpa,
            patch_index: patch.patch_index,
        })
        .collect::<Vec<_>>();
    let bytes = spiling_contracts::analysis::fields::encode(&summary, &nodes, &elements, &patches)
        .map_err(|error| error.to_string())?;
    budget.check()?;
    Ok(RunProduct { summary, bytes })
}
impl CachedOperator {
    pub fn definition_id(&self) -> &DefinitionId {
        &self.operator.subject.0
    }
}
