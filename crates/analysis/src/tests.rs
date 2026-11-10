// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_contracts::{
    analysis::{mechanics::*, *},
    geometry::*,
    project::*,
};
use std::sync::atomic::AtomicBool;

fn tensor() -> [[f64; 6]; 6] {
    std::array::from_fn(|row| {
        std::array::from_fn(|column| {
            if row == column {
                [2100.0, 1800.0, 1200.0, 500.0, 600.0, 700.0][row]
            } else {
                0.0
            }
        })
    })
}
fn settings() -> AnalysisRunSettings {
    let mut settings: AnalysisRunSettings = serde_json::from_slice(include_bytes!(
        "../../../fixtures/analysis/hook-assumptions.json"
    ))
    .unwrap();
    settings.laws.wall_mpa = tensor();
    settings.laws.skin_mpa = tensor();
    settings.laws.effective_core_mpa = tensor();
    settings.maximum_span_mm = 32.0;
    settings
}
fn beam() -> native::NativePrism {
    let points = vec![[0.0, 0.0], [20.0, 0.0], [20.0, 4.0], [0.0, 4.0], [0.0, 0.0]];
    native::NativePrism {
        bore: None,
        edges: points
            .windows(2)
            .enumerate()
            .map(|(index, pair)| native::Edge {
                ends: [pair[0], pair[1]],
                face: FaceId::from_step_entity(index as u64 + 1).unwrap(),
            })
            .collect(),
        loops: vec![points],
        height: [0.0, 2.0],
        shape_error_mm: 0.0,
        native_sample_hash: [0; 32],
    }
}

#[test]
fn native_circular_plate_retains_hole_volume_and_equilibrium() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/geometry/independent-exporters/occt-through-hole-mm.step");
    let definition = spiling_geometry::from_step_file(&path).unwrap();
    let cancel = AtomicBool::new(false);
    let mut case = case();
    case.definition_id = definition.id().clone();
    case.source_hash = definition.provenance().source_hash.clone();
    let prism = native::capture(&definition, case.occurrence_id, &cancel).unwrap();
    let bore = prism.bore.as_ref().unwrap();
    case.loads[0].face_id = bore.outer_faces()[1].clone();
    case.loads[0].force_n = [0.0, 0.0, -10.0];
    case.loads[0].moment_n_mm = [-100.0, 200.0, 0.0];
    case.restraints[0].face_id = bore.outer_faces()[0].clone();
    let mut settings = settings();
    settings.maximum_span_mm = 12.0;
    let budget = Budget::new(&cancel, 60);
    let operator = prepare(&prism, &settings, RigidPoseMm::IDENTITY, &case, &budget).unwrap();
    let solution = solve(&operator, &case, &settings, &budget).unwrap();
    let expected = (400.0 - std::f64::consts::PI * 9.0) * 8.0;
    assert!((solution.volumes.iter().sum::<f64>() - expected).abs() < 1e-6);
    let wall = settings.recipe.perimeter_count as f64 * settings.recipe.bead_width_mm;
    let skin_height =
        settings.recipe.top_skin_thickness_mm + settings.recipe.bottom_skin_thickness_mm;
    let remaining_height = 8.0 - skin_height;
    let wall_area =
        400.0 - (20.0 - 2.0 * wall).powi(2) + std::f64::consts::PI * ((3.0 + wall).powi(2) - 9.0);
    assert!((solution.volumes[0] - wall_area * remaining_height).abs() < 1e-6);
    assert!((solution.volumes[1] - expected / 8.0 * skin_height).abs() < 1e-6);
    assert!(
        (solution.volumes[2] - (expected - solution.volumes[0] - solution.volumes[1])).abs() < 1e-6
    );
    assert!(solution.assessment.free_residual_n < 1e-6);
    assert!(solution.energy.iter().sum::<f64>() > 0.0);
    assert!(
        operator
            .mesh
            .nodes
            .iter()
            .all(|point| (point[0] - 10.0).hypot(point[1] - 10.0) >= 3.0 - 1e-8)
    );
    eprintln!(
        "circular native plate: elements={} dofs={} setup_ms={} solve_ms={} assessment_ms={}",
        operator.mesh.elements.len(),
        operator.mesh.nodes.len() * 3,
        operator.setup_ms,
        solution.solve_ms,
        solution.assessment_ms
    );
}

#[test]
fn finite_disk_changes_loaded_area_not_total_force_and_reuses_original_operator() {
    use spiling_contracts::analysis::load::ForceFootprint;
    let settings = settings();
    let mut case = case();
    let cancel = AtomicBool::new(false);
    let budget = Budget::new(&cancel, 60);
    let operator = prepare(&beam(), &settings, RigidPoseMm::IDENTITY, &case, &budget).unwrap();
    case.loads[0].force_n = [0.0, -10.0, 0.0];
    case.loads[0].moment_n_mm = [10.0, 0.0, -200.0];
    let mut prior_loads = None;
    for diameter in [0.4, 1.5, 10.0] {
        case.loads[0].footprint = ForceFootprint::Disk {
            center_mm: [20.0, 2.0, 1.0],
            diameter_mm: diameter,
        };
        let result = solve(&operator, &case, &settings, &budget).unwrap();
        let area: f64 = result.patches.iter().map(|sample| sample.area_mm2).sum();
        if diameter < 2.0 {
            assert!(
                (area / (std::f64::consts::PI * diameter * diameter * 0.25) - 1.0).abs() < 1e-10
            );
        } else {
            assert!((area - 8.0).abs() < 1e-10);
        }
        for sample in &result.patches {
            assert!((sample.point_mm[0] - 20.0).abs() < 1e-10);
            assert!((sample.point_mm[1] - 2.0).hypot(sample.point_mm[2] - 1.0) <= diameter * 0.5);
            assert!((sample.traction_mpa[1] + 10.0 / area).abs() < 1e-8);
        }
        assert!(result.assessment.patch_force_error_n < 1e-10);
        assert!(result.assessment.patch_moment_error_n_mm < 1e-8);
        if let Some(prior) = &prior_loads {
            assert_ne!(prior, &result.applied_force_n);
        }
        prior_loads = Some(result.applied_force_n);
    }
    case.loads[0].footprint = ForceFootprint::Disk {
        center_mm: [19.0, 2.0, 1.0],
        diameter_mm: 1.0,
    };
    assert!(
        solve(&operator, &case, &settings, &budget)
            .err()
            .unwrap()
            .contains("contact center")
    );
}
fn case() -> AnalysisCase {
    AnalysisCase {
        id: AnalysisCaseId::new(),
        name: "analytical moment control".into(),
        layout_id: PrintLayoutId::new(),
        occurrence_id: OccurrenceId::new(1).unwrap(),
        definition_id: DefinitionId::from_source_sha256(&[1; 32]),
        source_hash: SourceHash::from_digest(&[1; 32]),
        mode: AnalysisMode::Basic,
        loads: vec![AnalysisFaceLoad {
            footprint: spiling_contracts::analysis::load::ForceFootprint::WholeFace {},
            face_id: FaceId::from_step_entity(2).unwrap(),
            force_n: [0.0; 3],
            moment_n_mm: [0.0, 0.0, 10.0],
        }],
        fasteners: vec![],
        restraints: vec![AnalysisFaceRestraint {
            face_id: FaceId::from_step_entity(4).unwrap(),
            fixed_translation: [true; 3],
        }],
    }
}

#[test]
fn disjoint_printed_regions_reproduce_independent_affine_energy() {
    let mut settings = settings();
    settings.recipe.perimeter_count = 1;
    settings.recipe.top_skin_thickness_mm = 0.25;
    settings.recipe.bottom_skin_thickness_mm = 0.25;
    settings.laws.compatible_recipe = settings.recipe.clone();
    settings.laws.skin_mpa = tensor().map(|row| row.map(|value| 1.5 * value));
    settings.laws.effective_core_mpa = tensor().map(|row| row.map(|value| 0.2 * value));
    let cancel = AtomicBool::new(false);
    let budget = Budget::new(&cancel, 60);
    let operator = prepare(&beam(), &settings, RigidPoseMm::IDENTITY, &case(), &budget).unwrap();
    let mut volumes = [0.0; 3];
    let mut energies = [0.0; 3];
    let strain = 0.01;
    for hex in &operator.mesh.elements {
        let matrix = element::stiffness(hex.size, &operator.tensors[hex.role]);
        let values = hex
            .nodes
            .map(|node| [strain * operator.mesh.nodes[node][0], 0.0, 0.0]);
        for row in 0..81 {
            for column in 0..81 {
                energies[hex.role] += 0.5
                    * values[row / 3][row % 3]
                    * values[column / 3][column % 3]
                    * matrix[81 * row.max(column) + row.min(column)];
            }
        }
        volumes[hex.role] += hex.size.iter().product::<f64>();
    }
    let expected_volumes = [34.5, 40.0, 85.5];
    for role in 0..3 {
        assert!((volumes[role] - expected_volumes[role]).abs() < 1e-12);
        let expected =
            0.5 * strain * strain * [2100.0, 3150.0, 420.0][role] * expected_volumes[role];
        assert!((energies[role] / expected - 1.0).abs() < 1e-10);
    }
}

#[test]
fn q2_rigid_affine_and_independent_bending_energy() {
    let size = [20.0, 4.0, 2.0];
    let stiffness = element::stiffness(size, &tensor());
    let curvature = 0.001;
    let values: [[f64; 3]; 27] = std::array::from_fn(|node| {
        let position = [
            (node % 3) as f64 * 10.0,
            ((node / 3) % 3) as f64 * 2.0 - 2.0,
            (node / 9) as f64,
        ];
        [
            -curvature * position[0] * position[1],
            curvature * position[0] * position[0] / 2.0,
            0.0,
        ]
    });
    let mut energy = 0.0;
    for row in 0..81 {
        for column in 0..81 {
            energy += 0.5
                * values[row / 3][row % 3]
                * values[column / 3][column % 3]
                * stiffness[81 * row.max(column) + row.min(column)];
        }
    }
    let expected = 0.5 * 2100.0 * curvature * curvature * (2.0 * 4.0_f64.powi(3) / 12.0) * 20.0;
    assert!(
        (energy / expected - 1.0).abs() < 1e-10,
        "{energy} != {expected}"
    );
    element::quadrature(|point, _| {
        let (shape, gradients) = element::basis(point, size);
        assert!((shape.iter().sum::<f64>() - 1.0).abs() < 1e-14);
        for axis in 0..3 {
            assert!(
                gradients
                    .iter()
                    .map(|gradient| gradient[axis])
                    .sum::<f64>()
                    .abs()
                    < 1e-14
            );
        }
        let (strain, stress) = element::state(&gradients, &values, &tensor());
        assert!((strain[0] + curvature * 2.0 * point[1]).abs() < 1e-14);
        assert!(strain[1..].iter().all(|value| value.abs() < 1e-14));
        assert!((stress[0] - 2100.0 * strain[0]).abs() < 1e-12);
        let affine = std::array::from_fn(|node| {
            let [x_coordinate, y_coordinate, z_coordinate] = [
                (node % 3) as f64 * 10.0,
                ((node / 3) % 3) as f64 * 2.0,
                (node / 9) as f64,
            ];
            [
                0.01 * x_coordinate + 0.02 * y_coordinate,
                0.03 * y_coordinate + 0.04 * z_coordinate,
                0.05 * z_coordinate + 0.06 * x_coordinate,
            ]
        });
        let strain = element::state(&gradients, &affine, &tensor()).0;
        for (actual, expected) in strain.into_iter().zip([0.01, 0.03, 0.05, 0.04, 0.06, 0.02]) {
            assert!((actual - expected).abs() < 1e-14);
        }
        let rigid = std::array::from_fn(|node| {
            let x_coordinate = (node % 3) as f64 * 10.0;
            let y_coordinate = ((node / 3) % 3) as f64 * 2.0;
            [3.0 - 0.7 * y_coordinate, 2.0 + 0.7 * x_coordinate, 1.0]
        });
        assert!(
            element::state(&gradients, &rigid, &tensor())
                .0
                .iter()
                .all(|value| value.abs() < 1e-14)
        );
    });
}
#[test]
fn solved_pure_bending_and_source_origin_wrench() {
    let cancel = AtomicBool::new(false);
    let budget = Budget::new(&cancel, 60);
    let settings = settings();
    let mut case = case();
    let operator = prepare(&beam(), &settings, RigidPoseMm::IDENTITY, &case, &budget).unwrap();
    let solution = solve(&operator, &case, &settings, &budget).unwrap();
    let inertia = 2.0 * 4.0_f64.powi(3) / 12.0;
    let curvature = 10.0 / (2100.0 * inertia);
    for (point, displacement) in operator.mesh.nodes.iter().zip(&solution.displacement_mm) {
        assert!((displacement[0] + curvature * point[0] * (point[1] - 2.0)).abs() < 1e-10);
        assert!((displacement[1] - curvature * point[0] * point[0] / 2.0).abs() < 1e-10);
        assert!(displacement[2].abs() < 1e-10);
    }
    assert!(
        (solution.energy.iter().sum::<f64>() / (10.0 * 10.0 * 20.0 / (2.0 * 2100.0 * inertia))
            - 1.0)
            .abs()
            < 1e-10
    );
    case.loads[0].force_n = [0.0, -2.0, 3.0];
    case.loads[0].moment_n_mm = [7.0, -60.0, -40.0];
    let solution = solve(&operator, &case, &settings, &budget).unwrap();
    let mut force = [0.0; 3];
    let mut moment = [0.0; 3];
    for sample in solution.patches {
        let [x_coordinate, y_coordinate, z_coordinate] = sample.point_mm;
        let [fx, fy, fz] = sample.traction_mpa.map(|entry| entry * sample.area_mm2);
        force[0] += fx;
        force[1] += fy;
        force[2] += fz;
        moment[0] += y_coordinate * fz - z_coordinate * fy;
        moment[1] += z_coordinate * fx - x_coordinate * fz;
        moment[2] += x_coordinate * fy - y_coordinate * fx;
    }
    for axis in 0..3 {
        assert!((force[axis] - case.loads[0].force_n[axis]).abs() < 1e-10);
        assert!((moment[axis] - case.loads[0].moment_n_mm[axis]).abs() < 1e-9);
    }
    case.loads.push(AnalysisFaceLoad {
        footprint: spiling_contracts::analysis::load::ForceFootprint::WholeFace {},
        face_id: FaceId::from_step_entity(3).unwrap(),
        force_n: case.loads[0].force_n.map(|value| -value),
        moment_n_mm: case.loads[0].moment_n_mm.map(|value| -value),
    });
    let multiple = solve(&operator, &case, &settings, &budget).unwrap();
    for (index, load) in case.loads.iter().enumerate() {
        let samples = multiple
            .patches
            .iter()
            .filter(|sample| sample.patch_index as usize == index);
        let mut moment = [0.0; 3];
        let mut force = [0.0; 3];
        for sample in samples {
            let [x_coordinate, y_coordinate, z_coordinate] = sample.point_mm;
            let [force_x, force_y, force_z] =
                sample.traction_mpa.map(|value| value * sample.area_mm2);
            force[0] += force_x;
            force[1] += force_y;
            force[2] += force_z;
            moment[0] += y_coordinate * force_z - z_coordinate * force_y;
            moment[1] += z_coordinate * force_x - x_coordinate * force_z;
            moment[2] += x_coordinate * force_y - y_coordinate * force_x;
        }
        for axis in 0..3 {
            assert!((force[axis] - load.force_n[axis]).abs() < 1e-10);
            assert!((moment[axis] - load.moment_n_mm[axis]).abs() < 1e-9);
        }
    }
    case.restraints[0].fixed_translation = [true, false, false];
    assert!(
        prepare(&beam(), &settings, RigidPoseMm::IDENTITY, &case, &budget)
            .err()
            .unwrap()
            .contains("rigid restraint ranks")
    );
}

#[test]
fn complete_face_restraint_rank_is_independent_of_source_origin() {
    let cancel = AtomicBool::new(false);
    let budget = Budget::new(&cancel, 60);
    let settings = settings();
    let case = case();
    let reference = prepare(&beam(), &settings, RigidPoseMm::IDENTITY, &case, &budget).unwrap();
    for translation in [
        [1_000_000.0, 0.0, 0.0],
        [-1_000_000.0, 1_000_000.0, -1_000_000.0],
    ] {
        let mut prism = beam();
        for boundary in &mut prism.loops {
            for point in boundary {
                for axis in 0..2 {
                    point[axis] += translation[axis];
                }
            }
        }
        for edge in &mut prism.edges {
            for point in &mut edge.ends {
                for axis in 0..2 {
                    point[axis] += translation[axis];
                }
            }
        }
        prism.height = prism.height.map(|height| height + translation[2]);
        let translated = prepare(&prism, &settings, RigidPoseMm::IDENTITY, &case, &budget).unwrap();
        assert_eq!(translated.ranks, reference.ranks);
        assert_eq!(translated.mesh.nodes.len(), reference.mesh.nodes.len());
    }
}

#[test]
fn native_side_ownership_cannot_extend_a_height_split_face() {
    let cancel = AtomicBool::new(false);
    let definition = spiling_geometry::from_step_bytes_with_cancel(
        include_bytes!("../../../fixtures/geometry/analysis-height-split-box.step"),
        &cancel,
    )
    .unwrap();
    let refusal = native::capture(&definition, OccurrenceId::new(1).unwrap(), &cancel).unwrap_err();
    assert!(
        refusal.contains("complete native side-face trim"),
        "{refusal}"
    );
}
#[test]
fn engineering_shear_and_rotation() {
    let rotated = element::rotate(
        &tensor(),
        [[0.0, 0.0, 1.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0]],
    );
    for (axis, expected) in [1200.0, 1800.0, 2100.0, 700.0, 600.0, 500.0]
        .into_iter()
        .enumerate()
    {
        assert!((rotated[axis][axis] - expected).abs() < 1e-12);
    }
}

#[test]
fn no_gap_welding_law_drift_or_cancelled_preparation() {
    let mut disconnected = beam();
    let second = vec![
        [20.0, 4.0],
        [40.0, 4.0],
        [40.0, 8.0],
        [20.0, 8.0],
        [20.0, 4.0],
    ];
    disconnected.edges.extend(
        second
            .windows(2)
            .enumerate()
            .map(|(index, pair)| native::Edge {
                ends: [pair[0], pair[1]],
                face: FaceId::from_step_entity(index as u64 + 10).unwrap(),
            }),
    );
    disconnected.loops.push(second);
    let cancel = AtomicBool::new(false);
    let budget = Budget::new(&cancel, 60);
    let settings = settings();
    let case = case();
    let refusal = prepare(
        &disconnected,
        &settings,
        RigidPoseMm::IDENTITY,
        &case,
        &budget,
    )
    .err()
    .unwrap();
    assert!(
        refusal.contains("contact cannot") || refusal.contains("rigid restraint ranks"),
        "{refusal}"
    );
    let mut narrow_gap = beam();
    let second = vec![
        [20.0 + 1e-9, 0.0],
        [40.0, 0.0],
        [40.0, 4.0],
        [20.0 + 1e-9, 4.0],
        [20.0 + 1e-9, 0.0],
    ];
    narrow_gap.edges.extend(
        second
            .windows(2)
            .enumerate()
            .map(|(index, pair)| native::Edge {
                ends: [pair[0], pair[1]],
                face: FaceId::from_step_entity(index as u64 + 10).unwrap(),
            }),
    );
    narrow_gap.loops.push(second);
    let refusal = prepare(
        &narrow_gap,
        &settings,
        RigidPoseMm::IDENTITY,
        &case,
        &budget,
    )
    .err()
    .unwrap();
    assert!(
        refusal.contains("no gap or thin region was merged away"),
        "{refusal}"
    );
    let operator = prepare(&beam(), &settings, RigidPoseMm::IDENTITY, &case, &budget).unwrap();
    let mut incompatible = settings.clone();
    incompatible.recipe.infill_fraction = 0.4;
    assert!(incompatible.validate().is_err());
    assert!(solve(&operator, &case, &incompatible, &budget).is_err());
    let mut invalid = settings.laws.clone();
    invalid.wall_mpa[0][5] = f64::NAN;
    assert!(invalid.validate().is_err());
    let mut invalid = settings.laws.clone();
    invalid.effective_core_mpa[0][0] = 0.0;
    assert!(invalid.validate().is_err());
    cancel.store(true, std::sync::atomic::Ordering::Release);
    assert!(
        prepare(&beam(), &settings, RigidPoseMm::IDENTITY, &case, &budget)
            .err()
            .unwrap()
            .starts_with("Cancelled:")
    );
}
fn attachment_native_plate(
    cancel: &AtomicBool,
) -> (native::NativePrism, AnalysisCase, AnalysisRunSettings) {
    use spiling_contracts::analysis::fastener::*;
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/geometry/independent-exporters/occt-through-hole-mm.step");
    let definition = spiling_geometry::from_step_file(&path).unwrap();
    let mut case = case();
    case.definition_id = definition.id().clone();
    case.source_hash = definition.provenance().source_hash.clone();
    let prism = native::capture(&definition, case.occurrence_id, cancel).unwrap();
    let bore = prism.bore.as_ref().unwrap();
    case.restraints.clear();
    case.loads[0].face_id = bore.outer_faces()[1].clone();
    case.loads[0].force_n = [0.0, 0.0, -10.0];
    case.loads[0].moment_n_mm = [-100.0, 200.0, 0.0];
    let center = bore.center_mm();
    let height = bore.height_mm();
    case.fasteners = vec![AnalysisFastener {
        id: 1,
        name: "ideal mounting connection".into(),
        hole: AnalysisHole {
            bore_faces: bore.bore_faces().to_vec(),
            cap_faces: bore.cap_faces().clone(),
            center_mm: [center[0], center[1], height[0]],
            radius_mm: bore.radius_mm(),
            depth_mm: height[1] - height[0],
        },
        attachment: FastenerAttachment::Clamp {
            end: HoleEnd::Start,
            washer_diameter_mm: 8.0,
        },
    }];
    let mut settings = settings();
    settings.maximum_span_mm = 12.0;
    (prism, case, settings)
}

fn attachment_facets(
    mesh: &mesh::Mesh,
) -> Vec<spiling_contracts::analysis::attachment::AttachmentFacet> {
    use spiling_contracts::analysis::attachment::AttachmentFacet;
    mesh.facets
        .iter()
        .map(|facet| {
            let hex = &mesh.elements[facet.element];
            AttachmentFacet {
                element_index: facet.element as u32,
                axis: facet.axis,
                side: facet.side,
                nodes: hex.nodes.map(|node| node as u32),
                positions: hex.nodes.map(|node| mesh.nodes[node]),
                midpoint_weight: hex.midpoint_weight,
            }
        })
        .collect()
}

#[test]
fn curved_native_partition_remains_conforming_across_recipe_widths() {
    let cancel = AtomicBool::new(false);
    let (prism, case, mut settings) = attachment_native_plate(&cancel);
    settings.recipe.perimeter_count = 2;
    for width in [0.4, 0.45, 0.57] {
        settings.recipe.bead_width_mm = width;
        settings.laws.compatible_recipe = settings.recipe.clone();
        let operator = prepare(
            &prism,
            &settings,
            RigidPoseMm::IDENTITY,
            &case,
            &Budget::new(&cancel, 60),
        )
        .unwrap();
        assert!(!operator.mesh.elements.is_empty());
        assert!(
            operator
                .mesh
                .facets
                .iter()
                .all(|facet| { facet.face.is_some() || (facet.axis == 1 && facet.side == 0) })
        );
    }
}

// Independently reconstruct Ku from original element matrices, reactions from
// retained physical samples, and generalized motions from the shared contract.
// This deliberately does not use the augmented factor or its inverse columns.
fn attachment_original_equilibrium(
    operator: &PreparedOperator,
    case: &AnalysisCase,
    solution: &Solution,
) {
    use spiling_contracts::analysis::{attachment, chart};
    let mesh = &operator.mesh;
    let facets = attachment_facets(mesh);
    let mut reactions = vec![[0.0; 3]; mesh.nodes.len()];
    for (index, fastener) in case.fasteners.iter().enumerate() {
        let samples = attachment::samples(fastener, &facets).unwrap();
        let coupling = attachment::coupling(fastener, &samples, mesh.nodes.len()).unwrap();
        let motions = coupling.motions(&solution.displacement_mm);
        for (motion, value) in motions.iter().enumerate() {
            if fastener.constrained_motions()[motion] {
                assert!(value.abs() < 1e-9, "motion {motion}: {value}");
            }
        }
        let patches = solution
            .patches
            .iter()
            .filter(|p| p.patch_index as usize == case.loads.len() + index)
            .collect::<Vec<_>>();
        assert_eq!(patches.len(), samples.len());
        let mut force = [0.0; 3];
        let mut moment = [0.0; 3];
        for (sample, patch) in samples.iter().zip(patches) {
            assert_eq!(sample.element_index, patch.element_index);
            assert_eq!(sample.point_mm, patch.point_mm);
            assert_eq!(sample.area_mm2, patch.area_mm2);
            for axis in 0..3 {
                force[axis] += patch.area_mm2 * patch.traction_mpa[axis];
                moment[axis] +=
                    patch.area_mm2 * chart::cross(patch.point_mm, patch.traction_mpa)[axis];
                for local in 0..27 {
                    reactions[sample.nodes[local] as usize][axis] +=
                        sample.area_mm2 * sample.values[local] * patch.traction_mpa[axis];
                }
            }
        }
        let response = &solution.fastener_responses[index];
        for axis in 0..3 {
            assert!((force[axis] - response.reaction_force_n[axis]).abs() < 1e-9);
            assert!((moment[axis] - response.reaction_moment_n_mm[axis]).abs() < 1e-8);
        }
        assert!(
            (samples.iter().map(|s| s.area_mm2).sum::<f64>() / response.area_mm2 - 1.0).abs()
                < 1e-12
        );
    }
    let mut internal = vec![[0.0; 3]; mesh.nodes.len()];
    for hex in &mesh.elements {
        let matrix = crate::mapped::stiffness(
            &hex.nodes.map(|node| mesh.nodes[node]),
            hex.midpoint_weight,
            &operator.tensors[hex.role],
        )
        .unwrap();
        for row in 0..81 {
            internal[hex.nodes[row / 3]][row % 3] += (0..81)
                .map(|column| {
                    matrix[81 * row.max(column) + row.min(column)]
                        * solution.displacement_mm[hex.nodes[column / 3]][column % 3]
                })
                .sum::<f64>();
        }
    }
    let free_residual = solve::norm(
        (0..3 * mesh.nodes.len())
            .filter(|&dof| !operator.fixed[dof])
            .map(|dof| {
                internal[dof / 3][dof % 3]
                    - solution.applied_force_n[dof / 3][dof % 3]
                    - reactions[dof / 3][dof % 3]
            }),
    );
    assert!(
        free_residual < 1e-6,
        "original Ku-f-R residual {free_residual}"
    );
    let mut resultant = [0.0; 3];
    let mut moment = [0.0; 3];
    let mut original_energy = 0.0;
    let mut external_work = 0.0;
    let mut reaction_work = 0.0;
    for node in 0..mesh.nodes.len() {
        let total: [f64; 3] = std::array::from_fn(|axis| {
            if operator.fixed[3 * node + axis] {
                reactions[node][axis] = internal[node][axis] - solution.applied_force_n[node][axis];
            }
            solution.applied_force_n[node][axis] + reactions[node][axis]
        });
        let arm = chart::cross(mesh.nodes[node], total);
        for axis in 0..3 {
            resultant[axis] += total[axis];
            moment[axis] += arm[axis];
            original_energy += 0.5 * internal[node][axis] * solution.displacement_mm[node][axis];
            external_work +=
                solution.applied_force_n[node][axis] * solution.displacement_mm[node][axis];
            reaction_work += reactions[node][axis] * solution.displacement_mm[node][axis];
        }
    }
    assert!(solve::norm(resultant) < 1e-6);
    assert!(solve::norm(moment) < 1e-5);
    assert!((original_energy / solution.energy.iter().sum::<f64>() - 1.0).abs() < 1e-7);
    assert!((external_work / (2.0 * original_energy) - 1.0).abs() < 1e-7);
    assert!(reaction_work.abs() < 1e-7 * external_work.abs());
}

#[test]
fn ideal_clamp_native_plate_without_face_restraints_preserves_original_equilibrium() {
    let cancel = AtomicBool::new(false);
    let (prism, case, settings) = attachment_native_plate(&cancel);
    let budget = Budget::new(&cancel, 60);
    let operator = prepare(&prism, &settings, RigidPoseMm::IDENTITY, &case, &budget).unwrap();
    assert!(operator.fixed.iter().all(|fixed| !*fixed));
    assert_eq!(operator.ranks, vec![6]);
    let solution = solve(&operator, &case, &settings, &budget).unwrap();
    attachment_original_equilibrium(&operator, &case, &solution);
    let expected = std::f64::consts::PI * (16.0 - 9.0);
    assert!((solution.fastener_responses[0].area_mm2 / expected - 1.0).abs() < 1e-9);
    for axis in 0..3 {
        assert!(
            (solution.fastener_responses[0].reaction_force_n[axis] + case.loads[0].force_n[axis])
                .abs()
                < 1e-6
        );
        assert!(
            (solution.fastener_responses[0].reaction_moment_n_mm[axis]
                + case.loads[0].moment_n_mm[axis])
                .abs()
                < 1e-5
        );
    }
    // A distributing clamp preserves deformation within the footprint.
    assert!(
        operator.connectors.connections[0]
            .samples
            .iter()
            .any(|sample| {
                let u: [f64; 3] = std::array::from_fn(|axis| {
                    (0..27)
                        .map(|local| {
                            sample.values[local]
                                * solution.displacement_mm[sample.nodes[local] as usize][axis]
                        })
                        .sum()
                });
                solve::norm(u) > 1e-10
            })
    );
}

#[test]
fn ideal_bonded_engagement_native_plate_clips_both_selected_ends() {
    use spiling_contracts::analysis::fastener::*;
    let cancel = AtomicBool::new(false);
    let (prism, mut case, settings) = attachment_native_plate(&cancel);
    let budget = Budget::new(&cancel, 60);
    for end in [HoleEnd::Start, HoleEnd::End] {
        case.fasteners[0].attachment = FastenerAttachment::BondedEngagement {
            end,
            engagement_mm: 3.1,
        };
        let operator = prepare(&prism, &settings, RigidPoseMm::IDENTITY, &case, &budget).unwrap();
        let solution = solve(&operator, &case, &settings, &budget).unwrap();
        attachment_original_equilibrium(&operator, &case, &solution);
        assert!(
            (solution.fastener_responses[0].area_mm2 / (std::f64::consts::TAU * 3.0 * 3.1) - 1.0)
                .abs()
                < 1e-9
        );
        let range = if end == HoleEnd::Start {
            [0.0, 3.1]
        } else {
            [4.9, 8.0]
        };
        for sample in &operator.connectors.connections[0].samples {
            assert!(sample.point_mm[2] >= range[0] && sample.point_mm[2] <= range[1]);
        }
    }
}

#[test]
fn ideal_attachment_load_and_case_edits_reuse_prepared_operator() {
    let cancel = AtomicBool::new(false);
    let (prism, mut case, settings) = attachment_native_plate(&cancel);
    let budget = Budget::new(&cancel, 60);
    let operator = prepare(&prism, &settings, RigidPoseMm::IDENTITY, &case, &budget).unwrap();
    let first = solve(&operator, &case, &settings, &budget).unwrap();
    case.id = AnalysisCaseId::new();
    case.name = "edited service case".into();
    for value in &mut case.loads[0].force_n {
        *value *= 2.0;
    }
    for value in &mut case.loads[0].moment_n_mm {
        *value *= 2.0;
    }
    let second = solve(&operator, &case, &settings, &budget).unwrap();
    attachment_original_equilibrium(&operator, &case, &second);
    for (a, b) in first
        .displacement_mm
        .iter()
        .flatten()
        .zip(second.displacement_mm.iter().flatten())
    {
        assert!((b - 2.0 * a).abs() < 1e-9);
    }
    assert!(
        (second.energy.iter().sum::<f64>() / first.energy.iter().sum::<f64>() - 4.0).abs() < 1e-7
    );
    case.fasteners[0].attachment =
        spiling_contracts::analysis::fastener::FastenerAttachment::Clamp {
            end: spiling_contracts::analysis::fastener::HoleEnd::Start,
            washer_diameter_mm: 12.0,
        };
    assert!(
        solve(&operator, &case, &settings, &budget)
            .err()
            .unwrap()
            .contains("StaleOperator")
    );
}

#[test]
fn ideal_clamp_finite_washer_footprint_changes_compliance() {
    use spiling_contracts::analysis::fastener::*;
    let cancel = AtomicBool::new(false);
    let (prism, mut case, settings) = attachment_native_plate(&cancel);
    let budget = Budget::new(&cancel, 60);
    let mut energies = Vec::new();
    for diameter in [8.0, 12.0] {
        case.fasteners[0].attachment = FastenerAttachment::Clamp {
            end: HoleEnd::Start,
            washer_diameter_mm: diameter,
        };
        let operator = prepare(&prism, &settings, RigidPoseMm::IDENTITY, &case, &budget).unwrap();
        let solution = solve(&operator, &case, &settings, &budget).unwrap();
        attachment_original_equilibrium(&operator, &case, &solution);
        assert!(
            (solution.fastener_responses[0].area_mm2
                / (std::f64::consts::PI * (diameter * diameter * 0.25 - 9.0))
                - 1.0)
                .abs()
                < 1e-9
        );
        energies.push(solution.energy.iter().sum::<f64>());
    }
    assert!(
        (energies[0] / energies[1] - 1.0).abs() > 1e-3,
        "finite washer footprint must affect part response: {energies:?}"
    );
}

#[test]
fn ideal_pin_free_spin_refuses_and_additional_support_resolves_mechanism() {
    use spiling_contracts::analysis::fastener::*;
    let cancel = AtomicBool::new(false);
    let (prism, mut case, settings) = attachment_native_plate(&cancel);
    let budget = Budget::new(&cancel, 60);
    for allow_axial_slide in [false, true] {
        case.fasteners[0].attachment = FastenerAttachment::Pin {
            allow_axial_slide,
            allow_spin: true,
        };
        let error = prepare(&prism, &settings, RigidPoseMm::IDENTITY, &case, &budget)
            .err()
            .unwrap();
        assert!(
            error.contains("DisconnectedOrMechanism") && error.contains("add another support"),
            "{error}"
        );
    }
    case.restraints = vec![AnalysisFaceRestraint {
        face_id: prism.bore.as_ref().unwrap().outer_faces()[0].clone(),
        fixed_translation: [false, true, true],
    }];
    let operator = prepare(&prism, &settings, RigidPoseMm::IDENTITY, &case, &budget).unwrap();
    let solution = solve(&operator, &case, &settings, &budget).unwrap();
    attachment_original_equilibrium(&operator, &case, &solution);
    assert!(
        (solution.fastener_responses[0].area_mm2 / (std::f64::consts::TAU * 3.0 * 8.0) - 1.0).abs()
            < 1e-9
    );
    // Allowed generalized motions are not silently locked by the connection.
    let motions = operator.connectors.connections[0]
        .coupling
        .motions(&solution.displacement_mm);
    assert!(motions[2].abs() > 1e-10 || motions[5].abs() > 1e-10);
}

#[test]
fn ideal_attachment_samples_are_canonical_and_projection_preserves_rigid_modes() {
    use spiling_contracts::analysis::{attachment, chart};
    let cancel = AtomicBool::new(false);
    let (prism, case, settings) = attachment_native_plate(&cancel);
    let budget = Budget::new(&cancel, 60);
    let operator = prepare(&prism, &settings, RigidPoseMm::IDENTITY, &case, &budget).unwrap();
    let mut facets = attachment_facets(&operator.mesh);
    let a = attachment::samples(&case.fasteners[0], &facets).unwrap();
    facets.reverse();
    let b = attachment::samples(&case.fasteners[0], &facets).unwrap();
    assert_eq!(a.len(), b.len());
    assert!(a.len() <= 20_000);
    for (a, b) in a.iter().zip(&b) {
        assert_eq!(a.element_index, b.element_index);
        assert_eq!(a.local_point, b.local_point);
        assert_eq!(a.point_mm, b.point_mm);
        assert_eq!(a.area_mm2, b.area_mm2);
        assert_eq!(a.values, b.values);
    }
    let coupling = attachment::coupling(&case.fasteners[0], &a, operator.mesh.nodes.len()).unwrap();
    let center = case.fasteners[0].hole.center_mm;
    let scale = case.fasteners[0].hole.depth_mm.max(4.0);
    for mode in 0..6 {
        let u = operator
            .mesh
            .nodes
            .iter()
            .map(|p| {
                if mode < 3 {
                    std::array::from_fn(|axis| if axis == mode { 1.0 } else { 0.0 })
                } else {
                    let unit = std::array::from_fn(|axis| if axis == mode - 3 { 1.0 } else { 0.0 });
                    chart::cross(
                        unit,
                        std::array::from_fn(|axis| (p[axis] - center[axis]) / scale),
                    )
                }
            })
            .collect::<Vec<_>>();
        let motions = coupling.motions(&u);
        for (motion, value) in motions.iter().enumerate() {
            assert!((value - if motion == mode { 1.0 } else { 0.0 }).abs() < 1e-10);
        }
    }
}

#[test]
fn ideal_attachment_incomplete_or_oversized_seat_refuses() {
    use spiling_contracts::analysis::{attachment, fastener::*};
    let cancel = AtomicBool::new(false);
    let (prism, mut case, settings) = attachment_native_plate(&cancel);
    let budget = Budget::new(&cancel, 60);
    let operator = prepare(&prism, &settings, RigidPoseMm::IDENTITY, &case, &budget).unwrap();
    let mut facets = attachment_facets(&operator.mesh);
    facets.retain(|facet| {
        !(facet.axis == 2 && facet.side == 0 && facet.positions.iter().any(|point| point[0] > 10.0))
    });
    assert!(attachment::samples(&case.fasteners[0], &facets).is_err());
    case.fasteners[0].attachment = FastenerAttachment::Clamp {
        end: HoleEnd::Start,
        washer_diameter_mm: 21.0,
    };
    assert!(
        prepare(&prism, &settings, RigidPoseMm::IDENTITY, &case, &budget)
            .err()
            .unwrap()
            .contains("complete washer annulus")
    );
}
#[test]
fn ideal_washer_quadrature_crosses_radial_bands_on_both_caps() {
    use spiling_contracts::analysis::{attachment, fastener::*};
    let cancel = AtomicBool::new(false);
    let (prism, mut case, settings) = attachment_native_plate(&cancel);
    let budget = Budget::new(&cancel, 60);
    let operator = prepare(&prism, &settings, RigidPoseMm::IDENTITY, &case, &budget).unwrap();
    let facets = attachment_facets(&operator.mesh);
    for end in [HoleEnd::Start, HoleEnd::End] {
        // The 9.8 mm radius crosses the core/outer-wall radial interface near
        // the side centers but remains inside the physical rectangular plate.
        case.fasteners[0].attachment = FastenerAttachment::Clamp {
            end,
            washer_diameter_mm: 19.6,
        };
        let samples = attachment::samples(&case.fasteners[0], &facets).unwrap();
        let expected = std::f64::consts::PI * (9.8_f64.powi(2) - 9.0);
        assert!(
            (samples.iter().map(|sample| sample.area_mm2).sum::<f64>() / expected - 1.0).abs()
                < 1e-9
        );
        let wall = settings.recipe.perimeter_count as f64 * settings.recipe.bead_width_mm;
        let mut bands = [false; 3];
        for sample in samples {
            let radius = (sample.point_mm[0] - 10.0).hypot(sample.point_mm[1] - 10.0);
            assert!((3.0 - 1e-10..=9.8 + 1e-10).contains(&radius));
            assert!(
                (sample.point_mm[2] - if end == HoleEnd::Start { 0.0 } else { 8.0 }).abs() < 1e-10
            );
            assert_eq!(
                operator.mesh.elements[sample.element_index as usize].role,
                1
            );
            let boundary_distance = [sample.point_mm[0], sample.point_mm[1]]
                .into_iter()
                .map(|coordinate| coordinate.min(20.0 - coordinate))
                .fold(f64::INFINITY, f64::min);
            let band = if radius < 3.0 + wall {
                0
            } else if boundary_distance < wall {
                2
            } else {
                1
            };
            bands[band] = true;
        }
        assert_eq!(bands, [true; 3]);
    }
}

#[test]
fn ideal_washer_band_tangencies_have_unique_physical_samples() {
    use spiling_contracts::analysis::{attachment, fastener::*};
    let cancel = AtomicBool::new(false);
    let (prism, mut case, settings) = attachment_native_plate(&cancel);
    let budget = Budget::new(&cancel, 60);
    let operator = prepare(&prism, &settings, RigidPoseMm::IDENTITY, &case, &budget).unwrap();
    let facets = attachment_facets(&operator.mesh);
    for diameter in [8.0, 10.8, 11.9999, 12.0, 12.0001, 16.0, 19.6, 20.0] {
        case.fasteners[0].attachment = FastenerAttachment::Clamp {
            end: HoleEnd::End,
            washer_diameter_mm: diameter,
        };
        let samples = attachment::samples(&case.fasteners[0], &facets).unwrap();
        let mut seen = std::collections::BTreeMap::new();
        for sample in samples {
            let key = (sample.element_index, sample.point_mm.map(f64::to_bits));
            assert!(
                seen.insert(key, (sample.local_point, sample.area_mm2))
                    .is_none(),
                "diameter={diameter} element={} local={:?} point={:?} area={} previous={:?}",
                sample.element_index,
                sample.local_point,
                sample.point_mm,
                sample.area_mm2,
                seen.get(&key),
            );
        }
    }
}
