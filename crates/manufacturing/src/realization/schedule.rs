// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Adapter to the deposition scheduler: borrow path ends, apply its order and
//! entries in place. The scheduler never sees or changes path geometry.
use super::*;
use spiling_deposition_routing as routing;

fn closed(path: &DepositionPath) -> bool {
    path.points_mm.len() > 2 && path.points_mm.first() == path.points_mm.last()
}

fn machine(
    intent: &ManufacturingIntent,
    routed: Option<&FixedDualJob>,
    start: Option<[f64; 3]>,
) -> Result<routing::Machine, ManufacturingError> {
    let recipe = &intent.recipe;
    let Some(job) = routed else {
        return Ok(routing::Machine {
            start,
            initial_tool: Some(0),
            tools: 1,
            travel: routing::Travel {
                speed_mm_s: recipe.travel_speed_mm_s,
                lift_mm: 0.0,
                retraction: match recipe.process.retraction {
                    RetractionPolicy::Enabled {
                        minimum_travel_mm,
                        distance_mm,
                        speed_mm_s,
                        z_hop_mm,
                    } => Some(routing::Retraction {
                        minimum_mm: minimum_travel_mm,
                        seconds: 2.0 * distance_mm / speed_mm_s,
                        lift_mm: z_hop_mm,
                    }),
                    RetractionPolicy::Disabled => None,
                },
            },
            change: routing::ToolChange {
                swap_s: 0.0,
                prewarm_lead_s: None,
                service_s: 0.0,
                service: routing::Service::WhenUsed,
                stations_xy: vec![None],
            },
        });
    };
    // Rigid dual carriage: both tools deposit supported waste on every layer
    // and each group resumes from its own waste-lane end. Every model entry
    // lifts to one safe plane, a per-path constant that ordering cannot change.
    let mut swap_s = 0.0f64;
    let mut service_s = 0.0f64;
    let mut stations_xy = Vec::with_capacity(job.tools.len());
    for (index, tool) in job.tools.iter().enumerate() {
        let line = tool.waste_line_xy_mm;
        let process = intent.process_for_tool(Some(index as u8))?;
        swap_s = swap_s.max(2.0 * tool.retract_filament_mm / tool.retract_speed_mm_s);
        service_s = service_s
            .max((line[0][0] - line[1][0]).hypot(line[0][1] - line[1][1]) / process.speed_mm_s);
        stations_xy.push(Some(line[1]));
    }
    Ok(routing::Machine {
        start: None,
        initial_tool: Some(0),
        tools: job.tools.len() as u32,
        travel: routing::Travel {
            speed_mm_s: recipe.travel_speed_mm_s,
            lift_mm: 0.0,
            retraction: None,
        },
        change: routing::ToolChange {
            swap_s,
            prewarm_lead_s: None,
            service_s,
            service: routing::Service::EveryBarrier,
            stations_xy,
        },
    })
}

/// Reorder every layer's paths, reverse open paths and rotate closed non-wall
/// paths as scheduled. Closed wall seams stay with the authored seam policy.
pub(super) fn apply(
    plan: &mut NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    routed: Option<&FixedDualJob>,
    start: Option<[f64; 3]>,
    cancel: &AtomicBool,
    order: routing::ToolOrder,
) -> Result<(), ManufacturingError> {
    let machine = machine(intent, routed, start)?;
    let count: usize = plan.layers.iter().map(|layer| layer.paths.len()).sum();
    let mut tasks = Vec::new();
    tasks.try_reserve_exact(count).map_err(|_| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "deposition schedule exceeds available memory",
        )
    })?;
    // Export the construction's actual attachment receipts, not a role rank.
    let dependencies = attachment_dependencies(plan, cancel)?;
    for (barrier, layer) in plan.layers.iter().enumerate() {
        check_cancel(cancel)?;
        for path in &layer.paths {
            let points = &path.points_mm;
            let length: f64 = points.windows(2).map(|p| distance(p[0], p[1])).sum();
            tasks.push(routing::Task {
                ends: if closed(path) {
                    routing::Ends::Loop {
                        seams: &points[..points.len() - 1],
                    }
                } else {
                    routing::Ends::Open {
                        first: points[0],
                        last: points[points.len() - 1],
                        reversible: path.owner != Some(DepositionOwner::Wall),
                    }
                },
                tool: routed.map_or(Ok(0), |job| job.tool_for_path(path))?.into(),
                barrier: barrier as u32,
                prerequisites: &dependencies[tasks.len()],
                duration_s: length / intent.process_for_path(path)?.speed_mm_s,
                seam_policy: (closed(path) && path.owner == Some(DepositionOwner::Wall))
                    .then_some(intent.recipe.seam_policy),
                physical_z_mm: path.physical_z_mm,
            });
        }
    }
    let scheduled = routing::schedule(&tasks, &machine, order, cancel);
    drop(tasks);
    let scheduled = match scheduled {
        Ok(scheduled) => scheduled,
        Err(routing::Error::Cancelled) => return check_cancel(cancel),
        Err(other) => {
            return Err(error(
                ManufacturingErrorCode::InvalidSpecification,
                format!("deposition schedule refused: {other:?}"),
            ));
        }
    };
    plan.routing_tool_orders.clear();
    if routed.is_some() {
        for groups in scheduled.groups.as_chunks::<2>().0 {
            plan.routing_tool_orders
                .push([groups[0].tool as u8, groups[1].tool as u8]);
        }
    }
    // Visits are grouped by ascending layer; a layer's visits are contiguous.
    let mut visits = scheduled.visits.iter();
    let mut offset = 0usize;
    for layer in &mut plan.layers {
        check_cancel(cancel)?;
        let n = layer.paths.len();
        let mut slots: Vec<Option<DepositionPath>> = std::mem::take(&mut layer.paths)
            .into_iter()
            .map(Some)
            .collect();
        layer.paths.reserve_exact(n);
        for visit in visits.by_ref().take(n) {
            let mut path = (visit.task as usize)
                .checked_sub(offset)
                .and_then(|index| slots.get_mut(index))
                .and_then(Option::take)
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::InvalidSpecification,
                        "deposition schedule left its layer",
                    )
                })?;
            if closed(&path) {
                if visit.seam != 0 {
                    path.rotate_closed(visit.seam as usize)?;
                }
            } else if visit.reverse {
                path.reverse();
            }
            layer.paths.push(path);
        }
        if layer.paths.len() != n {
            return Err(error(
                ManufacturingErrorCode::InvalidSpecification,
                "deposition schedule dropped a path",
            ));
        }
        offset += n;
    }
    Ok(())
}

/// A receipt names the actual stroke, not every path sharing a process role.
/// The independent verifier still qualifies each requested bond from bytes.
fn attachment_dependencies(
    plan: &NormalizedPrintPlan,
    cancel: &AtomicBool,
) -> Result<Vec<Vec<u32>>, ManufacturingError> {
    use spiling_contracts::manufacturing::program_coordinates::project_xyz6_point_mm;
    fn contains(
        path: &DepositionPath,
        line: [[f64; 2]; 2],
        z: f64,
    ) -> Result<bool, ManufacturingError> {
        let expected = [
            project_xyz6_point_mm([line[0][0], line[0][1], z])?,
            project_xyz6_point_mm([line[1][0], line[1][1], z])?,
        ];
        for edge in path.checked_edges()? {
            let actual = [
                project_xyz6_point_mm(edge.points_mm[0])?,
                project_xyz6_point_mm(edge.points_mm[1])?,
            ];
            if actual == expected || actual == [expected[1], expected[0]] {
                return Ok(true);
            }
        }
        Ok(false)
    }
    let mut graph = vec![Vec::new(); plan.layers.iter().map(|l| l.paths.len()).sum()];
    let mut base = 0;
    for layer in &plan.layers {
        for interface in &layer.region_coverage.process_interfaces {
            for (line, bonds) in interface
                .centerlines_mm
                .iter()
                .zip(&interface.endpoint_bonds)
            {
                check_cancel(cancel)?;
                let mut separators = Vec::new();
                for (index, path) in layer.paths.iter().enumerate() {
                    if path.kind == DepositionKind::SeparatorWall
                        && contains(path, *line, layer.z_mm)?
                    {
                        separators.push(index);
                    }
                }
                for bond in bonds {
                    let wall = match *bond {
                        ProcessEndpointBond::Wall { wall_segment_mm }
                        | ProcessEndpointBond::Skin {
                            wall_segment_mm, ..
                        } => wall_segment_mm,
                    };
                    let mut walls = Vec::new();
                    for (index, path) in layer.paths.iter().enumerate() {
                        if path.owner == Some(DepositionOwner::Wall)
                            && contains(path, wall, layer.z_mm)?
                        {
                            walls.push(index);
                        }
                    }
                    match *bond {
                        ProcessEndpointBond::Wall { .. } => {
                            for &separator in &separators {
                                graph[base + separator]
                                    .extend(walls.iter().map(|p| (base + p) as u32));
                            }
                        }
                        ProcessEndpointBond::Skin {
                            skin_kind,
                            process_kind,
                            skin_segment_mm,
                            ..
                        } => {
                            for (index, path) in layer.paths.iter().enumerate() {
                                if path.kind == process_kind
                                    && path.owner
                                        == Some(DepositionOwner::MandatorySkin { skin: skin_kind })
                                    && contains(path, skin_segment_mm, layer.z_mm)?
                                {
                                    graph[base + index]
                                        .extend(walls.iter().map(|p| (base + p) as u32));
                                    for &separator in &separators {
                                        graph[base + separator].push((base + index) as u32);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        base += layer.paths.len();
    }
    for edges in &mut graph {
        edges.sort_unstable();
        edges.dedup();
    }
    Ok(graph)
}
