// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use geo::Contains;
use geo::kernels::{Kernel, Orientation, RobustKernel};

mod inward;

struct Ear {
    polygon: usize,
    ring: usize,
    vertex: usize,
    constraint: usize,
    previous_constraint: usize,
}

fn replace_incident_constraints(
    constraints: &mut Vec<[Option<bool>; 2]>,
    ear: &Ear,
    work: &mut usize,
    foreign_work: usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    if !constraints.is_empty() {
        charge(work, constraints.len(), foreign_work, cancel)?;
        constraints[ear.previous_constraint] = [None; 2];
        constraints.remove(ear.constraint);
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "Pre-stroke feasibility borrows original interface ownership and the same bounded role-cut workspace policy as wall construction"
)]
fn short_role_piece(
    [origin, target]: [Coord<f64>; 2],
    interface: &MultiPolygon<f64>,
    cached: &mut Option<bool>,
    cuts: &mut Vec<(f64, Coord<f64>)>,
    retained: usize,
    owned_bytes: usize,
    work: &mut usize,
    foreign_work: usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    charge(work, 1, foreign_work, cancel)?;
    if let Some(short) = *cached {
        return Ok(short);
    }
    super::role_cuts(
        [origin, target],
        interface,
        cuts,
        crate::working_set::units().saturating_sub(retained),
        owned_bytes,
        work,
        foreign_work,
        cancel,
    )?;
    let mut start = origin;
    let mut owned_start = origin;
    let mut owner = None;
    let mut split = false;
    for end in cuts.iter().map(|cut| cut.1).chain(std::iter::once(target)) {
        charge(work, region::point_count(interface), foreign_work, cancel)?;
        let kind = interface.intersects(&Point::new(
            start.x + (end.x - start.x) * 0.5,
            start.y + (end.y - start.y) * 0.5,
        ));
        if owner.is_some_and(|previous| previous != kind) {
            split = true;
            if !commands::command_length(owned_start, start)? {
                *cached = Some(true);
                return Ok(true);
            }
            owned_start = start;
        }
        owner = Some(kind);
        start = end;
    }
    let short = split && !commands::command_length(owned_start, target)?;
    *cached = Some(short);
    Ok(short)
}

#[expect(
    clippy::too_many_arguments,
    reason = "Exact directed-edge role constraints are reused across ear removals under the same live resource and work limits"
)]
fn find_ear(
    centers: &MultiPolygon<f64>,
    interface: Option<&MultiPolygon<f64>>,
    constraints: &mut [[Option<bool>; 2]],
    retained: usize,
    owned_bytes: usize,
    work: &mut usize,
    foreign_work: usize,
    cancel: &AtomicBool,
) -> Result<Option<Ear>, ManufacturingError> {
    let mut cuts = Vec::new();
    let mut ring_start = 0;
    for (polygon_index, polygon) in centers.0.iter().enumerate() {
        for (ring_index, ring) in std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .enumerate()
        {
            charge(work, 1, foreign_work, cancel)?;
            let count = ring.0.len() - 1;
            if count <= 3 {
                ring_start += count;
                continue;
            }
            for vertex in 0..count {
                charge(work, 1, foreign_work, cancel)?;
                let previous = ring.0[(vertex + count - 1) % count];
                let at = ring.0[vertex];
                let next = ring.0[(vertex + 1) % count];
                if RobustKernel::orient2d(previous, at, next) != Orientation::CounterClockwise {
                    continue;
                }
                let mut constrained = !commands::command_length(previous, at)?
                    || !commands::command_length(at, next)?;
                if !constrained && let Some(interface) = interface {
                    for (neighbor, constraint, direction) in [
                        (previous, ring_start + (vertex + count - 1) % count, 1),
                        (next, ring_start + vertex, 0),
                    ] {
                        if short_role_piece(
                            [at, neighbor],
                            interface,
                            &mut constraints[constraint][direction],
                            &mut cuts,
                            retained,
                            owned_bytes,
                            work,
                            foreign_work,
                            cancel,
                        )? {
                            constrained = true;
                            break;
                        }
                    }
                }
                if !constrained {
                    continue;
                }
                let chord = Line::new(previous, next);
                charge(work, region::point_count(centers), foreign_work, cancel)?;
                if !centers.contains(&Point::new(
                    previous.x * 0.5 + next.x * 0.5,
                    previous.y * 0.5 + next.y * 0.5,
                )) {
                    continue;
                }
                let mut admissible = true;
                'boundaries: for (owner_index, owner) in centers.0.iter().enumerate() {
                    for (boundary_index, boundary) in std::iter::once(owner.exterior())
                        .chain(owner.interiors())
                        .enumerate()
                    {
                        for (index, edge) in boundary.0.windows(2).enumerate() {
                            charge(work, 1, foreign_work, cancel)?;
                            let own_triangle = owner_index == polygon_index
                                && boundary_index == ring_index
                                && [vertex, (vertex + count - 1) % count, (vertex + 1) % count]
                                    .contains(&index);
                            if !own_triangle
                                && [(previous, at), (at, next), (next, previous)]
                                    .into_iter()
                                    .all(|(start, end)| {
                                        RobustKernel::orient2d(start, end, edge[0])
                                            != Orientation::Clockwise
                                    })
                            {
                                admissible = false;
                                break 'boundaries;
                            }
                            match line_intersection(chord, Line::new(edge[0], edge[1])) {
                                None => {}
                                Some(LineIntersection::SinglePoint { intersection, .. })
                                    if intersection == previous || intersection == next => {}
                                _ => {
                                    admissible = false;
                                    break 'boundaries;
                                }
                            }
                        }
                    }
                }
                if admissible {
                    return Ok(Some(Ear {
                        polygon: polygon_index,
                        ring: ring_index,
                        vertex,
                        constraint: ring_start + vertex,
                        previous_constraint: ring_start + (vertex + count - 1) % count,
                    }));
                }
            }
            ring_start += count;
        }
    }
    Ok(None)
}

enum Change {
    Ear(Ear),
    Inward(inward::Inward),
}

#[expect(
    clippy::too_many_arguments,
    reason = "Optional construction keeps current proposal, unchanged physical and role domains, and cumulative resource limits distinct"
)]
fn find_change(
    current: &MultiPolygon<f64>,
    original: &MultiPolygon<f64>,
    interface: Option<&MultiPolygon<f64>>,
    constraints: &mut [[Option<bool>; 2]],
    retained: usize,
    owned_bytes: usize,
    work: &mut usize,
    foreign_work: usize,
    cancel: &AtomicBool,
) -> Result<Option<Change>, ManufacturingError> {
    if let Some(ear) = find_ear(
        current,
        interface,
        constraints,
        retained,
        owned_bytes,
        work,
        foreign_work,
        cancel,
    )? {
        return Ok(Some(Change::Ear(ear)));
    }
    Ok(inward::find(
        current,
        original,
        interface,
        constraints,
        retained,
        owned_bytes,
        work,
        foreign_work,
        cancel,
    )?
    .map(Change::Inward))
}

pub(super) fn construct(
    centers: &MultiPolygon<f64>,
    interface: Option<&MultiPolygon<f64>>,
    retained: usize,
    owned_bytes: usize,
    work: &mut usize,
    foreign_work: usize,
    cancel: &AtomicBool,
) -> Result<Option<MultiPolygon<f64>>, ManufacturingError> {
    check_cancel(cancel)?;
    let mut constraints = Vec::new();
    if interface.is_some() {
        let count = region::point_count(centers);
        charge(work, count, foreign_work, cancel)?;
        super::super::super::remaining_geometry_units(retained.saturating_add(count), owned_bytes)?;
        super::super::reserve_owned(&mut constraints, count, owned_bytes)?;
        constraints.resize(count, [None; 2]);
    }
    let retained = retained.saturating_add(constraints.capacity());
    let owned_bytes = owned_bytes.saturating_add(super::super::vec_bytes(&constraints));
    super::super::super::remaining_geometry_units(retained, owned_bytes)?;
    let Some(mut change) = find_change(
        centers,
        centers,
        interface,
        &mut constraints,
        retained,
        owned_bytes,
        work,
        foreign_work,
        cancel,
    )?
    else {
        return Ok(None);
    };
    let mut proposal = region::offset_admitted(
        centers,
        0.0,
        |scratch, scratch_bytes, _| {
            super::super::super::remaining_geometry_units(
                retained.saturating_add(scratch),
                owned_bytes.saturating_add(scratch_bytes),
            )?;
            Ok(())
        },
        cancel,
    )?;
    let proposal_bytes = super::super::super::native::material_bytes(&mut proposal);
    let mut remaining_edits = region::point_count(centers).saturating_mul(3);
    loop {
        charge(work, 1, foreign_work, cancel)?;
        remaining_edits = remaining_edits
            .checked_sub(1)
            .ok_or_else(|| resource("optional support proposal edit budget exceeded"))?;
        let (address, point) = match &change {
            Change::Ear(ear) => (ear, None),
            Change::Inward(inward) => (&inward.address, Some(inward.point)),
        };
        let mut edit = |ring: &mut geo::LineString<f64>| {
            if let Some(point) = point {
                charge(work, 1, foreign_work, cancel)?;
                ring.0[address.vertex] = point;
            } else {
                charge(work, ring.0.len(), foreign_work, cancel)?;
                ring.0.remove(address.vertex);
            }
            if address.vertex == 0 {
                let first = ring.0[0];
                *ring.0.last_mut().expect("retained proposal ring") = first;
            }
            Ok::<_, ManufacturingError>(())
        };
        if address.ring == 0 {
            proposal.0[address.polygon].try_exterior_mut(edit)?;
        } else {
            proposal.0[address.polygon]
                .try_interiors_mut(|rings| edit(&mut rings[address.ring - 1]))?;
        }
        if point.is_some() && !constraints.is_empty() {
            charge(work, 2, foreign_work, cancel)?;
            constraints[address.previous_constraint] = [None; 2];
            constraints[address.constraint] = [None; 2];
        } else {
            replace_incident_constraints(&mut constraints, address, work, foreign_work, cancel)?;
        }
        let Some(next) = find_change(
            &proposal,
            centers,
            interface,
            &mut constraints,
            retained.saturating_add(region::point_count(&proposal)),
            owned_bytes.saturating_add(proposal_bytes),
            work,
            foreign_work,
            cancel,
        )?
        else {
            break;
        };
        change = next;
    }
    region::check_size(&proposal)?;
    Ok(Some(proposal))
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{LineString, Polygon, Relate, Translate};

    fn notch() -> MultiPolygon<f64> {
        MultiPolygon(vec![Polygon::new(
            LineString::from(vec![
                (0.98, -0.05),
                (1.0, 0.0),
                (0.995, 0.0024),
                (0.945, 0.045),
                (-1.0, 1.0),
                (-1.0, -1.0),
                (0.5, -1.0),
                (0.98, -0.05),
            ]),
            vec![],
        )])
    }

    #[test]
    fn unchanged_directed_role_queries_reuse_exact_answers_and_still_charge_cancel() {
        let interface = MultiPolygon(vec![Polygon::new(
            LineString::from(vec![
                (0.01, -1.0),
                (2.0, -1.0),
                (2.0, 1.0),
                (0.01, 1.0),
                (0.01, -1.0),
            ]),
            vec![],
        )]);
        let cancel = AtomicBool::new(false);
        let origin = Coord { x: 0.0, y: 0.0 };
        let target = Coord { x: 1.0, y: 0.0 };
        for (edge, expected) in [
            ([origin, target], true),
            ([target, origin], true),
            ([target, Coord { x: 1.5, y: 0.0 }], false),
        ] {
            let mut cuts = Vec::new();
            let mut cached = None;
            let mut work = 0;
            assert_eq!(
                short_role_piece(
                    edge,
                    &interface,
                    &mut cached,
                    &mut cuts,
                    0,
                    0,
                    &mut work,
                    0,
                    &cancel,
                )
                .unwrap(),
                expected,
            );
            assert!(work > 1);
            for _ in 0..128 {
                let before = work;
                assert_eq!(
                    short_role_piece(
                        edge,
                        &interface,
                        &mut cached,
                        &mut cuts,
                        0,
                        0,
                        &mut work,
                        0,
                        &cancel,
                    )
                    .unwrap(),
                    expected,
                );
                assert_eq!(work - before, 1);
            }
            for (mut work, cancel, expected) in [
                (usize::MAX, false, ManufacturingErrorCode::ResourceLimit),
                (0, true, ManufacturingErrorCode::Cancelled),
            ] {
                assert_eq!(
                    short_role_piece(
                        edge,
                        &interface,
                        &mut cached,
                        &mut cuts,
                        0,
                        0,
                        &mut work,
                        0,
                        &AtomicBool::new(cancel),
                    )
                    .unwrap_err()
                    .code,
                    expected,
                );
            }
        }
    }

    #[test]
    fn ear_replacement_invalidates_both_chord_directions_without_other_ring_changes() {
        let ring = notch().0.remove(0).exterior().clone();
        let count = ring.0.len() - 1;
        for vertex in 0..count {
            let mut constraints = vec![[Some(false), Some(true)]; count * 2];
            let ear = Ear {
                polygon: 1,
                ring: 0,
                vertex,
                constraint: count + vertex,
                previous_constraint: count + (vertex + count - 1) % count,
            };
            let mut work = 0;
            replace_incident_constraints(
                &mut constraints,
                &ear,
                &mut work,
                0,
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(work, count * 2);
            assert_eq!(constraints.len(), count * 2 - 1);
            assert!(
                constraints[..count]
                    .iter()
                    .all(|value| *value == [Some(false), Some(true)])
            );
            let mut retained = ring.0[..count].to_vec();
            retained.remove(vertex);
            for (index, origin) in retained.iter().enumerate() {
                let target = retained[(index + 1) % retained.len()];
                let original_edge = ring.0.windows(2).any(|edge| edge == [*origin, target]);
                assert_eq!(
                    constraints[count + index],
                    if original_edge {
                        [Some(false), Some(true)]
                    } else {
                        [None; 2]
                    },
                );
            }
        }
    }

    #[test]
    fn repeated_optional_ears_keep_original_interface_scans_within_remaining_work() {
        let centers = MultiPolygon(
            (0..24)
                .map(|index| notch().0.remove(0).translate(index as f64 * 4.0, 0.0))
                .collect(),
        );
        let mut boundary: Vec<_> = (0..128)
            .map(|index| {
                let angle = index as f64 * std::f64::consts::TAU / 128.0;
                Coord {
                    x: -10.0 + angle.cos(),
                    y: angle.sin(),
                }
            })
            .collect();
        boundary.push(boundary[0]);
        let interface = MultiPolygon(vec![Polygon::new(LineString::new(boundary), vec![])]);
        let cancel = AtomicBool::new(false);
        let expected = construct(&centers, None, 0, 0, &mut 0, 0, &cancel)
            .unwrap()
            .unwrap();
        let mut work = usize::MAX - 300_000;
        let proposal = construct(&centers, Some(&interface), 0, 0, &mut work, 0, &cancel)
            .unwrap()
            .unwrap();
        assert_eq!(proposal, expected);
        assert_eq!(proposal.0.len(), centers.0.len());
        for (source, result) in centers.0.iter().zip(&proposal.0) {
            for edge in result.exterior().0.windows(2) {
                assert!(source.relate(&Line::new(edge[0], edge[1])).is_covers());
            }
        }
    }

    #[test]
    fn short_middle_interface_span_constrains_optional_ear_before_stroke_authority() {
        let centers = MultiPolygon(vec![Polygon::new(
            LineString::from(vec![
                (0.0, 0.0),
                (1.0, 0.0),
                (1.0, 1.0),
                (0.0, 1.0),
                (0.0, 0.0),
            ]),
            vec![],
        )]);
        let interface = MultiPolygon(vec![Polygon::new(
            LineString::from(vec![
                (0.2, -0.1),
                (0.21, -0.1),
                (0.21, 0.01),
                (0.2, 0.01),
                (0.2, -0.1),
            ]),
            vec![],
        )]);
        let original = (centers.clone(), interface.clone());
        let cancel = AtomicBool::new(false);
        let origin = Coord { x: 0.0, y: 0.0 };
        let target = Coord { x: 1.0, y: 0.0 };
        for edge in [[origin, target], [target, origin]] {
            assert!(
                short_role_piece(
                    edge,
                    &interface,
                    &mut None,
                    &mut Vec::new(),
                    0,
                    0,
                    &mut 0,
                    0,
                    &cancel,
                )
                .unwrap()
            );
        }
        let proposal = construct(&centers, Some(&interface), 0, 0, &mut 0, 0, &cancel)
            .unwrap()
            .unwrap();
        assert!(!proposal.0[0].exterior().0.contains(&origin));
        for edge in proposal.0[0].exterior().0.windows(2) {
            assert!(centers.relate(&Line::new(edge[0], edge[1])).is_covers());
            assert!(commands::command_length(edge[0], edge[1]).unwrap());
        }
        assert_eq!((centers, interface), original);
    }

    #[test]
    fn optional_convex_tooth_is_constructed_inside_unchanged_clearance_domain() {
        let centers = notch();
        let original = centers.clone();
        let proposal = construct(&centers, None, 0, 0, &mut 0, 0, &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert_eq!(centers, original);
        assert_eq!(proposal.0.len(), centers.0.len());
        assert_eq!(proposal.0[0].interiors().len(), 0);
        assert_eq!(proposal.0[0].exterior().0.len(), 7);
        assert!(
            !proposal.0[0]
                .exterior()
                .0
                .contains(&Coord { x: 1.0, y: 0.0 })
        );
        for edge in proposal.0[0].exterior().0.windows(2) {
            assert!(centers.relate(&Line::new(edge[0], edge[1])).is_covers());
            assert!(commands::command_length(edge[0], edge[1]).unwrap());
        }
    }

    #[test]
    fn optional_ear_cannot_remove_a_hole_or_cross_concave_material() {
        let mut centers = notch();
        centers.0[0].interiors_push(LineString::from(vec![
            (0.9915, -0.016),
            (0.9915, -0.0158),
            (0.9917, -0.0158),
            (0.9917, -0.016),
            (0.9915, -0.016),
        ]));
        assert!(
            find_ear(
                &centers,
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
        let outer = LineString::from(vec![
            (-2.0, -2.0),
            (2.0, -2.0),
            (2.0, 2.0),
            (-2.0, 2.0),
            (-2.0, -2.0),
        ]);
        let mut hole: Vec<_> = (0..=36)
            .map(|index| {
                let theta = -(index as f64) * std::f64::consts::TAU / 36.0;
                Coord {
                    x: theta.cos() * 0.1,
                    y: theta.sin() * 0.1,
                }
            })
            .collect();
        hole[36] = hole[0];
        let centers = MultiPolygon(vec![Polygon::new(outer, vec![LineString::new(hole)])]);
        assert!(
            find_ear(
                &centers,
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
    fn conservative_wall_proposal_does_not_change_original_hatches() {
        let centers = notch();
        let cancel = AtomicBool::new(false);
        let wall_centers = construct(&centers, None, 0, 0, &mut 0, 0, &cancel)
            .unwrap()
            .unwrap();
        let recipe = crate::planner::tests::recipe(1.0);
        for axis in [region::HatchAxis::X, region::HatchAxis::Y] {
            for all_interface in [false, true] {
                let layer = region::HatchLayer {
                    roof: None,
                    width_mm: recipe.bead_width_mm,
                    z: recipe.layer_height_mm,
                    lower_z: 0.0,
                    height: recipe.layer_height_mm,
                    axis,
                    top_axis: axis,
                    recipe: &recipe,
                };
                let domains = Domains {
                    centers: centers.clone(),
                    wall_centers: Some(wall_centers.clone()),
                    ordinary: None,
                    interface: None,
                    all_interface,
                    retained: 0,
                    owned_bytes: 0,
                    anchor_owned_bytes: 0,
                };
                let kind = if all_interface {
                    DepositionKind::SupportInterface
                } else {
                    DepositionKind::Support
                };
                let mut expected = Vec::new();
                region::hatch(&centers, kind, 1.0, &layer, &mut expected, &mut 0, &cancel).unwrap();
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
                assert_eq!(actual[0].points_mm.len(), 7);
                assert_eq!(actual[0].kind, kind);
                assert_eq!(domains.centers, centers);
            }
        }
    }

    #[test]
    fn original_interface_crossing_constrains_optional_wall_before_stroke_authority() {
        let centers = notch();
        let interface = MultiPolygon(vec![Polygon::new(
            LineString::from(vec![
                (-2.0, 0.013),
                (2.0, 0.013),
                (2.0, 2.0),
                (-2.0, 2.0),
                (-2.0, 0.013),
            ]),
            vec![],
        )]);
        let original = (centers.clone(), interface.clone());
        let cancel = AtomicBool::new(false);
        let corner = Coord {
            x: 0.995,
            y: 0.0024,
        };
        let geometric = construct(&centers, None, 0, 0, &mut 0, 0, &cancel)
            .unwrap()
            .unwrap();
        assert!(geometric.0[0].exterior().0.contains(&corner));
        let proposal = construct(&centers, Some(&interface), 0, 0, &mut 0, 0, &cancel)
            .unwrap()
            .unwrap();
        assert!(!proposal.0[0].exterior().0.contains(&corner));
        for edge in proposal.0[0].exterior().0.windows(2) {
            assert!(centers.relate(&Line::new(edge[0], edge[1])).is_covers());
        }
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
                    assert!(
                        commands::command_length(
                            Coord {
                                x: edge[0][0],
                                y: edge[0][1]
                            },
                            Coord {
                                x: edge[1][0],
                                y: edge[1][1]
                            },
                        )
                        .unwrap()
                    );
                }
            }
        }
        assert_eq!((centers, interface), original);
    }

    #[test]
    fn interface_tangency_without_role_change_does_not_clip_optional_corner() {
        let centers = MultiPolygon(vec![Polygon::new(
            LineString::from(vec![
                (0.0, 0.0),
                (1.0, 0.0),
                (1.0, 1.0),
                (0.0, 1.0),
                (0.0, 0.0),
            ]),
            vec![],
        )]);
        let interface = MultiPolygon(vec![Polygon::new(
            LineString::from(vec![(1.0, 0.99), (1.1, 0.98), (1.1, 1.0), (1.0, 0.99)]),
            vec![],
        )]);
        assert!(
            construct(
                &centers,
                Some(&interface),
                0,
                0,
                &mut 0,
                0,
                &AtomicBool::new(false)
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn optional_proposal_preserves_resource_and_cancellation_refusals() {
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
            for interface in [None, Some(notch())] {
                assert_eq!(
                    construct(
                        &notch(),
                        interface.as_ref(),
                        retained,
                        0,
                        &mut work,
                        0,
                        &AtomicBool::new(cancel)
                    )
                    .unwrap_err()
                    .code,
                    expected,
                );
            }
        }
    }

    #[test]
    fn irreducible_optional_ring_is_omitted_and_reported_not_silently_dropped() {
        let centers = MultiPolygon(vec![Polygon::new(
            LineString::from(vec![(0.0, 0.0), (0.01, 0.0), (0.0, 0.01), (0.0, 0.0)]),
            vec![],
        )]);
        let cancel = AtomicBool::new(false);
        assert!(
            construct(&centers, None, 0, 0, &mut 0, 0, &cancel)
                .unwrap()
                .is_none()
        );
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
        // No command fits anywhere on this ring: it deposits nothing, and its
        // whole length is returned for the layer's report.
        let omitted = commands::fit(
            &centers,
            &mut paths,
            &mut segments,
            0,
            0,
            &mut 0,
            0,
            &cancel,
        )
        .unwrap();
        assert!(paths.is_empty());
        assert_eq!(segments, 0);
        assert!((omitted - (0.02 + 0.01 * std::f64::consts::SQRT_2)).abs() < 1e-12);
    }
}
