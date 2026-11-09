// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Layer-native retained construction, not a load solver or removable support.
//! Continuous orthogonal webs use one physical lattice. Roof corbels grow from
//! those webs at 1 mm rise per 0.5 mm run; opposing tips merge once.
use super::*;
use spiling_contracts::manufacturing::program_coordinates::MIN_DEPOSITION_COMMAND_MM;

#[derive(Clone, Copy)]
struct Lattice {
    min: f64,
    max: f64,
    first: f64,
    pitch: f64,
    count: usize,
}
impl Lattice {
    fn new(min: f64, max: f64, pitch: f64) -> Result<Self, ManufacturingError> {
        let count = ((max - min) / pitch).ceil().max(1.0);
        if !count.is_finite() || count > MAX_DEPOSITION_SEGMENTS as f64 {
            return Err(resource("web + roof lattice exceeds the command budget"));
        }
        let first = min + ((max - min) - (count - 1.0) * pitch) * 0.5;
        Ok(Self {
            min,
            max,
            first,
            pitch,
            count: count as usize,
        })
    }
    fn root(self, index: usize) -> f64 {
        if index == 0 {
            self.min
        } else if index == self.count + 1 {
            self.max
        } else {
            self.first + (index - 1) as f64 * self.pitch
        }
    }
}

fn rectangle(polygon: &Polygon<f64>) -> Result<geo::Rect<f64>, ManufacturingError> {
    let bounds = polygon
        .bounding_rect()
        .ok_or_else(|| unsupported("web + roof has no interior bounds"))?;
    if !polygon.interiors().is_empty()
        || bounds.width() <= 0.0
        || bounds.height() <= 0.0
        || polygon.exterior().lines().any(|edge| {
            !((edge.start.x == edge.end.x
                && [bounds.min().x, bounds.max().x].contains(&edge.start.x))
                || (edge.start.y == edge.end.y
                    && [bounds.min().y, bounds.max().y].contains(&edge.start.y)))
        })
    {
        return Err(unsupported(
            "web + roof currently requires a rectangular interior without holes",
        ));
    }
    Ok(bounds)
}

pub(super) fn hatch(
    fill: &MultiPolygon<f64>,
    fraction: f64,
    layer: &HatchLayer<'_>,
    paths: &mut Vec<DepositionPath>,
    segments: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    if fill.0.is_empty() {
        return Ok(());
    }
    if fill.0.len() != 1 {
        return Err(unsupported(
            "web + roof currently requires one connected rectangular interior",
        ));
    }
    let roof = layer
        .roof_z_mm
        .ok_or_else(|| unsupported("web + roof lacks its physical roof horizon"))?;
    let polygon = &fill.0[0];
    let bounds = rectangle(polygon)?;
    // Freeze pitch at the captured nominal interval: refined physical heights
    // change bead area, never move a continuous web sideways.
    let nominal = layer.recipe.layer_height_mm;
    let spacing = layer.width_mm - nominal + std::f64::consts::PI * nominal / 4.0;
    let pitch = spacing * 2.0 / fraction;
    let lattices = [
        Lattice::new(bounds.min().x, bounds.max().x, pitch)?,
        Lattice::new(bounds.min().y, bounds.max().y, pitch)?,
    ];
    let mut proposals = 0usize;
    for (axis, fixed) in [(HatchAxis::X, 1), (HatchAxis::Y, 0)] {
        let active = HatchLayer { axis, ..*layer };
        let lattice = lattices[fixed];
        let rows = HatchRows {
            count: lattice.count,
            start: lattice.first,
            spacing: lattice.pitch,
            edges: polygon.exterior().0.len().saturating_sub(1),
        };
        hatch_polygon(
            polygon,
            DepositionKind::SparseFill,
            &rows,
            &active,
            paths,
            segments,
            cancel,
        )?;
        let along = lattices[1 - fixed];
        // Leave room for nominal bead/command effects; actual skin spans are
        // independently checked against the unchanged captured bridge limit.
        let target_gap = layer.recipe.process.max_bridge_span_mm * 0.75;
        for gap in 0..=lattice.count {
            check_cancel(cancel)?;
            let a = lattice.root(gap);
            let b = lattice.root(gap + 1);
            let divisions = ((b - a) / target_gap).ceil().max(1.0);
            if !divisions.is_finite() || divisions > MAX_DEPOSITION_SEGMENTS as f64 {
                return Err(resource("web + roof roof-contact budget exceeded"));
            }
            if divisions <= 1.0 {
                continue;
            }
            for root in 0..=along.count {
                check_cancel(cancel)?;
                proposals += 1;
                if proposals > MAX_DEPOSITION_SEGMENTS as usize {
                    return Err(resource("web + roof proposal work budget exceeded"));
                }
                let start = along.root(root);
                let stop = along.root(root + 1);
                let from_right = root == 0;
                let from_left = root == along.count;
                // Boundary chambers grow outward from an actual continuous
                // web, not inward from a nominal fill boundary. That boundary
                // is not an earlier deposited wall centerline certificate.
                let reach = (stop - start) * if from_right || from_left { 1.0 } else { 0.5 };
                let run = (reach - 0.5 * (roof - layer.z).max(0.0)).max(0.0);
                // A branch is an optional construction proposal until it has a
                // real command-length segment. Its first deposition and every
                // later extension still need earlier-layer actual support.
                if run < MIN_DEPOSITION_COMMAND_MM {
                    continue;
                }
                let additional = (divisions as usize - 1)
                    .checked_mul(if run >= reach || from_right || from_left {
                        1
                    } else {
                        2
                    })
                    .ok_or_else(|| resource("web + roof branch count overflow"))?;
                if additional > MAX_DEPOSITION_SEGMENTS as usize - *segments {
                    return Err(resource("web + roof branches exceed the command budget"));
                }
                for contact in 1..divisions as usize {
                    check_cancel(cancel)?;
                    let coordinate = a + (b - a) * contact as f64 / divisions;
                    let hits = if run >= reach {
                        [start, stop, stop, stop]
                    } else if from_right {
                        [stop - run, stop, stop, stop]
                    } else if from_left {
                        [start, start + run, stop, stop]
                    } else {
                        [start, start + run, stop - run, stop]
                    };
                    append_hatch_row(
                        if run >= reach || from_right || from_left {
                            &hits[..2]
                        } else {
                            &hits
                        },
                        coordinate,
                        DepositionKind::SparseFill,
                        &active,
                        (gap + root + contact) % 2 == 1,
                        paths,
                        segments,
                    )?;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn paths(z: f64, height: f64) -> Vec<DepositionPath> {
        let mut recipe = super::super::super::tests::recipe(0.08);
        recipe.sparse_infill = SparseInfillPolicy::WebRoof {};
        recipe.layer_height_mm = 0.2;
        recipe.bead_width_mm = 0.45;
        recipe.process.max_bridge_span_mm = 4.0;
        let layer = HatchLayer {
            roof_z_mm: Some(24.0),
            width_mm: 0.45,
            z,
            lower_z: z - height,
            height,
            axis: HatchAxis::X,
            top_axis: HatchAxis::X,
            recipe: &recipe,
        };
        let domain = MultiPolygon(vec![geo::Rect::new([0.0, 0.0], [24.0, 24.0]).to_polygon()]);
        let mut result = Vec::new();
        hatch(
            &domain,
            0.08,
            &layer,
            &mut result,
            &mut 0,
            &AtomicBool::new(false),
        )
        .unwrap();
        result
    }
    #[test]
    fn physical_roof_growth_keeps_webs_and_never_doubles_a_collinear_road() {
        let low = paths(1.0, 0.2);
        let middle = paths(21.0, 0.2);
        let roof = paths(24.0, 0.2);
        assert!(low.len() < middle.len());
        assert!(
            roof.iter()
                .map(|p| p
                    .checked_edges()
                    .unwrap()
                    .map(|e| (e.points_mm[1][0] - e.points_mm[0][0])
                        .hypot(e.points_mm[1][1] - e.points_mm[0][1]))
                    .sum::<f64>())
                .sum::<f64>()
                > 24.0 * low.len() as f64
        );
        for path in &low {
            assert!(middle.iter().any(|p| {
                p.points_mm
                    .iter()
                    .map(|v| [v[0], v[1]])
                    .eq(path.points_mm.iter().map(|v| [v[0], v[1]]))
            }));
        }
        for set in [&middle, &roof] {
            for (index, a) in set.iter().enumerate() {
                for b in &set[index + 1..] {
                    for fixed in 0..2 {
                        if a.points_mm[0][fixed] == a.points_mm[1][fixed]
                            && b.points_mm[0][fixed] == b.points_mm[1][fixed]
                            && a.points_mm[0][fixed] == b.points_mm[0][fixed]
                        {
                            let along = 1 - fixed;
                            let min = |p: &DepositionPath| {
                                p.points_mm[0][along].min(p.points_mm[1][along])
                            };
                            let max = |p: &DepositionPath| {
                                p.points_mm[0][along].max(p.points_mm[1][along])
                            };
                            assert!(min(a).max(min(b)) >= max(a).min(max(b)));
                        }
                    }
                }
            }
        }
        // Same world height, differently refined physical interval: stable XY.
        let fine = paths(21.0, 0.1);
        assert_eq!(
            middle.iter().map(|p| &p.points_mm).collect::<Vec<_>>(),
            fine.iter().map(|p| &p.points_mm).collect::<Vec<_>>()
        );
    }
    #[test]
    fn roof_contacts_and_cancellation_are_bounded_before_large_growth() {
        let mut recipe = super::super::super::tests::recipe(0.08);
        recipe.sparse_infill = SparseInfillPolicy::WebRoof {};
        recipe.process.max_bridge_span_mm = 1e-12;
        let layer = HatchLayer {
            roof_z_mm: Some(24.0),
            width_mm: 0.45,
            z: 24.0,
            lower_z: 23.8,
            height: 0.2,
            axis: HatchAxis::X,
            top_axis: HatchAxis::X,
            recipe: &recipe,
        };
        let domain = MultiPolygon(vec![geo::Rect::new([0.0, 0.0], [24.0, 24.0]).to_polygon()]);
        let mut paths = Vec::new();
        let error = hatch(
            &domain,
            0.08,
            &layer,
            &mut paths,
            &mut 0,
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(error.code, ManufacturingErrorCode::ResourceLimit);
        assert!(paths.len() < 100);
        let error = hatch(
            &domain,
            0.08,
            &layer,
            &mut Vec::new(),
            &mut 0,
            &AtomicBool::new(true),
        )
        .unwrap_err();
        assert_eq!(error.code, ManufacturingErrorCode::Cancelled);
    }
}
