// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub(super) enum LayerStep {
    DemandPreparation,
    Demand,
    RasterPreparation,
    RasterAdmission,
    DonorPreparation,
    Support,
    ObjectPreparation,
    Object,
}

impl LayerStep {
    const ALL: [Self; 8] = [
        Self::DemandPreparation,
        Self::Demand,
        Self::RasterPreparation,
        Self::RasterAdmission,
        Self::DonorPreparation,
        Self::Support,
        Self::ObjectPreparation,
        Self::Object,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::DemandPreparation => "demand_anchor_preparation",
            Self::Demand => "demand",
            Self::RasterPreparation => "raster_preparation",
            Self::RasterAdmission => "raster_admission",
            Self::DonorPreparation => "donor_preparation",
            Self::Support => "final_support",
            Self::ObjectPreparation => "object_anchor_preparation",
            Self::Object => "final_object",
        }
    }

    fn preparation(self) -> bool {
        matches!(
            self,
            Self::DemandPreparation
                | Self::ObjectPreparation
                | Self::RasterPreparation
                | Self::DonorPreparation
        )
    }
}

#[derive(Default)]
struct Stage {
    previous: Option<usize>,
    calls: usize,
    reordered: usize,
    elapsed: Duration,
    prepared_pairs: usize,
    peak_bytes: usize,
}

/// Stages publish in ascending layer order. A second evaluation of any
/// published node also violates that order. Time each ordered segment
/// separately so the scope can be added without double-counting sub-phases.
pub(super) struct LayerDagTrace {
    enabled: bool,
    stages: [Stage; LayerStep::ALL.len()],
}

impl LayerDagTrace {
    pub(super) fn new() -> Self {
        Self {
            enabled: std::env::var_os("SPILING_TRACE_SLICE_PHASES").is_some(),
            stages: Default::default(),
        }
    }

    pub(super) fn start(&self) -> Option<Instant> {
        self.enabled.then(Instant::now)
    }

    pub(super) fn finish(&mut self, step: LayerStep, layer: usize, started: Option<Instant>) {
        self.record(step, layer, started.map(|started| started.elapsed()));
    }

    pub(super) fn record(&mut self, step: LayerStep, layer: usize, elapsed: Option<Duration>) {
        if let Some(elapsed) = elapsed {
            let stage = &mut self.stages[step as usize];
            stage.calls += 1;
            // Donor nodes are built on their first receiver's demand. Their
            // Pending/Ready/Released transition prevents a second evaluation.
            if !matches!(step, LayerStep::DonorPreparation) {
                stage.reordered +=
                    usize::from(stage.previous.is_some_and(|previous| layer <= previous));
            }
            stage.previous = Some(layer);
            stage.elapsed += elapsed;
        }
    }

    pub(super) fn prepared(&mut self, step: LayerStep, pairs: usize, bytes: usize) {
        let stage = &mut self.stages[step as usize];
        stage.prepared_pairs = stage.prepared_pairs.saturating_add(pairs);
        stage.peak_bytes = stage.peak_bytes.max(bytes);
    }

    pub(super) fn elapsed(&self, step: LayerStep) -> Duration {
        self.stages[step as usize].elapsed
    }
}

impl Drop for LayerDagTrace {
    fn drop(&mut self) {
        for step in LayerStep::ALL {
            let stage = &self.stages[step as usize];
            if stage.calls != 0 {
                eprintln!(
                    "{}",
                    serde_json::json!({
                        "event": "slice_phase",
                        "scope": "support.layer_dag",
                        "phase": step.label(),
                        "kind": if step.preparation() { "preparation" } else { "ordered" },
                        "calls": stage.calls,
                        "repeated_or_out_of_order": stage.reordered,
                        "seconds": stage.elapsed.as_secs_f64(),
                        "prepared_pairs": stage.prepared_pairs,
                        "peak_receipt_bytes": stage.peak_bytes,
                    })
                );
            }
        }
    }
}
