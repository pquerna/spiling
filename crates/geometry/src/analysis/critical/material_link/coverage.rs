// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;

pub(in crate::analysis::critical) const MAX_CORRECTION_BYTES: usize = MAX_EDGE_BYTES;

/// Error-only, source-local witness. Native allocation IDs remain private
/// proof keys and must never become an IPC diagnostic identifier.
fn vertex_diagnostic(
    definition: &Definition,
    incidence: &SourceEdgeIncidence,
    vertex: monstertruck_topology::VertexId<monstertruck_modeling::Point3>,
) -> Option<([usize; 3], [f64; 3])> {
    let id = incidence.vertex_edges(vertex).next()?;
    let owner = incidence.get(id)?.first;
    let face = definition.solid.face_iter().nth(owner.face)?;
    let edge = face
        .absolute_boundaries()
        .get(owner.wire)?
        .get(owner.index)?;
    if edge.id() != id {
        return None;
    }
    let point = if edge.absolute_front().id() == vertex {
        edge.absolute_front().point()
    } else if edge.absolute_back().id() == vertex {
        edge.absolute_back().point()
    } else {
        return None;
    };
    Some((
        [owner.face, owner.wire, owner.index],
        [point.x, point.y, point.z],
    ))
}

#[derive(Clone, Copy)]
struct NativeEndpointExit {
    edge: monstertruck_topology::EdgeId<Curve>,
    absolute_back: bool,
    parameter: f64,
}

/// Query-local proof storage, never an alternative source representation.
/// Every nonzero source transport support is retained until a COMPLETE regular
/// fiber census has calibrated the closed boundary's total covering degree.
pub(in crate::analysis::critical) struct ClosedRadialLinks {
    supports: Vec<[Interval; 3]>,
    pub source_error_mm: f64,
    exits: Vec<NativeEndpointExit>,
    unchanged_source: bool,
    all_vertex_supports_outside: bool,
}
impl ClosedRadialLinks {
    pub fn new(
        source: OriginalTrimSource<'_, '_>,
        incidence: &SourceEdgeIncidence,
        prism: &SamplePrism,
        request: &SlabRequest,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        Self::prepare(
            source,
            incidence,
            Some((prism, request)),
            remaining_bytes,
            work,
            cancel,
            &mut |_, _, _| Ok(()),
        )
    }

    /// The SAME complete source vertex inventory and literal native exits as
    /// radial coverage, qualified independently of a physical witness field.
    pub(super) fn geometry(
        source: OriginalTrimSource<'_, '_>,
        incidence: &SourceEdgeIncidence,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
        visitor: &mut impl FnMut(usize, [Interval; 3], [Interval; 3]) -> Result<(), GeometryError>,
    ) -> Result<Self, GeometryError> {
        Self::prepare(
            source,
            incidence,
            None,
            remaining_bytes,
            work,
            cancel,
            visitor,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn prepare(
        source: OriginalTrimSource<'_, '_>,
        incidence: &SourceEdgeIncidence,
        field: Option<(&SamplePrism, &SlabRequest)>,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
        visitor: &mut impl FnMut(usize, [Interval; 3], [Interval; 3]) -> Result<(), GeometryError>,
    ) -> Result<Self, GeometryError> {
        let definition = source.definition();
        let allowance = source_allowance(definition)?;
        let vertices = incidence.vertices().ok_or_else(|| {
            failure(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "material link vertex inventory was not prepared",
                0,
                (),
            )
        })?;
        let unchanged_source = definition.material_transport_is_proved_zero(work, cancel)?;
        let mut result = Self {
            supports: Vec::new(),
            source_error_mm: 0.0,
            exits: Vec::new(),
            unchanged_source,
            all_vertex_supports_outside: true,
        };
        for vertex in vertices {
            if unchanged_source {
                let Some((prism, request)) = field else {
                    if !incidence.vertex_link_is_cycle(definition, vertex, work, cancel)? {
                        return Err(failure(
                            GeometryErrorCode::AnalysisIndeterminate,
                            "source vertex has no complete original incident cycle",
                            0,
                            vertex_diagnostic(definition, incidence, vertex),
                        ));
                    }
                    continue;
                };
                let proof = vertex_fan::original_vertex_fan(
                    definition,
                    source,
                    incidence,
                    vertex,
                    prism,
                    request,
                    allowance,
                    remaining_bytes,
                    work,
                    cancel,
                )?;
                result.all_vertex_supports_outside &=
                    matches!(proof, vertex_fan::OriginalVertexFanProof::OutsideClosedSlab);
                continue;
            }
            let Some(plan) = vertex_transport::SourceVertexTransportPlan::prepare(
                source,
                incidence,
                vertex,
                remaining_bytes,
                work,
                cancel,
            )?
            else {
                return Err(failure(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "source vertex has no evaluatable immutable conical recipe",
                    0,
                    vertex_diagnostic(definition, incidence, vertex),
                ));
            };
            if let Some((prism, request)) = field {
                let Some(proof) =
                    plan.prove_radial(prism, request, remaining_bytes, work, cancel)?
                else {
                    return Err(failure(
                        GeometryErrorCode::AnalysisIndeterminate,
                        "actual source conical star has no complete positive closed-slab radial link",
                        0,
                        (
                            vertex_diagnostic(definition, incidence, vertex),
                            request.height_interval_mm,
                        ),
                    ));
                };
                let proved_request = proof.request();
                if proved_request.frame != request.frame
                    || proved_request.height_interval_mm != request.height_interval_mm
                    || proved_request.sample_height_mm != request.sample_height_mm
                    || proved_request.native_shape_error_mm != request.native_shape_error_mm
                {
                    return Err(failure(
                        GeometryErrorCode::InvalidGeometry,
                        "source conical radial receipt belongs to another closed-slab query",
                        0,
                        vertex_diagnostic(definition, incidence, vertex),
                    ));
                }
                result.all_vertex_supports_outside &= proof.outside_closed_slab();
                result.source_error_mm = result.source_error_mm.max(proof.source_error_mm());
            } else {
                plan.visit_qualified_geometry(source, remaining_bytes, work, cancel, visitor)?;
                result.source_error_mm = result.source_error_mm.max(plan.source_error_mm());
            }
            plan.visit_position_supports(work, cancel, &mut |_realized, homotopy| {
                result.record_support(homotopy)
            })?;
            plan.visit_native_exits(work, cancel, &mut |edge, absolute_back, parameter| {
                result.record_exit(NativeEndpointExit {
                    edge,
                    absolute_back,
                    parameter,
                })
            })?;
        }
        Ok(result)
    }

    pub fn all_vertex_supports_outside_closed_slab(&self) -> bool {
        self.all_vertex_supports_outside
    }

    /// Before central cells are recorded, these are precisely the retained
    /// whole source-only vertex completion/homotopy supports.
    pub fn vertex_supports(&self) -> impl Iterator<Item = &[Interval; 3]> {
        self.supports.iter()
    }

    pub(super) fn allocated_bytes(&self) -> usize {
        self.supports
            .capacity()
            .saturating_mul(std::mem::size_of::<[Interval; 3]>())
            .saturating_add(
                self.exits
                    .capacity()
                    .saturating_mul(std::mem::size_of::<NativeEndpointExit>()),
            )
    }

    fn record_exit(&mut self, exit: NativeEndpointExit) -> Result<(), GeometryError> {
        if !exit.parameter.is_finite() {
            return Err(failure(
                GeometryErrorCode::InvalidGeometry,
                "source endpoint recipe has a nonfinite native exit",
                0,
                exit.parameter,
            ));
        }
        if let Some(previous) = self.exits.iter().find(|previous| {
            previous.edge == exit.edge && previous.absolute_back == exit.absolute_back
        }) {
            if previous.parameter != exit.parameter {
                return Err(failure(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "opposed original uses disagree on immutable native endpoint exit",
                    0,
                    (previous.parameter, exit.parameter),
                ));
            }
            return Ok(());
        }
        if self.exits.len() == self.exits.capacity() {
            let next = self.exits.capacity().saturating_mul(2).max(32);
            let bytes = self
                .supports
                .capacity()
                .saturating_mul(std::mem::size_of::<[Interval; 3]>())
                .saturating_add(next.saturating_mul(std::mem::size_of::<NativeEndpointExit>()));
            if bytes > MAX_CORRECTION_BYTES {
                return Err(failure(
                    GeometryErrorCode::ResourceLimit,
                    "source endpoint exits exhaust correction proof storage",
                    0,
                    bytes,
                ));
            }
            self.exits
                .try_reserve_exact(next - self.exits.len())
                .map_err(|_| {
                    failure(
                        GeometryErrorCode::ResourceLimit,
                        "source endpoint exit allocation refused",
                        0,
                        next,
                    )
                })?;
            if self.allocated_bytes() > MAX_CORRECTION_BYTES {
                return Err(failure(
                    GeometryErrorCode::ResourceLimit,
                    "source endpoint exit allocator exceeds correction proof cap",
                    0,
                    self.allocated_bytes(),
                ));
            }
        }
        self.exits.push(exit);
        Ok(())
    }

    /// Both literal limits come from the same source-only corner recipes used
    /// by the active-star proof. A closed edge retains its two distinct germs.
    pub fn central_native_range(
        &self,
        edge: monstertruck_topology::EdgeId<Curve>,
        original: [f64; 2],
        source_owner: [usize; 3],
    ) -> Result<[f64; 2], GeometryError> {
        if self.unchanged_source {
            return Ok(original);
        }
        let find = |back| {
            self.exits
                .iter()
                .find(|exit| exit.edge == edge && exit.absolute_back == back)
                .map(|exit| exit.parameter)
        };
        let (Some(front), Some(back)) = (find(false), find(true)) else {
            return Err(failure(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "source central collar lacks complete native endpoint ownership",
                source_owner[0],
                (source_owner[1], source_owner[2], original),
            ));
        };
        if front <= original[0] || back >= original[1] || front >= back {
            return Err(failure(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "source endpoint exits do not partition the original native edge",
                source_owner[0],
                (source_owner[1], source_owner[2], original, [front, back]),
            ));
        }
        Ok([front, back])
    }

    pub fn record(
        &mut self,
        cell: &MaterialLinkCell,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        work.charge(1, cancel)?;
        self.source_error_mm = self.source_error_mm.max(cell.source_error_mm);
        if cell.source_error_mm == 0.0 {
            return Ok(());
        }
        self.record_support(cell.position)
    }

    fn record_support(&mut self, position: [Interval; 3]) -> Result<(), GeometryError> {
        if self.supports.len() == self.supports.capacity() {
            let capacity = self
                .supports
                .capacity()
                .max(32)
                .checked_mul(2)
                .filter(|capacity| {
                    capacity
                        .checked_mul(std::mem::size_of::<[Interval; 3]>())
                        .and_then(|bytes| {
                            bytes.checked_add(
                                self.exits
                                    .capacity()
                                    .saturating_mul(std::mem::size_of::<NativeEndpointExit>()),
                            )
                        })
                        .is_some_and(|bytes| bytes <= MAX_CORRECTION_BYTES)
                })
                .ok_or_else(|| {
                    failure(
                        GeometryErrorCode::ResourceLimit,
                        "material correction support capacity exhausted",
                        0,
                        self.supports.len(),
                    )
                })?;
            self.supports
                .try_reserve_exact(capacity - self.supports.len())
                .map_err(|_| {
                    failure(
                        GeometryErrorCode::ResourceLimit,
                        "material correction support allocation refused",
                        0,
                        capacity,
                    )
                })?;
            if self.allocated_bytes() > MAX_CORRECTION_BYTES {
                return Err(failure(
                    GeometryErrorCode::ResourceLimit,
                    "material correction support allocator capacity exceeded",
                    0,
                    self.supports.capacity(),
                ));
            }
        }
        self.supports.push(position);
        Ok(())
    }

    pub fn degree_one(
        &self,
        source: OriginalTrimSource<'_, '_>,
        prism: &SamplePrism,
        request: &SlabRequest,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        work.charge(1, cancel)?;
        // Cap exclusion is proved against the complete corrected source AABB.
        // In disk mode the fiber has constant height. In core mode its exact
        // meridian relation is pz=beta*(rho-R); candidate beta is bounded below
        // so every fiber point inside that AABB stays between BOTH closed cuts.
        if prism.frame.z_axis != request.frame.z_axis {
            return Err(failure(
                GeometryErrorCode::AnalysisIndeterminate,
                "radial calibration needs an independently bound source cap fiber",
                0,
                (),
            ));
        }
        let center_height = prism
            .source_height
            .height(prism.center.map(Interval::point));
        if center_height.lo <= request.height_interval_mm[0]
            || center_height.hi >= request.height_interval_mm[1]
        {
            return Err(failure(
                GeometryErrorCode::AnalysisIndeterminate,
                "radial calibration fiber meets a closed cap",
                0,
                center_height,
            ));
        }
        let x = prism.frame.x_axis.map(Interval::point);
        let y = prism.frame.y_axis.map(Interval::point);
        let z = prism.frame.z_axis.map(Interval::point);
        let allowance = source_allowance(source.definition())?;
        let source_box = std::array::from_fn(|axis| Interval {
            lo: down(source.definition().bounds.min[axis] - allowance),
            hi: up(source.definition().bounds.max[axis] + allowance),
        });
        let p = project(source_box, prism.frame);
        let rho_max = length([p[0], p[1], Interval::point(0.0)])?.hi;
        let cap_margin = down(
            (center_height.lo - request.height_interval_mm[0])
                .min(request.height_interval_mm[1] - center_height.hi),
        );
        let meridian_extent = up(rho_max + prism.core_radius.unwrap_or(0.0));
        let height_rows = prism.dual.map(|column| prism.source_height.dot(column));
        let height_meridian_extent = mul(
            Interval::point(height_rows[2].abs_upper()),
            Interval::point(meridian_extent),
        )
        .hi;
        // Candidate ordering is not geometry authority. Every successful
        // candidate exhaustively excludes all correction supports and counts
        // all original roots. No sampled intersection, proximity deduplication
        // or fixture-specific direction can construct the receipt.
        for alpha in [0.0, 1.0, -1.0, 0.5, -0.5, 2.0, -2.0, 0.2718281828459045] {
            let angular =
                std::array::from_fn(|axis| y[axis].sub(mul(Interval::point(alpha), x[axis])));
            let normalization = add(Interval::point(1.0), square(Interval::point(alpha))).sqrt()?;
            // On this fiber py=alpha*px. Account actual cut-height drift in
            // the native frame rather than silently identifying its Z row.
            let transverse_height = mul(
                Interval::point(
                    add(height_rows[0], mul(Interval::point(alpha), height_rows[1])).abs_upper(),
                ),
                Interval::point(p[0].abs_upper()),
            )
            .hi;
            let remaining_margin = Interval::point(cap_margin)
                .sub(Interval::point(transverse_height))
                .lo;
            if remaining_margin <= 0.0 || height_meridian_extent <= 0.0 {
                continue;
            }
            let beta_bound = Interval::point(remaining_margin)
                .div(Interval::point(height_meridian_extent))?
                .lo
                * 0.25;
            for beta in [
                0.0,
                beta_bound,
                -beta_bound,
                beta_bound * 0.5,
                -beta_bound * 0.5,
            ] {
                work.charge(1, cancel)?;
                if prism.core_radius.is_none() && beta != 0.0 {
                    continue;
                }
                if add(
                    Interval::point(transverse_height),
                    mul(
                        Interval::point(beta.abs()),
                        Interval::point(height_meridian_extent),
                    ),
                )
                .hi >= cap_margin
                {
                    continue;
                }
                let meridian = if prism.core_radius.is_some() {
                    std::array::from_fn(|axis| {
                        z[axis].sub(mul(mul(Interval::point(beta), normalization), x[axis]))
                    })
                } else {
                    z
                };
                let rows = [angular, meridian];
                let offsets = [
                    Interval::point(0.0),
                    Interval::point(prism.core_radius.unwrap_or(0.0)).mul(Interval::point(beta)),
                ];
                let forward = cross(rows[0], rows[1]);
                if length(forward)?.lo <= 0.0 {
                    continue;
                }
                let fiber = fiber::OriginalFiber {
                    center: prism.center,
                    rows,
                    offsets,
                    forward,
                    min_forward: 0.0,
                    core: prism.core_radius.map(|radius| fiber::FiberCore {
                        frame: prism.frame,
                        radius,
                    }),
                };
                if !self.misses_all_supports(&fiber, work, cancel)? {
                    continue;
                }
                if fiber::original_fiber_degree_one(
                    source,
                    &fiber,
                    request,
                    prism.source_height,
                    remaining_bytes,
                    work,
                    cancel,
                )?
                .is_some()
                {
                    // A proper local homeomorphism of the complete closed
                    // boundary to S²/T² has constant total fiber cardinality.
                    // One exhaustive regular fiber therefore proves ONE global
                    // sheet, including disconnected-component and torus-cycle
                    // multiplicity. This does not assert material convexity.
                    return Ok(());
                }
            }
        }
        Err(failure(
            GeometryErrorCode::AnalysisIndeterminate,
            "closed material radial map has no complete degree-one regular fiber",
            0,
            request.height_interval_mm,
        ))
    }

    pub(super) fn misses_all_supports(
        &self,
        fiber: &fiber::OriginalFiber,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        for position in &self.supports {
            work.charge(1, cancel)?;
            let relative =
                std::array::from_fn(|axis| position[axis].sub(Interval::point(fiber.center[axis])));
            if (0..2)
                .any(|row| !add(dot(fiber.rows[row], relative), fiber.offsets[row]).contains_zero())
            {
                continue;
            }
            if let Some(core) = &fiber.core {
                let p = project(*position, core.frame);
                if p[0].hi <= 0.0 || length([p[0], p[1], Interval::point(0.0)])?.hi <= core.radius {
                    continue;
                }
            } else if dot(fiber.forward, relative).hi <= fiber.min_forward {
                continue;
            }
            return Ok(false);
        }
        Ok(true)
    }
}
