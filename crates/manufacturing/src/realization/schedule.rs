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
) -> Result<(Option<BambuEstimate>, u64), ManufacturingError> {
    let machine = machine(intent, routed, start)?;
    let router = routed.map(FixedDualJob::router).transpose()?;
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
    // A layer is one barrier; a first layer with a skirt or brim prints the
    // skirt, then the brim, then everything else, as three barriers.
    let mut estimate = match &intent.printer.workflow {
        JobWorkflow::CompleteMarlin { job }
            if intent.printer.capabilities.output_dialect == OutputDialect::BambuLabGcode3mfV1 =>
        {
            Some(BambuEstimate {
                starts_s: Vec::with_capacity(plan.layers.len()),
                total_s: 0.0,
                filament_mm: job.prime_filament_length_mm,
                max_z_mm: job.prime_end_mm[2].max(job.prime_start_mm[2]),
            })
        }
        _ => None,
    };
    let filament_area = filament_area(intent.recipe.filament_diameter_mm);
    let mut base = 0u32;
    for layer in &plan.layers {
        check_cancel(cancel)?;
        let staged = layer.paths.iter().any(|path| path.kind.is_adhesion());
        for path in &layer.paths {
            let barrier = base
                + if !staged {
                    0
                } else {
                    match path.kind {
                        DepositionKind::Skirt => 0,
                        DepositionKind::Brim => 1,
                        _ => 2,
                    }
                };
            let points = &path.points_mm;
            let process = intent.process_for_path(path)?;
            let length = if let Some(estimate) = &mut estimate {
                let flow = speeds::flow_factor(&process, path.kind, layer.index);
                let height = path.height_mm();
                let mut length = 0.0;
                for edge in path.checked_edges()? {
                    let [a, b] = [edge.points_mm[0], edge.points_mm[1]];
                    length += distance(a, b);
                    let [a, b] = [project_xyz6_point_mm(a)?, project_xyz6_point_mm(b)?];
                    estimate.filament_mm +=
                        distance(a, b) * bead_area(edge.width_mm, height, flow) / filament_area;
                    estimate.max_z_mm = estimate.max_z_mm.max(a[2]).max(b[2]);
                }
                length
            } else {
                points.windows(2).map(|p| distance(p[0], p[1])).sum()
            };
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
                tool: router
                    .as_ref()
                    .map_or(Ok(0), |job| job.tool_for_path(path))?
                    .into(),
                barrier,
                layer: layer.index,
                prerequisites: &dependencies[tasks.len()],
                duration_s: length / process.speed_mm_s,
                seam_policy: (closed(path) && path.owner == Some(DepositionOwner::Wall))
                    .then_some(intent.recipe.seam_policy),
                physical_z_mm: path.physical_z_mm,
            });
        }
        base += if staged { 3 } else { 1 };
    }
    let scheduled = routing::schedule(&tasks, &machine, order, cancel);
    drop(tasks);
    let scheduled = match scheduled {
        Ok(scheduled) => scheduled,
        Err(routing::Error::Cancelled) => {
            check_cancel(cancel)?;
            unreachable!("routing cancels only on a set cancellation flag");
        }
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
    if let Some(estimate) = &mut estimate {
        // Empty layers start at the next group's time. Skirt/brim barriers
        // belong to their layer, whose first group supplies the start time.
        let mut groups = scheduled.groups.iter().peekable();
        let mut barrier = 0;
        for layer in &plan.layers {
            let start = groups
                .peek()
                .map_or(scheduled.cost.total_s, |group| group.begins_at_s);
            estimate.starts_s.push(start.min(scheduled.cost.total_s));
            barrier += if layer.paths.iter().any(|path| path.kind.is_adhesion()) {
                3
            } else {
                1
            };
            while groups.next_if(|group| group.barrier < barrier).is_some() {}
        }
        estimate.total_s = scheduled.cost.total_s;
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
    Ok((estimate, scheduled.work))
}

/// A receipt names the actual stroke, not every path sharing a process role.
/// The independent verifier still qualifies each requested bond from bytes.
fn attachment_dependencies(
    plan: &NormalizedPrintPlan,
    cancel: &AtomicBool,
) -> Result<Vec<Vec<u32>>, ManufacturingError> {
    let mut graph = vec![Vec::new(); plan.layers.iter().map(|l| l.paths.len()).sum()];
    let mut base = 0;
    for layer in &plan.layers {
        if layer.region_coverage.process_interfaces.is_empty() {
            base += layer.paths.len();
            continue;
        }
        let edges = LayerEdges::new(layer, cancel)?;
        for interface in &layer.region_coverage.process_interfaces {
            for (line, bonds) in interface
                .centerlines_mm
                .iter()
                .zip(&interface.endpoint_bonds)
            {
                check_cancel(cancel)?;
                let separators: Vec<usize> = edges
                    .paths_on(*line, layer.z_mm)?
                    .iter()
                    .map(|p| *p as usize)
                    .filter(|p| layer.paths[*p].kind == DepositionKind::SeparatorWall)
                    .collect();
                for bond in bonds {
                    let wall = match *bond {
                        ProcessEndpointBond::Wall { wall_segment_mm }
                        | ProcessEndpointBond::Skin {
                            wall_segment_mm, ..
                        } => wall_segment_mm,
                    };
                    let walls: Vec<usize> = edges
                        .paths_on(wall, layer.z_mm)?
                        .iter()
                        .map(|p| *p as usize)
                        .filter(|p| layer.paths[*p].owner == Some(DepositionOwner::Wall))
                        .collect();
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
                            for index in edges.paths_on(skin_segment_mm, layer.z_mm)? {
                                let index = *index as usize;
                                let path = &layer.paths[index];
                                if path.kind == process_kind
                                    && path.owner
                                        == Some(DepositionOwner::MandatorySkin { skin: skin_kind })
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

/// One projected, undirected XYZ6 edge; equal keys are equal command edges.
type EdgeKey = [[u64; 3]; 2];

/// A layer's receipt-eligible paths by projected edge, projected once.
struct LayerEdges {
    paths: std::collections::HashMap<EdgeKey, Vec<u32>>,
}
impl LayerEdges {
    fn new(layer: &PrintLayer, cancel: &AtomicBool) -> Result<Self, ManufacturingError> {
        let mut paths = std::collections::HashMap::<EdgeKey, Vec<u32>>::new();
        for (index, path) in layer.paths.iter().enumerate() {
            check_cancel(cancel)?;
            let eligible = path.kind == DepositionKind::SeparatorWall
                || matches!(
                    path.owner,
                    Some(DepositionOwner::Wall | DepositionOwner::MandatorySkin { .. })
                );
            if !eligible {
                continue;
            }
            for edge in path.checked_edges()? {
                let list = paths
                    .entry(key(edge.points_mm[0], edge.points_mm[1])?)
                    .or_default();
                // Paths arrive in ascending order; keep each once.
                if list.last() != Some(&(index as u32)) {
                    list.push(index as u32);
                }
            }
        }
        Ok(Self { paths })
    }
    /// Eligible paths, ascending, holding this segment as one command edge.
    fn paths_on(&self, line: [[f64; 2]; 2], z: f64) -> Result<&[u32], ManufacturingError> {
        let key = key([line[0][0], line[0][1], z], [line[1][0], line[1][1], z])?;
        Ok(self.paths.get(&key).map_or(&[], Vec::as_slice))
    }
}

fn key(a: [f64; 3], b: [f64; 3]) -> Result<EdgeKey, ManufacturingError> {
    use spiling_contracts::manufacturing::program_coordinates::project_xyz6_point_mm;
    // Signed zeros compare equal, so they share one key.
    let bits = |p: [f64; 3]| p.map(|v| (v + 0.0).to_bits());
    let (a, b) = (
        bits(project_xyz6_point_mm(a)?),
        bits(project_xyz6_point_mm(b)?),
    );
    Ok(if a <= b { [a, b] } else { [b, a] })
}
