// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Explicit developer measurements through production hatch dispatch.
//! No timing/resource threshold or alternate infill generator.
use super::*;
use geo::Area;
use std::time::Instant;

fn setting(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.to_owned())
}

#[inline(never)]
fn observe(paths: &[DepositionPath], length: &mut f64, volume: &mut f64, signature: &mut u64) {
    for path in paths {
        for point in &path.points_mm {
            for value in point {
                *signature = signature.rotate_left(7) ^ value.to_bits();
            }
        }
        for edge in path.checked_edges().unwrap() {
            let span = (edge.points_mm[1][0] - edge.points_mm[0][0])
                .hypot(edge.points_mm[1][1] - edge.points_mm[0][1]);
            let h = path.height_mm();
            *length += span;
            *volume += span * ((edge.width_mm - h) * h + std::f64::consts::PI * h * h / 4.0);
        }
    }
}

#[test]
#[ignore = "explicit infill timing/perf workload; no performance admission gate"]
fn measure_infill_generation() {
    let pattern = setting("SPILING_INFILL_PATTERN", "gyroid");
    let shape = setting("SPILING_INFILL_SHAPE", "annulus");
    let size: f64 = setting("SPILING_INFILL_SIZE", "48").parse().unwrap();
    let fraction: f64 = setting("SPILING_INFILL_FRACTION", "0.15").parse().unwrap();
    let layers: usize = setting("SPILING_INFILL_LAYERS", "20").parse().unwrap();
    let repeats: usize = setting("SPILING_INFILL_REPEATS", "3").parse().unwrap();
    let observe_output = setting("SPILING_INFILL_OBSERVE", "1") == "1";
    let backend =
        spiling_compute::ComputeBackend::parse(&setting("SPILING_INFILL_COMPUTE", "cpu")).unwrap();
    let setup = Instant::now();
    let device = spiling_compute::configure(spiling_compute::ComputeSettings {
        default_backend: backend,
        fea_backend: None,
    })
    .unwrap();
    let setup_us = setup.elapsed().as_micros();
    assert!(
        size.is_finite()
            && size > 0.0
            && fraction > 0.0
            && fraction < 1.0
            && layers > 0
            && repeats > 0
    );
    let rect = |a, b| geo::Rect::new(a, b).to_polygon();
    let ring = |radius: f64, sides: usize| {
        LineString(
            (0..=sides)
                .map(|i| {
                    let angle = i as f64 * std::f64::consts::TAU / sides as f64;
                    Coord {
                        x: size * 0.5 + radius * angle.cos(),
                        y: size * 0.5 + radius * angle.sin(),
                    }
                })
                .collect(),
        )
    };
    let domain = match shape.as_str() {
        "rectangle" => MultiPolygon(vec![rect([0.0, 0.0], [size, size])]),
        "annulus" => MultiPolygon(vec![Polygon::new(
            ring(size * 0.5, 192),
            vec![ring(size * 0.1, 96)],
        )]),
        "notch" => MultiPolygon(vec![Polygon::new(
            LineString::from(vec![
                (0.0, 0.0),
                (size, 0.0),
                (size, size * 0.4),
                (size * 0.35, size * 0.4),
                (size * 0.35, size),
                (0.0, size),
                (0.0, 0.0),
            ]),
            vec![],
        )]),
        "islands" => MultiPolygon(vec![
            rect([0.0, 0.0], [size * 0.4, size]),
            rect([size * 0.6, 0.0], [size, size]),
        ]),
        _ => panic!("unknown measurement shape"),
    };
    let mut recipe = super::super::tests::recipe(fraction);
    recipe.layer_height_mm = 0.2;
    recipe.bead_width_mm = 0.45;
    recipe.sparse_infill = match pattern.as_str() {
        "rectilinear" => SparseInfillPolicy::Fine,
        "grid" => SparseInfillPolicy::Grid,
        "gyroid" => SparseInfillPolicy::Gyroid,
        "cathedral" => SparseInfillPolicy::web_roof(),
        _ => panic!("unknown measurement pattern"),
    };
    let cancel = AtomicBool::new(false);
    let roof_z = if setting("SPILING_INFILL_ROOF", "core") == "roof" {
        (layers + 1) as f64 * 0.2
    } else {
        40.0
    };
    for repeat in 0..=repeats {
        let mut nanos = 0u128;
        let mut path_count = 0;
        let mut segment_count = 0;
        let mut length = 0.0;
        let mut volume = 0.0;
        let mut signature = 0u64;
        for index in 1..=layers {
            let mut paths = Vec::new();
            let mut segments = 0;
            let z = index as f64 * 0.2;
            let layer = HatchLayer {
                roof: Some(Roof {
                    z_mm: roof_z,
                    frame_mm: [[0.0, 0.0], [size, size]],
                    ceilings: &[],
                    columns: None,
                }),
                width_mm: 0.45,
                z,
                lower_z: z - 0.2,
                height: 0.2,
                axis: if index % 2 == 0 {
                    HatchAxis::X
                } else {
                    HatchAxis::Y
                },
                top_axis: HatchAxis::X,
                recipe: &recipe,
            };
            let start = Instant::now();
            hatch(
                &domain,
                DepositionKind::SparseFill,
                fraction,
                &layer,
                &mut paths,
                &mut segments,
                &cancel,
            )
            .unwrap();
            nanos += start.elapsed().as_nanos();
            path_count += paths.len();
            segment_count += segments;
            if observe_output {
                observe(&paths, &mut length, &mut volume, &mut signature);
            }
            let retire = Instant::now();
            drop(std::hint::black_box(paths));
            nanos += retire.elapsed().as_nanos();
        }
        if repeat > 0 {
            println!(
                "INFILL_PERF {}",
                serde_json::json!({
                    "pattern":pattern,"shape":shape,"size_mm":size,"fraction":fraction,"roof_z_mm":roof_z,
                    "layers":layers,"sample":repeat,"generation_us":nanos as f64 / 1000.0,
                    "paths":path_count,"segments":segment_count,"length_mm":length,"volume_mm3":volume,
                    "signature":format!("{signature:016x}"),"observed":observe_output,
                    "domain_area_mm2":domain.unsigned_area(),"compute_setup_us":setup_us,
                    "device":device.default_device,"engine_build":spiling_contracts::ENGINE_BUILD_ID,
                    "debug_assertions":cfg!(debug_assertions),"software_only":true,
                    "scope":"production layer hatch plus output drop; Cathedral includes local seed discovery; excludes observation, source/walls/skins/routing/verification"
                })
            );
        }
    }
}
