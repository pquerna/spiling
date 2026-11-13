use super::{cancelled, charge, failed, material, rectangle, rows, walls};
use geo::{Area, BooleanOps, BoundingRect, MultiPolygon};
use spiling_contracts::manufacturing::*;
use std::{mem::size_of, sync::atomic::AtomicBool};

struct Residual<'a> {
    paths: Vec<&'a DepositionPath>,
    fill: MultiPolygon<f64>,
}

fn same_walls(
    earlier: &[&DepositionPath],
    current: &[&DepositionPath],
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    if earlier.len() != current.len() {
        return Ok(false);
    }
    let width = |path: &DepositionPath, index: usize| match &path.width {
        WidthProfile::Constant { width_mm } => *width_mm,
        WidthProfile::PerEdge { widths_mm } => widths_mm[index],
    };
    for (earlier, current) in earlier.iter().zip(current) {
        cancelled(cancel)?;
        let count = current.checked_edges()?.len();
        if count == 0
            || earlier.checked_edges()?.len() != count
            || earlier.kind != DepositionKind::Perimeter
            || current.kind != DepositionKind::Perimeter
            || current.owner != Some(DepositionOwner::Wall)
            || current.material_domain != earlier.material_domain
            || current.points_mm.first().map(|point| &point[..2])
                != current.points_mm.last().map(|point| &point[..2])
            || earlier.points_mm.first().map(|point| &point[..2])
                != earlier.points_mm.last().map(|point| &point[..2])
        {
            return Ok(false);
        }
        let top = program_coordinates::project_xyz6_coordinate_mm(current.physical_z_mm[1])?;
        for point in &current.points_mm {
            charge(work, 1)?;
            if program_coordinates::project_xyz6_coordinate_mm(point[2])? != top {
                return Err(failed(
                    "functional cached Wall has a nonhorizontal physical interval",
                ));
            }
        }
        let mut matched = false;
        for start in 0..count {
            charge(work, 1)?;
            if current.points_mm[start][..2] != earlier.points_mm[0][..2] {
                continue;
            }
            for reverse in [false, true] {
                let mut complete = true;
                for index in 0..count {
                    cancelled(cancel)?;
                    charge(work, 1)?;
                    let vertex = if reverse {
                        (start + count - index) % count
                    } else {
                        (start + index) % count
                    };
                    let edge = if reverse {
                        (vertex + count - 1) % count
                    } else {
                        vertex
                    };
                    if current.points_mm[vertex][..2] != earlier.points_mm[index][..2]
                        || width(current, edge) != width(earlier, index)
                    {
                        complete = false;
                        break;
                    }
                }
                if complete {
                    matched = true;
                    break;
                }
            }
            if matched {
                break;
            }
        }
        if !matched {
            return Ok(false);
        }
    }
    Ok(true)
}

fn admit(bytes: usize) -> Result<(), ManufacturingError> {
    if bytes as u64 > crate::working_set::bytes_u64() {
        return Err(ManufacturingError::new(
            ManufacturingErrorCode::ResourceLimit,
            "functional reconstructed fill exceeds retained-byte limit",
        ));
    }
    Ok(())
}

fn along(policy: RectilinearAxisPolicy, course: u32) -> usize {
    match policy {
        RectilinearAxisPolicy::X => 0,
        RectilinearAxisPolicy::Y => 1,
        RectilinearAxisPolicy::AlternatingXY => usize::from(!course.is_multiple_of(2)),
        RectilinearAxisPolicy::AlternatingYX => usize::from(course.is_multiple_of(2)),
    }
}

fn points(domain: &MultiPolygon<f64>) -> usize {
    domain
        .0
        .iter()
        .map(|polygon| {
            polygon.exterior().0.len()
                + polygon
                    .interiors()
                    .iter()
                    .map(|ring| ring.0.len())
                    .sum::<usize>()
        })
        .sum()
}

fn boolean_work(
    left: &MultiPolygon<f64>,
    right: &MultiPolygon<f64>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    cancelled(cancel)?;
    charge(
        work,
        points(left).checked_mul(points(right)).ok_or_else(|| {
            ManufacturingError::new(
                ManufacturingErrorCode::ResourceLimit,
                "functional demand Boolean work overflow",
            )
        })?,
    )
}

fn bytes(domain: &MultiPolygon<f64>) -> usize {
    domain.0.capacity() * size_of::<geo::Polygon<f64>>()
        + domain
            .0
            .iter()
            .map(|polygon| {
                polygon.exterior().0.capacity() * size_of::<geo::Coord<f64>>()
                    + std::mem::size_of_val(polygon.interiors())
                    + polygon
                        .interiors()
                        .iter()
                        .map(|ring| ring.0.capacity() * size_of::<geo::Coord<f64>>())
                        .sum::<usize>()
            })
            .sum::<usize>()
}

pub(super) fn retained_bytes(plan: &NormalizedPrintPlan) -> Result<usize, ManufacturingError> {
    let mut total = plan.skin_command_fit_bytes()?;
    let mut add = |value: usize| -> Result<(), ManufacturingError> {
        total = total
            .checked_add(value)
            .filter(|value| *value as u64 <= crate::working_set::bytes_u64())
            .ok_or_else(|| {
                ManufacturingError::new(
                    ManufacturingErrorCode::ResourceLimit,
                    "functional reconstruction retained-byte limit exceeded",
                )
            })?;
        Ok(())
    };
    add(size_of::<NormalizedPrintPlan>())?;
    add(plan.material_domains.capacity() * size_of::<ResolvedMaterialDomain>())?;
    add(plan.material_frames.capacity() * size_of::<SurfaceColorFrame>())?;
    add(plan.surface_resolution.capacity() * size_of::<SurfaceResolution>())?;
    add(plan.process_resolution.capacity() * size_of::<ProcessRegionResolution>())?;
    add(plan.sparse_bands.capacity() * size_of::<SparseBandDecision>())?;
    for resolution in &plan.surface_resolution {
        add(resolution.allocated_bytes())?;
    }
    for resolution in &plan.process_resolution {
        add(resolution.definition_id.allocated_bytes())?;
    }
    if let Some(frame) = &plan.functional_material_frame {
        add(frame.definition_id.allocated_bytes()
            + frame.source_hash.allocated_bytes()
            + frame.face_id.allocated_bytes())?;
        add(frame.key_centers_u_mm.capacity() * size_of::<f64>()
            + frame.solid_base_zones.capacity() * size_of::<FunctionalBaseSolidZone>())?;
    }
    add(plan.layers.capacity() * size_of::<PrintLayer>())?;
    for layer in &plan.layers {
        add(crate::planner::retained_coverage_bytes(
            &layer.region_coverage,
        ))?;
        add(layer.paths.capacity() * size_of::<DepositionPath>())?;
        for path in &layer.paths {
            add(path.points_mm.capacity() * size_of::<[f64; 3]>())?;
            if let WidthProfile::PerEdge { widths_mm } = &path.width {
                add(widths_mm.capacity() * size_of::<f64>())?;
            }
        }
    }
    for domain in &plan.material_domains {
        add(domain.polygons.capacity() * size_of::<MaterialDomainPolygon>())?;
        for polygon in &domain.polygons {
            add(polygon.holes_mm.capacity() * size_of::<Vec<[f64; 2]>>())?;
            add(polygon.exterior_mm.capacity() * size_of::<[f64; 2]>())?;
            for hole in &polygon.holes_mm {
                add(hole.capacity() * size_of::<[f64; 2]>())?;
            }
        }
    }
    Ok(total)
}

fn mandatory_skin(
    plan: &NormalizedPrintPlan,
    index: usize,
    recipe: &PlanarPrintRecipe,
) -> Option<ProcessSkinKind> {
    let layer = &plan.layers[index];
    let lower_target = layer.lower_z_mm - recipe.bottom_skin_thickness_mm;
    if lower_target < plan.layers[0].lower_z_mm - 0.000001 {
        return Some(ProcessSkinKind::BottomSkin);
    }
    let upper_target = layer.z_mm + recipe.top_skin_thickness_mm;
    let above = plan
        .layers
        .partition_point(|candidate| candidate.z_mm < upper_target - 0.000001)
        .max(index + 1);
    (above == plan.layers.len()).then_some(ProcessSkinKind::TopSkin)
}

pub(super) fn verify(
    plan: &NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    frame: &FunctionalMaterialFrame,
    request: &FunctionalMaterialConstruction,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let recipe = &intent.recipe;
    if recipe.infill_fraction == 1.0 {
        return Ok(());
    }
    if plan.interior_selection.is_some()
        || !plan.material_frames.is_empty()
        || !plan.surface_resolution.is_empty()
        || !plan.process_resolution.is_empty()
        || !plan.sparse_bands.is_empty()
    {
        return Err(failed(
            "functional sparse-parent replay does not admit auxiliary selection, surface/process constructions or coarse bands",
        ));
    }
    if recipe.perimeter_count != 1 {
        return Err(failed(
            "functional sparse-parent reconstruction currently requires one perimeter",
        ));
    }
    let JobWorkflow::FixedDualMarlin { job } = &intent.printer.workflow else {
        return Err(failed("functional solid demand lost its loaded-tool route"));
    };
    let retained = retained_bytes(plan)?;
    let mut residuals: Vec<Option<Residual<'_>>> =
        (0..plan.material_domains.len()).map(|_| None).collect();
    let alternation = spiling_contracts::manufacturing::hatch_alternation_keys(&plan.layers);
    for (layer_index, layer) in plan.layers.iter().enumerate() {
        cancelled(cancel)?;
        let mut demands = Vec::new();
        let mut region_index = 0;
        let skin = mandatory_skin(plan, layer_index, recipe);
        let cached_bytes = residuals
            .iter()
            .flatten()
            .map(|cached| {
                bytes(&cached.fill) + cached.paths.capacity() * size_of::<&DepositionPath>()
            })
            .sum::<usize>()
            + residuals.capacity() * size_of::<Option<Residual<'_>>>();
        let mut staged = retained + cached_bytes;
        admit(staged + plan.material_domains.len() * size_of::<(usize, MultiPolygon<f64>)>())?;
        let mut fills = Vec::with_capacity(plan.material_domains.len());
        for (domain_index, domain) in
            plan.material_domains
                .iter()
                .enumerate()
                .filter(|(_, domain)| {
                    domain.physical_z_mm[0] <= layer.lower_z_mm
                        && layer.z_mm <= domain.physical_z_mm[1]
                })
        {
            let source = material(domain);
            staged += bytes(&source);
            let wall_paths = layer
                .paths
                .iter()
                .filter(|path| {
                    path.material_domain == Some(domain.reference)
                        && path.owner == Some(DepositionOwner::Wall)
                })
                .collect::<Vec<_>>();
            let material_id = match domain.reference.role {
                MaterialConstructionRole::FunctionalRegion => &request.material_id,
                MaterialConstructionRole::FunctionalBase => &request.base_material_id,
                _ => {
                    return Err(failed(
                        "functional row reconstruction contains an unrelated material owner",
                    ));
                }
            };
            let tool = job.tool_for_material(material_id)?;
            let starting_work = *work;
            let reused = if let Some(cached) = &residuals[domain_index] {
                same_walls(&cached.paths, &wall_paths, work, cancel)?
            } else {
                false
            };
            let fill = if reused {
                let cached = residuals[domain_index].as_ref().unwrap();
                admit(staged + bytes(&cached.fill))?;
                cached.fill.clone()
            } else {
                walls::residual(
                &source,
                &wall_paths,
                recipe,
                job.tools[tool as usize].nozzle_diameter_mm,
                staged + wall_paths.capacity() * size_of::<&DepositionPath>(),
                work,
                cancel,
            )
            .map_err(|error| {
                ManufacturingError::new(
                    error.code,
                    format!(
                        "{} (functional wall reconstruction, layer {}, role {:?}, starting work {})",
                        error.message, layer.index, domain.reference.role, starting_work
                    ),
                )
            })?
            };
            if !reused {
                admit(
                    staged
                        + 2 * bytes(&fill)
                        + 2 * wall_paths.capacity() * size_of::<&DepositionPath>(),
                )?;
                let cached = Residual {
                    paths: wall_paths.clone(),
                    fill: fill.clone(),
                };
                staged +=
                    bytes(&cached.fill) + cached.paths.capacity() * size_of::<&DepositionPath>();
                residuals[domain_index] = Some(cached);
            }
            staged += bytes(&fill) + wall_paths.capacity() * size_of::<&DepositionPath>();
            if staged as u64 > crate::working_set::bytes_u64() {
                return Err(ManufacturingError::new(
                    ManufacturingErrorCode::ResourceLimit,
                    "functional reconstructed fill exceeds retained-byte limit",
                ));
            }
            let inset = 0.5 * recipe.bead_width_mm;
            for polygon in &fill.0 {
                let bounds = polygon
                    .bounding_rect()
                    .ok_or_else(|| failed("functional reconstructed fill has no bounds"))?;
                if bounds.min().x < frame.stock_bounds_mm.min[0] + inset
                    || bounds.min().y < frame.stock_bounds_mm.min[1] + inset
                    || bounds.max().x > frame.stock_bounds_mm.max[0] - inset
                    || bounds.max().y > frame.stock_bounds_mm.max[1] - inset
                {
                    return Err(failed(
                        "functional reconstruction cannot prove whole-fill native skin survival",
                    ));
                }
            }
            fills.push((domain_index, fill));
        }
        let mut combined = MultiPolygon(Vec::new());
        if skin.is_none() {
            admit(staged + fills.iter().map(|(_, fill)| bytes(fill)).sum::<usize>())?;
            for (_, fill) in &fills {
                combined.0.extend(fill.0.iter().cloned());
            }
            staged += bytes(&combined);
        }
        for (domain_index, mut fill) in fills {
            let domain = &plan.material_domains[domain_index];
            if skin.is_none() {
                let source = material(domain);
                boolean_work(&combined, &source, work, cancel)?;
                fill = combined.intersection(&source);
                staged += bytes(&source) + bytes(&fill);
                admit(staged)?;
            }
            if let Some(skin) = skin {
                demands.push(rows::Demand {
                    material: domain.reference,
                    owner: DepositionOwner::MandatorySkin { skin },
                    kind: match skin {
                        ProcessSkinKind::BottomSkin => DepositionKind::BottomSkin,
                        ProcessSkinKind::TopSkin => DepositionKind::TopSkin,
                    },
                    fraction: 1.0,
                    along: along(
                        if skin == ProcessSkinKind::TopSkin {
                            recipe.top_skin_axis
                        } else {
                            recipe.rectilinear_axis
                        },
                        alternation[layer_index],
                    ),
                    domain: fill,
                });
                continue;
            }
            let pieces = if domain.reference.role == MaterialConstructionRole::FunctionalRegion {
                vec![(fill, 1.0, None)]
            } else {
                let zone = frame.solid_base_zones.iter().enumerate().find(|(_, zone)| {
                    zone.physical_z_mm[0] <= layer.lower_z_mm && layer.z_mm <= zone.physical_z_mm[1]
                });
                let Some((zone_index, zone)) = zone else {
                    let fraction = recipe.infill_fraction;
                    let area = fill.unsigned_area();
                    if area > 0.0 {
                        let coverage = layer
                            .region_coverage
                            .process_regions
                            .get(region_index)
                            .ok_or_else(|| {
                                failed("functional inherited Base demand has no process owner")
                            })?;
                        if coverage.material_domain != Some(domain.reference)
                            || coverage.functional_solid_zone.is_some()
                            || coverage.fraction != fraction
                        {
                            return Err(failed(
                                "functional inherited Base demand changes its process owner or fraction",
                            ));
                        }
                        demands.push(rows::Demand {
                            material: domain.reference,
                            owner: DepositionOwner::Regional {
                                region_index: region_index as u16,
                            },
                            kind: DepositionKind::SparseFill,
                            fraction,
                            along: along(recipe.rectilinear_axis, alternation[layer_index]),
                            domain: fill,
                        });
                        region_index += 1;
                    }
                    continue;
                };
                let solid = MultiPolygon(vec![rectangle(
                    frame,
                    [0.0, frame.width_mm],
                    zone.inward_depth_mm,
                )]);
                boolean_work(&fill, &solid, work, cancel)?;
                let root = fill.intersection(&solid);
                boolean_work(&fill, &solid, work, cancel)?;
                let remainder = fill.difference(&solid);
                staged += bytes(&solid) + bytes(&root) + bytes(&remainder);
                vec![
                    (root, 1.0, Some(zone_index as u8)),
                    (remainder, recipe.infill_fraction, None),
                ]
            };
            for (piece, fraction, zone_index) in pieces {
                if piece.unsigned_area() == 0.0 {
                    continue;
                }
                let coverage = layer
                    .region_coverage
                    .process_regions
                    .get(region_index)
                    .ok_or_else(|| {
                        failed("functional complete-row demand lost its process partition")
                    })?;
                if coverage.material_domain != Some(domain.reference)
                    || coverage.fraction != fraction
                    || coverage.functional_solid_zone != zone_index
                {
                    return Err(failed(
                        "functional complete-row process partition changes its original material, fraction or solid scope",
                    ));
                }
                demands.push(rows::Demand {
                    material: domain.reference,
                    owner: DepositionOwner::Regional {
                        region_index: region_index as u16,
                    },
                    kind: if fraction == 1.0 {
                        DepositionKind::SolidFill
                    } else {
                        DepositionKind::SparseFill
                    },
                    fraction,
                    along: along(recipe.rectilinear_axis, alternation[layer_index]),
                    domain: piece,
                });
                region_index += 1;
            }
        }
        if region_index != layer.region_coverage.process_regions.len() {
            return Err(failed(
                "functional complete-row process partition has extra owners",
            ));
        }
        rows::verify(layer, &demands, recipe.bead_width_mm, staged, work, cancel)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wall() -> DepositionPath {
        DepositionPath {
            material_domain: Some(MaterialDomainRef {
                construction_index: 0,
                role: MaterialConstructionRole::FunctionalBase,
            }),
            owner: Some(DepositionOwner::Wall),
            kind: DepositionKind::Perimeter,
            width: WidthProfile::Constant { width_mm: 0.45 },
            physical_z_mm: [0.0, 0.2],
            points_mm: vec![
                [1.0, 1.0, 0.2],
                [2.0, 1.0, 0.2],
                [2.0, 2.0, 0.2],
                [1.0, 2.0, 0.2],
                [1.0, 1.0, 0.2],
            ],
            arcs: Vec::new(),
        }
    }

    #[test]
    fn functional_wall_reuse_keeps_closure_role_and_every_edge() {
        let original = wall();
        let check = |current: &DepositionPath| {
            same_walls(&[&original], &[current], &mut 0, &AtomicBool::new(false)).unwrap()
        };
        assert!(check(&original));
        let mut reversed = original.clone();
        reversed.reverse();
        assert!(check(&reversed));
        let mut moved_seam = original.clone();
        moved_seam.rotate_closed(2).unwrap();
        assert!(check(&moved_seam));
        let mut open = original.clone();
        open.points_mm.last_mut().unwrap()[0] += 0.01;
        assert!(!check(&open));
        assert!(!same_walls(&[&open], &[&original], &mut 0, &AtomicBool::new(false)).unwrap());
        let mut changed = original.clone();
        changed.kind = DepositionKind::Bridge;
        assert!(!check(&changed));
        changed = original.clone();
        changed.width = WidthProfile::PerEdge {
            widths_mm: vec![0.45, 0.45, 0.45, 0.44],
        };
        assert!(!check(&changed));
    }
}
