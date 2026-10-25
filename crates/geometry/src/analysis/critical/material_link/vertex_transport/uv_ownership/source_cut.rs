// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Relative source-face surgery at a fixed proper crosscut. The reference
//! disk is NOT subtracted from native UV. Its outer B cuts the original face;
//! the component incident to the two original endpoint germs is replaced.
//! Native winding on the other side is unchanged. In particular, a connector
//! inside an EndpointUncertainty envelope need not itself be Jordan.

use super::*;

pub(super) const STACK_BYTES: usize =
    std::mem::size_of::<[([[f64; 2]; 2], u8); MAX_DEPTH as usize + 1]>()
        + std::mem::size_of::<[([f64; 2], u8); MAX_DEPTH as usize + 1]>()
        + 4 * std::mem::size_of::<UvBoundaryJet>()
        + 2 * std::mem::size_of::<Box2>()
        + std::mem::size_of::<Interval>()
        + 3 * std::mem::size_of::<OriginalCollar<'static, 'static>>()
        + 4 * std::mem::size_of::<crate::parametric::ChartCurveEnclosure>()
        + 2 * std::mem::size_of::<CorrectionJet>();

/// Call-local authority for the actual source complement, not a position
/// overcover. Its owner references bind the face/map, NativeV, literal germs,
/// fixed exits, completion, B and the original collar source laws together.
/// Only the relative-crosscut and exact pole-quotient constructors issue it.
pub(in crate::analysis::critical::material_link::vertex_transport) struct SourceFaceCut<'a, 'd> {
    domain: &'a PreparedFaceTrimDomain<'d>,
    recipe: &'a CornerRecipe,
}

impl SourceFaceCut<'_, '_> {
    pub(in crate::analysis::critical::material_link::vertex_transport) fn binds(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
    ) -> bool {
        std::ptr::eq(self.domain, domain) && std::ptr::eq(self.recipe, recipe)
    }

    pub(super) fn for_pole<'a, 'd>(
        comparison: &PoleComparison,
        domain: &'a PreparedFaceTrimDomain<'d>,
        recipe: &'a CornerRecipe,
        scratch: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<SourceFaceCut<'a, 'd>>, GeometryError> {
        // This separate lane has an authoritative analytic pole quotient and
        // original local inverse, not an unknown ordinary UV connector.
        Ok(comparison
            .disk_owned(domain, recipe, scratch, work, cancel)?
            .then_some(SourceFaceCut { domain, recipe }))
    }
}

/// A fixed endpoint root for EVERY member of a germ homotopy. A single
/// preconditioning matrix must contract the whole derivative family. This
/// proves both uniqueness and the same one-sided endpoint order, not merely
/// nonsingularity of a separately chosen matrix at each lambda.
fn pair_misses(
    root: Option<[f64; 2]>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    mut jets: impl FnMut(
        [Interval; 2],
        &mut GeometryWorkBudget,
        &AtomicBool,
    ) -> Result<[UvBoundaryJet; 2], GeometryError>,
) -> Result<bool, GeometryError> {
    let before = work.consumed();
    let mut active = ([[0.0, 1.0]; 2], 0);
    let mut phase = "pair traversal";
    let outcome = (|| {
        let mut stack = [([[0.0; 2]; 2], 0_u8); MAX_DEPTH as usize + 1];
        stack[0] = ([[0.0, 1.0]; 2], 0);
        let mut pending = 1;
        while pending != 0 {
            pending -= 1;
            let (cell, depth) = stack[pending];
            active = (cell, depth);
            phase = "pair traversal";
            work.charge(1, cancel)?;
            let parameter = cell.map(|v| Interval { lo: v[0], hi: v[1] });
            phase = "whole homotopy pair jets";
            let [a, b] = jets(parameter, work, cancel)?;
            if disjoint(a.position, b.position) {
                continue;
            }
            if let Some(root) = root {
                let extended = std::array::from_fn(|i| Interval {
                    lo: parameter[i].lo.min(root[i]),
                    hi: parameter[i].hi.max(root[i]),
                });
                phase = "uniform fixed-endpoint injectivity";
                let [a, b] = jets(extended, work, cancel)?;
                if pair_jacobian_injective(a.derivative, b.derivative) {
                    continue;
                }
            }
            // The center enclosures still contain EVERY homotopy member.
            // For each fixed lambda the complete-family first derivative
            // bounds its own center-to-cell integral. No sampled lambda,
            // independent endpoint membership, or interior map is asserted.
            phase = "whole-family midpoint jets";
            let [ma, mb] = jets(parameter.map(|v| Interval::point(v.mid())), work, cancel)?;
            let pa = mean_value_boundary_position(&a, ma.position, parameter[0])?;
            let pb = mean_value_boundary_position(&b, mb.position, parameter[1])?;
            if disjoint(pa, pb) {
                continue;
            }
            let axis = usize::from(cell[1][1] - cell[1][0] > cell[0][1] - cell[0][0]);
            let mid = cell[axis][0] * 0.5 + cell[axis][1] * 0.5;
            if depth >= MAX_DEPTH || mid <= cell[axis][0] || mid >= cell[axis][1] {
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
        Ok(true)
    })();
    outcome.map_err(|error| {
        work.annotate_error(
            error,
            before,
            format_args!(
                "sourceCut pair phase={phase} parameter_pair={:?} depth={} known_endpoint={root:?}",
                active.0, active.1,
            ),
        )
    })
}

fn curve_misses_box(
    envelope: Box2,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    mut image: impl FnMut(Interval, &mut GeometryWorkBudget, &AtomicBool) -> Result<Box2, GeometryError>,
) -> Result<bool, GeometryError> {
    let before = work.consumed();
    let mut active = ([0.0, 1.0], 0);
    let outcome = (|| {
        let mut stack = [([0.0; 2], 0_u8); MAX_DEPTH as usize + 1];
        stack[0] = ([0.0, 1.0], 0);
        let mut pending = 1;
        while pending != 0 {
            pending -= 1;
            let (range, depth) = stack[pending];
            active = (range, depth);
            work.charge(1, cancel)?;
            let cut = image(
                Interval {
                    lo: range[0],
                    hi: range[1],
                },
                work,
                cancel,
            )?;
            if disjoint(envelope, cut) {
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
        Ok(true)
    })();
    outcome.map_err(|error| {
        work.annotate_error(
            error,
            before,
            format_args!(
                "sourceCut fixed-cut curve vs whole joint envelope parameter_window={:?} depth={}",
                active.0, active.1,
            ),
        )
    })
}

impl UvJointComparison {
    fn germ_homotopy_jet(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        index: usize,
        s: Interval,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<UvBoundaryJet, GeometryError> {
        let end = if index == 0 {
            recipe.outgoing
        } else {
            recipe.incoming
        };
        let original = end_curve(domain, end, s, work, cancel)?;
        let completed = self.completed_end_jet(domain, recipe, index, s, work, cancel)?;
        // W(s,lambda)=gamma(s)+lambda*c(s). These are enclosures of that
        // expression and its s derivative for ALL lambda, not membership tests.
        Ok(UvBoundaryJet {
            position: hull2(original.position, completed.position),
            derivative: hull2(original.derivative, completed.derivative),
        })
    }

    fn joint_misses_cut(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        envelope: Box2,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        for boundary in [
            ComparisonBoundary::OutgoingCross,
            ComparisonBoundary::Bridge,
            ComparisonBoundary::IncomingCross,
        ] {
            if boundary == ComparisonBoundary::OutgoingCross && !recipe.outgoing.collar
                || boundary == ComparisonBoundary::IncomingCross && !recipe.incoming.collar
            {
                continue;
            }
            let before = work.consumed();
            if !curve_misses_box(envelope, work, cancel, |t, work, cancel| {
                Ok(self
                    .boundary_jet(domain, recipe, boundary, t, work, cancel)?
                    .position)
            })
            .map_err(|error| {
                work.annotate_error(
                    error,
                    before,
                    format_args!("sourceCut joint envelope vs fixed boundary={boundary:?}",),
                )
            })? {
                return Ok(false);
            }
        }
        // Also exclude passage around either exit into a retained central
        // continuation. The whole convex envelope, not its midpoint, must be
        // strictly on the endpoint side of BOTH exact exit supports.
        for end in [recipe.outgoing, recipe.incoming] {
            let at = chart_jet(domain, end.use_.coedge, [end.exit; 2], work, cancel)?;
            let direction = if end.spatial_high { -1.0 } else { 1.0 };
            let row = at.derivative.map(|v| v.mul(Interval::point(direction)));
            if dot2(
                row,
                std::array::from_fn(|i| envelope[i].sub(at.position[i])),
            )
            .hi >= 0.0
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn joint_misses_other_sources(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        envelope: Box2,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        for index in 0..domain.coedges().len() {
            let own = [recipe.outgoing, recipe.incoming]
                .into_iter()
                .find(|end| end.use_.coedge == index);
            let central_map = own
                .map(|_| domain.original_coedge_correspondence(index))
                .transpose()?;
            for span in domain.boundary_parameter_intervals(index)? {
                let mut range = span;
                if let Some(map) = central_map {
                    // The boundary inventory is in original PCurve parameters;
                    // the collar and literal exits use original spatial-provider parameters.
                    let Some(spatial) = map.affine_spatial_parameter_preimage(span)? else {
                        return Ok(false);
                    };
                    let spatial_range = map.spatial_range();
                    range = [
                        spatial.lo.max(spatial_range[0]),
                        spatial.hi.min(spatial_range[1]),
                    ];
                    // Restrict that proved spatial preimage to the SAME central
                    // scope, preserving both germs of a closed original edge.
                    for end in [recipe.outgoing, recipe.incoming] {
                        if own.is_some_and(|owner| end.edge == owner.edge) {
                            if end.spatial_high {
                                range[1] = range[1].min(end.exit);
                            } else {
                                range[0] = range[0].max(end.exit);
                            }
                        }
                    }
                    if range[0] > range[1] {
                        continue;
                    }
                }
                let mut stack = [([0.0; 2], 0_u8); MAX_DEPTH as usize + 1];
                stack[0] = (range, 0);
                let mut pending = 1;
                while pending != 0 {
                    pending -= 1;
                    let (range, depth) = stack[pending];
                    let before = work.consumed();
                    let image = (|| {
                        work.charge(1, cancel)?;
                        if own.is_some() {
                            OriginalCollar::spatial_collar_overcover(
                                domain, index, range, work, cancel,
                            )
                        } else {
                            OriginalCollar::other_collar_overcover(domain, index, range, work, cancel)
                        }
                    })().map_err(|error| work.annotate_error(error, before, format_args!(
                        "sourceCut joint envelope vs original collar coedge={index} spatial_owned={} parameter_window={range:?} depth={depth}",
                        own.is_some(),
                    )))?;
                    if disjoint(envelope, image) {
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
        }
        for (index, joint) in domain.original_joints()?.iter().enumerate() {
            work.charge(1, cancel)?;
            if matches!(self.authority,
                EndpointAuthority::EndpointUncertaintyJoint { index: own, .. } if own == index)
            {
                continue;
            }
            if !disjoint(envelope, joint.enclosure) {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Extend the original attachment by a proved connected UV bridge, not by
    /// padding the native joint or asserting trim membership. The source cut
    /// must already be qualified. Missing B, both retained central continuations
    /// and every foreign source keeps the whole bridge on the same replaced
    /// side as the original joint. Only UV laws are evaluated here.
    pub(in crate::analysis::critical::material_link::vertex_transport) fn connection_stays_on_replaced_side(
        &self,
        domain: &PreparedFaceTrimDomain<'_>,
        recipe: &CornerRecipe,
        connection: Box2,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        self.validate(domain, recipe)?;
        let EndpointAuthority::EndpointUncertaintyJoint { enclosure, .. } = self.authority else {
            return Ok(false);
        };
        if !contains(connection, enclosure) {
            return Ok(false);
        }
        Ok(
            self.joint_misses_cut(domain, recipe, connection, work, cancel)?
                && self.joint_misses_other_sources(domain, recipe, connection, work, cancel)?,
        )
    }

    pub(in crate::analysis::critical::material_link::vertex_transport) fn source_cut<'a, 'd>(
        &self,
        domain: &'a PreparedFaceTrimDomain<'d>,
        recipe: &'a CornerRecipe,
        scratch: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<SourceFaceCut<'a, 'd>>, GeometryError> {
        let before = work.consumed();
        let mut phase = "relative-cut scratch admission";
        let mut active_pair = None;
        let outcome = (|| {
            // Keep both traversal frames and live collar/jet state separate from
            // the native query allowance. The vertex owner also reserves the
            // aggregate ownership frame before borrowing/preparing its domain.
            let query_bytes = scratch.checked_sub(STACK_BYTES).ok_or_else(|| {
                GeometryError::new(
                    GeometryErrorCode::ResourceLimit,
                    "source-face relative-cut frame exceeds admitted scratch",
                )
            })?;
            // Supplies simple B, degree-one disk, complete foreign trim/collar
            // and joint exclusion, exact central ribbon laws and exit separation.
            // None of these facts alone supplies original complement ownership.
            phase = "complete reference disk ownership";
            if !self.disk_owned(domain, recipe, query_bytes, work, cancel)? {
                return Ok(None);
            }
            for index in 0..2 {
                for boundary in [
                    ComparisonBoundary::OutgoingCross,
                    ComparisonBoundary::Bridge,
                    ComparisonBoundary::IncomingCross,
                ] {
                    if boundary == ComparisonBoundary::OutgoingCross && !recipe.outgoing.collar
                        || boundary == ComparisonBoundary::IncomingCross && !recipe.incoming.collar
                    {
                        continue;
                    }
                    let root = match (index, boundary) {
                        (0, ComparisonBoundary::OutgoingCross) => Some([1.0, 0.0]),
                        (0, ComparisonBoundary::Bridge) if !recipe.outgoing.collar => {
                            Some([1.0, 0.0])
                        }
                        (1, ComparisonBoundary::IncomingCross) => Some([1.0, 1.0]),
                        (1, ComparisonBoundary::Bridge) if !recipe.incoming.collar => {
                            Some([1.0, 1.0])
                        }
                        _ => None,
                    };
                    // c_i(1)=0 is an exact completion identity. The root therefore
                    // uses the SAME original coedge exit for every lambda.
                    phase = "whole germ homotopy vs fixed crosscut";
                    active_pair = Some((index, boundary));
                    if !pair_misses(root, work, cancel, |parameter, work, cancel| {
                        Ok([
                            self.germ_homotopy_jet(
                                domain,
                                recipe,
                                index,
                                parameter[0],
                                work,
                                cancel,
                            )?,
                            self.boundary_jet(
                                domain,
                                recipe,
                                boundary,
                                parameter[1],
                                work,
                                cancel,
                            )?,
                        ])
                    })? {
                        return Err(ownership_failure(
                            recipe,
                            format_args!(
                                "sourceCut germ_homotopy={index} crosses_fixed_cut={boundary:?}"
                            ),
                        ));
                    }
                }
            }
            active_pair = None;
            phase = "whole native joint vs fixed cut and exit supports";
            let envelope = match self.authority {
                EndpointAuthority::ExactNativeAdjacency => self.joint,
                EndpointAuthority::EndpointUncertaintyJoint { enclosure, .. } => enclosure,
                EndpointAuthority::SphericalPoleJoint { .. } => return Ok(None),
            };
            if !self.joint_misses_cut(domain, recipe, envelope, work, cancel)? {
                return Err(ownership_failure(
                    recipe,
                    "sourceCut whole_native_joint_crosses_cut",
                ));
            }
            phase = "whole native joint vs foreign sources and central continuations";
            if !self.joint_misses_other_sources(domain, recipe, envelope, work, cancel)? {
                return Err(ownership_failure(
                    recipe,
                    "sourceCut whole_native_joint_foreign_or_central",
                ));
            }
            // The reference interior anchor is insufficient: original germs could
            // separate it from B. Calibrate B itself, AFTER excluding ALL original
            // boundaries from its open arc. Connectedness then fixes the native
            // material side along the entire proper crosscut.
            phase = "fixed B source anchor and original Inside classification";
            let anchor = self
                .boundary_jet(
                    domain,
                    recipe,
                    ComparisonBoundary::Bridge,
                    Interval::point(0.5),
                    work,
                    cancel,
                )?
                .position;
            let classifier_bytes = query_bytes.min(MAX_SCRATCH_BYTES);
            let before_classify = work.consumed();
            if domain.classify_with_byte_limit(uv_box(anchor), classifier_bytes, work, cancel)
            .map_err(|error| work.annotate_error(error, before_classify, format_args!(
                "sourceCut fixed B midpoint original Inside query anchor={anchor:?} classifier_bytes={classifier_bytes}",
            )))? != TrimClassification::Inside
        {
            return Err(ownership_failure(
                recipe,
                format_args!("sourceCut fixed_B_not_native_inside anchor={anchor:?}"),
            ));
        }
            // Relative crosscut theorem: the uniform fixed-exit order connects
            // each original germ to the replaced side of the reference disk. Its
            // entire homotopy cannot cross B or go around the central exit seam.
            // The convex native envelope is connected to those same endpoints and
            // misses B and both continuations, so EVERY connector confined to it
            // is on that side too. Its possibly non-Jordan loops change winding
            // only there. Foreign boundaries are excluded by disk_owned. Cutting
            // the original winding domain along B therefore retains precisely the
            // other side, with B and the fixed central ribbons its sole new
            // interfaces. This is relative boundary surgery, NOT an assertion
            // that the completed UV disk is the removed original set.
            Ok(Some(SourceFaceCut { domain, recipe }))
        })();
        outcome.map_err(|error| work.annotate_error(error, before, format_args!(
            "sourceCut face={} coedges={:?} phase={phase} germ_homotopy_fixed_boundary={active_pair:?}",
            recipe.outgoing.use_.face,
            [recipe.outgoing.use_.coedge, recipe.incoming.use_.coedge],
        )))
    }
}

#[cfg(test)]
mod properties {
    use super::*;

    #[test]
    fn a_fixed_exit_does_not_allow_germ_homotopy_to_cross_retained_side() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(20_000_000);
        // B(t)=(1,t). All germ homotopies fix gamma(1)=(1,0).
        // The small bow stays on the replaced side. A larger bow crosses B
        // away from that exit although its original and completed endpoint
        // curves are each regular and their exit equality is unchanged.
        for (bow, expected) in [(0.25, true), (2.0, false)] {
            let result = pair_misses(Some([1.0, 0.0]), &mut work, &cancel, |p, _, _| {
                let [s, t] = p;
                let complement = source_unit_complement(s)?;
                let shift = Interval { lo: 0.0, hi: bow };
                Ok([
                    UvBoundaryJet {
                        position: [s.add(shift.mul(s).mul(complement)), s.mul(complement)],
                        derivative: [
                            Interval::point(1.0).add(
                                shift.mul(Interval::point(1.0).sub(Interval::point(2.0).mul(s))),
                            ),
                            Interval::point(1.0).sub(Interval::point(2.0).mul(s)),
                        ],
                    },
                    UvBoundaryJet {
                        position: [Interval::point(1.0), t],
                        derivative: [Interval::point(0.0), Interval::point(1.0)],
                    },
                ])
            })
            .unwrap();
            assert_eq!(result, expected);
        }
    }

    #[test]
    fn native_joint_envelope_must_miss_cut_even_when_its_center_misses() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(20_000_000);
        for (x, expected) in [
            (
                Interval {
                    lo: -0.25,
                    hi: 0.25,
                },
                true,
            ),
            (
                Interval {
                    lo: -0.25,
                    hi: 1.25,
                },
                false,
            ),
        ] {
            let envelope = [x, Interval { lo: 0.25, hi: 0.75 }];
            assert!(x.mid() < 1.0, "both envelope centers miss B");
            assert_eq!(
                curve_misses_box(envelope, &mut work, &cancel, |t, _, _| {
                    Ok([Interval::point(1.0), t])
                })
                .unwrap(),
                expected,
            );
        }
    }
}
