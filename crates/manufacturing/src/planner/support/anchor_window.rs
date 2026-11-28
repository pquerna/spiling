// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::{
    DepositionPath, LayerDagTrace, LayerStep, ManufacturingError, ManufacturingErrorCode,
    PrintLayer, anchors, raster, vec_bytes,
};
use std::sync::atomic::AtomicBool;

// Dependency operands are borrowed directly from the layer arrays. Refill
// before mutating them: pipelined's two closures cannot share these mutable
// arrays without copying the authored and staged support paths.
const ANCHOR_WINDOW_LAYERS: usize = 32;

#[derive(Default)]
pub(super) struct AnchorWindow {
    first: usize,
    end: usize,
    owned: usize,
    layers: Vec<Option<anchors::PreparedLayer>>,
}

impl AnchorWindow {
    pub(super) fn exhausted(&self, index: usize) -> bool {
        index >= self.end
    }

    pub(super) fn bytes(&self) -> usize {
        self.owned
    }

    pub(super) fn take(&mut self, index: usize) -> Option<anchors::PreparedLayer> {
        let receipt = self.layers.get_mut(index - self.first)?.take()?;
        self.owned -= receipt.bytes();
        Some(receipt)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Window preparation needs the live layer and support ledger"
    )]
    pub(super) fn fill(
        &mut self,
        first: usize,
        layers: &[PrintLayer],
        staged: &[Option<(Vec<DepositionPath>, raster::WallReceipt)>],
        skip: impl Fn(usize) -> bool + Sync,
        bytes: usize,
        units: usize,
        step: LayerStep,
        trace: &mut LayerDagTrace,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        use rayon::prelude::*;

        self.layers = Vec::new();
        self.first = first;
        self.owned = 0;
        let end = (first + ANCHOR_WINDOW_LAYERS).min(layers.len());
        self.end = end;
        let started = trace.start();
        let working_set = crate::working_set::current();
        let slot_bytes = (end - first) * size_of::<Option<anchors::PreparedLayer>>();
        let active_layers = (first..end).filter(|&index| !skip(index)).count();
        if active_layers == 0 {
            self.layers.resize_with(end - first, || None);
            trace.finish(step, first, started);
            return Ok(());
        }
        let allowance =
            wave_allowance(bytes, units).saturating_sub(slot_bytes) / active_layers.max(1);
        if allowance < size_of::<Option<anchors::PreparedLayer>>() {
            return Ok(());
        }
        self.layers = (first..end)
            .into_par_iter()
            .map(|index| {
                if skip(index) {
                    return Ok(None);
                }
                crate::working_set::scope(working_set, || {
                    let support = staged
                        .get(index - 1)
                        .and_then(Option::as_ref)
                        .into_iter()
                        .flat_map(|(paths, _)| paths.iter());
                    match anchors::PreparedLayer::build(
                        &layers[index].paths,
                        layers[index - 1].paths.iter().chain(support),
                        allowance,
                        cancel,
                    ) {
                        Err(failure) if failure.code == ManufacturingErrorCode::ResourceLimit => {
                            Ok(None)
                        }
                        result => result,
                    }
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.owned = vec_bytes(&self.layers)
            + self
                .layers
                .iter()
                .flatten()
                .map(anchors::PreparedLayer::bytes)
                .sum::<usize>();
        if started.is_some() {
            let pairs = self
                .layers
                .iter()
                .flatten()
                .map(anchors::PreparedLayer::pairs)
                .sum::<usize>();
            trace.prepared(step, pairs, self.owned);
        }
        trace.finish(step, first, started);
        Ok(())
    }
}

// Leave the ordered consumers room to grow their live geometry and scratch.
fn wave_allowance(bytes: usize, units: usize) -> usize {
    crate::working_set::bytes()
        .saturating_sub(bytes)
        .min(crate::working_set::units().saturating_sub(units) * size_of::<geo::Coord<f64>>())
        / 4
}
