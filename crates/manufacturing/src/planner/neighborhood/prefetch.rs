use super::{Cached, Material, SkinNeighborhoods, Storage, Usage, resource};
use crate::{check_cancel, working_set};
use rayon::prelude::*;
use spiling_contracts::manufacturing::{ManufacturingError, ManufacturingErrorCode};
use std::{mem::size_of, sync::atomic::AtomicBool};

struct Prepared<'a> {
    material: Material<'a>,
    peak: Usage,
}

pub(super) struct Peak {
    staged: Usage,
    live: Usage,
}

impl Peak {
    pub(super) fn record(&mut self, points: usize, bytes: usize) {
        self.live.points = self.live.points.max(points - self.staged.points);
        self.live.bytes = self.live.bytes.max(bytes - self.staged.bytes);
    }
}

struct Slot<'a> {
    pair: [usize; 2],
    order: usize,
    result: Option<Prepared<'a>>,
}

#[derive(Default)]
pub(super) struct Table<'a> {
    slots: Vec<Slot<'a>>,
    geometry: Usage,
    remaining: usize,
}

impl Table<'_> {
    pub(super) fn usage(&self) -> Usage {
        Usage {
            points: self.geometry.points,
            bytes: self.geometry.bytes + self.slots.capacity() * size_of::<Slot<'_>>(),
        }
    }

    fn revoke_for(&mut self, live: Usage) {
        if !self.slots.is_empty() && !live.add(self.usage()).is_ok_and(Usage::fits) {
            *self = Self::default();
        }
    }
}

impl<'a> SkinNeighborhoods<'a> {
    pub(in crate::planner) fn has_prefetch(&self) -> bool {
        !self.prefetch.slots.is_empty()
    }

    pub(in crate::planner) fn clear_prefetch(&mut self) {
        self.prefetch = Table::default();
    }

    pub(super) fn revoke_prefetch_for(&mut self, live: Usage) {
        if !self.has_prefetch() {
            return;
        }
        let tree = Usage {
            points: self.retained,
            bytes: self.tree_bytes(),
        };
        match live.add(tree) {
            Ok(live) => self.prefetch.revoke_for(live),
            Err(_) => self.clear_prefetch(),
        }
    }

    #[expect(
        clippy::type_complexity,
        reason = "Split the revocable admission callback from the work ledger"
    )]
    pub(in crate::planner) fn fixed_budget_and_work(
        &mut self,
        external_points: usize,
        external_bytes: usize,
    ) -> Result<
        (
            impl FnMut(usize, usize) -> Result<(), ManufacturingError> + '_,
            &mut usize,
        ),
        ManufacturingError,
    > {
        let base = Usage {
            points: external_points,
            bytes: external_bytes,
        }
        .add(Usage {
            points: self.retained,
            bytes: self.tree_bytes(),
        })?;
        let table = &mut self.prefetch;
        let mut admit = super::classification::fixed_owned_budget(base.points, base.bytes);
        Ok((
            move |points, bytes| {
                admit(points, bytes)?;
                table.revoke_for(base.add(Usage { points, bytes })?);
                Ok(())
            },
            &mut self.work,
        ))
    }

    pub(super) fn take_prefetched(
        &mut self,
        pair: [usize; 2],
        external: Usage,
    ) -> Option<Material<'a>> {
        let table = &mut self.prefetch;
        let index = table
            .slots
            .binary_search_by_key(&pair, |slot| slot.pair)
            .ok()?;
        let Prepared { material, peak } = table.slots[index].result.take()?;
        let use_now = material.temporary_usage();
        table.geometry.points -= use_now.points;
        table.geometry.bytes -= use_now.bytes;
        table.remaining -= 1;
        if table.remaining == 0 {
            self.clear_prefetch();
        }
        let live = external.add(peak).ok()?;
        self.revoke_prefetch_for(live);
        live.add(Usage {
            points: self.retained,
            bytes: self.tree_bytes(),
        })
        .is_ok_and(Usage::fits)
        .then_some(material)
    }

    pub(in crate::planner) fn prefetch(
        &mut self,
        requests: impl Iterator<Item = (usize, Option<usize>)>,
        request_count: usize,
        retained_points: usize,
        outside_bytes: usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        match self.prepare(
            requests,
            request_count,
            retained_points,
            outside_bytes,
            cancel,
        ) {
            Err(failure) if failure.code == ManufacturingErrorCode::Cancelled => Err(failure),
            Err(_) => check_cancel(cancel),
            result => result,
        }
    }

    fn prepare(
        &mut self,
        requests: impl Iterator<Item = (usize, Option<usize>)>,
        request_count: usize,
        retained_points: usize,
        outside_bytes: usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        check_cancel(cancel)?;
        let workers = rayon::current_num_threads();
        if workers <= 1 || request_count <= 1 || !self.prefetch.slots.is_empty() {
            return Ok(());
        }
        let external = self
            .query_external(retained_points, outside_bytes)?
            .add(Usage {
                points: self.retained,
                bytes: self.tree_bytes(),
            })?;
        let available = super::super::remaining_geometry_units(external.points, external.bytes)?
            * size_of::<geo::Coord<f64>>();
        let slots_bytes = request_count
            .checked_mul(size_of::<Slot<'_>>())
            .filter(|&bytes| bytes < available)
            .ok_or_else(|| resource("skin prefetch request bytes do not fit"))?;
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(request_count)
            .map_err(|_| resource("skin prefetch request allocation refused"))?;
        if slots.capacity() * size_of::<Slot<'_>>() > slots_bytes {
            return Ok(());
        }
        for (order, (current, neighbor)) in requests.enumerate() {
            check_cancel(cancel)?;
            let Some(neighbor) = neighbor else { continue };
            if slots.len() == request_count
                || current >= self.sections.len()
                || neighbor >= self.sections.len()
            {
                return Ok(());
            }
            let mut pair = [current, neighbor];
            pair.sort_unstable();
            if self
                .memo
                .as_ref()
                .is_some_and(|memo| !memo.entries.is_empty())
                && self.range_key(pair[0], pair[1]).is_some_and(|key| {
                    self.memo
                        .as_ref()
                        .is_some_and(|memo| memo.entries.contains_key(&key))
                })
            {
                continue;
            }
            slots.push(Slot {
                pair,
                order,
                result: None,
            });
        }
        slots.sort_unstable_by_key(|slot| (slot.pair, slot.order));
        slots.dedup_by_key(|slot| slot.pair);
        slots.sort_unstable_by_key(|slot| slot.order);
        if slots.len() <= 1 {
            return Ok(());
        }

        let chunk = slots.len().div_ceil(workers);
        let lanes = slots.len().div_ceil(chunk);
        let receipts = lanes * size_of::<Result<usize, ManufacturingError>>();
        let share = available
            .saturating_sub(slots_bytes)
            .saturating_sub(receipts)
            / lanes;
        let lane_allowance = working_set::WorkingSet::new(share)?;
        let captured = working_set::current();
        let source: &Self = self;
        let work: Vec<_> = slots
            .par_chunks_mut(chunk)
            .map(|slots| {
                working_set::scope(captured, || {
                    working_set::scope(lane_allowance, || source.prepare_lane(slots, cancel))
                })
            })
            .collect();
        check_cancel(cancel)?;
        let mut total_work = self.work;
        for receipt in work {
            super::charge(&mut total_work, receipt?)?;
        }
        let geometry = slots.iter().try_fold(Usage::default(), |sum, slot| {
            sum.add(
                slot.result
                    .as_ref()
                    .expect("successful lane filled slot")
                    .material
                    .temporary_usage(),
            )
        })?;
        let remaining = slots.len();
        let table = Table {
            slots,
            geometry,
            remaining,
        };
        if !external.add(table.usage())?.fits() {
            return Ok(());
        }
        self.prefetch = table;
        self.prefetch.slots.sort_unstable_by_key(|slot| slot.pair);
        self.work = total_work;
        Ok(())
    }

    fn prepare_lane(
        &self,
        slots: &mut [Slot<'a>],
        cancel: &AtomicBool,
    ) -> Result<usize, ManufacturingError> {
        let tree_bytes = self.tree_bytes().saturating_sub(self.retained_owned_bytes);
        if tree_bytes > working_set::bytes() {
            return Err(resource("skin prefetch lane index does not fit"));
        }
        let mut nodes = Vec::new();
        nodes
            .try_reserve_exact(self.nodes.len())
            .map_err(|_| resource("skin prefetch lane index allocation refused"))?;
        let groups = self
            .groups
            .as_ref()
            .map(|starts| {
                let mut groups = Vec::new();
                groups
                    .try_reserve_exact(starts.len())
                    .map_err(|_| resource("skin prefetch group allocation refused"))?;
                groups.extend_from_slice(starts);
                Ok::<_, ManufacturingError>(groups)
            })
            .transpose()?;
        if nodes.capacity() * size_of::<Cached>()
            + groups
                .as_ref()
                .map_or(0, |groups| groups.capacity() * size_of::<usize>())
            > working_set::bytes()
        {
            return Err(resource("skin prefetch lane index capacity does not fit"));
        }
        nodes.resize_with(self.nodes.len(), || Cached::Missing);
        let mut lane = Self {
            sections: self.sections,
            groups,
            memo: None,
            identities: Vec::new(),
            fill_clearance_mm: self.fill_clearance_mm,
            nodes,
            base: self.base,
            leaf_identities_ready: false,
            retained: 0,
            retained_capacity: working_set::units(),
            retained_owned_bytes: 0,
            retained_owned_byte_capacity: working_set::bytes(),
            work: self.nodes.len(),
            prefetch: Table::default(),
            measure_peak: None,
        };
        let mut staged = Usage::default();
        for slot in slots {
            check_cancel(cancel)?;
            lane.measure_peak = Some(Peak {
                staged,
                live: Usage::default(),
            });
            let [first, last] = slot.pair;
            let mut material = lane.survival_range(first, last, staged, cancel)?;
            if let Storage::Cached(shared) = material.storage {
                material.storage = Storage::Detached(shared);
            }
            let usage = material.temporary_usage();
            lane.admit(staged, usage, cancel)?;
            staged = staged.add(usage)?;
            slot.result = Some(Prepared {
                material,
                peak: lane
                    .measure_peak
                    .take()
                    .expect("lane is recording admission")
                    .live,
            });
        }
        Ok(lane.work)
    }
}
