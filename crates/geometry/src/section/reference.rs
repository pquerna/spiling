// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{canonicalize_section_in_place, kernel_error, resource_error};
use crate::parametric::Interval;
use crate::{GeometryError, GeometryWorkBudget, NativeSection, PlaneFrameMm, check_cancel};
use spiling_contracts::geometry::{SECTION_PLANE_TOLERANCE_MM, SECTION_SAMPLING_TOLERANCE_MM};
use std::sync::atomic::AtomicBool;

/// An untrusted reference proposal, never a material or Jordan certificate.
/// Indices refer to nonadjacent edges of the exactly closed input loop.
pub(super) struct CycleCut {
    pub first_edge: usize,
    pub second_edge: usize,
    pub point_mm: [f64; 3],
    pub keep_middle: bool,
}

/// Consume the sampled geometric intersection and erase only cycles whose full
/// 3D bidirectional correspondence is independently bounded. Native sampling
/// never promised that the old PL loop was Jordan; this is not a topology-
/// preserving repair of that loop. Every actual-G material/degree/metric gate
/// remains mandatory on the returned geometry, with all native loop owners.
///
/// No point buffer is copied or grown. Search has at most the original loop's
/// quadratic pair inventory, including restarts after accepted compactions;
/// unresolved crossings confer no authority. Each row and candidate scan is
/// charged before execution, so neither dense crossings nor rejection can
/// introduce an unbounded cubic search under the shared work allowance.
pub(crate) fn simplify_section_reference(
    mut section: NativeSection,
    maximum_chord_error_mm: f64,
    query_byte_limit: u32,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<NativeSection, GeometryError> {
    work.charge(1, cancel)?;
    if !maximum_chord_error_mm.is_finite()
        || maximum_chord_error_mm <= 0.0
        || maximum_chord_error_mm > SECTION_SAMPLING_TOLERANCE_MM
    {
        return Err(kernel_error(
            "invalid narrowed native reference chord allowance",
        ));
    }
    if !section.is_native_intersection()
        || !section.achieved_chord_error_mm().is_finite()
        || section.achieved_chord_error_mm() < 0.0
        || section.achieved_chord_error_mm() > maximum_chord_error_mm
        || !section.achieved_plane_error_mm().is_finite()
        || section.achieved_plane_error_mm() < 0.0
        || section.achieved_plane_error_mm() > SECTION_PLANE_TOLERANCE_MM
        || section.loop_source_faces.len() != section.loops.len()
        || section
            .loop_source_faces
            .iter()
            .any(|owners| owners.is_empty() || owners.windows(2).any(|pair| pair[0] >= pair[1]))
    {
        return Err(kernel_error(
            "invalid native reference normalization authority",
        ));
    }
    // Fixed live stack scratch is admitted just like heap-backed proof scratch.
    // No proposal allocates or clones any section/point buffer.
    let scratch = super::reference_distance::CYCLE_DISTANCE_STACK_BYTES
        .checked_add(2048)
        .ok_or_else(|| resource_error("native reference scratch size overflow"))?;
    if section
        .retained_bytes()
        .checked_add(scratch)
        .is_none_or(|bytes| bytes > query_byte_limit as usize)
    {
        return Err(resource_error("native reference exceeds query capacity"));
    }
    for boundary in &section.loops {
        work.charge(boundary.points_mm.len() as u64 + 1, cancel)?;
        if boundary.points_mm.len() < 4
            || boundary.points_mm.first() != boundary.points_mm.last()
            || boundary
                .points_mm
                .iter()
                .flatten()
                .any(|value| !value.is_finite())
        {
            return Err(kernel_error(
                "native reference loop is not finite and exactly closed",
            ));
        }
    }
    // Charge every edit against this query's remaining source chord allowance.
    // World placement independently admits its affine and arithmetic error.
    let allowance = (maximum_chord_error_mm - section.achieved_chord_error_mm()).next_down();
    if allowance <= 0.0 {
        return Ok(section);
    }
    let mut displacement = 0.0;
    let mut changed = false;
    for loop_index in 0..section.loops.len() {
        let initial_count = section.loops[loop_index].points_mm.len() - 1;
        let mut pairs_left = initial_count
            .checked_mul(initial_count - 1)
            .and_then(|pairs| pairs.checked_div(2))
            .ok_or_else(|| resource_error("native reference pair inventory overflow"))?;
        let mut first = 0;
        while pairs_left != 0 {
            let count = section.loops[loop_index].points_mm.len() - 1;
            if first + 2 >= count {
                break;
            }
            let row_pairs = (count - first - 2).min(pairs_left);
            work.charge(row_pairs as u64, cancel)?;
            pairs_left -= row_pairs;
            let mut accepted = None;
            for second in first + 2..first + 2 + row_pairs {
                check_cancel(cancel)?;
                if first == 0 && second + 1 == count {
                    continue;
                }
                let points = &section.loops[loop_index].points_mm;
                let Some(point_mm) = crossing_point(
                    points[first],
                    points[first + 1],
                    points[second],
                    points[second + 1],
                    &section.frame,
                ) else {
                    continue;
                };
                // The rounded point is constructed on the first old segment.
                // Direct outward residual bounds independently account for its
                // interpolation rounding, without increasing the query plane max.
                if plane_residual(point_mm, &section.frame) > section.achieved_plane_error_mm() {
                    continue;
                }
                for keep_middle in [false, true] {
                    let cut = CycleCut {
                        first_edge: first,
                        second_edge: second,
                        point_mm,
                        keep_middle,
                    };
                    let candidate_count = candidate_len(points.len(), &cut);
                    if candidate_count < 4 || candidate_count >= points.len() {
                        continue;
                    }
                    let remaining = (allowance - displacement).next_down();
                    if remaining <= 0.0 {
                        break;
                    }
                    // Expensive scans only follow proper, bbox-pruned crossings.
                    let oriented = candidate_orientation(
                        points,
                        &cut,
                        &section.frame,
                        !section.loops[loop_index].is_hole,
                        work,
                        cancel,
                    )?;
                    let foreign_clear = oriented
                        && foreign_boundaries_clear(&section, loop_index, &cut, work, cancel)?;
                    if !foreign_clear {
                        continue;
                    }
                    let distance = super::reference_distance::cycle_distance(
                        points, &cut, remaining, work, cancel,
                    )?;
                    let Some(distance) = distance else { continue };
                    let next = (displacement + distance).next_up();
                    if next <= allowance {
                        accepted = Some((cut, next));
                        break;
                    }
                }
                if accepted.is_some() {
                    break;
                }
            }
            if let Some((cut, next)) = accepted {
                let points = &mut section.loops[loop_index].points_mm;
                work.charge(points.len() as u64, cancel)?;
                compact(points, &cut);
                displacement = next;
                changed = true;
                // Revisit altered edges without allowing repeated compactions
                // to enlarge the original quadratic proposal-search inventory.
                first = 0;
            } else {
                first += 1;
            }
        }
    }
    if changed {
        section.charge_reference_displacement(displacement)?;
        if section.achieved_chord_error_mm() > maximum_chord_error_mm {
            return Err(kernel_error(
                "native reference exceeds its narrowed chord allowance",
            ));
        }
        canonicalize_section_in_place(&mut section, query_byte_limit, work, cancel)?;
    }
    check_cancel(cancel)?;
    Ok(section)
}

fn project(point: [f64; 3], origin: [f64; 3], frame: &PlaneFrameMm) -> [Interval; 2] {
    [frame.x_axis, frame.y_axis].map(|axis| {
        (0..3).fold(Interval::point(0.0), |sum, k| {
            sum.add(
                Interval::point(point[k])
                    .sub(Interval::point(origin[k]))
                    .mul(Interval::point(axis[k])),
            )
        })
    })
}

fn cross(a: [Interval; 2], b: [Interval; 2]) -> Interval {
    a[0].mul(b[1]).sub(a[1].mul(b[0]))
}

fn subtract(a: [Interval; 2], b: [Interval; 2]) -> [Interval; 2] {
    [a[0].sub(b[0]), a[1].sub(b[1])]
}

fn opposite(a: Interval, b: Interval) -> bool {
    a.lo > 0.0 && b.hi < 0.0 || a.hi < 0.0 && b.lo > 0.0
}

fn crossing_point(
    a: [f64; 3],
    b: [f64; 3],
    c: [f64; 3],
    d: [f64; 3],
    frame: &PlaneFrameMm,
) -> Option<[f64; 3]> {
    let [b2, c2, d2] = [b, c, d].map(|point| project(point, a, frame));
    // Conservative projected bboxes eliminate most pairs before root work.
    if (0..2).any(|k| {
        b2[k].hi.max(0.0) < c2[k].lo.min(d2[k].lo) || c2[k].hi.max(d2[k].hi) < b2[k].lo.min(0.0)
    }) {
        return None;
    }
    if !opposite(cross(b2, c2), cross(b2, d2))
        || !opposite(
            cross(subtract(d2, c2), subtract([Interval::point(0.0); 2], c2)),
            cross(subtract(d2, c2), subtract(b2, c2)),
        )
    {
        return None;
    }
    // Ordinary finite arithmetic only proposes a root. The distance checker
    // proves correspondence independently of this calculation and its rounding.
    let mid = |value: Interval| value.lo * 0.5 + value.hi * 0.5;
    let [b2, c2, d2] = [b2, c2, d2].map(|point| point.map(mid));
    let direction = [d2[0] - c2[0], d2[1] - c2[1]];
    let determinant = |u: [f64; 2], v: [f64; 2]| u[0] * v[1] - u[1] * v[0];
    let t = determinant(c2, direction) / determinant(b2, direction);
    if !t.is_finite() || t <= 0.0 || t >= 1.0 {
        return None;
    }
    let point = std::array::from_fn(|k| a[k] + t * (b[k] - a[k]));
    point.iter().all(|value| value.is_finite()).then_some(point)
}

fn plane_residual(point: [f64; 3], frame: &PlaneFrameMm) -> f64 {
    (0..3)
        .fold(Interval::point(0.0), |sum, k| {
            sum.add(
                Interval::point(point[k])
                    .sub(Interval::point(frame.origin_mm[k]))
                    .mul(Interval::point(frame.z_axis[k])),
            )
        })
        .abs_upper()
}

fn candidate_len(old_len: usize, cut: &CycleCut) -> usize {
    if cut.keep_middle {
        cut.second_edge - cut.first_edge + 2
    } else {
        old_len - (cut.second_edge - cut.first_edge) + 1
    }
}

fn candidate_point(points: &[[f64; 3]], cut: &CycleCut, index: usize) -> [f64; 3] {
    if cut.keep_middle {
        if index == 0 || index + 1 == candidate_len(points.len(), cut) {
            cut.point_mm
        } else {
            points[cut.first_edge + index]
        }
    } else if index <= cut.first_edge {
        points[index]
    } else if index == cut.first_edge + 1 {
        cut.point_mm
    } else {
        points[index + cut.second_edge - cut.first_edge - 1]
    }
}

pub(super) fn candidate_segments<'a>(
    points: &'a [[f64; 3]],
    cut: &'a CycleCut,
) -> impl Iterator<Item = [[f64; 3]; 2]> + 'a {
    let count = candidate_len(points.len(), cut) - 1;
    let start = if cut.keep_middle { 0 } else { cut.first_edge };
    (start..count).chain(0..start).map(move |index| {
        [
            candidate_point(points, cut, index),
            candidate_point(points, cut, index + 1),
        ]
    })
}

fn candidate_orientation(
    points: &[[f64; 3]],
    cut: &CycleCut,
    frame: &PlaneFrameMm,
    positive: bool,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let len = candidate_len(points.len(), cut);
    work.charge(len as u64, cancel)?;
    let origin = candidate_point(points, cut, 0);
    let mut previous = [Interval::point(0.0); 2];
    let mut area = Interval::point(0.0);
    for index in 1..len {
        let current = project(candidate_point(points, cut, index), origin, frame);
        area = area.add(cross(previous, current));
        previous = current;
    }
    Ok(area.lo.is_finite()
        && area.hi.is_finite()
        && if positive {
            area.lo > 0.0
        } else {
            area.hi < 0.0
        })
}

/// The complete replaced chain and its new segments enclose every possible
/// changed winding region, even when the old loop is self-intersecting. A
/// foreign boundary strictly outside its conservative projected rectangle can
/// neither intersect that region nor be swallowed by it. Overlap refuses the
/// proposal, including a whole tiny hole with no boundary crossing.
fn foreign_boundaries_clear(
    section: &NativeSection,
    loop_index: usize,
    cut: &CycleCut,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    if section.loops.len() == 1 {
        return Ok(true);
    }
    let points = &section.loops[loop_index].points_mm;
    work.charge(points.len() as u64, cancel)?;
    let mut bounds = project(cut.point_mm, section.frame.origin_mm, &section.frame);
    for (index, point) in points.iter().enumerate() {
        let replaced = if cut.keep_middle {
            index <= cut.first_edge + 1 || index >= cut.second_edge
        } else {
            index >= cut.first_edge && index <= cut.second_edge + 1
        };
        if replaced {
            let local = project(*point, section.frame.origin_mm, &section.frame);
            for k in 0..2 {
                bounds[k].lo = bounds[k].lo.min(local[k].lo);
                bounds[k].hi = bounds[k].hi.max(local[k].hi);
            }
        }
    }
    for (other, boundary) in section.loops.iter().enumerate() {
        if other == loop_index {
            continue;
        }
        work.charge(boundary.points_mm.len() as u64, cancel)?;
        for edge in boundary.points_mm.windows(2) {
            let ends = [edge[0], edge[1]]
                .map(|point| project(point, section.frame.origin_mm, &section.frame));
            let separated = (0..2).any(|k| {
                ends[0][k].hi.max(ends[1][k].hi) < bounds[k].lo
                    || ends[0][k].lo.min(ends[1][k].lo) > bounds[k].hi
            });
            if !separated {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn compact(points: &mut Vec<[f64; 3]>, cut: &CycleCut) {
    let len = candidate_len(points.len(), cut);
    // Forward writes precede every unread source element, for either cycle.
    for index in 0..len {
        points[index] = candidate_point(points, cut, index);
    }
    points.truncate(len);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PlaneMm, SectionLoopMm};
    use spiling_contracts::geometry::{GeometryErrorCode, MAX_SECTION_BYTES};

    fn reference_section(points: Vec<[f64; 3]>, chord_error: f64) -> NativeSection {
        let plane = PlaneMm {
            origin_mm: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
        };
        NativeSection {
            plane,
            frame: PlaneFrameMm::from_plane(plane).unwrap(),
            loops: vec![SectionLoopMm {
                is_hole: false,
                points_mm: points,
            }],
            loop_source_faces: vec![vec![3, 7]],
            sampling_tolerance_mm: SECTION_SAMPLING_TOLERANCE_MM,
            boolean_tolerance_mm: spiling_contracts::geometry::SECTION_BOOLEAN_TOLERANCE_MM,
            certificate: crate::NativeSectionCertificate {
                definition_id: spiling_contracts::geometry::DefinitionId::from_source_sha256(
                    &[0; 32],
                ),
                authority: crate::SectionAuthority::NativeIntersection,
                maximum_chord_error_mm: chord_error,
                maximum_plane_error_mm: 0.0,
            },
        }
    }

    fn small_bowtie() -> Vec<[f64; 3]> {
        vec![
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [1.9999, 0.000001, 0.0],
            [2.0, -0.000003, 0.0],
            [3.0, 0.0, 0.0],
            [3.0, 3.0, 0.0],
            [0.0, 3.0, 0.0],
            [0.0, 0.0, 0.0],
        ]
    }

    fn normalize(section: NativeSection) -> NativeSection {
        simplify_section_reference(
            section,
            SECTION_SAMPLING_TOLERANCE_MM,
            MAX_SECTION_BYTES,
            &mut GeometryWorkBudget::new(1_000_000),
            &AtomicBool::new(false),
        )
        .unwrap()
    }

    #[test]
    fn bounded_reference_cycle_erases_sampled_bowtie_with_geometric_not_material_authority() {
        let original = small_bowtie();
        let section = reference_section(original.clone(), 0.00499);
        assert!(
            crossing_point(
                original[0],
                original[1],
                original[2],
                original[3],
                &section.frame
            )
            .is_some()
        );
        let section = normalize(section);
        let points = &section.loops[0].points_mm;
        assert!(points.len() < original.len());
        assert_eq!(points.first(), points.last());
        for first in 0..points.len() - 1 {
            for second in first + 2..points.len() - 1 {
                if first == 0 && second + 1 == points.len() - 1 {
                    continue;
                }
                assert!(
                    crossing_point(
                        points[first],
                        points[first + 1],
                        points[second],
                        points[second + 1],
                        &section.frame
                    )
                    .is_none()
                );
            }
        }
        assert!(section.achieved_chord_error_mm() > 0.00499);
        assert!(section.achieved_chord_error_mm() < SECTION_SAMPLING_TOLERANCE_MM);
        assert_eq!(section.achieved_plane_error_mm(), 0.0);
        assert_eq!(section.loop_source_faces, [vec![3, 7]]);
    }

    #[test]
    fn bounded_reference_cycle_preserves_foreign_hole_inside_erased_cycle() {
        let original = small_bowtie();
        let hole = vec![
            [1.99995, 0.0000001, 0.0],
            [1.99995, 0.0000002, 0.0],
            [1.99996, 0.0000002, 0.0],
            [1.99996, 0.0000001, 0.0],
            [1.99995, 0.0000001, 0.0],
        ];
        let mut section = reference_section(original.clone(), 0.00499);
        section.loops.push(SectionLoopMm {
            is_hole: true,
            points_mm: hole.clone(),
        });
        section.loop_source_faces.push(vec![11]);
        let section = normalize(section);
        assert_eq!(section.loops[0].points_mm, original);
        assert_eq!(section.loops[1].points_mm, hole);
        assert!(section.loops[1].is_hole);
        assert_eq!(section.loop_source_faces[1], [11]);
    }

    #[test]
    fn bounded_reference_cycle_retains_unaffordable_crossing_and_real_concavity() {
        let original = small_bowtie();
        let section = normalize(reference_section(original.clone(), 0.004999999));
        assert_eq!(section.loops[0].points_mm, original);
        let concave = vec![
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [1.0, 0.1, 0.0],
            [3.0, 0.0, 0.0],
            [3.0, 3.0, 0.0],
            [0.0, 3.0, 0.0],
            [0.0, 0.0, 0.0],
        ];
        assert_eq!(
            normalize(reference_section(concave.clone(), 0.00499)).loops[0].points_mm,
            concave
        );
    }

    #[test]
    fn bounded_reference_cycle_charges_successive_compactions_against_one_remaining_allowance() {
        let points = vec![
            [0.0, 0.0, 0.0],
            [2.0, 0.0, 0.0],
            [1.999999, 0.00000001, 0.0],
            [2.0, -0.00000003, 0.0],
            [3.0, 0.0, 0.0],
            [3.0, 2.0, 0.0],
            [2.99999999, 1.999999, 0.0],
            [3.00000003, 2.0, 0.0],
            [3.0, 3.0, 0.0],
            [0.0, 3.0, 0.0],
            [0.0, 0.0, 0.0],
        ];
        let section = normalize(reference_section(points, 0.00498));
        // Each erased crossing has an old vertex almost 0.03 micrometres
        // from the retained segments. The cumulative charge must include
        // both edits, not just the last one.
        assert!(section.achieved_chord_error_mm() - 0.00498 > 0.00000005);
        assert!(section.achieved_chord_error_mm() - 0.00498 < 0.00001);
    }

    #[test]
    fn narrowed_reference_budget_bounds_actual_cycle_correspondence() {
        let maximum = SECTION_SAMPLING_TOLERANCE_MM * 0.5;
        let original = small_bowtie();
        let normalized = simplify_section_reference(
            reference_section(original.clone(), maximum - 0.00001),
            maximum,
            MAX_SECTION_BYTES,
            &mut GeometryWorkBudget::new(1_000_000),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(normalized.loops[0].points_mm.len() < original.len());
        assert!(normalized.achieved_chord_error_mm() <= maximum);
        let retained = simplify_section_reference(
            reference_section(original.clone(), maximum - 0.000000001),
            maximum,
            MAX_SECTION_BYTES,
            &mut GeometryWorkBudget::new(1_000_000),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(retained.loops[0].points_mm, original);
        assert!(retained.achieved_chord_error_mm() <= maximum);
    }

    #[test]
    fn bounded_reference_cycle_consumes_owner_on_capacity_work_and_cancellation_failure() {
        for (capacity, work, cancelled, expected) in [
            (0, 100_000, false, GeometryErrorCode::ResourceLimit),
            (
                MAX_SECTION_BYTES,
                1,
                false,
                GeometryErrorCode::ResourceLimit,
            ),
            (0, 0, true, GeometryErrorCode::Cancelled),
        ] {
            let error = simplify_section_reference(
                reference_section(small_bowtie(), 0.00499),
                SECTION_SAMPLING_TOLERANCE_MM,
                capacity,
                &mut GeometryWorkBudget::new(work),
                &AtomicBool::new(cancelled),
            )
            .unwrap_err();
            assert_eq!(error.code, expected);
        }
    }

    #[test]
    fn bounded_reference_middle_cycle_compacts_without_reversing_native_hole_orientation() {
        let mut points = small_bowtie();
        points.pop();
        points.rotate_left(2);
        points.push(points[0]);
        let section = normalize(reference_section(points.clone(), 0.00499));
        assert!(section.loops[0].points_mm.len() < points.len());
        let mut hole_points = small_bowtie();
        hole_points.reverse();
        let mut hole = reference_section(hole_points.clone(), 0.00499);
        hole.loops[0].is_hole = true;
        let hole = normalize(hole);
        assert!(hole.loops[0].is_hole);
        assert!(hole.loops[0].points_mm.len() < hole_points.len());
        assert_eq!(
            hole.loops[0].points_mm.first(),
            hole.loops[0].points_mm.last()
        );
    }
}
