// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;
use spiling_compute::{MAX_BATCH_ITEMS, MAX_MASK_WORDS, MIN_BATCH_PAIRS, MaskRows};

pub(crate) struct AnchorBatch {
    mask: MaskRows,
    edge_prefix: Vec<usize>,
}

impl PreparedAnchorPaths<'_> {
    pub(crate) fn screen(
        &self,
        candidates: &[AnchorCandidate],
        cancel: &AtomicBool,
    ) -> Result<Option<AnchorBatch>, ManufacturingError> {
        check_cancel(cancel)?;
        let count = self.paths.len();
        if count == 0
            || count > MAX_BATCH_ITEMS
            || candidates.len() > MAX_BATCH_ITEMS
            || count.saturating_mul(candidates.len()) < MIN_BATCH_PAIRS
            || candidates.len().saturating_mul(count.div_ceil(32)) > MAX_MASK_WORDS
        {
            return Ok(None);
        }
        let mut radius = 0.0_f64;
        for candidate in candidates {
            check_cancel(cancel)?;
            if !candidate.bead_width_mm.is_finite()
                || candidate.bead_width_mm <= 0.0
                || candidate
                    .endpoints_mm
                    .iter()
                    .flatten()
                    .any(|value| !value.is_finite())
            {
                return Ok(None);
            }
            radius = radius.max(candidate.bead_width_mm * 0.5);
        }
        let allocation = || {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "anchor compute staging allocation refused",
            )
        };
        let mut boxes = Vec::new();
        let mut queries = Vec::new();
        let mut edge_prefix = Vec::new();
        boxes.try_reserve_exact(count).map_err(|_| allocation())?;
        queries
            .try_reserve_exact(candidates.len())
            .map_err(|_| allocation())?;
        edge_prefix
            .try_reserve_exact(count + 1)
            .map_err(|_| allocation())?;
        edge_prefix.push(0usize);
        for path in &self.paths {
            check_cancel(cancel)?;
            let reach = radius + path.max_radius;
            let bounds = [
                path.bounds[0][0] - reach,
                path.bounds[0][1] - reach,
                path.bounds[1][0] + reach,
                path.bounds[1][1] + reach,
            ];
            if bounds.iter().any(|value| !value.is_finite()) {
                return Ok(None);
            }
            boxes.push(bounds);
            edge_prefix.push(
                edge_prefix
                    .last()
                    .copied()
                    .unwrap()
                    .checked_add(path.edges)
                    .ok_or_else(allocation)?,
            );
        }
        for candidate in candidates {
            check_cancel(cancel)?;
            let [start, end] = candidate.endpoints_mm;
            queries.push([
                start[0].min(end[0]),
                start[1].min(end[1]),
                start[0].max(end[0]),
                start[1].max(end[1]),
            ]);
        }
        let Some(mask) = spiling_compute::box_overlaps(&boxes, &queries, cancel)
            .map_err(crate::compute_error)?
        else {
            return Ok(None);
        };
        Ok(Some(AnchorBatch { mask, edge_prefix }))
    }
}

impl AnchorWorkspace {
    pub(crate) fn gaps_screened(
        &mut self,
        candidate: AnchorCandidate,
        paths: &PreparedAnchorPaths<'_>,
        batch: &AnchorBatch,
        row: usize,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<&[AnchorGap], ManufacturingError> {
        if batch.mask.columns() != paths.paths.len()
            || batch.edge_prefix.len() != paths.paths.len() + 1
        {
            return Err(error(
                ManufacturingErrorCode::VerificationFailed,
                "anchor compute inventory mismatch",
            ));
        }
        let selected = batch.mask.selected_columns(row).ok_or_else(|| {
            error(
                ManufacturingErrorCode::VerificationFailed,
                "anchor compute query missing",
            )
        })?;
        let mut cursor = 0;
        let inputs = selected
            .chain(std::iter::once(paths.paths.len()))
            .flat_map(|index| {
                let skipped = batch.edge_prefix[index] - batch.edge_prefix[cursor];
                cursor = (index + 1).min(paths.paths.len());
                std::iter::once(AnchorInput::Skipped(skipped)).chain(
                    PreparedAnchorPaths::inputs_from(candidate, paths.paths.get(index).into_iter()),
                )
            });
        self.gaps_inputs(candidate, inputs, work, cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiling_contracts::manufacturing::{DepositionKind, WidthProfile};

    #[test]
    fn screened_anchors_match_scalar_gaps_and_refusal_counters() {
        let cancel = AtomicBool::new(false);
        let paths = (0..64)
            .map(|index| {
                let position = f64::from(index) * 4.0;
                DepositionPath {
                    kind: DepositionKind::SolidFill,
                    owner: None,
                    material_domain: None,
                    width: WidthProfile::PerEdge {
                        widths_mm: vec![0.4, 1.2],
                    },
                    physical_z_mm: [0.0, 0.2],
                    points_mm: vec![
                        [position, -4.0, 0.2],
                        [position + 1.0, 4.0, 0.2],
                        [position + 2.0, -4.0, 0.2],
                    ],
                    arcs: Vec::new(),
                    exposed: false,
                }
            })
            .collect::<Vec<_>>();
        let candidates = (0..128)
            .map(|index| {
                let position = f64::from(index) * 3.0 - 32.0;
                AnchorCandidate {
                    endpoints_mm: [[position, -3.0, 0.4], [position + 1.0, 3.0, 0.4]],
                    bead_width_mm: if index % 2 == 0 { 0.4 } else { 2.4 },
                }
            })
            .collect::<Vec<_>>();
        let prepared = PreparedAnchorPaths::new(paths.iter(), &cancel).unwrap();
        let batch = prepared.screen(&candidates, &cancel).unwrap().unwrap();
        let capture = |gaps: &[AnchorGap]| {
            gaps.iter()
                .map(|gap| (gap.start, gap.end, gap.left, gap.right, gap.direct_ends))
                .collect::<Vec<_>>()
        };
        for (row, candidate) in candidates.into_iter().enumerate() {
            for initial in [0, usize::MAX - 128, usize::MAX - 127, usize::MAX] {
                let mut scalar = AnchorWorkspace::default();
                let mut screened = AnchorWorkspace::default();
                let mut scalar_work = initial;
                let mut screened_work = initial;
                let expected = scalar
                    .gaps_prepared(candidate, &prepared, &mut scalar_work, &cancel)
                    .map(capture)
                    .map_err(|failure| (failure.code, failure.message));
                let actual = screened
                    .gaps_screened(
                        candidate,
                        &prepared,
                        &batch,
                        row,
                        &mut screened_work,
                        &cancel,
                    )
                    .map(capture)
                    .map_err(|failure| (failure.code, failure.message));
                assert_eq!(actual, expected, "row={row}, initial={initial}");
                assert_eq!(screened_work, scalar_work);
            }
        }
        assert!(prepared.screen(&[], &cancel).unwrap().is_none());
        cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            prepared.screen(&[], &cancel).err().unwrap().code,
            ManufacturingErrorCode::Cancelled
        );
    }
}
