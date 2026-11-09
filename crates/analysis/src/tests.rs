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
            face_id: FaceId::from_step_entity(2).unwrap(),
            force_n: [0.0; 3],
            moment_n_mm: [0.0, 0.0, 10.0],
        }],
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
