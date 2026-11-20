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
        ring_book: RingBook::Declined,
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

#[expect(
    clippy::too_many_arguments,
    reason = "The differential witness controls both domain owners and the original work and cancellation state"
)]
fn run_segment(
    current: &MultiPolygon<f64>,
    original: &MultiPolygon<f64>,
    domain: &MultiPolygon<f64>,
    edge: [Coord<f64>; 2],
    book: Option<&RingBounds>,
    mut work: usize,
    foreign_work: usize,
    cancelled: bool,
) -> (Result<bool, ManufacturingError>, usize) {
    let mut search = Search {
        current,
        original,
        interface: None,
        retained: 0,
        owned_bytes: 0,
        work: &mut work,
        foreign_work,
        cancel: &AtomicBool::new(cancelled),
        cuts: Vec::new(),
        pocket: LineString::new(Vec::new()),
        ring_book: book.cloned().map_or(RingBook::Declined, RingBook::Ready),
    };
    let result = search.segment_inside(domain, edge);
    (result, work)
}

#[test]
fn inward_segment_screen_preserves_current_and_original_closed_ring_queries() {
    let remote = Rect::new((10.0, 10.0), (12.0, 12.0)).to_polygon();
    let mut current = wide_concave(false);
    current.0.insert(0, remote.clone());
    let mut original = wide_concave(true);
    original.0.insert(0, remote);
    let cancel = AtomicBool::new(false);
    let book = RingBounds::prepare(&current, &original, 0, 0, &cancel).unwrap();
    assert_eq!((book.current_len, book.entries.len()), (2, 5));
    for (height, expected_original) in [(0.3, false), (0.29, false), (0.28, true)] {
        let edge = [Coord { x: 0.4, y: height }, Coord { x: 0.8, y: height }];
        for (domain, expected) in [(&current, true), (&original, expected_original)] {
            let exhaustive = run_segment(&current, &original, domain, edge, None, 0, 0, false);
            assert_eq!(exhaustive.0, Ok(expected));
            assert_eq!(
                run_segment(&current, &original, domain, edge, Some(&book), 0, 0, false),
                exhaustive
            );
        }
    }
}

#[test]
fn inward_segment_screen_falls_through_for_an_unknown_original_hole() {
    let mut current = wide_concave(false);
    current.0[0].interiors_push(LineString::from(vec![
        (1.29, 0.29),
        (1.29, 0.31),
        (1.31, 0.31),
        (1.31, 0.29),
        (1.29, 0.29),
    ]));
    let original = wide_concave(true);
    let edge = [Coord { x: 0.4, y: 0.3 }, Coord { x: 0.8, y: 0.3 }];
    let cancel = AtomicBool::new(false);
    let mut book = RingBounds::prepare(&current, &original, 0, 0, &cancel).unwrap();
    let exhaustive = run_segment(&current, &original, &original, edge, None, 0, 0, false);
    assert_eq!(exhaustive.0, Ok(false));
    assert_eq!(
        run_segment(
            &current,
            &original,
            &original,
            edge,
            Some(&book),
            0,
            0,
            false
        ),
        exhaustive
    );
    book.entries[book.current_len + 1] = None;
    assert_eq!(
        run_segment(
            &current,
            &original,
            &original,
            edge,
            Some(&book),
            0,
            0,
            false
        ),
        exhaustive
    );
}

#[test]
fn inward_segment_book_releases_bytes_before_role_cut_growth() {
    let domain = wide_concave(false);
    let interface = MultiPolygon(vec![Rect::new((0.6, 0.2), (0.65, 0.4)).to_polygon()]);
    let cancel = AtomicBool::new(false);
    let book = RingBounds::prepare(&domain, &domain, 0, 0, &cancel).unwrap();
    let book_bytes =
        std::mem::size_of::<RingBook>() + super::super::super::super::vec_bytes(&book.entries);
    let owned_bytes = crate::working_set::bytes() - book_bytes;
    let edge = [Coord { x: 0.4, y: 0.3 }, Coord { x: 0.8, y: 0.3 }];
    let run = |ring_book| {
        let mut work = 0;
        let can_rebuild = matches!(ring_book, RingBook::Unprepared);
        let mut search = Search {
            current: &domain,
            original: &domain,
            interface: Some(&interface),
            retained: 0,
            owned_bytes,
            work: &mut work,
            foreign_work: 0,
            cancel: &cancel,
            cuts: Vec::new(),
            pocket: LineString::new(Vec::new()),
            ring_book,
        };
        let inside = search.segment_inside(&domain, edge).unwrap();
        assert_eq!(matches!(search.ring_book, RingBook::Ready(_)), can_rebuild);
        let short = search.short(edge, &mut None);
        assert_eq!(search.cuts.len(), 2);
        assert!(!matches!(search.ring_book, RingBook::Ready(_)));
        let after = search.segment_inside(&domain, edge).unwrap();
        assert!(matches!(search.ring_book, RingBook::Declined));
        (inside, short, after, work)
    };
    let exhaustive = run(RingBook::Declined);
    assert!(exhaustive.1.is_ok());
    assert_eq!(run(RingBook::Unprepared), exhaustive);
}

#[test]
fn inward_segment_screen_descends_on_overflow_and_declines_under_pressure() {
    let mut domain = wide_concave(false);
    domain
        .0
        .insert(0, Rect::new((10.0, 10.0), (12.0, 12.0)).to_polygon());
    let cancel = AtomicBool::new(false);
    let book = RingBounds::prepare(&domain, &domain, 0, 0, &cancel).unwrap();
    assert!(
        RingBounds::prepare(&domain, &domain, 0, crate::working_set::bytes(), &cancel).is_none()
    );
    let invalid = LineString::from(vec![(0.0, 0.0), (f64::NAN, 1.0)]);
    assert!(finite_ring_bounds(&invalid, &cancel).is_none());
    let edge = [Coord { x: 0.4, y: 0.3 }, Coord { x: 0.8, y: 0.3 }];
    let near_overflow = usize::MAX - region::point_count(&domain) - 2;
    for (work, foreign, cancelled, expected) in [
        (
            near_overflow,
            0,
            false,
            ManufacturingErrorCode::ResourceLimit,
        ),
        (0, 1, false, ManufacturingErrorCode::ResourceLimit),
        (near_overflow, 0, true, ManufacturingErrorCode::Cancelled),
    ] {
        let exhaustive = run_segment(
            &domain, &domain, &domain, edge, None, work, foreign, cancelled,
        );
        assert_eq!(exhaustive.0.as_ref().unwrap_err().code, expected);
        assert_eq!(
            run_segment(
                &domain,
                &domain,
                &domain,
                edge,
                Some(&book),
                work,
                foreign,
                cancelled
            ),
            exhaustive
        );
    }
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
        base_bytes: 0,
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
        region::hatch_support(
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
        base_bytes: 0,
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
