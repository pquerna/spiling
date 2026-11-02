// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Run with `cargo bench -p spiling-appearance --bench stock`.
//! Timings exclude input cloning and report encoding. Hashes retain complete
//! output identity; this benchmark does not impose machine-dependent thresholds.
use sha2::{Digest, Sha256};
use spiling_contracts::{
    appearance::AppearanceColor,
    geometry::SourceHash,
    library::{DeclaredAppearance, DefinitionSource, FilamentId, FilamentRef},
    optical::{OpticalPatch, OpticalTarget},
    stock::{StockCandidate, StockQuery, StockScope},
};
use std::{hint::black_box, time::Instant};

fn candidates(count: u32) -> Vec<StockCandidate> {
    (1..=count)
        .map(|n| StockCandidate {
            filament: FilamentRef {
                id: FilamentId(format!("00000000-0000-4000-a000-{n:012}")),
                revision: SourceHash::from_bytes(&n.to_le_bytes()),
            },
            name: format!("Synthetic filament {n}"),
            appearance: Some(DeclaredAppearance {
                color: AppearanceColor::Srgb {
                    channels: [n * 37, n * 71 + 11, n * 137 + 255]
                        .map(|v| f64::from(v % 256) / 255.0),
                },
                source: DefinitionSource::Operator,
            }),
            hueforge_td_mm: None,
            in_project: true,
            in_library: false,
            on_hand_spools: 0,
            profile_count: 0,
        })
        .collect()
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
            lightness_tolerance: Some(5.0),
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
fn main() {
    let repeats = std::env::var("SPILING_STOCK_BENCH_REPEATS")
        .map(|v| v.parse::<usize>().expect("integer repeat count"))
        .unwrap_or(5);
    assert!((1..=100).contains(&repeats));
    for count in [20, 64, 512] {
        let inputs = candidates(count);
        let query = query();
        let expected =
            spiling_appearance::stock::compare(query.clone(), None, inputs.clone(), &|| false)
                .unwrap();
        let mut times = Vec::with_capacity(repeats);
        for _ in 0..repeats {
            let inputs = inputs.clone();
            let query = query.clone();
            let start = Instant::now();
            let report = spiling_appearance::stock::compare(
                black_box(query),
                None,
                black_box(inputs),
                &|| false,
            )
            .unwrap();
            times.push(start.elapsed().as_micros());
            assert_eq!(report, expected);
            black_box(report);
        }
        times.sort_unstable();
        println!(
            "{}",
            serde_json::json!({
                "candidates": count,
                "repeats": repeats,
                "median_us": times[times.len()/2],
                "minimum_us": times[0],
                "evaluations": expected.evaluations,
                "visited_pairs": expected.visited_pairs,
                "total_pairs": expected.total_pairs,
                "status": expected.status,
                "report_sha256": format!("{:x}", Sha256::digest(expected.encode().unwrap())),
            })
        );
    }
}
