// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_contracts::{appearance::*, geometry::SourceHash, library::*, optical::*, stock::*};
use std::{cell::Cell, sync::Arc};

fn candidate(n: u8, color: Option<AppearanceColor>) -> StockCandidate {
    StockCandidate {
        filament: FilamentRef {
            id: FilamentId(format!("00000000-0000-4000-a000-{n:012}")),
            revision: SourceHash::from_bytes(&[n]),
        },
        name: format!("Synthetic {n}"),
        appearance: color.map(|color| DeclaredAppearance {
            color,
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
            color: AppearanceColor::Srgb {
                channels: [0.4, 0.5, 0.8],
            },
            delta_e00_tolerance: 2.0,
            lightness_tolerance: Some(1.0),
        },
        patch: OpticalPatch {
            origin_mm: [0.0, 0.0, 0.13],
            normal_xy: [0.0, -1.0],
            width_mm: 10.0,
            height_mm: 10.13,
        },
        slot_height_mm: 0.2,
        max_evaluations: 65536,
    }
}

/// Independently compose full general optical reports, retaining the original
/// checked palette/request path rather than the optimized stock grid.
fn general_pair(
    query: &StockQuery,
    candidates: &[StockCandidate],
    a: usize,
    b: usize,
) -> Option<StockPair> {
    let keys = [a, b].map(|i| format!("{}/declared", candidates[i].filament.id.0));
    let palette = PaletteDocument {
        name: "Synthetic parity control".into(),
        source_url: "https://example.com/stock-parity".into(),
        source_revision: "synthetic-1".into(),
        license: "OSL-3.0".into(),
        conversion: PALETTE_CONVERSION.into(),
        rows: vec![
            candidates[a].palette_row(keys[0].clone()),
            candidates[b].palette_row(keys[1].clone()),
        ],
    };
    let prepared =
        PreparedPalette::new(palette.clone(), &palette.canonical_bytes().unwrap()).unwrap();
    let request = query.request(
        keys.into(),
        LayerCycle {
            slots: [0, 1]
                .map(|material| OpticalSlot {
                    material,
                    height_mm: query.slot_height_mm,
                })
                .into(),
            phase_slot: 0,
            origin_z_mm: query.patch.origin_mm[2],
        },
    );
    let report =
        spiling_appearance::optical::compare(&prepared, Arc::new(request), &|| false).unwrap();
    assert!(report.is_complete());
    report.alternatives.first().map(|&index| StockPair {
        candidates: [a as u16, b as u16],
        cycle: report.request.candidate(index).unwrap(),
        assessment: report.visits[index as usize].assessment.clone(),
    })
}
fn score(pair: &StockPair) -> f64 {
    let OpticalAssessment::Estimate { delta_e00, .. } = pair.assessment else {
        panic!("estimate required")
    };
    delta_e00
}

#[test]
fn prepared_search_matches_general_studies_for_finite_windows_unknowns_and_ties() {
    let srgb = |channels| Some(AppearanceColor::Srgb { channels });
    let mut candidates = vec![
        candidate(1, srgb([0.0; 3])),
        candidate(2, srgb([0.0; 3])),
        candidate(3, srgb([1.0; 3])),
        candidate(4, srgb([0.1, 0.4, 0.8])),
        candidate(5, srgb([0.12345, 0.98765, 0.33333])),
        candidate(6, None),
        candidate(
            7,
            Some(AppearanceColor::D65Lab {
                channels: [51.0, 19.0, -40.0],
            }),
        ),
        candidate(
            8,
            Some(AppearanceColor::XyzY1 {
                channels: [0.4, 0.7, 0.9],
            }),
        ),
    ];
    let mut another_revision = candidates[0].clone();
    another_revision.filament.revision = SourceHash::from_bytes(b"another exact revision");
    candidates.push(another_revision);
    for (height, slot, budget) in [
        (10.13, 0.2, 65536),
        (0.005, 1.0, 65536),
        (0.37, 0.01, 65536),
        (1.01, 0.1, 95),
    ] {
        let mut query = query();
        query.patch.height_mm = height;
        query.slot_height_mm = slot;
        query.max_evaluations = budget;
        let actual =
            spiling_appearance::stock::compare(query.clone(), None, candidates.clone(), &|| false)
                .unwrap();
        let eligible: Vec<_> = (0..candidates.len())
            .flat_map(|a| ((a + 1)..candidates.len()).map(move |b| (a, b)))
            .filter(|&(a, b)| candidates[a].filament.id != candidates[b].filament.id)
            .collect();
        let mut expected = Vec::new();
        let mut unknown = 0;
        for &(a, b) in eligible.iter().take((budget / 32) as usize) {
            match general_pair(&query, &candidates, a, b) {
                Some(pair) => expected.push(pair),
                None => unknown += 1,
            }
        }
        expected.sort_by(|a, b| {
            score(a)
                .total_cmp(&score(b))
                .then(a.candidates.cmp(&b.candidates))
        });
        expected.truncate(MAX_STOCK_RESULTS);
        assert_eq!(actual.pairs, expected, "height {height}, slot {slot}");
        assert_eq!(actual.unassessed_pairs, unknown);
        assert_eq!(
            actual.visited_pairs as usize,
            eligible.len().min((budget / 32) as usize)
        );
        assert_eq!(actual.evaluations, actual.visited_pairs * 32);
        assert_eq!(
            StockMatchReport::decode(&actual.encode().unwrap()).unwrap(),
            actual
        );
    }
}

#[test]
fn cancellation_does_not_publish_a_partially_evaluated_pair() {
    let candidates: Vec<_> = (1..=3)
        .map(|n| {
            candidate(
                n,
                Some(AppearanceColor::Srgb {
                    channels: [f64::from(n) / 3.0; 3],
                }),
            )
        })
        .collect();
    for partial in [0, 1, 17, 31] {
        let calls = Cell::new(0);
        // Initial check + three singles + first pair admission + 32 visits +
        // second pair admission. Stop partway through the second pair.
        let limit = 38 + partial;
        let actual = spiling_appearance::stock::compare(query(), None, candidates.clone(), &|| {
            let call = calls.get();
            calls.set(call + 1);
            call == limit
        })
        .unwrap();
        assert_eq!(actual.status, StockSearchStatus::Cancelled);
        assert_eq!(actual.visited_pairs, 1);
        assert_eq!(actual.evaluations, 32 + partial);
        assert_eq!(actual.pairs.len(), 1);
        assert_eq!(actual.pairs[0].candidates, [0, 1]);
        actual.validate().unwrap();
    }
}

#[test]
fn unvisited_grid_refusals_do_not_override_cancellation_or_work_limit() {
    let candidates = vec![candidate(1, None), candidate(2, None)];
    let mut query = query();
    query.patch.origin_mm[2] = 0.0;
    query.patch.height_mm = 1e20;
    let cancelled =
        spiling_appearance::stock::compare(query.clone(), None, candidates.clone(), &|| true)
            .unwrap();
    assert_eq!(cancelled.status, StockSearchStatus::Cancelled);
    query.max_evaluations = 1;
    let limited =
        spiling_appearance::stock::compare(query.clone(), None, candidates.clone(), &|| false)
            .unwrap();
    assert_eq!(limited.status, StockSearchStatus::WorkLimit);
    assert_eq!(limited.evaluations, 0);
    query.max_evaluations = 32;
    assert!(spiling_appearance::stock::compare(query, None, candidates, &|| false).is_err());
}

#[test]
fn prepared_materials_preserve_palette_row_admission_at_the_pair_boundary() {
    let mut rows = vec![candidate(1, None), candidate(2, None)];
    rows[1].name = " Noncanonical appearance name".into();
    rows[1].validate().unwrap();
    let mut query = query();
    query.max_evaluations = 1;
    let limited =
        spiling_appearance::stock::compare(query.clone(), None, rows.clone(), &|| false).unwrap();
    assert_eq!(limited.status, StockSearchStatus::WorkLimit);
    query.max_evaluations = 32;
    assert!(
        spiling_appearance::stock::compare(query, None, rows, &|| false)
            .unwrap_err()
            .contains("noncanonical")
    );
}
