use super::*;
use i_overlay::core::overlay::ShapeType;
use i_overlay::core::{fill_rule::FillRule, overlay_rule::OverlayRule};
use i_overlay::float::overlay::FloatOverlay;
use i_overlay::i_float::{adapter::FloatPointAdapter, float::rect::FloatRect};

pub(in crate::planner) struct Frame {
    pub(in crate::planner) adapter: FloatPointAdapter<[f64; 2], f64>,
    bounds: [[f64; 2]; 2],
    distance: f64,
}

impl Frame {
    pub(super) fn new(
        input: &SupportInput<'_>,
        requested: f64,
        owned_bytes: usize,
        cancel: &AtomicBool,
    ) -> Result<Self, ManufacturingError> {
        check_cancel(cancel)?;
        let mut bounds = input.build_bounds;
        let mut seen = Vec::new();
        for section in input.sections {
            check_cancel(cancel)?;
            if let Some(material) = material_bounds(&section.material) {
                bounds = union_frame(Some(bounds), material);
            }
            if let Some(occupied) = &section.occupied {
                for piece in &occupied.pieces {
                    if let Some(rect) = piece.polygon.bounding_rect() {
                        bounds = union_frame(
                            Some(bounds),
                            [[rect.min().x, rect.min().y], [rect.max().x, rect.max().y]],
                        );
                    }
                }
                for source in &occupied.tolerant {
                    let identity = std::sync::Arc::as_ptr(&source.model) as usize;
                    if seen.contains(&identity) {
                        continue;
                    }
                    let live_bytes = owned_bytes.saturating_add(vec_bytes(&seen));
                    reserve_owned(&mut seen, 1, live_bytes)?;
                    seen.push(identity);
                    let (lower, upper) = source.model.bounds();
                    let margin = source.model.support_margin();
                    bounds = union_frame(
                        Some(bounds),
                        [
                            [
                                (lower[0] - margin).next_down(),
                                (lower[1] - margin).next_down(),
                            ],
                            [(upper[0] + margin).next_up(), (upper[1] + margin).next_up()],
                        ],
                    );
                }
            }
        }
        let clearance_envelope = (clearance_distance_in_frame(bounds, bounds, requested)?
            + super::super::OFFSET_TOLERANCE_MM)
            .next_up();
        let margin = input.recipe.bead_width_mm / 2.0
            + clearance_envelope
            + super::super::OFFSET_TOLERANCE_MM;
        bounds = [
            bounds[0].map(|value| (value - margin).next_down()),
            bounds[1].map(|value| (value + margin).next_up()),
        ];
        if !bounds.iter().flatten().all(|value| value.is_finite()) {
            return Err(error(
                ManufacturingErrorCode::NativeConditioningFailure,
                "support clearance work frame has non-finite bounds",
            ));
        }
        let adapter = FloatPointAdapter::new(FloatRect {
            min_x: bounds[0][0],
            min_y: bounds[0][1],
            max_x: bounds[1][0],
            max_y: bounds[1][1],
        });
        if !adapter.inv_scale.is_finite()
            || adapter.inv_scale * 2.0f64.sqrt() > super::super::OFFSET_TOLERANCE_MM * 0.5
        {
            return Err(error(
                ManufacturingErrorCode::NativeConditioningFailure,
                "support clearance work frame is too coarse for the offset tolerance",
            ));
        }
        let distance = clearance_distance_in_frame(bounds, bounds, requested)?;
        if distance > clearance_envelope {
            return Err(error(
                ManufacturingErrorCode::NativeConditioningFailure,
                "support clearance offset leaves its fixed work frame",
            ));
        }
        Ok(Self {
            adapter,
            bounds,
            distance,
        })
    }

    pub(super) fn identity(&self) -> [u64; 5] {
        [
            self.bounds[0][0].to_bits(),
            self.bounds[0][1].to_bits(),
            self.bounds[1][0].to_bits(),
            self.bounds[1][1].to_bits(),
            self.distance.to_bits(),
        ]
    }

    fn difference(
        &self,
        column: &MultiPolygon<f64>,
        clearance: &MultiPolygon<f64>,
    ) -> MultiPolygon<f64> {
        self.overlay(column, clearance, OverlayRule::Difference)
    }

    fn intersection(
        &self,
        column: &MultiPolygon<f64>,
        clearance: &MultiPolygon<f64>,
    ) -> MultiPolygon<f64> {
        self.overlay(column, clearance, OverlayRule::Intersect)
    }

    fn overlay(
        &self,
        column: &MultiPolygon<f64>,
        clearance: &MultiPolygon<f64>,
        rule: OverlayRule,
    ) -> MultiPolygon<f64> {
        let subject = packed(column);
        let clip = packed(clearance);
        let capacity = subject.iter().chain(&clip).map(Vec::len).sum();
        let shapes = FloatOverlay::with_adapter(self.adapter.clone(), capacity)
            .unsafe_add_source(&subject, ShapeType::Subject)
            .unsafe_add_source(&clip, ShapeType::Clip)
            .overlay(rule, FillRule::EvenOdd);
        unpack(shapes)
    }
}

#[derive(Clone, Copy)]
struct Query {
    target: usize,
    root: usize,
    first_section: usize,
    last_section: usize,
    bed: f64,
    ceiling: f64,
}

struct Column {
    query: Query,
    material: Option<OwnedMaterial>,
}

pub(super) struct Columns {
    frame: Frame,
    cursor: Cursor,
    ready: Vec<Column>,
    single: bool,
}

#[derive(Clone, Copy, Default)]
struct Cursor {
    demand: usize,
    root: usize,
    top: Option<usize>,
}

impl Cursor {
    fn query(
        &mut self,
        input: &Input<'_>,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<Option<Query>, ManufacturingError> {
        while let Some(demand) = input.demands.get(self.demand) {
            check_cancel(cancel)?;
            let top = if let Some(top) = self.top {
                top
            } else if let Some(top) = contact_layer(
                input.layers,
                demand.target,
                &input.layers[demand.target].paths[demand.failure.path],
                input.gap,
                work,
                cancel,
            )? {
                self.top = Some(top);
                top
            } else {
                self.advance_demand();
                continue;
            };
            let root = if self.root == 0 {
                Root::Bed
            } else if let Some(root) = input.roots.get(self.root - 1) {
                *root
            } else {
                self.advance_demand();
                continue;
            };
            let first = root.first_layer();
            if first > top
                || (self.root == 0
                    && demand.target < input.valid
                    && input
                        .replay
                        .get(&demand.target)
                        .is_some_and(|(path, recorded_top, _)| {
                            *path == demand.failure.path && *recorded_top == top
                        }))
            {
                self.root += 1;
                continue;
            }
            return Ok(Some(Query {
                target: demand.target,
                root: self.root,
                first_section: span_at(input.support.sections, first.saturating_sub(1) as u32),
                last_section: span_at(
                    input.support.sections,
                    (top + 1).min(input.layers.len() - 1) as u32,
                ),
                bed: program_coordinates::project_xyz6_bead_height_mm(
                    input.layers[first].z_mm,
                    input.layers[first].height_mm,
                )?[0],
                ceiling: program_coordinates::project_xyz6_bead_height_mm(
                    input.layers[top].z_mm,
                    input.layers[top].height_mm,
                )?[1],
            }));
        }
        Ok(None)
    }

    fn advance_demand(&mut self) {
        self.demand += 1;
        self.root = 0;
        self.top = None;
    }
}

#[cfg(test)]
thread_local! {
    static SINGLE_COLUMN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(in crate::planner) fn with_single_column<T>(operation: impl FnOnce() -> T) -> T {
    struct Restore(bool);
    impl Drop for Restore {
        fn drop(&mut self) {
            SINGLE_COLUMN.with(|value| value.set(self.0));
        }
    }
    let _restore = Restore(SINGLE_COLUMN.with(|value| value.replace(true)));
    operation()
}

pub(super) struct Input<'a> {
    pub(super) support: &'a SupportInput<'a>,
    pub(super) layers: &'a [PrintLayer],
    pub(super) demands: &'a [Demand],
    pub(super) roots: &'a [Root],
    pub(super) landings: &'a [OwnedMaterial],
    pub(super) replay: &'a std::collections::BTreeMap<usize, (usize, usize, MultiPolygon<f64>)>,
    pub(super) valid: usize,
    pub(super) gap: f64,
    pub(super) requested: f64,
}

pub(super) struct Admission<'a> {
    pub(super) retained: usize,
    pub(super) owned_bytes: usize,
    pub(super) support: &'a [Rc<OwnedMaterial>],
    pub(super) interfaces: &'a [Rc<OwnedMaterial>],
    pub(super) protected: &'a OwnedMaterial,
    pub(super) foreign_work: usize,
}

impl Admission<'_> {
    fn geometry_bytes(&self) -> usize {
        geometry_receipt(self.support, self.interfaces, &[self.protected]).1
    }

    fn scratch(
        &self,
        live: (usize, usize),
        temporary: &[&OwnedMaterial],
        operands: (usize, usize),
    ) -> Result<(), ManufacturingError> {
        remaining_geometry_budget(
            self.retained
                .saturating_add(live.0)
                .saturating_add(operands.0.saturating_mul(3)),
            self.owned_bytes
                .saturating_add(live.1)
                .saturating_add(operands.1.saturating_mul(3)),
            self.support,
            self.interfaces,
            temporary,
        )?;
        Ok(())
    }
}

impl Columns {
    pub(super) fn new(frame: Frame) -> Self {
        let single = std::env::var_os("SPILING_SUPPORT_PROJECTION_SINGLE_COLUMN").is_some();
        #[cfg(test)]
        let single = single || SINGLE_COLUMN.with(std::cell::Cell::get);
        Self {
            frame,
            cursor: Cursor::default(),
            ready: Vec::new(),
            single,
        }
    }

    pub(super) fn receipt(&self) -> (usize, usize) {
        let (units, bytes) = column_receipt(&self.ready);
        (units, bytes.saturating_add(vec_bytes(&self.ready)))
    }

    pub(super) fn take(
        &mut self,
        identity: (usize, usize),
        input: &Input<'_>,
        admission: &Admission<'_>,
        work: &mut usize,
        trace: &ProjectionTrace,
        cancel: &AtomicBool,
    ) -> Result<OwnedMaterial, ManufacturingError> {
        check_cancel(cancel)?;
        if self.ready.iter().all(|column| column.material.is_none()) {
            self.prepare(input, admission, work, trace, cancel)?;
        }
        let index = self
            .ready
            .binary_search_by_key(&identity, |column| (column.query.target, column.query.root))
            .map_err(|_| {
                error(
                    ManufacturingErrorCode::NativeAnalysisIndeterminate,
                    "support column cohort lost its demand/root identity",
                )
            })?;
        self.ready[index].material.take().ok_or_else(|| {
            error(
                ManufacturingErrorCode::NativeAnalysisIndeterminate,
                "support column cohort was consumed twice",
            )
        })
    }

    fn prepare(
        &mut self,
        input: &Input<'_>,
        admission: &Admission<'_>,
        work: &mut usize,
        trace: &ProjectionTrace,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        let _column_time = trace.measure(&trace.column_time);
        let start = self.cursor;
        let mut limit = if self.single { 1 } else { usize::MAX };
        loop {
            check_cancel(cancel)?;
            self.ready.clear();
            self.cursor = start;
            self.seed(input, admission, limit, work, trace, cancel)?;
            let result = process(
                &mut self.ready,
                &self.frame,
                input,
                admission,
                work,
                trace,
                cancel,
            );
            if result
                .as_ref()
                .is_err_and(|failure| failure.code == ManufacturingErrorCode::ResourceLimit)
                && self.ready.len() > 1
            {
                limit = self.ready.len() / 2;
                self.ready = Vec::new();
                trace.add(&trace.cohort_retries, 1);
                continue;
            }
            result?;
            trace.add(&trace.cohorts, 1);
            return Ok(());
        }
    }

    fn seed(
        &mut self,
        input: &Input<'_>,
        admission: &Admission<'_>,
        limit: usize,
        work: &mut usize,
        trace: &ProjectionTrace,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        let state_allowance = crate::working_set::bytes()
            .saturating_sub(admission.owned_bytes)
            .saturating_sub(admission.geometry_bytes())
            / 8;
        let mut state_bytes = 0usize;
        let mut search: Option<(usize, OwnedMaterial)> = None;
        while let Some(query) = self.cursor.query(input, work, cancel)? {
            check_cancel(cancel)?;
            if self.ready.len() == limit {
                break;
            }
            let live = self.receipt();
            if search
                .as_ref()
                .is_none_or(|(target, _)| *target != query.target)
            {
                drop(search.take());
                let candidate = &input.support.sections
                    [span_at(input.support.sections, query.target as u32)]
                .material;
                let scratch = (material_units(candidate), packed_bytes(candidate));
                let admitted = admission.scratch(live, &[admission.protected], scratch);
                if !self.ready.is_empty()
                    && admitted
                        .as_ref()
                        .is_err_and(|failure| failure.code == ManufacturingErrorCode::ResourceLimit)
                {
                    break;
                }
                admitted?;
                charge_boolean(candidate, candidate, work, admission.foreign_work, cancel)?;
                let material = OwnedMaterial::new(
                    candidate.buffer_with_style(
                        BufferStyle::new(input.support.recipe.bead_width_mm / 2.0)
                            .line_join(LineJoin::Bevel),
                    ),
                );
                let admitted = remaining_geometry_budget(
                    admission.retained.saturating_add(live.0),
                    admission.owned_bytes.saturating_add(live.1),
                    admission.support,
                    admission.interfaces,
                    &[admission.protected, &material],
                );
                if !self.ready.is_empty()
                    && admitted
                        .as_ref()
                        .is_err_and(|failure| failure.code == ManufacturingErrorCode::ResourceLimit)
                {
                    break;
                }
                admitted?;
                search = Some((query.target, material));
            }
            let search_material = &search.as_ref().expect("prepared cohort search").1;
            let landing = if query.root == 0 {
                None
            } else {
                let Root::Part { landing, .. } = input.roots[query.root - 1] else {
                    unreachable!()
                };
                Some(&input.landings[landing])
            };
            let scratch = (
                search_material
                    .units
                    .saturating_add(landing.map_or(0, |landing| landing.units)),
                search_material
                    .bytes
                    .saturating_add(landing.map_or(0, |landing| landing.bytes)),
            );
            let admitted =
                admission.scratch(live, &[admission.protected, search_material], scratch);
            if !self.ready.is_empty()
                && admitted
                    .as_ref()
                    .is_err_and(|failure| failure.code == ManufacturingErrorCode::ResourceLimit)
            {
                break;
            }
            admitted?;
            let material = if query.root == 0 {
                OwnedMaterial::new(search_material.material.clone())
            } else {
                let landing = landing.expect("part-root landing");
                charge_boolean(
                    search_material,
                    landing,
                    work,
                    admission.foreign_work,
                    cancel,
                )?;
                OwnedMaterial::new(self.frame.intersection(search_material, landing))
            };
            region::check_size(&material)?;
            if trace.enabled {
                trace.frame_distance.set(self.frame.distance);
                trace.frame_quantum.set(self.frame.adapter.inv_scale);
                if let Some(screen) = ColumnScreen::new(&material, input.requested)? {
                    trace
                        .old_max_reach
                        .set(trace.old_max_reach.get().max(screen.reach));
                }
            }
            if !self.ready.is_empty()
                && state_bytes.saturating_add(material.bytes) > state_allowance
            {
                break;
            }
            let admitted = remaining_geometry_budget(
                admission.retained.saturating_add(live.0),
                admission.owned_bytes.saturating_add(live.1),
                admission.support,
                admission.interfaces,
                &[admission.protected, search_material, &material],
            );
            if !self.ready.is_empty()
                && admitted
                    .as_ref()
                    .is_err_and(|failure| failure.code == ManufacturingErrorCode::ResourceLimit)
            {
                break;
            }
            admitted?;
            let reserved = reserve_owned(
                &mut self.ready,
                1,
                admission
                    .owned_bytes
                    .saturating_add(live.1)
                    .saturating_add(material.bytes)
                    .saturating_add(search_material.bytes)
                    .saturating_add(admission.geometry_bytes()),
            );
            if !self.ready.is_empty()
                && reserved
                    .as_ref()
                    .is_err_and(|failure| failure.code == ManufacturingErrorCode::ResourceLimit)
            {
                break;
            }
            reserved?;
            state_bytes = state_bytes.saturating_add(material.bytes);
            self.ready.push(Column {
                query,
                material: Some(material),
            });
            self.cursor.root += 1;
        }
        drop(search);
        if self.ready.is_empty() {
            return Err(error(
                ManufacturingErrorCode::NativeAnalysisIndeterminate,
                "support column cohort is unexpectedly empty",
            ));
        }
        Ok(())
    }
}

fn column_receipt(columns: &[Column]) -> (usize, usize) {
    columns
        .iter()
        .filter_map(|column| column.material.as_ref())
        .fold((0usize, 0usize), |(units, bytes), material| {
            (
                units.saturating_add(material.units),
                bytes.saturating_add(material.bytes),
            )
        })
}

fn packed_bytes(material: &MultiPolygon<f64>) -> usize {
    material_units(material)
        .saturating_mul(std::mem::size_of::<geo::Coord<f64>>())
        .saturating_add(
            material
                .0
                .len()
                .saturating_mul(std::mem::size_of::<Polygon<f64>>()),
        )
        .saturating_add(
            material
                .0
                .iter()
                .map(|polygon| polygon.interiors().len())
                .sum::<usize>()
                .saturating_mul(std::mem::size_of::<geo::LineString<f64>>()),
        )
}

fn process(
    columns: &mut Vec<Column>,
    frame: &Frame,
    input: &Input<'_>,
    admission: &Admission<'_>,
    work: &mut usize,
    trace: &ProjectionTrace,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let mut groups: Vec<(f64, f64, usize)> = Vec::new();
    reserve_owned(
        &mut groups,
        columns.len(),
        admission
            .owned_bytes
            .saturating_add(vec_bytes(columns))
            .saturating_add(column_receipt(columns).1)
            .saturating_add(admission.geometry_bytes()),
    )?;
    for (section_index, section) in input.support.sections.iter().enumerate() {
        check_cancel(cancel)?;
        groups.clear();
        let interval = [
            input.support.schedule[section.start as usize].lower_z,
            input.support.schedule[(section.end - 1) as usize].z,
        ];
        for (index, column) in columns.iter().enumerate() {
            if column
                .material
                .as_ref()
                .is_none_or(|material| material.0.is_empty())
                || section_index < column.query.first_section
                || section_index > column.query.last_section
            {
                continue;
            }
            let lower = interval[0].max(column.query.bed);
            let upper = interval[1].min(column.query.ceiling);
            if lower <= upper {
                groups.push((lower, upper, index));
                trace.add(&trace.visits, 1);
            }
        }
        super::super::boolean::charge(
            work,
            groups
                .len()
                .saturating_mul(groups.len().max(1).ilog2() as usize + 1),
        )?;
        groups.sort_unstable_by(|left, right| {
            left.0
                .total_cmp(&right.0)
                .then(left.1.total_cmp(&right.1))
                .then(left.2.cmp(&right.2))
        });
        check_cancel(cancel)?;
        let mut first = 0;
        while first < groups.len() {
            let mut end = first + 1;
            while end < groups.len()
                && groups[end].0.to_bits() == groups[first].0.to_bits()
                && groups[end].1.to_bits() == groups[first].1.to_bits()
            {
                end += 1;
            }
            let scope = [groups[first].0, groups[first].1];
            trace.add(&trace.section_builds, 1);
            let live_metadata = vec_bytes(columns).saturating_add(vec_bytes(&groups));
            let mut batch = PieceBatch::default();
            let live = std::cell::Cell::new(column_receipt(columns));
            let scanned = std::cell::Cell::new(0usize);
            let scoped = |height: Option<[f64; 2]>| {
                height.map_or(interval, |height| {
                    [height[0].max(interval[0]), height[1].min(interval[1])]
                })
            };
            let result = section.visit_occupied_screened(
                cancel,
                |_, height| {
                    check_cancel(cancel)?;
                    scanned.set(scanned.get().saturating_add(1));
                    let [lower, upper] = scoped(height);
                    Ok(lower <= upper && lower <= scope[1] && upper >= scope[0])
                },
                |piece, height| {
                    check_cancel(cancel)?;
                    let [lower, upper] = scoped(height);
                    if lower > upper || lower > scope[1] || upper < scope[0] {
                        return Ok(true);
                    }
                    trace.add(&trace.built, 1);
                    super::super::boolean::budget_polygon_accumulated(piece, work, cancel)?;
                    let (units, bytes) = live.get();
                    batch.stage(
                        piece,
                        admission.retained.saturating_add(units),
                        admission
                            .owned_bytes
                            .saturating_add(bytes)
                            .saturating_add(live_metadata),
                        admission.support,
                        admission.interfaces,
                        &[admission.protected],
                    )?;
                    trace.add(&trace.staged, 1);
                    if batch.points >= PieceBatch::FLUSH_POINTS {
                        apply_batch(
                            &mut batch,
                            columns,
                            &groups[first..end],
                            frame,
                            admission,
                            live_metadata,
                            work,
                            trace,
                            cancel,
                        )?;
                        live.set(column_receipt(columns));
                        return Ok(groups[first..end].iter().any(|(_, _, index)| {
                            columns[*index]
                                .material
                                .as_ref()
                                .is_some_and(|material| !material.0.is_empty())
                        }));
                    }
                    Ok(true)
                },
            );
            trace.add(&trace.screened, scanned.get());
            if let Some(maximum) = trace.section_pieces.get(section.start as usize) {
                maximum.set(maximum.get().max(scanned.get()));
            }
            *work = work.checked_add(scanned.get()).ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "shared support projection work counter overflow",
                )
            })?;
            result?;
            apply_batch(
                &mut batch,
                columns,
                &groups[first..end],
                frame,
                admission,
                live_metadata,
                work,
                trace,
                cancel,
            )?;
            first = end;
        }
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "Shared batch subtraction inventories the cohort and caller-owned operands"
)]
fn apply_batch(
    batch: &mut PieceBatch,
    columns: &mut [Column],
    group: &[(f64, f64, usize)],
    frame: &Frame,
    admission: &Admission<'_>,
    metadata: usize,
    work: &mut usize,
    trace: &ProjectionTrace,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    if batch.pieces.0.is_empty() {
        return Ok(());
    }
    check_cancel(cancel)?;
    trace.add(&trace.batches, 1);
    let staged = std::mem::take(batch);
    if let Some(bounds) = staged.frame {
        let reach = separation_reach(frame.distance);
        if group.iter().all(|&(_, _, index)| {
            columns[index]
                .material
                .as_ref()
                .and_then(|material| material_bounds(material))
                .is_none_or(|column| {
                    ColumnScreen {
                        column,
                        frame: frame.bounds,
                        reach,
                    }
                    .rejects(bounds)
                })
        }) {
            return Ok(());
        }
    }
    let (units, bytes) = column_receipt(columns);
    let retained = admission.retained.saturating_add(units);
    let owned = admission
        .owned_bytes
        .saturating_add(bytes)
        .saturating_add(metadata);
    remaining_geometry_budget(
        retained.saturating_add(staged.points.saturating_mul(3)),
        owned.saturating_add(staged.owned_bytes().saturating_mul(3)),
        admission.support,
        admission.interfaces,
        &[admission.protected],
    )?;
    boolean_budget(&staged.pieces, &staged.pieces, cancel)?;
    let union_time = trace.measure(&trace.union_time);
    let merged = OwnedMaterial::new(union_pieces_in_frame(&staged.pieces, Some(frame)));
    drop(union_time);
    region::check_size(&merged)?;
    let offset_time = trace.measure(&trace.offset_time);
    let clearance = OwnedMaterial::new(region::offset_admitted_in_frame(
        &merged,
        frame.distance,
        frame,
        |scratch, scratch_bytes, overlay| {
            remaining_geometry_budget(
                retained
                    .saturating_add(staged.points)
                    .saturating_add(scratch),
                owned
                    .saturating_add(staged.owned_bytes())
                    .saturating_add(scratch_bytes),
                admission.support,
                admission.interfaces,
                &[admission.protected, &merged],
            )?;
            if let Some((left, right)) = overlay {
                boolean_budget(left, right, cancel)?;
            }
            Ok(())
        },
        cancel,
    )?);
    drop(offset_time);
    region::check_size(&clearance)?;
    drop(staged);
    drop(merged);
    let bounds = material_bounds(&clearance);
    let mut live = column_receipt(columns);
    for &(_, _, index) in group {
        check_cancel(cancel)?;
        let material = columns[index]
            .material
            .as_ref()
            .expect("unconsumed support cohort");
        let Some(column_bounds) = material_bounds(material) else {
            continue;
        };
        let Some(bounds) = bounds else {
            continue;
        };
        let screen = ColumnScreen {
            column: column_bounds,
            frame: frame.bounds,
            reach: frame.adapter.inv_scale * 2.0f64.sqrt(),
        };
        if screen.rejects(bounds) {
            continue;
        }
        let outside_units = admission
            .retained
            .saturating_add(live.0.saturating_sub(material.units));
        let outside_bytes = admission
            .owned_bytes
            .saturating_add(metadata)
            .saturating_add(live.1.saturating_sub(material.bytes));
        remaining_geometry_budget(
            outside_units.saturating_add(
                material
                    .units
                    .saturating_add(clearance.units)
                    .saturating_mul(3),
            ),
            outside_bytes.saturating_add(
                material
                    .bytes
                    .saturating_add(clearance.bytes)
                    .saturating_mul(3),
            ),
            admission.support,
            admission.interfaces,
            &[admission.protected, material, &clearance],
        )?;
        charge_boolean(material, &clearance, work, admission.foreign_work, cancel)?;
        let difference_time = trace.measure(&trace.difference_time);
        let next = OwnedMaterial::new(frame.difference(material, &clearance));
        drop(difference_time);
        region::check_size(&next)?;
        remaining_geometry_budget(
            outside_units,
            outside_bytes,
            admission.support,
            admission.interfaces,
            &[admission.protected, material, &clearance, &next],
        )?;
        live.0 = live.0 - material.units + next.units;
        live.1 = live.1 - material.bytes + next.bytes;
        columns[index].material = Some(next);
    }
    Ok(())
}

fn packed(material: &MultiPolygon<f64>) -> Vec<Vec<[f64; 2]>> {
    material
        .0
        .iter()
        .flat_map(|polygon| std::iter::once(polygon.exterior()).chain(polygon.interiors()))
        .map(|ring| ring.0.iter().map(|point| [point.x, point.y]).collect())
        .collect()
}

fn unpack(shapes: Vec<Vec<Vec<[f64; 2]>>>) -> MultiPolygon<f64> {
    MultiPolygon(
        shapes
            .into_iter()
            .map(|shape| {
                let mut rings = shape.into_iter().map(|ring| {
                    let mut line =
                        geo::LineString(ring.into_iter().map(geo::Coord::from).collect());
                    line.close();
                    line
                });
                Polygon::new(
                    rings.next().unwrap_or_else(geo::LineString::empty),
                    rings.collect(),
                )
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{Area, LineString, polygon};

    fn rectangle(left: f64, bottom: f64, right: f64, top: f64) -> Polygon<f64> {
        polygon![(x:left,y:bottom),(x:right,y:bottom),(x:right,y:top),(x:left,y:top)]
    }

    fn support_input<'a>(
        sections: &'a [SectionSpan],
        schedule: &'a [super::super::super::LayerElevation],
        recipe: &'a PlanarPrintRecipe,
    ) -> SupportInput<'a> {
        SupportInput {
            build_bounds: [[-30.0, -30.0], [100.0, 100.0]],
            sections,
            schedule,
            native_points: 0,
            native_owned_bytes: 0,
            recipe,
            nominal_height_mm: 0.2,
            source_z_allowance_mm: 0.0,
            replay: false,
            phases: &[],
        }
    }

    #[test]
    fn shared_sections_preserve_streamed_clearance_and_contact_for_physical_scopes() {
        let cancel = AtomicBool::new(false);
        let elevations = [0.0, 0.2, 0.55, 1.1, 1.2];
        let schedule: Vec<_> = elevations
            .windows(2)
            .enumerate()
            .map(|(index, height)| super::super::super::LayerElevation {
                lower_z: height[0],
                z: height[1],
                sample: (height[0] + height[1]) / 2.0,
                height: height[1] - height[0],
                course: index as u32,
            })
            .collect();
        let sections: Vec<_> = schedule
            .iter()
            .enumerate()
            .map(|(index, elevation)| {
                let mut occupied = super::super::super::ContinuousOccupied::default();
                for repeat in 0..3500 {
                    let polygon = if repeat % 2 == 0 {
                        rectangle(2.0 + index as f64, 1.0, 3.0 + index as f64, 11.0)
                    } else {
                        Polygon::new(
                            rectangle(7.0, 3.0, 10.0, 9.0).exterior().clone(),
                            vec![rectangle(7.5, 4.0, 9.5, 8.0).exterior().clone()],
                        )
                    };
                    let scope = if repeat % 3 == 0 {
                        [elevation.z, elevation.z]
                    } else {
                        [elevation.lower_z, elevation.z]
                    };
                    occupied.push(polygon, scope).unwrap();
                }
                SectionSpan {
                    start: index as u32,
                    end: index as u32 + 1,
                    material: Arc::new(MultiPolygon(vec![rectangle(0.0, 0.0, 12.0, 12.0)])),
                    process_domains: Vec::new(),
                    occupied: Some(Arc::new(occupied)),
                    source_area_mm2: 144.0,
                }
            })
            .collect();
        let recipe = crate::planner::tests::recipe(1.0);
        let support = support_input(&sections, &schedule, &recipe);
        let frame = Frame::new(&support, 0.2, 0, &cancel).unwrap();
        let protected = OwnedMaterial::new(MultiPolygon(Vec::new()));
        let admission = Admission {
            retained: 0,
            owned_bytes: 0,
            support: &[],
            interfaces: &[],
            protected: &protected,
            foreign_work: 0,
        };
        let replay = Default::default();
        let input = Input {
            support: &support,
            layers: &[],
            demands: &[],
            roots: &[],
            landings: &[],
            replay: &replay,
            valid: 0,
            gap: 0.2,
            requested: 0.2,
        };
        let circle = Polygon::new(
            LineString(
                (0..=64)
                    .map(|index| {
                        let angle = index as f64 * std::f64::consts::TAU / 64.0;
                        geo::Coord {
                            x: 6.0 + 6.0 * angle.cos(),
                            y: 6.0 + 6.0 * angle.sin(),
                        }
                    })
                    .collect(),
            ),
            Vec::new(),
        );
        let original = MultiPolygon(vec![circle]);
        let queries: Vec<_> = [(0.0, 1.2), (0.2, 0.55), (0.55, 1.1), (0.551, 0.9)]
            .into_iter()
            .enumerate()
            .map(|(index, (bed, ceiling))| Query {
                target: index,
                root: usize::from(index > 0),
                first_section: 0,
                last_section: sections.len() - 1,
                bed,
                ceiling,
            })
            .collect();
        let mut columns: Vec<_> = queries
            .iter()
            .map(|&query| Column {
                query,
                material: Some(OwnedMaterial::new(original.clone())),
            })
            .collect();
        let trace = ProjectionTrace::new(schedule.len(), 0, 0);
        process(
            &mut columns,
            &frame,
            &input,
            &admission,
            &mut 0,
            &trace,
            &cancel,
        )
        .unwrap();
        for (query, column) in queries.iter().zip(&columns) {
            let mut expected = original.clone();
            for (section, elevation) in sections.iter().zip(&schedule) {
                let pieces = MultiPolygon(
                    section
                        .occupied
                        .as_ref()
                        .unwrap()
                        .pieces
                        .iter()
                        .filter(|piece| {
                            let lower = piece.height_mm[0].max(elevation.lower_z);
                            let upper = piece.height_mm[1].min(elevation.z);
                            lower <= upper && lower <= query.ceiling && upper >= query.bed
                        })
                        .map(|piece| piece.polygon.clone())
                        .collect(),
                );
                let merged = union_pieces_in_frame(&pieces, Some(&frame));
                let clearance = region::offset_admitted_in_frame(
                    &merged,
                    frame.distance,
                    &frame,
                    |_, _, _| Ok(()),
                    &cancel,
                )
                .unwrap();
                expected = frame.difference(&expected, &clearance);
            }
            let actual = column.material.as_ref().unwrap();
            assert_eq!(frame.difference(actual, &expected).unsigned_area(), 0.0);
            assert_eq!(frame.difference(&expected, actual).unsigned_area(), 0.0);
            let contact = MultiPolygon(vec![rectangle(0.5, 0.5, 11.5, 11.5)]);
            assert_eq!(
                frame.intersection(actual, &contact).unsigned_area(),
                frame.intersection(&expected, &contact).unsigned_area(),
            );
        }
        let failure = process(
            &mut columns,
            &frame,
            &input,
            &admission,
            &mut 0,
            &trace,
            &AtomicBool::new(true),
        )
        .unwrap_err();
        assert_eq!(failure.code, ManufacturingErrorCode::Cancelled);
        let failure =
            crate::working_set::scope(crate::working_set::WorkingSet::new(1).unwrap(), || {
                process(
                    &mut columns,
                    &frame,
                    &input,
                    &admission,
                    &mut 0,
                    &trace,
                    &cancel,
                )
            })
            .unwrap_err();
        assert_eq!(failure.code, ManufacturingErrorCode::ResourceLimit);

        let layers: Vec<_> = schedule
            .iter()
            .enumerate()
            .map(|(index, elevation)| PrintLayer {
                index: index as u32,
                hatch_course: elevation.course,
                lower_z_mm: elevation.lower_z,
                z_mm: elevation.z,
                section_z_mm: elevation.sample,
                height_mm: elevation.height,
                paths: vec![DepositionPath {
                    material_domain: None,
                    kind: DepositionKind::Perimeter,
                    owner: Some(DepositionOwner::Wall),
                    width: WidthProfile::Constant { width_mm: 0.4 },
                    physical_z_mm: [elevation.lower_z, elevation.z],
                    points_mm: vec![[0.0, 0.0, elevation.z], [12.0, 0.0, elevation.z]],
                    arcs: Vec::new(),
                    exposed: false,
                    connectors: Vec::new(),
                }],
                region_coverage: crate::planner::empty_coverage(144.0),
                skin_command_fits: Vec::new(),
            })
            .collect();
        let demands = [Demand {
            target: 3,
            failure: strokes::Failure {
                path: 0,
                first: 0,
                last: 1,
                start: 0.0,
                end: 1.0,
                span: 12.0,
            },
        }];
        let input = Input {
            layers: &layers,
            demands: &demands,
            ..input
        };
        let frame = Frame::new(&support, 0.2, 0, &cancel).unwrap();
        let mut columns = Columns::new(frame);
        columns.single = true;
        let mut trace = ProjectionTrace::new(schedule.len(), 0, 0);
        trace.enabled = true;
        let failure = crate::working_set::scope(
            crate::working_set::WorkingSet::new(16 * 1024).unwrap(),
            || columns.prepare(&input, &admission, &mut 0, &trace, &cancel),
        )
        .unwrap_err();
        assert_eq!(failure.code, ManufacturingErrorCode::ResourceLimit);
        assert_eq!(columns.ready.len(), 1);
        assert!(trace.section_builds.get() > 0);
        assert_eq!(trace.cohort_retries.get(), 0);
        let failure = columns
            .prepare(&input, &admission, &mut 0, &trace, &AtomicBool::new(true))
            .unwrap_err();
        assert_eq!(failure.code, ManufacturingErrorCode::Cancelled);
        let perforated: Vec<_> = sections
            .iter()
            .enumerate()
            .map(|(index, section)| {
                let mut occupied = crate::planner::ContinuousOccupied::default();
                if index == 0 {
                    for row in 0..8 {
                        for column in 0..8 {
                            let left = 1.0 + column as f64 * 1.25;
                            let bottom = 1.0 + row as f64 * 1.25;
                            occupied
                                .push(
                                    rectangle(left, bottom, left + 0.4, bottom + 0.4),
                                    [0.0, 0.2],
                                )
                                .unwrap();
                        }
                    }
                }
                SectionSpan {
                    start: section.start,
                    end: section.end,
                    material: Arc::clone(&section.material),
                    process_domains: Vec::new(),
                    occupied: Some(Arc::new(occupied)),
                    source_area_mm2: section.source_area_mm2,
                }
            })
            .collect();
        let support = support_input(&perforated, &schedule, &recipe);
        let roots = vec![
            Root::Part {
                object_layer: 0,
                first_support_layer: 0,
                landing: 0
            };
            128
        ];
        let landings = [OwnedMaterial::new(MultiPolygon(vec![rectangle(
            0.0, 0.0, 12.0, 12.0,
        )]))];
        let input = Input {
            support: &support,
            roots: &roots,
            landings: &landings,
            ..input
        };
        let frame = Frame::new(&support, 0.2, 0, &cancel).unwrap();
        let mut shared = Columns::new(frame);
        let mut trace = ProjectionTrace::new(schedule.len(), 0, 0);
        trace.enabled = true;
        crate::working_set::scope(
            crate::working_set::WorkingSet::new(512 * 1024).unwrap(),
            || shared.prepare(&input, &admission, &mut 0, &trace, &cancel),
        )
        .unwrap();
        assert!(trace.cohort_retries.get() > 0);
        assert!(shared.ready.len() < roots.len());
        let frame = Frame::new(&support, 0.2, 0, &cancel).unwrap();
        let mut single = Columns::new(frame);
        single.single = true;
        single
            .prepare(&input, &admission, &mut 0, &trace, &cancel)
            .unwrap();
        assert_eq!(
            shared.ready[0].material.as_ref().unwrap().material,
            single.ready[0].material.as_ref().unwrap().material
        );
    }

    #[test]
    fn remote_crossing_strips_never_allocate_their_irrelevant_union() {
        let recipe = crate::planner::tests::recipe(1.0);
        let support = support_input(&[], &[], &recipe);
        let cancel = AtomicBool::new(false);
        let frame = Frame::new(&support, 0.2, 0, &cancel).unwrap();
        let original = MultiPolygon(vec![rectangle(0.0, 0.0, 1.0, 1.0)]);
        let mut columns = [Column {
            query: Query {
                target: 0,
                root: 0,
                first_section: 0,
                last_section: 0,
                bed: 0.0,
                ceiling: 1.0,
            },
            material: Some(OwnedMaterial::new(original.clone())),
        }];
        let protected = OwnedMaterial::new(MultiPolygon(Vec::new()));
        let admission = Admission {
            retained: 0,
            owned_bytes: 0,
            support: &[],
            interfaces: &[],
            protected: &protected,
            foreign_work: 0,
        };
        let trace = ProjectionTrace::new(0, 0, 0);
        crate::working_set::scope(
            crate::working_set::WorkingSet::new(512 * 1024).unwrap(),
            || {
                let mut batch = PieceBatch::default();
                for index in 0..100 {
                    let start = 20.0 + index as f64 * 0.6;
                    batch
                        .stage(
                            &rectangle(start, 20.0, start + 0.2, 80.0),
                            0,
                            0,
                            &[],
                            &[],
                            &[],
                        )
                        .unwrap();
                    batch
                        .stage(
                            &rectangle(20.0, start, 80.0, start + 0.2),
                            0,
                            0,
                            &[],
                            &[],
                            &[],
                        )
                        .unwrap();
                }
                assert!(batch.points < PieceBatch::FLUSH_POINTS);
                apply_batch(
                    &mut batch,
                    &mut columns,
                    &[(0.0, 1.0, 0)],
                    &frame,
                    &admission,
                    0,
                    &mut 0,
                    &trace,
                    &cancel,
                )
                .unwrap();
                assert!(batch.pieces.0.is_empty());
            },
        );
        assert_eq!(columns[0].material.as_ref().unwrap().material, original);
    }

    #[test]
    fn near_tangent_contact_length_does_not_measure_normal_boundary_error() {
        let recipe = crate::planner::tests::recipe(1.0);
        let support = support_input(&[], &[], &recipe);
        let cancel = AtomicBool::new(false);
        let frame = Frame::new(&support, 0.2, 0, &cancel).unwrap();
        let quantum = frame.adapter.inv_scale;
        let slope = quantum / (4.0 * crate::planner::OFFSET_TOLERANCE_MM);
        let lower = DepositionPath {
            material_domain: None,
            kind: DepositionKind::SupportInterface,
            owner: None,
            width: WidthProfile::Constant { width_mm: 0.45 },
            physical_z_mm: [0.0, 0.2],
            points_mm: vec![[0.0, 0.0, 0.2], [10.0, 0.0, 0.2]],
            arcs: Vec::new(),
            exposed: false,
            connectors: Vec::new(),
        };
        let candidate = AnchorCandidate {
            endpoints_mm: [
                [1.0, 0.225 - 4.0 * slope, 0.4],
                [9.0, 0.225 + 4.0 * slope, 0.4],
            ],
            bead_width_mm: 0.45,
        };
        let mut shifted = lower.clone();
        for point in &mut shifted.points_mm {
            point[1] -= quantum;
        }
        let mut workspace = AnchorWorkspace::default();
        let original = workspace
            .gaps(
                candidate,
                std::iter::empty(),
                std::iter::once(&lower),
                &mut 0,
                &cancel,
            )
            .unwrap()[0]
            .start;
        let changed = workspace
            .gaps(
                candidate,
                std::iter::empty(),
                std::iter::once(&shifted),
                &mut 0,
                &cancel,
            )
            .unwrap()[0]
            .start;
        let length = (candidate.endpoints_mm[1][0] - candidate.endpoints_mm[0][0])
            .hypot(candidate.endpoints_mm[1][1] - candidate.endpoints_mm[0][1]);
        assert!((original - changed) * length > crate::planner::OFFSET_TOLERANCE_MM);
        assert!(quantum < crate::planner::OFFSET_TOLERANCE_MM);
    }

    #[test]
    fn fixed_frame_checks_quantum_and_replay_identity_before_projection() {
        let recipe = crate::planner::tests::recipe(1.0);
        let mut support = support_input(&[], &[], &recipe);
        let cancel = AtomicBool::new(false);
        let frame = Frame::new(&support, 0.2, 0, &cancel).unwrap();
        assert!(
            frame.adapter.inv_scale * 2.0f64.sqrt() <= crate::planner::OFFSET_TOLERANCE_MM / 2.0
        );
        let seed = MultiPolygon(vec![rectangle(0.0, 0.0, 12.0, 12.0)]);
        let old = ColumnScreen::new(&seed, 0.2).unwrap().unwrap();
        assert!(frame.distance <= old.reach);
        assert_eq!(
            frame.identity(),
            Frame::new(&support, 0.2, 0, &cancel).unwrap().identity()
        );
        assert_ne!(
            frame.identity(),
            Frame::new(&support, 0.3, 0, &cancel).unwrap().identity()
        );
        support.build_bounds[1][0] = 1e10;
        let failure = Frame::new(&support, 0.2, 0, &cancel).err().unwrap();
        assert_eq!(
            failure.code,
            ManufacturingErrorCode::NativeConditioningFailure
        );
        let failure = Frame::new(&support, 0.2, 0, &AtomicBool::new(true))
            .err()
            .unwrap();
        assert_eq!(failure.code, ManufacturingErrorCode::Cancelled);
    }
}
