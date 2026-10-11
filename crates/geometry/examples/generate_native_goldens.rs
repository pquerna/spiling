// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Native public-facade goldens; kept separate from cheap kernel-independent vectors.
use serde_json::{Value, json};
use spiling_contracts::{display::SectionChunkBuilder, geometry::*};
use spiling_geometry::{import_step, section, tessellate};
use std::{error::Error, path::Path, sync::atomic::AtomicBool};

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut result, "{byte:02x}").expect("String write");
    }
    result
}
fn store(root: &Path, name: &str, value: &Value, check: bool) -> Result<(), Box<dyn Error>> {
    let output = format!("{}\n", serde_json::to_string_pretty(value)?);
    let path = root.join(name);
    let notice = "SPDX-FileCopyrightText: 2026 Spiling contributors\nSPDX-License-Identifier: OSL-3.0\nLicensed under the Open Software License version 3.0\n\nOriginal Spiling native-derived interoperability fixture; no external CAD source.\n";
    if check {
        if std::fs::read_to_string(&path)? != output
            || std::fs::read_to_string(root.join(format!("{name}.license")))? != notice
        {
            return Err(format!("native golden drift: {}", path.display()).into());
        }
    } else {
        std::fs::write(path, output)?;
        std::fs::write(root.join(format!("{name}.license")), notice)?;
    }
    Ok(())
}
fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if !arguments.is_empty() && arguments != ["--check"] {
        return Err("usage: generate_native_goldens [--check]".into());
    }
    let check = !arguments.is_empty();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let protocol = root.join("protocol");
    let cancel = AtomicBool::new(false);
    let session = SessionId::parse("550e8400-e29b-41d4-a716-446655440000")?;
    let box_definition = import_step(&std::fs::read(root.join("geometry/box-mm.step"))?, &cancel)?;
    let mesh = tessellate(&box_definition, DisplayProfile::MeshMm005V1, &cancel)?;
    store(
        &protocol,
        "native-mesh.json",
        &json!({
            "encoding": "hex", "purpose": "Native six-face box SPLM from the public geometry facade.",
            "source_hash": box_definition.provenance().source_hash,
            "face_table": box_definition.faces().iter().enumerate().map(|(ordinal, face)| FaceIndexRow {ordinal:ordinal as u32, face_id:face.face_id.clone()}).collect::<Vec<_>>(),
            "vertex_count":mesh.vertex_count, "triangle_count":mesh.triangle_count, "total_bytes":mesh.total_bytes,
            "chunks":mesh.chunks.into_iter().map(|chunk|json!({"metadata":chunk.descriptor.into_metadata(session.clone(), ArtifactId::new(1).expect("fixed artifact")),"data":hex(&chunk.bytes)})).collect::<Vec<_>>()
        }),
        check,
    )?;
    let definition = import_step(
        &std::fs::read(root.join("geometry/through-hole.step"))?,
        &cancel,
    )?;
    let plane = PlaneMm {
        origin_mm: [0.0, 0.0, 4.0],
        normal: [0.0, 0.0, 1.0],
    };
    let native = section(&definition, plane, &cancel)?;
    if native.loops.len() != 2 || native.loops[0].is_hole || !native.loops[1].is_hole {
        return Err("native through-hole topology changed".into());
    }
    for (name, artifact, cap) in [
        ("native-section.json", 2, MAX_GEOMETRY_CHUNK_BYTES),
        ("native-section-multichunk.json", 3, 1632),
    ] {
        let mut builder = SectionChunkBuilder::with_chunk_limit(
            session.clone(),
            ArtifactId::new(artifact)?,
            SceneRevision(1),
            plane,
            cap,
        )?;
        for boundary in &native.loops {
            builder.push_loop(
                OccurrenceId::new(1)?,
                definition.id().clone(),
                boundary.is_hole,
                &boundary.points_mm,
            )?;
        }
        let packed = builder.finish()?;
        store(
            &protocol,
            name,
            &json!({
                "encoding":"hex", "purpose":"Native radius-3 through-hole midplane; complete loops from the public geometry facade.",
                "source_hash":definition.provenance().source_hash,
                "summary":packed.summary, "loops":packed.loops,
                "chunks":packed.chunks.into_iter().map(|chunk|json!({"metadata":chunk.metadata,"data":hex(&chunk.bytes)})).collect::<Vec<_>>()
            }),
            check,
        )?;
    }
    println!(
        "native box mesh and through-hole single/multiple-chunk goldens {}",
        if check { "match" } else { "generated" }
    );
    Ok(())
}
