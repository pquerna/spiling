// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Diagnostic: import each STEP in a directory and take tolerant sections at
//! every 0.2 mm layer sample. Reports import outcome, sections that refuse,
//! joined gaps and time. Not a manufacturing result.
use spiling_geometry::tolerant::{TolerantModel, TolerantPolicy};
use std::{sync::atomic::AtomicBool, time::Instant};

fn main() {
    let dir = std::env::args()
        .nth(1)
        .expect("usage: tolerant_corpus STEP_DIR");
    let cancel = AtomicBool::new(false);
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let bytes = std::fs::read(&path).unwrap();
        let started = Instant::now();
        let definition = match spiling_geometry::from_step_bytes_with_cancel(&bytes, &cancel) {
            Ok(definition) => definition,
            Err(error) => {
                println!(
                    "{name:44} import refused: {}",
                    error.message.chars().take(90).collect::<String>()
                );
                continue;
            }
        };
        let model = match TolerantModel::build(&definition, TolerantPolicy::default(), &cancel) {
            Ok(model) => model,
            Err(error) => {
                println!("{name:44} tessellation refused: {}", error.message);
                continue;
            }
        };
        let built = started.elapsed();
        let bounds = definition.bounds_mm();
        let (mut ok, mut refused, mut gaps, mut max_gap, mut loops) = (0, 0, 0, 0.0f64, 0);
        let mut first_refusal = None;
        let mut z = bounds.min[2] + 0.1;
        while z < bounds.max[2] {
            match model.section(z) {
                Ok(section) => {
                    ok += 1;
                    gaps += section.joined_gaps;
                    max_gap = max_gap.max(section.max_gap_mm);
                    loops += section.loops.len();
                }
                Err(error) => {
                    refused += 1;
                    first_refusal.get_or_insert(error.message);
                }
            }
            z += 0.2;
        }
        println!(
            "{name:44} layers ok {ok:4} refused {refused:3} loops {loops:6} joined gaps {gaps:4} max gap {max_gap:.5} mm failed faces {} | mesh {:.2}s total {:.2}s {}",
            model.failed_faces,
            built.as_secs_f64(),
            started.elapsed().as_secs_f64(),
            first_refusal
                .map(|m| m.chars().take(100).collect::<String>())
                .unwrap_or_default()
        );
    }
}
