// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Native-only smoke surface. No engine session or occurrence semantics are simulated.
use serde_json::json;
use spiling_geometry::{
    BoundsPolicy, DisplayProfile, GeometryWorkBudget, NativeSection, PlaneMm,
    from_step_bytes_with_cancel, from_step_solid_with_cancel, inspect_face, projected_bounds,
    section, source_bodies, tessellate_with_work,
};
use std::{path::PathBuf, sync::atomic::AtomicBool, time::Instant};

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(x, y)| x * y).sum()
}
fn subtract(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}

fn section_numerics(result: &NativeSection) -> serde_json::Value {
    let loops: Vec<_> = result.loops().iter().map(|boundary| {
        let area = boundary.points_mm.windows(2).map(|pair| {
            let a = subtract(pair[0], result.frame().origin_mm);
            let b = subtract(pair[1], result.frame().origin_mm);
            dot(a,result.frame().x_axis)*dot(b,result.frame().y_axis)-dot(b,result.frame().x_axis)*dot(a,result.frame().y_axis)
        }).sum::<f64>() * 0.5;
        let residual = boundary.points_mm.iter().map(|point| dot(subtract(*point,result.frame().origin_mm),result.frame().z_axis).abs()).fold(0.0_f64,f64::max);
        let closure = boundary.points_mm.first().zip(boundary.points_mm.last()).map(|(first,last)| dot(subtract(*first,*last),subtract(*first,*last)).sqrt());
        json!({"is_hole":boundary.is_hole,"points":boundary.points_mm.len(),"signed_area_mm2":area,"maximum_plane_residual_mm":residual,"closure_mm":closure})
    }).collect();
    json!({"plane":result.plane(),"frame":result.frame(),"sampling_tolerance_mm":result.sampling_tolerance_mm(),"boolean_tolerance_mm":result.boolean_tolerance_mm(),"loops":loops})
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut mesh_only = false;
    let mut inventory_only = false;
    let mut import_only = false;
    let mut preview = false;
    let mut preparation_only = false;
    let mut selected_solid = None;
    let mut paths = Vec::<PathBuf>::new();
    let mut arguments = std::env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--mesh-only" {
            mesh_only = true;
        } else if argument == "--inventory" {
            inventory_only = true;
        } else if argument == "--import-only" {
            import_only = true;
        } else if argument == "--preview" {
            preview = true;
        } else if argument == "--preparation" {
            preparation_only = true;
        } else if argument == "--solid" {
            let entity = arguments
                .next()
                .ok_or("--solid requires a STEP entity number")?;
            selected_solid = Some(
                spiling_contracts::geometry::SourceSolidId::from_step_entity(
                    entity
                        .to_str()
                        .ok_or("solid entity must be UTF-8")?
                        .parse()?,
                )?,
            );
        } else if argument.to_string_lossy().starts_with("--") {
            return Err(format!("unknown option: {}", argument.to_string_lossy()).into());
        } else {
            paths.push(argument.into());
        }
    }
    if paths.is_empty() {
        return Err("usage: inspect_corpus [--inventory | --solid ENTITY] [--import-only | --mesh-only] [--preview] <original.step>...".into());
    }
    let cancel = AtomicBool::new(false);
    for path in paths {
        let bytes = std::fs::read(&path)?;
        let started = Instant::now();
        if inventory_only {
            let bodies = source_bodies(&bytes, &cancel)?;
            let entries: Vec<_> = bodies
                .iter()
                .map(|body| {
                    json!({
                        "solid": body.solid, "name": body.name, "placements": body.placements.len(),
                    })
                })
                .collect();
            println!(
                "{}",
                json!({"source": path, "source_bytes": bytes.len(),
                "bodies": entries, "inventory_us": started.elapsed().as_micros()})
            );
            continue;
        }
        let definition = match &selected_solid {
            Some(solid) => from_step_solid_with_cancel(&bytes, solid, &cancel)?,
            None => from_step_bytes_with_cancel(&bytes, &cancel)?,
        };
        let import_us = started.elapsed().as_micros();
        eprintln!(
            "{}",
            json!({"source": path, "solid": selected_solid,
            "stage": "import", "elapsed_us": import_us})
        );
        if import_only {
            println!(
                "{}",
                json!({"source":path,"source_bytes":bytes.len(),
                "definition_id":definition.id(),"provenance":definition.provenance(),
                "bounds_mm":definition.bounds_mm(),"face_count":definition.faces().len(),
                "timing_us":{"import":import_us}})
            );
            continue;
        }
        if preparation_only {
            let started = Instant::now();
            let definition = spiling_geometry::source::SourceDefinition::from(definition);
            let (model, allocation) =
                definition.prepare_with_budget(Default::default(), isize::MAX as usize, &cancel)?;
            let first_us = started.elapsed().as_micros();
            let repeated_at = Instant::now();
            let (repeated, reuse) =
                definition.prepare_with_budget(Default::default(), 0, &cancel)?;
            println!(
                "{}",
                json!({
                    "source":path,"definition_id":definition.id(),"provenance":definition.provenance(),
                    "face_count":definition.faces().len(),"failed_faces":model.failed_faces,
                    "open_edges":model.open_edges().len(),"conformed":{
                        "vertices":model.conformed.vertices,"max_offset_mm":model.conformed.max_offset_mm
                },"retained_bytes":model.retained_bytes(),"preparation_us":first_us,
                "allocated_bytes":allocation.allocated_bytes(), "reuse_allocated_bytes":reuse.allocated_bytes(),
                "shared_preparation":std::sync::Arc::ptr_eq(&model, &repeated),
                "reuse_us":repeated_at.elapsed().as_micros()
                })
            );
            continue;
        }
        let started = Instant::now();
        let profile = if preview {
            DisplayProfile::MeshTolerantMm005V1
        } else {
            DisplayProfile::MeshTrimmedMm005V2
        };
        let mesh = tessellate_with_work(
            &definition,
            profile,
            &cancel,
            spiling_contracts::display::MeshBudget::FROZEN,
            &mut GeometryWorkBudget::query(),
            &mut |update| {
                if preview {
                    eprintln!(
                        "{}",
                        json!({"source": path, "solid": selected_solid,
                        "stage": format!("{:?}", update.stage), "profile": profile,
                        "attempt": update.attempt, "completed_faces": update.completed_faces,
                        "total_faces": update.total_faces, "work": update.consumed_work,
                        "elapsed_us": started.elapsed().as_micros()})
                    );
                }
            },
        )?;
        let mesh_us = started.elapsed().as_micros();
        eprintln!(
            "{}",
            json!({"source": path, "solid": selected_solid,
            "stage": "display", "elapsed_us": mesh_us})
        );
        let chunks: Vec<_> = mesh.chunks.iter().map(|chunk| json!({"bytes":chunk.bytes.len(),"sha256":chunk.descriptor.sha256,"carrier_deviation_mm":chunk.descriptor.carrier_deviation_mm,"trim_deviation_mm":chunk.descriptor.trim_deviation_mm,"quantization_error_mm":chunk.descriptor.quantization_error_mm})).collect();
        let faces: Vec<_> = definition
            .faces()
            .iter()
            .map(|face| inspect_face(&definition, &face.face_id))
            .collect::<Result<_, _>>()?;
        let mut work = GeometryWorkBudget::new(1_000_000);
        let directional_bounds = if mesh_only {
            Vec::new()
        } else {
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
                .map(|direction| {
                    projected_bounds(
                        &definition,
                        direction,
                        BoundsPolicy {
                            maximum_endpoint_uncertainty_mm: 1e-6,
                        },
                        &mut work,
                        &cancel,
                    )
                })
                .into_iter()
                .map(|result| {
                    result.map(|bounds| {
                        json!({
                            "minimum": bounds.minimum,
                            "maximum": bounds.maximum,
                            "achieved_uncertainty_mm": bounds.achieved_uncertainty_mm,
                        })
                    })
                })
                .collect::<Result<Vec<_>, _>>()?
        };
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
            json!({"source":path,"source_bytes":bytes.len(),"definition_id":definition.id(),"provenance":definition.provenance(),"bounds_mm":definition.bounds_mm(),"directional_bounds":directional_bounds,"faces":faces,"mesh":{"vertices":mesh.vertex_count,"triangles":mesh.triangle_count,"total_bytes":mesh.total_bytes,"chunks":chunks},"section":native_section,"timing_us":{"import":import_us,"tessellate":mesh_us,"section":section_us}})
        );
    }
    Ok(())
}
