// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use spiling_compute::*;

fn coordinates() -> Vec<f64> {
    let mut values = vec![
        -f64::MAX,
        -1e200,
        -1.0,
        -f64::MIN_POSITIVE,
        -f64::from_bits(1),
        -0.0,
        0.0,
        f64::from_bits(1),
        f64::MIN_POSITIVE,
        1.0,
        f64::from_bits(1.0f64.to_bits() + 1),
        1e200,
        f64::MAX,
    ];
    let mut state = 0x18e9_f19d_7ab4_33c5u64;
    for _ in 0..93 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let value = f64::from_bits(state);
        if value.is_finite() {
            values.push(value);
        }
    }
    values
}

fn assert_masks() {
    let cancel = AtomicBool::new(false);
    let values = coordinates();
    for (columns, rows) in [
        (1usize, 4_099usize),
        (7, 593),
        (31, 137),
        (32, 131),
        (33, 137),
        (63, 131),
        (64, 67),
        (65, 67),
        (137, 1_031),
        (2_051, 129),
    ] {
        let ranges: Vec<_> = (0..columns)
            .map(|index| {
                [
                    values[index % values.len()],
                    values[(index * 7 + 3) % values.len()],
                ]
            })
            .collect();
        let queries: Vec<_> = (0..rows)
            .map(|index| values[index % values.len()])
            .collect();
        let crossings = row_crossings(&ranges, &queries, &cancel)
            .unwrap()
            .expect("admitted crossings");
        let closed = interval_membership(&ranges, &queries, &cancel)
            .unwrap()
            .expect("admitted closed intervals");
        assert_eq!((crossings.rows(), crossings.columns()), (rows, columns));
        for (row, coordinate) in queries.iter().enumerate() {
            let mut selected = Vec::new();
            for (column, [start, end]) in ranges.iter().enumerate() {
                let expected = (start <= coordinate && coordinate < end)
                    || (end <= coordinate && coordinate < start);
                let expected_closed = (start <= coordinate && coordinate <= end)
                    || (end <= coordinate && coordinate <= start);
                assert_eq!(
                    crossings.get(row, column),
                    Some(expected),
                    "crossing ({row}, {column})"
                );
                assert_eq!(
                    closed.get(row, column),
                    Some(expected_closed),
                    "interval ({row}, {column})"
                );
                if expected {
                    selected.push(column);
                }
            }
            assert_eq!(
                crossings.selected_columns(row).unwrap().collect::<Vec<_>>(),
                selected
            );
        }
        let boxes: Vec<_> = (0..columns)
            .map(|index| {
                let first = values[index % values.len()];
                let second = values[(index * 3 + 2) % values.len()];
                let third = values[(index * 13 + 1) % values.len()];
                let fourth = values[(index * 17 + 5) % values.len()];
                [
                    first.min(second),
                    third.min(fourth),
                    first.max(second),
                    third.max(fourth),
                ]
            })
            .collect();
        let queries: Vec<_> = (0..rows)
            .map(|index| {
                let first = values[(index * 3 + 2) % values.len()];
                let second = values[index % values.len()];
                let third = values[(index * 7 + 3) % values.len()];
                [first.min(second), third, first.max(second), third]
            })
            .collect();
        let overlaps = box_overlaps(&boxes, &queries, &cancel)
            .unwrap()
            .expect("admitted boxes");
        for (row, query) in queries.iter().enumerate() {
            for (column, bounds) in boxes.iter().enumerate() {
                let expected = bounds[0] <= query[2]
                    && query[0] <= bounds[2]
                    && bounds[1] <= query[3]
                    && query[1] <= bounds[3];
                assert_eq!(
                    overlaps.get(row, column),
                    Some(expected),
                    "box ({row}, {column})"
                );
            }
        }
    }
}

#[test]
fn cpu_matches_scalar_oracles_and_tiled_tails() {
    let status = configure(ComputeSettings {
        default_backend: ComputeBackend::Cpu,
        fea_backend: None,
    })
    .unwrap();
    assert_eq!(status.default_device.backend, ComputeBackend::Cpu);
    assert!(!status.default_device.hardware_accelerated);
    assert_eq!(status.default_device, status.fea_device);
    assert_masks();
    let cancel = AtomicBool::new(true);
    assert_eq!(
        row_crossings(&vec![[0.0, 1.0]; 128], &[0.0; 64], &cancel),
        Err(ComputeError::Cancelled)
    );
    cancel.store(false, Ordering::Relaxed);
    assert!(
        row_crossings(&vec![[0.0, 1.0]; MAX_BATCH_ITEMS], &[0.0; 129], &cancel)
            .unwrap()
            .is_none()
    );
}

fn run_session(case: &str) {
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "session_child", "--nocapture"])
        .env("SPILING_COMPUTE_TEST_SESSION", case);
    if case == "unavailable-vulkan" {
        command
            .env("VK_DRIVER_FILES", "/spiling-compute-test-no-such-icd.json")
            .env("VK_ICD_FILENAMES", "/spiling-compute-test-no-such-icd.json");
    }
    if case == "default" {
        command.env("CUBECL_WGPU_DEFAULT_DEVICE", "Cpu");
    }
    if case.starts_with("software-") {
        let icd = std::env::var_os("SPILING_COMPUTE_SOFTWARE_VULKAN_ICD").expect(
            "set SPILING_COMPUTE_SOFTWARE_VULKAN_ICD to an actual software Vulkan ICD manifest",
        );
        command
            .env("VK_DRIVER_FILES", &icd)
            .env("VK_ICD_FILENAMES", &icd);
    }
    let output = command.output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    eprintln!(
        "session {case}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn fresh_sessions_admit_defaults_and_explicit_fea() {
    run_session("inherit");
    run_session("explicit-cpu");
    run_session("default");
    run_session("unavailable-vulkan");
}

#[test]
fn session_child() {
    let Ok(case) = std::env::var("SPILING_COMPUTE_TEST_SESSION") else {
        return;
    };
    if case == "unavailable-vulkan" {
        let settings = ComputeSettings {
            default_backend: ComputeBackend::Vulkan,
            fea_backend: None,
        };
        assert!(matches!(
            configure(settings),
            Err(ComputeError::Initialization(_))
        ));
        assert!(matches!(status(), Err(ComputeError::Initialization(_))));
        assert_eq!(
            configure(ComputeSettings {
                default_backend: ComputeBackend::Cpu,
                fea_backend: None
            }),
            Err(ComputeError::AlreadyConfigured)
        );
        return;
    }
    let settings = match case.as_str() {
        "default" => ComputeSettings::default(),
        "explicit-cpu" => ComputeSettings {
            default_backend: ComputeBackend::Cpu,
            fea_backend: Some(ComputeBackend::Cpu),
        },
        "vulkan-fea" => ComputeSettings {
            default_backend: ComputeBackend::Cpu,
            fea_backend: Some(ComputeBackend::Vulkan),
        },
        "vulkan-default-cpu-fea" => ComputeSettings {
            default_backend: ComputeBackend::Vulkan,
            fea_backend: Some(ComputeBackend::Cpu),
        },
        "vulkan-default-auto-fea" | "software-vulkan-auto-fea" => ComputeSettings {
            default_backend: ComputeBackend::Vulkan,
            fea_backend: Some(ComputeBackend::Auto),
        },
        "vulkan" | "software-vulkan" => ComputeSettings {
            default_backend: ComputeBackend::Vulkan,
            fea_backend: None,
        },
        "software-default" => ComputeSettings::default(),
        _ => ComputeSettings {
            default_backend: ComputeBackend::Cpu,
            fea_backend: None,
        },
    };
    let first = if case == "default" {
        status().unwrap()
    } else {
        configure(settings).unwrap()
    };
    assert_eq!(first.settings, settings);
    assert_eq!(status().unwrap(), first);
    assert_eq!(configure(settings).unwrap(), first);
    assert_eq!(
        configure(ComputeSettings {
            default_backend: settings.default_backend,
            fea_backend: if settings.fea_backend.is_none() {
                Some(ComputeBackend::Auto)
            } else {
                None
            }
        }),
        Err(ComputeError::AlreadyConfigured)
    );
    first.validate().unwrap();
    if settings.fea_backend.is_none() {
        assert_eq!(first.default_device, first.fea_device);
    }
    if settings.default_backend == ComputeBackend::Auto
        && first.default_device.backend == ComputeBackend::Vulkan
    {
        assert!(first.default_device.hardware_accelerated);
    }
    if case == "vulkan-fea" {
        assert_eq!(first.default_device.backend, ComputeBackend::Cpu);
        assert_eq!(first.fea_device.backend, ComputeBackend::Vulkan);
    }
    if case == "vulkan-default-cpu-fea" {
        assert_eq!(first.default_device.backend, ComputeBackend::Vulkan);
        assert_eq!(first.fea_device.backend, ComputeBackend::Cpu);
    }
    eprintln!("resolved session: {first:?}");
    if case == "software-vulkan" || case == "software-vulkan-auto-fea" {
        assert_eq!(first.default_device.backend, ComputeBackend::Vulkan);
        assert!(!first.default_device.hardware_accelerated);
        if case == "software-vulkan-auto-fea" {
            assert_eq!(first.fea_device.backend, ComputeBackend::Cpu);
        }
    }
    if case == "software-default" {
        assert_eq!(first.default_device.backend, ComputeBackend::Cpu);
    }
    if case == "vulkan" || case == "software-vulkan" {
        assert_masks();
    }
}

#[test]
#[ignore = "opt-in actual Vulkan execution; requires an installed Vulkan adapter/driver"]
fn vulkan_matches_scalar_oracles_and_independent_fea() {
    run_session("vulkan");
    run_session("vulkan-fea");
    run_session("vulkan-default-cpu-fea");
    run_session("vulkan-default-auto-fea");
}

#[test]
#[ignore = "opt-in actual software Vulkan; set SPILING_COMPUTE_SOFTWARE_VULKAN_ICD to its ICD manifest"]
fn software_vulkan_is_explicit_and_auto_selects_cpu() {
    run_session("software-default");
    run_session("software-vulkan");
    run_session("software-vulkan-auto-fea");
}
