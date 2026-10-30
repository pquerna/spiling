// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
use super::*;
use geo::Area;
use spiling_contracts::{
    geometry::{DefinitionId, OccurrenceId},
    process::{ProcessLayerId, ProcessOperationId, ProcessProvenance, ResolvedProcessValue},
};
fn rectangle(x: [f64; 2], y: [f64; 2]) -> DiaphragmPolygonMm {
    DiaphragmPolygonMm {
        outer: vec![
            [x[0], y[0]],
            [x[1], y[0]],
            [x[1], y[1]],
            [x[0], y[1]],
            [x[0], y[0]],
        ],
        holes: Vec::new(),
    }
}
fn domain() -> SkinCommandFitDomain {
    SkinCommandFitDomain {
        region_index: 0,
        region: ProcessRegionCoverage {
            material_domain: None,
            area_mm2: 23.96,
            fraction: 1.0,
            contributions: vec![ProcessRegionContribution {
                occurrence_id: OccurrenceId::new(1).unwrap(),
                definition_id: DefinitionId::from_source_sha256(&[13; 32]),
                provenance: None,
            }],
        },
        skin: ProcessSkinKind::BottomSkin,
        regional: vec![rectangle([2.01, 8.0], [0.0, 4.0])],
        mandatory: vec![rectangle([2.0, 2.01], [1.0, 3.0])],
        source_owner: vec![rectangle([1.0, 8.0], [0.0, 4.0])],
        fits: Vec::new(),
    }
}
fn paths() -> Vec<DepositionPath> {
    vec![DepositionPath {
        material_domain: None,
        kind: DepositionKind::BottomSkin,
        owner: Some(DepositionOwner::MandatorySkin {
            skin: ProcessSkinKind::BottomSkin,
        }),
        width: WidthProfile::Constant { width_mm: 0.45 },
        physical_z_mm: [0.0, 0.4],
        points_mm: vec![[2.0, 1.5, 0.4], [2.01, 1.5, 0.4]],
    }]
}
fn select(domain: &SkinCommandFitDomain, paths: &[DepositionPath]) -> Option<[[f64; 3]; 2]> {
    let mut rows = Rows::default();
    let mut forbidden = Vec::new();
    let mut work = 0;
    choose(
        domain,
        [paths[0].points_mm[0], paths[0].points_mm[1]],
        0,
        0,
        paths,
        &mut rows,
        &mut forbidden,
        &Budget { bytes: 0 },
        0,
        &mut work,
        &mut |units, bytes, _| crate::planner::remaining_geometry_units(units, bytes).map(|_| ()),
        &AtomicBool::new(false),
    )
    .unwrap()
}
#[test]
fn minimum_skin_keeps_complete_requirement_and_extends_only_toward_ordinary_material() {
    let mut domain = domain();
    let paths = paths();
    let fitted = select(&domain, &paths).unwrap();
    assert_eq!(fitted[0], paths[0].points_mm[0]);
    assert!(fitted[1][0] > paths[0].points_mm[1][0]);
    assert!(fitted.iter().all(|p| p[1] == 1.5 && p[2] == 0.4));
    let commanded = [
        project_xyz6_point_mm(fitted[0]).unwrap(),
        project_xyz6_point_mm(fitted[1]).unwrap(),
    ];
    assert!(commanded[1][0] - commanded[0][0] >= MIN_DEPOSITION_COMMAND_MM);
    assert!(fitted[1][0] - fitted[0][0] <= MIN_DEPOSITION_COMMAND_MM + 0.000003 + 1e-14);
    domain.fits.push(SkinCommandFit {
        required_mm: [paths[0].points_mm[0], paths[0].points_mm[1]],
        fitted_mm: fitted,
    });
    domain.validate().unwrap();
}
#[test]
fn minimum_skin_never_extends_through_a_protected_source_hole() {
    let mut domain = domain();
    domain.source_owner[0]
        .holes
        .push(rectangle([2.015, 2.04], [1.0, 2.0]).outer);
    assert!(select(&domain, &paths()).is_none());
}
#[test]
fn minimum_skin_cannot_leave_a_positive_subminimum_regional_remainder() {
    let mut paths = paths();
    paths.push(DepositionPath {
        material_domain: None,
        kind: DepositionKind::SolidFill,
        owner: Some(DepositionOwner::Regional { region_index: 0 }),
        width: WidthProfile::Constant { width_mm: 0.45 },
        physical_z_mm: [0.0, 0.4],
        points_mm: vec![[2.02, 1.5, 0.4], [2.04, 1.5, 0.4]],
    });
    assert!(select(&domain(), &paths).is_none());
    let mut alternate = domain();
    alternate.regional.push(rectangle([1.0, 2.0], [0.0, 4.0]));
    let fitted = select(&alternate, &paths).unwrap();
    assert!(fitted[0][0] < 2.0 && fitted[1][0] < 2.02);
    assert!(fitted[0][0] <= 2.0 && fitted[1][0] >= 2.01);
}
#[test]
fn source_owner_uses_effective_winner_halfspace_not_unsplit_base_fraction() {
    let provenance = ProcessProvenance {
        layer_id: ProcessLayerId::new(1).unwrap(),
        operation_id: ProcessOperationId::new(1).unwrap(),
    };
    let mut target = domain().region;
    target.area_mm2 = 21.0;
    target.fraction = 0.8;
    target.contributions[0].provenance = Some(provenance);
    let mut material = MultiPolygon(vec![
        geo::Rect::new(
            geo::Coord { x: 0.0, y: 0.0 },
            geo::Coord { x: 10.0, y: 3.0 },
        )
        .to_polygon(),
    ]);
    let bytes = crate::planner::native::material_bytes(&mut material);
    let mut base = target.clone();
    base.fraction = 0.2;
    base.contributions[0].provenance = None;
    let sources = [crate::planner::ProcessDomain {
        material: Arc::new(material),
        coverage: base,
        bytes,
        zone: Some(crate::planner::zones::Zone {
            axis: 0,
            coordinate: 3.0,
            frame_origin_mm: [0.0, 0.0],
            values: [
                ResolvedProcessValue {
                    fraction: 0.2,
                    provenance: None,
                },
                ResolvedProcessValue {
                    fraction: 0.8,
                    provenance: Some(provenance),
                },
            ],
            provenance,
            authorized: true,
            diaphragm: None,
        }),
        physical_z_mm: [0.0, 0.4],
    }];
    let owner = source_owner(
        &target,
        &sources,
        &Budget { bytes },
        0,
        &mut 0,
        &mut |units, bytes, _| crate::planner::remaining_geometry_units(units, bytes).map(|_| ()),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!((owner.unsigned_area() - 21.0).abs() < 1e-8);
    assert!(!owner.contains(&geo::Point::new(2.5, 1.5)));
    assert!(owner.contains(&geo::Point::new(3.5, 1.5)));
}

#[test]
fn minimum_skin_searches_shorter_permitted_lengths_to_preserve_regional_material() {
    let mut domain = domain();
    domain.mandatory = vec![rectangle([0.0, 0.010], [1.0, 3.0])];
    domain.regional = vec![rectangle([0.010, 0.040002], [0.0, 4.0])];
    domain.source_owner = vec![rectangle([0.0, 0.040002], [0.0, 4.0])];
    let mut paths = paths();
    paths[0].points_mm = vec![[0.0, 1.5, 0.4], [0.010, 1.5, 0.4]];
    paths.push(DepositionPath {
        material_domain: None,
        kind: DepositionKind::SolidFill,
        owner: Some(DepositionOwner::Regional { region_index: 0 }),
        width: WidthProfile::Constant { width_mm: 0.45 },
        physical_z_mm: [0.0, 0.4],
        points_mm: vec![[0.010, 1.5, 0.4], [0.040002, 1.5, 0.4]],
    });
    let fitted = select(&domain, &paths).unwrap();
    assert!(fitted[0][0] <= 0.0 && fitted[1][0] >= 0.010);
    assert!(fitted[1][0] < 0.020003);
    assert!(fitted[1][0] - fitted[0][0] >= MIN_DEPOSITION_COMMAND_MM);
    assert!(0.040002 - fitted[1][0] >= MIN_DEPOSITION_COMMAND_MM);
}

#[test]
fn regional_reservation_consumes_complete_actual_stroke_without_a_zero_command_tail() {
    let mut paths = paths();
    paths[0].points_mm = vec![[0.0, 1.5, 0.4], [0.000003, 1.5, 0.4]];
    paths.push(DepositionPath {
        material_domain: None,
        kind: DepositionKind::SolidFill,
        owner: Some(DepositionOwner::Regional { region_index: 0 }),
        width: WidthProfile::Constant { width_mm: 0.45 },
        physical_z_mm: [0.0, 0.4],
        points_mm: vec![[0.000003, 1.5, 0.4], [0.0200034, 1.5, 0.4]],
    });
    let original = [
        project_xyz6_point_mm(paths[1].points_mm[0]).unwrap(),
        project_xyz6_point_mm(paths[1].points_mm[1]).unwrap(),
    ];
    let fitted = [[0.0, 1.5, 0.4], [0.020003, 1.5, 0.4]];
    let mut budget = Budget {
        bytes: crate::planner::bonds::path_bytes(&paths),
    };
    let mut segments = 2;
    reserve_regional_material(
        &mut paths,
        0,
        0,
        fitted,
        0,
        &mut segments,
        &mut budget,
        0,
        &mut 0,
        &mut |units, bytes, _| crate::planner::remaining_geometry_units(units, bytes).map(|_| ()),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(paths[1].points_mm.is_empty());
    assert!(fitted[0][0] <= original[0][0] && original[1][0] <= fitted[1][0]);
    assert_eq!(segments, 1);
}

#[test]
fn vertical_source_winner_uses_the_captured_interval_below_an_event_plane() {
    let provenance = ProcessProvenance {
        layer_id: ProcessLayerId::new(1).unwrap(),
        operation_id: ProcessOperationId::new(1).unwrap(),
    };
    let target = domain().region;
    let material = MultiPolygon(vec![
        geo::Rect::new(
            geo::Coord { x: 0.0, y: 0.0 },
            geo::Coord { x: 10.0, y: 3.0 },
        )
        .to_polygon(),
    ]);
    let source = crate::planner::ProcessDomain {
        material: Arc::new(material),
        coverage: target.clone(),
        bytes: 0,
        zone: Some(crate::planner::zones::Zone {
            axis: 2,
            coordinate: 0.4,
            frame_origin_mm: [0.0, 0.0],
            values: [
                ResolvedProcessValue {
                    fraction: 1.0,
                    provenance: None,
                },
                ResolvedProcessValue {
                    fraction: 1.0,
                    provenance: Some(provenance),
                },
            ],
            provenance,
            authorized: true,
            diaphragm: None,
        }),
        physical_z_mm: [0.0, 0.4],
    };
    let sources = [source];
    let owner = source_owner(
        &target,
        &sources,
        &Budget { bytes: 0 },
        0,
        &mut 0,
        &mut |units, bytes, _| crate::planner::remaining_geometry_units(units, bytes).map(|_| ()),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!((owner.unsigned_area() - 30.0).abs() < 1e-8);
    let mut other_winner = target;
    other_winner.contributions[0].provenance = Some(provenance);
    let wrong = source_owner(
        &other_winner,
        &sources,
        &Budget { bytes: 0 },
        0,
        &mut 0,
        &mut |units, bytes, _| crate::planner::remaining_geometry_units(units, bytes).map(|_| ()),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(wrong.unsigned_area(), 0.0);
}

#[test]
fn minimum_skin_keeps_nonrepresentable_unextended_endpoints_verbatim() {
    let mut domain = domain();
    domain.mandatory = vec![rectangle([2.0000003, 2.0100003], [1.0, 3.0])];
    domain.regional = vec![rectangle([2.0100003, 8.0], [0.0, 4.0])];
    let mut paths = paths();
    paths[0].points_mm = vec![[2.0000003, 1.5, 0.4], [2.0100003, 1.5, 0.4]];
    let fitted = select(&domain, &paths).unwrap();
    assert_eq!(fitted[0], paths[0].points_mm[0]);
    assert!(fitted[1][0] > paths[0].points_mm[1][0]);
    let actual = [
        project_xyz6_point_mm(fitted[0]).unwrap(),
        project_xyz6_point_mm(fitted[1]).unwrap(),
    ];
    assert!(actual[1][0] - actual[0][0] >= MIN_DEPOSITION_COMMAND_MM);
    assert!(fitted[1][0] - fitted[0][0] <= MIN_DEPOSITION_COMMAND_MM + 0.000003 + 1e-14);
}
