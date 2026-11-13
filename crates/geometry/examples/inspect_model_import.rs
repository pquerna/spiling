// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Repeatable import/retained-return measurements through the geometry facade.
use sha2::{Digest, Sha256};
use spiling_contracts::geometry::*;
use spiling_geometry::{
    GeometryWorkBudget, MeshBudget,
    source::{HybridDefinition, SourceDefinition, SurfaceComplex, decode_container},
    tolerant::{TolerantModel, TolerantPolicy},
};
use std::{sync::atomic::AtomicBool, time::Instant};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let include_display = args.iter().any(|arg| arg == "--display");
    for path in args.into_iter().filter(|arg| arg != "--display") {
        let bytes = std::fs::read(&path)?;
        let started = Instant::now();
        let format = if path.to_ascii_lowercase().ends_with(".stl") {
            MeshSourceFormat::Stl
        } else {
            MeshSourceFormat::ThreeMf
        };
        let sources = decode_container(
            &bytes,
            &path,
            format,
            usize::MAX,
            &ModelImportOptions::default(),
            &AtomicBool::new(false),
        )?;
        let elapsed = started.elapsed().as_secs_f64();
        for source in sources {
            let pack_started = Instant::now();
            let packed = source.complex.encode()?;
            let pack_seconds = pack_started.elapsed().as_secs_f64();
            let restore_started = Instant::now();
            let restored = SurfaceComplex::decode(&packed)?;
            let restore_seconds = restore_started.elapsed().as_secs_f64();
            let section_started = Instant::now();
            let bounds = restored.bounds()?;
            let z = (bounds.min[2] + bounds.max[2]) * 0.5;
            let section =
                TolerantModel::from_complex(&restored, TolerantPolicy::default())?.section(z)?;
            let section_seconds = section_started.elapsed().as_secs_f64();
            let total_seconds = started.elapsed().as_secs_f64();
            let packed_sha256 = SourceHash::from_digest(&Sha256::digest(&packed).into());
            let retained_bytes = restored.retained_bytes();
            let facets = restored.triangles.len();
            let faces = restored.faces.len();
            let mut families = std::collections::BTreeMap::<String, usize>::new();
            for face in &restored.faces {
                *families
                    .entry(format!("{:?}", face.geometry.family))
                    .or_default() += 1;
            }
            let display = if include_display {
                let provenance = SourceProvenance {
                    source_hash: SourceHash::from_bytes(&bytes),
                    source_name: path.clone(),
                    source_unit: source.source_unit,
                    uncertainty_mm: None,
                    source_solid: None,
                    normalized: Some(NormalizedSource {
                        hash: packed_sha256.clone(),
                        format: source.format,
                        selector: source.selector,
                        object_name: source.name,
                        findings: source.findings.clone(),
                    }),
                };
                let definition = SourceDefinition::Hybrid(HybridDefinition {
                    id: provenance.definition_id(),
                    provenance,
                    bounds,
                    complex: restored,
                });
                let display_started = Instant::now();
                let mesh = definition.display(
                    DisplayProfile::MeshTrimmedMm005V2,
                    &AtomicBool::new(false),
                    MeshBudget::FROZEN,
                    &mut GeometryWorkBudget::new(u64::MAX),
                    &mut |_| {},
                )?;
                Some(
                    serde_json::json!({"seconds":display_started.elapsed().as_secs_f64(),
                    "chunks":mesh.chunks.iter().map(|c| serde_json::json!({
                        "sha256":c.descriptor.sha256,"bytes":c.bytes.len(),
                        "bounds":c.descriptor.bounds_mm,
                    })).collect::<Vec<_>>() }),
                )
            } else {
                None
            };
            println!(
                "{}",
                serde_json::json!({"path":path,"decode_seconds":elapsed,"pack_seconds":pack_seconds,"restore_seconds":restore_seconds,"section_seconds":section_seconds,"total_seconds":total_seconds,"display":display,"packed_sha256":packed_sha256,"input_bytes":bytes.len(),"retained_bytes":retained_bytes,"packed_bytes":packed.len(),"facets":facets,"faces":faces,"families":families,"instances":source.placements.len(),"section_z_mm":z,"section_area_mm2":section.loops.iter().map(|l|l.area).sum::<f64>(),"findings":source.findings})
            );
        }
    }
    Ok(())
}
