// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Tolerant-mesh health of STEP files, without slicing: per body, whether it
//! imports and meshes, the faces the mesher could not mesh within its rules,
//! open mesh edges (a face left to the lenient fill, or a crack between faces)
//! and whether its support voids project. With supports on, an open tolerant
//! mesh refuses the whole job, so any of these is a supports-on refusal.
//!
//! Usage: `mesh_health PATH...` (files, or directories of .step/.stp files).
//! One tab-separated line per body, then a summary; exits 1 when any body
//! refused or has an open mesh, 0 when every body is closed.
use spiling_geometry::tolerant::{TolerantModel, TolerantPolicy};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn step_files(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(path)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()))
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        entries.sort();
        for entry in entries {
            step_files(&entry, out);
        }
    } else if path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("step") || e.eq_ignore_ascii_case("stp"))
    {
        out.push(path.to_path_buf());
    }
}

fn main() {
    let mut files = Vec::new();
    for arg in std::env::args().skip(1) {
        step_files(Path::new(&arg), &mut files);
    }
    if files.is_empty() {
        eprintln!("usage: mesh_health PATH...");
        std::process::exit(2);
    }
    let cancel = AtomicBool::new(false);
    let (mut bodies, mut open_bodies, mut refused) = (0usize, 0usize, 0usize);
    println!("file\tbody\tstatus\tfailed_faces\topen_edges\tmesh_bytes\tseconds\tdetail");
    for path in &files {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(e) => {
                refused += 1;
                println!("{name}\t-\tunreadable\t\t\t\t\t{e}");
                continue;
            }
        };
        let solids = match spiling_geometry::source_bodies(&bytes, &cancel) {
            Ok(solids) => solids,
            Err(e) => {
                refused += 1;
                println!("{name}\t-\timport refused\t\t\t\t\t{}", e.message);
                continue;
            }
        };
        let single = solids.len() == 1;
        for body in &solids {
            bodies += 1;
            let started = Instant::now();
            let label = format!("{:?}", body.solid);
            let definition = if single {
                spiling_geometry::from_step_bytes_with_cancel(&bytes, &cancel)
            } else {
                spiling_geometry::from_step_solid_with_cancel(&bytes, &body.solid, &cancel)
            };
            let definition = match definition {
                Ok(definition) => definition,
                Err(e) => {
                    refused += 1;
                    println!("{name}\t{label}\timport refused\t\t\t\t\t{}", e.message);
                    continue;
                }
            };
            let model = match TolerantModel::build(&definition, TolerantPolicy::default(), &cancel)
            {
                Ok(model) => model,
                Err(e) => {
                    refused += 1;
                    println!("{name}\t{label}\tmesh refused\t\t\t\t\t{}", e.message);
                    continue;
                }
            };
            let open = model.open_edges().len();
            let voids = model.support_voids(&cancel);
            let seconds = started.elapsed().as_secs_f64();
            let (status, detail) = match (&voids, open, model.failed_faces) {
                (Ok(_), 0, 0) => ("closed", String::new()),
                (Ok(_), _, _) => ("open", String::new()),
                (Err(e), _, _) => ("voids refused", e.message.clone()),
            };
            if status != "closed" {
                open_bodies += 1;
            }
            println!(
                "{name}\t{label}\t{status}\t{}\t{open}\t{}\t{seconds:.2}\t{detail}",
                model.failed_faces,
                model.retained_bytes()
            );
        }
    }
    println!(
        "# {} files, {bodies} bodies: {} closed, {open_bodies} open or voids refused, {refused} refused",
        files.len(),
        bodies.saturating_sub(open_bodies + refused).max(0)
    );
    if open_bodies + refused > 0 {
        std::process::exit(1);
    }
}
