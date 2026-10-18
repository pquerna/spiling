// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;
use geo::{Area, Contains, LineString, Point, Polygon};
use std::sync::atomic::Ordering;

fn rectangle(left: f64, right: f64) -> MultiPolygon<f64> {
    MultiPolygon(vec![Polygon::new(
        LineString::from(vec![
            (left, 0.0),
            (right, 0.0),
            (right, 10.0),
            (left, 10.0),
            (left, 0.0),
        ]),
        Vec::new(),
    )])
}
fn sections(materials: impl IntoIterator<Item = MultiPolygon<f64>>) -> Vec<SectionSpan> {
    materials
        .into_iter()
        .enumerate()
        .map(|(i, material)| SectionSpan {
            occupied: None,
            start: i as u32,
            end: i as u32 + 1,
            source_area_mm2: material.unsigned_area(),
            material: Arc::new(material),
            process_domains: Vec::new(),
        })
        .collect()
}
fn source_points(sections: &[SectionSpan]) -> usize {
    sections
        .iter()
        .map(|span| super::super::region::point_count(&span.material))
        .sum()
}
fn ring(radius: f64, count: usize, clockwise: bool) -> LineString<f64> {
    LineString::from(
        (0..=count)
            .map(|i| {
                let angle = std::f64::consts::TAU * (i % count) as f64 / count as f64
                    * if clockwise { -1.0 } else { 1.0 };
                (radius * angle.cos(), radius * angle.sin())
            })
            .collect::<Vec<_>>(),
    )
}
fn annulus(outer: f64, hole: f64) -> MultiPolygon<f64> {
    MultiPolygon(vec![Polygon::new(
        ring(outer, 512, false),
        vec![ring(hole, 64, true)],
    )])
}

#[test]
fn growing_shrinking_and_sliding_horizons_intersect_every_realized_span() {
    let spans = sections(
        [
            rectangle(0.0, 12.0),
            rectangle(1.0, 11.0),
            rectangle(-1.0, 10.0),
            rectangle(2.0, 13.0),
            rectangle(0.5, 9.0),
            rectangle(1.5, 12.0),
            rectangle(-2.0, 14.0),
        ]
        .map(|mut material| {
            material.0[0].interiors_push(LineString::from(vec![
                (4.0, 3.0),
                (4.0, 7.0),
                (6.0, 7.0),
                (6.0, 3.0),
                (4.0, 3.0),
            ]));
            material
        }),
    );
    let cancel = AtomicBool::new(false);
    let mut index = SkinNeighborhoods::new(&spans, 0.0, 0).unwrap();
    for (a, b) in [
        (0, 0),
        (0, 1),
        (0, 5),
        (0, 2),
        (1, 4),
        (2, 5),
        (4, 2),
        (6, 3),
        (0, 6),
        (3, 3),
        (1, 5),
    ] {
        let range = &spans[a.min(b)..=a.max(b)];
        let left = range
            .iter()
            .map(|s| s.material.0[0].exterior().0[0].x)
            .fold(f64::NEG_INFINITY, f64::max);
        let right = range
            .iter()
            .map(|s| s.material.0[0].exterior().0[1].x)
            .fold(f64::INFINITY, f64::min);
        let material = index
            .survival(a, Some(b), source_points(&spans), &cancel)
            .unwrap();
        assert!((material.unsigned_area() - ((right - left) * 10.0 - 8.0)).abs() < 1e-5);
        assert_eq!(material.0.len(), 1);
        assert_eq!(material.0[0].interiors().len(), 1);
        assert!(!material.contains(&Point::new(5.0, 5.0)));
        assert!(material.contains(&Point::new((left + right) * 0.5, 1.0)));
    }
    // Inclusive exact endpoint changes the surviving right boundary from 10 to 9.
    let before = index
        .survival(1, Some(3), source_points(&spans), &cancel)
        .unwrap();
    let at = index
        .survival(
            1,
            Some(4),
            source_points(&spans) + before.temporary_points(),
            &cancel,
        )
        .unwrap();
    assert!((before.unsigned_area() - at.unsigned_area() - 10.0).abs() < 1e-5);
}

#[test]
fn sliding_512_vertex_annuli_keep_expected_minimum_radius_and_protected_hole() {
    let dimensions = (0..96)
        .map(|i| {
            (
                10.0 + (i as f64 * 0.137).sin() * 0.3,
                2.0 + (i as f64 * 0.213).cos() * 0.05,
            )
        })
        .collect::<Vec<_>>();
    let spans = sections(dimensions.iter().map(|&(outer, hole)| annulus(outer, hole)));
    let cancel = AtomicBool::new(false);
    let mut index = SkinNeighborhoods::new(&spans, 0.0, 73).unwrap();
    // Sources plus all 93 aligned internal domains would exceed100k.
    // Reclaiming older unpinned ranges must preserve each exact horizon.
    assert!((96 + 93) * (513 + 65) > MAX_DEPOSITION_SEGMENTS as usize);
    for start in 0..=64 {
        let end = start + 31;
        let outer = dimensions[start..=end]
            .iter()
            .map(|d| d.0)
            .fold(f64::INFINITY, f64::min);
        let hole = dimensions[start..=end]
            .iter()
            .map(|d| d.1)
            .fold(f64::NEG_INFINITY, f64::max);
        let material = index
            .survival(start, Some(end), source_points(&spans), &cancel)
            .unwrap();
        let outer_area = 256.0 * outer.powi(2) * (std::f64::consts::TAU / 512.0).sin();
        let hole_area = 32.0 * hole.powi(2) * (std::f64::consts::TAU / 64.0).sin();
        assert!(
            (material.unsigned_area() - (outer_area - hole_area)).abs() < 1e-4,
            "window {start}..={end} changed the expected domain"
        );
        assert_eq!(material.0.len(), 1);
        assert_eq!(material.0[0].interiors().len(), 1);
        for point in &material.0[0].exterior().0 {
            assert!((point.x.hypot(point.y) - outer).abs() < 1e-5);
        }
        assert!(!material.contains(&Point::new(0.0, 0.0)));
        assert!(material.contains(&Point::new((outer + hole) * 0.5, 0.0)));
        assert!(
            source_points(&spans) + index.retained_points() + material.temporary_points()
                <= MAX_DEPOSITION_SEGMENTS as usize
        );
    }
    assert!(*index.work_mut() < MAX_BOOLEAN_PAIRS);
    assert!(index.retained_points() > 0);
}

#[test]
fn leaves_borrow_aligned_ranges_reuse_and_lower_lives_across_upper_query() {
    let spans = sections((0..8).map(|i| rectangle(i as f64 * 0.1, 12.0 - i as f64 * 0.1)));
    let cancel = AtomicBool::new(false);
    let mut index = SkinNeighborhoods::new(&spans, 0.0, 0).unwrap();
    let leaf = index
        .survival(3, Some(3), source_points(&spans), &cancel)
        .unwrap();
    assert!(std::ptr::eq(&*leaf, &*spans[3].material));
    assert_eq!(leaf.temporary_points(), 0);
    assert_eq!(index.retained_points(), 0);
    let aligned = index
        .survival(0, Some(3), source_points(&spans), &cancel)
        .unwrap();
    let retained = index.retained_points();
    let work = *index.work_mut();
    let reverse = index
        .survival(3, Some(0), source_points(&spans), &cancel)
        .unwrap();
    assert!(std::ptr::eq(&*aligned, &*reverse));
    assert_eq!(index.retained_points(), retained);
    assert!(*index.work_mut() - work < 16);
    let lower = index
        .survival(1, Some(5), source_points(&spans), &cancel)
        .unwrap();
    assert_eq!(lower.temporary_points(), 5);
    let upper = index
        .survival(
            2,
            Some(6),
            source_points(&spans) + lower.temporary_points(),
            &cancel,
        )
        .unwrap();
    assert!((lower.unsigned_area() - 110.0).abs() < 1e-5);
    assert!((upper.unsigned_area() - 108.0).abs() < 1e-5);
}

#[test]
fn empty_identity_absent_and_non_power_of_two_ranges_are_safe() {
    let cancel = AtomicBool::new(false);
    let empty_spans = Vec::new();
    let mut empty = SkinNeighborhoods::new(&empty_spans, 0.0, 0).unwrap();
    assert!(empty.survival(0, None, 0, &cancel).unwrap().0.is_empty());
    assert_eq!(
        empty.survival(0, Some(0), 0, &cancel).err().unwrap().code,
        ManufacturingErrorCode::ResourceLimit
    );
    let spans = sections([
        rectangle(0.0, 1.0),
        rectangle(2.0, 3.0),
        rectangle(2.0, 4.0),
    ]);
    let mut index = SkinNeighborhoods::new(&spans, 0.0, 0).unwrap();
    assert!(
        index
            .survival(0, Some(2), source_points(&spans), &cancel)
            .unwrap()
            .0
            .is_empty()
    );
    let work = *index.work_mut();
    assert!(
        index
            .survival(1, Some(0), source_points(&spans), &cancel)
            .unwrap()
            .0
            .is_empty()
    );
    assert!(*index.work_mut() - work < 16);
    assert_eq!(index.retained_points(), 0);
    let last = index
        .survival(2, Some(2), source_points(&spans), &cancel)
        .unwrap();
    assert!(std::ptr::eq(&*last, &*spans[2].material));
    assert!(
        index
            .survival(usize::MAX, None, 0, &cancel)
            .unwrap()
            .0
            .is_empty()
    );
    assert_eq!(
        index
            .survival(0, Some(usize::MAX), 0, &cancel)
            .err()
            .unwrap()
            .code,
        ManufacturingErrorCode::ResourceLimit
    );
    assert_eq!(
        SkinNeighborhoods::new(&spans, 0.0, usize::MAX)
            .err()
            .unwrap()
            .code,
        ManufacturingErrorCode::ResourceLimit
    );
}

#[test]
fn cache_and_live_query_temporary_share_retained_point_admission() {
    let spans = sections((0..4).map(|i| rectangle(i as f64, 12.0)));
    let cancel = AtomicBool::new(false);
    let mut index = SkinNeighborhoods::new(&spans, 0.0, 0).unwrap();
    let lower = index
        .survival(0, Some(2), source_points(&spans), &cancel)
        .unwrap();
    assert_eq!(index.retained_points(), 5);
    assert_eq!(lower.temporary_points(), 5);
    let pinned = index
        .survival(0, Some(1), source_points(&spans), &cancel)
        .unwrap();
    let external = MAX_DEPOSITION_SEGMENTS as usize - 15 + lower.temporary_points();
    let failure = index.survival(1, Some(3), external, &cancel).err().unwrap();
    assert_eq!(failure.code, ManufacturingErrorCode::ResourceLimit);
    assert!(failure.message.contains("retained"));
    // The new aligned range fits; only its simultaneously live folded query
    // result exceeds the aggregate retained budget. Lower remains usable.
    assert_eq!(index.retained_points(), 10);
    assert!((lower.unsigned_area() - 100.0).abs() < 1e-5);
    assert!((pinned.unsigned_area() - 110.0).abs() < 1e-5);
    assert_eq!(
        index
            .survival(0, Some(1), MAX_DEPOSITION_SEGMENTS as usize - 4, &cancel)
            .err()
            .unwrap()
            .code,
        ManufacturingErrorCode::ResourceLimit
    );
    let cached = index
        .survival(0, Some(1), MAX_DEPOSITION_SEGMENTS as usize - 5, &cancel)
        .unwrap();
    assert_eq!(cached.temporary_points(), 0);
}

fn comb(teeth: usize, transpose: bool) -> MultiPolygon<f64> {
    let width = 2.0 * teeth as f64;
    let mut points = vec![(0.0, 0.0), (width, 0.0), (width, 1.0)];
    for tooth in (0..teeth).rev() {
        let x = tooth as f64 * 2.0;
        points.extend([(x + 1.0, 1.0), (x + 1.0, width), (x, width), (x, 1.0)]);
    }
    points.push((0.0, 0.0));
    if transpose {
        for point in &mut points {
            *point = (point.1, point.0);
        }
        points.reverse();
    }
    MultiPolygon(vec![Polygon::new(LineString::from(points), Vec::new())])
}

#[test]
fn genuine_dense_comb_intersections_exhaust_shared_work_without_cache_publication() {
    let spans = sections([comb(2300, false), comb(2300, true)]);
    let mut index = SkinNeighborhoods::new(&spans, 0.0, MAX_BOOLEAN_PAIRS - 1_000_000).unwrap();
    let failure = index
        .survival(0, Some(1), source_points(&spans), &AtomicBool::new(false))
        .err()
        .unwrap();
    assert_eq!(failure.code, ManufacturingErrorCode::ResourceLimit);
    assert_eq!(index.retained_points(), 0);
    assert!(*index.work_mut() <= MAX_BOOLEAN_PAIRS);
}

#[test]
fn attribution_and_neighborhoods_keep_one_counter_and_cancel_warm_state() {
    let spans = sections([rectangle(0.0, 12.0), rectangle(1.0, 11.0)]);
    let cancel = AtomicBool::new(false);
    let mut index = SkinNeighborhoods::new(&spans, 0.0, 101).unwrap();
    let initial = *index.work_mut();
    classification::intersection_accumulated(
        &spans[0].material,
        &spans[1].material,
        index.work_mut(),
        &cancel,
    )
    .unwrap();
    assert!(*index.work_mut() > initial);
    let result = index
        .survival(0, Some(1), source_points(&spans), &cancel)
        .unwrap();
    let retained = index.retained_points();
    let work = *index.work_mut();
    cancel.store(true, Ordering::Relaxed);
    for neighbor in [None, Some(0), Some(1)] {
        assert_eq!(
            index
                .survival(0, neighbor, source_points(&spans), &cancel)
                .err()
                .unwrap()
                .code,
            ManufacturingErrorCode::Cancelled
        );
    }
    assert_eq!(index.retained_points(), retained);
    assert_eq!(*index.work_mut(), work);
    assert!((result.unsigned_area() - 100.0).abs() < 1e-5);
}

#[test]
fn charged_lru_eviction_skips_pins_and_recomputed_ranges_keep_exact_geometry() {
    let spans = sections((0..8).map(|i| rectangle(i as f64, 12.0)));
    let cancel = AtomicBool::new(false);
    let mut index = SkinNeighborhoods::new(&spans, 0.0, 0).unwrap();
    for first in [0, 2, 4] {
        let material = index
            .survival(first, Some(first + 1), source_points(&spans), &cancel)
            .unwrap();
        assert!((material.unsigned_area() - (11 - first) as f64 * 10.0).abs() < 1e-5);
    }
    assert_eq!(index.retained_points(), 15);
    drop(
        index
            .survival(2, Some(3), source_points(&spans), &cancel)
            .unwrap(),
    );
    let work = *index.work_mut();
    index
        .survival(0, None, MAX_DEPOSITION_SEGMENTS as usize - 10, &cancel)
        .unwrap();
    assert_eq!(*index.work_mut() - work, index.nodes.len());
    assert!(matches!(index.nodes[4], Cached::Missing));
    assert!(matches!(index.nodes[5], Cached::Geometry(_, _, _)));
    assert!(matches!(index.nodes[6], Cached::Geometry(_, _, _)));
    let pinned = index
        .survival(2, Some(3), source_points(&spans), &cancel)
        .unwrap();
    assert_eq!(
        index
            .survival(0, None, MAX_DEPOSITION_SEGMENTS as usize - 4, &cancel)
            .err()
            .unwrap()
            .code,
        ManufacturingErrorCode::ResourceLimit
    );
    assert_eq!(index.retained_points(), 5);
    assert!(matches!(index.nodes[5], Cached::Geometry(_, _, _)));
    assert!(matches!(index.nodes[6], Cached::Missing));
    assert!((pinned.unsigned_area() - 90.0).abs() < 1e-5);
    let rebuilt = index
        .survival(0, Some(1), source_points(&spans), &cancel)
        .unwrap();
    assert!((rebuilt.unsigned_area() - 110.0).abs() < 1e-5);
}

#[test]
fn frozen_pool_uses_owned_operands_when_pinned_and_reclaims_after_release() {
    let spans = sections((0..4).map(|i| rectangle(i as f64, 12.0)));
    let cancel = AtomicBool::new(false);
    let mut index = SkinNeighborhoods::new(&spans, 0.0, 0).unwrap();
    let pinned = index
        .survival(0, Some(1), source_points(&spans), &cancel)
        .unwrap();
    assert_eq!(index.freeze_retained_capacity(), 5);
    let other = index
        .survival(2, Some(3), source_points(&spans), &cancel)
        .unwrap();
    assert_eq!(other.temporary_points(), 5);
    assert_eq!(index.retained_points(), 5);
    assert!((other.unsigned_area() - 90.0).abs() < 1e-5);
    drop(other);
    let entire = index
        .survival(0, Some(3), source_points(&spans), &cancel)
        .unwrap();
    assert_eq!(entire.temporary_points(), 5);
    assert!((entire.unsigned_area() - 90.0).abs() < 1e-5);
    assert_eq!(index.retained_points(), 5);
    drop(entire);
    // Pinned lower and owned upper remain live with the output. Their actual
    // aggregate, not the frozen cache allowance alone, must fit the global cap.
    assert_eq!(
        index
            .survival(0, Some(3), MAX_DEPOSITION_SEGMENTS as usize - 14, &cancel)
            .err()
            .unwrap()
            .code,
        ManufacturingErrorCode::ResourceLimit
    );
    assert_eq!(index.retained_points(), 5);
    drop(pinned);
    let cached = index
        .survival(2, Some(3), source_points(&spans), &cancel)
        .unwrap();
    assert_eq!(cached.temporary_points(), 0);
    assert_eq!(index.retained_points(), 5);
    assert!(matches!(index.nodes[2], Cached::Missing));
    assert!(matches!(index.nodes[3], Cached::Geometry(_, _, _)));
}

#[test]
fn zero_frozen_pool_counts_both_recursive_temporary_operands_and_output() {
    let spans = sections((0..4).map(|i| rectangle(i as f64, 12.0)));
    let cancel = AtomicBool::new(false);
    let mut index = SkinNeighborhoods::new(&spans, 0.0, 0).unwrap();
    assert_eq!(index.freeze_retained_capacity(), 0);
    let material = index
        .survival(0, Some(3), source_points(&spans), &cancel)
        .unwrap();
    assert_eq!(material.temporary_points(), 5);
    assert!((material.unsigned_area() - 90.0).abs() < 1e-5);
    assert_eq!(index.retained_points(), 0);
    drop(material);
    assert_eq!(
        index
            .survival(0, Some(3), MAX_DEPOSITION_SEGMENTS as usize - 14, &cancel)
            .err()
            .unwrap()
            .code,
        ManufacturingErrorCode::ResourceLimit
    );
    let fits = index
        .survival(0, Some(3), MAX_DEPOSITION_SEGMENTS as usize - 15, &cancel)
        .unwrap();
    assert!((fits.unsigned_area() - 90.0).abs() < 1e-5);
    assert_eq!(index.retained_points(), 0);
}

#[test]
fn cancellation_at_eviction_scan_keeps_retained_geometry_and_work_unchanged() {
    let spans = sections([rectangle(0.0, 12.0), rectangle(1.0, 11.0)]);
    let mut index = SkinNeighborhoods::new(&spans, 0.0, 0).unwrap();
    drop(
        index
            .survival(0, Some(1), source_points(&spans), &AtomicBool::new(false))
            .unwrap(),
    );
    let work = *index.work_mut();
    let retained = index.retained_points();
    assert_eq!(
        index
            .evict_oldest(&AtomicBool::new(true))
            .err()
            .unwrap()
            .code,
        ManufacturingErrorCode::Cancelled
    );
    assert_eq!(*index.work_mut(), work);
    assert_eq!(index.retained_points(), retained);
    let material = index
        .survival(0, Some(1), source_points(&spans), &AtomicBool::new(false))
        .unwrap();
    assert!((material.unsigned_area() - 100.0).abs() < 1e-5);
}

#[test]
fn derived_final_d_horizons_match_brute_force_insets_with_protected_holes() {
    let cancel = AtomicBool::new(false);
    let inset = 0.6;
    let spans = sections((0..9).map(|i| {
        let mut material = rectangle((i % 3) as f64 * 0.4, 13.0 - (i % 4) as f64 * 0.3);
        let x = 5.0 + (i % 2) as f64 * 0.2;
        material.0[0].interiors_push(LineString::from(vec![
            (x, 3.0),
            (x, 7.0),
            (x + 1.0, 7.0),
            (x + 1.0, 3.0),
            (x, 3.0),
        ]));
        material
    }));
    let derived: Vec<_> = spans
        .iter()
        .map(|span| region::offset(&span.material, -inset, &cancel).unwrap())
        .collect();
    let mut index = SkinNeighborhoods::new(&spans, inset, 0).unwrap();
    for (first, last) in [(0, 0), (0, 7), (2, 5), (8, 3), (4, 4), (1, 8)] {
        let low = first.min(last);
        let high = first.max(last);
        let expected = derived[low + 1..high + 1]
            .iter()
            .fold(derived[low].clone(), |value, leaf| {
                classification::intersection(&value, leaf, &cancel).unwrap()
            });
        let actual = index
            .survival(first, Some(last), source_points(&spans), &cancel)
            .unwrap();
        assert!((actual.unsigned_area() - expected.unsigned_area()).abs() < 1e-5);
        assert!(geo::BooleanOps::xor(&*actual, &expected).unsigned_area() < 1e-5);
        assert!(!actual.contains(&Point::new(5.7, 5.0)));
        assert!(actual.contains(&Point::new(3.0, 2.0)));
    }
}

#[test]
fn derived_leaves_share_reclaimable_range_pool_and_survive_pinned_eviction() {
    let cancel = AtomicBool::new(false);
    let spans = sections((0..4).map(|i| rectangle(i as f64 * 0.1, 12.0)));
    let mut index = SkinNeighborhoods::new(&spans, 0.6, 0).unwrap();
    let leaf = index
        .survival(0, Some(0), source_points(&spans), &cancel)
        .unwrap();
    assert!(!std::ptr::eq(&*leaf, &*spans[0].material));
    assert_eq!(leaf.temporary_points(), 0);
    assert_eq!(index.retained_points(), region::point_count(&leaf));
    let again = index
        .survival(0, Some(0), source_points(&spans), &cancel)
        .unwrap();
    assert!(std::ptr::eq(&*leaf, &*again));
    drop(again);
    drop(
        index
            .survival(1, Some(1), source_points(&spans), &cancel)
            .unwrap(),
    );
    assert!(index.evict_oldest(&cancel).unwrap());
    assert_eq!(index.retained_points(), region::point_count(&leaf));
    let again = index
        .survival(0, Some(0), source_points(&spans), &cancel)
        .unwrap();
    assert!(std::ptr::eq(&*leaf, &*again));
    let pool = index.freeze_retained_capacity();
    let other = index
        .survival(1, Some(1), source_points(&spans), &cancel)
        .unwrap();
    assert_eq!(other.temporary_points(), region::point_count(&other));
    assert_eq!(index.retained_points(), pool);
    let expected = region::offset(&spans[1].material, -0.6, &cancel).unwrap();
    assert_eq!(&*other, &expected);
    drop(other);
    drop(again);
    drop(leaf);
    let rebuilt = index
        .survival(1, Some(1), source_points(&spans), &cancel)
        .unwrap();
    assert_eq!(rebuilt.temporary_points(), 0);
    assert_eq!(&*rebuilt, &expected);
    assert!(index.retained_points() <= pool);
}

#[test]
fn derived_leaf_zero_pool_work_retention_and_cancellation_refuse_honestly() {
    let cancel = AtomicBool::new(false);
    let spans = sections([rectangle(0.0, 12.0), rectangle(1.0, 11.0)]);
    let mut index = SkinNeighborhoods::new(&spans, 0.6, 0).unwrap();
    assert_eq!(index.freeze_retained_capacity(), 0);
    let result = index
        .survival(0, Some(1), source_points(&spans), &cancel)
        .unwrap();
    assert_eq!(result.temporary_points(), region::point_count(&result));
    assert_eq!(index.retained_points(), 0);
    let failure = index
        .survival(
            0,
            Some(0),
            MAX_DEPOSITION_SEGMENTS as usize - result.temporary_points(),
            &cancel,
        )
        .err()
        .unwrap();
    assert_eq!(failure.code, ManufacturingErrorCode::ResourceLimit);
    drop(result);
    let mut spent = SkinNeighborhoods::new(&spans, 0.6, 0).unwrap();
    // Exhaust the same cumulative counter before a missing derived leaf.
    *spent.work_mut() = MAX_BOOLEAN_PAIRS;
    assert_eq!(
        spent
            .survival(0, Some(0), source_points(&spans), &cancel)
            .err()
            .unwrap()
            .code,
        ManufacturingErrorCode::ResourceLimit,
    );
    assert_eq!(spent.retained_points(), 0);
    let mut warm = SkinNeighborhoods::new(&spans, 0.6, 0).unwrap();
    let leaf = warm
        .survival(0, Some(0), source_points(&spans), &cancel)
        .unwrap();
    cancel.store(true, Ordering::Relaxed);
    assert_eq!(
        warm.survival(0, Some(0), source_points(&spans), &cancel)
            .err()
            .unwrap()
            .code,
        ManufacturingErrorCode::Cancelled,
    );
    assert_eq!(warm.retained_points(), region::point_count(&leaf));
    let mut invalid = sections([rectangle(0.0, 12.0)]);
    Arc::get_mut(&mut invalid[0].material).unwrap().0[0]
        .exterior_mut(|ring| ring.0[0].x = f64::NAN);
    let mut refused = SkinNeighborhoods::new(&invalid, 0.6, 0).unwrap();
    assert_eq!(
        refused
            .survival(0, Some(0), 0, &AtomicBool::new(false))
            .err()
            .unwrap()
            .code,
        ManufacturingErrorCode::UnsupportedGeometry,
    );
    assert_eq!(refused.retained_points(), 0);
}

#[test]
fn returning_final_d_leaves_share_only_the_exact_derived_geometry() {
    let cancel = AtomicBool::new(false);
    let spans = sections([
        rectangle(0.0, 12.0),
        rectangle(2.0, 10.0),
        rectangle(0.0, 12.0),
    ]);
    let mut index = SkinNeighborhoods::new(&spans, 0.6, 0).unwrap();
    let first = index
        .survival(0, Some(0), source_points(&spans), &cancel)
        .unwrap();
    let retained = index.retained_points();
    let work = *index.work_mut();
    let returning = index
        .survival(2, Some(2), source_points(&spans), &cancel)
        .unwrap();
    assert!(std::ptr::eq(&*first, &*returning));
    assert_eq!(index.retained_points(), retained);
    assert!(*index.work_mut() - work < 16);
    // Returning to D says nothing about intervening physical material.
    let horizon = index
        .survival(0, Some(2), source_points(&spans), &cancel)
        .unwrap();
    let expected = region::offset(&spans[1].material, -0.6, &cancel).unwrap();
    assert!(geo::BooleanOps::xor(&*horizon, &expected).unsigned_area() < 1e-5);
    assert!(horizon.unsigned_area() < first.unsigned_area());
}

#[test]
fn canonical_leaf_identity_preserves_holes_frame_order_and_coordinate_bits() {
    let cancel = AtomicBool::new(false);
    let mut original = rectangle(0.0, 12.0);
    original.0[0].interiors_push(LineString::from(vec![
        (3.0, 3.0),
        (3.0, 5.0),
        (5.0, 5.0),
        (5.0, 3.0),
        (3.0, 3.0),
    ]));
    original.0[0].interiors_push(LineString::from(vec![
        (7.0, 3.0),
        (7.0, 5.0),
        (9.0, 5.0),
        (9.0, 3.0),
        (7.0, 3.0),
    ]));
    original.0.extend(rectangle(20.0, 24.0).0);
    let mut hole = original.clone();
    hole.0[0].interiors_mut(|rings| {
        rings[0].0[1].y = f64::from_bits(5.0_f64.to_bits() + 1);
    });
    let mut frame = original.clone();
    frame.0[0].exterior_mut(|ring| {
        for point in &mut ring.0 {
            point.x += 0.25;
        }
    });
    frame.0[0].interiors_mut(|rings| {
        for ring in rings {
            for point in &mut ring.0 {
                point.x += 0.25;
            }
        }
    });
    let mut order = original.clone();
    order.0[0].interiors_mut(|rings| rings.swap(0, 1));
    let mut winding = original.clone();
    winding.0[0].exterior_mut(|ring| ring.0.reverse());
    let mut signed_zero = original.clone();
    signed_zero.0[0].exterior_mut(|ring| ring.0[0].x = -0.0);
    let mut polygon_order = original.clone();
    polygon_order.0.swap(0, 1);
    let spans = sections([
        original.clone(),
        hole,
        frame,
        order,
        winding,
        signed_zero,
        polygon_order,
        original,
    ]);
    let mut index = SkinNeighborhoods::new(&spans, 0.2, 0).unwrap();
    let first = index
        .survival(0, Some(0), source_points(&spans), &cancel)
        .unwrap();
    for position in 1..spans.len() - 1 {
        let other = index
            .survival(position, Some(position), source_points(&spans), &cancel)
            .unwrap();
        assert!(!std::ptr::eq(&*first, &*other), "identity {position}");
    }
    let repeated = index
        .survival(
            spans.len() - 1,
            Some(spans.len() - 1),
            source_points(&spans),
            &cancel,
        )
        .unwrap();
    assert!(std::ptr::eq(&*first, &*repeated));
    assert!(!repeated.contains(&Point::new(4.0, 4.0)));
    assert!(!repeated.contains(&Point::new(8.0, 4.0)));
}

#[test]
fn canonical_leaves_remain_reclaimable_and_recompute_inside_frozen_pool() {
    let cancel = AtomicBool::new(false);
    let spans = sections([
        rectangle(0.0, 12.0),
        rectangle(1.0, 11.0),
        rectangle(0.0, 12.0),
        rectangle(1.0, 11.0),
    ]);
    let mut index = SkinNeighborhoods::new(&spans, 0.6, 0).unwrap();
    let pinned = index
        .survival(2, Some(2), source_points(&spans), &cancel)
        .unwrap();
    drop(
        index
            .survival(1, Some(1), source_points(&spans), &cancel)
            .unwrap(),
    );
    let pool = index.freeze_retained_capacity();
    index
        .admit_retained(MAX_DEPOSITION_SEGMENTS as usize - pinned.points, &cancel)
        .unwrap();
    assert_eq!(index.retained_points(), pinned.points);
    let again = index
        .survival(0, Some(0), source_points(&spans), &cancel)
        .unwrap();
    assert!(std::ptr::eq(&*pinned, &*again));
    let expected_area = pinned.unsigned_area();
    drop(pinned);
    drop(again);
    index
        .admit_retained(MAX_DEPOSITION_SEGMENTS as usize, &cancel)
        .unwrap();
    assert_eq!(index.retained_points(), 0);
    let rebuilt = index
        .survival(2, Some(2), source_points(&spans), &cancel)
        .unwrap();
    let canonical = index
        .survival(0, Some(0), source_points(&spans), &cancel)
        .unwrap();
    assert!(std::ptr::eq(&*rebuilt, &*canonical));
    assert!((rebuilt.unsigned_area() - expected_area).abs() < 1e-5);
    assert!(index.retained_points() <= pool);
    let work = *index.work_mut();
    cancel.store(true, Ordering::Relaxed);
    assert_eq!(
        index
            .survival(2, Some(2), source_points(&spans), &cancel)
            .err()
            .unwrap()
            .code,
        ManufacturingErrorCode::Cancelled
    );
    assert_eq!(*index.work_mut(), work);
}
