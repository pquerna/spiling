use super::{cancelled, charge, failed, project_xyz6_point_mm};
use geo::algorithm::convex_hull::quick_hull;
use geo::kernels::{Kernel, Orientation, RobustKernel};
use geo::{
    Area, BooleanOps, Contains, Coord, Distance, Euclidean, Line, LineString, MultiPolygon, Point,
    Polygon, unary_union,
};
use spiling_contracts::manufacturing::*;
use std::{mem::size_of, sync::atomic::AtomicBool};

const OFFSET_ERROR_MM: f64 = 0.005;
const AREA_ROUNDOFF: f64 = 0.0000001;
const COMMAND_SIDES: usize = 64;
const TURN_LOCATION_ENCLOSURE_MM: f64 = 0.0015;

fn axis_sign(value: f64) -> f64 {
    if value == 0.0 { 0.0 } else { value.signum() }
}

struct Budget<'a> {
    work: &'a mut usize,
    cancel: &'a AtomicBool,
    allocated: usize,
}

impl Budget<'_> {
    fn step(&mut self, count: usize) -> Result<(), ManufacturingError> {
        cancelled(self.cancel)?;
        charge(self.work, count)
    }

    fn reserve(&mut self, bytes: usize) -> Result<(), ManufacturingError> {
        self.allocated = self
            .allocated
            .checked_add(bytes)
            .filter(|total| *total as u64 <= crate::working_set::bytes_u64())
            .ok_or_else(resource)?;
        Ok(())
    }

    fn geometry(
        &mut self,
        mut shape: MultiPolygon<f64>,
    ) -> Result<MultiPolygon<f64>, ManufacturingError> {
        self.reserve(owned_geometry_bytes(&mut shape)?)?;
        self.step(point_count(&shape))?;
        for polygon in &shape.0 {
            if !polygon.unsigned_area().is_finite() || polygon.unsigned_area() <= 0.0 {
                return Err(failed(
                    "functional wall operation returned a degenerate polygon",
                ));
            }
            for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
                if ring.0.len() < 4
                    || ring.0.first() != ring.0.last()
                    || ring
                        .0
                        .iter()
                        .any(|point| !point.x.is_finite() || !point.y.is_finite())
                {
                    return Err(failed("functional wall operation returned an invalid ring"));
                }
            }
        }
        Ok(shape)
    }

    fn pair(
        &mut self,
        left: &MultiPolygon<f64>,
        right: &MultiPolygon<f64>,
    ) -> Result<(), ManufacturingError> {
        crate::planner::boolean::budget_limited(
            left,
            right,
            self.work,
            (crate::working_set::bytes())
                .checked_sub(self.allocated)
                .ok_or_else(resource)?,
            self.cancel,
        )
    }

    fn difference(
        &mut self,
        left: &MultiPolygon<f64>,
        right: &MultiPolygon<f64>,
    ) -> Result<MultiPolygon<f64>, ManufacturingError> {
        self.pair(left, right)?;
        self.geometry(left.difference(right))
    }

    fn intersection(
        &mut self,
        left: &MultiPolygon<f64>,
        right: &MultiPolygon<f64>,
    ) -> Result<MultiPolygon<f64>, ManufacturingError> {
        self.pair(left, right)?;
        self.geometry(left.intersection(right))
    }

    fn union(
        &mut self,
        left: &MultiPolygon<f64>,
        right: &MultiPolygon<f64>,
    ) -> Result<MultiPolygon<f64>, ManufacturingError> {
        self.pair(left, right)?;
        self.geometry(left.union(right))
    }

    fn unary(
        &mut self,
        shapes: &MultiPolygon<f64>,
    ) -> Result<MultiPolygon<f64>, ManufacturingError> {
        self.pair(shapes, shapes)?;
        self.geometry(unary_union(&shapes.0))
    }

    fn hull(
        &mut self,
        ends: [[f64; 2]; 2],
        disk: &[Coord<f64>],
        rails: Option<f64>,
    ) -> Result<Polygon<f64>, ManufacturingError> {
        let count = disk
            .len()
            .checked_mul(2)
            .and_then(|count| count.checked_add(4))
            .ok_or_else(resource)?;
        let capacity = count
            .checked_add(1)
            .and_then(usize::checked_next_power_of_two)
            .ok_or_else(resource)?;
        self.reserve(
            capacity
                .checked_mul(3 * size_of::<Coord<f64>>())
                .ok_or_else(resource)?,
        )?;
        self.step(count)?;
        let mut points = Vec::with_capacity(count);
        for end in ends {
            points.extend(disk.iter().map(|delta| Coord {
                x: end[0] + delta.x,
                y: end[1] + delta.y,
            }));
        }
        if let Some(radius) = rails {
            let delta = [ends[1][0] - ends[0][0], ends[1][1] - ends[0][1]];
            let length = delta[0].hypot(delta[1]);
            if length <= 0.0 || !length.is_finite() {
                return Err(failed(
                    "functional wall command has no finite positive length",
                ));
            }
            for end in ends {
                for sign in [-1.0, 1.0] {
                    points.push(Coord {
                        x: end[0] - sign * radius * delta[1] / length,
                        y: end[1] + sign * radius * delta[0] / length,
                    });
                }
            }
        }
        Ok(Polygon::new(quick_hull(&mut points), Vec::new()))
    }
}

fn resource() -> ManufacturingError {
    ManufacturingError::new(
        ManufacturingErrorCode::ResourceLimit,
        "functional wall replay cumulative allocation or geometry work exceeded",
    )
}

fn point_count(shape: &MultiPolygon<f64>) -> usize {
    shape
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

fn geometry_bytes(shape: &MultiPolygon<f64>) -> Result<usize, ManufacturingError> {
    let mut bytes = shape
        .0
        .capacity()
        .checked_mul(size_of::<Polygon<f64>>())
        .ok_or_else(resource)?;
    for polygon in &shape.0 {
        bytes = bytes
            .checked_add(
                polygon
                    .interiors()
                    .len()
                    .checked_mul(size_of::<LineString<f64>>())
                    .ok_or_else(resource)?,
            )
            .ok_or_else(resource)?;
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            bytes = bytes
                .checked_add(
                    ring.0
                        .capacity()
                        .checked_mul(size_of::<Coord<f64>>())
                        .ok_or_else(resource)?,
                )
                .ok_or_else(resource)?;
        }
    }
    Ok(bytes)
}

fn owned_geometry_bytes(shape: &mut MultiPolygon<f64>) -> Result<usize, ManufacturingError> {
    let mut bytes = shape
        .0
        .capacity()
        .checked_mul(size_of::<Polygon<f64>>())
        .ok_or_else(resource)?;
    for polygon in &mut shape.0 {
        let (exterior, interiors) =
            std::mem::replace(polygon, Polygon::new(LineString(Vec::new()), Vec::new()))
                .into_inner();
        let mut capacities = interiors
            .capacity()
            .checked_mul(size_of::<LineString<f64>>())
            .ok_or_else(resource)?;
        for ring in std::iter::once(&exterior).chain(interiors.iter()) {
            capacities = capacities
                .checked_add(
                    ring.0
                        .capacity()
                        .checked_mul(size_of::<Coord<f64>>())
                        .ok_or_else(resource)?,
                )
                .ok_or_else(resource)?;
        }
        *polygon = Polygon::new(exterior, interiors);
        bytes = bytes.checked_add(capacities).ok_or_else(resource)?;
    }
    Ok(bytes)
}

fn offset(
    source: &MultiPolygon<f64>,
    distance: f64,
    budget: &mut Budget<'_>,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    budget.step(point_count(source))?;
    if !distance.is_finite() || distance == 0.0 {
        return Err(failed(
            "functional wall offset requires a finite nonzero distance",
        ));
    }
    let radius = distance.abs();
    let radial_error = OFFSET_ERROR_MM * 0.5;
    let required = (std::f64::consts::PI / (radius / (radius + radial_error)).acos()).ceil();
    if !required.is_finite() || required > 4096.0 {
        return Err(resource());
    }
    let sides = (required as usize).max(4).div_ceil(4) * 4;
    let circumradius = radius / (std::f64::consts::PI / sides as f64).cos();
    if !circumradius.is_finite() || circumradius - radius > radial_error {
        return Err(resource());
    }
    budget.reserve(sides * size_of::<Coord<f64>>())?;
    budget.step(sides)?;
    let disk: Vec<_> = (0..sides)
        .map(|index| {
            let angle = std::f64::consts::TAU * (index as f64 + 0.5) / sides as f64;
            Coord {
                x: circumradius * angle.cos(),
                y: circumradius * angle.sin(),
            }
        })
        .collect();
    let mut result = if distance > 0.0 {
        budget.reserve(geometry_bytes(source)?)?;
        budget.geometry(source.clone())?
    } else {
        MultiPolygon(Vec::new())
    };
    for polygon in &source.0 {
        let edges = std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .map(|ring| ring.0.len().saturating_sub(1))
            .sum::<usize>();
        budget.reserve(
            edges
                .checked_mul(size_of::<Polygon<f64>>())
                .ok_or_else(resource)?,
        )?;
        let mut pieces = MultiPolygon(Vec::with_capacity(edges));
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            let mut start = 0;
            while start + 1 < ring.0.len() {
                budget.step(1)?;
                let begin = ring.0[start];
                let mut end = start + 1;
                while end + 1 < ring.0.len() {
                    budget.step(1)?;
                    let middle = ring.0[end];
                    let next = ring.0[end + 1];
                    if RobustKernel::orient2d(begin, middle, next) != Orientation::Collinear
                        || middle.x < begin.x.min(next.x)
                        || middle.x > begin.x.max(next.x)
                        || middle.y < begin.y.min(next.y)
                        || middle.y > begin.y.max(next.y)
                    {
                        break;
                    }
                    end += 1;
                }
                pieces.0.push(budget.hull(
                    [[begin.x, begin.y], [ring.0[end].x, ring.0[end].y]],
                    &disk,
                    None,
                )?);
                start = end;
            }
        }
        let tube = budget.unary(&pieces)?;
        if distance < 0.0 {
            budget.reserve(
                polygon
                    .exterior()
                    .0
                    .len()
                    .checked_add(
                        polygon
                            .interiors()
                            .iter()
                            .map(|ring| ring.0.len())
                            .sum::<usize>(),
                    )
                    .and_then(|count| count.checked_mul(size_of::<Coord<f64>>()))
                    .ok_or_else(resource)?,
            )?;
            let original = budget.geometry(MultiPolygon(vec![polygon.clone()]))?;
            let changed = budget.difference(&original, &tube)?;
            budget.reserve(
                result
                    .0
                    .len()
                    .checked_add(changed.0.len())
                    .ok_or_else(resource)?
                    .checked_mul(size_of::<Polygon<f64>>())
                    .ok_or_else(resource)?,
            )?;
            result
                .0
                .try_reserve_exact(changed.0.len())
                .map_err(|_| resource())?;
            result.0.extend(changed.0);
        } else {
            result = budget.union(&result, &tube)?;
        }
    }
    budget.geometry(result)
}

#[derive(Clone, Copy)]
struct Command {
    points: [[f64; 2]; 2],
    raw_points: [[f64; 2]; 2],
    lattice: [[i64; 2]; 2],
    width: f64,
    path: usize,
    ordinal: usize,
    path_edges: usize,
}

fn commands(
    paths: &[&DepositionPath],
    material: &MultiPolygon<f64>,
    budget: &mut Budget<'_>,
) -> Result<Vec<Command>, ManufacturingError> {
    let count = paths.iter().try_fold(0usize, |count, path| {
        count
            .checked_add(path.checked_edges()?.len())
            .filter(|count| *count <= crate::working_set::units())
            .ok_or_else(resource)
    })?;
    budget.reserve(
        count
            .checked_mul(size_of::<Command>())
            .ok_or_else(resource)?,
    )?;
    let mut commands = Vec::with_capacity(count);
    for (path_index, path) in paths.iter().enumerate() {
        budget.step(1)?;
        if path.owner != Some(DepositionOwner::Wall)
            || !matches!(
                path.kind,
                DepositionKind::Perimeter | DepositionKind::Bridge
            )
            || path.physical_z_mm != paths[0].physical_z_mm
            || path.material_domain != paths[0].material_domain
            || path.points_mm.len() < 4
            || project_xyz6_point_mm(path.points_mm[0])?
                != project_xyz6_point_mm(*path.points_mm.last().ok_or_else(resource)?)?
        {
            return Err(failed(
                "functional wall proof requires same-material closed source-following Wall loops on one physical interval",
            ));
        }
        let edges = path.checked_edges()?;
        let edge_count = edges.len();
        for (ordinal, edge) in edges.enumerate() {
            budget.step(point_count(material))?;
            let ends = [
                project_xyz6_point_mm(edge.points_mm[0])?,
                project_xyz6_point_mm(edge.points_mm[1])?,
            ];
            if ends[0][2] != ends[1][2]
                || ends[0][2]
                    != program_coordinates::project_xyz6_coordinate_mm(path.physical_z_mm[1])?
            {
                return Err(failed(
                    "functional wall proof requires horizontal actual XYZ6 commands",
                ));
            }
            let points = ends.map(|point| [point[0], point[1]]);
            let line = Line::new(Coord::from(points[0]), Coord::from(points[1]));
            let middle = Point::new(
                (points[0][0] + points[1][0]) * 0.5,
                (points[0][1] + points[1][1]) * 0.5,
            );
            if points[0] == points[1] || !material.contains(&middle) {
                return Err(failed(
                    "functional wall command collapses or leaves original material",
                ));
            }
            let needed =
                edge.width_mm * 0.5 + program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM;
            if needed <= OFFSET_ERROR_MM {
                return Err(failed(
                    "functional Wall is too narrow to prove full-segment source clearance",
                ));
            }
            for polygon in &material.0 {
                for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
                    for boundary in ring.lines() {
                        budget.step(1)?;
                        let clearance = Euclidean.distance(&line, &boundary);
                        if !clearance.is_finite() || clearance + OFFSET_ERROR_MM < needed {
                            return Err(failed(
                                "functional outer Wall violates original source exterior/hole clearance",
                            ));
                        }
                    }
                }
            }
            commands.push(Command {
                points,
                raw_points: std::array::from_fn(|index| {
                    [edge.points_mm[index][0], edge.points_mm[index][1]]
                }),
                lattice: points
                    .map(|point| point.map(|value| (value * 1_000_000.0).round() as i64)),
                width: edge.width_mm,
                path: path_index,
                ordinal,
                path_edges: edge_count,
            });
        }
    }
    Ok(commands)
}

fn orientation(begin: [i64; 2], end: [i64; 2], point: [i64; 2]) -> i128 {
    (i128::from(end[0]) - i128::from(begin[0])) * (i128::from(point[1]) - i128::from(begin[1]))
        - (i128::from(end[1]) - i128::from(begin[1]))
            * (i128::from(point[0]) - i128::from(begin[0]))
}

fn unique(commands: &[Command], budget: &mut Budget<'_>) -> Result<(), ManufacturingError> {
    for (index, left) in commands.iter().enumerate() {
        for right in &commands[index + 1..] {
            budget.step(1)?;
            if (0..2).any(|axis| {
                left.lattice[0][axis].min(left.lattice[1][axis])
                    > right.lattice[0][axis].max(right.lattice[1][axis])
                    || right.lattice[0][axis].min(right.lattice[1][axis])
                        > left.lattice[0][axis].max(left.lattice[1][axis])
            }) {
                continue;
            }
            let left_side = right
                .lattice
                .map(|point| orientation(left.lattice[0], left.lattice[1], point));
            let right_side = left
                .lattice
                .map(|point| orientation(right.lattice[0], right.lattice[1], point));
            if left_side == [0, 0] {
                let axis = usize::from(left.lattice[0][0] == left.lattice[1][0]);
                if left.lattice[0][axis]
                    .min(left.lattice[1][axis])
                    .max(right.lattice[0][axis].min(right.lattice[1][axis]))
                    < left.lattice[0][axis]
                        .max(left.lattice[1][axis])
                        .min(right.lattice[0][axis].max(right.lattice[1][axis]))
                {
                    return Err(failed(
                        "functional Wall contains duplicate, reversed or partially retraced XYZ6 commands",
                    ));
                }
            }
            let intersect = left_side[0].signum() * left_side[1].signum() <= 0
                && right_side[0].signum() * right_side[1].signum() <= 0;
            let adjacent = left.path == right.path
                && (right.ordinal == left.ordinal + 1
                    || left.ordinal == 0 && right.ordinal + 1 == left.path_edges);
            if intersect && !adjacent {
                return Err(failed(
                    "functional Wall has an unproved nonlocal crossing or contour contact",
                ));
            }
        }
    }
    Ok(())
}

fn follow_source(
    material: &MultiPolygon<f64>,
    commands: &[Command],
    path_count: usize,
    budget: &mut Budget<'_>,
) -> Result<(), ManufacturingError> {
    let rings = material
        .0
        .iter()
        .flat_map(|polygon| std::iter::once(polygon.exterior()).chain(polygon.interiors()));
    let ring_count = rings.clone().count();
    if ring_count != path_count {
        return Err(failed(
            "functional Wall proof requires exactly one complete loop per original source ring",
        ));
    }
    budget.reserve(ring_count)?;
    let mut used = vec![false; ring_count];
    for path in 0..path_count {
        let mut matched = None;
        for (ring_index, ring) in rings.clone().enumerate() {
            budget.step(commands.len())?;
            if used[ring_index] {
                continue;
            }
            let mut follows = true;
            for command in commands.iter().filter(|command| command.path == path) {
                let mut near_boundary = false;
                for boundary in ring.lines() {
                    budget.step(2)?;
                    let limit = command.width * 0.5 + OFFSET_ERROR_MM;
                    if command
                        .points
                        .iter()
                        .all(|point| Euclidean.distance(&Point::from(*point), &boundary) <= limit)
                    {
                        near_boundary = true;
                        break;
                    }
                }
                if !near_boundary {
                    follows = false;
                    break;
                }
            }
            if follows && matched.replace(ring_index).is_some() {
                return Err(failed(
                    "functional Wall loop has ambiguous original source-contour ownership",
                ));
            }
        }
        let matched = matched.ok_or_else(|| {
            failed("functional Wall loop does not follow one original source boundary")
        })?;
        used[matched] = true;
    }
    Ok(())
}

fn command_capsule(
    command: &Command,
    uncertainty: f64,
    budget: &mut Budget<'_>,
) -> Result<Polygon<f64>, ManufacturingError> {
    let radius = command.width * 0.5 + uncertainty;
    let magnitude = command
        .points
        .into_iter()
        .flatten()
        .fold(radius.max(1.0), |magnitude, value| {
            magnitude.max(value.abs())
        });
    let arithmetic = (128.0 * f64::EPSILON * magnitude).next_up();
    let radius = if uncertainty > 0.0 {
        (radius + arithmetic) / (std::f64::consts::PI / COMMAND_SIDES as f64).cos()
    } else {
        radius - arithmetic
    };
    if !radius.is_finite() || radius <= 0.0 {
        return Err(failed(
            "functional Wall has no positive command-enclosed occupancy",
        ));
    }
    let disk = std::array::from_fn::<_, COMMAND_SIDES, _>(|index| {
        let angle = std::f64::consts::TAU * index as f64 / COMMAND_SIDES as f64;
        Coord {
            x: radius * angle.cos(),
            y: radius * angle.sin(),
        }
    });
    budget.hull(
        command.points,
        &disk,
        (uncertainty <= 0.0).then_some(radius),
    )
}

fn occupancy(
    commands: &[Command],
    path: Option<usize>,
    uncertainty: f64,
    budget: &mut Budget<'_>,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    budget.step(commands.len().checked_mul(2).ok_or_else(resource)?)?;
    let count = commands
        .iter()
        .filter(|command| path.is_none_or(|path| path == command.path))
        .count();
    budget.reserve(
        count
            .checked_mul(size_of::<Polygon<f64>>())
            .ok_or_else(resource)?,
    )?;
    let mut pieces = MultiPolygon(Vec::with_capacity(count));
    for command in commands
        .iter()
        .filter(|command| path.is_none_or(|path| path == command.path))
    {
        pieces
            .0
            .push(command_capsule(command, uncertainty, budget)?);
    }
    budget.unary(&pieces)
}

fn inner_reach(
    commands: &[Command],
    distance: f64,
    budget: &mut Budget<'_>,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    budget.reserve(
        commands
            .len()
            .checked_mul(size_of::<Polygon<f64>>())
            .ok_or_else(resource)?,
    )?;
    let mut pieces = MultiPolygon(Vec::with_capacity(commands.len()));
    for command in commands {
        let mut expanded = *command;
        expanded.width = (command.width + 2.0 * distance).next_down();
        pieces.0.push(command_capsule(
            &expanded,
            -program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM,
            budget,
        )?);
    }
    budget.unary(&pieces)
}

fn classification_occupancy(
    commands: &[Command],
    budget: &mut Budget<'_>,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    budget.reserve(
        commands
            .len()
            .checked_mul(size_of::<Polygon<f64>>())
            .ok_or_else(resource)?,
    )?;
    let mut pieces = MultiPolygon(Vec::with_capacity(commands.len()));
    for command in commands {
        let mut original = *command;
        original.points = command.raw_points;
        pieces.0.push(command_capsule(
            &original,
            program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM,
            budget,
        )?);
    }
    budget.unary(&pieces)
}

struct SourceTurn {
    location: MultiPolygon<f64>,
    dose: f64,
}

fn source_turns(
    material: &MultiPolygon<f64>,
    width: f64,
    nozzle: f64,
    budget: &mut Budget<'_>,
) -> Result<Vec<SourceTurn>, ManufacturingError> {
    budget.reserve(
        point_count(material)
            .checked_mul(size_of::<SourceTurn>())
            .ok_or_else(resource)?,
    )?;
    let mut turns = Vec::new();
    let full_width_turn = width * 0.5 * (std::f64::consts::SQRT_2 - 1.0)
        + program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM
        + TURN_LOCATION_ENCLOSURE_MM
        <= nozzle * 0.25 - OFFSET_ERROR_MM * 0.5;
    for polygon in &material.0 {
        for (ring_index, ring) in std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .enumerate()
        {
            let origin = ring.0[0];
            budget.step(ring.0.len())?;
            let positive = ring
                .0
                .windows(2)
                .map(|edge| {
                    (edge[0].x - origin.x) * (edge[1].y - origin.y)
                        - (edge[1].x - origin.x) * (edge[0].y - origin.y)
                })
                .sum::<f64>()
                > 0.0;
            let material_left = positive == (ring_index == 0);
            for index in 0..ring.0.len() - 1 {
                budget.step(1)?;
                let vertex = ring.0[index];
                let previous = ring.0[if index == 0 {
                    ring.0.len() - 2
                } else {
                    index - 1
                }];
                let next = ring.0[index + 1];
                let incoming = [vertex.x - previous.x, vertex.y - previous.y];
                let outgoing = [next.x - vertex.x, next.y - vertex.y];
                let right_angle = (incoming[0] == 0.0
                    && outgoing[1] == 0.0
                    && incoming[1] != 0.0
                    && outgoing[0] != 0.0)
                    || (incoming[1] == 0.0
                        && outgoing[0] == 0.0
                        && incoming[0] != 0.0
                        && outgoing[1] != 0.0);
                let cross = incoming[0] * outgoing[1] - incoming[1] * outgoing[0];
                if cross == 0.0 {
                    continue;
                }
                if !right_angle || !full_width_turn {
                    return Err(failed(
                        "functional Wall cannot prove a nonorthogonal or width-recovering source contour turn",
                    ));
                }
                let convex = (cross > 0.0) == material_left;
                let side = if material_left { 1.0 } else { -1.0 };
                let center = if convex {
                    [
                        vertex.x
                            + side
                                * (-axis_sign(incoming[1]) - axis_sign(outgoing[1]))
                                * width
                                * 0.5,
                        vertex.y
                            + side
                                * (axis_sign(incoming[0]) + axis_sign(outgoing[0]))
                                * width
                                * 0.5,
                    ]
                } else {
                    [vertex.x, vertex.y]
                };
                budget.step(point_count(material))?;
                if convex && !material.contains(&Point::from(center)) {
                    return Err(failed(
                        "functional Wall source turn has no original material interior",
                    ));
                }
                let desired = if convex {
                    std::f64::consts::SQRT_2 * width * 0.5 + OFFSET_ERROR_MM
                } else {
                    width + OFFSET_ERROR_MM
                };
                let mut radius = desired
                    .min(incoming[0].hypot(incoming[1]) - width * 0.5)
                    .min(outgoing[0].hypot(outgoing[1]) - width * 0.5);
                for boundary in material
                    .0
                    .iter()
                    .flat_map(|polygon| {
                        std::iter::once(polygon.exterior()).chain(polygon.interiors())
                    })
                    .flat_map(|ring| ring.lines())
                {
                    budget.step(1)?;
                    if boundary.start != vertex && boundary.end != vertex {
                        radius = radius.min(
                            Euclidean.distance(&Point::from(center), &boundary)
                                - TURN_LOCATION_ENCLOSURE_MM,
                        );
                    }
                }
                if !radius.is_finite() || radius <= TURN_LOCATION_ENCLOSURE_MM {
                    return Err(failed(
                        "functional Wall source turn has no isolated positive local allowance",
                    ));
                }
                let disk = std::array::from_fn::<_, COMMAND_SIDES, _>(|index| {
                    let angle = std::f64::consts::TAU * index as f64 / COMMAND_SIDES as f64;
                    Coord {
                        x: radius * angle.cos(),
                        y: radius * angle.sin(),
                    }
                });
                let ball = budget.hull([center; 2], &disk, None)?;
                let ball = budget.geometry(MultiPolygon(vec![ball]))?;
                let region = budget.intersection(&ball, material)?;
                if !region.0.is_empty() {
                    budget.reserve(size_of::<SourceTurn>() * 2)?;
                    turns.try_reserve_exact(1).map_err(|_| resource())?;
                    let dose_radius = (width * 0.5).min(radius / std::f64::consts::SQRT_2);
                    let dose = {
                        (std::f64::consts::PI
                            * dose_radius.powi(2)
                            * (std::f64::consts::TAU / COMMAND_SIDES as f64).sin()
                            / (std::f64::consts::TAU / COMMAND_SIDES as f64)
                            / std::f64::consts::TAU)
                            .next_down()
                    };
                    turns.push(SourceTurn {
                        location: region,
                        dose,
                    });
                }
            }
        }
    }
    Ok(turns)
}

fn strip(
    command: &Command,
    budget: &mut Budget<'_>,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    budget.reserve(size_of::<Polygon<f64>>() + 5 * size_of::<Coord<f64>>())?;
    let delta = [
        command.points[1][0] - command.points[0][0],
        command.points[1][1] - command.points[0][1],
    ];
    let length = delta[0].hypot(delta[1]);
    let normal = [
        -delta[1] * command.width * 0.5 / length,
        delta[0] * command.width * 0.5 / length,
    ];
    let corners = [
        [
            command.points[0][0] + normal[0],
            command.points[0][1] + normal[1],
        ],
        [
            command.points[1][0] + normal[0],
            command.points[1][1] + normal[1],
        ],
        [
            command.points[1][0] - normal[0],
            command.points[1][1] - normal[1],
        ],
        [
            command.points[0][0] - normal[0],
            command.points[0][1] - normal[1],
        ],
        [
            command.points[0][0] + normal[0],
            command.points[0][1] + normal[1],
        ],
    ];
    budget.geometry(MultiPolygon(vec![Polygon::new(
        LineString::from(corners.to_vec()),
        Vec::new(),
    )]))
}

fn dose(
    material: &MultiPolygon<f64>,
    required: &MultiPolygon<f64>,
    nominal: &MultiPolygon<f64>,
    commands: &[Command],
    width: f64,
    nozzle: f64,
    budget: &mut Budget<'_>,
) -> Result<(), ManufacturingError> {
    let mut seen = MultiPolygon(Vec::new());
    let mut allowance = 0.0;
    for turn in source_turns(material, width, nozzle, budget)? {
        let fresh = budget.difference(&turn.location, &seen)?;
        if fresh.0.is_empty() {
            continue;
        }
        let local_budget = (turn.dose * fresh.unsigned_area() / turn.location.unsigned_area())
            .next_down()
            .max(0.0);
        let mut deposited = 0.0;
        for command in commands {
            let strip = strip(command, budget)?;
            deposited += budget.intersection(&strip, &fresh)?.unsigned_area();
        }
        let occupied = budget.intersection(nominal, &fresh)?.unsigned_area();
        if !deposited.is_finite() || (deposited - occupied).max(0.0) > local_budget + AREA_ROUNDOFF
        {
            return Err(failed(format!(
                "functional Wall exceeds its independently source-bound contour-turn dose: excess {}, allowance {}",
                (deposited - occupied).max(0.0),
                local_budget,
            )));
        }
        allowance += local_budget;
        seen = budget.union(&seen, &turn.location)?;
    }
    let mut prior = MultiPolygon(Vec::new());
    let mut overlaps = MultiPolygon(Vec::new());
    for command in commands {
        let current = strip(command, budget)?;
        let overlap = budget.intersection(&current, &prior)?;
        overlaps = budget.union(&overlaps, &overlap)?;
        prior = budget.union(&prior, &current)?;
    }
    if budget.difference(&overlaps, &seen)?.unsigned_area() > AREA_ROUNDOFF {
        return Err(failed(
            "functional Wall strip overlap leaves its independently source-bound turn locations",
        ));
    }
    budget.step(commands.len())?;
    let deposited = commands
        .iter()
        .map(|command| {
            (command.points[1][0] - command.points[0][0])
                .hypot(command.points[1][1] - command.points[0][1])
                * command.width
        })
        .sum::<f64>();
    let excess = (deposited - nominal.unsigned_area()).max(0.0);
    if !excess.is_finite()
        || excess > required.unsigned_area() * 0.05 + AREA_ROUNDOFF
        || excess > allowance + AREA_ROUNDOFF
    {
        return Err(failed(
            "functional Wall has excess dose outside proved source contour allowances; junctions/transitions are unavailable",
        ));
    }
    Ok(())
}

pub(super) fn residual(
    material: &MultiPolygon<f64>,
    paths: &[&DepositionPath],
    recipe: &PlanarPrintRecipe,
    nozzle_diameter_mm: f64,
    retained_bytes: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    let mut budget = Budget {
        work,
        cancel,
        allocated: retained_bytes,
    };
    budget.step(1)?;
    if recipe.perimeter_count != 1
        || !recipe.bead_width_mm.is_finite()
        || recipe.bead_width_mm <= 0.0
        || !nozzle_diameter_mm.is_finite()
        || nozzle_diameter_mm <= OFFSET_ERROR_MM * 2.0
        || material.0.is_empty()
        || paths.is_empty()
    {
        return Err(failed(
            "functional Wall reconstruction requires nonempty material/walls, one perimeter and a positive nozzle/width",
        ));
    }
    if paths.iter().any(|path| path.kind == DepositionKind::Bridge)
        && recipe.process.bridge_flow_factor != 1.0
    {
        return Err(failed(
            "functional bridged Wall requires unchanged nominal contour dose",
        ));
    }
    budget.reserve(geometry_bytes(material)?)?;
    for path in paths {
        budget.reserve(
            size_of::<DepositionPath>()
                + path
                    .points_mm
                    .capacity()
                    .checked_mul(size_of::<[f64; 3]>())
                    .ok_or_else(resource)?,
        )?;
        if let WidthProfile::PerEdge { widths_mm } = &path.width {
            budget.reserve(
                widths_mm
                    .capacity()
                    .checked_mul(size_of::<f64>())
                    .ok_or_else(resource)?,
            )?;
        }
    }
    let core = offset(material, -recipe.bead_width_mm, &mut budget)?;
    let required = budget.difference(material, &core)?;
    let commands = commands(paths, material, &mut budget)?;
    unique(&commands, &mut budget)?;
    follow_source(material, &commands, paths.len(), &mut budget)?;
    if commands
        .iter()
        .any(|command| command.width > recipe.bead_width_mm)
    {
        return Err(failed(
            "functional Wall lane cannot prove a source-width expansion or transition",
        ));
    }
    let inner = occupancy(
        &commands,
        None,
        -program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM,
        &mut budget,
    )?;
    let missing = budget.difference(&required, &inner)?;
    for component in &material.0 {
        budget.reserve(geometry_bytes(material)?)?;
        let component = budget.geometry(MultiPolygon(vec![component.clone()]))?;
        let target = budget.intersection(&required, &component)?;
        let defect = budget.intersection(&missing, &component)?;
        if defect.unsigned_area() > target.unsigned_area() * 0.02 + AREA_ROUNDOFF {
            return Err(failed(
                "functional Wall original material component underfill exceeds 2%",
            ));
        }
        if target.unsigned_area() > AREA_ROUNDOFF
            && budget.intersection(&inner, &component)?.0.is_empty()
        {
            return Err(failed(
                "functional Wall original component has no guaranteed-inner coverage",
            ));
        }
    }
    let reach = inner_reach(
        &commands,
        nozzle_diameter_mm * 0.25 - OFFSET_ERROR_MM * 0.5,
        &mut budget,
    )?;
    if budget.difference(&missing, &reach)?.unsigned_area() > AREA_ROUNDOFF {
        return Err(failed(
            "functional Wall underfill exceeds the nozzle/4 source-local extent",
        ));
    }
    let nominal = occupancy(&commands, None, 0.0, &mut budget)?;
    if budget.intersection(&nominal, &core)?.unsigned_area() > AREA_ROUNDOFF {
        return Err(failed(
            "functional Wall nominal occupancy invades original core without a proved source transition",
        ));
    }
    let mut prior = MultiPolygon(Vec::new());
    for path in 0..paths.len() {
        let tube = occupancy(&commands, Some(path), 0.0, &mut budget)?;
        let overlap = budget.intersection(&prior, &tube)?;
        if budget.intersection(&overlap, material)?.unsigned_area() > AREA_ROUNDOFF {
            return Err(failed(
                "functional Wall loops overlap without a proved source junction",
            ));
        }
        prior = budget.union(&prior, &tube)?;
    }
    dose(
        material,
        &required,
        &nominal,
        &commands,
        recipe.bead_width_mm,
        nozzle_diameter_mm,
        &mut budget,
    )?;
    let outer = classification_occupancy(&commands, &mut budget)?;
    budget.difference(&core, &outer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{Rect, polygon};

    fn recipe() -> PlanarPrintRecipe {
        PlanarPrintRecipe {
            id: "wall-replay".into(),
            revision: "1".into(),
            material_id: "nominal".into(),
            material_revision: "1".into(),
            filament_diameter_mm: 1.75,
            layer_height_mm: 0.2,
            layer_height_policy: LayerHeightPolicy::ProtectedMaximum,
            bead_width_mm: 0.45,
            sparse_infill: SparseInfillPolicy::Fine,
            perimeter_count: 1,
            infill_fraction: 0.2,
            top_skin_thickness_mm: 0.4,
            bottom_skin_thickness_mm: 0.4,
            print_speed_mm_s: 20.0,
            travel_speed_mm_s: 100.0,
            flow_multiplier: 1.0,
            process: ProcessPolicy {
                first_layer_speed_factor: 1.0,
                first_layer_flow_factor: 1.0,
                fan_first_layers: 0,
                fan_after_first_layers: 0,
                bridge_speed_factor: 0.5,
                bridge_flow_factor: 1.0,
                bridge_fan_pwm: 255,
                max_bridge_span_mm: 5.0,
                retraction: RetractionPolicy::Disabled,
            },
            supports: SupportPolicy::Disabled,
            seam_policy: SeamPolicy::Nearest,
            rectilinear_axis: RectilinearAxisPolicy::AlternatingXY,
            top_skin_axis: RectilinearAxisPolicy::AlternatingXY,
        }
    }

    fn square() -> MultiPolygon<f64> {
        MultiPolygon(vec![
            Rect::new(Coord { x: 0.0, y: 0.0 }, Coord { x: 20.0, y: 20.0 }).to_polygon(),
        ])
    }

    fn wall(inset: f64, width: f64) -> DepositionPath {
        DepositionPath {
            material_domain: None,
            kind: DepositionKind::Perimeter,
            owner: Some(DepositionOwner::Wall),
            width: WidthProfile::Constant { width_mm: width },
            physical_z_mm: [0.0, 0.2],
            points_mm: vec![
                [inset, inset, 0.2],
                [20.0 - inset, inset, 0.2],
                [20.0 - inset, 20.0 - inset, 0.2],
                [inset, 20.0 - inset, 0.2],
                [inset, inset, 0.2],
            ],
            arcs: Vec::new(),
        }
    }

    fn replay(paths: &[&DepositionPath]) -> Result<MultiPolygon<f64>, ManufacturingError> {
        residual(
            &square(),
            paths,
            &recipe(),
            0.4,
            0,
            &mut 0,
            &AtomicBool::new(false),
        )
    }

    fn eroded(material: &MultiPolygon<f64>, radius: f64) -> MultiPolygon<f64> {
        offset(
            material,
            -radius,
            &mut Budget {
                work: &mut 0,
                cancel: &AtomicBool::new(false),
                allocated: 0,
            },
        )
        .unwrap()
    }

    #[test]
    fn complete_source_wall_preserves_core_and_excludes_command_outer_occupancy() {
        let wall = wall(0.225, 0.45);
        let residual = replay(&[&wall]).unwrap();
        assert!(residual.contains(&Point::new(10.0, 10.0)));
        assert!(!residual.contains(&Point::new(0.4505, 10.0)));
        assert!(residual.contains(&Point::new(0.455, 10.0)));
        assert!(residual.unsigned_area() > 360.0);
    }

    #[test]
    fn bridged_source_wall_keeps_complete_geometry_and_nominal_dose() {
        let mut wall = wall(0.225, 0.45);
        wall.kind = DepositionKind::Bridge;
        assert!(replay(&[&wall]).is_ok());
        let mut changed = recipe();
        changed.process.bridge_flow_factor = 0.9;
        let failure = residual(
            &square(),
            &[&wall],
            &changed,
            0.4,
            0,
            &mut 0,
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(failure.message.contains("nominal contour dose"));
        wall.points_mm.pop();
        assert!(replay(&[&wall]).unwrap_err().message.contains("closed"));
    }

    #[test]
    fn constant_recipe_still_admits_different_actual_edge_widths() {
        let mut wall = wall(0.225, 0.45);
        wall.width = WidthProfile::PerEdge {
            widths_mm: vec![0.449, 0.45, 0.448, 0.45],
        };
        assert!(replay(&[&wall]).is_ok());
    }

    #[test]
    fn missing_walls_and_missing_original_components_refuse() {
        assert!(replay(&[]).is_err());
        let wall = wall(0.225, 0.45);
        let mut material = square();
        material
            .0
            .push(Rect::new(Coord { x: 30.0, y: 0.0 }, Coord { x: 33.0, y: 3.0 }).to_polygon());
        let failure = residual(
            &material,
            &[&wall],
            &recipe(),
            0.4,
            0,
            &mut 0,
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(failure.message.contains("one complete loop"));
    }

    #[test]
    fn relabeling_interior_deposition_as_wall_cannot_shrink_demand() {
        let interior = wall(5.0, 0.45);
        let failure = replay(&[&interior]).unwrap_err();
        assert!(failure.message.contains("original source boundary"));
    }

    #[test]
    fn small_source_following_core_invasion_is_not_a_transition_permission() {
        let wall = wall(0.229, 0.45);
        let failure = replay(&[&wall]).unwrap_err();
        assert!(
            failure.message.contains("invades original core")
                || failure.message.contains("nozzle/4 source-local extent"),
            "{}",
            failure.message
        );
    }

    #[test]
    fn duplicate_reversed_and_stitched_retrace_walls_refuse() {
        let first = wall(0.225, 0.45);
        let mut reversed = first.clone();
        reversed.reverse();
        for second in [&first, &reversed] {
            let failure = replay(&[&first, second]).unwrap_err();
            assert_eq!(failure.code, ManufacturingErrorCode::VerificationFailed);
        }
        let mut retraced = first.clone();
        retraced.points_mm.extend_from_slice(&first.points_mm[1..]);
        let failure = replay(&[&retraced]).unwrap_err();
        assert!(
            failure.message.contains("retraced XYZ6")
                || failure
                    .message
                    .contains("nonlocal crossing or contour contact"),
            "{failure:?}"
        );
    }

    #[test]
    fn original_component_keeps_its_two_percent_underfill_limit() {
        let wall = wall(0.215, 0.43);
        let failure = replay(&[&wall]).unwrap_err();
        assert!(
            failure.message.contains("underfill exceeds 2%"),
            "{}",
            failure.message
        );
    }

    #[test]
    fn source_turn_dose_does_not_authorize_duplicate_deposition() {
        let material = square();
        let wall = wall(0.225, 0.45);
        let mut work = 0;
        let cancel = AtomicBool::new(false);
        let mut budget = Budget {
            work: &mut work,
            cancel: &cancel,
            allocated: 0,
        };
        let core = offset(&material, -0.45, &mut budget).unwrap();
        let required = budget.difference(&material, &core).unwrap();
        let mut commands = commands(&[&wall], &material, &mut budget).unwrap();
        commands.push(commands[0]);
        let nominal = occupancy(&commands, None, 0.0, &mut budget).unwrap();
        let failure = dose(
            &material,
            &required,
            &nominal,
            &commands,
            0.45,
            0.4,
            &mut budget,
        )
        .unwrap_err();
        assert!(failure.message.contains("dose"));
    }

    #[test]
    fn contour_turns_keep_bounded_source_fits_but_not_open_or_shifted_lanes() {
        let shifted = wall(0.223, 0.448);
        assert!(replay(&[&shifted]).is_ok());
        let shifted = wall(0.24, 0.45);
        assert!(replay(&[&shifted]).is_err());
        let mut open = wall(0.225, 0.45);
        open.points_mm.pop();
        assert!(
            replay(&[&open])
                .unwrap_err()
                .message
                .contains("closed source-following Wall")
        );
        let mut multiple = recipe();
        multiple.perimeter_count = 2;
        let wall = wall(0.225, 0.45);
        assert!(
            residual(
                &square(),
                &[&wall],
                &multiple,
                0.4,
                0,
                &mut 0,
                &AtomicBool::new(false)
            )
            .is_err()
        );
    }

    #[test]
    fn nonconvex_core_uses_circular_boundary_neighborhood_not_manhattan_erosion() {
        let material = MultiPolygon(vec![polygon![
            (x: 0.0, y: 0.0), (x: 6.0, y: 0.0), (x: 6.0, y: 2.0),
            (x: 2.0, y: 2.0), (x: 2.0, y: 6.0), (x: 0.0, y: 6.0),
        ]]);
        let core = eroded(&material, 1.0);
        assert!(core.contains(&Point::new(1.2, 1.2)));
        assert!(!core.contains(&Point::new(1.4, 1.4)));
        assert!(!core.contains(&Point::new(0.99, 1.2)));
    }

    #[test]
    fn nonconvex_erosion_retains_its_euclidean_policy_after_oblique_rotation() {
        let rotate = |point: [f64; 2]| {
            let scale = std::f64::consts::FRAC_1_SQRT_2;
            [
                (point[0] - point[1]) * scale + 20.0,
                (point[0] + point[1]) * scale - 10.0,
            ]
        };
        let points = [
            [0.0, 0.0],
            [6.0, 0.0],
            [6.0, 2.0],
            [2.0, 2.0],
            [2.0, 6.0],
            [0.0, 6.0],
            [0.0, 0.0],
        ];
        let material = MultiPolygon(vec![Polygon::new(
            LineString(
                points
                    .into_iter()
                    .map(|point| Coord::from(rotate(point)))
                    .collect(),
            ),
            Vec::new(),
        )]);
        let core = eroded(&material, 1.0);
        assert!(core.contains(&Point::from(rotate([1.2, 1.2]))));
        assert!(!core.contains(&Point::from(rotate([1.4, 1.4]))));
    }

    #[test]
    fn original_hole_and_disappearing_small_component_are_not_replaced_by_tiles() {
        let material = MultiPolygon(vec![
            Polygon::new(
                Rect::new(Coord { x: 0.0, y: 0.0 }, Coord { x: 10.0, y: 10.0 })
                    .to_polygon()
                    .exterior()
                    .clone(),
                vec![
                    Rect::new(Coord { x: 3.0, y: 3.0 }, Coord { x: 7.0, y: 7.0 })
                        .to_polygon()
                        .exterior()
                        .clone(),
                ],
            ),
            Rect::new(Coord { x: 12.0, y: 0.0 }, Coord { x: 12.6, y: 1.0 }).to_polygon(),
        ]);
        let core = eroded(&material, 0.5);
        assert!(!core.contains(&Point::new(2.7, 2.7)));
        assert!(core.contains(&Point::new(2.6, 2.6)));
        assert!(!core.contains(&Point::new(5.0, 5.0)));
        assert!(!core.contains(&Point::new(12.3, 0.5)));
    }

    #[test]
    fn cancellation_and_cumulative_work_are_not_reset_by_wall_reconstruction() {
        let wall = wall(0.225, 0.45);
        let failure = residual(
            &square(),
            &[&wall],
            &recipe(),
            0.4,
            0,
            &mut 0,
            &AtomicBool::new(true),
        )
        .unwrap_err();
        assert_eq!(failure.code, ManufacturingErrorCode::Cancelled);
        let failure = residual(
            &square(),
            &[&wall],
            &recipe(),
            0.4,
            crate::working_set::bytes() - 1,
            &mut 0,
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(failure.code, ManufacturingErrorCode::ResourceLimit);
    }
}
