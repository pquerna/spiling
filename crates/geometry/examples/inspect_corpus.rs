// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Native-only smoke surface. No engine session or occurrence semantics are simulated.
use serde_json::json;
use spiling_geometry::{
    DisplayProfile, NativeSection, PlaneMm, import_step, inspect_face, section, tessellate,
};
use std::{path::PathBuf, sync::atomic::AtomicBool, time::Instant};

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(x, y)| x * y).sum()
}
fn subtract(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}

fn section_numerics(result: &NativeSection) -> serde_json::Value {
    let loops: Vec<_> = result.loops.iter().map(|boundary| {
        let area = boundary.points_mm.windows(2).map(|pair| {
            let a = subtract(pair[0], result.frame.origin_mm);
            let b = subtract(pair[1], result.frame.origin_mm);
            dot(a,result.frame.x_axis)*dot(b,result.frame.y_axis)-dot(b,result.frame.x_axis)*dot(a,result.frame.y_axis)
        }).sum::<f64>() * 0.5;
        let residual = boundary.points_mm.iter().map(|point| dot(subtract(*point,result.frame.origin_mm),result.frame.z_axis).abs()).fold(0.0_f64,f64::max);
        let closure = boundary.points_mm.first().zip(boundary.points_mm.last()).map(|(first,last)| dot(subtract(*first,*last),subtract(*first,*last)).sqrt());
        json!({"is_hole":boundary.is_hole,"points":boundary.points_mm.len(),"signed_area_mm2":area,"maximum_plane_residual_mm":residual,"closure_mm":closure})
    }).collect();
    json!({"plane":result.plane,"frame":result.frame,"sampling_tolerance_mm":result.sampling_tolerance_mm,"boolean_tolerance_mm":result.boolean_tolerance_mm,"loops":loops})
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut mesh_only = false;
    let mut paths = Vec::<PathBuf>::new();
    for argument in std::env::args_os().skip(1) {
        if argument == "--mesh-only" {
            mesh_only = true;
        } else {
            paths.push(argument.into());
        }
    }
    if paths.is_empty() {
        return Err("usage: inspect_corpus [--mesh-only] <original.step>...".into());
    }
    let cancel = AtomicBool::new(false);
    for path in paths {
        let bytes = std::fs::read(&path)?;
        let started = Instant::now();
        let definition = import_step(&bytes, &cancel)?;
        let import_us = started.elapsed().as_micros();
        let started = Instant::now();
        let mesh = tessellate(&definition, DisplayProfile::MeshMm005V1, &cancel)?;
        let mesh_us = started.elapsed().as_micros();
        let chunks: Vec<_> = mesh.chunks.iter().map(|chunk| json!({"bytes":chunk.bytes.len(),"sha256":chunk.descriptor.sha256,"carrier_deviation_mm":chunk.descriptor.carrier_deviation_mm,"quantization_error_mm":chunk.descriptor.quantization_error_mm})).collect();
        let faces: Vec<_> = definition
            .faces()
            .iter()
            .map(|face| inspect_face(&definition, &face.face_id))
            .collect::<Result<_, _>>()?;
        let (native_section, section_us) = if mesh_only {
            (None, None)
        } else {
            let bounds = definition.bounds_mm();
            let started = Instant::now();
            let result = section(
                &definition,
                PlaneMm {
                    origin_mm: [0.0, 0.0, (bounds.min[2] + bounds.max[2]) * 0.5],
                    normal: [0.0, 0.0, 1.0],
                },
                &cancel,
            )?;
            (
                Some(section_numerics(&result)),
                Some(started.elapsed().as_micros()),
            )
        };
        println!(
            "{}",
            json!({"source":path,"source_bytes":bytes.len(),"definition_id":definition.id(),"provenance":definition.provenance(),"bounds_mm":definition.bounds_mm(),"faces":faces,"mesh":{"vertices":mesh.vertex_count,"triangles":mesh.triangle_count,"total_bytes":mesh.total_bytes,"chunks":chunks},"section":native_section,"timing_us":{"import":import_us,"tessellate":mesh_us,"section":section_us}})
        );
    }
    Ok(())
}
