// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Repeatable import/retained-return measurements through the geometry facade.
use spiling_contracts::geometry::*;
use spiling_geometry::{
    source::{SurfaceComplex, decode_container},
    tolerant::{TolerantModel, TolerantPolicy},
};
use std::{sync::atomic::AtomicBool, time::Instant};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    for path in std::env::args().skip(1) {
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
            let packed = source.complex.encode()?;
            let restored = SurfaceComplex::decode(&packed)?;
            let bounds = restored.bounds()?;
            let z = (bounds.min[2] + bounds.max[2]) * 0.5;
            let section =
                TolerantModel::from_complex(&restored, TolerantPolicy::default())?.section(z)?;
            println!(
                "{}",
                serde_json::json!({"path":path,"decode_seconds":elapsed,"input_bytes":bytes.len(),"retained_bytes":restored.retained_bytes(),"packed_bytes":packed.len(),"facets":restored.triangles.len(),"instances":source.placements.len(),"section_z_mm":z,"section_area_mm2":section.loops.iter().map(|l|l.area).sum::<f64>(),"findings":source.findings})
            );
        }
    }
    Ok(())
}
