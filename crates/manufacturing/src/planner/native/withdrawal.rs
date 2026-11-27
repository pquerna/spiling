use super::{
    LayerElevation, XYZ6_SCREEN_SLACK_MM, charge_support_clearance, emitted_xy_segment,
    encode_polygons, path_xy_bounds,
};
use crate::{
    error,
    planner::{SectionSpan, native_error},
};
use geo::BoundingRect;
use program_coordinates::project_xyz6_segment_bead_height_mm;
use spiling_contracts::manufacturing::{
    MAX_ARC_POINT_DEVIATION_MM, MAX_ARC_SAGITTA_MM, MAX_FIRMWARE_ARC_DEVIATION_MM,
    ManufacturingError, ManufacturingErrorCode, PrintLayer, SupportPolicy, program_coordinates,
};
use spiling_geometry::{
    GeometryWorkBudget, PlanarCapsuleClearancePolicy, PlanarCapsuleMm,
    certify_planar_capsule_pair_clearance, planar_bounds_clear, prepare_planar_forbidden,
    prove_planar_capsule_clearance_prepared,
};
use std::{ops::Range, sync::atomic::AtomicBool};

type Bounds = [[f64; 2]; 2];

#[derive(Clone, Copy)]
struct Bead {
    capsule: PlanarCapsuleMm,
    bounds: Bounds,
    upper: f64,
}

impl Bead {
    fn new(capsule: PlanarCapsuleMm, upper: f64) -> Self {
        let [start, end] = capsule.segment_xy_mm;
        let radius = (capsule.radius_mm + capsule.endpoint_error_mm).next_up();
        Self {
            capsule,
            bounds: [
                std::array::from_fn(|axis| (start[axis].min(end[axis]) - radius).next_down()),
                std::array::from_fn(|axis| (start[axis].max(end[axis]) + radius).next_up()),
            ],
            upper,
        }
    }

    fn key(&self) -> [u64; 6] {
        let [start, end] = self.capsule.segment_xy_mm;
        [
            start[0],
            start[1],
            end[0],
            end[1],
            self.capsule.radius_mm,
            self.capsule.endpoint_error_mm,
        ]
        .map(f64::to_bits)
    }
}

struct Node {
    bounds: Bounds,
    upper: f64,
    contents: Contents,
}

enum Contents {
    Beads(Range<usize>),
    Children([usize; 2]),
}

pub(in crate::planner) struct Sweep {
    beads: Vec<Bead>,
    nodes: Vec<Node>,
    bytes: usize,
    ordered: bool,
}

impl Sweep {
    pub(super) fn new(bytes: usize) -> Self {
        Self {
            beads: Vec::new(),
            nodes: Vec::new(),
            bytes,
            ordered: false,
        }
    }

    pub(in crate::planner) fn hash_stage_output(
        &self,
        mut sink: impl std::io::Write,
    ) -> std::io::Result<()> {
        for bead in &self.beads {
            for bits in bead.key().into_iter().chain([bead.upper.to_bits()]) {
                sink.write_all(&bits.to_be_bytes())?;
            }
        }
        Ok(())
    }

    pub(in crate::planner) fn retained_bytes(&self) -> usize {
        self.beads
            .capacity()
            .saturating_mul(std::mem::size_of::<Bead>())
            .saturating_add(
                self.nodes
                    .capacity()
                    .saturating_mul(std::mem::size_of::<Node>()),
            )
    }

    fn coalesce(
        &mut self,
        work: &GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        if self.ordered || self.beads.is_empty() {
            return Ok(());
        }
        let debit =
            (self.beads.len() as u64).saturating_mul(u64::from(self.beads.len().ilog2()) + 1);
        work.charge(debit, cancel)
            .map_err(|failure| native_error(failure, "sort support withdrawal beads"))?;
        self.beads.sort_unstable_by(|left, right| {
            left.key()
                .cmp(&right.key())
                .then_with(|| right.upper.total_cmp(&left.upper))
        });
        self.beads.dedup_by_key(|bead| bead.key());
        self.ordered = true;
        Ok(())
    }

    pub(super) fn record(
        &mut self,
        segment: [[f64; 2]; 2],
        policy: PlanarCapsuleClearancePolicy,
        upper: f64,
        work: &GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        let segment_xy_mm = if segment[0] > segment[1] {
            [segment[1], segment[0]]
        } else {
            segment
        };
        let bead = Bead::new(
            PlanarCapsuleMm {
                segment_xy_mm,
                radius_mm: (policy.radius_mm
                    + program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM)
                    .next_up(),
                endpoint_error_mm: policy.endpoint_error_mm,
            },
            upper,
        );
        let limit = self.bytes / (2 * std::mem::size_of::<Bead>() + std::mem::size_of::<Node>());
        if self.beads.len() == limit {
            self.coalesce(work, cancel)?;
        }
        let ordered_slot = if self.ordered {
            match self.beads.binary_search_by_key(&bead.key(), Bead::key) {
                Ok(slot) => {
                    self.beads[slot].upper = self.beads[slot].upper.max(upper);
                    return Ok(());
                }
                Err(slot) => Some(slot),
            }
        } else {
            None
        };
        if self.beads.len() == self.beads.capacity() {
            let capacity = self.beads.len().max(16).saturating_mul(2).min(limit);
            if capacity <= self.beads.len()
                || self
                    .beads
                    .try_reserve_exact(capacity - self.beads.len())
                    .is_err()
                || self.beads.capacity() > limit
            {
                return Err(error(
                    ManufacturingErrorCode::ResourceLimit,
                    "support withdrawal sweep exceeds remaining admitted geometry bytes",
                ));
            }
        }
        if let Some(slot) = ordered_slot {
            work.charge((self.beads.len() - slot) as u64, cancel)
                .map_err(|failure| native_error(failure, "insert support withdrawal bead"))?;
            self.beads.insert(slot, bead);
        } else {
            self.beads.push(bead);
        }
        Ok(())
    }

    pub(super) fn certify_source(
        mut self,
        schedule: &[LayerElevation],
        sections: &[SectionSpan],
        xy_gap: f64,
        native_work: &mut GeometryWorkBudget,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, ManufacturingError> {
        if self.beads.is_empty() {
            return Ok(None);
        }
        self.coalesce(native_work, cancel)?;
        self.nodes
            .try_reserve_exact(self.beads.len())
            .map_err(|_| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "cannot allocate support withdrawal sweep index",
                )
            })?;
        if self.retained_bytes() > self.bytes {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "support withdrawal index exceeds remaining admitted geometry bytes",
            ));
        }
        let root = index(&mut self.nodes, &mut self.beads, 0, native_work, cancel)?;
        let enclosing = &self.nodes[root];
        for section in sections {
            charge_support_clearance(work, cancel)?;
            let interval = [
                schedule[section.start as usize].lower_z,
                schedule[(section.end - 1) as usize].z,
            ];
            if interval[0] > enclosing.upper {
                break;
            }
            section.visit_occupied_screened(
                cancel,
                |bounds, height| Ok(height.unwrap_or(interval)[0].max(interval[0]) <= enclosing.upper
                    && !planar_bounds_clear(enclosing.bounds, bounds, 0.0, xy_gap)),
                |polygon, height| {
                    let height = height.unwrap_or(interval);
                    let lower = height[0].max(interval[0]);
                    if lower > height[1].min(interval[1]) {
                        return Ok(true);
                    }
                    let bounds = polygon.bounding_rect().ok_or_else(|| error(
                        ManufacturingErrorCode::NativeAnalysisIndeterminate,
                        "support withdrawal encountered empty occupied geometry",
                    ))?;
                    let bounds = [bounds.min(), bounds.max()].map(|point| [point.x, point.y]);
                    let encoded = std::cell::OnceCell::new();
                    let mut prepared = None;
                    self.visit(root, bounds, lower, xy_gap, work, cancel, &mut |bead| {
                        if encoded.get().is_none() {
                            let _ = encoded.set(encode_polygons(std::iter::once(polygon), 0.0, cancel)?);
                        }
                        if prepared.is_none() {
                            prepared = Some(prepare_planar_forbidden(encoded.get().expect("withdrawal polygon"), native_work, cancel)
                                .map_err(|failure| native_error(failure, "support withdrawal: prepare continuous occupancy"))?);
                        }
                        let capsule = bead.capsule;
                        prove_planar_capsule_clearance_prepared(capsule.segment_xy_mm, prepared.as_ref().expect("withdrawal polygon proof"),
                            PlanarCapsuleClearancePolicy { radius_mm: capsule.radius_mm, endpoint_error_mm: capsule.endpoint_error_mm, required_gap_mm: xy_gap }, native_work, cancel)
                            .map_err(|failure| native_error(failure, "support withdrawal: actual bead against continuous occupied geometry"))?;
                        Ok(())
                    })?;
                    Ok(true)
                },
            )?;
        }
        Ok(Some(self))
    }

    pub(in crate::planner) fn certify_realized(
        &self,
        layers: &[PrintLayer],
        policy: &SupportPolicy,
        native_work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        let SupportPolicy::Breakaway {
            xy_clearance_mm: xy_gap,
            z_gap_mm: z_gap,
            ..
        } = *policy
        else {
            return Err(error(
                ManufacturingErrorCode::InvalidSpecification,
                "withdrawal proof lost the breakaway clearance policy",
            ));
        };
        // Use the same accepted contact gap as support admission and replay.
        let z_gap = program_coordinates::honoured_z_gap_mm(z_gap);
        let root = self.nodes.len() - 1;
        let enclosing = &self.nodes[root];
        let work = &mut 0;
        let width_slack = program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM;
        let arc_slack = (MAX_ARC_SAGITTA_MM
            + MAX_ARC_POINT_DEVIATION_MM
            + MAX_FIRMWARE_ARC_DEVIATION_MM
            + 8.0 * XYZ6_SCREEN_SLACK_MM)
            .next_up();
        for layer in layers {
            charge_support_clearance(work, cancel)?;
            for path in &layer.paths {
                charge_support_clearance(work, cancel)?;
                let path_arc_slack = if path.arcs.is_empty() { 0.0 } else { arc_slack };
                if !path.kind.is_model()
                    || path_xy_bounds(path).is_some_and(|(bounds, width)| {
                        planar_bounds_clear(
                            enclosing.bounds,
                            bounds,
                            (width * 0.5 + XYZ6_SCREEN_SLACK_MM + width_slack + path_arc_slack)
                                .next_up(),
                            xy_gap,
                        )
                    })
                {
                    continue;
                }
                let mut arcs = path.arcs.iter().peekable();
                for (ordinal, edge) in path.checked_edges()?.enumerate() {
                    charge_support_clearance(work, cancel)?;
                    arcs.next_if(|arc| arc.last as usize <= ordinal);
                    let arc_error = if arcs.peek().is_some_and(|arc| arc.first as usize <= ordinal)
                    {
                        arc_slack
                    } else {
                        0.0
                    };
                    let points = edge.points_mm;
                    let [object_lower, _] = project_xyz6_segment_bead_height_mm(
                        points[0][2],
                        points[1][2],
                        path.height_mm(),
                    )?;
                    let lower = program_coordinates::enclosed_vertical_gap_mm(object_lower, z_gap)?;
                    let (segment_xy_mm, endpoint_error_mm) = emitted_xy_segment(points)?;
                    let object = Bead::new(
                        PlanarCapsuleMm {
                            segment_xy_mm,
                            endpoint_error_mm,
                            radius_mm: (edge.width_mm * 0.5 + width_slack + arc_error).next_up(),
                        },
                        0.0,
                    );
                    self.visit(root, object.bounds, lower, xy_gap, work, cancel, &mut |bead| {
                        if program_coordinates::enclosed_vertical_gap_mm(object_lower, bead.upper)? >= z_gap {
                            return Ok(());
                        }
                        certify_planar_capsule_pair_clearance([bead.capsule, object.capsule], xy_gap, native_work, cancel)
                            .map_err(|failure| native_error(failure, &format!("support withdrawal: actual bead against deposited object layer {}", layer.index)))?;
                        Ok(())
                    })?;
                }
            }
        }
        Ok(())
    }

    #[expect(clippy::too_many_arguments)]
    fn visit(
        &self,
        index: usize,
        bounds: Bounds,
        lower: f64,
        gap: f64,
        work: &mut usize,
        cancel: &AtomicBool,
        test: &mut impl FnMut(&Bead) -> Result<(), ManufacturingError>,
    ) -> Result<(), ManufacturingError> {
        charge_support_clearance(work, cancel)?;
        let node = &self.nodes[index];
        if lower > node.upper || planar_bounds_clear(node.bounds, bounds, 0.0, gap) {
            return Ok(());
        }
        match &node.contents {
            Contents::Children(children) => {
                for &child in children {
                    self.visit(child, bounds, lower, gap, work, cancel, test)?;
                }
            }
            Contents::Beads(range) => {
                for bead in &self.beads[range.clone()] {
                    charge_support_clearance(work, cancel)?;
                    if lower <= bead.upper && !planar_bounds_clear(bead.bounds, bounds, 0.0, gap) {
                        test(bead)?;
                    }
                }
            }
        }
        Ok(())
    }
}

fn index(
    nodes: &mut Vec<Node>,
    beads: &mut [Bead],
    first: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<usize, ManufacturingError> {
    work.charge(beads.len() as u64, cancel)
        .map_err(|failure| native_error(failure, "index support withdrawal"))?;
    let mut bounds = beads[0].bounds;
    let mut upper = beads[0].upper;
    for bead in &beads[1..] {
        for axis in 0..2 {
            bounds[0][axis] = bounds[0][axis].min(bead.bounds[0][axis]);
            bounds[1][axis] = bounds[1][axis].max(bead.bounds[1][axis]);
        }
        upper = upper.max(bead.upper);
    }
    let contents = if beads.len() <= 8 {
        Contents::Beads(first..first + beads.len())
    } else {
        let axis = usize::from(bounds[1][1] - bounds[0][1] > bounds[1][0] - bounds[0][0]);
        let mid = beads.len() / 2;
        let center = |bead: &Bead| bead.bounds[0][axis] + bead.bounds[1][axis];
        beads.select_nth_unstable_by(mid, |left, right| center(left).total_cmp(&center(right)));
        let (left, right) = beads.split_at_mut(mid);
        Contents::Children([
            index(nodes, left, first, work, cancel)?,
            index(nodes, right, first + mid, work, cancel)?,
        ])
    };
    let ordinal = nodes.len();
    nodes.push(Node {
        bounds,
        upper,
        contents,
    });
    Ok(ordinal)
}

#[test]
fn duplicate_sweeps_keep_maximum_height_at_capacity() {
    let cancel = AtomicBool::new(false);
    let work = GeometryWorkBudget::compile();
    let mut sweep = Sweep::new(2 * std::mem::size_of::<Bead>() + std::mem::size_of::<Node>());
    let policy = PlanarCapsuleClearancePolicy {
        radius_mm: 0.2,
        endpoint_error_mm: 0.0,
        required_gap_mm: 0.3,
    };
    let edge = [[1.0, 1.0], [2.0, 1.0]];
    for upper in [1.0, 2.0, 0.5] {
        sweep.record(edge, policy, upper, &work, &cancel).unwrap();
    }
    sweep
        .record([edge[1], edge[0]], policy, 1.1, &work, &cancel)
        .unwrap();
    assert_eq!(sweep.beads.len(), 1);
    assert_eq!(sweep.beads[0].upper, 2.0);
    assert_eq!(
        sweep
            .record([[2.0, 1.0], [3.0, 1.0]], policy, 1.0, &work, &cancel)
            .unwrap_err()
            .code,
        ManufacturingErrorCode::ResourceLimit
    );
}
