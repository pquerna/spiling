// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use geo::{LineString, Polygon, Relate, Validation};

fn concave() -> MultiPolygon<f64> {
    MultiPolygon(vec![Polygon::new(
        LineString::from(vec![
            (-1.0, 0.0),
            (0.0, 0.0),
            (0.03, 0.01),
            (0.049, 0.014),
            (0.068, 0.016),
            (0.087, 0.016),
            (0.106, 0.014),
            (0.125, 0.01),
            (0.155, 0.0),
            (1.0, 0.0),
            (1.0, 1.0),
            (-1.0, 1.0),
            (-1.0, 0.0),
        ]),
        vec![],
    )])
}

fn wide_concave(hole: bool) -> MultiPolygon<f64> {
    MultiPolygon(vec![Polygon::new(
        LineString::from(vec![
            (-1.0, 0.0),
            (0.0, 0.0),
            (0.5, 0.1),
            (1.0, 0.0),
            (2.0, 0.0),
            (2.0, 2.0),
            (-1.0, 2.0),
            (-1.0, 0.0),
        ]),
        if hole {
            vec![LineString::from(vec![
                (0.49, 0.29),
                (0.49, 0.31),
                (0.51, 0.31),
                (0.51, 0.29),
                (0.49, 0.29),
            ])]
        } else {
            vec![]
        },
    )])
}

fn candidate_admissible(
    domain: &MultiPolygon<f64>,
    interface: Option<&MultiPolygon<f64>>,
    point: Coord<f64>,
) -> bool {
    let cancel = AtomicBool::new(false);
    let mut work = 0;
    let mut search = Search {
        current: domain,
        original: domain,
        interface,
        retained: 5,
        owned_bytes: 5 * std::mem::size_of::<Coord<f64>>(),
        work: &mut work,
        foreign_work: 0,
        cancel: &cancel,
        cuts: Vec::new(),
        pocket: LineString::new(Vec::with_capacity(5)),
    };
    let ring = &domain.0[0].exterior().0;
    search
        .admissible(
            &Ear {
                polygon: 0,
                ring: 0,
                vertex: 2,
                constraint: 2,
                previous_constraint: 1,
            },
            [ring[1], ring[2], ring[3]],
            point,
        )
        .unwrap()
}

#[test]
fn inward_chain_fixes_concave_commands_without_changing_original_domain_or_hatches() {
    let centers = concave();
    let original = centers.clone();
    let cancel = AtomicBool::new(false);
    let mut paths = Vec::new();
    let mut segments = 0;
    region::support_contour_paths(
        &centers,
        0.2,
        0.0,
        0.4,
        DepositionKind::Support,
        &mut paths,
        &mut segments,
        &cancel,
    )
    .unwrap();
    // Unmodified walls fit only by omitting part of the concave corner.
    assert!(
        commands::fit(
            &centers,
            &mut paths,
            &mut segments,
            0,
            0,
            &mut 0,
            0,
            &cancel,
        )
        .unwrap()
            > 0.0
    );
    let proposal = construct(&centers, None, 0, 0, &mut 0, 0, &cancel)
        .unwrap()
        .unwrap();
    assert!(proposal.is_valid());
    assert_eq!(proposal.0.len(), centers.0.len());
    assert_eq!(proposal.0[0].interiors().len(), 0);
    for edge in proposal.0[0].exterior().0.windows(2) {
        assert!(commands::command_length(edge[0], edge[1]).unwrap());
        assert!(centers.relate(&Line::new(edge[0], edge[1])).is_covers());
    }
    paths.clear();
    segments = 0;
    region::support_contour_paths(
        &proposal,
        0.2,
        0.0,
        0.4,
        DepositionKind::Support,
        &mut paths,
        &mut segments,
        &cancel,
    )
    .unwrap();
    // The inward proposal keeps the complete wall.
    assert_eq!(
        commands::fit(
            &centers,
            &mut paths,
            &mut segments,
            0,
            0,
            &mut 0,
            0,
            &cancel,
        )
        .unwrap(),
        0.0
    );
    assert_eq!(centers, original);
    let recipe = crate::planner::tests::recipe(1.0);
    let domains = Domains {
        centers: centers.clone(),
        wall_centers: Some(proposal),
        ordinary: None,
        interface: None,
        all_interface: false,
        retained: 0,
        owned_bytes: 0,
        anchor_owned_bytes: 0,
    };
    for axis in [region::HatchAxis::X, region::HatchAxis::Y] {
        let layer = region::HatchLayer {
            roof: None,
            width_mm: recipe.bead_width_mm,
            z: 0.2,
            lower_z: 0.0,
            height: 0.2,
            axis,
            top_axis: axis,
            recipe: &recipe,
        };
        let mut expected = Vec::new();
        region::hatch(
            &centers,
            DepositionKind::Support,
            1.0,
            &layer,
            &mut expected,
            &mut 0,
            &cancel,
        )
        .unwrap();
        let actual = domains
            .paths(
                &MultiPolygon(Vec::new()),
                1.0,
                &layer,
                0,
                &mut 0,
                0,
                &cancel,
            )
            .unwrap()
            .0;
        assert_eq!(&actual[1..], expected);
    }
}

#[test]
fn outside_concavity_chord_is_not_authorized_by_nearby_inward_proposal() {
    let domain = wide_concave(false);
    let first = Coord { x: 0.0, y: 0.0 };
    let last = Coord { x: 1.0, y: 0.0 };
    assert!(!domain.relate(&Line::new(first, last)).is_covers());
    assert!(candidate_admissible(
        &domain,
        None,
        Coord { x: 0.5, y: 1.0 }
    ));
    assert!(!candidate_admissible(
        &domain,
        None,
        Coord { x: 0.5, y: 0.05 }
    ));
}

#[test]
fn contained_replacement_segments_cannot_swallow_an_unchanged_hole() {
    let domain = wide_concave(true);
    let point = Coord { x: 0.5, y: 1.0 };
    for edge in [
        [Coord { x: 0.0, y: 0.0 }, point],
        [point, Coord { x: 1.0, y: 0.0 }],
    ] {
        assert!(domain.relate(&Line::new(edge[0], edge[1])).is_covers());
    }
    assert!(!candidate_admissible(&domain, None, point));
    assert_eq!(domain.0[0].interiors().len(), 1);
}

#[test]
fn inward_chain_preserves_hole_topology_and_original_interface_roles() {
    let mut centers = concave();
    centers.0[0].interiors_push(LineString::from(vec![
        (-0.6, 0.4),
        (-0.6, 0.6),
        (-0.4, 0.6),
        (-0.4, 0.4),
        (-0.6, 0.4),
    ]));
    let interface = MultiPolygon(vec![Polygon::new(
        LineString::from(vec![
            (-2.0, 0.5),
            (2.0, 0.5),
            (2.0, 2.0),
            (-2.0, 2.0),
            (-2.0, 0.5),
        ]),
        vec![],
    )]);
    let original = (centers.clone(), interface.clone());
    let cancel = AtomicBool::new(false);
    let proposal = construct(&centers, Some(&interface), 0, 0, &mut 0, 0, &cancel)
        .unwrap()
        .unwrap();
    assert!(proposal.is_valid());
    assert_eq!(proposal.0.len(), centers.0.len());
    assert_eq!(proposal.0[0].interiors(), centers.0[0].interiors());
    assert!(centers.relate(&proposal).is_covers());
    let domains = Domains {
        ordinary: Some(centers.difference(&interface)),
        interface: Some(centers.intersection(&interface)),
        centers: centers.clone(),
        wall_centers: Some(proposal),
        all_interface: false,
        retained: 0,
        owned_bytes: 0,
        anchor_owned_bytes: 0,
    };
    let recipe = crate::planner::tests::recipe(1.0);
    for axis in [region::HatchAxis::X, region::HatchAxis::Y] {
        let paths = domains
            .paths(
                &interface,
                1.0,
                &region::HatchLayer {
                    roof: None,
                    width_mm: recipe.bead_width_mm,
                    z: recipe.layer_height_mm,
                    lower_z: 0.0,
                    height: recipe.layer_height_mm,
                    axis,
                    top_axis: axis,
                    recipe: &recipe,
                },
                0,
                &mut 0,
                0,
                &cancel,
            )
            .unwrap()
            .0;
        for kind in [DepositionKind::Support, DepositionKind::SupportInterface] {
            assert!(paths.iter().any(|path| path.kind == kind));
        }
        for path in paths {
            for edge in path.points_mm.windows(2) {
                let first = Coord {
                    x: edge[0][0],
                    y: edge[0][1],
                };
                let last = Coord {
                    x: edge[1][0],
                    y: edge[1][1],
                };
                assert!(commands::command_length(first, last).unwrap());
                assert!(centers.relate(&Line::new(first, last)).is_covers());
            }
        }
    }
    assert_eq!((centers, interface), original);
}

#[test]
fn inward_segment_with_short_original_interface_span_remains_inadmissible() {
    let domain = wide_concave(false);
    let interface = MultiPolygon(vec![Polygon::new(
        LineString::from(vec![
            (0.2, -1.0),
            (0.205, -1.0),
            (0.205, 3.0),
            (0.2, 3.0),
            (0.2, -1.0),
        ]),
        vec![],
    )]);
    let original = interface.clone();
    let point = Coord { x: 0.5, y: 1.0 };
    assert!(candidate_admissible(&domain, None, point));
    assert!(!candidate_admissible(&domain, Some(&interface), point));
    assert_eq!(interface, original);
}

#[test]
fn thin_corridor_refuses_every_bounded_inward_candidate_instead_of_crossing_it() {
    let mut lower = concave().0.remove(0).exterior().0.clone();
    lower.truncate(10);
    let mut ring = lower.clone();
    ring.extend(lower.iter().rev().map(|point| Coord {
        x: point.x,
        y: point.y + 0.0001,
    }));
    ring.push(ring[0]);
    let domain = MultiPolygon(vec![Polygon::new(LineString::new(ring), vec![])]);
    assert!(domain.is_valid());
    assert!(
        find(
            &domain,
            &domain,
            None,
            &mut [],
            0,
            0,
            &mut 0,
            0,
            &AtomicBool::new(false),
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn inward_workspace_and_queries_preserve_resource_and_cancellation_refusals() {
    let domain = concave();
    for (retained, mut work, cancel, expected) in [
        (
            crate::working_set::units(),
            0,
            false,
            ManufacturingErrorCode::ResourceLimit,
        ),
        (0, usize::MAX, false, ManufacturingErrorCode::ResourceLimit),
        (0, 0, true, ManufacturingErrorCode::Cancelled),
    ] {
        assert_eq!(
            find(
                &domain,
                &domain,
                None,
                &mut [],
                retained,
                0,
                &mut work,
                0,
                &AtomicBool::new(cancel),
            )
            .err()
            .expect("bounded inward search must refuse")
            .code,
            expected,
        );
    }
}
