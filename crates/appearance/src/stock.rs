// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
//! Stock-wide composition of the existing D65 and finite-cycle reference evaluators.
use crate::prusa_v7::BaseColor;
use spiling_contracts::project::ProjectError;
use spiling_contracts::{library::LibraryIdentity, optical::*, stock::*};

struct PreparedMaterial {
    admission: Result<(), ProjectError>,
    basis: Result<BaseColor, OpticalUnknownReason>,
}

/// Search geometry is independent of filament colors. Validate the disclosed
/// grid once; retain arithmetic errors until their original visit boundary.
struct PreparedGrid {
    request: OpticalRequest,
    exposures: Vec<Result<CycleExposure, ProjectError>>,
}
impl PreparedGrid {
    fn new(query: &StockQuery, materials: Vec<String>) -> Result<Self, String> {
        let recipe = LayerCycle {
            slots: vec![
                OpticalSlot {
                    material: 0,
                    height_mm: query.slot_height_mm,
                },
                OpticalSlot {
                    material: 1,
                    height_mm: query.slot_height_mm,
                },
            ],
            phase_slot: 0,
            origin_z_mm: query.patch.origin_mm[2],
        };
        let request = query.request(materials, recipe);
        request.validate().map_err(|e| e.to_string())?;
        let exposures = (0..request.candidate_count().map_err(|e| e.to_string())?)
            .map(|index| {
                request
                    .candidate(index)
                    .and_then(|cycle| crate::optical::resolve_cycle(&cycle, &request.patch))
            })
            .collect();
        Ok(Self { request, exposures })
    }
}

pub fn compare(
    query: StockQuery,
    library: Option<LibraryIdentity>,
    candidates: Vec<StockCandidate>,
    cancelled: &dyn Fn() -> bool,
) -> Result<StockMatchReport, String> {
    query.validate()?;
    if candidates.len() > MAX_STOCK_CANDIDATES {
        return Err("stock exceeds 512 exact revisions; narrow the source scope".into());
    }
    for candidate in &candidates {
        candidate.validate()?;
    }
    let total_pairs = (0..candidates.len())
        .map(|a| {
            ((a + 1)..candidates.len())
                .filter(|&b| candidates[a].filament.id != candidates[b].filament.id)
                .count() as u32
        })
        .sum();
    let mut report = StockMatchReport {
        producer: STOCK_PRODUCER.into(),
        input_hash: StockMatchReport::input_hash(&query, &library, &candidates)?,
        query,
        library,
        candidates,
        singles: vec![],
        pairs: vec![],
        unassessed_singles: 0,
        unassessed_pairs: 0,
        total_pairs,
        visited_pairs: 0,
        evaluations: 0,
        status: StockSearchStatus::Complete,
    };
    if cancelled() {
        report.status = StockSearchStatus::Cancelled;
        return Ok(report);
    }
    let target = crate::color_to_d65_lab(report.query.target.color).map_err(|e| e.to_string())?;
    let mut bases = Vec::with_capacity(report.candidates.len());
    for (i, candidate) in report.candidates.iter().enumerate() {
        if cancelled() {
            report.status = StockSearchStatus::Cancelled;
            return Ok(report);
        }
        let row = candidate.palette_row(format!("{}/declared", candidate.filament.id.0));
        bases.push(PreparedMaterial {
            admission: row.validate(),
            basis: crate::optical::base(&row.evidence),
        });
        match candidate.appearance.as_ref().map(|a| a.color) {
            Some(color) => {
                let lab = crate::color_to_d65_lab(color).map_err(|e| e.to_string())?;
                let delta = crate::ciede2000(lab, target).map_err(|e| e.to_string())?;
                report.singles.push(StockSingle {
                    candidate: i as u16,
                    delta_e00: delta,
                    within_tolerance: delta <= report.query.target.delta_e00_tolerance
                        && report
                            .query
                            .target
                            .lightness_tolerance
                            .is_none_or(|v| (lab[0] - target[0]).abs() <= v),
                });
            }
            None => report.unassessed_singles += 1,
        }
    }
    report.singles.sort_by(|a, b| {
        a.delta_e00
            .total_cmp(&b.delta_e00)
            .then(a.candidate.cmp(&b.candidate))
    });
    report.singles.truncate(MAX_STOCK_RESULTS);
    let mut grid = None;
    'pairs: for a in 0..report.candidates.len() {
        for b in (a + 1)..report.candidates.len() {
            if report.candidates[a].filament.id == report.candidates[b].filament.id {
                continue;
            }
            if cancelled() {
                report.status = StockSearchStatus::Cancelled;
                break 'pairs;
            }
            // Admit a complete pair grid. An exhausted budget never conceals a partially evaluated pair.
            if report.evaluations + 32 > report.query.max_evaluations {
                report.status = StockSearchStatus::WorkLimit;
                break 'pairs;
            }
            // Palette row admission still belongs to the first visited pair
            // containing that material, after cancellation and budget checks.
            bases[a].admission.as_ref().map_err(|e| e.to_string())?;
            bases[b].admission.as_ref().map_err(|e| e.to_string())?;
            if grid.is_none() {
                grid = Some(PreparedGrid::new(
                    &report.query,
                    vec![
                        format!("{}/declared", report.candidates[a].filament.id.0),
                        format!("{}/declared", report.candidates[b].filament.id.0),
                    ],
                )?);
            }
            let grid = grid.as_ref().expect("admitted grid is prepared");
            let pair_bases = [
                bases[a].basis,
                bases[b].basis,
                Err(OpticalUnknownReason::MissingEvidence),
            ];
            // The requested cycle is also the first grid candidate. Preserve
            // its pre-cancellation numerical admission, then reuse its result.
            let requested = crate::optical::assess(
                *grid.exposures[0].as_ref().map_err(|e| e.to_string())?,
                &pair_bases,
                &grid.request.observation,
                &report.query.target,
                target,
            )
            .map_err(|e| e.to_string())?;
            let mut best: Option<(usize, f64, OpticalAssessment)> = None;
            for (index, exposure) in grid.exposures.iter().enumerate() {
                if cancelled() {
                    report.status = StockSearchStatus::Cancelled;
                    break 'pairs;
                }
                let assessment = if index == 0 {
                    requested.clone()
                } else {
                    crate::optical::assess(
                        *exposure.as_ref().map_err(|e| e.to_string())?,
                        &pair_bases,
                        &grid.request.observation,
                        &report.query.target,
                        target,
                    )
                    .map_err(|e| e.to_string())?
                };
                report.evaluations += 1;
                if let OpticalAssessment::Estimate { delta_e00, .. } = &assessment
                    && best
                        .as_ref()
                        .is_none_or(|(_, previous, _)| delta_e00.total_cmp(previous).is_lt())
                {
                    best = Some((index, *delta_e00, assessment));
                }
            }
            report.visited_pairs += 1;
            if let Some((index, _, assessment)) = best {
                report.pairs.push(StockPair {
                    candidates: [a as u16, b as u16],
                    cycle: grid
                        .request
                        .candidate(index as u32)
                        .map_err(|e| e.to_string())?,
                    assessment,
                });
                let score = |p: &StockPair| match p.assessment {
                    OpticalAssessment::Estimate { delta_e00, .. } => delta_e00,
                    _ => f64::INFINITY,
                };
                report.pairs.sort_by(|a, b| {
                    score(a)
                        .total_cmp(&score(b))
                        .then(a.candidates.cmp(&b.candidates))
                });
                report.pairs.truncate(MAX_STOCK_RESULTS);
            } else {
                report.unassessed_pairs += 1;
            }
        }
    }
    report.validate()?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiling_contracts::{appearance::AppearanceColor, geometry::SourceHash, library::*};
    fn candidate(n: u8, rgb: Option<[f64; 3]>) -> StockCandidate {
        StockCandidate {
            filament: FilamentRef {
                id: FilamentId(format!("00000000-0000-4000-a000-{n:012}")),
                revision: SourceHash::from_bytes(&[n]),
            },
            name: format!("Filament {n}"),
            appearance: rgb.map(|channels| DeclaredAppearance {
                color: AppearanceColor::Srgb { channels },
                source: DefinitionSource::Operator,
            }),
            hueforge_td_mm: None,
            in_project: true,
            in_library: false,
            on_hand_spools: 0,
            profile_count: 0,
        }
    }
    fn query() -> StockQuery {
        StockQuery {
            scope: StockScope::Project,
            on_hand_only: false,
            target: OpticalTarget {
                color: AppearanceColor::Srgb { channels: [0.0; 3] },
                delta_e00_tolerance: 2.0,
                lightness_tolerance: None,
            },
            patch: OpticalPatch {
                origin_mm: [0.0; 3],
                normal_xy: [0.0, -1.0],
                width_mm: 10.0,
                height_mm: 10.0,
            },
            slot_height_mm: 0.2,
            max_evaluations: 65536,
        }
    }
    #[test]
    fn ranks_exact_singles_and_complete_pair_grid_without_promoting_unknowns() {
        let rows = vec![
            candidate(1, Some([0.0; 3])),
            candidate(2, Some([1.0; 3])),
            candidate(3, None),
        ];
        let report = compare(query(), None, rows, &|| false).unwrap();
        assert_eq!(report.status, StockSearchStatus::Complete);
        assert_eq!(report.singles[0].candidate, 0);
        assert!(report.singles[0].delta_e00 < 1e-12);
        assert_eq!(report.unassessed_singles, 1);
        assert_eq!(report.visited_pairs, 3);
        assert_eq!(report.total_pairs, 3);
        assert_eq!(report.evaluations, 96);
        assert_eq!(report.unassessed_pairs, 2);
        assert_eq!(report.pairs.len(), 1);
        let bytes = report.encode().unwrap();
        assert_eq!(StockMatchReport::decode(&bytes).unwrap(), report);
    }
    #[test]
    fn twenty_products_are_searched_without_a_nearest_color_shortlist() {
        let rows = (1..=20)
            .map(|n| candidate(n, Some([f64::from(n) / 20.0, 0.2, 0.8])))
            .collect();
        let report = compare(query(), None, rows, &|| false).unwrap();
        assert_eq!(report.status, StockSearchStatus::Complete);
        assert_eq!(report.visited_pairs, 190);
        assert_eq!(report.evaluations, 6080);
        assert_eq!(report.singles.len(), MAX_STOCK_RESULTS);
        assert_eq!(report.pairs.len(), MAX_STOCK_RESULTS);
        assert_eq!(report.unassessed_pairs, 0);
        assert!(
            report
                .pairs
                .windows(2)
                .all(|p| match (&p[0].assessment, &p[1].assessment) {
                    (
                        OpticalAssessment::Estimate { delta_e00: a, .. },
                        OpticalAssessment::Estimate { delta_e00: b, .. },
                    ) => a <= b,
                    _ => false,
                })
        );
    }
    #[test]
    fn work_limit_and_cancellation_are_incomplete_and_same_product_revisions_do_not_pair() {
        let rows = vec![
            candidate(1, Some([0.0; 3])),
            candidate(2, Some([1.0; 3])),
            candidate(3, Some([0.5; 3])),
        ];
        let mut q = query();
        q.max_evaluations = 32;
        let report = compare(q, None, rows.clone(), &|| false).unwrap();
        assert_eq!(report.status, StockSearchStatus::WorkLimit);
        assert_eq!(report.visited_pairs, 1);
        let cancelled = compare(query(), None, rows.clone(), &|| true).unwrap();
        assert_eq!(cancelled.status, StockSearchStatus::Cancelled);
        assert_eq!(cancelled.evaluations, 0);
        let mut revision = rows[0].clone();
        revision.filament.revision = SourceHash::from_bytes(b"new evidence");
        let report = compare(query(), None, vec![rows[0].clone(), revision], &|| false).unwrap();
        assert_eq!(report.total_pairs, 0);
        assert!(report.pairs.is_empty());
    }
}
