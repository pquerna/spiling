// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Automatic retained joint flanges. Geometry and bead dimensions select the
//! construction; no load direction, stiffness or material calibration is inferred.
use super::*;

fn separated(roots: &[f64], index: usize, lattice: Lattice, width: f64) -> bool {
    let previous = index.checked_sub(1).map_or(lattice.min, |i| roots[i]);
    let next = roots.get(index + 1).copied().unwrap_or(lattice.max);
    roots[index] - previous >= 8.0 * width && next - roots[index] >= 8.0 * width
}

#[expect(
    clippy::too_many_arguments,
    reason = "borrowed domain and cumulative construction budgets"
)]
pub(super) fn append(
    polygon: &Polygon<f64>,
    roots: &[Vec<f64>; 2],
    lattices: &[Lattice; 2],
    layer: &HatchLayer<'_>,
    paths: &mut Vec<DepositionPath>,
    segments: &mut usize,
    work: &mut usize,
    cancel: &AtomicBool,
    memory: &mut HatchMemory,
) -> Result<(), ManufacturingError> {
    let edges = std::iter::once(polygon.exterior())
        .chain(polygon.interiors())
        .map(|r| r.0.len() - 1)
        .sum::<usize>();
    let depth = 3.0 * layer.width_mm;
    let mut center = Vec::new();
    let mut sides = [Vec::new(), Vec::new()];
    for fixed in 0..2 {
        let active = HatchLayer {
            axis: if fixed == 0 {
                HatchAxis::Y
            } else {
                HatchAxis::X
            },
            ..*layer
        };
        for (index, &root) in roots[fixed].iter().enumerate() {
            charge(work, 1, cancel)?;
            if !separated(&roots[fixed], index, lattices[fixed], layer.width_mm) {
                continue;
            }
            charge(work, edges * 3, cancel)?;
            center.clear();
            row_intersections(polygon, root, active.axis, &mut center, cancel)?;
            // Alternate the preferred handedness across parallel webs. Try the
            // other side only when the preferred flange would cross a boundary.
            let sign = if index.is_multiple_of(2) { 1.0 } else { -1.0 };
            let coordinates = [root + sign * layer.width_mm, root - sign * layer.width_mm];
            for (hits, &coordinate) in sides.iter_mut().zip(&coordinates) {
                hits.clear();
                row_intersections(polygon, coordinate, active.axis, hits, cancel)?;
            }
            for chord in center.as_chunks::<2>().0 {
                let along = &roots[1 - fixed];
                let begin = along.partition_point(|v| *v < chord[0] + depth);
                let end = along.partition_point(|v| *v <= chord[1] - depth);
                for node in begin..end {
                    charge(work, 1, cancel)?;
                    if !separated(along, node, lattices[1 - fixed], layer.width_mm) {
                        continue;
                    }
                    let span = [along[node] - depth, along[node] + depth];
                    for (hits, &coordinate) in sides.iter().zip(&coordinates) {
                        charge(work, hits.len(), cancel)?;
                        if hits
                            .as_chunks::<2>()
                            .0
                            .iter()
                            .any(|p| p[0] <= span[0] && span[1] <= p[1])
                        {
                            memory.append(&span, coordinate, &active, false, paths, segments)?;
                            break;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn construct(polygon: Polygon<f64>, width: f64, z: f64) -> Vec<DepositionPath> {
        let recipe = serde_json::from_str::<ManufacturingIntent>(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/manufacturing/web-roof.intent.json"
        )))
        .unwrap()
        .recipe;
        let layer = HatchLayer {
            roof: Some(Roof {
                z_mm: 48.,
                frame_mm: [[0., 0.], [48., 48.]],
                ceilings: &[],
                columns: None,
            }),
            width_mm: width,
            z,
            lower_z: z - 0.2,
            height: 0.2,
            axis: HatchAxis::X,
            top_axis: HatchAxis::X,
            recipe: &recipe,
        };
        let mut result = Vec::new();
        let mut memory = HatchMemory::new(&result, 0);
        append(
            &polygon,
            &[vec![8., 24., 40.], vec![8., 24., 40.]],
            &[Lattice::new(0., 48., 16.).unwrap(); 2],
            &layer,
            &mut result,
            &mut 0,
            &mut 0,
            &AtomicBool::new(false),
            &mut memory,
        )
        .unwrap();
        result
    }

    #[test]
    fn automatic_joints_scale_with_beads_and_keep_their_footprint_at_every_height() {
        let polygon = geo::Rect::new([0., 0.], [48., 48.]).to_polygon();
        let fine = construct(polygon.clone(), 0.45, 1.);
        let upper = construct(polygon.clone(), 0.45, 40.);
        let wide = construct(polygon, 0.9, 1.);
        assert_eq!(fine.len(), 18); // One side, both families, nine true intersections.
        for (a, b) in fine.iter().zip(&upper) {
            for (p, q) in a.points_mm.iter().zip(&b.points_mm) {
                assert_eq!(&p[..2], &q[..2]);
            }
        }
        for (paths, length) in [(&fine, 2.7), (&wide, 5.4)] {
            assert_eq!(paths.len(), 18);
            for path in paths {
                let [a, b] = [path.points_mm[0], path.points_mm[1]];
                assert!(((b[0] - a[0]).hypot(b[1] - a[1]) - length).abs() < 1e-10);
            }
        }
    }

    #[test]
    fn dense_joints_and_missing_intersections_do_not_acquire_orphan_flanges() {
        let polygon = geo::Rect::new([0., 0.], [48., 48.]).to_polygon();
        assert!(construct(polygon.clone(), 2.1, 1.).is_empty());
        let hole = geo::Rect::new([22., 22.], [26., 26.]).to_polygon();
        let perforated = Polygon::new(polygon.exterior().clone(), vec![hole.exterior().clone()]);
        let paths = construct(perforated, 0.45, 1.);
        assert_eq!(paths.len(), 16);
        for p in paths {
            let a = p.points_mm[0];
            let b = p.points_mm[1];
            assert!(!((22.0..26.0).contains(&a[0]) && (22.0..26.0).contains(&a[1])));
            assert!(!((22.0..26.0).contains(&b[0]) && (22.0..26.0).contains(&b[1])));
        }
    }

    #[test]
    fn automatic_joints_try_the_clear_side_without_bridging_a_hole() {
        let outer = geo::Rect::new([0., 0.], [48., 48.]).to_polygon();
        let hole = geo::Rect::new([23.8, 8.2], [24.2, 8.8]).to_polygon();
        let paths = construct(
            Polygon::new(outer.exterior().clone(), vec![hole.exterior().clone()]),
            0.45,
            1.,
        );
        let horizontal_at = |y: f64| {
            paths.iter().any(|p| {
                let [a, b] = [p.points_mm[0], p.points_mm[1]];
                (a[1] - y).abs() < 1e-10 && (b[1] - y).abs() < 1e-10 && a[0] < 24. && b[0] > 24.
            })
        };
        assert!(horizontal_at(7.55));
        assert!(!horizontal_at(8.45));
    }
}
