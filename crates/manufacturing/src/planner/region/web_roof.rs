// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Layer-native retained construction, not a load solver or removable support.
//! Continuous orthogonal webs use one physical lattice. Roof corbels grow from
//! those webs at 1 mm rise per 0.5 mm run; opposing tips merge once.
//! Every component/hole chord is rooted independently, without wall-root credit.
use super::*;
use spiling_contracts::manufacturing::program_coordinates::MIN_DEPOSITION_COMMAND_MM;
mod architecture;
mod columns;
mod joints;
pub(in crate::planner) use columns::{decode, synthesize};

struct HatchMemory {
    outside: usize,
    nested: usize,
}

impl HatchMemory {
    fn new(paths: &Vec<DepositionPath>, outside: usize) -> Self {
        Self {
            outside,
            nested: super::super::bonds::path_bytes(paths)
                .saturating_sub(paths.capacity() * std::mem::size_of::<DepositionPath>()),
        }
    }

    fn append(
        &mut self,
        hits: &[f64],
        coordinate: f64,
        layer: &HatchLayer<'_>,
        reverse: bool,
        paths: &mut Vec<DepositionPath>,
        segments: &mut usize,
    ) -> Result<(), ManufacturingError> {
        let count = hits.len() / 2;
        let required = paths.len().saturating_add(count);
        let capacity = if required > paths.capacity() {
            paths.capacity().saturating_mul(2).max(4).max(required)
        } else {
            paths.capacity()
        };
        let metadata = capacity
            .saturating_add(if required > paths.capacity() {
                paths.capacity()
            } else {
                0
            })
            .saturating_mul(std::mem::size_of::<DepositionPath>());
        super::super::remaining_geometry_units(
            0,
            self.outside
                .saturating_add(self.nested)
                .saturating_add(metadata)
                .saturating_add(count.saturating_mul(2 * std::mem::size_of::<[f64; 3]>())),
        )?;
        let before = paths.len();
        append_hatch_row(
            hits,
            coordinate,
            DepositionKind::SparseFill,
            layer,
            reverse,
            paths,
            segments,
        )?;
        self.nested = self.nested.saturating_add(
            (paths.len() - before).saturating_mul(2 * std::mem::size_of::<[f64; 3]>()),
        );
        Ok(())
    }
}

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
        if !pitch.is_finite() || pitch <= 0.0 || !min.is_finite() || !max.is_finite() || min >= max
        {
            return Err(unsupported(
                "web + roof lattice has unconditioned bounds or pitch",
            ));
        }
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

fn charge(work: &mut usize, amount: usize, cancel: &AtomicBool) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    *work = work
        .checked_add(amount)
        .ok_or_else(|| resource("web + roof clipping work budget exceeded"))?;
    Ok(())
}

/// Seed long clipped roof chords that the regular lattice misses. Seeds are
/// full-height webs, chosen before any roof growth; no wall/void is a root.
fn seed_webs(
    polygon: &Polygon<f64>,
    lattices: [Lattice; 2],
    layer: &HatchLayer<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<[Vec<f64>; 2], ManufacturingError> {
    if lattices[0].count.saturating_add(lattices[1].count) > MAX_DEPOSITION_SEGMENTS as usize {
        return Err(resource("web + roof base lattice budget exceeded"));
    }
    let mut roots = lattices.map(|l| {
        (0..l.count)
            .map(|i| l.first + i as f64 * l.pitch)
            .collect::<Vec<_>>()
    });
    let edges = std::iter::once(polygon.exterior())
        .chain(polygon.interiors())
        .map(|r| r.0.len() - 1)
        .sum::<usize>();
    let mut hits = Vec::new();
    for fixed in 0..2 {
        let l = lattices[fixed];
        let axis = if fixed == 0 {
            HatchAxis::Y
        } else {
            HatchAxis::X
        };
        for gap in 0..=l.count {
            let a = l.root(gap);
            let b = l.root(gap + 1);
            let divisions = ((b - a) / layer.recipe.web_roof_spacing_mm())
                .ceil()
                .max(1.);
            if !divisions.is_finite() || divisions > MAX_DEPOSITION_SEGMENTS as f64 {
                return Err(resource("web + roof seed contact budget exceeded"));
            }
            for contact in 1..divisions as usize {
                charge(work, edges, cancel)?;
                let c = a + (b - a) * contact as f64 / divisions;
                hits.clear();
                row_intersections(polygon, c, axis, &mut hits, cancel)?;
                for chord in hits.as_chunks::<2>().0 {
                    charge(work, roots[1 - fixed].len(), cancel)?;
                    if chord[1] - chord[0] > layer.recipe.web_roof_spacing_mm()
                        && !roots[1 - fixed]
                            .iter()
                            .any(|&v| chord[0] <= v && v <= chord[1])
                    {
                        let seed = (chord[0] + chord[1]) * 0.5;
                        // A web within a bead of the midpoint is already the
                        // support; the leftover gap is an ordinary short bridge.
                        if roots[1 - fixed]
                            .iter()
                            .any(|v| (seed - v).abs() < layer.width_mm)
                        {
                            continue;
                        }
                        if roots[0].len() + roots[1].len() >= MAX_DEPOSITION_SEGMENTS as usize {
                            return Err(resource("web + roof local seed budget exceeded"));
                        }
                        roots[1 - fixed].push(seed);
                        roots[1 - fixed].sort_unstable_by(f64::total_cmp);
                    }
                }
            }
        }
    }
    Ok(roots)
}

/// How far a corbel extends from `root` toward `limit` at height `z`. It grows
/// 0.5 mm per millimetre of rise and must reach every point of its catchment
/// by that point's own ceiling: the part roof, or a lower ceiling over it.
fn reach(root: f64, limit: f64, roof_z: f64, z: f64, lowered: &[[f64; 3]]) -> f64 {
    let (lo, hi) = (root.min(limit), root.max(limit));
    let span = hi - lo;
    let mut extent = (span - 0.5 * (roof_z - z).max(0.0)).max(0.0);
    for &[a, b, ceiling_z] in lowered {
        let (a, b) = (a.max(lo), b.min(hi));
        if b <= a {
            continue;
        }
        let far = if limit < root { root - a } else { b - root };
        extent = extent.max(far - 0.5 * (ceiling_z - z).max(0.0));
    }
    extent.min(span)
}

pub(in crate::planner) fn hatch(
    fill: &MultiPolygon<f64>,
    fraction: f64,
    layer: &HatchLayer<'_>,
    paths: &mut Vec<DepositionPath>,
    segments: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    hatch_with_budget(fill, fraction, layer, paths, segments, cancel, 0)
}

pub(in crate::planner) fn hatch_with_budget(
    fill: &MultiPolygon<f64>,
    fraction: f64,
    layer: &HatchLayer<'_>,
    paths: &mut Vec<DepositionPath>,
    segments: &mut usize,
    cancel: &AtomicBool,
    owned_bytes: usize,
) -> Result<(), ManufacturingError> {
    let first_path = paths.len();
    let mut memory = HatchMemory::new(paths, owned_bytes);
    let roof = layer
        .roof
        .ok_or_else(|| unsupported("web + roof lacks its physical roof horizon"))?;
    let nominal = layer.recipe.layer_height_mm;
    let pitch = 2.0 * (layer.width_mm - nominal + std::f64::consts::PI * nominal / 4.0) / fraction;
    let mut work = 0usize;
    let mut proposals = 0usize;
    let mut hits = Vec::new();
    let mut roots = Vec::new();
    let mut lowered: Vec<[f64; 3]> = Vec::new();
    // One part-level lattice: webs keep their XY through changing sections.
    let lattices = [
        Lattice::new(roof.frame_mm[0][0], roof.frame_mm[1][0], pitch)?,
        Lattice::new(roof.frame_mm[0][1], roof.frame_mm[1][1], pitch)?,
    ];
    for polygon in &fill.0 {
        check_cancel(cancel)?;
        let local_roots;
        let web_roots = if let Some(columns) = roof.columns {
            &columns.roots
        } else {
            local_roots = seed_webs(polygon, lattices, layer, &mut work, cancel)?;
            &local_roots
        };
        let architecture = match layer.recipe.sparse_infill {
            SparseInfillPolicy::WebRoof { architecture, .. } => architecture,
            _ => None,
        };
        if let Some(a) = architecture {
            architecture::admit(a, web_roots, &lattices, roof.z_mm, layer)?;
        }
        let edges = std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .map(|ring| ring.0.len().saturating_sub(1))
            .sum::<usize>();
        for (axis, fixed) in [(HatchAxis::X, 1), (HatchAxis::Y, 0)] {
            let active = HatchLayer { axis, ..*layer };
            let lattice = lattices[fixed];
            for &coordinate in &web_roots[fixed] {
                charge(&mut work, edges, cancel)?;
                hits.clear();
                row_intersections(polygon, coordinate, axis, &mut hits, cancel)?;
                if let Some(opening) = architecture.and_then(|a| a.opening) {
                    hits = architecture::perforate(
                        &hits,
                        &web_roots[1 - fixed],
                        opening,
                        layer.z,
                        &mut work,
                        cancel,
                    )?;
                }
                memory.append(&hits, coordinate, &active, false, paths, segments)?;
            }
            for gap in 0..=lattice.count {
                charge(&mut work, 1, cancel)?;
                let a = lattice.root(gap);
                let b = lattice.root(gap + 1);
                let divisions = ((b - a) / layer.recipe.web_roof_spacing_mm())
                    .ceil()
                    .max(1.0);
                if !divisions.is_finite() || divisions > MAX_DEPOSITION_SEGMENTS as f64 {
                    return Err(resource("web + roof roof-contact budget exceeded"));
                }
                for contact in 1..divisions as usize {
                    charge(&mut work, edges, cancel)?;
                    let coordinate = a + (b - a) * contact as f64 / divisions;
                    if web_roots[fixed].contains(&coordinate) {
                        continue;
                    }
                    // Lower ceilings crossing this roof row: [from, to, roof Z].
                    // A span thinner than a bead is a wall step the wall
                    // itself carries.
                    lowered.clear();
                    for ceiling in roof.ceilings {
                        for area in &ceiling.area.0 {
                            charge(&mut work, area.exterior().0.len(), cancel)?;
                            hits.clear();
                            row_intersections(area, coordinate, axis, &mut hits, cancel)?;
                            for span in hits.as_chunks::<2>().0 {
                                if span[1] - span[0] >= layer.width_mm {
                                    lowered.push([span[0], span[1], ceiling.roof_z_mm]);
                                }
                            }
                        }
                    }
                    hits.clear();
                    row_intersections(polygon, coordinate, axis, &mut hits, cancel)?;
                    for chord in hits.as_chunks::<2>().0 {
                        charge(&mut work, web_roots[1 - fixed].len(), cancel)?;
                        roots.clear();
                        roots.extend(
                            web_roots[1 - fixed]
                                .iter()
                                .copied()
                                .filter(|v| *v >= chord[0] && *v <= chord[1]),
                        );
                        // A clipped chamber may have no perpendicular web.
                        // It cannot invent a wall root. Ordinary independent
                        // skin support must still qualify its real short span.
                        let mut pending: Option<[f64; 2]> = None;
                        for (index, &root) in roots.iter().enumerate() {
                            proposals += 1;
                            if proposals > MAX_DEPOSITION_SEGMENTS as usize {
                                return Err(resource("web + roof proposal budget exceeded"));
                            }
                            charge(&mut work, 1, cancel)?;
                            let left = if index == 0 {
                                chord[0]
                            } else {
                                (roots[index - 1] + root) * 0.5
                            };
                            let right = if index + 1 == roots.len() {
                                chord[1]
                            } else {
                                (root + roots[index + 1]) * 0.5
                            };
                            charge(&mut work, lowered.len(), cancel)?;
                            let start = root - reach(root, left, roof.z_mm, layer.z, &lowered);
                            let stop = root + reach(root, right, roof.z_mm, layer.z, &lowered);
                            if stop - start < MIN_DEPOSITION_COMMAND_MM {
                                continue;
                            }
                            if let Some(previous) = &mut pending {
                                if start <= previous[1] {
                                    previous[1] = previous[1].max(stop);
                                    continue;
                                }
                                memory.append(
                                    previous,
                                    coordinate,
                                    &active,
                                    contact % 2 == 1,
                                    paths,
                                    segments,
                                )?;
                            }
                            pending = Some([start, stop]);
                        }
                        if let Some(interval) = pending {
                            memory.append(
                                &interval,
                                coordinate,
                                &active,
                                contact % 2 == 1,
                                paths,
                                segments,
                            )?;
                        }
                    }
                }
            }
        }
        if let Some(depth) = architecture.and_then(|a| a.pier_rib_depth_mm) {
            architecture::ribs(
                polygon, web_roots, depth, layer, paths, segments, &mut work, cancel,
            )?;
        } else if let Some(columns) = roof.columns {
            for &(fixed, coordinate, start, stop) in &columns.joints {
                charge(&mut work, edges, cancel)?;
                let active = HatchLayer {
                    axis: if fixed == 0 {
                        HatchAxis::Y
                    } else {
                        HatchAxis::X
                    },
                    ..*layer
                };
                hits.clear();
                row_intersections(polygon, coordinate, active.axis, &mut hits, cancel)?;
                for chord in hits.as_chunks::<2>().0 {
                    let interval = [start.max(chord[0]), stop.min(chord[1])];
                    if interval[1] - interval[0] >= MIN_DEPOSITION_COMMAND_MM {
                        memory.append(&interval, coordinate, &active, false, paths, segments)?;
                    }
                }
            }
        } else if architecture.is_none() {
            joints::append(
                polygon,
                web_roots,
                &lattices,
                layer,
                paths,
                segments,
                &mut work,
                cancel,
                &mut memory,
            )?;
        }
    }
    super::super::remaining_geometry_units(
        0,
        owned_bytes.saturating_add(super::super::bonds::path_bytes(paths).saturating_mul(3)),
    )?;
    architecture::merge(paths, first_path, layer, segments, &mut work, cancel)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    /// A flat part roof with the lattice framed on `fill`, as a prism has.
    pub(in crate::planner) fn flat(z_mm: f64, fill: &MultiPolygon<f64>) -> Roof<'static> {
        let bounds = fill.bounding_rect().unwrap();
        Roof {
            z_mm,
            frame_mm: [
                [bounds.min().x, bounds.min().y],
                [bounds.max().x, bounds.max().y],
            ],
            ceilings: &[],
            columns: None,
        }
    }
    fn paths(z: f64, height: f64) -> Vec<DepositionPath> {
        let mut recipe = super::super::super::tests::recipe(0.08);
        recipe.sparse_infill = SparseInfillPolicy::web_roof();
        recipe.layer_height_mm = 0.2;
        recipe.bead_width_mm = 0.45;
        recipe.process.max_bridge_span_mm = 4.0;
        let layer = HatchLayer {
            roof: Some(flat(
                24.0,
                &MultiPolygon(vec![geo::Rect::new([0.0, 0.0], [24.0, 24.0]).to_polygon()]),
            )),
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
    fn hatch_memory_refuses_before_staging_an_unfunded_road() {
        let recipe = super::super::super::tests::recipe(0.08);
        let layer = HatchLayer {
            roof: None,
            width_mm: 0.45,
            z: 0.2,
            lower_z: 0.0,
            height: 0.2,
            axis: HatchAxis::X,
            top_axis: HatchAxis::X,
            recipe: &recipe,
        };
        let mut paths = Vec::new();
        let mut segments = 0;
        let mut memory = HatchMemory::new(&paths, crate::working_set::bytes() - 1);
        let failure = memory
            .append(&[0.0, 10.0], 0.0, &layer, false, &mut paths, &mut segments)
            .unwrap_err();
        assert_eq!(failure.code, ManufacturingErrorCode::ResourceLimit);
        assert!(paths.is_empty());
        assert_eq!(segments, 0);
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
        recipe.sparse_infill = SparseInfillPolicy::web_roof();
        recipe.process.max_bridge_span_mm = 1e-12;
        let layer = HatchLayer {
            roof: Some(flat(
                24.0,
                &MultiPolygon(vec![geo::Rect::new([0.0, 0.0], [24.0, 24.0]).to_polygon()]),
            )),
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
    #[test]
    fn clipped_holes_concavity_and_disconnected_rooms_keep_roads_inside_each_component() {
        use geo::{Line, LineString, Relate};
        let outer = geo::Rect::new([0., 0.], [24., 24.])
            .to_polygon()
            .exterior()
            .clone();
        let holed = Polygon::new(
            outer,
            vec![
                geo::Rect::new([8., 8.], [16., 16.])
                    .to_polygon()
                    .exterior()
                    .clone(),
            ],
        );
        let concave = Polygon::new(
            LineString::from(vec![
                (30., 0.),
                (54., 0.),
                (54., 8.),
                (38., 8.),
                (38., 24.),
                (30., 24.),
                (30., 0.),
            ]),
            vec![],
        );
        let fill = MultiPolygon(vec![holed, concave]);
        for fraction in [0.03, 0.08, 0.16, 0.35] {
            let mut recipe = super::super::super::tests::recipe(fraction);
            recipe.sparse_infill = SparseInfillPolicy::web_roof();
            recipe.layer_height_mm = 0.2;
            recipe.bead_width_mm = 0.45;
            recipe.process.max_bridge_span_mm = 4.;
            for z in [1., 20., 23., 24.] {
                let layer = HatchLayer {
                    roof: Some(flat(24., &fill)),
                    width_mm: 0.45,
                    z,
                    lower_z: z - 0.2,
                    height: 0.2,
                    axis: HatchAxis::X,
                    top_axis: HatchAxis::X,
                    recipe: &recipe,
                };
                let mut paths = Vec::new();
                hatch(
                    &fill,
                    fraction,
                    &layer,
                    &mut paths,
                    &mut 0,
                    &AtomicBool::new(false),
                )
                .unwrap();
                assert!(!paths.is_empty());
                for path in &paths {
                    let a = path.points_mm[0];
                    let b = path.points_mm[1];
                    assert!(
                        fill.relate(&Line::new([a[0], a[1]], [b[0], b[1]]))
                            .is_covers(),
                        "{a:?} to {b:?}"
                    );
                }
                for (index, a) in paths.iter().enumerate() {
                    for b in &paths[index + 1..] {
                        for fixed in 0..2 {
                            if a.points_mm[0][fixed] == a.points_mm[1][fixed]
                                && b.points_mm[0][fixed] == b.points_mm[1][fixed]
                                && a.points_mm[0][fixed] == b.points_mm[0][fixed]
                            {
                                let along = 1 - fixed;
                                let lo = |p: &DepositionPath| {
                                    p.points_mm[0][along].min(p.points_mm[1][along])
                                };
                                let hi = |p: &DepositionPath| {
                                    p.points_mm[0][along].max(p.points_mm[1][along])
                                };
                                assert!(
                                    lo(a).max(lo(b)) >= hi(a).min(hi(b)),
                                    "duplicate collinear dose"
                                );
                            }
                        }
                    }
                }
            }
        }
    }
}
