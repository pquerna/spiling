// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use geo::{MapCoords, polygon};

struct Scene {
    candidate: RasterCandidate,
    required: DepositionPath,
    previous: DepositionPath,
    contact: DepositionPath,
    recipe: PlanarPrintRecipe,
}

fn scene(skin: ProcessSkinKind, axis: region::HatchAxis, hole: bool) -> Scene {
    let kind = match skin {
        ProcessSkinKind::BottomSkin => DepositionKind::BottomSkin,
        ProcessSkinKind::TopSkin => DepositionKind::TopSkin,
    };
    let mut required = DepositionPath {
        material_domain: None,
        kind,
        width: WidthProfile::Constant { width_mm: 0.6 },
        physical_z_mm: [0.8, 1.2],
        owner: Some(DepositionOwner::MandatorySkin { skin }),
        points_mm: vec![[1.1, 0.0, 1.2], [1.355, 0.0, 1.2]],
    };
    let mut previous = DepositionPath {
        material_domain: None,
        kind: DepositionKind::SparseFill,
        width: WidthProfile::Constant { width_mm: 0.6 },
        physical_z_mm: [0.4, 0.8],
        owner: None,
        points_mm: vec![[0.75, -1.0, 0.8], [0.75, 1.0, 0.8]],
    };
    let mut contact = DepositionPath {
        material_domain: None,
        kind: DepositionKind::SupportInterface,
        width: WidthProfile::Constant { width_mm: 0.6 },
        physical_z_mm: [0.0, 0.4],
        owner: None,
        points_mm: vec![[3.0, -1.0, 0.4], [3.0, 1.0, 0.4]],
    };
    let mut fill = polygon![
        (x: 1.1, y: -2.0), (x: 4.0, y: -2.0),
        (x: 4.0, y: 2.0), (x: 1.1, y: 2.0), (x: 1.1, y: -2.0),
    ];
    if hole {
        fill.interiors_push(geo::LineString::from(vec![
            (1.8, -0.4),
            (1.8, 0.4),
            (2.6, 0.4),
            (2.6, -0.4),
            (1.8, -0.4),
        ]));
    }
    if axis == region::HatchAxis::Y {
        for path in [&mut required, &mut previous, &mut contact] {
            for point in &mut path.points_mm {
                point.swap(0, 1);
            }
        }
        fill = fill.map_coords(|coordinate| geo::Coord {
            x: coordinate.y,
            y: coordinate.x,
        });
    }
    let recipe = crate::planner::tests::recipe(1.0);
    let layer = region::HatchLayer {
        z: 1.2,
        lower_z: 0.8,
        height: 0.4,
        axis,
        top_axis: axis,
        recipe: &recipe,
    };
    let candidate = RasterCandidate {
        paths: vec![required.clone()],
        whole_rows: region::bridge_rows(
            &MultiPolygon(vec![fill]),
            &[],
            std::iter::once(&required),
            &layer,
            &mut 0,
            &AtomicBool::new(false),
        )
        .unwrap(),
        process_interfaces: Vec::new(),
        skin_bridge_domains: Vec::new(),
        skin_command_fits: Vec::new(),
    };
    Scene {
        candidate,
        required,
        previous,
        contact,
        recipe,
    }
}

#[test]
fn demanded_skin_contact_extension_preserves_core_and_requires_actual_contact_gap() {
    for skin in [ProcessSkinKind::BottomSkin, ProcessSkinKind::TopSkin] {
        for axis in [region::HatchAxis::X, region::HatchAxis::Y] {
            let mut scene = scene(skin, axis, false);
            let cancel = AtomicBool::new(false);
            let mut anchors = AnchorWorkspace::default();
            let mut work = 0;
            let proof = component_admission(
                (&mut scene.candidate, axis, 100_000, true),
                std::iter::once(&scene.previous),
                std::iter::once(&scene.contact),
                &scene.recipe,
                &mut anchors,
                &mut work,
                &cancel,
            )
            .unwrap();
            assert!(proof.supported);
            assert_eq!(proof.bridges, [0]);
            assert_eq!(scene.candidate.paths.len(), 1);
            let path = &scene.candidate.paths[0];
            let along = usize::from(axis == region::HatchAxis::Y);
            assert_eq!(path.kind, DepositionKind::Bridge);
            assert_eq!(path.owner, scene.required.owner);
            assert_eq!(path.width, scene.required.width);
            assert_eq!(path.physical_z_mm, scene.required.physical_z_mm);
            assert_eq!(path.points_mm[0], scene.required.points_mm[0]);
            assert!(path.points_mm[1][along] > scene.required.points_mm[1][along]);
            assert!(path.points_mm[1][along] < scene.contact.points_mm[0][along]);
            assert!(path.points_mm[1][along] - path.points_mm[0][along] > 1.5);
            let floor = program_coordinates::project_xyz6_bead_height_mm(1.2, 0.4).unwrap()[0];
            let top = program_coordinates::project_xyz6_bead_height_mm(0.4, 0.4).unwrap()[1];
            assert!(program_coordinates::enclosed_vertical_gap_mm(floor, top).unwrap() >= 0.2);
            let actual = strokes::analyze(
                &scene.candidate.paths,
                std::iter::once(&scene.previous),
                std::iter::once(&scene.contact),
                &scene.recipe,
                &mut anchors,
                &mut work,
                &cancel,
            )
            .unwrap();
            assert!(actual.failure.is_none());
            assert_eq!(actual.bridges, [0]);
            let absent = strokes::analyze(
                &scene.candidate.paths,
                std::iter::once(&scene.previous),
                std::iter::empty(),
                &scene.recipe,
                &mut anchors,
                &mut work,
                &cancel,
            )
            .unwrap();
            assert!(absent.failure.is_some());
        }
    }
}

#[test]
fn demanded_skin_contact_extension_refuses_foreign_ineligible_or_noncovering_contacts() {
    for case in 0..8 {
        let mut scene = scene(ProcessSkinKind::BottomSkin, region::HatchAxis::X, case == 7);
        let demanded_reorientation = case != 0;
        match case {
            1 => {
                scene.candidate.paths[0].owner = Some(DepositionOwner::Regional { region_index: 0 })
            }
            2 => scene.contact.kind = DepositionKind::SolidFill,
            3 => scene.contact.physical_z_mm = [0.8, 1.2],
            4 => {
                for point in &mut scene.contact.points_mm {
                    point[1] += 10.0;
                }
            }
            5 => scene.recipe.process.max_bridge_span_mm = 1.0,
            _ => {}
        }
        let required = scene.candidate.paths.clone();
        let proof = component_admission(
            (
                &mut scene.candidate,
                region::HatchAxis::X,
                100_000,
                demanded_reorientation,
            ),
            std::iter::once(&scene.previous),
            std::iter::once(&scene.contact).filter(|_| case != 6),
            &scene.recipe,
            &mut AnchorWorkspace::default(),
            &mut 0,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(proof.bridges.is_empty(), "case {case}");
        assert_eq!(scene.candidate.paths, required, "case {case}");
    }
}

#[test]
fn demanded_skin_contact_extension_never_promotes_supported_or_tiny_spans() {
    for tiny_gap in [false, true] {
        let mut scene = scene(ProcessSkinKind::BottomSkin, region::HatchAxis::X, false);
        if tiny_gap {
            for point in &mut scene.previous.points_mm {
                point[0] = 1.0;
            }
            for point in &mut scene.contact.points_mm {
                point[0] = 1.605;
            }
            scene.candidate.paths[0].points_mm = vec![[1.301, 0.0, 1.2], [1.304, 0.0, 1.2]];
        } else {
            for point in &mut scene.contact.points_mm {
                point[0] = 1.2;
            }
        }
        let required = scene.candidate.paths.clone();
        let proof = component_admission(
            (&mut scene.candidate, region::HatchAxis::X, 100_000, true),
            std::iter::once(&scene.previous),
            std::iter::once(&scene.contact),
            &scene.recipe,
            &mut AnchorWorkspace::default(),
            &mut 0,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(proof.supported);
        assert!(proof.bridges.is_empty());
        assert_eq!(scene.candidate.paths, required);
    }
}

#[test]
fn consumed_contact_bridge_survives_authored_axis_merge_and_fresh_analyzer() {
    let mut scene = scene(ProcessSkinKind::BottomSkin, region::HatchAxis::X, false);
    let cancel = AtomicBool::new(false);
    let mut anchors = AnchorWorkspace::default();
    let mut work = 0;
    let admitted = component_admission(
        (&mut scene.candidate, region::HatchAxis::X, 100_000, true),
        std::iter::once(&scene.previous),
        std::iter::once(&scene.contact),
        &scene.recipe,
        &mut anchors,
        &mut work,
        &cancel,
    )
    .unwrap();
    assert!(admitted.supported);
    assert_eq!(admitted.bridges, [0]);
    assert_eq!(scene.candidate.paths[0].kind, DepositionKind::Bridge);
    assert_eq!(scene.candidate.paths[0].owner, scene.required.owner);
    scene.candidate.whole_rows.clear();
    let frozen = scene.candidate.paths.clone();
    let merged = component_admission(
        (&mut scene.candidate, region::HatchAxis::Y, 100_000, false),
        std::iter::once(&scene.previous),
        std::iter::once(&scene.contact),
        &scene.recipe,
        &mut anchors,
        &mut work,
        &cancel,
    )
    .unwrap();
    assert!(merged.supported);
    assert_eq!(merged.bridges, admitted.bridges);
    assert_eq!(scene.candidate.paths, frozen);
    let actual = strokes::analyze(
        &scene.candidate.paths,
        std::iter::once(&scene.previous),
        std::iter::once(&scene.contact),
        &scene.recipe,
        &mut anchors,
        &mut work,
        &cancel,
    )
    .unwrap();
    assert!(actual.failure.is_none());
    assert_eq!(actual.bridges, merged.bridges);
}

#[test]
fn consumed_contact_bridge_revalidation_requires_complete_actual_gap_proof() {
    for case in 0..7 {
        let mut scene = scene(ProcessSkinKind::BottomSkin, region::HatchAxis::X, false);
        scene.candidate.whole_rows.clear();
        scene.candidate.paths[0].kind = DepositionKind::Bridge;
        scene.candidate.paths[0].points_mm[1][0] = 2.7;
        match case {
            0 => scene.candidate.paths[0].kind = DepositionKind::BottomSkin,
            2 => {
                for point in &mut scene.contact.points_mm {
                    point[1] += 10.0;
                }
            }
            3 => scene.contact.physical_z_mm = [0.8, 1.2],
            4 => scene.recipe.process.max_bridge_span_mm = 1.0,
            5 => scene.contact.points_mm = vec![[1.0, 0.0, 0.4], [3.0, 0.0, 0.4]],
            6 => {
                for point in &mut scene.previous.points_mm {
                    point[0] = 1.0;
                }
                for point in &mut scene.contact.points_mm {
                    point[0] = 1.605;
                }
                scene.candidate.paths[0].points_mm = vec![[1.301, 0.0, 1.2], [1.304, 0.0, 1.2]];
            }
            _ => {}
        }
        let frozen = scene.candidate.paths.clone();
        let contact = std::iter::once(&scene.contact).filter(|_| case != 1);
        let mut anchors = AnchorWorkspace::default();
        let mut work = 0;
        let cancel = AtomicBool::new(false);
        let merged = component_admission(
            (&mut scene.candidate, region::HatchAxis::Y, 100_000, false),
            std::iter::once(&scene.previous),
            contact.clone(),
            &scene.recipe,
            &mut anchors,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert!(merged.bridges.is_empty(), "case {case}");
        assert_eq!(merged.supported, matches!(case, 0 | 5 | 6), "case {case}");
        assert_eq!(scene.candidate.paths, frozen, "case {case}");
        let actual = strokes::analyze(
            &scene.candidate.paths,
            std::iter::once(&scene.previous),
            contact,
            &scene.recipe,
            &mut anchors,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert_eq!(actual.failure.is_none(), merged.supported, "case {case}");
        assert_eq!(actual.bridges, merged.bridges, "case {case}");
    }
}

#[test]
fn demanded_skin_contact_extension_preserves_work_retention_and_cancellation_bounds() {
    for case in 0..3 {
        let mut scene = scene(ProcessSkinKind::TopSkin, region::HatchAxis::X, false);
        let mut work = if case == 0 {
            anchors::MAX_ANCHOR_PAIR_WORK
        } else {
            0
        };
        let failure = component_admission(
            (
                &mut scene.candidate,
                region::HatchAxis::X,
                if case == 1 { 0 } else { 100_000 },
                true,
            ),
            std::iter::once(&scene.previous),
            std::iter::once(&scene.contact),
            &scene.recipe,
            &mut AnchorWorkspace::default(),
            &mut work,
            &AtomicBool::new(case == 2),
        )
        .err()
        .expect("bounded contact construction must refuse");
        assert_eq!(
            failure.code,
            if case == 2 {
                ManufacturingErrorCode::Cancelled
            } else {
                ManufacturingErrorCode::ResourceLimit
            }
        );
        assert_eq!(scene.candidate.paths, [scene.required]);
    }
}
