// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
use super::*;
use geo::Polygon;

fn native_ring(count: usize) -> LineString<f64> {
    let mut points = (0..count)
        .map(|index| {
            let theta = std::f64::consts::TAU * index as f64 / count as f64;
            Coord {
                x: 5.0 * theta.cos(),
                y: 5.0 * theta.sin(),
            }
        })
        .collect::<Vec<_>>();
    points.push(points[0]);
    LineString(points)
}
fn native_edge_lift(native: &LineString<f64>, phase: f64) -> LineString<f64> {
    let mut points = Vec::new();
    for edge in native.0.windows(2) {
        for fraction in [0.0, 0.5] {
            let p = Coord {
                x: edge[0].x + (edge[1].x - edge[0].x) * fraction,
                y: edge[0].y + (edge[1].y - edge[0].y) * fraction,
            };
            let radius = p.x.hypot(p.y);
            let height = 0.03 + 0.02 * (3.0 * p.y.atan2(p.x) + phase).sin();
            points.push(Coord {
                x: p.x + height * p.x / radius,
                y: p.y + height * p.y / radius,
            });
        }
    }
    points.push(points[0]);
    LineString(points)
}
fn brute_retention(original: &LineString<f64>, derived: &LineString<f64>) -> bool {
    (0..original.0.len() - 1).all(|old| {
        (0..derived.0.len() - 1).all(|new| retention_pair(original, old, derived, new).is_ok())
    })
}
fn brute_simple(material: &MultiPolygon<f64>) -> bool {
    let rings = || {
        material
            .0
            .iter()
            .flat_map(|polygon| std::iter::once(polygon.exterior()).chain(polygon.interiors()))
    };
    for (owner, ring) in rings().enumerate() {
        for index in 0..ring.0.len() - 1 {
            if ring.0[index] == ring.0[index + 1] {
                return false;
            }
            for (other_owner, other) in rings().enumerate().skip(owner) {
                for other_index in 0..other.0.len() - 1 {
                    if owner == other_owner && other_index <= index {
                        continue;
                    }
                    if simple_pair(ring, index, owner, other, other_index, other_owner).is_err() {
                        return false;
                    }
                }
            }
        }
    }
    true
}
#[test]
fn varying_native_edge_layers_preserve_exact_holes_with_shared_work() {
    let native = native_ring(128);
    let hole = LineString(vec![
        Coord { x: -0.5, y: -0.5 },
        Coord { x: -0.5, y: 0.5 },
        Coord { x: 0.5, y: 0.5 },
        Coord { x: 0.5, y: -0.5 },
        Coord { x: -0.5, y: -0.5 },
    ]);
    let cancel = AtomicBool::new(false);
    let mut work = 0;
    for layer in 0..96 {
        let derived = native_edge_lift(&native, layer as f64 * 0.137);
        certify_retention(&native, &derived, &mut work, &cancel).unwrap();
        let material = MultiPolygon(vec![Polygon::new(derived, vec![hole.clone()])]);
        certify_simple(&material, &mut work, &cancel).unwrap();
        assert_eq!(material.0[0].interiors(), std::slice::from_ref(&hole));
    }
    assert!(work < super::super::super::MAX_BOOLEAN_PAIRS);
}
#[test]
fn swept_candidates_match_exhaustive_crossings_contacts_and_simplicity() {
    let native = native_ring(40);
    let cancel = AtomicBool::new(false);
    for index in 0..40 {
        let mut derived = native_edge_lift(&native, index as f64 * 0.37);
        // Alternate entering crossings and exact native-vertex contacts; the
        // latter must retain the original neighboring-edge orientation checks.
        derived.0[index * 2] = if index % 2 == 0 {
            Coord {
                x: native.0[index].x * 0.99,
                y: native.0[index].y * 0.99,
            }
        } else {
            native.0[index]
        };
        derived.0[80] = derived.0[0];
        assert_eq!(
            certify_retention(&native, &derived, &mut 0, &cancel).is_ok(),
            brute_retention(&native, &derived)
        );
        let mut material = MultiPolygon(vec![Polygon::new(derived, Vec::new())]);
        if index % 3 == 0 {
            let points = &mut material.0[0];
            points.exterior_mut(|ring| ring.0.swap(7, 43));
        }
        assert_eq!(
            certify_simple(&material, &mut 0, &cancel).is_ok(),
            brute_simple(&material)
        );
    }
    // A protected hole contacting the exterior is never pruned by a strict
    // bounding-box comparison: closed boxes preserve endpoint equality.
    let outer = native_edge_lift(&native, 0.4);
    let point = outer.0[0];
    let touching = LineString(vec![
        point,
        Coord {
            x: point.x - 0.2,
            y: point.y + 0.1,
        },
        Coord {
            x: point.x - 0.2,
            y: point.y - 0.1,
        },
        point,
    ]);
    let material = MultiPolygon(vec![Polygon::new(outer, vec![touching])]);
    assert!(!brute_simple(&material));
    assert!(certify_simple(&material, &mut 0, &cancel).is_err());
}
#[test]
fn index_honors_cumulative_admission_cancellation_and_fresh_recovery() {
    let original = native_ring(128);
    let derived = native_edge_lift(&original, 0.4);
    let material = MultiPolygon(vec![Polygon::new(derived.clone(), Vec::new())]);
    let cancel = AtomicBool::new(false);
    let mut work = 0;
    certify_retention(&original, &derived, &mut work, &cancel).unwrap();
    let retention_work = work;
    assert!(retention_work > 0);
    certify_simple(&material, &mut work, &cancel).unwrap();
    assert!(
        work > retention_work,
        "successive proofs share cumulative work"
    );

    // A valid query cannot restart a caller's exhausted shared allowance.
    let mut exhausted = super::super::super::MAX_BOOLEAN_PAIRS;
    assert_eq!(
        certify_retention(&original, &derived, &mut exhausted, &cancel)
            .unwrap_err()
            .code,
        ManufacturingErrorCode::ResourceLimit
    );
    assert_eq!(exhausted, super::super::super::MAX_BOOLEAN_PAIRS);
    let mut cancelled_work = 0;
    assert_eq!(
        certify_retention(
            &original,
            &derived,
            &mut cancelled_work,
            &AtomicBool::new(true)
        )
        .unwrap_err()
        .code,
        ManufacturingErrorCode::Cancelled
    );

    // Neither refusal poisons later proofs over the same original material.
    let mut fresh_work = 0;
    certify_retention(&original, &derived, &mut fresh_work, &cancel).unwrap();
    certify_simple(&material, &mut fresh_work, &cancel).unwrap();
    assert_eq!(fresh_work, work);
}
