// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Connect checked native graph cells through shared roots, then select material
//! from original directed trim crossings. Artificial cuts never become BREP IDs.
use super::{
    Admission, SectionChartEndpoint, SectionCurve, SectionEndpoint, SectionGraphChart,
    SectionQuery, SectionSpan,
    atlas::{self, GraphCell},
    cuts::{CutInventory, CutRootId},
    direct::{Arc, EdgeRoot, Event, Graph},
    indeterminate, resource_error,
    roots::sign,
};
use crate::{GeometryError, GeometryWorkBudget, PlaneMm, parametric::Interval};
use solidcore::{FaceId, surface::surface::Surface};
use spiling_contracts::geometry::RigidPoseMm;
use std::sync::atomic::AtomicBool;

#[derive(Clone, Copy, Debug)]
enum NodeKind {
    Cut(CutRootId),
    Trim(usize),
}
struct Node {
    kind: NodeKind,
    trim: Option<usize>,
    adjacent: [Option<usize>; 2],
}
impl Node {
    fn new(kind: NodeKind, trim: Option<usize>) -> Self {
        Self {
            kind,
            trim,
            adjacent: [None; 2],
        }
    }
    fn attach(&mut self, segment: usize) -> Result<(), GeometryError> {
        let slot = self
            .adjacent
            .iter_mut()
            .find(|entry| entry.is_none())
            .ok_or_else(|| indeterminate("native atlas carrier root has branching incidence"))?;
        *slot = Some(segment);
        Ok(())
    }
    fn uv(&self, cuts: &CutInventory<'_>, events: &[Event]) -> [Interval; 2] {
        match self.kind {
            NodeKind::Cut(id) => cuts.root(id).uv,
            NodeKind::Trim(index) => events[index].uv,
        }
    }
    fn point(&self, cuts: &CutInventory<'_>, events: &[Event], roots: &[EdgeRoot]) -> [f64; 3] {
        if let Some(index) = self.trim {
            return roots[events[index].root_index].point;
        }
        match self.kind {
            NodeKind::Cut(id) => cuts.root(id).point,
            NodeKind::Trim(index) => roots[events[index].root_index].point,
        }
    }
    fn physical_error(
        &self,
        cuts: &CutInventory<'_>,
        events: &[Event],
        roots: &[EdgeRoot],
    ) -> Result<f64, GeometryError> {
        if let Some(index) = self.trim {
            let root = roots[events[index].root_index];
            return norm(std::array::from_fn(|axis| {
                root.position[axis].sub(Interval::point(root.point[axis]))
            }));
        }
        match self.kind {
            NodeKind::Cut(id) => Ok(cuts.root(id).endpoint_error_mm),
            NodeKind::Trim(_) => Err(indeterminate(
                "native atlas trim node lost its original root",
            )),
        }
    }
    fn expression(
        &self,
        cuts: &CutInventory<'_>,
        events: &[Event],
    ) -> Result<SectionChartEndpoint, GeometryError> {
        if let Some(index) = self.trim {
            let event = events[index];
            return Ok(SectionChartEndpoint::TrimRoot {
                edgeuse: event.edgeuse,
                parameter: [event.parameter.lo, event.parameter.hi],
            });
        }
        match self.kind {
            NodeKind::Cut(id) => {
                let cut = cuts.cut(id.cut);
                let root = cuts.root(id);
                Ok(SectionChartEndpoint::ParameterCut {
                    axis: cut.axis as u8,
                    coordinate: cut.coordinate,
                    root: u32::try_from(id.root)
                        .map_err(|_| resource_error("native atlas root ordinal overflow"))?,
                    parameter: [root.parameter.lo, root.parameter.hi],
                })
            }
            NodeKind::Trim(_) => Err(indeterminate(
                "native atlas endpoint lost its authored trim",
            )),
        }
    }
}
struct Segment {
    cell: usize,
    /// Increasing local independent parameter, not material-left direction.
    nodes: [usize; 2],
    material: Option<bool>,
    used: bool,
}
fn norm(values: [Interval; 3]) -> Result<f64, GeometryError> {
    Ok(values
        .into_iter()
        .fold(Interval::point(0.0), |sum, value| sum.add(value.square()))
        .sqrt()?
        .hi)
}
fn graph<'a>(surface: &'a Surface, cell: &GraphCell, scale: f64, plane: PlaneMm) -> Graph<'a> {
    Graph {
        surface,
        domain: cell.graph_domain,
        axis: cell.axis,
        scale,
        plane,
        dependent_sign: cell.dependent_sign,
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn intersect_face(
    surface: &Surface,
    domain: [[f64; 2]; 2],
    scale: f64,
    plane: PlaneMm,
    outward: f64,
    face: FaceId,
    ordinal: usize,
    events: &[Event],
    roots: &[EdgeRoot],
    query: &SectionQuery,
    admission: &mut Admission,
    arcs: &mut Vec<Arc>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    let mut cuts = CutInventory::new(
        surface,
        domain,
        scale,
        plane,
        query
            .maximum_chord_error_mm
            .min(query.maximum_plane_error_mm)
            / 64.0,
    );
    let protected_cut = |axis: usize,
                         coordinate: f64,
                         work: &mut GeometryWorkBudget,
                         cancel: &AtomicBool|
     -> Result<bool, GeometryError> {
        for event in events {
            work.charge(1, cancel)?;
            if event.uv[axis].contains(coordinate) {
                return Ok(true);
            }
        }
        Ok(false)
    };
    let cells = atlas::cover(
        surface,
        domain,
        scale,
        plane,
        &protected_cut,
        &mut cuts,
        admission,
        work,
        cancel,
    )?;
    if cells.is_empty() {
        return if events.is_empty() {
            Ok(())
        } else {
            Err(indeterminate(
                "native atlas excluded a captured original trim root",
            ))
        };
    }
    let mut nodes = Vec::new();
    let mut offsets = Vec::new();
    for (cut_index, cut) in cuts.cuts().iter().enumerate() {
        work.charge(1, cancel)?;
        admission.push(&mut offsets, nodes.len())?;
        for root in 0..cut.roots.len() {
            work.charge(1, cancel)?;
            admission.push(
                &mut nodes,
                Node::new(
                    NodeKind::Cut(CutRootId {
                        cut: cut_index,
                        root,
                    }),
                    None,
                ),
            )?;
        }
    }
    let mut trim_nodes = Vec::new();
    for (event_index, event) in events.iter().enumerate() {
        work.charge(1, cancel)?;
        let mut matched = None;
        let mut fixed_cut = false;
        for (cut_index, cut) in cuts.cuts().iter().enumerate() {
            work.charge(1, cancel)?;
            if !event.uv[cut.axis].is_point() || event.uv[cut.axis].lo != cut.coordinate {
                continue;
            }
            fixed_cut = true;
            for (root_index, root) in cut.roots.iter().enumerate() {
                work.charge(1, cancel)?;
                if root
                    .parameter
                    .intersection(event.uv[1 - cut.axis])
                    .is_none()
                {
                    continue;
                }
                let node = offsets[cut_index] + root_index;
                if matched.is_some_and(|old| old != node) {
                    return Err(indeterminate(
                        "native trim root meets ambiguous native cut roots",
                    ));
                }
                matched = Some(node);
            }
        }
        let node = match matched {
            Some(node) => {
                if nodes[node].trim.replace(event_index).is_some() {
                    return Err(indeterminate(
                        "distinct original trim roots share one atlas root",
                    ));
                }
                node
            }
            None if fixed_cut => {
                return Err(indeterminate(
                    "original trim root has no same-line atlas root identity",
                ));
            }
            None => {
                let node = nodes.len();
                admission.push(
                    &mut nodes,
                    Node::new(NodeKind::Trim(event_index), Some(event_index)),
                )?;
                node
            }
        };
        admission.push(&mut trim_nodes, node)?;
    }
    let mut segments = Vec::new();
    let mut local = Vec::new();
    for (cell_index, cell) in cells.iter().enumerate() {
        work.charge(1, cancel)?;
        local.clear();
        for axis in 0..2 {
            for side in 0..2 {
                let cut_index = cell.cuts[axis][side];
                let other = cell.cell[1 - axis];
                for (root_index, root) in cuts.cut(cut_index).roots.iter().enumerate() {
                    work.charge(1, cancel)?;
                    if root.parameter.hi < other[0] || root.parameter.lo > other[1] {
                        continue;
                    }
                    if root.parameter.lo <= other[0] || root.parameter.hi >= other[1] {
                        return Err(indeterminate(
                            "native atlas root crosses an unresolved cell corner",
                        ));
                    }
                    admission.push(&mut local, offsets[cut_index] + root_index)?;
                }
            }
        }
        for &node in &trim_nodes {
            work.charge(1, cancel)?;
            let uv = nodes[node].uv(&cuts, events);
            if (0..2)
                .any(|axis| uv[axis].hi < cell.cell[axis][0] || uv[axis].lo > cell.cell[axis][1])
            {
                continue;
            }
            if (0..2)
                .any(|axis| uv[axis].lo < cell.cell[axis][0] || uv[axis].hi > cell.cell[axis][1])
            {
                return Err(indeterminate(
                    "native trim uncertainty straddles an unbound atlas cut",
                ));
            }
            admission.push(&mut local, node)?;
        }
        let independent = 1 - cell.axis;
        let count = u64::try_from(local.len()).unwrap_or(u64::MAX);
        work.charge(
            count.saturating_mul(u64::from(count.max(1).ilog2()) + 1),
            cancel,
        )?;
        local.sort_unstable_by(|&a, &b| {
            nodes[a].uv(&cuts, events)[independent]
                .lo
                .total_cmp(&nodes[b].uv(&cuts, events)[independent].lo)
        });
        local.dedup();
        let mut previous: Option<usize> = None;
        let mut inside_cell = false;
        for &node in &local {
            work.charge(1, cancel)?;
            if let Some(previous) = previous {
                if nodes[previous].uv(&cuts, events)[independent].hi
                    >= nodes[node].uv(&cuts, events)[independent].lo
                {
                    return Err(indeterminate(format!(
                        "native atlas roots lack strict local graph ordering: cell={:?}, axis={}, graph={:?}; previous={:?} trim={:?} uv={:?}; next={:?} trim={:?} uv={:?}",
                        cell.cell,
                        cell.axis,
                        cell.graph_domain,
                        nodes[previous].kind,
                        nodes[previous].trim,
                        nodes[previous].uv(&cuts, events),
                        nodes[node].kind,
                        nodes[node].trim,
                        nodes[node].uv(&cuts, events)
                    )));
                }
                if inside_cell {
                    add_segment(
                        cell_index,
                        [previous, node],
                        cell,
                        &mut nodes,
                        events,
                        &mut segments,
                        admission,
                    )?;
                }
            }
            if let Some(entering) = cell_crossing(&nodes[node], cell, &cuts)? {
                if inside_cell == entering {
                    return Err(indeterminate(
                        "native atlas cell roots lack alternating entry/exit",
                    ));
                }
                inside_cell = entering;
            } else if !inside_cell {
                return Err(indeterminate(
                    "original trim root has no owned carrier interval",
                ));
            }
            previous = Some(node);
        }
        if inside_cell {
            return Err(indeterminate(
                "native atlas carrier escapes a cell without a bound root",
            ));
        }
    }
    let mut queue = Vec::new();
    for (index, segment) in segments.iter().enumerate() {
        work.charge(1, cancel)?;
        if segment.material.is_some() {
            admission.push(&mut queue, index)?;
        }
    }
    let mut cursor = 0;
    while cursor < queue.len() {
        work.charge(1, cancel)?;
        let current = queue[cursor];
        cursor += 1;
        let material = segments[current].material.ok_or_else(|| {
            indeterminate("native atlas propagation lost its original trim label")
        })?;
        for node in segments[current].nodes {
            if nodes[node].trim.is_some() {
                continue;
            }
            for adjacent in nodes[node].adjacent.into_iter().flatten() {
                match segments[adjacent].material {
                    Some(old) if old != material => {
                        return Err(indeterminate(
                            "native atlas material labels disagree across a shared cut",
                        ));
                    }
                    Some(_) => {}
                    None => {
                        segments[adjacent].material = Some(material);
                        admission.push(&mut queue, adjacent)?;
                    }
                }
            }
        }
    }
    for segment in &segments {
        work.charge(1, cancel)?;
        if segment.material.is_none() {
            return Err(indeterminate(
                "native atlas has an interior/periodic carrier component without original trim authority",
            ));
        }
    }
    for node in &nodes {
        work.charge(1, cancel)?;
        let degree = node.adjacent.iter().flatten().count();
        let inside = node
            .adjacent
            .iter()
            .flatten()
            .filter(|&&index| segments[index].material == Some(true))
            .count();
        if node.trim.is_some() {
            if inside != 1 {
                return Err(indeterminate(
                    "original trim root lacks exactly one inside atlas continuation",
                ));
            }
        } else if degree != 0 {
            let NodeKind::Cut(id) = node.kind else {
                return Err(indeterminate("native atlas node lost its cut provenance"));
            };
            let cut = cuts.cut(id.cut);
            let outer =
                cut.coordinate == domain[cut.axis][0] || cut.coordinate == domain[cut.axis][1];
            if degree != 2 && !outer || inside != 0 && inside != 2 {
                return Err(indeterminate(
                    "native atlas cut has incomplete carrier/material incidence",
                ));
            }
        }
    }
    for &start in &trim_nodes {
        work.charge(1, cancel)?;
        let next = nodes[start]
            .adjacent
            .into_iter()
            .flatten()
            .find(|&index| segments[index].material == Some(true) && !segments[index].used);
        let Some(mut current_segment) = next else {
            continue;
        };
        let start_trim = nodes[start]
            .trim
            .ok_or_else(|| indeterminate("native atlas start lost its trim"))?;
        let mut current_node = start;
        let mut points = Vec::new();
        let mut tangents = Vec::new();
        let mut charts = Vec::new();
        let mut first_chart = None;
        let mut achieved = [0.0_f64; 2];
        let mut reverse = None;
        admission.push(&mut points, nodes[start].point(&cuts, events, roots))?;
        let end_trim = loop {
            work.charge(1, cancel)?;
            let segment = &mut segments[current_segment];
            if segment.used || segment.material != Some(true) {
                return Err(indeterminate(
                    "native atlas revisits material before its original trim end",
                ));
            }
            segment.used = true;
            let reversed = segment.nodes[1] == current_node;
            if !reversed && segment.nodes[0] != current_node {
                return Err(indeterminate("native atlas path enters a foreign segment"));
            }
            let cell = &cells[segment.cell];
            let expression = sample_segment(
                surface,
                scale,
                plane,
                outward,
                cell,
                segment.nodes,
                reversed,
                &nodes,
                &cuts,
                events,
                roots,
                query,
                &mut points,
                &mut tangents,
                &mut achieved,
                &mut reverse,
                admission,
                work,
                cancel,
            )?;
            if let Some(first) = first_chart {
                if charts.is_empty() {
                    admission.push(&mut charts, first)?;
                }
                admission.push(&mut charts, expression)?;
            } else {
                first_chart = Some(expression);
            }
            let end = segment.nodes[usize::from(!reversed)];
            if let Some(trim) = nodes[end].trim {
                break trim;
            }
            current_node = end;
            current_segment = nodes[end]
                .adjacent
                .into_iter()
                .flatten()
                .find(|&index| index != current_segment && segments[index].material == Some(true))
                .ok_or_else(|| indeterminate("native atlas material path has an unowned end"))?;
        };
        let reverse =
            reverse.ok_or_else(|| indeterminate("native atlas lacks a material orientation"))?;
        let first =
            first_chart.ok_or_else(|| indeterminate("native atlas has an empty authored arc"))?;
        let mut trim_order = [start_trim, end_trim];
        let (curve, span_reversed) = if charts.is_empty() {
            if first.reversed {
                trim_order.swap(0, 1);
            }
            (
                SectionCurve::SurfacePlaneGraph {
                    independent_axis: first.independent_axis,
                    independent: first.independent,
                    dependent: first.dependent,
                },
                reverse != first.reversed,
            )
        } else {
            (SectionCurve::SurfacePlaneAtlas { charts }, reverse)
        };
        if reverse {
            points.reverse();
            tangents.reverse();
            for tangent in &mut tangents {
                for value in tangent {
                    *value = value.neg();
                }
            }
        }
        let original = trim_order.map(|index| events[index]);
        let native = original.map(|event| roots[event.root_index]);
        let endpoint = |index: usize| {
            let root = roots[events[index].root_index];
            SectionEndpoint::EdgeRoot {
                edge: root.edge,
                root: root.ordinal,
            }
        };
        let mut endpoints = [endpoint(start_trim), endpoint(end_trim)];
        if reverse {
            endpoints.swap(0, 1);
        }
        admission.push(
            arcs,
            Arc {
                span: Some(SectionSpan {
                    face,
                    source_face_ordinal: ordinal,
                    definition_plane: plane,
                    section_pose: RigidPoseMm::IDENTITY,
                    curve,
                    endpoints,
                    trim_edgeuses: original.map(|event| event.edgeuse),
                    trim_parameters: original.map(|event| [event.parameter.lo, event.parameter.hi]),
                    native_edge_parameters: native
                        .map(|root| [root.parameter.lo, root.parameter.hi]),
                    reversed: span_reversed,
                    maximum_chord_error_mm: achieved[0],
                    maximum_plane_error_mm: achieved[1],
                }),
                points,
                tangents,
            },
        )?;
    }
    if segments
        .iter()
        .any(|segment| segment.material == Some(true) && !segment.used)
    {
        return Err(indeterminate(
            "native atlas omitted an inside carrier component",
        ));
    }
    Ok(())
}

fn cell_crossing(
    node: &Node,
    cell: &GraphCell,
    cuts: &CutInventory<'_>,
) -> Result<Option<bool>, GeometryError> {
    let NodeKind::Cut(id) = node.kind else {
        return Ok(None);
    };
    let cut = cuts.cut(id.cut);
    let Some(side) = cell.cuts[cut.axis]
        .iter()
        .position(|&index| index == id.cut)
    else {
        return Ok(None);
    };
    if cut.axis != cell.axis {
        if sign(cuts.root(id).derivative) != cell.dependent_sign {
            return Err(indeterminate(
                "native atlas shared root contradicts its graph derivative",
            ));
        }
        return Ok(Some(side == 0));
    }
    let slope_sign = -sign(cuts.root(id).derivative) * cell.dependent_sign;
    if slope_sign == 0 {
        return Err(indeterminate(
            "native atlas boundary lacks transverse crossing",
        ));
    }
    Ok(Some(if side == 0 {
        slope_sign > 0
    } else {
        slope_sign < 0
    }))
}

fn add_segment(
    cell_index: usize,
    pair: [usize; 2],
    cell: &GraphCell,
    nodes: &mut [Node],
    events: &[Event],
    segments: &mut Vec<Segment>,
    admission: &mut Admission,
) -> Result<(), GeometryError> {
    let mut material = None;
    for (end, &node) in pair.iter().enumerate() {
        if let Some(index) = nodes[node].trim {
            let crossing =
                events[index].crossing * cell.dependent_sign * if cell.axis == 1 { 1 } else { -1 };
            if crossing == 0 {
                return Err(indeterminate("native atlas trim lacks a directed crossing"));
            }
            let inside = crossing == if end == 0 { -1 } else { 1 };
            if material.is_some_and(|old| old != inside) {
                return Err(indeterminate(
                    "original trim crossings disagree on native atlas material",
                ));
            }
            material = Some(inside);
        }
    }
    let index = segments.len();
    admission.push(
        segments,
        Segment {
            cell: cell_index,
            nodes: pair,
            material,
            used: false,
        },
    )?;
    nodes[pair[0]].attach(index)?;
    nodes[pair[1]].attach(index)
}

#[allow(clippy::too_many_arguments)]
fn sample_segment(
    surface: &Surface,
    scale: f64,
    plane: PlaneMm,
    outward: f64,
    cell: &GraphCell,
    pair: [usize; 2],
    reversed: bool,
    nodes: &[Node],
    cuts: &CutInventory<'_>,
    events: &[Event],
    roots: &[EdgeRoot],
    query: &SectionQuery,
    points: &mut Vec<[f64; 3]>,
    tangents: &mut Vec<[Interval; 3]>,
    achieved: &mut [f64; 2],
    reverse: &mut Option<bool>,
    admission: &mut Admission,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<SectionGraphChart, GeometryError> {
    let graph = graph(surface, cell, scale, plane);
    let independent = 1 - cell.axis;
    let uv = pair.map(|index| nodes[index].uv(cuts, events));
    let positions = pair.map(|index| nodes[index].point(cuts, events, roots));
    let x = [uv[0][independent].mid(), uv[1][independent].mid()];
    let mut errors = [0.0; 2];
    let mut endpoint_tangents = [[Interval::point(0.0); 3]; 2];
    for end in 0..2 {
        let (_, jet) = graph.enclose(
            [uv[end][independent].lo, uv[end][independent].hi],
            work,
            cancel,
        )?;
        errors[end] = norm(std::array::from_fn(|axis| {
            jet.position[axis].sub(Interval::point(positions[end][axis]))
        }))?
        .max(nodes[pair[end]].physical_error(cuts, events, roots)?);
        endpoint_tangents[end] = graph.velocity(jet)?;
    }
    let point_start = points.len();
    let tangent_start = tangents.len();
    graph.sample(
        x,
        positions[0],
        positions[1],
        errors[0],
        errors[1],
        query,
        0,
        points,
        tangents,
        achieved,
        admission,
        work,
        cancel,
    )?;
    let tangent_end = tangents.len();
    if tangent_start == tangent_end || points.len() == point_start {
        return Err(indeterminate(
            "native atlas graph produced no bounded segment",
        ));
    }
    for (end, endpoint_tangent) in endpoint_tangents.iter().enumerate() {
        let slot = if end == 0 {
            tangent_start
        } else {
            tangent_end - 1
        };
        for axis in 0..3 {
            tangents[slot][axis] = tangents[slot][axis].hull(endpoint_tangent[axis]);
        }
    }
    let desired = graph.material_reverse(x[0] * 0.5 + x[1] * 0.5, outward, work, cancel)?;
    let whole_reverse = desired != reversed;
    if reverse.is_some_and(|old| old != whole_reverse) {
        return Err(indeterminate(
            "native atlas material orientation disagrees across charts",
        ));
    }
    *reverse = Some(whole_reverse);
    if reversed {
        // sample appends the upper end but not the lower. Reverse only the
        // appended interior, then replace that end: no extra point buffer.
        let last = points.len() - 1;
        points[point_start..last].reverse();
        points[last] = positions[0];
        tangents[tangent_start..].reverse();
        for tangent in &mut tangents[tangent_start..] {
            for value in tangent {
                *value = value.neg();
            }
        }
    }
    Ok(SectionGraphChart {
        independent_axis: independent as u8,
        independent: [uv[0][independent].lo, uv[1][independent].hi],
        dependent: cell.graph_domain[cell.axis],
        endpoints: [
            nodes[pair[0]].expression(cuts, events)?,
            nodes[pair[1]].expression(cuts, events)?,
        ],
        reversed,
    })
}
