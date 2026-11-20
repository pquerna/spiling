use super::*;
use i_overlay::core::overlay::ShapeType;
use i_overlay::core::{fill_rule::FillRule, overlay_rule::OverlayRule};
use i_overlay::float::overlay::FloatOverlay;
use i_overlay::i_float::{adapter::FloatPointAdapter, float::rect::FloatRect};

pub(in crate::planner) struct Frame {
    pub(in crate::planner) adapter: FloatPointAdapter<[f64; 2], f64>,
    pub(super) bounds: [[f64; 2]; 2],
    pub(super) distance: f64,
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

    #[cfg(test)]
    pub(super) fn identity(&self) -> [u64; 5] {
        [
            self.bounds[0][0].to_bits(),
            self.bounds[0][1].to_bits(),
            self.bounds[1][0].to_bits(),
            self.bounds[1][1].to_bits(),
            self.distance.to_bits(),
        ]
    }

    pub(super) fn difference(
        &self,
        column: &MultiPolygon<f64>,
        clearance: &MultiPolygon<f64>,
    ) -> MultiPolygon<f64> {
        self.overlay(column, clearance, OverlayRule::Difference)
    }

    pub(super) fn union(
        &self,
        column: &MultiPolygon<f64>,
        clearance: &MultiPolygon<f64>,
    ) -> MultiPolygon<f64> {
        self.overlay(column, clearance, OverlayRule::Union)
    }

    pub(super) fn intersection(
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
    use geo::polygon;

    fn rectangle(left: f64, bottom: f64, right: f64, top: f64) -> Polygon<f64> {
        polygon![(x:left,y:bottom),(x:right,y:bottom),(x:right,y:top),(x:left,y:top)]
    }

    #[test]
    fn tree_y_screen_preserves_closed_contacts_and_rounded_crossings() {
        let mut paths = Vec::new();
        for row in 0..64 {
            for column in 0..20 {
                let left = f64::from(column) * 64.0;
                let bottom = f64::from(row) * 0.125;
                paths.push(vec![
                    [left, bottom],
                    [left + 62.0, bottom],
                    [left + 62.0, bottom + 0.0625],
                    [left, bottom + 0.0625],
                ]);
            }
        }
        paths.extend([
            vec![[3.0, 0.0625], [59.0, 0.0625], [31.0, 0.125]],
            vec![[2.0, -1.0], [47.0, 2.1], [4.0, 2.4]],
            vec![[61.0, 2.2], [0.0, 1.9], [2.0, 0.01]],
        ]);
        let frame = FloatPointAdapter::new(FloatRect {
            min_x: -1.0,
            min_y: -2.0,
            max_x: 1281.0,
            max_y: 10.0,
        });
        let capacity = paths.iter().map(Vec::len).sum();
        let unscreened = FloatOverlay::with_adapter(frame.clone(), capacity)
            .unsafe_add_source(&paths, ShapeType::Subject)
            .overlay(OverlayRule::Subject, FillRule::Positive);
        let screened = FloatOverlay::with_adapter(frame, capacity)
            .with_exact_y_screen()
            .unsafe_add_source(&paths, ShapeType::Subject)
            .overlay(OverlayRule::Subject, FillRule::Positive);
        assert_eq!(screened, unscreened);
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
