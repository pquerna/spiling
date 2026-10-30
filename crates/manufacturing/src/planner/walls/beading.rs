// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Finite arm-local births/deaths. A count event never changes the width of a
//! continuing bead instantaneously. Junction arms keep their incoming ordinal
//! allocation; the complete commanded union, not an inscribed disk, qualifies
//! the separately reserved central patch.
use super::skeleton::Skeleton;
use super::*;
use geo::{Contains, Coord, Distance, Euclidean, Line, Point};

#[derive(Clone, Copy)]
pub(super) struct Incident {
    pub edge: usize,
    pub end: usize,
}
pub(super) struct Adjacency {
    offsets: Vec<usize>,
    incident: Vec<Incident>,
    degree: Vec<usize>,
}
impl Adjacency {
    fn new(
        graph: &Skeleton,
        cells: &[Option<f64>],
        contour: bool,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<Self, ManufacturingError> {
        let mut degree = vec![0usize; graph.junctions.len()];
        for (index, edge) in graph.edges.iter().enumerate() {
            check_cancel(cancel)?;
            boolean::charge(work, 1)?;
            if cells[index].is_some() != contour {
                continue;
            }
            for vertex in edge.vertex_ids {
                degree[vertex] += 1;
            }
        }
        let mut offsets = Vec::with_capacity(degree.len() + 1);
        offsets.push(0);
        let mut total = 0;
        for &count in &degree {
            total += count;
            offsets.push(total);
        }
        let mut incident = vec![Incident { edge: 0, end: 0 }; total];
        let mut cursors = offsets[..degree.len()].to_vec();
        for (index, edge) in graph.edges.iter().enumerate() {
            if cells[index].is_some() != contour {
                continue;
            }
            for (end, vertex) in edge.vertex_ids.into_iter().enumerate() {
                incident[cursors[vertex]] = Incident { edge: index, end };
                cursors[vertex] += 1;
            }
        }
        Ok(Self {
            offsets,
            incident,
            degree,
        })
    }
    pub(super) fn at(&self, vertex: usize) -> &[Incident] {
        &self.incident[self.offsets[vertex]..self.offsets[vertex + 1]]
    }
}

/// Both disjoint CSR partitions describe ONE original source-node incidence.
/// Geometry must not lose a declared point endpoint when its cell changes from
/// an opposing arm into contour transport. No coordinates or new graph are owned.
#[derive(Clone, Copy)]
struct SourceIncidence<'a> {
    graph: &'a Skeleton,
    arms: &'a Adjacency,
    contours: &'a Adjacency,
}
impl<'a> SourceIncidence<'a> {
    fn at(self, vertex: usize) -> impl Iterator<Item = &'a Incident> {
        self.arms.at(vertex).iter().chain(self.contours.at(vertex))
    }
}

fn contour_components(
    graph: &Skeleton,
    adjacency: &Adjacency,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Vec<usize>, ManufacturingError> {
    let mut components = vec![usize::MAX; graph.junctions.len()];
    let mut pending = Vec::with_capacity(graph.junctions.len());
    for start in 0..components.len() {
        if components[start] != usize::MAX {
            continue;
        }
        components[start] = start;
        pending.push(start);
        while let Some(vertex) = pending.pop() {
            check_cancel(cancel)?;
            boolean::charge(work, 1)?;
            for incident in adjacency.at(vertex) {
                boolean::charge(work, 1)?;
                let next = graph.edges[incident.edge].vertex_ids[1 - incident.end];
                if components[next] == usize::MAX {
                    components[next] = start;
                    pending.push(next);
                }
            }
        }
    }
    Ok(components)
}
#[derive(Clone, Copy)]
struct Span {
    edge: usize,
    t: [f64; 2],
    s: [f64; 2],
}
#[derive(Clone, Copy)]
enum ArmCount {
    Source(usize),
    Uniform { count: usize, width: f64 },
}
impl ArmCount {
    fn count(self) -> usize {
        match self {
            Self::Source(count) | Self::Uniform { count, .. } => count,
        }
    }
}
struct Arm<'a> {
    spans: &'a [Span],
    ends: [usize; 2],
    length: f64,
    count: Option<ArmCount>,
}
struct ArmRange {
    spans: [usize; 2],
    ends: [usize; 2],
    length: f64,
    count: Option<ArmCount>,
    events: [usize; 2],
    choices: Vec<(f64, ArmCount)>,
    choice: usize,
}
impl ArmRange {
    fn arm<'a>(&self, spans: &'a [Span]) -> Arm<'a> {
        Arm {
            spans: &spans[self.spans[0]..self.spans[1]],
            ends: self.ends,
            length: self.length,
            count: self.count,
        }
    }
}
#[derive(Clone, Copy)]
struct Event {
    begin: f64,
    end: f64,
    old: usize,
    high: usize,
    increasing: bool,
    start_width: f64,
    arm_gradient: f64,
}
#[derive(Clone, Copy)]
struct Field {
    pairs: usize,
    central: bool,
    continuing_width: f64,
    newborn_width: Option<f64>,
    split: bool,
    split_start: bool,
}
struct CornerPlan {
    edge: usize,
    widths: [f64; 2],
    outer: contours::OuterWidth,
    events: [usize; 2],
    spines: Vec<[f64; 2]>,
}
#[derive(Clone, Copy, Debug)]
struct CornerEvent {
    parameter: f64,
    ordinal: usize,
    position_error_mm: f64,
    residual_bound_mm: f64,
    cap_radius_mm: f64,
    /// Nominal-width turn owned by this birth and its literal source corner.
    bevel: Option<InnerBevel>,
}

#[derive(Clone, Copy, Debug)]
struct InnerBevel {
    parameters: [f64; 2],
    points: [[f64; 2]; 2],
    width_mm: f64,
}

/// Immutable source walks/events survive candidate qualification. Finite
/// whole-arm counts and source-owned inner-turn constructions advance without
/// reconstructing the skeleton or independent material target.
pub(super) struct SourcePlan {
    cells: Vec<Option<f64>>,
    adjacency: Adjacency,
    contour_adjacency: Adjacency,
    contour_components: Vec<usize>,
    spans: Vec<Span>,
    arms: Vec<ArmRange>,
    events: Vec<Event>,
    source_permissions: usize,
    emission_bytes: usize,
    complete_joints: bool,
    bevel_available: bool,
    bevel_corners: bool,
}

pub(super) fn construct(
    graph: &Skeleton,
    nominal_fill: &MultiPolygon<f64>,
    request: &WallRequest<'_>,
    permissions: &mut Vec<Junction>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<SourcePlan, ManufacturingError> {
    // Geometry-sized metadata has a fixed bound. The sampled walk grows under
    // actual-capacity admission below, not a speculative 100k-command reserve.
    let walk_bytes = graph.edges.len()
        * (3 * std::mem::size_of::<ArmRange>()
            + 2 * std::mem::size_of::<Incident>()
            + 3 * std::mem::size_of::<Option<f64>>()
            + 1)
        + (graph.junctions.len() + 1)
            * (6 * std::mem::size_of::<usize>()
                + std::mem::size_of::<f64>()
                + std::mem::size_of::<Option<usize>>())
        + (permissions.len() + graph.junctions.len()) * std::mem::size_of::<Junction>() * 3
        + graph.junctions.len() * 2 * std::mem::size_of::<usize>();
    let walk_request = staged_request(request, walk_bytes + permission_bytes(permissions))?;
    let cells = contours::cells(graph, request, work, cancel)?;
    let adjacency = Adjacency::new(graph, &cells, false, work, cancel)?;
    let contour_adjacency = Adjacency::new(graph, &cells, true, work, cancel)?;
    let contour_components = contour_components(graph, &contour_adjacency, work, cancel)?;
    for (vertex, junction) in graph.junctions.iter().enumerate() {
        if adjacency.degree[vertex] + contour_adjacency.degree[vertex] != junction.valence {
            return Err(unsupported(
                "source graph contour/arm partition changed retained valence",
            ));
        }
    }
    let incidence = SourceIncidence {
        graph,
        arms: &adjacency,
        contours: &contour_adjacency,
    };
    let mut junction_permissions = vec![None; graph.junctions.len()];
    for (vertex, junction) in graph.junctions.iter().enumerate() {
        let valence = junction.valence;
        if valence >= 3 && !nominal_fill.contains(&Point::from(graph.junctions[vertex].point)) {
            // A source-graph junction wholly in nominal core requires no
            // wall connection and grants no arbitrary core-overlap permission.
            junction_permissions[vertex] = Some(permissions.len());
            permissions.push(Junction {
                center: graph.junctions[vertex].point,
                radius_mm: request.limits.max_bead_width_mm,
                valence,
                ports: Vec::new(),
                transition: None,
            });
        }
    }
    let mut visited = vec![false; graph.edges.len()];
    let mut spans = Vec::new();
    let mut arms = Vec::new();
    for vertex in 0..adjacency.degree.len() {
        if adjacency.degree[vertex] == 2 {
            continue;
        }
        for &incident in adjacency.at(vertex) {
            if visited[incident.edge] {
                continue;
            }
            arms.push(walk(
                graph,
                &adjacency,
                incident,
                &mut visited,
                &mut spans,
                &walk_request,
                work,
                cancel,
            )?);
        }
    }
    let mut bevel_available = false;
    for index in 0..graph.edges.len() {
        if !bevel_available
            && request.recipe.perimeter_count > 1
            && cells[index].is_some()
            && contours::source_corner(&graph.edges[index], graph).is_some()
        {
            bevel_available = true;
        }
        if visited[index] || cells[index].is_some() {
            continue;
        }
        arms.push(walk(
            graph,
            &adjacency,
            Incident {
                edge: index,
                end: 0,
            },
            &mut visited,
            &mut spans,
            &walk_request,
            work,
            cancel,
        )?);
    }
    // Walks fixed the exact global span count. Events cannot outnumber spans;
    // their disjoint corridors retain at most spans+two endpoints per event.
    // Include free/spine/break buffers, port payload, every permission header
    // and a possible old+new output-edge reallocation before event growth.
    let event_bytes = spans.capacity() * std::mem::size_of::<Span>()
        + arms.capacity() * std::mem::size_of::<ArmRange>()
        + spans.len()
            * (3 * std::mem::size_of::<Event>()
                + 10 * std::mem::size_of::<[f64; 2]>()
                + 3 * std::mem::size_of::<f64>())
        + (permissions.len() + spans.len()) * 3 * std::mem::size_of::<Junction>()
        + graph.edges.len() * 12 * std::mem::size_of::<[[f64; 2]; 2]>()
        + graph.junctions.len()
            * (std::mem::size_of::<Option<contours::VertexField>>()
                + 4 * std::mem::size_of::<f64>()
                + std::mem::size_of::<Option<usize>>())
        + graph.edges.len()
            * (3 * std::mem::size_of::<CornerPlan>() + std::mem::size_of::<Option<f64>>())
        + (graph.junctions.len() + 2 * graph.edges.len()) * std::mem::size_of::<(f64, usize)>()
        + graph.edges.len()
            * request.recipe.perimeter_count as usize
            * 24
            * (std::mem::size_of::<Junction>() + std::mem::size_of::<CornerEvent>());
    let corner_completion_bytes = graph.edges.len()
        * (16 * request.recipe.perimeter_count as usize + 32)
        * 3
        * std::mem::size_of::<[f64; 2]>();
    let event_staging = event_bytes
        + corner_completion_bytes
        + (graph.junctions.len() + 1) * 5 * std::mem::size_of::<usize>()
        + graph.edges.len() * (2 * std::mem::size_of::<Incident>() + 1);
    admit_stage(request, event_staging + permission_bytes(permissions))?;
    let mut events = Vec::new();
    let mut choice_bytes = 0usize;
    for range in &mut arms {
        range.events[0] = events.len();
        let permission_start = permissions.len();
        // Pointwise nearest-preferred counts are only a candidate objective.
        // Keep useful finite transitions when they fit. If this candidate has
        // no admitted finite field, enumerate legal whole-arm counts before
        // rejecting the part; resource and cancellation errors are never tried
        // again as a different candidate. No bead has been emitted yet.
        if let Err(reason) = plan_events(
            graph,
            incidence,
            &range.arm(&spans),
            request,
            &mut events,
            permissions,
            work,
            cancel,
        ) {
            if reason.code != ManufacturingErrorCode::UnsupportedGeometry {
                return Err(reason);
            }
            events.truncate(range.events[0]);
            permissions.truncate(permission_start);
            range.choices = whole_arm_counts(
                graph,
                incidence,
                &range.arm(&spans),
                request,
                event_staging + choice_bytes + permission_bytes(permissions),
                work,
                cancel,
            )?;
            choice_bytes += range.choices.capacity() * std::mem::size_of::<(f64, ArmCount)>();
            let Some(&(_, count)) = range.choices.first() else {
                return Err(reason);
            };
            range.count = Some(count);
        }
        range.events[1] = events.len();
    }
    // Source-field arm ports are fixed before any candidate sweep. Even-count
    // arms use their complete cross-section, not an incidental medial seam.
    for range in &arms {
        let arm = range.arm(&spans);
        for side in 0..2 {
            let vertex = arm.ends[side];
            let Some(permission) = junction_permissions[vertex] else {
                continue;
            };
            let distance = (request.limits.max_bead_width_mm * 0.8).min(arm.length * 0.5);
            let s = if side == 0 {
                distance
            } else {
                arm.length - distance
            };
            let (curve, t, _) = locate(graph, &arm, s);
            let point = curve.at(t);
            let port = curve.supports.map(|support| support.foot(point));
            permissions[permission].ports.push(port);
        }
    }
    drop(junction_permissions);
    Ok(SourcePlan {
        cells,
        adjacency,
        contour_adjacency,
        contour_components,
        spans,
        arms,
        events,
        source_permissions: permissions.len(),
        emission_bytes: event_staging + choice_bytes,
        complete_joints: false,
        bevel_available,
        bevel_corners: false,
    })
}

pub(super) fn retained_bytes(plan: &SourcePlan) -> usize {
    let adjacency_bytes = |a: &Adjacency| {
        (a.offsets.capacity() + a.degree.capacity()) * std::mem::size_of::<usize>()
            + a.incident.capacity() * std::mem::size_of::<Incident>()
    };
    plan.cells.capacity() * std::mem::size_of::<Option<f64>>()
        + adjacency_bytes(&plan.adjacency)
        + adjacency_bytes(&plan.contour_adjacency)
        + plan.contour_components.capacity() * std::mem::size_of::<usize>()
        + plan.spans.capacity() * std::mem::size_of::<Span>()
        + plan.arms.capacity() * std::mem::size_of::<ArmRange>()
        + plan.events.capacity() * std::mem::size_of::<Event>()
        + plan
            .arms
            .iter()
            .map(|arm| arm.choices.capacity() * std::mem::size_of::<(f64, ArmCount)>())
            .sum::<usize>()
}

/// Preserve all existing sharp-corner field candidates first, then enumerate
/// the same finite fields with source-owned inner bevels. Every construction
/// retains the complete original-D/R/ports/command/excess qualifier.
pub(super) fn advance(plan: &mut SourcePlan) -> bool {
    if !plan.complete_joints {
        // Try the source-predeclared joint construction for this nearer count
        // before changing its width/count objective. The candidate set and all
        // independent qualification gates stay unchanged.
        plan.complete_joints = true;
        return true;
    }
    plan.complete_joints = false;
    for arm in plan.arms.iter_mut().rev() {
        if arm.choices.is_empty() {
            continue;
        }
        if arm.choice + 1 < arm.choices.len() {
            arm.choice += 1;
            arm.count = Some(arm.choices[arm.choice].1);
            return true;
        }
        arm.choice = 0;
        arm.count = Some(arm.choices[0].1);
    }
    if plan.bevel_available && !plan.bevel_corners {
        plan.bevel_corners = true;
        return true;
    }
    false
}

pub(super) fn emit(
    plan: &SourcePlan,
    graph: &Skeleton,
    nominal_fill: &MultiPolygon<f64>,
    request: &WallRequest<'_>,
    edges: &mut Vec<BoundaryEdge>,
    permissions: &mut Vec<Junction>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let SourcePlan {
        cells,
        adjacency,
        contour_adjacency,
        contour_components,
        spans,
        arms,
        events,
        source_permissions,
        emission_bytes,
        complete_joints,
        bevel_available: _,
        bevel_corners,
    } = plan;
    let incidence = SourceIncidence {
        graph,
        arms: adjacency,
        contours: contour_adjacency,
    };
    edges.clear();
    permissions.truncate(*source_permissions);
    let staged = staged_request(request, emission_bytes + permission_bytes(permissions))?;
    let request = &staged;
    admit_stage(
        request,
        edges.capacity() * std::mem::size_of::<BoundaryEdge>(),
    )?;
    // A corner cell is not an independent .45-mm policy. Its retained source
    // node inherits the already resolved incoming width/pair field. This
    // transports original graph correspondence, never a nearby-coordinate weld.
    let mut vertex_fields = vec![None::<contours::VertexField>; graph.junctions.len()];
    for range in arms {
        let arm = range.arm(spans);
        let arm_events = &events[range.events[0]..range.events[1]];
        for span in arm.spans {
            let curve = &graph.edges[span.edge];
            for (t, s) in span.t.into_iter().zip(span.s) {
                let vertex = match t {
                    0.0 => curve.vertex_ids[0],
                    1.0 => curve.vertex_ids[1],
                    _ => continue,
                };
                boolean::charge(work, arm_events.len().saturating_add(1))?;
                let event = arm_events
                    .iter()
                    .copied()
                    .find(|event| event.begin <= s && s <= event.end);
                let incoming = field(graph, adjacency, &arm, arm_events, s, event, request)?;
                let value = &mut vertex_fields[vertex];
                let incoming = contours::VertexField {
                    width: incoming.continuing_width,
                    pairs: incoming.pairs,
                    central: incoming.central,
                };
                *value = Some(match *value {
                    None => incoming,
                    Some(prior) if prior.pairs > 0 && incoming.pairs == 0 => prior,
                    Some(prior) if prior.pairs == 0 => incoming,
                    Some(prior) => contours::VertexField {
                        width: prior.width.min(incoming.width),
                        pairs: prior.pairs.min(incoming.pairs),
                        central: prior.central
                            && incoming.central
                            && prior.pairs == incoming.pairs
                            && prior.width == incoming.width,
                    },
                });
            }
        }
    }
    // Every source/count/event field is resolved before candidate beads. Shared
    // junction endcaps include central newborns, not merely pointwise parity.
    let mut junction_caps = vec![0.0f64; graph.junctions.len()];
    for range in arms {
        let arm = range.arm(spans);
        let arm_events = &events[range.events[0]..range.events[1]];
        for side in 0..2 {
            let s = if side == 0 { 0.0 } else { arm.length };
            let event = arm_events
                .iter()
                .copied()
                .find(|event| event.begin <= s && s <= event.end);
            let field = field(graph, adjacency, &arm, arm_events, s, event, request)?;
            for lane in lane_ids(field) {
                let central = matches!(lane, Lane::Center)
                    || matches!(lane, Lane::Newborn) && !field.split
                    || matches!(lane, Lane::Split) && field.split_start;
                if central {
                    let radius = (lane_width(lane, field) * 0.5
                        - super::super::OFFSET_TOLERANCE_MM)
                        .max(0.0);
                    let vertex = arm.ends[side];
                    junction_caps[vertex] = junction_caps[vertex].max(radius);
                }
            }
        }
    }
    let contour_widths = contours::transport(
        graph,
        cells,
        contour_adjacency,
        &vertex_fields,
        None,
        request,
        work,
        cancel,
    )?;
    let mut needs_outer = false;
    for (edge, curve) in graph.edges.iter().enumerate() {
        check_cancel(cancel)?;
        boolean::charge(work, 64)?;
        if cells[edge].is_some()
            && contours::corner_seed(curve, graph, &contour_widths, request, work, cancel)?
                .is_some()
        {
            needs_outer = true;
            break;
        }
    }
    let outer_widths = if needs_outer {
        Some(contours::transport(
            graph,
            cells,
            contour_adjacency,
            &vertex_fields,
            Some(&contour_widths),
            request,
            work,
            cancel,
        )?)
    } else {
        None
    };
    emit_corners(
        graph,
        incidence,
        cells,
        contour_components,
        &vertex_fields,
        &contour_widths,
        outer_widths.as_deref(),
        &junction_caps,
        nominal_fill,
        *complete_joints,
        *bevel_corners,
        request,
        edges,
        permissions,
        work,
        cancel,
    )?;
    for range in arms {
        emit_arm(
            graph,
            incidence,
            &range.arm(spans),
            &events[range.events[0]..range.events[1]],
            &junction_caps,
            nominal_fill,
            request,
            edges,
            work,
            cancel,
        )?;
    }
    Ok(())
}

fn contour_width(widths: [f64; 2], t: f64) -> f64 {
    if t == 0.0 {
        widths[0]
    } else if t == 1.0 {
        widths[1]
    } else {
        widths[0] + (widths[1] - widths[0]) * t
    }
}

/// Same source-parameter contour point used by the ordinary straight branches.
/// Only a birth-owned bevel can replace their central finite turn.
#[allow(clippy::too_many_arguments)]
fn nominal_corner_point(
    incidence: SourceIncidence<'_>,
    curve: &MedialEdge,
    t: f64,
    site: BoundarySupport,
    ordinal: usize,
    width: f64,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<[f64; 2], ManufacturingError> {
    let p = curve.at(t);
    let foot = canonical_foot(incidence, curve, t, site, p, work, cancel)?;
    let delta = sub(p, foot);
    let length = norm(delta);
    if !length.is_finite() || length <= 0.0 {
        return Err(unsupported(
            "source inner contour has no positive finite radius",
        ));
    }
    Ok(add(
        foot,
        mul(delta, (ordinal as f64 + 0.5) * width / length),
    ))
}

/// Retain the nominal-radius tangent ports of one literal convex source birth,
/// but connect them with one finite straight command. No continuous-tube area
/// theorem substitutes for the independent finite-command dose qualification.
#[allow(clippy::too_many_arguments)]
fn bevel_corner(
    incidence: SourceIncidence<'_>,
    curve: &MedialEdge,
    widths: [f64; 2],
    event: &mut CornerEvent,
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    if event.ordinal == 0 || widths[0] != widths[1] {
        return Ok(());
    }
    let Some((_, terminal, _)) = contours::source_corner(curve, incidence.graph) else {
        return Ok(());
    };
    check_cancel(cancel)?;
    boolean::charge(work, 64)?;
    let present = 1 - terminal;
    let birth = curve.at(event.parameter);
    let width = widths[0];
    let mut ends = [[0.0; 2]; 2];
    for (side, site) in curve.supports.into_iter().enumerate() {
        ends[side] = nominal_corner_point(
            incidence,
            curve,
            present as f64,
            site,
            event.ordinal,
            width,
            work,
            cancel,
        )?;
    }
    let rays = ends.map(|p| sub(p, birth));
    let lengths = rays.map(norm);
    if lengths
        .into_iter()
        .any(|length| !length.is_finite() || length <= 0.0)
    {
        return Ok(());
    }
    let units = std::array::from_fn::<_, 2, _>(|side| mul(rays[side], lengths[side].recip()));
    let cosine = dot(units[0], units[1]);
    if !cosine.is_finite() || !(-1.0..1.0).contains(&cosine) {
        return Ok(());
    }
    let radius_mm = width * 0.5;
    let travel = radius_mm * ((1.0 + cosine) / (1.0 - cosine)).sqrt();
    if !travel.is_finite() || travel <= 0.0 || lengths.into_iter().any(|length| travel > length) {
        return Ok(());
    }
    let parameters = std::array::from_fn::<_, 2, _>(|side| {
        let fraction = travel / lengths[side];
        if fraction == 1.0 {
            present as f64
        } else {
            event.parameter + (present as f64 - event.parameter) * fraction
        }
    });
    let targets = units.map(|unit| add(birth, mul(unit, travel)));
    let mut points = [[0.0; 2]; 2];
    for (side, site) in curve.supports.into_iter().enumerate() {
        points[side] = nominal_corner_point(
            incidence,
            curve,
            parameters[side],
            site,
            event.ordinal,
            width,
            work,
            cancel,
        )?;
    }
    let magnitude = curve
        .endpoints
        .into_iter()
        .flatten()
        .map(f64::abs)
        .fold(1.0, f64::max);
    let arithmetic = (1024.0 * f64::EPSILON * magnitude).next_up();
    let endpoint_error = (norm(sub(points[0], targets[0])).max(norm(sub(points[1], targets[1])))
        + arithmetic)
        .next_up();
    if endpoint_error > FIT_ERROR_MM * 0.5 {
        return Ok(());
    }
    let permission_radius =
        (request.limits.max_bead_width_mm - event.position_error_mm).next_down();
    if !qualification::junction_contains_capsule(
        birth,
        targets,
        radius_mm + FIT_ERROR_MM + program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM,
        permission_radius,
    ) {
        return Ok(());
    }
    let bevel = InnerBevel {
        parameters,
        points,
        width_mm: width,
    };
    let midpoint = add(points[0], mul(sub(points[1], points[0]), 0.5));
    // Replacing a sharp birth also replaces its source endcap witness. Never
    // lend the old larger central ball to completion after moving the contour.
    let cap = (event.cap_radius_mm
        - norm(sub(midpoint, birth))
        - arithmetic
        - endpoint_error
        - program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM)
        .next_down();
    if cap > 0.0 {
        event.cap_radius_mm = cap;
        event.bevel = Some(bevel);
    }
    Ok(())
}

/// Locate only an already endpoint-bracketed birth in the original site field.
/// Bisection proposes a parameter; a fresh oriented source bracket encloses its
/// position/residual before publication. This is not exact/native authority or
/// an additional command/material allowance.
fn corner_birth(
    curve: &MedialEdge,
    widths: contours::OuterWidth,
    ordinal: usize,
    range: [f64; 2],
    endpoint_values: [f64; 2],
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<CornerEvent, ManufacturingError> {
    let depth = ordinal as f64 + 0.5;
    let value = |t| curve.supports[0].radius(curve.at(t)) - depth * widths.at(t);
    if endpoint_values.into_iter().any(|v| !v.is_finite())
        || endpoint_values == [0.0; 2]
        || endpoint_values.into_iter().all(|v| v != 0.0)
            && endpoint_values[0].is_sign_positive() == endpoint_values[1].is_sign_positive()
    {
        return Err(unsupported(
            "source contour birth has no oriented endpoint bracket",
        ));
    }
    let increasing = endpoint_values[0] < endpoint_values[1];
    let mut bracket = if endpoint_values[0] == 0.0 {
        [range[0]; 2]
    } else if endpoint_values[1] == 0.0 {
        [range[1]; 2]
    } else {
        range
    };
    // A fixed proposal budget, never a claim that binary64 found an exact root.
    for _ in 0..64 {
        check_cancel(cancel)?;
        boolean::charge(work, 16)?;
        let middle = bracket[0] + (bracket[1] - bracket[0]) * 0.5;
        if middle == bracket[0] || middle == bracket[1] {
            break;
        }
        let residual = value(middle);
        if !residual.is_finite() {
            return Err(unsupported("source contour birth is nonfinite"));
        }
        if residual == 0.0 {
            bracket = [middle, middle];
            break;
        }
        if (residual < 0.0) == increasing {
            bracket[0] = middle;
        } else {
            bracket[1] = middle;
        }
    }
    check_cancel(cancel)?;
    boolean::charge(work, 64)?;
    let parameter = bracket[0] + (bracket[1] - bracket[0]) * 0.5;
    let center = curve.at(parameter);
    let scale = curve
        .endpoints
        .into_iter()
        .flatten()
        .chain(
            curve
                .supports
                .into_iter()
                .flat_map(|site| site.a.into_iter().chain(site.b)),
        )
        .map(f64::abs)
        .fold(1.0, f64::max);
    let arithmetic = (1024.0 * f64::EPSILON * scale).next_up();
    let speed = ((norm(sub(curve.endpoints[1], curve.endpoints[0]))
        + 4.0 * curve.chord_error_bound(0.0, 1.0))
    .next_up()
        + arithmetic)
        .next_up();
    if !speed.is_finite() || speed <= 0.0 {
        return Err(unsupported(
            "source contour birth has no finite source metric",
        ));
    }
    // The existing construction enclosure schedules ONE validation bracket.
    // Its whole-image and oriented scalar bounds below remain mandatory.
    let reach = (FIT_ERROR_MM / (2.0 * speed).next_up())
        .next_down()
        .min(1.0);
    let range = [(parameter - reach).max(0.0), (parameter + reach).min(1.0)];
    let points = range.map(|t| curve.at(t));
    let deviation = curve.chord_error_bound(range[0], range[1]);
    let scalar_error = (deviation + arithmetic * (1.0 + depth)).next_up();
    let ends = range.map(value);
    let position_error_mm = ((points
        .into_iter()
        .map(|p| norm(sub(p, center)))
        .fold(0.0, f64::max)
        + deviation)
        .next_up()
        + arithmetic)
        .next_up();
    let residual_bound_mm = (value(parameter).abs() + scalar_error).next_up();
    let oriented = if increasing {
        ends[0] < -scalar_error && ends[1] > scalar_error
    } else {
        ends[0] > scalar_error && ends[1] < -scalar_error
    };
    if ![
        reach,
        deviation,
        scalar_error,
        ends[0],
        ends[1],
        position_error_mm,
        residual_bound_mm,
    ]
    .into_iter()
    .all(f64::is_finite)
        || range[0] >= range[1]
        || !oriented
        || position_error_mm > FIT_ERROR_MM
        || residual_bound_mm > FIT_ERROR_MM
    {
        return Err(unsupported(&format!(
            "source contour birth location is unproved: \
            parameter={parameter}, range={range:?}, values={ends:?}, scalar_error={scalar_error}, \
            position_error={position_error_mm}, residual_bound={residual_bound_mm}, \
            ordinal={ordinal}, widths={widths:?}, endpoints={:?}, supports={:?}",
            curve.endpoints, curve.supports
        )));
    }
    let cap_radius_mm = ((range
        .map(|t| widths.at(t))
        .into_iter()
        .fold(f64::INFINITY, f64::min)
        * 0.5
        - position_error_mm)
        .next_down()
        - arithmetic)
        .next_down();
    if !cap_radius_mm.is_finite() || cap_radius_mm <= 0.0 {
        return Err(unsupported(
            "source contour birth has no bounded positive cap",
        ));
    }
    Ok(CornerEvent {
        parameter,
        ordinal,
        position_error_mm,
        residual_bound_mm,
        cap_radius_mm,
        bevel: None,
    })
}

#[allow(clippy::too_many_arguments)]
fn emit_corners(
    graph: &Skeleton,
    incidence: SourceIncidence<'_>,
    cells: &[Option<f64>],
    components: &[usize],
    vertex_fields: &[Option<contours::VertexField>],
    vertex_widths: &[f64],
    outer_widths: Option<&[f64]>,
    junction_caps: &[f64],
    nominal_fill: &MultiPolygon<f64>,
    complete_joints: bool,
    bevel_corners: bool,
    request: &WallRequest<'_>,
    output: &mut Vec<BoundaryEdge>,
    permissions: &mut Vec<Junction>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let mut corners = Vec::new();
    let mut contour_events = Vec::new();
    let preferred = source_width_mm(request)?;
    // Resolve all endpoint-bracketed source births before any corner bead.
    for (edge, curve) in graph.edges.iter().enumerate() {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        if cells[edge].is_none() {
            continue;
        }
        let widths = curve.vertex_ids.map(|vertex| vertex_widths[vertex]);
        let outer = contours::OuterWidth::new(
            curve
                .vertex_ids
                .map(|vertex| outer_widths.unwrap_or(vertex_widths)[vertex]),
            preferred,
            outer_widths.and(cells[edge]),
        );
        let begin = contour_events.len();
        for ordinal in 0..request.recipe.perimeter_count as usize {
            boolean::charge(work, 1)?;
            let depth = ordinal as f64 + 0.5;
            let profile = if ordinal == 0 {
                outer
            } else {
                contours::OuterWidth::new(widths, preferred, None)
            };
            let mut knots = profile.knots();
            boolean::sort_by(&mut knots, |t| *t, work, cancel)?;
            let mut prior_parameter = None;
            for interval in knots.windows(2) {
                if interval[0] == interval[1] {
                    continue;
                }
                let values = [interval[0], interval[1]]
                    .map(|t| curve.supports[0].radius(curve.at(t)) - depth * profile.at(t));
                if values.into_iter().any(|v| !v.is_finite()) {
                    return Err(unsupported("source contour birth endpoint is nonfinite"));
                }
                if values == [0.0; 2]
                    || values[0] != 0.0
                        && values[1] != 0.0
                        && values[0].is_sign_positive() == values[1].is_sign_positive()
                    || values[0] == 0.0 && interval[0] == 0.0
                    || values[1] == 0.0 && interval[1] == 1.0
                {
                    continue;
                }
                if contour_events.len() >= MAX_DEPOSITION_SEGMENTS as usize {
                    return Err(resource("source contour event limit exceeded"));
                }
                let mut event = corner_birth(
                    curve,
                    profile,
                    ordinal,
                    [interval[0], interval[1]],
                    values,
                    work,
                    cancel,
                )?;
                if prior_parameter == Some(event.parameter) {
                    continue;
                }
                prior_parameter = Some(event.parameter);
                if bevel_corners {
                    bevel_corner(incidence, curve, widths, &mut event, request, work, cancel)?;
                }
                permissions.push(Junction {
                    center: curve.at(event.parameter),
                    radius_mm: (request.limits.max_bead_width_mm - event.position_error_mm)
                        .next_down(),
                    valence: 2,
                    ports: Vec::new(),
                    transition: None,
                });
                contour_events.push(event);
            }
        }
        let events = [begin, contour_events.len()];
        corners.push(CornerPlan {
            edge,
            widths,
            outer,
            events,
            spines: Vec::new(),
        });
    }
    if complete_joints {
        // Resolve the COMPLETE source event inventory before considering any
        // completion. A degree-two cell is not a contour-component boundary.
        for index in 0..corners.len() {
            check_cancel(cancel)?;
            boolean::charge(work, 1)?;
            let corner = &corners[index];
            let spines = corner_completion(
                &graph.edges[corner.edge],
                corner.widths,
                corner.outer,
                &contour_events[corner.events[0]..corner.events[1]],
                &corners,
                &contour_events,
                components,
                graph,
                junction_caps,
                nominal_fill,
                request,
                work,
                cancel,
            )?;
            corners[index].spines = spines;
        }
    }
    for corner in corners {
        check_cancel(cancel)?;
        let curve = &graph.edges[corner.edge];
        let events = &contour_events[corner.events[0]..corner.events[1]];
        let sampling_request = staged_request(
            request,
            output.capacity() * std::mem::size_of::<BoundaryEdge>(),
        )?;
        admit_stage(&sampling_request, std::mem::size_of::<(f64, [f64; 2])>())?;
        let mut samples = vec![(0.0, curve.endpoints[0])];
        refine(
            curve,
            0.0,
            1.0,
            0,
            &sampling_request,
            &mut samples,
            work,
            cancel,
        )?;
        // Keep every affine width switch before subdivision. Constant-section
        // cells use the exact same field for events and commanded edge widths.
        let mut knots = corner.outer.knots();
        boolean::sort_by(&mut knots, |t| *t, work, cancel)?;
        for interval in knots.windows(2) {
            if interval[0] == interval[1] {
                continue;
            }
            let change = (corner.outer.at(interval[1]) - corner.outer.at(interval[0]))
                .abs()
                .max(
                    (contour_width(corner.widths, interval[1])
                        - contour_width(corner.widths, interval[0]))
                    .abs(),
                );
            let steps = (change / (2.0 * FIT_ERROR_MM)).ceil().max(1.0) as usize;
            for step in 0..steps {
                let t = interval[0] + (interval[1] - interval[0]) * step as f64 / steps as f64;
                if t == 0.0 {
                    continue;
                }
                check_cancel(cancel)?;
                boolean::charge(work, 1)?;
                if samples.len() >= MAX_DEPOSITION_SEGMENTS as usize {
                    return Err(resource("source contour width subdivision limit exceeded"));
                }
                reserve_stage(&mut samples, 1, &sampling_request, 0)?;
                samples.push((t, curve.at(t)));
            }
        }
        for event in events {
            if samples.len() >= MAX_DEPOSITION_SEGMENTS as usize {
                return Err(resource("source contour subdivision limit exceeded"));
            }
            reserve_stage(&mut samples, 1, &sampling_request, 0)?;
            samples.push((event.parameter, curve.at(event.parameter)));
            if let Some(bevel) = event.bevel.as_ref() {
                for t in bevel.parameters {
                    if samples.len() >= MAX_DEPOSITION_SEGMENTS as usize {
                        return Err(resource(
                            "source inner bevel port subdivision limit exceeded",
                        ));
                    }
                    reserve_stage(&mut samples, 1, &sampling_request, 0)?;
                    samples.push((t, curve.at(t)));
                }
            }
        }
        boolean::sort_by(&mut samples, |sample| sample.0, work, cancel)?;
        samples.dedup_by(|a, b| a.0 == b.0);
        let ids = curve.supports.map(|site| site.boundary_index);
        let command_request = staged_request(
            request,
            samples.capacity() * std::mem::size_of::<(f64, [f64; 2])>(),
        )?;
        for interval in samples.windows(2) {
            let midpoint = (interval[0].0 + interval[1].0) * 0.5;
            let radius = curve.supports[0].radius(curve.at(midpoint));
            let pairs = ((radius / contour_width(corner.widths, midpoint) + 0.5).floor() as usize)
                .min(request.recipe.perimeter_count as usize);
            let outer_present = radius >= corner.outer.at(midpoint) * 0.5;
            for (side, site) in curve.supports.into_iter().enumerate() {
                for ordinal in 0..pairs.max(usize::from(outer_present)) {
                    if ordinal == 0 && !outer_present {
                        continue;
                    }
                    boolean::charge(work, events.len())?;
                    if events.iter().any(|event| {
                        event.ordinal == ordinal
                            && event.bevel.as_ref().is_some_and(|bevel| {
                                let port = bevel.parameters[side];
                                event.parameter.min(port) < midpoint
                                    && midpoint < event.parameter.max(port)
                            })
                    }) {
                        continue;
                    }
                    let widths = [interval[0].0, interval[1].0].map(|t| {
                        if ordinal == 0 {
                            corner.outer.at(t)
                        } else {
                            contour_width(corner.widths, t)
                        }
                    });
                    boolean::charge(work, events.len().saturating_mul(4).saturating_add(1))?;
                    let mut points = [[0.0; 2]; 2];
                    for end in 0..2 {
                        let (t, p) = interval[end];
                        let vertex = match t {
                            0.0 => Some(curve.vertex_ids[0]),
                            1.0 => Some(curve.vertex_ids[1]),
                            _ => None,
                        };
                        let bevel_port = events
                            .iter()
                            .filter(|event| event.ordinal == ordinal)
                            .find_map(|event| {
                                event.bevel.as_ref().and_then(|bevel| {
                                    (bevel.parameters[side] == t).then_some(bevel.points[side])
                                })
                            });
                        points[end] = if let Some(port) = bevel_port {
                            port
                        } else if events
                            .iter()
                            .any(|event| event.parameter == t && event.ordinal == ordinal)
                            || vertex.and_then(|vertex| vertex_fields[vertex]).is_some_and(
                                |field| {
                                    field.pairs > 0
                                        && field.central
                                        && ordinal == field.pairs
                                        && field.width == widths[end]
                                },
                            )
                        {
                            p
                        } else {
                            let foot = canonical_foot(incidence, curve, t, site, p, work, cancel)?;
                            add(
                                foot,
                                mul(
                                    sub(p, foot),
                                    (ordinal as f64 + 0.5)
                                        * if ordinal == 0 {
                                            widths[end]
                                        } else {
                                            contour_width(corner.widths, t)
                                        }
                                        / norm(sub(p, foot)),
                                ),
                            )
                        };
                    }
                    push_edge(
                        output,
                        points,
                        widths[0].min(widths[1]),
                        ids,
                        Some(site.boundary_index),
                        ordinal,
                        widths,
                        &command_request,
                    )?;
                }
            }
        }
        for event in events {
            if let Some(bevel) = event.bevel.as_ref() {
                check_cancel(cancel)?;
                boolean::charge(work, 1)?;
                push_edge(
                    output,
                    bevel.points,
                    bevel.width_mm,
                    ids,
                    Some(ids[0]),
                    event.ordinal,
                    [bevel.width_mm; 2],
                    &command_request,
                )?;
            }
        }
        if !corner.spines.is_empty() {
            let width = admitted_width_range(request)?[0];
            for parameters in corner.spines {
                check_cancel(cancel)?;
                boolean::charge(work, 1)?;
                push_edge(
                    output,
                    parameters.map(|t| curve.at(t)),
                    width,
                    ids,
                    None,
                    request.recipe.perimeter_count as usize,
                    [width; 2],
                    &command_request,
                )?;
            }
        }
    }
    Ok(())
}

/// Subtract a complete finite endcap's source-parameter footprint. This is
/// proposal ownership only; neither a cap nor a point query proves material.
fn subtract_corner_cap(
    free: &mut Vec<[f64; 2]>,
    origin: [f64; 2],
    delta: [f64; 2],
    length_squared: f64,
    center: [f64; 2],
    radius: f64,
    deviation: f64,
) -> Result<(), ManufacturingError> {
    let offset = sub(center, origin);
    // Projection is a proposal only. A triangle-inequality travel interval
    // about its evaluated point stays inside the cap after the WHOLE original
    // curve/chord discrepancy and arithmetic are reserved.
    let projection = (dot(offset, delta) / length_squared).clamp(0.0, 1.0);
    let scale = origin
        .into_iter()
        .chain(delta)
        .chain(center)
        .map(f64::abs)
        .fold(1.0, f64::max);
    let arithmetic = (1024.0 * f64::EPSILON * scale).next_up();
    let point = add(origin, mul(delta, projection));
    let distance = (norm(sub(point, center)) + arithmetic).next_up();
    let length = (norm(delta) + arithmetic).next_up();
    if ![projection, radius, deviation, distance, length]
        .into_iter()
        .all(f64::is_finite)
        || length <= 0.0
        || deviation < 0.0
    {
        return Err(unsupported("source corner cap footprint is unconditioned"));
    }
    let remaining = ((radius - distance).next_down() - deviation).next_down();
    if remaining <= 0.0 {
        return Ok(());
    }
    let reach = (remaining / length).next_down();
    let cover = [
        (projection - reach).next_up(),
        (projection + reach).next_down(),
    ];
    let mut index = 0;
    while index < free.len() {
        let [a, b] = free[index];
        if cover[1] <= a || b <= cover[0] {
            index += 1;
            continue;
        }
        if cover[0] <= a && b <= cover[1] {
            free.swap_remove(index);
            continue;
        }
        if a < cover[0] && cover[1] < b {
            free[index] = [a, cover[0]];
            free.push([cover[1], b]);
        } else if cover[0] <= a {
            free[index][0] = cover[1];
        } else {
            free[index][1] = cover[0];
        }
        index += 1;
    }
    Ok(())
}

fn cap_may_meet_interval(center: [f64; 2], radius: f64, points: [[f64; 2]; 2], error: f64) -> bool {
    (0..2).all(|axis| {
        (center[axis] + radius).next_up()
            >= (points[0][axis].min(points[1][axis]) - error).next_down()
            && (center[axis] - radius).next_down()
                <= (points[0][axis].max(points[1][axis]) + error).next_up()
    })
}

/// The inventory is complete and source-owned before any corner bead. Borrow
/// its events rather than treating an incidental degree-two cell as an owner
/// boundary. This certifies a whole source interval, not a sampled point.
#[allow(clippy::too_many_arguments)]
fn component_cap_covers(
    curve: &MedialEdge,
    range: [f64; 2],
    all_corners: &[CornerPlan],
    all_events: &[CornerEvent],
    components: &[usize],
    graph: &Skeleton,
    junction_caps: &[f64],
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    let component = components[curve.vertex_ids[0]];
    let points = range.map(|t| curve.at(t));
    let error = curve.chord_error_bound(range[0], range[1]);
    if !error.is_finite() {
        return Err(unsupported(
            "source cap interval enclosure is unconditioned",
        ));
    }
    for corner in all_corners {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        let source = &graph.edges[corner.edge];
        if components[source.vertex_ids[0]] != component {
            continue;
        }
        for event in &all_events[corner.events[0]..corner.events[1]] {
            check_cancel(cancel)?;
            boolean::charge(work, 8)?;
            let center = source.at(event.parameter);
            if cap_may_meet_interval(center, event.cap_radius_mm, points, error)
                && qualification::junction_contains_capsule(
                    center,
                    points,
                    error,
                    event.cap_radius_mm,
                )
            {
                return Ok(true);
            }
        }
    }
    for (vertex, &cap) in junction_caps.iter().enumerate() {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        if cap <= 0.0 || components[vertex] != component {
            continue;
        }
        boolean::charge(work, 8)?;
        let center = graph.junctions[vertex].point;
        let radius = cap + super::super::OFFSET_TOLERANCE_MM;
        if cap_may_meet_interval(center, radius, points, error)
            && qualification::junction_contains_capsule(center, points, error, radius)
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Completion intervals are proposals from retained site/width fields and
/// source births/endcaps. Endpoint deficit interpolation only schedules a
/// proposal; whole-curve enclosure and full independent D/R proof still apply.
/// No candidate missing-area polygon supplies a location or an overlap grant.
#[allow(clippy::too_many_arguments)]
fn corner_completion(
    curve: &MedialEdge,
    widths: [f64; 2],
    outer: contours::OuterWidth,
    events: &[CornerEvent],
    all_corners: &[CornerPlan],
    all_events: &[CornerEvent],
    components: &[usize],
    graph: &Skeleton,
    junction_caps: &[f64],
    nominal_fill: &MultiPolygon<f64>,
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Vec<[f64; 2]>, ManufacturingError> {
    let mut breaks = Vec::new();
    reserve_stage(&mut breaks, events.len() * 3 + 8, request, 0)?;
    breaks.extend(outer.knots());
    for event in events {
        breaks.push(event.parameter);
        if let Some(bevel) = event.bevel.as_ref() {
            breaks.extend(bevel.parameters);
        }
    }
    boolean::sort_by(&mut breaks, |t| *t, work, cancel)?;
    breaks.dedup();
    let origin = curve.endpoints[0];
    let delta = sub(curve.endpoints[1], origin);
    let length = norm(delta);
    if !length.is_finite() || length <= 0.0 {
        return Err(unsupported(
            "source corner has no finite completion interval",
        ));
    }
    let width = admitted_width_range(request)?[0];
    let minimum_length = program_coordinates::MIN_DEPOSITION_COMMAND_MM + 0.000003;
    let command_length_lower = |parameters: [f64; 2]| -> Result<f64, ManufacturingError> {
        let points = [
            point(key(curve.at(parameters[0]))?),
            point(key(curve.at(parameters[1]))?),
        ];
        let magnitude = points
            .iter()
            .flatten()
            .copied()
            .map(f64::abs)
            .fold(1.0, f64::max);
        let arithmetic = 128.0 * f64::EPSILON * magnitude;
        let chord = norm(sub(points[1], points[0]));
        if !chord.is_finite() || !arithmetic.is_finite() {
            return Err(unsupported(
                "source completion has no finite decoded command metric",
            ));
        }
        Ok((chord - arithmetic).next_down())
    };
    let mut free = Vec::new();
    let mut spines = Vec::<[f64; 2]>::new();
    for pair in breaks.windows(2) {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        let middle = (pair[0] + pair[1]) * 0.5;
        let p = curve.at(middle);
        if nominal_fill.contains(&Point::from(p)) {
            continue;
        }
        let pairs = ((curve.supports[0].radius(p) / contour_width(widths, middle) + 0.5).floor()
            as usize)
            .min(request.recipe.perimeter_count as usize);
        // The first outer rounded joint already has its explicit bounded
        // underfill policy. Complete only an interior inter-lane source gap.
        if pairs == 0 {
            continue;
        }
        let mut need = [pair[0], pair[1]];
        let mut empty = false;
        for site in curve.supports {
            let gap = [pair[0], pair[1]].map(|t| {
                site.radius(curve.at(t))
                    - if pairs == 1 {
                        outer.at(t)
                    } else {
                        (pairs as f64 - 0.5) * contour_width(widths, t)
                            + contour_width(widths, t) * 0.5
                    }
                    - super::super::OFFSET_TOLERANCE_MM
            });
            if gap[0] <= 0.0 && gap[1] <= 0.0 {
                empty = true;
                break;
            }
            if gap[0] <= 0.0 || gap[1] <= 0.0 {
                let root = pair[0] - gap[0] * (pair[1] - pair[0]) / (gap[1] - gap[0]);
                if gap[0] <= 0.0 {
                    need[0] = need[0].max(root);
                } else {
                    need[1] = need[1].min(root);
                }
            }
        }
        if empty || need[0] >= need[1] {
            continue;
        }
        if component_cap_covers(
            curve,
            need,
            all_corners,
            all_events,
            components,
            graph,
            junction_caps,
            work,
            cancel,
        )? {
            continue;
        }
        free.clear();
        free.push(need);
        for event in events {
            boolean::charge(work, 1)?;
            subtract_corner_cap(
                &mut free,
                origin,
                delta,
                length * length,
                curve.at(event.parameter),
                event.cap_radius_mm,
                curve.chord_error_bound(0.0, 1.0),
            )?;
        }
        for vertex in curve.vertex_ids {
            if junction_caps[vertex] > 0.0 {
                subtract_corner_cap(
                    &mut free,
                    origin,
                    delta,
                    length * length,
                    graph.junctions[vertex].point,
                    junction_caps[vertex] + super::super::OFFSET_TOLERANCE_MM,
                    curve.chord_error_bound(0.0, 1.0),
                )?;
            }
        }
        for &[lo, hi] in &free {
            check_cancel(cancel)?;
            boolean::charge(work, 64)?;
            let span = ((hi - lo) * length).max(minimum_length);
            if span > length {
                return Err(unsupported(
                    "required source corner completion has no minimum-length interval",
                ));
            }
            let start = ((lo + hi) * 0.5 - span * 0.5 / length).clamp(0.0, 1.0 - span / length);
            let mut parameters = [start, start + span / length];
            let mut actual_lower = command_length_lower(parameters)?;
            if actual_lower < program_coordinates::MIN_DEPOSITION_COMMAND_MM {
                // Whole-curve endpoint chord is only an initial scheduler for
                // a local quadratic. Enlarge ONE centered original interval
                // from its actual XYZ6 chord, never infer admission from the
                // scale factor or assume monotonicity after endpoint clamping.
                let center = (parameters[0] + parameters[1]) * 0.5;
                let prior_span = parameters[1] - parameters[0];
                let proposed = if actual_lower > 0.0 {
                    (prior_span * (minimum_length / actual_lower).next_up()).next_up()
                } else {
                    1.0
                };
                let parameter_span = proposed.min(1.0).max(prior_span);
                let start = (center - parameter_span * 0.5).clamp(0.0, 1.0 - parameter_span);
                parameters = [start, (start + parameter_span).min(1.0)];
                actual_lower = command_length_lower(parameters)?;
            }
            if actual_lower < program_coordinates::MIN_DEPOSITION_COMMAND_MM {
                return Err(unsupported(&format!(
                    "source completion has no admitted minimum-length XYZ6 chord: parameters={parameters:?}, length_lower={actual_lower}"
                )));
            }
            let points = parameters.map(|t| curve.at(t));
            let radius = (width * 0.5
                + program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM
                + curve.chord_error_bound(parameters[0], parameters[1]))
            .next_up();
            let component = components[curve.vertex_ids[0]];
            let mut authorized = false;
            // Borrow only permissions already declared by this original
            // contour component; do not manufacture a grant from the spine.
            'births: for corner in all_corners {
                check_cancel(cancel)?;
                boolean::charge(work, 1)?;
                let source = &graph.edges[corner.edge];
                if components[source.vertex_ids[0]] != component {
                    continue;
                }
                for event in &all_events[corner.events[0]..corner.events[1]] {
                    check_cancel(cancel)?;
                    boolean::charge(work, 8)?;
                    let center = source.at(event.parameter);
                    let limit =
                        (request.limits.max_bead_width_mm - event.position_error_mm).next_down();
                    if cap_may_meet_interval(center, limit, points, radius)
                        && qualification::junction_contains_capsule(center, points, radius, limit)
                    {
                        authorized = true;
                        break 'births;
                    }
                }
            }
            for (vertex, node) in graph.junctions.iter().enumerate() {
                if authorized {
                    break;
                }
                check_cancel(cancel)?;
                boolean::charge(work, 1)?;
                if components[vertex] != component
                    || node.valence < 3
                    || !cap_may_meet_interval(
                        node.point,
                        request.limits.max_bead_width_mm,
                        points,
                        radius,
                    )
                {
                    continue;
                }
                boolean::charge(work, region::point_count(nominal_fill).saturating_add(8))?;
                if !nominal_fill.contains(&Point::from(node.point)) {
                    authorized |= qualification::junction_contains_capsule(
                        node.point,
                        points,
                        radius,
                        request.limits.max_bead_width_mm,
                    );
                }
            }
            if !authorized {
                let nodes = curve.vertex_ids.map(|v| {
                    (
                        v,
                        graph.junctions[v].point,
                        graph.junctions[v].valence,
                        junction_caps[v],
                        nominal_fill.contains(&Point::from(graph.junctions[v].point)),
                    )
                });
                let mut neighbor = None::<(f64, usize, f64, usize, [f64; 2], f64, bool, bool, f64)>;
                for corner in all_corners {
                    let source = &graph.edges[corner.edge];
                    if components[source.vertex_ids[0]] != component {
                        continue;
                    }
                    for event in &all_events[corner.events[0]..corner.events[1]] {
                        check_cancel(cancel)?;
                        boolean::charge(work, 1)?;
                        let (t, ordinal) = (event.parameter, event.ordinal);
                        let center = source.at(t);
                        let birth_width = if ordinal == 0 {
                            corner.outer.at(t)
                        } else {
                            contour_width(corner.widths, t)
                        };
                        let distance = points
                            .into_iter()
                            .map(|p| norm(sub(p, center)))
                            .fold(0.0, f64::max);
                        if neighbor.as_ref().is_none_or(|prior| distance < prior.0) {
                            neighbor = Some((
                                distance,
                                corner.edge,
                                t,
                                ordinal,
                                center,
                                birth_width,
                                qualification::junction_contains_capsule(
                                    center,
                                    points,
                                    curve.chord_error_bound(parameters[0], parameters[1]),
                                    event.cap_radius_mm,
                                ),
                                qualification::junction_contains_capsule(
                                    center,
                                    points,
                                    radius,
                                    (request.limits.max_bead_width_mm - event.position_error_mm)
                                        .next_down(),
                                ),
                                event.residual_bound_mm,
                            ));
                        }
                    }
                }
                return Err(unsupported(&format!(
                    "required corner completion has no predeclared source-joint enclosure: \
                    parameters={parameters:?}, need={need:?}, free={:?}, points={points:?}, \
                    width={width}, field_widths={widths:?}, pairs={pairs}, \
                    curve_endpoints={:?}, chord_enclosure={}, nodes={nodes:?}, \
                    events_count={}, first_events={:?}, supports={:?}, \
                    source_component={component}, nearest_component_birth={neighbor:?}",
                    [lo, hi],
                    curve.endpoints,
                    curve.chord_error_bound(0.0, 1.0),
                    events.len(),
                    &events[..events.len().min(4)],
                    curve.supports,
                )));
            }
            spines.push(parameters);
        }
    }
    boolean::sort_by(&mut spines, |span| span[0], work, cancel)?;
    let mut retained = 0;
    for index in 0..spines.len() {
        let span = spines[index];
        if retained > 0 && span[0] <= spines[retained - 1][1] {
            spines[retained - 1][1] = spines[retained - 1][1].max(span[1]);
        } else {
            spines[retained] = span;
            retained += 1;
        }
    }
    spines.truncate(retained);
    Ok(spines)
}

fn walk(
    graph: &Skeleton,
    adjacency: &Adjacency,
    seed: Incident,
    visited: &mut [bool],
    spans: &mut Vec<Span>,
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<ArmRange, ManufacturingError> {
    let first = spans.len();
    let mut length = 0.0;
    let mut ends = [graph.edges[seed.edge].vertex_ids[seed.end]; 2];
    let mut current = seed;
    loop {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        if visited[current.edge] {
            break;
        }
        visited[current.edge] = true;
        let curve = &graph.edges[current.edge];
        let sampling_request =
            staged_request(request, spans.capacity() * std::mem::size_of::<Span>())?;
        admit_stage(&sampling_request, std::mem::size_of::<(f64, [f64; 2])>())?;
        let mut samples = vec![(0.0, curve.endpoints[0])];
        refine(
            curve,
            0.0,
            1.0,
            0,
            &sampling_request,
            &mut samples,
            work,
            cancel,
        )?;
        if current.end == 1 {
            samples.reverse();
        }
        for pair in samples.windows(2) {
            let step = norm(sub(pair[1].1, pair[0].1));
            if !step.is_finite() || step <= 0.0 {
                return Err(unsupported("nonpositive medial arm interval"));
            }
            if spans.len() >= MAX_DEPOSITION_SEGMENTS as usize {
                return Err(resource("medial arm subdivision limit exceeded"));
            }
            let next = length + step;
            reserve_stage(
                spans,
                1,
                request,
                samples.capacity() * std::mem::size_of::<(f64, [f64; 2])>(),
            )?;
            spans.push(Span {
                edge: current.edge,
                t: [pair[0].0, pair[1].0],
                s: [length, next],
            });
            length = next;
        }
        let vertex = curve.vertex_ids[1 - current.end];
        ends[1] = vertex;
        if adjacency.degree[vertex] != 2 {
            break;
        }
        let Some(next) = adjacency
            .at(vertex)
            .iter()
            .copied()
            .find(|item| !visited[item.edge])
        else {
            break;
        };
        current = next;
    }
    Ok(ArmRange {
        spans: [first, spans.len()],
        ends,
        length,
        count: None,
        events: [0; 2],
        choices: Vec::new(),
        choice: 0,
    })
}

fn locate<'a>(graph: &'a Skeleton, arm: &Arm, s: f64) -> (&'a MedialEdge, f64, usize) {
    let index = arm
        .spans
        .partition_point(|span| span.s[1] < s)
        .min(arm.spans.len() - 1);
    let span = arm.spans[index];
    let fraction = ((s - span.s[0]) / (span.s[1] - span.s[0])).clamp(0.0, 1.0);
    (
        &graph.edges[span.edge],
        span.t[0] + fraction * (span.t[1] - span.t[0]),
        index,
    )
}
fn thickness(graph: &Skeleton, arm: &Arm, s: f64) -> f64 {
    let (curve, t, _) = locate(graph, arm, s);
    let p = curve.at(t);
    curve.supports[0].radius(p) + curve.supports[1].radius(p)
}
fn raw(
    graph: &Skeleton,
    arm: &Arm,
    s: f64,
    request: &WallRequest<'_>,
) -> Result<Allocation, ManufacturingError> {
    allocate(thickness(graph, arm, s) * 0.5, request)
}
fn field_parameter(adjacency: &Adjacency, arm: &Arm, s: f64, request: &WallRequest<'_>) -> f64 {
    let radius = request.limits.max_bead_width_mm.min(arm.length * 0.5);
    if adjacency.degree[arm.ends[0]] >= 3 && s < radius {
        return radius;
    }
    if adjacency.degree[arm.ends[1]] >= 3 && s > arm.length - radius {
        return arm.length - radius;
    }
    s
}
fn normal(
    graph: &Skeleton,
    adjacency: &Adjacency,
    arm: &Arm,
    s: f64,
    request: &WallRequest<'_>,
) -> Result<Allocation, ManufacturingError> {
    let s = field_parameter(adjacency, arm, s, request);
    let Some(field) = arm.count else {
        return raw(graph, arm, s, request);
    };
    let count = field.count();
    let target = 2 * request.recipe.perimeter_count as usize;
    let thickness = thickness(graph, arm, s);
    let preferred = source_width_mm(request)?;
    let preferred_core = count == target && thickness >= target as f64 * preferred;
    let width = match field {
        ArmCount::Source(_) => {
            if preferred_core {
                preferred
            } else {
                thickness / count as f64
            }
        }
        ArmCount::Uniform { width, .. } => width,
    };
    let residual = preferred_core && width == preferred;
    let [minimum, maximum] = admitted_width_range(request)?;
    if !width.is_finite() || !(minimum..=maximum).contains(&width) {
        return Err(unsupported(
            "whole-arm count field escaped its admitted width enclosure",
        ));
    }
    Ok(Allocation {
        count,
        width,
        residual,
    })
}
fn count_width(
    graph: &Skeleton,
    adjacency: &Adjacency,
    arm: &Arm,
    s: f64,
    count: usize,
    request: &WallRequest<'_>,
) -> Result<f64, ManufacturingError> {
    let [minimum, maximum] = admitted_width_range(request)?;
    let s = field_parameter(adjacency, arm, s, request);
    let target = 2 * request.recipe.perimeter_count as usize;
    let preferred = source_width_mm(request)?;
    let width = if count == target && thickness(graph, arm, s) >= target as f64 * preferred {
        preferred
    } else {
        (thickness(graph, arm, s) / count as f64).max(minimum)
    };
    if !width.is_finite() || width > maximum {
        let (curve, parameter, _) = locate(graph, arm, s);
        return Err(unsupported(&format!(
            "finite count transition exceeds local width envelope: count={count}, width={width}, admitted={minimum}..{maximum}, thickness={}, arm_s={s}/{}, parameter={parameter}, source_vertices={:?}, supports={:?}",
            thickness(graph, arm, s),
            arm.length,
            curve.vertex_ids,
            curve.supports,
        )));
    }
    Ok(width)
}

/// Enclose the complete frozen incoming thickness field, not just its samples.
/// Distance to a convex site is convex on a chord and is 1-Lipschitz in
/// position. The analytic medial chord bound therefore supplies both sides.
#[allow(clippy::too_many_arguments)]
fn span_thickness_bounds(
    graph: &Skeleton,
    adjacency: &Adjacency,
    arm: &Arm,
    span: Span,
    range: [f64; 2],
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<[f64; 2], ManufacturingError> {
    check_cancel(cancel)?;
    boolean::charge(work, 1)?;
    let s = range.map(|s| field_parameter(adjacency, arm, s, request));
    let endpoints = s.map(|s| thickness(graph, arm, s));
    let mut lower = endpoints[0].min(endpoints[1]);
    let mut upper = endpoints[0].max(endpoints[1]);
    if s[0] != s[1] {
        if s[0] < span.s[0] || s[1] > span.s[1] {
            return Err(unsupported("frozen arm field lost its source interval"));
        }
        let curve = &graph.edges[span.edge];
        let t = s.map(|s| {
            span.t[0] + (s - span.s[0]) / (span.s[1] - span.s[0]) * (span.t[1] - span.t[0])
        });
        let p = t.map(|t| curve.at(t));
        let chord = Line::new(Coord::from(p[0]), Coord::from(p[1]));
        let deviation = curve.chord_error_bound(t[0].min(t[1]), t[0].max(t[1]));
        let near = curve
            .supports
            .into_iter()
            .map(|site| match site.point {
                Some(point) => Euclidean.distance(&chord, &Point::from(point)),
                None => {
                    Euclidean.distance(&chord, &Line::new(Coord::from(site.a), Coord::from(site.b)))
                }
            })
            .sum::<f64>();
        let far = p.map(|p| {
            curve
                .supports
                .into_iter()
                .map(|site| site.radius(p))
                .sum::<f64>()
        });
        let scale = p
            .into_iter()
            .flatten()
            .chain(
                curve
                    .supports
                    .into_iter()
                    .flat_map(|site| site.a.into_iter().chain(site.b)),
            )
            .map(f64::abs)
            .fold(1.0, f64::max);
        let arithmetic = 256.0 * f64::EPSILON * scale;
        lower = lower.min((near - 2.0 * deviation - arithmetic).max(0.0));
        upper = upper.max(far[0].max(far[1]) + 2.0 * deviation + arithmetic);
    }
    if !lower.is_finite() || !upper.is_finite() || lower > upper {
        return Err(unsupported(
            "whole-arm thickness enclosure is not finite and ordered",
        ));
    }
    Ok([lower, upper])
}

fn arm_thickness_bounds(
    graph: &Skeleton,
    adjacency: &Adjacency,
    arm: &Arm,
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<[f64; 2], ManufacturingError> {
    let mut bounds = [f64::INFINITY, 0.0f64];
    for &span in arm.spans {
        let local =
            span_thickness_bounds(graph, adjacency, arm, span, span.s, request, work, cancel)?;
        bounds[0] = bounds[0].min(local[0]);
        bounds[1] = bounds[1].max(local[1]);
    }
    Ok(bounds)
}

#[allow(clippy::too_many_arguments)]
fn count_span_widths_admitted(
    graph: &Skeleton,
    incidence: SourceIncidence<'_>,
    arm: &Arm,
    span: Span,
    range: [f64; 2],
    count: usize,
    depth: u8,
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    let [minimum, maximum] = admitted_width_range(request)?;
    let target = 2 * request.recipe.perimeter_count as usize;
    let preferred = source_width_mm(request)?;
    let width = |thickness: f64| {
        let value = thickness / count as f64;
        if count == target {
            value.min(preferred)
        } else {
            value
        }
    };
    let bounds = span_thickness_bounds(
        graph,
        incidence.arms,
        arm,
        span,
        range,
        request,
        work,
        cancel,
    )?
    .map(width);
    if bounds[0] >= minimum && bounds[1] <= maximum {
        return Ok(true);
    }
    let middle = range[0] + (range[1] - range[0]) * 0.5;
    // Point witnesses can reject this proposal, never admit an interval.
    for s in [range[0], middle, range[1]] {
        boolean::charge(work, 1)?;
        let value = width(thickness(
            graph,
            arm,
            field_parameter(incidence.arms, arm, s, request),
        ));
        if !value.is_finite() || !(minimum..=maximum).contains(&value) {
            return Ok(false);
        }
    }
    if depth == MAX_REFINEMENT || middle == range[0] || middle == range[1] {
        return Ok(false);
    }
    Ok(count_span_widths_admitted(
        graph,
        incidence,
        arm,
        span,
        [range[0], middle],
        count,
        depth + 1,
        request,
        work,
        cancel,
    )? && count_span_widths_admitted(
        graph,
        incidence,
        arm,
        span,
        [middle, range[1]],
        count,
        depth + 1,
        request,
        work,
        cancel,
    )?)
}

/// Try finite source-count fields before conservative uniform-count proposals.
/// Uniform proposals fit the complete source-arm thickness lower enclosure;
/// neither field supplies required-material, source-port or overlap authority.
/// Actual-lane metrics and independent D/R/command qualification still follow.
fn whole_arm_counts(
    graph: &Skeleton,
    incidence: SourceIncidence<'_>,
    arm: &Arm,
    request: &WallRequest<'_>,
    live_bytes: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Vec<(f64, ArmCount)>, ManufacturingError> {
    let adjacency = incidence.arms;
    let target = 2 * request.recipe.perimeter_count as usize;
    if arm.spans.is_empty() || target == 0 {
        return Ok(Vec::new());
    }
    let [minimum, maximum] = admitted_width_range(request)?;
    let preferred = source_width_mm(request)?;
    let bounds = arm_thickness_bounds(graph, adjacency, arm, request, work, cancel)?;
    let mut choices = Vec::new();
    for count in 1..=target {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        let widths = bounds.map(|thickness| {
            let width = thickness / count as f64;
            if count == target {
                width.min(preferred)
            } else {
                width
            }
        });
        if widths[0] < minimum || widths[1] > maximum {
            let mut bounded = true;
            for &span in arm.spans {
                if !count_span_widths_admitted(
                    graph, incidence, arm, span, span.s, count, 0, request, work, cancel,
                )? {
                    bounded = false;
                    break;
                }
            }
            if !bounded {
                continue;
            }
        }
        let candidate = Arm {
            spans: arm.spans,
            ends: arm.ends,
            length: arm.length,
            count: Some(ArmCount::Source(count)),
        };
        let mut score = 0.0;
        let mut admitted = true;
        for &span in arm.spans {
            check_cancel(cancel)?;
            boolean::charge(work, 1)?;
            let ratio = metric_interval(
                graph, incidence, &candidate, span, None, span.s[0], span.s[1], 0, request, work,
                cancel,
            )?;
            if !ratio.is_finite() || ratio > 1.0 {
                admitted = false;
                break;
            }
            let [begin, end] = span
                .s
                .map(|s| normal(graph, adjacency, &candidate, s, request).map(|a| a.width));
            score += 0.5
                * ((begin? - preferred).abs() + (end? - preferred).abs())
                * (span.s[1] - span.s[0]);
        }
        if !score.is_finite() {
            return Err(unsupported("whole-arm width objective overflow"));
        }
        if admitted {
            if choices.len() == choices.capacity() {
                let next = choices.capacity().saturating_mul(2).max(4).min(target);
                let bytes = (choices.capacity() + next) * std::mem::size_of::<(f64, ArmCount)>();
                admit_stage(request, live_bytes + bytes)?;
                choices.reserve_exact(next - choices.len());
            }
            choices.push((score, ArmCount::Source(count)));
        }
    }
    if choices.is_empty() {
        for count in 1..=target {
            check_cancel(cancel)?;
            boolean::charge(work, 1)?;
            let mut width = (bounds[0] / count as f64).min(maximum);
            if count == target {
                width = width.min(preferred);
            }
            if !width.is_finite() || width < minimum {
                continue;
            }
            let field = ArmCount::Uniform { count, width };
            let candidate = Arm {
                spans: arm.spans,
                ends: arm.ends,
                length: arm.length,
                count: Some(field),
            };
            let mut admitted = true;
            for &span in arm.spans {
                let ratio = metric_interval(
                    graph, incidence, &candidate, span, None, span.s[0], span.s[1], 0, request,
                    work, cancel,
                )?;
                if !ratio.is_finite() || ratio > 1.0 {
                    admitted = false;
                    break;
                }
            }
            if !admitted {
                continue;
            }
            let score = (width - preferred).abs() * arm.length;
            if !score.is_finite() {
                return Err(unsupported("whole-arm width objective overflow"));
            }
            if choices.len() == choices.capacity() {
                let next = choices.capacity().saturating_mul(2).max(4).min(target);
                let bytes = (choices.capacity() + next) * std::mem::size_of::<(f64, ArmCount)>();
                admit_stage(request, live_bytes + bytes)?;
                choices.reserve_exact(next - choices.len());
            }
            choices.push((score, field));
        }
    }
    boolean::charge(
        work,
        choices
            .len()
            .saturating_mul((usize::BITS - choices.len().max(1).leading_zeros()) as usize + 1),
    )?;
    choices.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(a.1.count().cmp(&b.1.count())));
    check_cancel(cancel)?;
    Ok(choices)
}

fn plan_events(
    graph: &Skeleton,
    incidence: SourceIncidence<'_>,
    arm: &Arm,
    request: &WallRequest<'_>,
    events: &mut Vec<Event>,
    permissions: &mut Vec<Junction>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let adjacency = incidence.arms;
    if arm.spans.is_empty() {
        return Ok(());
    }
    let event_start = events.len();
    let mut previous = normal(graph, adjacency, arm, 0.0, request)?.count;
    for span in arm.spans {
        let next = normal(graph, adjacency, arm, span.s[1], request)?.count;
        if next != previous && next > 0 && previous > 0 {
            if next.abs_diff(previous) != 1 {
                return Err(unsupported(
                    "medial arm count change cannot be resolved as finite single-bead events",
                ));
            }
            let mut lo = span.s[0];
            let mut hi = span.s[1];
            for _ in 0..40 {
                boolean::charge(work, 1)?;
                let mid = (lo + hi) * 0.5;
                if normal(graph, adjacency, arm, mid, request)?.count == previous {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            let event_s = (lo + hi) * 0.5;
            let old = previous.min(next);
            let high = previous.max(next);
            let old_width = count_width(graph, adjacency, arm, event_s, old, request)?;
            let new_width = count_width(graph, adjacency, arm, event_s, high, request)?;
            // Medial distance is only a proposal coordinate. Reserve fitting
            // headroom, then solve against source-supported continuing lanes.
            let mut arm_gradient = program_coordinates::MAX_WALL_WIDTH_GRADIENT * 0.9;
            let mut length = ((old_width - new_width).abs() / arm_gradient)
                .max(program_coordinates::MIN_DEPOSITION_COMMAND_MM);
            let mut chosen = None;
            for _ in 0..32 {
                check_cancel(cancel)?;
                boolean::charge(work, 1)?;
                if length > arm.length {
                    let (curve, parameter, _) = locate(graph, arm, event_s);
                    return Err(unsupported(&format!(
                        "wall arm is too short for its finite actual-lane width/count transition: count={previous}->{next}, required_length={length}, available={}, objective_at={event_s}, widths={old_width}->{new_width}, thickness={:?}, parameter={parameter}, source_ends={:?}, supports={:?}",
                        arm.length,
                        [0.0, event_s, arm.length].map(|s| thickness(
                            graph,
                            arm,
                            field_parameter(adjacency, arm, s, request)
                        )),
                        arm.ends,
                        curve.supports,
                    )));
                }
                let mut begin = (event_s - length * 0.5).clamp(0.0, arm.length - length);
                if let Some(previous) = events[event_start..].last() {
                    begin = begin.max(previous.end);
                }
                let end = begin + length;
                if end > arm.length {
                    return Err(unsupported(
                        "neighboring finite wall transitions exceed their available arm length",
                    ));
                }
                let birth = if next > previous { begin } else { end };
                let finish = if next > previous { end } else { begin };
                let incoming = normal(graph, adjacency, arm, birth, request)?;
                let start_width = if incoming.count == old {
                    incoming.width
                } else {
                    count_width(graph, adjacency, arm, birth, old, request)?
                };
                let target_width = count_width(graph, adjacency, arm, finish, high, request)?;
                let needed = ((start_width - target_width).abs() / arm_gradient)
                    .max(program_coordinates::MIN_DEPOSITION_COMMAND_MM);
                if needed > length {
                    length = needed.next_up();
                    continue;
                }
                let event = Event {
                    begin,
                    end,
                    old,
                    high,
                    increasing: next > previous,
                    start_width,
                    arm_gradient,
                };
                let ratio =
                    actual_lane_metric(graph, incidence, arm, event, request, work, cancel)?;
                if ratio <= 1.0 {
                    chosen = Some(event);
                    break;
                }
                if !ratio.is_finite() {
                    return Err(unsupported(
                        "finite transition has an unresolved actual continuing-lane metric",
                    ));
                }
                arm_gradient /= ratio.next_up();
                length = ((start_width - target_width).abs() / arm_gradient)
                    .max(program_coordinates::MIN_DEPOSITION_COMMAND_MM)
                    .next_up();
            }
            let event = chosen.ok_or_else(|| unsupported("finite transition cannot converge inside the declared actual-bead gradient envelope"))?;
            let begin = event.begin;
            let end = event.end;
            if event_s < begin || event_s > end {
                count_width(graph, adjacency, arm, begin, previous, request)?;
            }
            let mut corridor = Vec::new();
            corridor.push(locate(graph, arm, begin).0.at(locate(graph, arm, begin).1));
            for span in arm.spans {
                if begin < span.s[1] && span.s[1] < end {
                    corridor.push(graph.edges[span.edge].at(span.t[1]));
                }
            }
            corridor.push(locate(graph, arm, end).0.at(locate(graph, arm, end).1));
            permissions.push(Junction {
                center: locate(graph, arm, event_s)
                    .0
                    .at(locate(graph, arm, event_s).1),
                radius_mm: request.limits.max_bead_width_mm,
                valence: 3,
                ports: Vec::new(),
                transition: Some(TransitionCorridor {
                    points: corridor,
                    incoming_count: previous,
                    outgoing_count: next,
                }),
            });
            events.push(event);
        }
        previous = next;
    }
    Ok(())
}

fn emit_arm(
    graph: &Skeleton,
    incidence: SourceIncidence<'_>,
    arm: &Arm,
    events: &[Event],
    junction_caps: &[f64],
    nominal_fill: &MultiPolygon<f64>,
    request: &WallRequest<'_>,
    output: &mut Vec<BoundaryEdge>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let mut breaks = Vec::with_capacity(arm.spans.len() + 2 * events.len() + 1);
    breaks.push(0.0);
    breaks.extend(arm.spans.iter().map(|span| span.s[1]));
    for event in events {
        breaks.extend([event.begin, event.end]);
    }
    breaks.sort_unstable_by(f64::total_cmp);
    breaks.dedup();
    for pair in breaks.windows(2) {
        check_cancel(cancel)?;
        emit_interval(
            graph, incidence, arm, events, pair[0], pair[1], 0, request, output, work, cancel,
        )?;
    }
    emit_junction_spines(
        graph,
        incidence,
        arm,
        events,
        &breaks,
        junction_caps,
        nominal_fill,
        request,
        output,
        work,
        cancel,
    )?;
    Ok(())
}

fn center_gap(
    graph: &Skeleton,
    incidence: SourceIncidence<'_>,
    arm: &Arm,
    events: &[Event],
    junction_caps: &[f64],
    nominal_fill: &MultiPolygon<f64>,
    s: f64,
    event: Option<Event>,
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<f64, ManufacturingError> {
    let adjacency = incidence.arms;
    let (curve, t, _) = locate(graph, arm, s);
    let p = curve.at(t);
    if nominal_fill.contains(&Point::from(p)) {
        return Ok(f64::NEG_INFINITY);
    }
    let allocation = field(graph, adjacency, arm, events, s, event, request)?;
    let mut gap = f64::INFINITY;
    for lane in lane_ids(allocation) {
        let q = lane_point(incidence, curve, t, lane, allocation, work, cancel)?;
        gap = gap.min(norm(sub(p, q)) - lane_width(lane, allocation) * 0.5);
    }
    for vertex in arm.ends {
        gap = gap.min(norm(sub(p, graph.junctions[vertex].point)) - junction_caps[vertex]);
    }
    // Existing central newborn/continuing endpoint caps are construction
    // ownership, not unallocated gaps. This also avoids a microscopic extra
    // junction stroke where an already-owned endcap covers the whole need.
    for &event in events {
        boolean::charge(work, 1)?;
        let birth = if event.increasing {
            event.begin
        } else {
            event.end
        };
        let positions = if event.old % 2 == 0 {
            [event.begin, event.end]
        } else {
            [birth; 2]
        };
        for at in positions {
            let (edge, t, _) = locate(graph, arm, at);
            let width = if event.old % 2 == 0 {
                count_width(graph, adjacency, arm, at, event.high, request)?
            } else {
                event.start_width
            };
            let radius = (width * 0.5 - super::super::OFFSET_TOLERANCE_MM).max(0.0);
            gap = gap.min(norm(sub(p, edge.at(t))) - radius);
        }
    }
    Ok(gap)
}

fn central(field: Field) -> bool {
    field.central && !field.split || field.newborn_width.is_some() && !field.split
}

fn emit_junction_spines(
    graph: &Skeleton,
    incidence: SourceIncidence<'_>,
    arm: &Arm,
    events: &[Event],
    breaks: &[f64],
    junction_caps: &[f64],
    nominal_fill: &MultiPolygon<f64>,
    request: &WallRequest<'_>,
    output: &mut Vec<BoundaryEdge>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let adjacency = incidence.arms;
    let width = admitted_width_range(request)?[0];
    let reach = (request.limits.max_bead_width_mm
        - width * 0.5
        - FIT_ERROR_MM
        - program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM
        - 0.0001)
        .min(arm.length);
    if reach < program_coordinates::MIN_DEPOSITION_COMMAND_MM {
        return Ok(());
    }
    let mut permissions = [[0.0, 0.0]; 2];
    let mut active = [false; 2];
    for side in 0..2 {
        let vertex = arm.ends[side];
        let s = if side == 0 { 0.0 } else { arm.length };
        let allocation = normal(graph, adjacency, arm, s, request)?;
        if adjacency.degree[vertex] >= 3
            && allocation.count > 0
            && allocation.count % 2 == 0
            && !nominal_fill.contains(&Point::from(graph.junctions[vertex].point))
        {
            active[side] = true;
            permissions[side] = if side == 0 {
                [0.0, reach]
            } else {
                [arm.length - reach, arm.length]
            };
        }
    }
    if active[0] && active[1] && permissions[0][1] >= permissions[1][0] {
        permissions[0][1] = arm.length;
        active[1] = false;
    }
    let mut free = Vec::<[f64; 2]>::new();
    let mut spines = Vec::<[f64; 2]>::new();
    for side in 0..2 {
        if !active[side] {
            continue;
        }
        free.clear();
        // Subtract every already-owned central interval BEFORE allocating any
        // completion, including the central child of an even-to-odd event.
        for pair in breaks.windows(2) {
            let lo = pair[0].max(permissions[side][0]);
            let hi = pair[1].min(permissions[side][1]);
            if lo >= hi {
                continue;
            }
            let midpoint = (lo + hi) * 0.5;
            let event = events
                .iter()
                .copied()
                .find(|event| event.begin <= midpoint && midpoint <= event.end);
            if central(field(
                graph, adjacency, arm, events, midpoint, event, request,
            )?) {
                continue;
            }
            if let Some(last) = free.last_mut().filter(|last| last[1] == lo) {
                last[1] = hi;
            } else {
                free.push([lo, hi]);
            }
        }
        for &[begin, end] in &free {
            for pair in breaks.windows(2) {
                check_cancel(cancel)?;
                boolean::charge(work, 1)?;
                let lo = pair[0].max(begin);
                let hi = pair[1].min(end);
                if lo >= hi {
                    continue;
                }
                let midpoint = (lo + hi) * 0.5;
                let event = events
                    .iter()
                    .copied()
                    .find(|event| event.begin <= midpoint && midpoint <= event.end);
                let mut needed = false;
                for at in [lo, midpoint, hi] {
                    needed |= center_gap(
                        graph,
                        incidence,
                        arm,
                        events,
                        junction_caps,
                        nominal_fill,
                        at,
                        event,
                        request,
                        work,
                        cancel,
                    )? > super::super::OFFSET_TOLERANCE_MM;
                }
                if !needed {
                    continue;
                }
                let minimum = program_coordinates::MIN_DEPOSITION_COMMAND_MM + 0.000003;
                if end - begin < minimum {
                    return Err(unsupported(
                        "required junction completion has no unowned minimum-length command interval",
                    ));
                }
                let length = (hi - lo).max(minimum);
                let start = (midpoint - length * 0.5).clamp(begin, end - length);
                let interval = [start, start + length];
                if let Some(last) = spines.last_mut().filter(|last| last[1] >= interval[0]) {
                    last[1] = last[1].max(interval[1]);
                } else {
                    spines.push(interval);
                }
            }
        }
    }
    for [begin, end] in spines {
        for span in arm.spans {
            check_cancel(cancel)?;
            boolean::charge(work, 1)?;
            let lo = span.s[0].max(begin);
            let hi = span.s[1].min(end);
            if lo >= hi {
                continue;
            }
            let curve = &graph.edges[span.edge];
            let parameter = |s: f64| {
                span.t[0] + (s - span.s[0]) / (span.s[1] - span.s[0]) * (span.t[1] - span.t[0])
            };
            push_edge(
                output,
                [curve.at(parameter(lo)), curve.at(parameter(hi))],
                width,
                curve.supports.map(|site| site.boundary_index),
                None,
                usize::MAX,
                [width; 2],
                request,
            )?;
        }
    }
    Ok(())
}

fn field(
    graph: &Skeleton,
    adjacency: &Adjacency,
    arm: &Arm,
    events: &[Event],
    s: f64,
    event: Option<Event>,
    request: &WallRequest<'_>,
) -> Result<Field, ManufacturingError> {
    if let Some(event) = event {
        let birth_distance = if event.increasing {
            s - event.begin
        } else {
            event.end - s
        };
        let new_width = count_width(graph, adjacency, arm, s, event.high, request)?;
        let continuing_width =
            (event.start_width - event.arm_gradient * birth_distance).max(new_width);
        return Ok(Field {
            pairs: event.old / 2,
            central: event.old % 2 == 1,
            continuing_width,
            newborn_width: Some(new_width),
            split: event.old % 2 == 1,
            split_start: birth_distance == 0.0,
        });
    }
    let normal = normal(graph, adjacency, arm, s, request)?;
    let mut count = normal.count;
    // A staged delayed event controls count until its complete finite corridor
    // ends, even if a pointwise nominal objective would change it sooner.
    for event in events {
        if s < event.begin {
            break;
        }
        if s >= event.end {
            count = if event.increasing {
                event.high
            } else {
                event.old
            };
        }
    }
    if count == 0 {
        return Ok(Field {
            pairs: 0,
            central: false,
            continuing_width: 0.0,
            newborn_width: None,
            split: false,
            split_start: false,
        });
    }
    let width = if count == normal.count {
        normal.width
    } else {
        count_width(graph, adjacency, arm, s, count, request)?
    };
    Ok(Field {
        pairs: count / 2,
        central: count % 2 == 1,
        continuing_width: width,
        newborn_width: None,
        split: false,
        split_start: false,
    })
}

#[derive(Clone, Copy)]
enum Lane {
    Side(usize, usize),
    Center,
    Split,
    Newborn,
}
fn lane_ids(field: Field) -> impl Iterator<Item = Lane> {
    (0..2)
        .flat_map(move |side| (0..field.pairs).map(move |ordinal| Lane::Side(side, ordinal)))
        .chain(field.central.then_some(if field.split {
            Lane::Split
        } else {
            Lane::Center
        }))
        .chain(field.newborn_width.map(|_| Lane::Newborn))
}
fn lane_width(lane: Lane, field: Field) -> f64 {
    if matches!(lane, Lane::Newborn) {
        field
            .newborn_width
            .expect("newborn lane belongs to a reserved event")
    } else {
        field.continuing_width
    }
}
fn same_literal_site(a: &BoundarySupport, b: &BoundarySupport) -> bool {
    a.boundary_index == b.boundary_index
        && a.segment_index == b.segment_index
        && a.a == b.a
        && a.b == b.b
        && a.point == b.point
}

fn canonical_foot(
    incidence: SourceIncidence<'_>,
    curve: &MedialEdge,
    t: f64,
    site: BoundarySupport,
    p: [f64; 2],
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<[f64; 2], ManufacturingError> {
    let vertex = if t == 0.0 {
        Some(curve.vertex_ids[0])
    } else if t == 1.0 {
        Some(curve.vertex_ids[1])
    } else {
        None
    };
    if site.point.is_none() {
        if let Some(vertex) = vertex {
            let mut focus = None;
            let mut retain_focus = |point| -> Result<(), ManufacturingError> {
                if focus.is_some_and(|prior| prior != point) {
                    return Err(unsupported(
                        "original source-node segment has ambiguous declared endpoint feet",
                    ));
                }
                focus = Some(point);
                Ok(())
            };
            let mut pairs = [None; 2];
            let mut incident_count = 0usize;
            for incident in incidence.at(vertex) {
                check_cancel(cancel)?;
                boolean::charge(work, 2)?;
                let supports = &incidence.graph.edges[incident.edge].supports;
                if incident_count < pairs.len() {
                    pairs[incident_count] = Some(supports);
                }
                incident_count += 1;
                for other in supports {
                    if let Some(point) = other.point {
                        if other.boundary_index == site.boundary_index
                            && (point == site.a || point == site.b)
                        {
                            retain_focus(point)?;
                        }
                    }
                }
            }
            if incident_count == 2 {
                boolean::charge(work, 16)?;
                if let [Some(a), Some(b)] = pairs {
                    // A degree-two site switch {A,B}->{A,C} has no retained
                    // B/C primary branch. Only its UNshared ports may use a
                    // unique literal B/C boundary endpoint. In particular,
                    // quantized collinearity does not authorize coalescing the
                    // original noncollinear segments or moving source vertices.
                    let common = a
                        .iter()
                        .filter(|x| b.iter().any(|y| same_literal_site(x, y)))
                        .count();
                    let in_a = a.iter().any(|x| same_literal_site(x, &site));
                    let in_b = b.iter().any(|x| same_literal_site(x, &site));
                    if common == 1 && in_a != in_b {
                        let (present, opposite) = if in_a { (a, b) } else { (b, a) };
                        for other in opposite {
                            if other.point.is_some()
                                || other.boundary_index != site.boundary_index
                                || present.iter().any(|x| same_literal_site(x, other))
                            {
                                continue;
                            }
                            for point in [site.a, site.b] {
                                if point == other.a || point == other.b {
                                    retain_focus(point)?;
                                }
                            }
                        }
                    }
                }
            }
            if let Some(point) = focus {
                return Ok(point);
            }
        }
    }
    Ok(site.foot(p))
}
fn lane_point(
    incidence: SourceIncidence<'_>,
    curve: &MedialEdge,
    t: f64,
    lane: Lane,
    field: Field,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<[f64; 2], ManufacturingError> {
    let p = curve.at(t);
    if matches!(lane, Lane::Split) && field.split_start {
        return Ok(p);
    }
    let (side, distance) = match lane {
        Lane::Center => return Ok(p),
        Lane::Side(side, ordinal) => (side, (ordinal as f64 + 0.5) * field.continuing_width),
        Lane::Split => (1, (field.pairs as f64 + 0.5) * field.continuing_width),
        Lane::Newborn if field.split => (
            0,
            field.pairs as f64 * field.continuing_width + field.newborn_width.unwrap() * 0.5,
        ),
        Lane::Newborn => return Ok(p),
    };
    let foot = canonical_foot(incidence, curve, t, curve.supports[side], p, work, cancel)?;
    Ok(add(foot, mul(sub(p, foot), distance / norm(sub(p, foot)))))
}

fn actual_lane_metric(
    graph: &Skeleton,
    incidence: SourceIncidence<'_>,
    arm: &Arm,
    event: Event,
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<f64, ManufacturingError> {
    let mut worst = 0.0f64;
    for &span in arm.spans {
        let lo = span.s[0].max(event.begin);
        let hi = span.s[1].min(event.end);
        if lo < hi {
            worst = worst.max(metric_interval(
                graph,
                incidence,
                arm,
                span,
                Some(event),
                lo,
                hi,
                0,
                request,
                work,
                cancel,
            )?);
        }
    }
    Ok(worst)
}

#[allow(clippy::too_many_arguments)]
fn metric_interval(
    graph: &Skeleton,
    incidence: SourceIncidence<'_>,
    arm: &Arm,
    span: Span,
    event: Option<Event>,
    lo: f64,
    hi: f64,
    depth: u8,
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<f64, ManufacturingError> {
    let adjacency = incidence.arms;
    check_cancel(cancel)?;
    boolean::charge(work, 1)?;
    let middle = (lo + hi) * 0.5;
    let curve = &graph.edges[span.edge];
    let parameter =
        |s: f64| span.t[0] + ((s - span.s[0]) / (span.s[1] - span.s[0])) * (span.t[1] - span.t[0]);
    let ts = [parameter(lo), parameter(middle), parameter(hi)];
    let fields = [
        field(graph, adjacency, arm, &[], lo, event, request)?,
        field(graph, adjacency, arm, &[], middle, event, request)?,
        field(graph, adjacency, arm, &[], hi, event, request)?,
    ];
    let chord = curve.chord_error_bound(ts[0].min(ts[2]), ts[0].max(ts[2]));
    let mut fit = true;
    let mut worst = 0.0f64;
    for lane in lane_ids(fields[1]) {
        let widths = fields.map(|field| lane_width(lane, field));
        let points = [
            lane_point(incidence, curve, ts[0], lane, fields[0], work, cancel)?,
            lane_point(incidence, curve, ts[1], lane, fields[1], work, cancel)?,
            lane_point(incidence, curve, ts[2], lane, fields[2], work, cancel)?,
        ];
        fit &= norm(sub(points[1], mul(add(points[0], points[2]), 0.5)))
            + chord
            + (widths[2] - widths[0]).abs() * 0.25
            <= FIT_ERROR_MM;
        for index in 0..2 {
            let change = (widths[index + 1] - widths[index]).abs();
            if change == 0.0 {
                continue;
            }
            let actual_lane_length = norm(sub(points[index + 1], points[index]));
            worst = worst.max(
                change / (program_coordinates::MAX_WALL_WIDTH_GRADIENT * 0.95 * actual_lane_length),
            );
        }
    }
    if fit {
        return Ok(worst);
    }
    // An unresolved candidate is not a finite admitted metric. Event callers
    // retain their typed refusal; whole-arm enumeration may consider another
    // count without swallowing an input, cancellation or resource error.
    if depth == MAX_REFINEMENT {
        return Ok(f64::INFINITY);
    }
    Ok(metric_interval(
        graph,
        incidence,
        arm,
        span,
        event,
        lo,
        middle,
        depth + 1,
        request,
        work,
        cancel,
    )?
    .max(metric_interval(
        graph,
        incidence,
        arm,
        span,
        event,
        middle,
        hi,
        depth + 1,
        request,
        work,
        cancel,
    )?))
}

/// A neighboring interval can retain the FULL continuing source radius at a
/// shared endpoint. Capping both contemporaneous chords to their mutual
/// distance spends the same clearance twice when subdivisions are uneven.
/// Reserve each original endpoint radius against the opposite XYZ6 chord;
/// endpoint fields stay immutable, and fitting subsequently contains these
/// packed source capsules rather than reconstructing their nominal widths.
fn ordinary_packing_width(pair: &[Line<f64>; 2], widths: [f64; 2]) -> f64 {
    if Euclidean.distance(&pair[0], &pair[1]) <= 0.0 {
        return 0.0;
    }
    let magnitude = pair
        .iter()
        .flat_map(|line| [line.start.x, line.start.y, line.end.x, line.end.y])
        .fold(1.0_f64, |m, value| m.max(value.abs()));
    let arithmetic = 128.0 * f64::EPSILON * magnitude;
    let proposed = widths[0].min(widths[1]);
    let mut packed = f64::INFINITY;
    for side in 0..2 {
        for (endpoint, width) in [pair[side].start, pair[side].end].into_iter().zip(widths) {
            let distance = Euclidean.distance(&Point::from(endpoint), &pair[1 - side]);
            let upper = (distance + arithmetic).next_up();
            let upper_width = ((2.0 * upper).next_up() - width).next_up();
            // As before, an arithmetic enclosure straddling the source width
            // retains it for independent actual-capsule qualification. This
            // proposal supplies no new overlap or junction authority.
            if upper_width < proposed {
                let lower = (distance - arithmetic).next_down().max(0.0);
                packed = packed.min(((2.0 * lower).next_down() - width).next_down());
            }
        }
    }
    packed
}

#[allow(clippy::too_many_arguments)]
fn emit_interval(
    graph: &Skeleton,
    incidence: SourceIncidence<'_>,
    arm: &Arm,
    events: &[Event],
    lo: f64,
    hi: f64,
    depth: u8,
    request: &WallRequest<'_>,
    output: &mut Vec<BoundaryEdge>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let adjacency = incidence.arms;
    check_cancel(cancel)?;
    boolean::charge(work, 1)?;
    let middle = (lo + hi) * 0.5;
    let event = events
        .iter()
        .copied()
        .find(|event| event.begin <= middle && middle <= event.end);
    let center_field = field(graph, adjacency, arm, events, middle, event, request)?;
    if center_field.pairs == 0 && !center_field.central && center_field.newborn_width.is_none() {
        return Ok(());
    }
    let (curve, _, _) = locate(graph, arm, middle);
    let span = arm.spans[locate(graph, arm, middle).2];
    let parameter =
        |s: f64| span.t[0] + ((s - span.s[0]) / (span.s[1] - span.s[0])) * (span.t[1] - span.t[0]);
    let ts = [parameter(lo), parameter(hi)];
    let fields = [
        field(graph, adjacency, arm, events, lo, event, request)?,
        field(graph, adjacency, arm, events, hi, event, request)?,
    ];
    let lanes = || lane_ids(center_field);
    let bound = curve.chord_error_bound(ts[0].min(ts[1]), ts[0].max(ts[1]));
    let mut fit = true;
    for lane in lanes() {
        let widths = fields.map(|field| lane_width(lane, field));
        let points = [
            lane_point(incidence, curve, ts[0], lane, fields[0], work, cancel)?,
            lane_point(incidence, curve, ts[1], lane, fields[1], work, cancel)?,
        ];
        let midpoint = lane_point(
            incidence,
            curve,
            parameter(middle),
            lane,
            center_field,
            work,
            cancel,
        )?;
        fit &= norm(sub(midpoint, mul(add(points[0], points[1]), 0.5)))
            + bound
            + (widths[1] - widths[0]).abs() * 0.5
            <= FIT_ERROR_MM;
    }
    if !fit {
        if depth == MAX_REFINEMENT {
            return Err(unsupported(
                "finite beading field cannot meet its commanded center/width enclosure",
            ));
        }
        emit_interval(
            graph,
            incidence,
            arm,
            events,
            lo,
            middle,
            depth + 1,
            request,
            output,
            work,
            cancel,
        )?;
        return emit_interval(
            graph,
            incidence,
            arm,
            events,
            middle,
            hi,
            depth + 1,
            request,
            output,
            work,
            cancel,
        );
    }
    let ids = curve.supports.map(|site| site.boundary_index);
    // Every ordinary neighboring lane pair must fit its actual XYZ6 source
    // chords: same-side offsets around point sites are not parallel lines,
    // just as opposing nonparallel supports are not a packing thickness.
    // Finite births retain their independently predeclared corridor authority.
    let mut packing_width = f64::INFINITY;
    if event.is_none() && center_field.pairs > 0 {
        let line = |lane, work: &mut usize| -> Result<Line<f64>, ManufacturingError> {
            let a = point(key(lane_point(
                incidence, curve, ts[0], lane, fields[0], work, cancel,
            )?)?);
            let b = point(key(lane_point(
                incidence, curve, ts[1], lane, fields[1], work, cancel,
            )?)?);
            Ok(Line::new(Coord::from(a), Coord::from(b)))
        };
        let mut consider = |a, b| -> Result<(), ManufacturingError> {
            check_cancel(cancel)?;
            boolean::charge(work, 1)?;
            let pair = [line(a, work)?, line(b, work)?];
            let radius = fields[0].continuing_width.max(fields[1].continuing_width) * 0.5;
            let authorized = arm.ends.into_iter().any(|vertex| {
                adjacency.degree[vertex] >= 3
                    && pair.iter().all(|line| {
                        qualification::junction_contains_capsule(
                            graph.junctions[vertex].point,
                            [[line.start.x, line.start.y], [line.end.x, line.end.y]],
                            radius,
                            request.limits.max_bead_width_mm,
                        )
                    })
            });
            if !authorized {
                boolean::charge(work, 4)?;
                packing_width = packing_width.min(ordinary_packing_width(
                    &pair,
                    fields.map(|field| field.continuing_width),
                ));
            }
            Ok(())
        };
        for side in 0..2 {
            for ordinal in 1..center_field.pairs {
                consider(Lane::Side(side, ordinal - 1), Lane::Side(side, ordinal))?;
            }
        }
        let inner = center_field.pairs - 1;
        if center_field.central {
            let center = if center_field.split {
                Lane::Split
            } else {
                Lane::Center
            };
            consider(Lane::Side(0, inner), center)?;
            consider(center, Lane::Side(1, inner))?;
        } else {
            consider(Lane::Side(0, inner), Lane::Side(1, inner))?;
        }
    }
    if packing_width <= 0.0 {
        return Err(unsupported(&format!(
            "ordinary adjacent-lane packing crosses: sites={:?}, parameters={ts:?}, pairs={}, width={}, medial={:?}",
            curve.supports,
            center_field.pairs,
            center_field.continuing_width,
            [curve.at(ts[0]), curve.at(ts[1])]
        )));
    }
    for lane in lanes() {
        let points = [
            lane_point(incidence, curve, ts[0], lane, fields[0], work, cancel)?,
            lane_point(incidence, curve, ts[1], lane, fields[1], work, cancel)?,
        ];
        // A constant command takes the inward width enclosure. Endpoint-mean
        // widths can overlap an adjacent touching lane along half this command,
        // outside any source-authorized birth/junction corridor.
        let width = lane_width(lane, fields[0])
            .min(lane_width(lane, fields[1]))
            .min(packing_width);
        let (facing, ordinal) = match lane {
            Lane::Side(side, ordinal) => (Some(ids[side]), ordinal),
            Lane::Split => (Some(ids[1]), center_field.pairs),
            Lane::Newborn if center_field.split => (Some(ids[0]), center_field.pairs),
            _ => (None, center_field.pairs),
        };
        push_edge(
            output,
            points,
            width,
            ids,
            facing,
            ordinal,
            fields.map(|field| lane_width(lane, field)),
            request,
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod corner_width_tests {
    use super::*;
    use geo::polygon;

    #[test]
    fn uneven_source_intervals_keep_neighbor_clearance_through_assembly_and_fitting() {
        let widths = [0.45; 2];
        let angles = [-0.01_f64, 0.0, 0.03];
        let lines = std::array::from_fn::<_, 2, _>(|interval| {
            [1.0, 1.45].map(|radius| {
                let ends = [angles[interval], angles[interval + 1]]
                    .map(|angle| point(key([radius * angle.cos(), radius * angle.sin()]).unwrap()));
                Line::new(Coord::from(ends[0]), Coord::from(ends[1]))
            })
        });
        let old_caps = lines.map(|pair| Euclidean.distance(&pair[0], &pair[1]).min(widths[0]));
        let crossing = Euclidean.distance(&lines[0][0], &lines[1][1]);
        assert!(
            crossing < (old_caps[0] + old_caps[1]) * 0.5,
            "independently sharing each interval's deficit overlaps a neighbor's endpoint cap"
        );
        let caps = lines.map(|pair| ordinary_packing_width(&pair, widths).min(widths[0]));
        let mut sources = Vec::new();
        for (interval, pair) in lines.iter().enumerate() {
            for (ordinal, line) in pair.iter().enumerate() {
                sources.push(BoundaryEdge {
                    points: [[line.start.x, line.start.y], [line.end.x, line.end.y]],
                    width_mm: caps[interval],
                    source_widths: widths,
                    supports: [0, 0],
                    facing_boundary: Some(0),
                    ordinal,
                });
            }
        }
        // Include cross-interval pairs, not just the two pairs used to propose
        // caps. Immutable continuing endpoint fields are not overwritten.
        for a in sources.iter().filter(|edge| edge.ordinal == 0) {
            for b in sources.iter().filter(|edge| edge.ordinal == 1) {
                let line = |edge: &BoundaryEdge| {
                    Line::new(Coord::from(edge.points[0]), Coord::from(edge.points[1]))
                };
                assert!(Euclidean.distance(&line(a), &line(b)) >= (a.width_mm + b.width_mm) * 0.5);
                assert_eq!(a.source_widths, widths);
                assert_eq!(b.source_widths, widths);
            }
        }
        let material = MultiPolygon(vec![polygon![
            (x: 0.0, y: -1.0), (x: 3.0, y: -1.0),
            (x: 3.0, y: 1.0), (x: 0.0, y: 1.0),
        ]]);
        let required = region::RequiredMaterial::from_section(&material);
        let mut recipe = crate::planner::tests::recipe(0.25);
        recipe.bead_width_mm = widths[0];
        let limits = DepositionLimits {
            min_bead_width_mm: 0.30,
            max_bead_width_mm: 0.75,
            min_bead_height_mm: 0.000001,
            max_bead_height_mm: 0.30,
        };
        let request = WallRequest {
            material: &material,
            required: &required,
            recipe: &recipe,
            limits: &limits,
            nozzle_diameter_mm: 0.4,
            lower_z: 0.0,
            upper_z: 0.2,
            retained_points: region::point_count(&material),
            owned_bytes: 0,
        };
        let (paths, associations) = assemble(
            &sources,
            &[],
            &request,
            false,
            &mut 0,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(paths.len(), 2, "both original continuing lanes survive");
        let packed_minimum = caps[0].min(caps[1]);
        for path in &paths {
            for edge in path.checked_edges().unwrap() {
                assert!(edge.width_mm <= packed_minimum && edge.width_mm >= 0.302);
                assert!(
                    norm(sub(
                        [edge.points_mm[1][0], edge.points_mm[1][1]],
                        [edge.points_mm[0][0], edge.points_mm[0][1]],
                    )) >= program_coordinates::MIN_DEPOSITION_COMMAND_MM
                );
            }
        }
        assert!(associations.iter().all(|owner| {
            owner.supports == [0, 0] && owner.facing_boundary == Some(0) && owner.ordinal < 2
        }));
        for a in paths[0].checked_edges().unwrap() {
            for b in paths[1].checked_edges().unwrap() {
                let line = |edge: &DepositionEdge<'_>| {
                    Line::new(
                        [edge.points_mm[0][0], edge.points_mm[0][1]],
                        [edge.points_mm[1][0], edge.points_mm[1][1]],
                    )
                };
                assert!(
                    Euclidean.distance(&line(&a), &line(&b)) >= (a.width_mm + b.width_mm) * 0.5
                );
            }
        }
    }

    #[test]
    fn thick_layer_resolves_source_count_depth_birth_and_packing_before_emission() {
        let material = MultiPolygon(vec![polygon![
            (x: 0.0, y: 0.0), (x: 20.0, y: 0.0),
            (x: 20.0, y: 12.0), (x: 0.0, y: 12.0),
        ]]);
        let required = region::RequiredMaterial::from_section(&material);
        let limits = DepositionLimits {
            min_bead_width_mm: 0.30,
            max_bead_width_mm: 0.90,
            min_bead_height_mm: 0.000001,
            max_bead_height_mm: 0.60,
        };
        let cancel = AtomicBool::new(false);
        let mut work = 0;
        let graph = skeleton::construct(
            &material,
            region::point_count(&material),
            0,
            &mut work,
            &cancel,
        )
        .unwrap();
        for (preferred, height) in [(0.45, 0.1), (0.6, 0.599999833)] {
            let mut recipe = crate::planner::tests::recipe(0.25);
            recipe.bead_width_mm = preferred;
            recipe.layer_height_mm = height;
            recipe.perimeter_count = 2;
            let request = WallRequest {
                material: &material,
                required: &required,
                recipe: &recipe,
                limits: &limits,
                nozzle_diameter_mm: 0.4,
                lower_z: 0.0,
                upper_z: height,
                retained_points: region::point_count(&material),
                owned_bytes: 0,
            };
            let minimum = admitted_width_range(&request).unwrap()[0];
            let resolved = source_width_mm(&request).unwrap();
            assert_eq!(resolved, preferred.max(minimum));
            assert_eq!(request.recipe.bead_width_mm, preferred);
            let allocation = allocate(6.0, &request).unwrap();
            assert_eq!(allocation.count, 4);
            assert_eq!(allocation.width, resolved);
            assert!(allocation.residual);
            let cells = contours::cells(&graph, &request, &mut work, &cancel).unwrap();
            let adjacency = Adjacency::new(&graph, &cells, false, &mut work, &cancel).unwrap();
            let contour_adjacency =
                Adjacency::new(&graph, &cells, true, &mut work, &cancel).unwrap();
            let incidence = SourceIncidence {
                graph: &graph,
                arms: &adjacency,
                contours: &contour_adjacency,
            };
            let (edge, curve) = graph
                .edges
                .iter()
                .enumerate()
                .find(|(edge, curve)| {
                    cells[*edge].is_none()
                        && curve.supports.iter().all(|site| site.point.is_none())
                        && curve
                            .endpoints
                            .iter()
                            .all(|p| (curve.supports[0].radius(*p) - 6.0).abs() < 1e-6)
                })
                .unwrap();
            let length = norm(sub(curve.endpoints[1], curve.endpoints[0]));
            let spans = [Span {
                edge,
                t: [0.0, 1.0],
                s: [0.0, length],
            }];
            let arm = Arm {
                spans: &spans,
                ends: curve.vertex_ids,
                length,
                count: Some(ArmCount::Source(4)),
            };
            let allocation = normal(&graph, &adjacency, &arm, length * 0.5, &request).unwrap();
            assert_eq!(allocation.width, resolved);
            assert_eq!(
                count_width(&graph, &adjacency, &arm, length * 0.5, 4, &request).unwrap(),
                resolved,
            );
            let mut output = Vec::new();
            emit_interval(
                &graph,
                incidence,
                &arm,
                &[],
                length * 0.25,
                length * 0.75,
                0,
                &request,
                &mut output,
                &mut work,
                &cancel,
            )
            .unwrap();
            assert!(!output.is_empty());
            for edge in output {
                assert!(edge.width_mm >= minimum && edge.width_mm <= resolved);
                assert_eq!(edge.source_widths, [resolved; 2]);
                assert!(edge.ordinal < recipe.perimeter_count as usize);
            }
            // Adjacent source cells use the SAME width for their contour depth
            // and finite birth root, never the below-floor authored preference.
            let depth = 1.5 * resolved;
            let curve = graph
                .edges
                .iter()
                .find(|curve| {
                    adjacent_supports(curve.supports)
                        && curve.supports[0].radius(curve.endpoints[0]) > depth
                        && curve.supports[0].radius(curve.endpoints[1]) < depth
                })
                .unwrap();
            let seed = refinement_allocation(curve, 0.5, &request).unwrap();
            assert_eq!(seed.width, resolved);
            let values = curve.endpoints.map(|p| curve.supports[0].radius(p) - depth);
            let event = corner_birth(
                curve,
                contours::OuterWidth::new([resolved; 2], resolved, None),
                1,
                [0.0, 1.0],
                values,
                &mut work,
                &cancel,
            )
            .unwrap();
            assert!(
                (curve.supports[0].radius(curve.at(event.parameter)) - depth).abs() <= FIT_ERROR_MM
            );
        }
    }

    #[test]
    fn nominal_inner_turns_preserve_material_and_protected_hole() {
        let material = MultiPolygon(vec![geo::Polygon::new(
            geo::LineString::from(vec![
                (0.0, 0.0),
                (20.0, 0.0),
                (20.0, 20.0),
                (0.0, 20.0),
                (0.0, 0.0),
            ]),
            vec![geo::LineString::from(vec![
                (8.0, 8.0),
                (8.0, 12.0),
                (12.0, 12.0),
                (12.0, 8.0),
                (8.0, 8.0),
            ])],
        )]);
        let mut recipe = crate::planner::tests::recipe(0.25);
        recipe.bead_width_mm = 0.45;
        recipe.perimeter_count = 2;
        let limits = DepositionLimits {
            min_bead_width_mm: 0.30,
            max_bead_width_mm: 0.70,
            min_bead_height_mm: 0.000001,
            max_bead_height_mm: 0.30,
        };
        let required = region::RequiredMaterial::from_section(&material);
        let request = WallRequest {
            material: &material,
            required: &required,
            recipe: &recipe,
            limits: &limits,
            nozzle_diameter_mm: 0.4,
            lower_z: 0.0,
            upper_z: 0.2,
            retained_points: region::point_count(&material),
            owned_bytes: 0,
        };
        let plan = plan_section_walls(request, &mut 0, &AtomicBool::new(false)).unwrap();
        assert_eq!(plan.protected_hole_area_mm2, 16.0);
        assert!(plan.correspondence.iter().any(|owner| owner.ordinal == 0));
        assert!(plan.correspondence.iter().any(|owner| owner.ordinal == 1));
        for path in plan.paths {
            for edge in path.checked_edges().unwrap() {
                let length = norm(sub(
                    [edge.points_mm[1][0], edge.points_mm[1][1]],
                    [edge.points_mm[0][0], edge.points_mm[0][1]],
                ));
                assert!(length >= program_coordinates::MIN_DEPOSITION_COMMAND_MM);
                assert!(edge.width_mm >= admitted_width_range(&request).unwrap()[0]);
                assert!(edge.width_mm <= limits.max_bead_width_mm);
            }
        }
    }
}
