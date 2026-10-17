// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_geometry::{GeometryWorkBudget, PlacedSlabRequest, SlabRequest, PlaneFrameMm, SectionLoopMm,
    SourceLeafRequest, NativeUnionPolicy, import_step, prepare_material_source,
    certify_native_source_leaf, MAX_PREPARED_SECTION_BYTES, MAX_PREPARED_SECTION_SOURCE_BYTES};
use spiling_contracts::geometry::{OccurrenceId, RigidPoseMm};
use std::{sync::atomic::AtomicBool, time::Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let path = args.first().ok_or("usage: parity_probe SOURCE [LOWER UPPER SAMPLE WORK_LIMIT]")?;
    let number = |i: usize, default: f64| -> Result<f64, Box<dyn std::error::Error>> {
        Ok(args.get(i).map(|text| text.parse()).transpose()?.unwrap_or(default))
    };
    let interval = [number(1, 0.1)?, number(2, 0.2)?];
    let sample = number(3, 0.15)?;
    let cancel = AtomicBool::new(false);
    let definition = import_step(&std::fs::read(path)?, &cancel)?;
    let limit = args.get(4).map(|text| text.parse::<u64>()).transpose()?.unwrap_or(20_000_000);
    let mut work = GeometryWorkBudget::new(limit.min(20_000_000));
    let start = Instant::now();
    let source = prepare_material_source(&definition, MAX_PREPARED_SECTION_SOURCE_BYTES,
        &cancel, &mut work)?;
    eprintln!("prepared source bytes={} work={} elapsed={:?}", source.retained_bytes(),
        work.consumed(), start.elapsed());
    let before = work.consumed();
    let raw = args.get(5).is_some_and(|mode| mode == "raw");
    let slab = if raw {
        source.analyze_slab(SlabRequest {
            frame: PlaneFrameMm { origin_mm: [0.0; 3], x_axis: [1.0, 0.0, 0.0],
                y_axis: [0.0, 1.0, 0.0], z_axis: [0.0, 0.0, 1.0] },
            height_interval_mm: interval, sample_height_mm: sample, native_shape_error_mm: 0.1,
        }, MAX_PREPARED_SECTION_BYTES - source.retained_bytes(), &mut work, &cancel)
    } else {
        source.analyze_placed_slab(&PlacedSlabRequest {
            pose: RigidPoseMm::IDENTITY, world_height_interval_mm: interval,
            sample_world_z_mm: sample, native_shape_error_mm: 0.1,
        }, MAX_PREPARED_SECTION_BYTES - source.retained_bytes(), &mut work, &cancel)
    };
    eprintln!("slab work={} total={} elapsed={:?}", work.consumed()-before,
        work.consumed(), start.elapsed());
    let slab = slab?;
    println!("closed={:?} transition={} collar={} loops={} retained={}",
        slab.closed_shape_error_mm(), slab.has_transition_material(), slab.has_critical_collar(),
        slab.section().loops().len(), slab.retained_bytes());
    if raw { return Ok(()); }
    let actual: Vec<_> = slab.section().loops().iter().map(|ring| SectionLoopMm {
        is_hole: ring.is_hole,
        points_mm: ring.points_mm.iter().map(|p| [p[0], p[1], sample]).collect(),
    }).collect();
    let leaf = certify_native_source_leaf(&SourceLeafRequest {
        definition: &definition, occurrence_id: &OccurrenceId::new(1).unwrap(),
        pose: RigidPoseMm::IDENTITY, analysis: &slab, actual_used_source_loops: &actual,
        policy: NativeUnionPolicy { candidate_coordinate_error_mm: 0.0 },
    }, &mut work, &cancel)?;
    println!("leaf={:?} closed={:?} total_work={} elapsed={:?}", leaf.proof_kind(),
        leaf.closed_shape_error_mm(), work.consumed(), start.elapsed());
    println!("leaf_evidence={leaf:?}");
    Ok(())
}
