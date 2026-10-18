// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Source-only Cartesian expression of an original analytic pole quotient.
//! Neither the original UV boundary nor the selected finite B/G is changed.

use super::*;
use monstertruck_modeling::{SphericalPole, TrimJointKind};
use source_chart::PoleChart;

#[derive(Clone, Copy)]
pub(in crate::analysis::critical::material_link::vertex_transport) struct PoleComparison {
    vertex: VertexId<Point3>,
    ends: [EndKey; 2],
    joint_index: usize,
    pole: SphericalPole,
    polar_axis: usize,
    allowance: f64,
    transport: [GermTransport; 2],
}

struct PoleStarJet {
    xy: Box2,
}

struct PoleAngularJet {
    q: Box2,
    factored_jacobian: Interval,
}

struct RawBoundary {
    source_uv: Box2,
    position: Box3,
    derivative_uv: Box2,
    displacement: Box3,
}

/// Call-local coefficient law. Borrowing all three immutable authorities keeps
/// the normalized common polar function bound to its actual source operands.
#[derive(Clone, Copy)]
struct SharedPolarGerm<'a, 'd> {
    comparison: &'a PoleComparison,
    domain: &'a PreparedFaceTrimDomain<'d>,
    recipe: &'a CornerRecipe,
}

#[derive(Clone, Copy)]
struct ExitSupport<'a, 'd> {
    row: Box2,
    shared_polar: Option<SharedPolarGerm<'a, 'd>>,
}

type ExitEvidence<'a, 'd> = Result<ExitSupport<'a, 'd>, (Interval, Interval)>;

fn polar_sin_cos(
    polar: Interval,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(Interval, Interval), GeometryError> {
    let mut native = monstertruck_modeling::ValidatedGeometryBudget::new(
        usize::try_from(work.remaining()).unwrap_or(usize::MAX),
    );
    let result = monstertruck_modeling::validated_sin_cos(
        monstertruck_modeling::ValidatedInterval {
            lower: polar.lo,
            upper: polar.hi,
        },
        &mut native,
        cancel,
    );
    crate::parametric::debit_native_work(work, native.consumed(), cancel)?;
    let (sine, cosine) = result.map_err(crate::parametric::native_error)?;
    let adapt = |value: monstertruck_modeling::ValidatedInterval| Interval {
        lo: value.lower,
        hi: value.upper,
    };
    Ok((adapt(sine), adapt(cosine)))
}

impl SharedPolarGerm<'_, '_> {
    /// No two-dimensional inverse is needed for this scalar support. The
    /// sealed affine FUNCTION identity makes phi(s) common to every piece and
    /// theta, including each original offset and the actual middle bulge.
    fn before_exit(
        &self,
        lower_s: f64,
        row: Box2,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        work.charge(1, cancel)?;
        let s = source_unit_parameter(Interval::new(lower_s, 1.0)?)?;
        let polar = self.comparison.polar_axis;
        let angular = 1 - polar;
        if row[angular].lo != 0.0 || row[angular].hi != 0.0 {
            return Ok(false);
        }
        let germ = end_curve(self.domain, self.recipe.outgoing, s, work, cancel)?;
        let phi = germ.position[polar];
        // This is the SAME principal covering branch as the ordinary inverse.
        // A native pole endpoint outside it is not snapped or discarded: that
        // case retains the general geometric/inverse proof.
        if phi.lo <= 0.0 || phi.hi >= down(std::f64::consts::PI) {
            return Ok(false);
        }
        let (sine, cosine) = polar_sin_cos(phi, work, cancel)?;
        let hemisphere = match self.comparison.pole {
            SphericalPole::North => 1.0,
            SphericalPole::South => -1.0,
        };
        if cosine.mul(Interval::point(hemisphere)).lo <= 0.0
            || row[polar].mul(germ.derivative[polar]).lo <= 0.0
        {
            return Ok(false);
        }
        let radius_squared_s = Interval::point(2.0)
            .mul(sine)
            .mul(cosine)
            .mul(germ.derivative[polar]);
        if radius_squared_s.lo <= 0.0 {
            return Ok(false);
        }
        let exit = end_curve(
            self.domain,
            self.recipe.outgoing,
            Interval::point(1.0),
            work,
            cancel,
        )?;
        let (exit_sine, _) = polar_sin_cos(exit.position[polar], work, cancel)?;
        let radius_squared = exit_sine.square();
        if radius_squared.lo <= 0.0 || radius_squared.hi >= 1.0 {
            return Ok(false);
        }
        // SAME endpoint operands give r_raw(1)^2=R^2, not equality of two
        // rounded evaluations. Integrating the strict derivative factors
        // R^2-r_raw(s)^2 by (1-s), including its CLOSED zero at s=1.
        // The reference has r_ref(s)^2=s^2 R^2 by the literal sphere law.
        // Convexity of squared norm therefore puts the ENTIRE Cartesian
        // lambda homotopy inside R^2, strictly for s<1. Principal-branch
        // monotonicity and the checked row*phi' sign give before-exit support.
        // The caller STILL proves all outer zero-locus/cutover ownership.
        Ok(true)
    }
}

// Account for the nested native-range / cap-image DFS, canonical original
// curve storage, and simultaneously live coefficient/local-inverse jets.
// This is independent of the parent's combined ownership-scratch constant.
pub(super) const STACK_BYTES: usize =
    2 * std::mem::size_of::<[([[f64; 2]; 2], u8); MAX_DEPTH as usize + 1]>()
        + 3 * std::mem::size_of::<[([f64; 2], u8); MAX_DEPTH as usize + 1]>()
        + comparison_transport::STACK_BYTES
        + super::super::super::vertex_fan::ORIGINAL_UV_AFFINE_COORDINATE_STACK_BYTES
        + 8 * std::mem::size_of::<crate::parametric::SurfaceEnclosure>()
        + 6 * std::mem::size_of::<PoleChart>()
        + 4 * std::mem::size_of::<RawBoundary>()
        + 4 * std::mem::size_of::<PoleImage>()
        + std::mem::size_of::<[Option<ExitEvidence<'static, 'static>>; 2]>()
        + std::mem::size_of::<Option<Box2>>()
        + std::mem::size_of::<Option<(Box2, TrimClassification)>>();

fn active(recipe: &CornerRecipe, piece: Piece) -> bool {
    match piece {
        Piece::Outgoing => recipe.outgoing.collar,
        Piece::Incoming => recipe.incoming.collar,
        Piece::Middle => true,
    }
}

fn raw_boundary(
    domain: &PreparedFaceTrimDomain<'_>,
    recipe: &CornerRecipe,
    piece: Piece,
    t: Interval,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<RawBoundary, GeometryError> {
    let boundary = super::super::boundary_jet(
        domain,
        recipe.outgoing,
        recipe.incoming,
        piece,
        t,
        recipe.bulge,
        work,
        cancel,
    )?;
    let derivative_uv = match piece {
        Piece::Outgoing => recipe.outgoing.offset,
        Piece::Incoming => recipe.incoming.offset.map(Interval::neg),
        Piece::Middle => {
            let a = end_curve(domain, recipe.outgoing, Interval::point(1.0), work, cancel)?;
            let b = end_curve(domain, recipe.incoming, Interval::point(1.0), work, cancel)?;
            std::array::from_fn(|i| {
                b.position[i]
                    .add(recipe.incoming.offset[i])
                    .sub(a.position[i].add(recipe.outgoing.offset[i]))
                    .add(
                        Interval::point(1.0)
                            .sub(Interval::point(2.0).mul(t))
                            .mul(recipe.bulge[i]),
                    )
            })
        }
    };
    let margin = match piece {
        Piece::Outgoing => domain
            .original_coedge_correspondence(recipe.outgoing.use_.coedge)?
            .margin_uv(),
        Piece::Incoming => domain
            .original_coedge_correspondence(recipe.incoming.use_.coedge)?
            .margin_uv(),
        Piece::Middle => [0.0; 2],
    };
    domain.native().face().with_surface(|surface| {
        let jet = surface_enclosure(surface, uv_box(boundary.uv), work, cancel)?;
        let margin_box = margin.map(|radius| Interval {
            lo: -radius,
            hi: radius,
        });
        let margin_jet = surface_enclosure(
            surface,
            uv_box(std::array::from_fn(|i| boundary.uv[i].add(margin_box[i]))),
            work,
            cancel,
        )?;
        let margin_shift = compose_first(&margin_jet.du, &margin_jet.dv, margin_box);
        Ok(RawBoundary {
            source_uv: boundary.uv,
            position: jet.position,
            derivative_uv,
            // The finite collar boundary can be the spatial source C rather
            // than S(U). Retain that SAME actual B minus raw carrier trace.
            displacement: std::array::from_fn(|i| boundary.displacement[i].add(margin_shift[i])),
        })
    })
}

impl PoleComparison {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare(
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        vertex: VertexId<Point3>,
        joint_index: usize,
        allowance: f64,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        work.charge(1, cancel)?;
        if remaining_bytes < STACK_BYTES {
            return Err(GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "pole comparison preparation exceeds admitted scratch",
            ));
        }
        if !allowance.is_finite() || allowance <= 0.0 {
            return Ok(None);
        }
        let Some(joint) = domain.native().joints().get(joint_index) else {
            return Ok(None);
        };
        let TrimJointKind::SphericalPole {
            polar_axis,
            pole,
            vertex_residual,
            ..
        } = joint.kind
        else {
            return Ok(None);
        };
        let Ok([outgoing, incoming]) = native_endpoint_operands(domain, recipe, vertex) else {
            return Ok(None);
        };
        if joint.vertex_id != vertex
            || !same_native_use(joint.outgoing, outgoing)
            || !same_native_use(joint.incoming, incoming)
            || length(vertex_residual.map(|v| Interval {
                lo: v.lower,
                hi: v.upper,
            }))?
            .hi > allowance
        {
            return Ok(None);
        }
        let mut keys = [None; 2];
        for (i, end) in [recipe.outgoing, recipe.incoming].into_iter().enumerate() {
            let original = domain.original_coedge(end.use_.coedge)?;
            let proof = domain.original_coedge_correspondence(end.use_.coedge)?;
            let Some(binding) = proof.binding() else {
                return Ok(None);
            };
            let face = domain.native().face();
            let Some(edge) = face
                .absolute_boundaries()
                .get(end.use_.wire)
                .and_then(|wire| wire.get(end.use_.index))
            else {
                return Ok(None);
            };
            let range = proof.native_range();
            let delta = Interval::point(end.exit).sub(Interval::point(end.endpoint));
            if proof.provenance()
                == crate::kernel::OriginalCorrespondenceProvenance::CompletedPoleGraph
                || binding.owner != [end.use_.face, end.use_.wire, end.use_.index]
                || binding.face != face.id()
                || binding.edge != edge.id()
                || binding.edge != end.edge
                || binding.source.native_orientation != edge.orientation()
                || binding.endpoints != [edge.front().id(), edge.back().id()]
                || binding.endpoints
                    [usize::from(end.absolute_back == binding.source.native_orientation)]
                    != vertex
                || original.source_chart_parameter_range
                    != Some((proof.uv_range()[0], proof.uv_range()[1]))
                || end.endpoint != range[usize::from(end.absolute_back)]
                || end.absolute_back != (end.use_.orientation == (i == 1))
                || end.delta_t.lo != delta.lo
                || end.delta_t.hi != delta.hi
                || !end.exit.is_finite()
                || end.exit <= range[0]
                || end.exit >= range[1]
                || proof
                    .margin_uv()
                    .into_iter()
                    .any(|v| !v.is_finite() || v < 0.0)
                || !equal_box(
                    chart_jet(domain, end.use_.coedge, [end.endpoint; 2], work, cancel)?.position,
                    end.joint_uv,
                )
            {
                return Ok(None);
            }
            keys[i] = Some(EndKey {
                binding: *binding,
                coedge: end.use_.coedge,
                orientation: end.use_.orientation,
                absolute_back: end.absolute_back,
                endpoint: end.endpoint,
                exit: end.exit,
                joint_uv: end.joint_uv,
                offset: end.offset,
                collar: end.collar,
                native_range: range,
                uv_range: proof.uv_range(),
                margin: proof.margin_uv(),
                position_error: proof.position_error_mm(),
                provenance: proof.provenance(),
            });
        }
        let chart_ok = domain.native().face().with_surface(|surface| {
            let monstertruck_modeling::Surface::SphericalSurface(processor) = surface else {
                return Ok(false);
            };
            Ok(polar_axis == usize::from(!processor.orientation())
                && source_chart::SphereChart::new(surface, domain, work, cancel)?.is_some()
                && PoleChart::coefficients(surface, pole)?.is_some())
        })?;
        if !chart_ok {
            return Ok(None);
        }
        let mut comparison = Self {
            vertex,
            ends: keys.map(|key| key.expect("both original pole operands checked")),
            joint_index,
            pole,
            polar_axis,
            allowance,
            transport: [GermTransport {
                position_error_mm: 0.0,
                relative_derivative_error: 0.0,
            }; 2],
        };
        for i in 0..2 {
            let Some(receipt) =
                comparison.certify_germ(domain, recipe, i, remaining_bytes, work, cancel)?
            else {
                return Ok(None);
            };
            comparison.transport[i] = receipt;
        }
        Ok(Some(comparison))
    }

    pub(super) fn joint_index_matches(&self, index: usize) -> bool {
        self.joint_index == index
    }

    pub(super) fn validate(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        vertex: VertexId<Point3>,
    ) -> Result<(), GeometryError> {
        if !self.allowance.is_finite()
            || self.allowance <= 0.0
            || self.transport.iter().any(|receipt| {
                !receipt.position_error_mm.is_finite()
                    || receipt.position_error_mm < 0.0
                    || receipt.position_error_mm > self.allowance
                    || !receipt.relative_derivative_error.is_finite()
                    || receipt.relative_derivative_error < 0.0
                    || receipt.relative_derivative_error >= 0.25
            })
        {
            return Err(ownership_failure(
                recipe,
                "pole fresh germ receipt exceeds frozen allowance/strict derivative",
            ));
        }
        let Some(joint) = domain.native().joints().get(self.joint_index) else {
            return Err(ownership_failure(recipe, "pole missing original joint"));
        };
        let Ok([outgoing, incoming]) = native_endpoint_operands(domain, recipe, vertex) else {
            return Err(ownership_failure(recipe, "pole original endpoint operands"));
        };
        let kind_matches = matches!(joint.kind, TrimJointKind::SphericalPole { pole, polar_axis, .. }
            if pole == self.pole && polar_axis == self.polar_axis);
        if vertex != self.vertex
            || joint.vertex_id != vertex
            || !kind_matches
            || !same_native_use(joint.outgoing, outgoing)
            || !same_native_use(joint.incoming, incoming)
        {
            return Err(ownership_failure(
                recipe,
                "pole original ordered-use/NativeV identity",
            ));
        }
        for (i, end) in [recipe.outgoing, recipe.incoming].into_iter().enumerate() {
            if !same_end(
                self.ends[i],
                end,
                domain.original_coedge_correspondence(end.use_.coedge)?,
            ) {
                return Err(ownership_failure(
                    recipe,
                    "pole whole-forward receipt identity",
                ));
            }
        }
        Ok(())
    }

    /// Complete-set cover in the EXISTING original pole chart. Raw removed-F
    /// domains are covered by their entire literal boundary UV box, not by an
    /// assumed onto star map. Completed germs/reference stars have the stored
    /// Cartesian law q=s*q_B; all their straight comparison homotopies lie in
    /// the convex q box below. The original hemisphere graph bounds that whole
    /// box only after strict hemisphere and radicand admission.
    pub(super) fn source_position_overcover(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        vertex: VertexId<Point3>,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<SourcePositionOvercover, GeometryError> {
        let before = work.consumed();
        let mut phase = "literal pole/joint/ordered-use binding";
        let outcome = (|| {
            work.charge(1, cancel)?;
            self.validate(domain, recipe, vertex)?;
            let chart = self.chart(domain)?;
            let joint = &domain.native().joints()[self.joint_index];
            let mut boundary_uv = joint.enclosure.map(|v| Interval {
                lo: v.lower,
                hi: v.upper,
            });
            let mut raw_uv = boundary_uv;
            // ZERO2 is the validated analytic pole in this original chart,
            // never a snapped rounded-PI native parameter.
            let mut q_cover = ZERO2;
            phase = "whole literal original endpoint germs";
            for end in [recipe.outgoing, recipe.incoming] {
                work.charge(1, cancel)?;
                boundary_uv = hull2(
                    boundary_uv,
                    end_curve(domain, end, UNIT, work, cancel)?.position,
                );
            }
            phase = "all selected outer-B and raw-reference pieces";
            for piece in [Piece::Outgoing, Piece::Middle, Piece::Incoming] {
                work.charge(1, cancel)?;
                let boundary = raw_boundary(domain, recipe, piece, UNIT, work, cancel)?;
                if !chart.same_hemisphere(boundary.position) {
                    return Ok(SourcePositionOvercover::Unavailable(
                        "SphericalPole whole outer-B source hemisphere unresolved",
                    ));
                }
                boundary_uv = hull2(boundary_uv, boundary.source_uv);
                let (q, _, _) =
                    chart.covering_jet(boundary.source_uv, ZERO2, ZERO2, work, cancel)?;
                q_cover = hull2(q_cover, q);
                // This formula image is used ONLY to cover the auxiliary
                // homotopy operands. It is not removed-domain authority.
                raw_uv = hull2(
                    raw_uv,
                    super::super::star_jet(
                        domain,
                        recipe.outgoing,
                        recipe.incoming,
                        piece,
                        UNIT,
                        UNIT,
                        recipe.bulge,
                        work,
                        cancel,
                    )?
                    .uv,
                );
            }
            raw_uv = hull2(raw_uv, boundary_uv);
            phase = "whole raw boundary/reference box source hemisphere";
            let raw_position = domain.native().face().with_surface(|surface| {
                crate::parametric::surface_piecewise_enclosure(
                    surface,
                    uv_box(raw_uv),
                    work,
                    cancel,
                )
                .map(|jet| jet.position)
            })?;
            if !chart.same_hemisphere(raw_position) {
                return Ok(SourcePositionOvercover::Unavailable(
                    "SphericalPole full raw source box hemisphere unresolved",
                ));
            }
            let (raw_q, _, _) = chart.covering_jet(raw_uv, ZERO2, ZERO2, work, cancel)?;
            q_cover = hull2(q_cover, raw_q);
            // Every raw/completed germ and every straight q homotopy stays in
            // this convex box. Check the ENTIRE rectangle, not only its rays.
            let radicand = Interval::point(1.0)
                .sub(q_cover[0].square())
                .sub(q_cover[1].square());
            if radicand.lo <= 0.0 {
                return Ok(SourcePositionOvercover::Unavailable(
                    "SphericalPole complete projected box leaves strict graph hemisphere",
                ));
            }
            phase = "complete original hemisphere graph box";
            let (mut position, _, _) = chart.graph_jet(q_cover, ZERO2)?;
            if let Some(clipped) = original_domain_intersection(domain, boundary_uv) {
                phase = "complete removed original-F boundary-box intersection";
                // Bound all original-domain points in the filled UV box,
                // independently of any radial-star surjectivity or ownership.
                let same_raw_box =
                    (0..2).all(|i| clipped[i].lo == raw_uv[i].lo && clipped[i].hi == raw_uv[i].hi);
                let source = if same_raw_box {
                    raw_position
                } else {
                    domain.native().face().with_surface(|surface| {
                        crate::parametric::surface_piecewise_enclosure(
                            surface,
                            uv_box(clipped),
                            work,
                            cancel,
                        )
                        .map(|jet| jet.position)
                    })?
                };
                position = hull3(position, source);
            }
            Ok(SourcePositionOvercover::Bounded(position))
        })();
        outcome.map_err(|error| {
            work.annotate_error(error, before, format_args!(
            "SphericalPole complete source-position overcover face={} coedges={:?} phase={phase}",
            recipe.outgoing.use_.face,
            [recipe.outgoing.use_.coedge, recipe.incoming.use_.coedge]
        ))
        })
    }

    fn chart(&self, domain: &PreparedFaceTrimDomain<'_>) -> Result<PoleChart, GeometryError> {
        domain.native().face().with_surface(|surface| {
            PoleChart::coefficients(surface, self.pole)?.ok_or_else(|| {
                refusal("pole comparison lost its literal analytic source coefficients")
            })
        })
    }

    fn star_jet(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        piece: Piece,
        s: Interval,
        t: Interval,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<PoleStarJet, GeometryError> {
        self.validate(domain, recipe, self.vertex)?;
        let s = source_unit_parameter(s)?;
        let boundary = raw_boundary(
            domain,
            recipe,
            piece,
            source_unit_parameter(t)?,
            work,
            cancel,
        )?;
        let chart = self.chart(domain)?;
        if !chart.same_hemisphere(boundary.position) {
            return Err(ownership_failure(
                recipe,
                "pole outer trace source hemisphere",
            ));
        }
        let (q, _, _) = chart.covering_jet(boundary.source_uv, ZERO2, ZERO2, work, cancel)?;
        chart.graph_jet(q.map(|v| s.mul(v)), q)?;
        Ok(PoleStarJet {
            xy: q.map(|v| s.mul(v)),
        })
    }

    fn factored_jet(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        piece: Piece,
        s: Interval,
        t: Interval,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<PoleAngularJet, GeometryError> {
        self.validate(domain, recipe, self.vertex)?;
        let s = source_unit_parameter(s)?;
        let t = source_unit_parameter(t)?;
        let boundary = raw_boundary(domain, recipe, piece, t, work, cancel)?;
        let chart = self.chart(domain)?;
        if !chart.same_hemisphere(boundary.position) {
            return Err(ownership_failure(
                recipe,
                "pole angular trace source hemisphere",
            ));
        }
        let (q, qt, _) = chart.covering_jet(
            boundary.source_uv,
            boundary.derivative_uv,
            ZERO2,
            work,
            cancel,
        )?;
        chart.graph_jet(q.map(|v| s.mul(v)), q)?;
        Ok(PoleAngularJet {
            q,
            factored_jacobian: determinant(q, qt),
        })
    }

    fn certify_germ(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        index: usize,
        bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<GermTransport>, GeometryError> {
        let end = if index == 0 {
            recipe.outgoing
        } else {
            recipe.incoming
        };
        let chart = self.chart(domain)?;
        let exit = end_curve(domain, end, Interval::point(1.0), work, cancel)?;
        let position = domain
            .native()
            .face()
            .with_surface(|surface| {
                surface_enclosure(surface, uv_box(exit.position), work, cancel)
            })?
            .position;
        if !chart.same_hemisphere(position) {
            return Ok(None);
        }
        let (p, _, _) = chart.covering_jet(exit.position, ZERO2, ZERO2, work, cancel)?;
        // Stream original exact cuts BEFORE the canonical original edge lock.
        let cuts = comparison_transport::exact_interior_cuts(domain, end, bytes, work, cancel)?;
        let bytes = bytes
            .checked_sub(cuts.capacity().saturating_mul(std::mem::size_of::<f64>()))
            .ok_or_else(|| refusal("pole original cut inventory exceeds scratch"))?;
        domain.with_original_spatial_curve(
            end.use_.coedge,
            bytes,
            work,
            cancel,
            |spatial, work, cancel| {
                let mut result = GermTransport {
                    position_error_mm: 0.0,
                    relative_derivative_error: 0.0,
                };
                let mut lower = end.endpoint.min(end.exit);
                for upper in cuts
                    .iter()
                    .copied()
                    .chain(std::iter::once(end.endpoint.max(end.exit)))
                {
                    let mut stack = [([0.0; 2], 0_u8); MAX_DEPTH as usize + 1];
                    stack[0] = ([lower, upper], 0);
                    let mut pending = 1;
                    while pending != 0 {
                        pending -= 1;
                        let (range, depth) = stack[pending];
                        work.charge(1, cancel)?;
                        let source = spatial.jet((range[0], range[1]), work, cancel)?;
                        let s = Interval::new(range[0], range[1])?
                            .sub(Interval::point(end.endpoint))
                            .div(end.delta_t)?;
                        let s = intersection(s, UNIT)
                            .ok_or_else(|| refusal("pole germ affine source image"))?;
                        let (_, derivative, _) = chart.graph_jet(p.map(|v| s.mul(v)), p)?;
                        let source_derivative = source.derivative.map(|v| v.mul(end.delta_t));
                        let relative =
                            comparison_transport::relative_error(source_derivative, derivative)?;
                        let midpoint = range[0] * 0.5 + range[1] * 0.5;
                        let source_midpoint = spatial.jet((midpoint, midpoint), work, cancel)?;
                        let sm = Interval::point(midpoint)
                            .sub(Interval::point(end.endpoint))
                            .div(end.delta_t)?;
                        let sm = intersection(sm, UNIT)
                            .ok_or_else(|| refusal("pole germ midpoint affine image"))?;
                        let (reference_midpoint, _, _) =
                            chart.graph_jet(p.map(|v| sm.mul(v)), p)?;
                        let radius = s.sub(sm).abs_upper();
                        let error = length(std::array::from_fn(|i| {
                            let derivative_error =
                                source_derivative[i].sub(derivative[i]).abs_upper();
                            let remainder = up(radius * derivative_error);
                            source_midpoint.position[i]
                                .sub(reference_midpoint[i])
                                .add(Interval {
                                    lo: -remainder,
                                    hi: remainder,
                                })
                        }))?
                        .hi;
                        if error <= self.allowance && relative.is_some() {
                            result.position_error_mm = result.position_error_mm.max(error);
                            result.relative_derivative_error =
                                result.relative_derivative_error.max(relative.unwrap());
                            continue;
                        }
                        let mid = range[0] * 0.5 + range[1] * 0.5;
                        if depth >= MAX_DEPTH || mid <= range[0] || mid >= range[1] {
                            return Ok(None);
                        }
                        stack[pending] = ([mid, range[1]], depth + 1);
                        stack[pending + 1] = ([range[0], mid], depth + 1);
                        pending += 2;
                    }
                    lower = upper;
                }
                Ok(Some(result))
            },
        )
    }

    pub(super) fn source_cost(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        piece: Piece,
        t: Interval,
        point: [f64; 3],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<ComparisonSourceCost, GeometryError> {
        self.validate(domain, recipe, self.vertex)?;
        let chart = self.chart(domain)?;
        let boundary = raw_boundary(domain, recipe, piece, t, work, cancel)?;
        if !chart.same_hemisphere(boundary.position) {
            return Err(ownership_failure(
                recipe,
                "pole source cost outer hemisphere",
            ));
        }
        let (p, _, _) = chart.covering_jet(boundary.source_uv, ZERO2, ZERO2, work, cancel)?;
        let (_, _, acceleration) = chart.graph_jet(p.map(|v| UNIT.mul(v)), p)?;
        let (pole, _, _) = chart.graph_jet(ZERO2, ZERO2)?;
        let raw = super::super::star_jet(
            domain,
            recipe.outgoing,
            recipe.incoming,
            piece,
            UNIT,
            t,
            recipe.bulge,
            work,
            cancel,
        )?;
        let apex = super::super::star_jet(
            domain,
            recipe.outgoing,
            recipe.incoming,
            piece,
            Interval::point(0.0),
            t,
            recipe.bulge,
            work,
            cancel,
        )?;
        let (_, _, covering_acceleration) =
            chart.covering_jet(raw.uv, raw.ds, raw.dss, work, cancel)?;
        let (apex_xy, _, _) = chart.covering_jet(apex.uv, ZERO2, ZERO2, work, cancel)?;
        let homotopy_image = self.image(domain, recipe, piece, UNIT, t, work, cancel)?;
        // q_raw and q_ref=s*q_B have the IDENTICAL original source outer
        // endpoint. Subtract their local covering expressions BEFORE mapping
        // through the hemisphere graph. Since q_ref''=0, only q_raw'' enters
        // the interpolation remainder; summing world accelerations would
        // incorrectly charge the common normal curvature twice.
        let delta_xy = std::array::from_fn(|i| {
            let remainder = up(covering_acceleration[i].abs_upper() * 0.125);
            UNIT.mul(apex_xy[i].neg()).add(Interval {
                lo: -remainder,
                hi: remainder,
            })
        });
        let (_, physical_shift, _) = chart.graph_jet(homotopy_image.q, delta_xy)?;
        let vertex_displacement: Box3 =
            std::array::from_fn(|i| Interval::point(point[i]).sub(pole[i]));
        let displacement: Box3 = std::array::from_fn(|i| {
            let remainder = up(acceleration[i].abs_upper() * 0.125);
            let face = vertex_displacement[i]
                .hull(boundary.displacement[i])
                .add(Interval {
                    lo: -remainder,
                    hi: remainder,
                });
            let homotopy_radius = physical_shift[i].abs_upper();
            face.add(Interval {
                lo: -homotopy_radius,
                hi: homotopy_radius,
            })
        });
        let homotopy_mm = length(physical_shift)?.hi;
        let endpoint_mm = length(vertex_displacement)?
            .hi
            .max(length(boundary.displacement)?.hi);
        let curvature_mm = up(length(acceleration)?.hi * 0.125);
        let face_mm = up(endpoint_mm + curvature_mm);
        let error_mm = up(face_mm + homotopy_mm)
            .max(self.transport[0].position_error_mm)
            .max(self.transport[1].position_error_mm);
        if !error_mm.is_finite() || error_mm > self.allowance {
            return Err(ownership_failure(
                recipe,
                "pole physical comparison exceeds frozen allowance",
            ));
        }
        let source = domain
            .native()
            .face()
            .with_surface(|surface| surface_enclosure(surface, uv_box(raw.uv), work, cancel))?;
        Ok(ComparisonSourceCost {
            source: hull3(homotopy_image.position, source.position),
            displacement,
            error_mm,
            homotopy_mm,
        })
    }
}

struct PoleImage {
    q: Box2,
    qs: Box2,
    position: Box3,
    raw_uv: Box2,
}

impl PoleComparison {
    /// The entire straight Cartesian homotopy, not just its two end maps.
    /// The hemisphere graph is the SAME original sphere on this whole cap.
    fn image(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        piece: Piece,
        s: Interval,
        t: Interval,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<PoleImage, GeometryError> {
        let chart = self.chart(domain)?;
        let boundary = raw_boundary(domain, recipe, piece, t, work, cancel)?;
        let raw = super::super::star_jet(
            domain,
            recipe.outgoing,
            recipe.incoming,
            piece,
            s,
            t,
            recipe.bulge,
            work,
            cancel,
        )?;
        let raw_surface = domain
            .native()
            .face()
            .with_surface(|surface| surface_enclosure(surface, uv_box(raw.uv), work, cancel))?;
        if !chart.same_hemisphere(boundary.position) || !chart.same_hemisphere(raw_surface.position)
        {
            return Err(ownership_failure(
                recipe,
                "pole whole original source cap hemisphere",
            ));
        }
        let (p, _, _) = chart.covering_jet(boundary.source_uv, ZERO2, ZERO2, work, cancel)?;
        let (actual, actual_s, _) = chart.covering_jet(raw.uv, raw.ds, raw.dss, work, cancel)?;
        let q = hull2(p.map(|v| s.mul(v)), actual);
        let qs = hull2(p, actual_s);
        let (position, _, _) = chart.graph_jet(q, ZERO2)?;
        Ok(PoleImage {
            q,
            qs,
            position,
            raw_uv: raw.uv,
        })
    }

    /// Uniform strict inclusion in an ORIGINAL UV box using the existing
    /// source local-inverse operator. This is not a nearest-root query or a
    /// separate topology census. The real pole-axis branch is checked again
    /// on the contracted root, in addition to graph hemisphere qualification.
    fn inverse(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        chart: &PoleChart,
        target: Box2,
        proposal: Box2,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<(Box2, crate::parametric::SurfaceEnclosure)>, GeometryError> {
        use crate::analysis::critical::material_link::fiber::{
            FiberNewton, OriginalFiber, fiber_krawczyk,
        };
        let (center, rows, offsets, forward, min_forward) = chart.fiber_parameters(target);
        let fiber = OriginalFiber {
            center,
            rows,
            offsets,
            forward,
            forward_offset: Interval::point(0.0),
            min_forward,
            core: None,
        };
        let bounds = domain.uv_bounds();
        for scale in [1.0, 2.0, 4.0] {
            let uv: [[f64; 2]; 2] = std::array::from_fn(|i| {
                let padding = up(proposal[i].width() * scale);
                [
                    down(proposal[i].lo - padding).max(bounds[i][0]),
                    up(proposal[i].hi + padding).min(bounds[i][1]),
                ]
            });
            if uv.iter().any(|range| range[0] >= range[1])
                || (0..2).any(|i| proposal[i].lo < uv[i][0] || proposal[i].hi > uv[i][1])
            {
                continue;
            }
            let result = domain.native().face().with_surface(|surface| {
                let jet = surface_enclosure(surface, uv, work, cancel)?;
                let FiberNewton::Enclosure { uv, unique: true } =
                    fiber_krawczyk(surface, uv, &jet, &fiber, work, cancel)?
                else {
                    return Ok(None);
                };
                let jet = surface_enclosure(surface, uv, work, cancel)?;
                let relative =
                    std::array::from_fn(|i| jet.position[i].sub(Interval::point(center[i])));
                if !chart.same_hemisphere(jet.position)
                    || dot(forward, relative).lo <= min_forward
                    || uv[self.polar_axis][0] <= 0.0
                    || uv[self.polar_axis][1] >= down(std::f64::consts::PI)
                {
                    return Ok(None);
                }
                Ok(Some((uv.map(|v| Interval { lo: v[0], hi: v[1] }), jet)))
            })?;
            if result.is_some() {
                return Ok(result);
            }
        }
        Ok(None)
    }

    fn misses_original_box(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        other: Box2,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let chart = self.chart(domain)?;
        let other_world = domain
            .native()
            .face()
            .with_surface(|surface| surface_enclosure(surface, uv_box(other), work, cancel))?
            .position;
        // Projection is two-to-one globally. Never confuse the opposite pole
        // with a collision, or use the projection to identify source sheets.
        if chart.opposite_hemisphere(other_world) {
            return Ok(true);
        }
        let local = chart.project(other_world);
        let other_q = [local[0], local[1]];
        for piece in [Piece::Outgoing, Piece::Middle, Piece::Incoming] {
            if !active(recipe, piece) {
                continue;
            }
            let mut stack = [([[0.0; 2]; 2], 0_u8); MAX_DEPTH as usize + 1];
            stack[0] = ([[0.0, 1.0]; 2], 0);
            let mut pending = 1;
            while pending != 0 {
                pending -= 1;
                let (cell, depth) = stack[pending];
                work.charge(1, cancel)?;
                let image = self.image(
                    domain,
                    recipe,
                    piece,
                    Interval::new(cell[0][0], cell[0][1])?,
                    Interval::new(cell[1][0], cell[1][1])?,
                    work,
                    cancel,
                )?;
                if disjoint(image.position, other_world) || disjoint(image.q, other_q) {
                    continue;
                }
                if depth >= MAX_DEPTH {
                    return Ok(false);
                }
                let axis = usize::from(cell[1][1] - cell[1][0] > cell[0][1] - cell[0][0]);
                let mid = cell[axis][0] * 0.5 + cell[axis][1] * 0.5;
                if mid <= cell[axis][0] || mid >= cell[axis][1] {
                    return Ok(false);
                }
                let mut left = cell;
                let mut right = cell;
                left[axis][1] = mid;
                right[axis][0] = mid;
                stack[pending] = (right, depth + 1);
                stack[pending + 1] = (left, depth + 1);
                pending += 2;
            }
        }
        Ok(true)
    }

    fn regular_jordan(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let before = work.consumed();
        let mut phase = "original pole middle factored orientation";
        let mut active_piece_range = None;
        let outcome = (|| {
            let middle = self.factored_jet(
                domain,
                recipe,
                Piece::Middle,
                Interval::point(0.5),
                Interval::point(0.5),
                work,
                cancel,
            )?;
            let sign = if middle.factored_jacobian.lo > 0.0 {
                1.0
            } else if middle.factored_jacobian.hi < 0.0 {
                -1.0
            } else {
                return Ok(false);
            };
            let row = middle.q.map(|v| Interval::point(v.mid()));
            phase = "literal original first/last source rays";
            let first = raw_boundary(
                domain,
                recipe,
                Piece::Outgoing,
                Interval::point(0.0),
                work,
                cancel,
            )?;
            let last = raw_boundary(
                domain,
                recipe,
                Piece::Incoming,
                Interval::point(1.0),
                work,
                cancel,
            )?;
            let chart = self.chart(domain)?;
            let (first, _, _) = chart.covering_jet(first.source_uv, ZERO2, ZERO2, work, cancel)?;
            let (last, _, _) = chart.covering_jet(last.source_uv, ZERO2, ZERO2, work, cancel)?;
            if determinant(first, last).mul(Interval::point(sign)).lo <= 0.0 {
                return Ok(false);
            }
            for piece in [Piece::Outgoing, Piece::Middle, Piece::Incoming] {
                if !active(recipe, piece) {
                    continue;
                }
                let mut stack = [([0.0; 2], 0_u8); MAX_DEPTH as usize + 1];
                stack[0] = ([0.0, 1.0], 0);
                let mut pending = 1;
                while pending != 0 {
                    pending -= 1;
                    let (range, depth) = stack[pending];
                    active_piece_range = Some((
                        match piece {
                            Piece::Outgoing => "outgoing",
                            Piece::Middle => "middle",
                            Piece::Incoming => "incoming",
                        },
                        range,
                        depth,
                    ));
                    phase = "whole radial factored Jacobian/angular half-plane";
                    work.charge(1, cancel)?;
                    let h = self.factored_jet(
                        domain,
                        recipe,
                        piece,
                        UNIT,
                        Interval::new(range[0], range[1])?,
                        work,
                        cancel,
                    )?;
                    if h.factored_jacobian.mul(Interval::point(sign)).lo > 0.0
                        && dot2(row, h.q).lo > 0.0
                    {
                        continue;
                    }
                    let mid = range[0] * 0.5 + range[1] * 0.5;
                    if depth >= MAX_DEPTH || mid <= range[0] || mid >= range[1] {
                        return Ok(false);
                    }
                    stack[pending] = ([mid, range[1]], depth + 1);
                    stack[pending + 1] = ([range[0], mid], depth + 1);
                    pending += 2;
                }
            }
            // The original boundary expressions agree at all three piece joins.
            // Strict angular monotonicity inside ONE open half-plane rules out
            // winding aliases and self intersections. Distinct first/last rays
            // close this simple arc at the single analytic-pole apex. The radial
            // determinant has this same sign on the entire closed factored disk.
            Ok(true)
        })();
        outcome.map_err(|error| work.annotate_error(error, before, format_args!(
            "SphericalPole Jordan/factoredJac face={} coedges={:?} phase={phase} piece_range={active_piece_range:?}",
            recipe.outgoing.use_.face,
            [recipe.outgoing.use_.coedge, recipe.incoming.use_.coedge]
        )))
    }

    fn calibrate(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let chart = self.chart(domain)?;
        let reference = self.star_jet(
            domain,
            recipe,
            Piece::Middle,
            Interval::point(0.5),
            Interval::point(0.5),
            work,
            cancel,
        )?;
        let mut half_width = 0.25_f64;
        let mut last_proposal = None;
        let mut last_classification = None;
        for _ in 0..=MAX_DEPTH {
            work.charge(1, cancel)?;
            let lower = 0.5 - half_width;
            let upper = 0.5 + half_width;
            if lower >= 0.5 || upper <= 0.5 {
                break;
            }
            // Only the SOURCE-UV proposal shrinks. The actual reference
            // parameter/point remains (1/2,1/2), and no part of its domain is
            // excluded from the already proved closed Cartesian Jordan disk.
            let window = Interval::new(lower, upper)?;
            let proposal = super::super::star_jet(
                domain,
                recipe.outgoing,
                recipe.incoming,
                Piece::Middle,
                window,
                window,
                recipe.bulge,
                work,
                cancel,
            )?
            .uv;
            last_proposal = Some(proposal);
            if let Some((uv, _)) =
                self.inverse(domain, &chart, reference.xy, proposal, work, cancel)?
            {
                // Strict original-carrier inverse inclusion, the REAL pole
                // half-ray and canonical branch were proved by inverse().
                let membership = domain.classify(uv_box(uv), work, cancel)?;
                if membership == TrimClassification::Inside {
                    return Ok(true);
                }
                last_classification = Some((uv, membership));
                if membership == TrimClassification::Outside {
                    break;
                }
            }
            half_width *= 0.5;
        }
        Err(ownership_failure(
            recipe,
            format_args!(
                "pole calibration strict original inverse/Inside target_q={:?} last_source_uv_proposal={last_proposal:?} last_classification={last_classification:?}",
                reference.xy,
            ),
        ))
    }

    fn central_failure(
        &self,
        recipe: &CornerRecipe,
        end: End,
        stage: impl std::fmt::Display,
    ) -> GeometryError {
        ownership_failure(
            recipe,
            format_args!(
                "pole central {stage} owner={:?} coedge={} exit={} offsets={:?} bulge={:?} sealed_native_uv_ranges={:?}",
                [end.use_.face, end.use_.wire, end.use_.index],
                end.use_.coedge,
                end.exit,
                [recipe.outgoing.offset, recipe.incoming.offset],
                recipe.bulge,
                self.ends
                    .each_ref()
                    .map(|key| (key.native_range, key.uv_range)),
            ),
        )
    }

    /// Exact support of the unmodified OUTER UV boundary at an owned exit.
    /// Its own cross-section has zero support by the retained canonical
    /// tangent/rotated-offset operand identity, not by a numerical zero test.
    fn outer_before_exit(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        index: usize,
        row: Box2,
        at: Box2,
        exact_exit: bool,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let other = if index == 0 {
            recipe.incoming
        } else {
            recipe.outgoing
        };
        let own = if index == 0 {
            recipe.outgoing
        } else {
            recipe.incoming
        };
        let own_curve = end_curve(domain, own, UNIT, work, cancel)?;
        if dot2(row, own_curve.derivative).lo <= 0.0 {
            return Ok(false);
        }
        if exact_exit {
            // Equality fixes the normal-support ZERO SET, not its ownership.
            // The whole boundary theorem still excludes every nonowning zero.
            return self.outer_boundary_owned(domain, recipe, index, work, cancel);
        }
        let other_at = end_curve(domain, other, Interval::point(1.0), work, cancel)?.position;
        let other_cross = hull2(
            other_at,
            std::array::from_fn(|i| other_at[i].add(other.offset[i])),
        );
        let support = dot2(row, std::array::from_fn(|i| other_cross[i].sub(at[i])));
        let bulge = dot2(row, recipe.bulge);
        let total = support.add(Interval::point(bulge.hi.max(0.0)));
        if support.hi < 0.0 && total.hi < 0.0 {
            return Ok(true);
        }
        if support.hi > 0.0 || total.hi > 0.0 {
            return Ok(false);
        }
        self.outer_boundary_owned(domain, recipe, index, work, cancel)
    }

    /// Same original affine coordinate operator at the SAME sealed native
    /// parameter, not equality of independently rounded exit enclosures.
    fn exact_shared_exit_level(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        row: Box2,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.validate(domain, recipe, self.vertex)?;
        let polar = self.polar_axis;
        let angular = 1 - polar;
        let zero = |value: Interval| value.lo == 0.0 && value.hi == 0.0;
        if !zero(row[angular])
            || !zero(recipe.outgoing.offset[polar])
            || !zero(recipe.incoming.offset[polar])
            || !zero(recipe.bulge[polar])
            || recipe.outgoing.exit != recipe.incoming.exit
        {
            return Ok(false);
        }
        let first = domain.original_coedge_correspondence(recipe.outgoing.use_.coedge)?;
        let second = domain.original_coedge_correspondence(recipe.incoming.use_.coedge)?;
        if !first.same_affine_native_uv_map(second) {
            return Ok(false);
        }
        // This is the complete original affine map, including orientation and
        // normalization. A native endpoint/exit is never snapped to this range.
        first.uv_parameter_image([recipe.outgoing.exit; 2])?;
        second.uv_parameter_image([recipe.incoming.exit; 2])?;
        let coedges = domain.native().coedges();
        super::super::super::vertex_fan::original_uv_affine_coordinate_functions_equal(
            coedges[recipe.outgoing.use_.coedge].curve,
            coedges[recipe.incoming.use_.coedge].curve,
            first.uv_range(),
            polar,
            work,
            cancel,
        )
    }

    /// The existing exit law fixes the outer level. Equal LITERAL native
    /// endpoint/exit operands additionally fix the same affine normalized
    /// parameter for both germs; same_end already checks its stored delta
    /// against that exact subtraction expression, not interval overlap.
    fn exit_polar_law<'a, 'd>(
        &'a self,
        domain: &'a PreparedFaceTrimDomain<'d>,
        recipe: &'a CornerRecipe,
        row: Box2,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(bool, Option<SharedPolarGerm<'a, 'd>>), GeometryError> {
        let exact_exit = self.exact_shared_exit_level(domain, recipe, row, work, cancel)?;
        let a = recipe.outgoing;
        let b = recipe.incoming;
        let shared = (exact_exit
            && a.endpoint == b.endpoint
            && a.exit == b.exit
            && a.absolute_back == b.absolute_back)
            .then_some(SharedPolarGerm {
                comparison: self,
                domain,
                recipe,
            });
        Ok((exact_exit, shared))
    }

    /// When an entire outer arc has zero normal support, zero does NOT mean
    /// owned. Prove the complete boundary zero locus separately in the
    /// ORIGINAL UV expression. The scalar factors theta and (1-w), together
    /// with exact piece joins, leave only the authorized cross endpoint.
    fn outer_boundary_owned(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        index: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let a = end_curve(domain, recipe.outgoing, Interval::point(1.0), work, cancel)?.position;
        let b = end_curve(domain, recipe.incoming, Interval::point(1.0), work, cancel)?.position;
        let u0: Box2 = std::array::from_fn(|i| a[i].add(recipe.outgoing.offset[i]));
        let u1: Box2 = std::array::from_fn(|i| b[i].add(recipe.incoming.offset[i]));
        let delta: Box2 = std::array::from_fn(|i| u1[i].sub(u0[i]));
        let direction = if index == 0 { 1.0 } else { -1.0 };
        let endpoint_slope: Box2 = std::array::from_fn(|i| {
            delta[i]
                .add(recipe.bulge[i].mul(Interval::point(direction)))
                .mul(Interval::point(direction))
        });
        let row = endpoint_slope.map(|v| Interval::point(v.mid()));
        let slopes: Box2 = std::array::from_fn(|i| {
            delta[i]
                .add(Interval { lo: -1.0, hi: 1.0 }.mul(recipe.bulge[i]))
                .mul(Interval::point(direction))
        });
        if dot2(row, slopes).lo <= 0.0 {
            return Ok(false);
        }
        let own = if index == 0 {
            recipe.outgoing
        } else {
            recipe.incoming
        };
        if own.collar && dot2(row, own.offset).lo <= 0.0 {
            return Ok(false);
        }
        let own_join = if index == 0 { u0 } else { u1 };
        let (other, other_at) = if index == 0 {
            (recipe.incoming, b)
        } else {
            (recipe.outgoing, a)
        };
        let other_cross = hull2(
            other_at,
            std::array::from_fn(|i| other_at[i].add(other.offset[i])),
        );
        Ok(dot2(
            row,
            std::array::from_fn(|i| other_cross[i].sub(own_join[i])),
        )
        .lo > 0.0)
    }

    /// Pull the complete Cartesian homotopy back through the existing local
    /// inverse near a shared exit. Integrating its positive support derivative
    /// from the EXACT outer cross-section proves the whole closed cell lies
    /// before that cross-section. This avoids the invalid assumption that a
    /// nonlinear sphere projection preserves UV tangent/offset orthogonality.
    fn cell_before_exit(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        piece: Piece,
        cell: [[f64; 2]; 2],
        row: Box2,
        shared_polar: Option<SharedPolarGerm<'_, '_>>,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Result<(), (&'static str, Option<Interval>)>, GeometryError> {
        if let Some(law) = shared_polar {
            if law.before_exit(cell[0][0], row, work, cancel)? {
                return Ok(Ok(()));
            }
        }
        let chart = self.chart(domain)?;
        let image = self.image(
            domain,
            recipe,
            piece,
            Interval::new(cell[0][0], 1.0)?,
            Interval::new(cell[1][0], cell[1][1])?,
            work,
            cancel,
        )?;
        let Some((_, jet)) = self.inverse(domain, &chart, image.q, image.raw_uv, work, cancel)?
        else {
            return Ok(Err(("reference_uv_inverse", None)));
        };
        let du = chart.project_vector(jet.du);
        let dv = chart.project_vector(jet.dv);
        let det = determinant([du[0], du[1]], [dv[0], dv[1]]);
        if det.contains_zero() {
            return Ok(Err(("reference_inverse_jacobian", Some(det))));
        }
        let derivative = [
            image.qs[0]
                .mul(dv[1])
                .sub(image.qs[1].mul(dv[0]))
                .div(det)?,
            du[0]
                .mul(image.qs[1])
                .sub(du[1].mul(image.qs[0]))
                .div(det)?,
        ];
        let support = dot2(row, derivative);
        Ok(if support.lo > 0.0 {
            Ok(())
        } else {
            Err(("reference_inverse_directional_support", Some(support)))
        })
    }

    fn central_misses(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        end: End,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let Some(collar) = OriginalCollar::for_enclosure(domain, end.use_, work, cancel)? else {
            return Err(self.central_failure(recipe, end, "constructor"));
        };
        let Some(embedding) = collar.embedding([end.exit; 2], [0.0, 1.0], work, cancel)? else {
            return Err(self.central_failure(recipe, end, "scoped_exit_embedding"));
        };
        if !embedding.covers(&collar, [end.exit; 2], [0.0, 1.0]) {
            return Err(self.central_failure(recipe, end, "scoped_exit_receipt"));
        }
        let mut central = collar.range;
        for owned in [recipe.outgoing, recipe.incoming] {
            if owned.edge == end.edge {
                central[usize::from(owned.absolute_back)] = owned.exit;
            }
        }
        if central[0] > central[1] {
            return Err(self.central_failure(recipe, end, "native_remaining_range"));
        }
        let chart = self.chart(domain)?;
        let mut native_stack = [([0.0; 2], 0_u8); MAX_DEPTH as usize + 1];
        native_stack[0] = (central, 0);
        let mut native_pending = 1;
        // The source-only outer trace and exact coefficient law do not change
        // during the native-cell census. Retain each owned exit proof once.
        let mut exit_supports: [Option<ExitEvidence<'_, '_>>; 2] = [None; 2];
        while native_pending != 0 {
            native_pending -= 1;
            let (range, depth) = native_stack[native_pending];
            work.charge(1, cancel)?;
            let beta = collar.bubble(range)?.0;
            let other = collar.normal_image(range, UNIT.mul(beta), work, cancel)?;
            if self.misses_original_box(domain, recipe, other, work, cancel)? {
                continue;
            }
            let mut supported = false;
            let mut last_support = None;
            for (index, owned) in [recipe.outgoing, recipe.incoming].into_iter().enumerate() {
                if owned.edge != end.edge {
                    continue;
                }
                let direction = if owned.absolute_back { -1.0 } else { 1.0 };
                let evidence = match exit_supports[index] {
                    Some(evidence) => evidence,
                    None => {
                        let at =
                            chart_jet(domain, owned.use_.coedge, [owned.exit; 2], work, cancel)?;
                        let row = at.derivative.map(|v| v.mul(Interval::point(direction)));
                        let (exact_exit, shared_polar) =
                            self.exit_polar_law(domain, recipe, row, work, cancel)?;
                        let evidence = if self.outer_before_exit(
                            domain,
                            recipe,
                            index,
                            row,
                            at.position,
                            exact_exit,
                            work,
                            cancel,
                        )? {
                            Ok(ExitSupport { row, shared_polar })
                        } else {
                            let other_end = if index == 0 {
                                recipe.incoming
                            } else {
                                recipe.outgoing
                            };
                            let other_at =
                                end_curve(domain, other_end, Interval::point(1.0), work, cancel)?
                                    .position;
                            let other_cross = hull2(
                                other_at,
                                std::array::from_fn(|i| other_at[i].add(other_end.offset[i])),
                            );
                            Err((
                                dot2(
                                    row,
                                    std::array::from_fn(|i| other_cross[i].sub(at.position[i])),
                                ),
                                dot2(row, recipe.bulge),
                            ))
                        };
                        exit_supports[index] = Some(evidence);
                        evidence
                    }
                };
                let support = match evidence {
                    Ok(support) => support,
                    Err((support, bulge)) => {
                        last_support =
                            Some(("outer_exit_support", index, Some(support), bulge, None));
                        continue;
                    }
                };
                let row = support.row;
                if if owned.absolute_back {
                    range[1] > owned.exit
                } else {
                    range[0] < owned.exit
                } {
                    last_support = Some((
                        "central_native_side",
                        index,
                        None,
                        dot2(row, recipe.bulge),
                        None,
                    ));
                    continue;
                }
                // The SAME original exit and canonical quarter-turn offset
                // give Phi(exit,w)=0. The signed native difference is >=0
                // including its closed zero; the strict prefix derivative
                // below factors Phi without subtracting rounded exit boxes.
                let prefix = [range[0].min(owned.exit), range[1].max(owned.exit)];
                let original = chart_jet(domain, owned.use_.coedge, prefix, work, cancel)?;
                let (beta, beta_t) = collar.bubble(prefix)?;
                let derivative = match collar.transport {
                    CarrierTransport::Exact => original.derivative,
                    CarrierTransport::Collar { width, inward } => {
                        let n = rotate(original.derivative, inward);
                        let nt = rotate(original.second_derivative, inward);
                        std::array::from_fn(|i| {
                            original.derivative[i].add(
                                Interval { lo: 0.0, hi: width }
                                    .mul(beta_t.mul(n[i]).add(beta.mul(nt[i]))),
                            )
                        })
                    }
                };
                if dot2(row, derivative.map(|v| v.mul(Interval::point(direction)))).lo <= 0.0 {
                    last_support = Some((
                        "central_directional_support",
                        index,
                        Some(dot2(
                            row,
                            derivative.map(|v| v.mul(Interval::point(direction))),
                        )),
                        dot2(row, recipe.bulge),
                        None,
                    ));
                    continue;
                }
                let other_world = domain
                    .native()
                    .face()
                    .with_surface(|surface| {
                        surface_enclosure(surface, uv_box(other), work, cancel)
                    })?
                    .position;
                let other_local = chart.project(other_world);
                let other_q = [other_local[0], other_local[1]];
                let mut all_pieces = true;
                for piece in [Piece::Outgoing, Piece::Middle, Piece::Incoming] {
                    if !active(recipe, piece) {
                        continue;
                    }
                    let mut stack = [([[0.0; 2]; 2], 0_u8); MAX_DEPTH as usize + 1];
                    stack[0] = ([[0.0, 1.0]; 2], 0);
                    let mut pending = 1;
                    while pending != 0 {
                        pending -= 1;
                        let (cell, cell_depth) = stack[pending];
                        work.charge(1, cancel)?;
                        let image = self.image(
                            domain,
                            recipe,
                            piece,
                            Interval::new(cell[0][0], cell[0][1])?,
                            Interval::new(cell[1][0], cell[1][1])?,
                            work,
                            cancel,
                        )?;
                        if disjoint(image.position, other_world) || disjoint(image.q, other_q) {
                            continue;
                        }
                        let inverse_refusal = match self.cell_before_exit(
                            domain,
                            recipe,
                            piece,
                            cell,
                            row,
                            support.shared_polar,
                            work,
                            cancel,
                        )? {
                            Ok(()) => continue,
                            Err(reason) => reason,
                        };
                        let axis = usize::from(cell[1][1] - cell[1][0] > cell[0][1] - cell[0][0]);
                        let mid = cell[axis][0] * 0.5 + cell[axis][1] * 0.5;
                        if cell_depth >= MAX_DEPTH || mid <= cell[axis][0] || mid >= cell[axis][1] {
                            all_pieces = false;
                            last_support = Some((
                                inverse_refusal.0,
                                index,
                                inverse_refusal.1,
                                dot2(row, recipe.bulge),
                                Some(cell),
                            ));
                            break;
                        }
                        let mut left = cell;
                        let mut right = cell;
                        left[axis][1] = mid;
                        right[axis][0] = mid;
                        stack[pending] = (right, cell_depth + 1);
                        stack[pending + 1] = (left, cell_depth + 1);
                        pending += 2;
                    }
                    if !all_pieces {
                        break;
                    }
                }
                if all_pieces {
                    supported = true;
                    break;
                }
            }
            if supported {
                continue;
            }
            let mid = range[0] * 0.5 + range[1] * 0.5;
            if depth >= MAX_DEPTH || mid <= range[0] || mid >= range[1] {
                return Err(self.central_failure(
                    recipe,
                    end,
                    format_args!(
                        "remaining_cell range={range:?} depth={depth} last_support={last_support:?}"
                    ),
                ));
            }
            native_stack[native_pending] = ([mid, range[1]], depth + 1);
            native_stack[native_pending + 1] = ([range[0], mid], depth + 1);
            native_pending += 2;
        }
        Ok(true)
    }

    pub(super) fn disk_owned(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        remaining_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        let before = work.consumed();
        let mut phase = "original pole operand validation";
        let mut active_boundary = None;
        let mut active_joint = None;
        let outcome = (|| {
            self.validate(domain, recipe, self.vertex)?;
            work.charge(1, cancel)?;
            if remaining_bytes < STACK_BYTES {
                return Err(GeometryError::new(
                    GeometryErrorCode::ResourceLimit,
                    "pole comparison closed ownership exceeds admitted scratch",
                ));
            }
            phase = "complete Cartesian Jordan/factored Jacobian";
            if !self.regular_jordan(domain, recipe, work, cancel)? {
                return Err(ownership_failure(
                    recipe,
                    "pole closed Cartesian Jordan/factoredJac",
                ));
            }
            phase = "nonowning original collar inventory";
            let Some(collar) =
                OriginalCollar::for_enclosure(domain, recipe.outgoing.use_, work, cancel)?
            else {
                return Err(ownership_failure(
                    recipe,
                    "pole original nonowning inventory",
                ));
            };
            for (index, _) in domain.native().coedges().iter().enumerate() {
                if index == recipe.outgoing.use_.coedge || index == recipe.incoming.use_.coedge {
                    continue;
                }
                for span in domain.boundary_parameter_intervals(index)? {
                    let mut stack = [([0.0; 2], 0_u8); MAX_DEPTH as usize + 1];
                    stack[0] = (span, 0);
                    let mut pending = 1;
                    while pending != 0 {
                        pending -= 1;
                        let (range, depth) = stack[pending];
                        active_boundary = Some((index, range, depth));
                        phase = "foreign original trim/collar image";
                        work.charge(1, cancel)?;
                        let other = collar.other_collar_enclosure(index, range, work, cancel)?;
                        phase = "complete pole reference vs foreign trim/collar exclusion";
                        if self.misses_original_box(domain, recipe, other, work, cancel)? {
                            continue;
                        }
                        let mid = range[0] * 0.5 + range[1] * 0.5;
                        if depth >= MAX_DEPTH || mid <= range[0] || mid >= range[1] {
                            return Err(ownership_failure(
                                recipe,
                                format_args!(
                                    "pole foreign original trim+collar coedge={index} range={range:?}"
                                ),
                            ));
                        }
                        stack[pending] = ([mid, range[1]], depth + 1);
                        stack[pending + 1] = ([range[0], mid], depth + 1);
                        pending += 2;
                    }
                }
            }
            active_boundary = None;
            for (index, joint) in domain.native().joints().iter().enumerate() {
                active_joint = Some(index);
                phase = "foreign original joint exclusion";
                work.charge(1, cancel)?;
                if index == self.joint_index {
                    continue;
                }
                let other = joint.enclosure.map(|v| Interval {
                    lo: v.lower,
                    hi: v.upper,
                });
                if !self.misses_original_box(domain, recipe, other, work, cancel)? {
                    return Err(ownership_failure(
                        recipe,
                        format_args!("pole foreign original joint index={index}"),
                    ));
                }
            }
            active_joint = None;
            phase = "original outgoing/incoming central exclusion";
            if !self.central_misses(domain, recipe, recipe.outgoing, work, cancel)?
                || recipe.incoming.edge != recipe.outgoing.edge
                    && !self.central_misses(domain, recipe, recipe.incoming, work, cancel)?
            {
                return Err(ownership_failure(
                    recipe,
                    "pole original shared-exit central atlas/support",
                ));
            }
            phase = "strict original local-inverse Inside calibration";
            if !self.calibrate(domain, recipe, work, cancel)? {
                return Err(ownership_failure(
                    recipe,
                    "pole original local-inverse Inside calibration",
                ));
            }
            Ok(true)
        })();
        outcome.map_err(|error| work.annotate_error(error, before, format_args!(
            "SphericalPole disk ownership face={} coedges={:?} phase={phase} foreign_boundary={active_boundary:?} foreign_joint={active_joint:?}",
            recipe.outgoing.use_.face,
            [recipe.outgoing.use_.coedge, recipe.incoming.use_.coedge]
        )))
    }
}

#[cfg(test)]
mod properties {
    use super::*;

    #[test]
    fn all_original_housing_poles_keep_complete_closed_cutover_ownership() {
        let cancel = AtomicBool::new(false);
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step"),
        )
        .unwrap();
        let definition = crate::import_step(&bytes, &cancel).unwrap();
        let geometry_before =
            serde_json::to_vec(&definition.solid.boundaries()[0].compress()).unwrap();
        let mut work = GeometryWorkBudget::new(20_000_000);
        let standalone = OriginalTrimSource::standalone(&definition);
        let originals = standalone
            .prepare_domains_if_missing(MAX_SCRATCH_BYTES, &mut work, &cancel)
            .unwrap()
            .expect("one complete original-domain owner");
        let source = standalone.borrowing_domains(Some(&originals));
        let available = MAX_SCRATCH_BYTES
            .checked_sub(originals.retained_bytes())
            .unwrap();
        let mut poles = Vec::new();
        let mut vertices = Vec::new();
        for (face, _) in definition.solid.face_iter().enumerate() {
            let domain = source
                .face_domain(face, available, &mut work, &cancel)
                .unwrap();
            for (joint, original) in domain.native().joints().iter().enumerate() {
                if matches!(original.kind, TrimJointKind::SphericalPole { .. }) {
                    poles.push((face, joint, original.vertex_id));
                    if !vertices.contains(&original.vertex_id) {
                        vertices.push(original.vertex_id);
                    }
                }
            }
        }
        assert!(
            poles.len() > 1,
            "all original poles, not only the lowest, must be exercised"
        );
        let mut incidence =
            SourceEdgeIncidence::new(&definition, available, &mut work, &cancel).unwrap();
        incidence
            .prepare_vertices(available, &mut work, &cancel)
            .unwrap();
        let available = available.checked_sub(incidence.owned_bytes()).unwrap();
        let mut seen = vec![false; poles.len()];
        let mut polar_laws = 0;
        for vertex in vertices {
            // One canonical source-only plan per ACTUAL NativeV. Neither
            // pole selection nor cutovers are chosen from failure coordinates.
            let plan = SourceVertexTransportPlan::prepare(
                source, &incidence, vertex, available, &mut work, &cancel,
            )
            .unwrap()
            .expect("every original housing pole has its fixed G plan");
            let allowance = source_allowance(&definition).unwrap();
            assert!(plan.source_error_mm() <= allowance);
            for (recipe_index, recipe) in plan.recipes.iter().enumerate() {
                let Some(OriginalCornerComparison::SphericalPole(comparison)) = recipe.comparison
                else {
                    continue;
                };
                let face = recipe.outgoing.use_.face;
                let index = poles
                    .iter()
                    .position(|&(f, j, v)| {
                        f == face && j == comparison.joint_index && v == comparison.vertex
                    })
                    .expect("comparison belongs to an actual original pole/joint/NativeV");
                assert!(!seen[index], "one comparison per original pole joint");
                let domain = source
                    .face_domain(face, available, &mut work, &cancel)
                    .unwrap();
                let scratch = available
                    .checked_sub(plan.retained_bytes() + CONICAL_STACK_BYTES + domain.owned_bytes())
                    .unwrap();
                comparison.validate(&domain, recipe, vertex).unwrap();
                let original = OriginalCornerComparison::SphericalPole(comparison);
                assert!(
                    original
                        .validates_original_joint(&domain, recipe, vertex, comparison.joint_index,)
                        .unwrap()
                );
                assert!(
                    !original
                        .validates_original_joint(
                            &domain,
                            recipe,
                            vertex,
                            domain.native().joints().len(),
                        )
                        .unwrap()
                );
                let exact_exit = |edge, back, parameter| {
                    [recipe.incoming, recipe.outgoing].into_iter().any(|end| {
                        end.edge == edge && end.absolute_back == back && end.exit == parameter
                    })
                };
                assert!(
                    plan.qualified_original_joint_image(
                        source,
                        &domain,
                        comparison.joint_index,
                        scratch,
                        &mut work,
                        &cancel,
                        &exact_exit,
                    )
                    .unwrap(),
                    "whole original pole image/source cut face={face}"
                );
                assert!(
                    !plan
                        .qualified_original_joint_image(
                            source,
                            &domain,
                            comparison.joint_index,
                            scratch,
                            &mut work,
                            &cancel,
                            &|edge, back, parameter| {
                                exact_exit(edge, back, parameter)
                                    && !(edge == recipe.incoming.edge
                                        && back == recipe.incoming.absolute_back
                                        && parameter == recipe.incoming.exit)
                            },
                        )
                        .unwrap(),
                    "both absolute-germ exits remain mandatory"
                );
                assert!(
                    comparison
                        .disk_owned(&domain, recipe, scratch, &mut work, &cancel,)
                        .unwrap(),
                    "complete closed pole ownership face={face} joint={}",
                    comparison.joint_index
                );
                let mut cells = 0;
                for cell in plan.cells.iter().filter(|cell| cell.recipe == recipe_index) {
                    let cost = comparison
                        .source_cost(
                            &domain,
                            recipe,
                            cell.piece,
                            Interval::new(cell.parameter[0], cell.parameter[1]).unwrap(),
                            plan.point,
                            &mut work,
                            &cancel,
                        )
                        .unwrap();
                    assert!(cost.error_mm.is_finite() && cost.error_mm <= allowance);
                    assert!(cost.homotopy_mm.is_finite() && cost.homotopy_mm <= allowance);
                    assert!(length(cost.displacement).unwrap().hi <= allowance);
                    cells += 1;
                }
                assert!(cells > 0);
                let direction = if recipe.outgoing.absolute_back {
                    -1.0
                } else {
                    1.0
                };
                let at = chart_jet(
                    &domain,
                    recipe.outgoing.use_.coedge,
                    [recipe.outgoing.exit; 2],
                    &mut work,
                    &cancel,
                )
                .unwrap();
                let row = at.derivative.map(|v| v.mul(Interval::point(direction)));
                if let (_, Some(law)) = comparison
                    .exit_polar_law(&domain, recipe, row, &mut work, &cancel)
                    .unwrap()
                {
                    assert!(law.before_exit(0.5, row, &mut work, &cancel).unwrap());
                    assert!(
                        !law.before_exit(0.5, row.map(Interval::neg), &mut work, &cancel,)
                            .unwrap(),
                        "a radius derivative alone cannot authorize the opposite cutover sign"
                    );
                    polar_laws += 1;
                }
                let mut wrong_gauge = *recipe;
                wrong_gauge.outgoing.endpoint = up(wrong_gauge.outgoing.endpoint);
                match comparison.exit_polar_law(&domain, &wrong_gauge, row, &mut work, &cancel) {
                    Err(error) => assert_eq!(error.code, GeometryErrorCode::AnalysisIndeterminate),
                    Ok(_) => panic!("changed literal original normalization must refuse"),
                }
                assert!(
                    original
                        .validates_original_joint(
                            &domain,
                            &wrong_gauge,
                            vertex,
                            comparison.joint_index,
                        )
                        .is_err(),
                    "whole-joint dispatch must bind the literal original germ"
                );
                let mut wrong_axis = comparison;
                wrong_axis.polar_axis = 1 - wrong_axis.polar_axis;
                assert!(
                    OriginalCornerComparison::SphericalPole(wrong_axis)
                        .validates_original_joint(&domain, recipe, vertex, comparison.joint_index)
                        .is_err(),
                    "whole-joint dispatch cannot borrow another pole axis"
                );
                let mut wrong_branch = comparison;
                wrong_branch.pole = match comparison.pole {
                    SphericalPole::North => SphericalPole::South,
                    SphericalPole::South => SphericalPole::North,
                };
                match wrong_branch.exit_polar_law(&domain, recipe, row, &mut work, &cancel) {
                    Err(error) => assert_eq!(error.code, GeometryErrorCode::AnalysisIndeterminate),
                    Ok(_) => panic!("foreign original pole branch must refuse"),
                }
                assert!(
                    OriginalCornerComparison::SphericalPole(wrong_branch)
                        .validates_original_joint(&domain, recipe, vertex, comparison.joint_index)
                        .is_err(),
                    "whole-joint dispatch cannot borrow the opposite original pole"
                );
                seen[index] = true;
            }
        }
        assert!(
            seen.into_iter().all(|value| value),
            "every original pole and closed cutover was consumed"
        );
        assert!(
            polar_laws > 0,
            "original affine polar functions exercise the closed support lemma"
        );
        assert_eq!(
            geometry_before,
            serde_json::to_vec(&definition.solid.boundaries()[0].compress()).unwrap(),
        );
    }

    #[test]
    fn lowest_original_housing_pole_consumes_fixed_source_plan_and_full_closed_ownership() {
        let cancel = AtomicBool::new(false);
        let bytes = std::fs::read(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step"),
        )
        .unwrap();
        let definition = crate::import_step(&bytes, &cancel).unwrap();
        let geometry_before =
            serde_json::to_vec(&definition.solid.boundaries()[0].compress()).unwrap();
        let mut work = GeometryWorkBudget::new(20_000_000);
        let standalone = OriginalTrimSource::standalone(&definition);
        let originals = standalone
            .prepare_domains_if_missing(MAX_SCRATCH_BYTES, &mut work, &cancel)
            .unwrap()
            .expect("one canonical owner for all original Housing trim domains");
        let source = standalone.borrowing_domains(Some(&originals));
        let available = MAX_SCRATCH_BYTES
            .checked_sub(originals.retained_bytes())
            .unwrap();

        // Select the first-layer consumer by ORIGINAL Native pole incidence
        // and shared vertex height, never a fixture face or coordinate match.
        let mut lowest: Option<(f64, usize, usize, VertexId<Point3>)> = None;
        for (face_index, _) in definition.solid.face_iter().enumerate() {
            let domain = source
                .face_domain(face_index, available, &mut work, &cancel)
                .unwrap();
            for (joint_index, joint) in domain.native().joints().iter().enumerate() {
                if !matches!(joint.kind, TrimJointKind::SphericalPole { .. }) {
                    continue;
                }
                assert!(joint.vertex_point.z.is_finite());
                if lowest
                    .as_ref()
                    .is_none_or(|selected| joint.vertex_point.z < selected.0)
                {
                    lowest = Some((
                        joint.vertex_point.z,
                        face_index,
                        joint_index,
                        joint.vertex_id,
                    ));
                }
            }
        }
        let (_, face_index, joint_index, vertex) =
            lowest.expect("the unchanged Housing must retain an original Native spherical pole");
        let mut incidence =
            SourceEdgeIncidence::new(&definition, available, &mut work, &cancel).unwrap();
        incidence
            .prepare_vertices(available, &mut work, &cancel)
            .unwrap();
        let available = available.checked_sub(incidence.owned_bytes()).unwrap();
        let plan = SourceVertexTransportPlan::prepare(
            source, &incidence, vertex, available, &mut work, &cancel,
        )
        .unwrap()
        .expect("the lowest actual Housing pole must retain the fixed source-only G plan");
        let allowance = source_allowance(&definition).unwrap();
        assert!(plan.source_error_mm() <= allowance);
        let (recipe_index, recipe, comparison) = plan
            .recipes
            .iter()
            .enumerate()
            .find_map(|(index, recipe)| {
                if recipe.outgoing.use_.face != face_index {
                    return None;
                }
                match recipe.comparison {
                    Some(OriginalCornerComparison::SphericalPole(comparison))
                        if comparison.vertex == vertex && comparison.joint_index == joint_index =>
                    {
                        Some((index, *recipe, comparison))
                    }
                    _ => None,
                }
            })
            .expect("the actual immutable recipe must retain its consumed PoleComparison");
        let fraction_before = plan.fraction.to_bits();
        let exits_before = [
            recipe.outgoing.exit.to_bits(),
            recipe.incoming.exit.to_bits(),
        ];
        let bulge_before = recipe.bulge.map(|v| [v.lo.to_bits(), v.hi.to_bits()]);
        let domain = source
            .face_domain(face_index, available, &mut work, &cancel)
            .unwrap();
        let scratch = available
            .checked_sub(plan.retained_bytes() + CONICAL_STACK_BYTES + domain.owned_bytes())
            .unwrap();
        comparison.validate(&domain, &recipe, vertex).unwrap();
        assert!(
            comparison
                .disk_owned(&domain, &recipe, scratch, &mut work, &cancel)
                .unwrap(),
            "Native pole incidence alone is not the closed source-disk ownership theorem",
        );
        let mut covered_cells = 0;
        for cell in plan.cells.iter().filter(|cell| cell.recipe == recipe_index) {
            let cost = comparison
                .source_cost(
                    &domain,
                    &recipe,
                    cell.piece,
                    Interval::new(cell.parameter[0], cell.parameter[1]).unwrap(),
                    plan.point,
                    &mut work,
                    &cancel,
                )
                .unwrap();
            assert!(cost.error_mm.is_finite() && cost.error_mm <= allowance);
            assert!(cost.homotopy_mm.is_finite() && cost.homotopy_mm <= allowance);
            assert!(length(cost.displacement).unwrap().hi <= allowance);
            assert!(
                cost.source
                    .iter()
                    .all(|v| v.lo.is_finite() && v.hi.is_finite())
            );
            covered_cells += 1;
        }
        assert!(
            covered_cells > 0,
            "the pole comparison must cover actual fixed G cells"
        );

        let mut foreign = None;
        'faces: for face in definition.solid.face_iter() {
            for wire in face.absolute_boundaries().iter() {
                for edge in wire.iter() {
                    for endpoint in [edge.front(), edge.back()] {
                        if endpoint.id() != vertex {
                            foreign = Some(endpoint.id());
                            break 'faces;
                        }
                    }
                }
            }
        }
        let error = comparison
            .validate(
                &domain,
                &recipe,
                foreign.expect("Housing must have another original NativeV"),
            )
            .unwrap_err();
        assert_eq!(error.code, GeometryErrorCode::AnalysisIndeterminate);
        let mut reversed = recipe;
        std::mem::swap(&mut reversed.outgoing, &mut reversed.incoming);
        assert_eq!(
            comparison
                .validate(&domain, &reversed, vertex)
                .unwrap_err()
                .code,
            GeometryErrorCode::AnalysisIndeterminate,
        );
        let mut wrong_branch = comparison;
        wrong_branch.pole = match comparison.pole {
            SphericalPole::North => SphericalPole::South,
            SphericalPole::South => SphericalPole::North,
        };
        assert_eq!(
            wrong_branch
                .validate(&domain, &recipe, vertex)
                .unwrap_err()
                .code,
            GeometryErrorCode::AnalysisIndeterminate,
        );
        let mut missing_joint = comparison;
        missing_joint.joint_index = domain.native().joints().len();
        assert_eq!(
            missing_joint
                .validate(&domain, &recipe, vertex)
                .unwrap_err()
                .code,
            GeometryErrorCode::AnalysisIndeterminate,
        );
        let mut excessive_position = comparison;
        excessive_position.transport[0].position_error_mm = up(allowance);
        assert_eq!(
            excessive_position
                .validate(&domain, &recipe, vertex)
                .unwrap_err()
                .code,
            GeometryErrorCode::AnalysisIndeterminate,
        );
        let mut nonstrict_derivative = comparison;
        nonstrict_derivative.transport[1].relative_derivative_error = 0.25;
        assert_eq!(
            nonstrict_derivative
                .validate(&domain, &recipe, vertex)
                .unwrap_err()
                .code,
            GeometryErrorCode::AnalysisIndeterminate,
        );
        // Semantic mutations above must not be accepted as resource refusals,
        // and none is permitted to change the fixed construction or source.
        assert_eq!(plan.fraction.to_bits(), fraction_before);
        assert_eq!(
            [
                plan.recipes[recipe_index].outgoing.exit.to_bits(),
                plan.recipes[recipe_index].incoming.exit.to_bits()
            ],
            exits_before,
        );
        assert_eq!(
            plan.recipes[recipe_index]
                .bulge
                .map(|v| [v.lo.to_bits(), v.hi.to_bits()]),
            bulge_before,
        );
        assert_eq!(
            geometry_before,
            serde_json::to_vec(&definition.solid.boundaries()[0].compress()).unwrap(),
        );
    }

    #[test]
    fn original_affine_sphere_graph_keeps_opposite_hemispheres_and_closed_cap_distinct() {
        use monstertruck_modeling::{Matrix4, Processor, Sphere, Surface};
        let transform = Matrix4::from_cols(
            [2.0, 0.5, 0.0, 0.0].into(),
            [0.0, 3.0, 0.25, 0.0].into(),
            [0.0, 0.0, 4.0, 0.0].into(),
            [10.0, 11.0, 12.0, 1.0].into(),
        );
        let surface = Surface::SphericalSurface(Processor::with_transform(
            Sphere::new(Point3::new(1.0, -2.0, 3.0), 2.0),
            transform,
        ));
        let xy = [0.125, -0.25].map(Interval::point);
        let north = PoleChart::coefficients(&surface, SphericalPole::North)
            .unwrap()
            .unwrap();
        let south = PoleChart::coefficients(&surface, SphericalPole::South)
            .unwrap()
            .unwrap();
        let (north_position, _, _) = north.graph_jet(xy, ZERO2).unwrap();
        let (south_position, _, _) = south.graph_jet(xy, ZERO2).unwrap();
        for (chart, own, opposite) in [
            (north, north_position, south_position),
            (south, south_position, north_position),
        ] {
            assert!(chart.same_hemisphere(own));
            assert!(chart.opposite_hemisphere(opposite));
            assert!(!chart.same_hemisphere(opposite));
            let local_own = chart.project(own);
            let local_opposite = chart.project(opposite);
            for axis in 0..2 {
                assert!(contains([local_own[axis]], [xy[axis]]));
                assert!(contains([local_opposite[axis]], [xy[axis]]));
            }
            let (origin, _, _, forward, minimum) = chart.fiber_parameters(xy);
            assert!(dot(forward, relative(own, origin)).lo > minimum);
            assert!(dot(forward, relative(opposite, origin)).hi < minimum);
            let error = chart
                .graph_jet(
                    [Interval::new(0.0, 1.0).unwrap(), Interval::point(0.0)],
                    ZERO2,
                )
                .unwrap_err();
            assert_eq!(error.code, GeometryErrorCode::AnalysisIndeterminate);
        }
    }
}
