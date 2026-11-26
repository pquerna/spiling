// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::established::{self, Donor};
use super::{LayerDagTrace, LayerStep, Root, reserve_owned, sweep::PartContact, vec_bytes};
use crate::{check_cancel, error};
use spiling_contracts::manufacturing::{
    ManufacturingError, ManufacturingErrorCode, PlanarPrintRecipe, PrintLayer,
};

#[derive(Default)]
enum Receipt {
    #[default]
    Pending,
    Ready(Vec<Donor>),
    Released,
}

#[derive(Default)]
struct DonorSlot {
    last_reader: Option<usize>,
    receipt: Receipt,
}

pub(super) struct DonorCache {
    slots: Vec<DonorSlot>,
    donor_bytes: usize,
}

pub(super) struct DonorInput<'a> {
    pub(super) layers: &'a [PrintLayer],
    pub(super) target: usize,
    pub(super) contact_top: Option<usize>,
    pub(super) recipe: &'a PlanarPrintRecipe,
    pub(super) raster: &'a super::grid::Raster<'a>,
    pub(super) owned_bytes: usize,
}

impl DonorCache {
    pub(super) fn new<'a>(
        layers: usize,
        roots: &[Root],
        proposals: impl IntoIterator<Item = (usize, &'a PartContact)>,
        outside: usize,
        cancel: &std::sync::atomic::AtomicBool,
        work: &mut usize,
    ) -> Result<Self, ManufacturingError> {
        let mut slots = Vec::new();
        reserve_owned(&mut slots, layers, outside)?;
        slots.resize_with(layers, DonorSlot::default);
        for (layer, proposal) in proposals {
            check_cancel(cancel)?;
            for &root in &proposal.roots {
                super::super::native::charge_support_clearance(work, cancel)?;
                if let Root::Part { object_layer, .. } = roots[root] {
                    let slot = &mut slots[object_layer];
                    slot.last_reader = slot.last_reader.max(Some(layer));
                }
            }
        }
        Ok(Self {
            slots,
            donor_bytes: 0,
        })
    }

    pub(super) fn bytes(&self) -> usize {
        vec_bytes(&self.slots).saturating_add(self.donor_bytes)
    }

    /// The donor layer, the immediately lower deposition and its selected
    /// contact top are final before the receiver's support can read them.
    /// All receivers share the exact same measured donor spans.
    pub(super) fn get(
        &mut self,
        input: DonorInput<'_>,
        trace: &mut LayerDagTrace,
    ) -> Result<(&[Donor], usize), ManufacturingError> {
        let DonorInput {
            layers,
            target,
            contact_top,
            recipe,
            raster,
            owned_bytes,
        } = input;
        let slot_bytes = vec_bytes(&self.slots);
        let slot = &mut self.slots[target];
        if matches!(slot.receipt, Receipt::Pending) {
            let started = trace.start();
            let mut donors = Vec::new();
            established::local_donors(
                layers,
                target,
                contact_top,
                recipe,
                raster,
                owned_bytes,
                &mut donors,
            )?;
            self.donor_bytes = self.donor_bytes.saturating_add(vec_bytes(&donors));
            slot.receipt = Receipt::Ready(donors);
            trace.finish(LayerStep::DonorPreparation, target, started);
        }
        match &slot.receipt {
            Receipt::Ready(donors) => Ok((donors, slot_bytes.saturating_add(self.donor_bytes))),
            Receipt::Pending | Receipt::Released => Err(error(
                ManufacturingErrorCode::NativeAnalysisIndeterminate,
                "final part donors were requested after their last receiver",
            )),
        }
    }

    pub(super) fn release(
        &mut self,
        receiver: usize,
        roots: &[Root],
        proposal: &PartContact,
        cancel: &std::sync::atomic::AtomicBool,
        work: &mut usize,
    ) -> Result<(), ManufacturingError> {
        for &root in &proposal.roots {
            super::super::native::charge_support_clearance(work, cancel)?;
            if let Root::Part { object_layer, .. } = roots[root] {
                let slot = &mut self.slots[object_layer];
                if slot.last_reader == Some(receiver)
                    && let Receipt::Ready(donors) =
                        std::mem::replace(&mut slot.receipt, Receipt::Released)
                {
                    self.donor_bytes -= vec_bytes(&donors);
                }
            }
        }
        Ok(())
    }
}
