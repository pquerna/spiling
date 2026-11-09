// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Optional panel windows and vertical pier flanges. Native clipping and later
//! actual-anchor proof remain mandatory; these are not mechanical certificates.
use super::*;

pub(super) fn admit(
    a: WebArchitecture,
    roots: &[Vec<f64>; 2],
    lattices: &[Lattice; 2],
    roof: f64,
    layer: &HatchLayer<'_>,
) -> Result<(), ManufacturingError> {
    if let Some(o) = a.opening {
        // Any clipped chord's first/last root is at most one full root gap
        // from its boundary. Half-neighbour catchments are smaller. Protect
        // all possible corbel roots from their earliest 2:1 growth, plus one
        // physical interval and one bead of separation. No fixed top belt.
        let mut reach: f64 = 0.0;
        for axis in 0..2 {
            let mut last = lattices[axis].min;
            for &v in &roots[axis] {
                reach = reach.max(v - last);
                last = v;
            }
            reach = reach.max(lattices[axis].max - last);
        }
        let protected_from = roof - 2.0 * reach - layer.width_mm - layer.recipe.layer_height_mm;
        if o.bottom_z_mm + o.height_mm > protected_from {
            return Err(unsupported(&format!(
                "web opening reaches the protected roof-root corridor: top={} mm, corridor starts={} mm",
                o.bottom_z_mm + o.height_mm,
                protected_from
            )));
        }
    }
    Ok(())
}

pub(super) fn perforate(
    hits: &[f64],
    roots: &[f64],
    o: WebOpening,
    z: f64,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Vec<f64>, ManufacturingError> {
    let h = z - o.bottom_z_mm;
    if h <= 0.0 || h >= o.height_mm {
        return Ok(hits.to_vec());
    }
    let half = 0.5 * o.width_mm * ((o.height_mm - h) / o.cap_height_mm).min(1.0);
    let mut output = Vec::new();
    for chord in hits.as_chunks::<2>().0 {
        let begin = roots.partition_point(|v| *v < chord[0]);
        let end = roots.partition_point(|v| *v <= chord[1]);
        let mut start = chord[0];
        for bay in roots[begin..end].windows(2) {
            charge(work, 1, cancel)?;
            if bay[1] - bay[0] < o.width_mm + 2.0 * o.pier_width_mm {
                continue;
            }
            let center = bay[0] + (bay[1] - bay[0]) * 0.5;
            output.extend([start, center - half]);
            start = center + half;
            if output.len() > 2 * MAX_DEPOSITION_SEGMENTS as usize {
                return Err(resource("web opening interval budget exceeded"));
            }
        }
        output.extend([start, chord[1]]);
    }
    Ok(output)
}

#[expect(
    clippy::too_many_arguments,
    reason = "explicit borrowed domain and cumulative construction budgets"
)]
pub(super) fn ribs(
    polygon: &Polygon<f64>,
    roots: &[Vec<f64>; 2],
    depth: f64,
    layer: &HatchLayer<'_>,
    paths: &mut Vec<DepositionPath>,
    segments: &mut usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let edges = std::iter::once(polygon.exterior())
        .chain(polygon.interiors())
        .map(|r| r.0.len() - 1)
        .sum::<usize>();
    let mut hits = Vec::new();
    for fixed in 0..2 {
        let axis = if fixed == 0 {
            HatchAxis::Y
        } else {
            HatchAxis::X
        };
        let active = HatchLayer { axis, ..*layer };
        for &root in &roots[fixed] {
            for sign in [-1.0, 1.0] {
                let coordinate = root + sign * layer.width_mm;
                charge(work, edges, cancel)?;
                hits.clear();
                row_intersections(polygon, coordinate, axis, &mut hits, cancel)?;
                for chord in hits.as_chunks::<2>().0 {
                    for &node in &roots[1 - fixed] {
                        charge(work, 1, cancel)?;
                        let span = [(node - depth).max(chord[0]), (node + depth).min(chord[1])];
                        if span[1] - span[0] >= MIN_DEPOSITION_COMMAND_MM {
                            append_hatch_row(
                                &span,
                                coordinate,
                                DepositionKind::SparseFill,
                                &active,
                                false,
                                paths,
                                segments,
                            )?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

pub(super) fn merge(
    paths: &mut Vec<DepositionPath>,
    first: usize,
    layer: &HatchLayer<'_>,
    segments: &mut usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let n = paths.len() - first;
    charge(
        work,
        n.saturating_mul(2 + n.max(1).ilog2() as usize),
        cancel,
    )?;
    let mut rows = Vec::<(usize, f64, f64, f64)>::with_capacity(paths.len() - first);
    for p in paths.drain(first..) {
        check_cancel(cancel)?;
        let fixed = usize::from(p.points_mm[0][0] != p.points_mm[1][0]);
        let a = p.points_mm[0][1 - fixed];
        let b = p.points_mm[1][1 - fixed];
        rows.push((fixed, p.points_mm[0][fixed], a.min(b), a.max(b)));
        *segments -= 1;
    }
    rows.sort_unstable_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.total_cmp(&b.1))
            .then(a.2.total_cmp(&b.2))
    });
    let mut pending: Option<(usize, f64, f64, f64)> = None;
    let append = |p: (usize, f64, f64, f64), paths: &mut Vec<_>, segments: &mut usize| {
        let active = HatchLayer {
            axis: if p.0 == 0 { HatchAxis::Y } else { HatchAxis::X },
            ..*layer
        };
        append_hatch_row(
            &[p.2, p.3],
            p.1,
            DepositionKind::SparseFill,
            &active,
            false,
            paths,
            segments,
        )
    };
    for row in rows {
        check_cancel(cancel)?;
        if let Some(p) = &mut pending {
            if p.0 == row.0 && p.1 == row.1 && row.2 <= p.3 {
                p.3 = p.3.max(row.3);
                continue;
            }
            append(*p, paths, segments)?;
        }
        pending = Some(row);
    }
    if let Some(p) = pending {
        append(p, paths, segments)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_keep_intersection_piers_and_close_at_physical_height() {
        let o = WebOpening {
            bottom_z_mm: 2.,
            height_mm: 10.,
            width_mm: 4.,
            cap_height_mm: 4.,
            pier_width_mm: 1.8,
        };
        let cancel = AtomicBool::new(false);
        let rows = |z| perforate(&[0., 30.], &[2., 12., 22.], o, z, &mut 0, &cancel).unwrap();
        assert_eq!(rows(2.), vec![0., 30.]);
        assert_eq!(rows(8.), vec![0., 5., 9., 15., 19., 30.]);
        assert_eq!(rows(11.), vec![0., 6.5, 7.5, 16.5, 17.5, 30.]);
        assert_eq!(rows(12.), vec![0., 30.]);
        // A clipped end bay and a narrow bay retain their complete web.
        assert_eq!(
            perforate(&[3., 21.], &[2., 12., 22.], o, 8., &mut 0, &cancel).unwrap(),
            vec![3., 21.]
        );
        assert!(
            perforate(
                &[0., 30.],
                &[2., 12., 22.],
                o,
                8.,
                &mut 0,
                &AtomicBool::new(true)
            )
            .is_err()
        );
    }
}
