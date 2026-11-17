// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;

fn path(x: f64) -> DepositionPath {
    DepositionPath {
        material_domain: None,
        kind: DepositionKind::Support,
        owner: None,
        width: WidthProfile::Constant { width_mm: 0.4 },
        physical_z_mm: [0.0, 0.2],
        points_mm: vec![[x, 0.5, 0.2], [x + 1.0, 0.5, 0.2]],
        arcs: Vec::new(),
        exposed: false,
        connectors: Vec::new(),
    }
}

fn layer(paths: Vec<DepositionPath>) -> PrintLayer {
    PrintLayer {
        index: 17,
        hatch_course: 0,
        z_mm: 0.2,
        lower_z_mm: 0.0,
        height_mm: 0.2,
        section_z_mm: 0.1,
        paths,
        skin_command_fits: Vec::new(),
        region_coverage: PlanarRegionCoverage {
            section_area_mm2: 0.0,
            source_section_area_mm2: 0.0,
            surface_added_area_mm2: 0.0,
            wall_area_mm2: 0.0,
            bottom_skin_area_mm2: 0.0,
            top_skin_area_mm2: 0.0,
            solid_interior_area_mm2: 0.0,
            sparse_interior_area_mm2: 0.0,
            intentional_void_area_mm2: 0.0,
            protected_hole_area_mm2: 0.0,
            nominal_deposition_volume_mm3: 0.0,
            support_area_mm2: 0.0,
            support_nominal_deposition_volume_mm3: 0.0,
            unsupported_overhang_mm: 0.0,
            support_omitted_wall_mm: 0.0,
            thin_widened_mm: 0.0,
            thin_omitted_area_mm2: 0.0,
            process_regions: Vec::new(),
            process_interfaces: Vec::new(),
            separator_area_mm2: 0.0,
            process_diaphragms: Vec::new(),
            diaphragm_area_mm2: 0.0,
            skin_bridge_domains: Vec::new(),
            web_roof_domains: Vec::new(),
        },
    }
}

fn evaluate(
    layers: &[PrintLayer],
    height: spiling_geometry::ProtectedVoidHeightScope,
    allowance: usize,
) -> (Result<(), ManufacturingError>, ClearanceTrace) {
    let cancel = AtomicBool::new(false);
    let mut work = 0;
    let index = clearance_index::ClearanceIndex::build(
        layers.iter().map(|layer| layer.paths.as_slice()),
        allowance,
        &mut work,
        &cancel,
    )
    .unwrap();
    assert_eq!(index.is_some(), allowance != 0);
    let polygon =
        geo::Rect::new(geo::Coord { x: 0.0, y: 0.0 }, geo::Coord { x: 1.0, y: 1.0 }).to_polygon();
    let trace = ClearanceTrace {
        enabled: true,
        ..ClearanceTrace::default()
    };
    let result = certify_polygon_capsules(
        PolygonDeposition {
            layers,
            range: 0..layers.len(),
            index: index.as_ref(),
        },
        &ForbiddenPolygonScope {
            polygon: &polygon,
            height,
            label: "ordered-index regression",
        },
        0.2,
        &mut spiling_geometry::GeometryWorkBudget::new(1_000_000),
        &mut work,
        &trace,
        &cancel,
    );
    (result, trace)
}

#[test]
fn indexed_native_clearance_matches_exhaustive_first_error_and_narrow_tests() {
    for variant in 0..9 {
        let mut paths = (0..65).map(|_| path(100.0)).collect::<Vec<_>>();
        let mut height = spiling_geometry::ProtectedVoidHeightScope::Bounded {
            lower_mm: 0.0,
            upper_mm: 0.2,
        };
        match variant {
            0 => {}
            1 => paths[0] = path(0.0),
            2 => {
                paths[0] = path(0.0);
                paths[64].width = WidthProfile::Constant { width_mm: f64::NAN };
            }
            3 => {
                paths[0].width = WidthProfile::Constant { width_mm: f64::NAN };
                paths[64] = path(0.0);
            }
            4 => {
                paths[64].points_mm = vec![[-2.0, 1.35, 0.2], [0.0, 1.35, 0.2], [2.0, 1.35, 0.2]];
                paths[64].width = WidthProfile::PerEdge {
                    widths_mm: vec![0.25, 1.0],
                };
            }
            5 => {
                paths[0] = path(0.0);
                height = spiling_geometry::ProtectedVoidHeightScope::OpenBelow { upper_mm: -0.1 };
            }
            6 => {
                paths[64] = path(0.0);
                paths[64].kind = DepositionKind::Brim;
            }
            7 => {
                paths[64] = path(0.0);
                paths[64].kind = DepositionKind::Skirt;
            }
            _ => {
                paths[0] = path(0.0);
                paths[0].kind = DepositionKind::Perimeter;
            }
        }
        let layers = [layer(paths)];
        let (ordinary, ordinary_trace) = evaluate(&layers, height, 0);
        let (indexed, indexed_trace) = evaluate(&layers, height, usize::MAX);
        assert_eq!(
            format!("{indexed:?}"),
            format!("{ordinary:?}"),
            "variant {variant}"
        );
        assert_eq!(
            indexed_trace.polygon_tests.get(),
            ordinary_trace.polygon_tests.get()
        );
        if variant == 0 {
            assert!(indexed.is_ok());
            assert_eq!(indexed_trace.skipped_paths.get(), 65);
            assert_eq!(indexed_trace.paths.get(), 0);
        }
        if [1, 2, 4, 6, 7].contains(&variant) {
            assert_eq!(
                indexed.as_ref().unwrap_err().code,
                ManufacturingErrorCode::NativeAnalysisIndeterminate
            );
        }
        if variant == 3 {
            assert_eq!(
                indexed.as_ref().unwrap_err().code,
                ManufacturingErrorCode::InvalidSpecification
            );
        }
    }
}

#[test]
fn physical_scope_ranges_preserve_irregular_height_endpoints_and_open_below() {
    let schedule = [(0.0, 0.123), (0.123, 0.347), (0.347, 0.51)]
        .into_iter()
        .enumerate()
        .map(|(course, (lower_z, z))| LayerElevation {
            lower_z,
            z,
            sample: (lower_z + z) * 0.5,
            height: z - lower_z,
            course: course as u32,
        })
        .collect::<Vec<_>>();
    let layers = (0..schedule.len())
        .map(|_| layer(vec![path(100.0)]))
        .collect::<Vec<_>>();
    let extensions = [0.002, 0.003];
    let cancel = AtomicBool::new(false);
    for boundary in [0.0_f64, 0.123, 0.347, 0.51] {
        for height in [boundary.next_down(), boundary, boundary.next_up()] {
            for lower in [None, Some(height - 0.01), Some(height)] {
                let scope = lower.map_or(
                    spiling_geometry::ProtectedVoidHeightScope::OpenBelow { upper_mm: height },
                    |lower_mm| spiling_geometry::ProtectedVoidHeightScope::Bounded {
                        lower_mm,
                        upper_mm: height,
                    },
                );
                let expected = schedule
                    .iter()
                    .enumerate()
                    .filter(|(_, layer)| {
                        lower.is_none_or(|lower| (layer.z + extensions[1]).next_up() >= lower)
                            && (layer.lower_z - extensions[0]).next_down() <= height
                    })
                    .map(|(index, _)| index)
                    .collect::<Vec<_>>();
                let range =
                    scoped_support_layer_range(&schedule, scope, extensions, &mut 0, &cancel)
                        .unwrap();
                assert_eq!(range.clone().collect::<Vec<_>>(), expected);
                let borrowed =
                    scoped_support_layers(&layers, &schedule, scope, extensions, &mut 0, &cancel)
                        .unwrap();
                assert_eq!(borrowed.as_ptr(), layers[range].as_ptr());
            }
        }
    }
}

#[test]
fn skipped_range_work_preserves_the_original_overflow_and_cancellation_state() {
    let cancel = AtomicBool::new(false);
    for remaining in [0, 1, 3, 16] {
        let mut scalar = usize::MAX - remaining;
        let mut batched = scalar;
        let ordinary = (0..17).try_for_each(|_| charge_support_clearance(&mut scalar, &cancel));
        let indexed = charge_support_clearance_range(&mut batched, 17, &cancel);
        assert_eq!(format!("{ordinary:?}"), format!("{indexed:?}"));
        assert_eq!(scalar, batched);
    }
    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    let mut work = 7;
    assert_eq!(
        charge_support_clearance_range(&mut work, 17, &cancel)
            .unwrap_err()
            .code,
        ManufacturingErrorCode::Cancelled
    );
    assert_eq!(work, 7);
}
