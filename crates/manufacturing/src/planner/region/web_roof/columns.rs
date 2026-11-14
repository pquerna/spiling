// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;

pub(in crate::planner) fn decode(polygon: &DiaphragmPolygonMm) -> Polygon<f64> {
    Polygon::new(
        polygon
            .outer
            .iter()
            .map(|point| (point[0], point[1]))
            .collect(),
        polygon
            .holes
            .iter()
            .map(|ring| ring.iter().map(|point| (point[0], point[1])).collect())
            .collect(),
    )
}

pub(in crate::planner) fn synthesize(
    layers: &[PrintLayer],
    recipe: &PlanarPrintRecipe,
    owned_bytes: usize,
    cancel: &AtomicBool,
) -> Result<Option<WebColumns>, ManufacturingError> {
    if !matches!(
        recipe.sparse_infill,
        SparseInfillPolicy::WebRoof {
            architecture: None,
            ..
        }
    ) {
        return Ok(None);
    }
    let Some(frame) = layers
        .iter()
        .flat_map(|layer| &layer.region_coverage.web_roof_domains)
        .next()
        .map(|domain| domain.frame_mm)
    else {
        return Ok(None);
    };
    let pitch = 2.0
        * (recipe.bead_width_mm - recipe.layer_height_mm
            + std::f64::consts::PI * recipe.layer_height_mm / 4.0)
        / recipe.infill_fraction;
    let lattices = [
        Lattice::new(frame[0][0], frame[1][0], pitch)?,
        Lattice::new(frame[0][1], frame[1][1], pitch)?,
    ];
    let base_count = lattices[0].count.saturating_add(lattices[1].count);
    if base_count > crate::working_set::units() {
        return Err(resource("Cathedral base column inventory exceeded"));
    }
    let mut work = 0;
    let mut bounds = frame;
    for point in layers
        .iter()
        .flat_map(|layer| &layer.region_coverage.web_roof_domains)
        .flat_map(|domain| &domain.extent)
        .flat_map(|polygon| std::iter::once(&polygon.outer).chain(&polygon.holes))
        .flatten()
    {
        charge(&mut work, 1, cancel)?;
        for axis in 0..2 {
            bounds[0][axis] = bounds[0][axis].min(point[axis]);
            bounds[1][axis] = bounds[1][axis].max(point[axis]);
        }
    }
    let seed_limit = base_count
        .saturating_add(
            (0..2)
                .map(|axis| {
                    (((bounds[1][axis] - bounds[0][axis]) / recipe.bead_width_mm)
                        .ceil()
                        .max(0.0)
                        .min(crate::working_set::units() as f64)) as usize
                        + 2
                })
                .sum::<usize>(),
        )
        .min(crate::working_set::units());
    let roots_budget = seed_limit.saturating_mul(2 * std::mem::size_of::<f64>());
    super::super::super::remaining_geometry_units(
        0,
        owned_bytes.saturating_add(base_count.saturating_mul(2 * std::mem::size_of::<f64>())),
    )?;
    let mut columns = WebColumns {
        roots: lattices.map(|lattice| {
            (0..lattice.count)
                .map(|index| lattice.first + index as f64 * lattice.pitch)
                .collect()
        }),
        joints: Vec::new(),
    };
    let mut paths = Vec::new();
    for joints in [false, true] {
        for layer in layers.iter().rev() {
            let hatch = HatchLayer {
                roof: Some(Roof {
                    z_mm: layer.z_mm,
                    frame_mm: frame,
                    ceilings: &[],
                    columns: None,
                }),
                width_mm: recipe.bead_width_mm,
                z: layer.z_mm,
                lower_z: layer.lower_z_mm,
                height: layer.height_mm,
                axis: HatchAxis::X,
                top_axis: HatchAxis::X,
                recipe,
            };
            for domain in &layer.region_coverage.web_roof_domains {
                if domain.frame_mm != frame {
                    return Err(unsupported(
                        "Cathedral column inventory requires one part-level frame",
                    ));
                }
                for extent in &domain.extent {
                    let points =
                        extent.outer.len() + extent.holes.iter().map(Vec::len).sum::<usize>();
                    let scratch = points
                        .saturating_mul(5 * std::mem::size_of::<geo::Coord<f64>>())
                        .saturating_add(
                            extent.holes.len() * std::mem::size_of::<geo::LineString<f64>>(),
                        );
                    super::super::super::remaining_geometry_units(
                        0,
                        owned_bytes
                            .saturating_add(columns.allocated_bytes())
                            .saturating_add(scratch)
                            .saturating_add(roots_budget)
                            .saturating_add(super::super::super::bonds::path_bytes(&paths)),
                    )?;
                    let polygon = decode(extent);
                    if joints {
                        paths.clear();
                        let mut memory = HatchMemory::new(
                            &paths,
                            owned_bytes
                                .saturating_add(scratch)
                                .saturating_add(columns.allocated_bytes()),
                        );
                        joints::append(
                            &polygon,
                            &columns.roots,
                            &lattices,
                            &hatch,
                            &mut paths,
                            &mut 0,
                            &mut work,
                            cancel,
                            &mut memory,
                        )?;
                        let paths_bytes = super::super::super::bonds::path_bytes(&paths);
                        for path in &paths {
                            let [start, stop] = [path.points_mm[0], path.points_mm[1]];
                            let fixed = usize::from(start[0] != stop[0]);
                            let along = 1 - fixed;
                            let proposal = (
                                fixed,
                                start[fixed],
                                start[along].min(stop[along]),
                                start[along].max(stop[along]),
                            );
                            charge(
                                &mut work,
                                columns.joints.len().max(1).ilog2().saturating_add(1) as usize,
                                cancel,
                            )?;
                            match columns
                                .joints
                                .binary_search_by(|existing| compare(*existing, proposal))
                            {
                                Ok(_) => {}
                                Err(index) => {
                                    super::super::super::remaining_geometry_units(
                                        0,
                                        owned_bytes
                                            .saturating_add(scratch)
                                            .saturating_add(paths_bytes)
                                            .saturating_add(
                                                columns.allocated_bytes().saturating_mul(3),
                                            )
                                            .saturating_add(256),
                                    )?;
                                    charge(&mut work, columns.joints.len() - index, cancel)?;
                                    if columns.joints.len() >= crate::working_set::units() {
                                        return Err(resource(
                                            "Cathedral joint column budget exceeded",
                                        ));
                                    }
                                    columns.joints.insert(index, proposal);
                                }
                            }
                        }
                    } else {
                        let local = seed_webs(&polygon, lattices, &hatch, &mut work, cancel)?;
                        let mut root_count = columns.roots[0].len() + columns.roots[1].len();
                        for (axis, proposals) in local.into_iter().enumerate() {
                            let other_bytes = columns.roots[1 - axis].capacity()
                                * std::mem::size_of::<f64>()
                                + columns.joints.capacity()
                                    * std::mem::size_of::<(usize, f64, f64, f64)>();
                            let roots = &mut columns.roots[axis];
                            for coordinate in proposals {
                                charge(
                                    &mut work,
                                    roots.len().ilog2().saturating_add(1) as usize,
                                    cancel,
                                )?;
                                let index = roots.partition_point(|root| *root < coordinate);
                                if roots.get(index).is_some_and(|root| {
                                    (root - coordinate).abs() < recipe.bead_width_mm
                                }) || index.checked_sub(1).is_some_and(|previous| {
                                    (roots[previous] - coordinate).abs() < recipe.bead_width_mm
                                }) {
                                    continue;
                                }
                                if root_count >= crate::working_set::units() {
                                    return Err(resource("Cathedral seed column budget exceeded"));
                                }
                                charge(&mut work, roots.len() - index, cancel)?;
                                super::super::super::remaining_geometry_units(
                                    0,
                                    owned_bytes
                                        .saturating_add(scratch)
                                        .saturating_add(roots_budget)
                                        .saturating_add(other_bytes)
                                        .saturating_add(
                                            roots
                                                .capacity()
                                                .saturating_mul(3 * std::mem::size_of::<f64>()),
                                        )
                                        .saturating_add(256),
                                )?;
                                roots.insert(index, coordinate);
                                root_count += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    super::super::super::remaining_geometry_units(
        0,
        owned_bytes.saturating_add(columns.allocated_bytes()),
    )?;
    Ok(Some(columns))
}

fn compare(left: (usize, f64, f64, f64), right: (usize, f64, f64, f64)) -> std::cmp::Ordering {
    left.0
        .cmp(&right.0)
        .then_with(|| left.1.total_cmp(&right.1))
        .then_with(|| left.2.total_cmp(&right.2))
        .then_with(|| left.3.total_cmp(&right.3))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(index: u32, polygon: Polygon<f64>, frame: [[f64; 2]; 2]) -> PrintLayer {
        let mut coverage = crate::planner::empty_coverage(0.08);
        coverage.web_roof_domains.push(WebRoofDomain {
            region_index: 0,
            frame_mm: frame,
            ceilings: Vec::new(),
            extent: vec![DiaphragmPolygonMm {
                outer: polygon
                    .exterior()
                    .0
                    .iter()
                    .map(|point| [point.x, point.y])
                    .collect(),
                holes: polygon
                    .interiors()
                    .iter()
                    .map(|ring| ring.0.iter().map(|point| [point.x, point.y]).collect())
                    .collect(),
            }],
        });
        PrintLayer {
            index,
            hatch_course: index,
            lower_z_mm: f64::from(index) * 0.2,
            z_mm: f64::from(index + 1) * 0.2,
            height_mm: 0.2,
            section_z_mm: f64::from(index) * 0.2 + 0.1,
            paths: Vec::new(),
            skin_command_fits: Vec::new(),
            region_coverage: coverage,
        }
    }

    fn roads(
        polygon: Polygon<f64>,
        columns: &WebColumns,
        frame: [[f64; 2]; 2],
        recipe: &PlanarPrintRecipe,
    ) -> Vec<DepositionPath> {
        let mut paths = Vec::new();
        hatch(
            &MultiPolygon(vec![polygon]),
            recipe.infill_fraction,
            &HatchLayer {
                roof: Some(Roof {
                    z_mm: 100.0,
                    frame_mm: frame,
                    ceilings: &[],
                    columns: Some(columns),
                }),
                z: 0.2,
                lower_z: 0.0,
                height: 0.2,
                width_mm: recipe.bead_width_mm,
                axis: HatchAxis::X,
                top_axis: HatchAxis::X,
                recipe,
            },
            &mut paths,
            &mut 0,
            &AtomicBool::new(false),
        )
        .unwrap();
        paths
    }

    #[test]
    fn future_seed_columns_reach_the_lower_interior_without_crossing_its_hole() {
        let mut recipe = crate::planner::tests::recipe(0.08);
        recipe.sparse_infill = SparseInfillPolicy::web_roof();
        recipe.layer_height_mm = 0.2;
        recipe.bead_width_mm = 0.45;
        let frame = [[0.0, 0.0], [40.0, 40.0]];
        let outer = geo::Rect::new(frame[0], frame[1]).to_polygon();
        let hole = geo::Rect::new([9.0, 15.0], [10.5, 25.0]).to_polygon();
        let lower = Polygon::new(outer.exterior().clone(), vec![hole.exterior().clone()]);
        let upper = geo::Rect::new([5.5, 0.0], [14.0, 40.0]).to_polygon();
        let layers = [layer(0, lower.clone(), frame), layer(1, upper, frame)];
        let columns = synthesize(&layers, &recipe, 0, &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert!(columns.roots[0].contains(&9.75));
        let seeded: Vec<_> = roads(lower, &columns, frame, &recipe)
            .into_iter()
            .filter(|path| path.points_mm.iter().all(|point| point[0] == 9.75))
            .collect();
        assert_eq!(seeded.len(), 2);
        assert!(seeded.iter().all(|path| {
            let start = path.points_mm[0][1].min(path.points_mm[1][1]);
            let stop = path.points_mm[0][1].max(path.points_mm[1][1]);
            stop <= 15.0 || start >= 25.0
        }));
        for roots in &columns.roots {
            assert!(
                roots
                    .windows(2)
                    .all(|pair| pair[1] - pair[0] >= recipe.bead_width_mm)
            );
        }
    }

    #[test]
    fn future_joint_columns_begin_as_clipped_roads_instead_of_full_floating_flanges() {
        let mut recipe = crate::planner::tests::recipe(0.08);
        recipe.sparse_infill = SparseInfillPolicy::web_roof();
        recipe.layer_height_mm = 0.2;
        recipe.bead_width_mm = 0.45;
        recipe.infill_fraction = 2.0
            * (recipe.bead_width_mm - recipe.layer_height_mm
                + std::f64::consts::PI * recipe.layer_height_mm / 4.0)
            / 16.0;
        let frame = [[0.0, 0.0], [48.0, 48.0]];
        let lower = geo::Rect::new([0.0, 0.0], [23.5, 48.0]).to_polygon();
        let upper = geo::Rect::new(frame[0], frame[1]).to_polygon();
        let layers = [layer(0, lower.clone(), frame), layer(1, upper, frame)];
        let columns = synthesize(&layers, &recipe, 0, &AtomicBool::new(false))
            .unwrap()
            .unwrap();
        assert!(
            columns
                .joints
                .iter()
                .any(|&(fixed, coordinate, start, stop)| fixed == 1
                    && (coordinate - 8.45).abs() < 1e-9
                    && (start - 22.65).abs() < 1e-9
                    && (stop - 25.35).abs() < 1e-9)
        );
        assert!(roads(lower, &columns, frame, &recipe).iter().any(|path| {
            path.points_mm
                .iter()
                .all(|point| (point[1] - 8.45).abs() < 1e-9)
                && (path.points_mm[0][0].min(path.points_mm[1][0]) - 22.65).abs() < 1e-9
                && (path.points_mm[0][0].max(path.points_mm[1][0]) - 23.5).abs() < 1e-9
        }));
    }

    #[test]
    fn column_construction_retains_cancellation_and_live_byte_limits() {
        let mut recipe = crate::planner::tests::recipe(0.08);
        recipe.sparse_infill = SparseInfillPolicy::web_roof();
        let frame = [[0.0, 0.0], [40.0, 40.0]];
        let layers = [layer(
            0,
            geo::Rect::new(frame[0], frame[1]).to_polygon(),
            frame,
        )];
        assert!(synthesize(&layers, &recipe, 0, &AtomicBool::new(true)).is_err());
        assert_eq!(
            synthesize(
                &layers,
                &recipe,
                crate::working_set::bytes(),
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .code,
            ManufacturingErrorCode::ResourceLimit
        );
    }
}
