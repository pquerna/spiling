// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! A constant INNER material query, not source-section invariance. The origin
//! is the complete per-role radial metric and filled-map theorem, never a
//! generic filled Hausdorff bound or an occupied outer envelope.
use super::*;
use crate::analysis::planar::{
    FillRule, material_at, point_segment_distance, segment_distance_lower,
};

#[derive(Debug, Clone)]
pub struct NativeInnerMaterialPrism {
    source: NativeUnionSourceCapture,
    interval: [f64; 2],
    sample: f64,
    native_sample_sha256: [u8; 32],
    actual_sha256: [u8; 32],
    native_displacement_mm: f64,
    coordinate_displacement_mm: f64,
}

impl NativeSourceLeafReceipt {
    /// Preserve only the direct complete per-role constructor's stronger
    /// theorem. Endpoint products, directed leaves and general seals cannot
    /// acquire inner authority through this constructor.
    pub fn capture_constant_inner_prism(&self) -> Option<NativeInnerMaterialPrism> {
        if self.proof_kind != NativeSourceLeafProofKind::ClosedSlab
            || self.has_owned_transition_material
            || !self.positive
        {
            return None;
        }
        Some(NativeInnerMaterialPrism {
            source: self.source.clone(),
            interval: self.world_height_interval_mm,
            sample: self.sample_world_z_mm,
            native_sample_sha256: self.native_sample_sha256,
            actual_sha256: self.candidate_geometry_sha256,
            native_displacement_mm: self.radial_product?.inner_displacement_mm?,
            // This bound was independently checked against the complete actual
            // oriented P/Q cycle correspondence, not assumed from a budget.
            coordinate_displacement_mm: self.sample_coordinate_error_mm?,
        })
    }
}

impl NativeInnerMaterialPrism {
    pub fn source(&self) -> &NativeUnionSourceCapture {
        &self.source
    }
    pub fn world_height_interval_mm(&self) -> [f64; 2] {
        self.interval
    }
    pub fn sample_world_z_mm(&self) -> f64 {
        self.sample
    }
    pub fn native_displacement_mm(&self) -> f64 {
        self.native_displacement_mm
    }
    pub fn coordinate_displacement_mm(&self) -> f64 {
        self.coordinate_displacement_mm
    }
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + source_retained_bytes(&self.source)
    }
    pub fn binding_sha256(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(b"spiling-native-constant-inner-material-1\0");
        hash_source(&mut hash, &self.source);
        hash.update(self.native_sample_sha256);
        hash.update(self.actual_sha256);
        for x in self.interval.into_iter().chain([
            self.sample,
            self.native_displacement_mm,
            self.coordinate_displacement_mm,
        ]) {
            hash.update(x.to_bits().to_le_bytes());
        }
        hash.finalize().into()
    }

    /// Certify ALL filled G, expanded by the conservative XY bead/projection
    /// radius, over the supplied OUTER physical Z enclosure. Returns a strict
    /// conservative clearance surplus, or None for a proved envelope veto.
    /// Unresolved predicates, resources and cancellation remain errors.
    #[allow(clippy::too_many_arguments)]
    pub fn certify_region(
        &self,
        source: &NativeUnionSourceCapture,
        actual: &[SectionLoopMm],
        eligible: &[SectionLoopMm],
        outer_z_mm: [f64; 2],
        outer_radius_mm: f64,
        byte_limit: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<f64>, GeometryError> {
        work.charge(1, cancel)?;
        if !self.source.same_native_material(source)
            || self.source.occurrence_id() != source.occurrence_id()
            || self.source.pose() != source.pose()
            || self.source.native_shape_error_policy_mm().to_bits()
                != source.native_shape_error_policy_mm().to_bits()
            || candidate_capture(actual, self.sample, work, cancel)?.geometry_sha256
                != self.actual_sha256
        {
            return Err(error(
                GeometryErrorCode::InvalidGeometry,
                "inner-material query differs from its original source/pose/reference/contributor binding",
            ));
        }
        if outer_z_mm.iter().any(|x| !x.is_finite())
            || outer_z_mm[0] >= outer_z_mm[1]
            || !outer_radius_mm.is_finite()
            || outer_radius_mm < 0.0
        {
            return Err(error(
                GeometryErrorCode::InvalidGeometry,
                "invalid inner-material prism dimensions",
            ));
        }
        if outer_z_mm[0] < self.interval[0] || outer_z_mm[1] > self.interval[1] {
            return Ok(None);
        }
        candidate_capture(eligible, self.sample, work, cancel)?;
        if eligible.is_empty() {
            return Ok(None);
        }
        let mut scratch = Scratch {
            bytes: MAX_SCRATCH - byte_limit.min(MAX_SCRATCH),
        };
        let mut q = Vec::new();
        let mut g = Vec::new();
        scratch.reserve(&mut q, actual.len(), work, cancel)?;
        scratch.reserve(&mut g, eligible.len(), work, cancel)?;
        for boundary in actual {
            q.push(ring(boundary, &mut scratch, work, cancel)?);
        }
        for boundary in eligible {
            g.push(ring(boundary, &mut scratch, work, cancel)?);
        }
        feature_clearance(&g, 0.0, &mut scratch, work, cancel)?;
        let required = Interval::point(self.native_displacement_mm)
            .add(Interval::point(self.coordinate_displacement_mm))
            .add(Interval::point(outer_radius_mm));
        if !required.hi.is_finite() {
            return Err(error(
                GeometryErrorCode::ConditioningFailure,
                "inner-material margin overflow",
            ));
        }
        let mut minimum = f64::INFINITY;
        for a in &g {
            for b in &q {
                for i in 0..a.points.len() {
                    for j in 0..b.points.len() {
                        work.charge(1, cancel)?;
                        let (p, r) = (a.points[i], a.points[(i + 1) % a.points.len()]);
                        let (s, t) = (b.points[j], b.points[(j + 1) % b.points.len()]);
                        let mut lower = segment_gap(p, r, s, t);
                        if lower <= required.hi {
                            lower = lower.max(segment_distance_lower(p, r, s, t)?);
                        }
                        if lower <= required.hi {
                            let upper = [
                                point_segment_distance(p.map(Interval::point), s, t)?.hi,
                                point_segment_distance(r.map(Interval::point), s, t)?.hi,
                                point_segment_distance(s.map(Interval::point), p, r)?.hi,
                                point_segment_distance(t.map(Interval::point), p, r)?.hi,
                            ]
                            .into_iter()
                            .fold(f64::INFINITY, f64::min);
                            let opposite = |x: Interval, y: Interval| {
                                (x.lo > 0.0 && y.hi < 0.0) || (x.hi < 0.0 && y.lo > 0.0)
                            };
                            let crossing = opposite(
                                cross(delta(r, p), delta(s, p)),
                                cross(delta(r, p), delta(t, p)),
                            ) && opposite(
                                cross(delta(t, s), delta(p, s)),
                                cross(delta(t, s), delta(r, s)),
                            );
                            if upper <= required.lo || crossing {
                                return Ok(None);
                            }
                            return Err(error(
                                GeometryErrorCode::AnalysisIndeterminate,
                                "inner-material strict boundary margin is unresolved",
                            ));
                        }
                        minimum = minimum.min(lower);
                    }
                }
            }
        }
        // Complete boundary separation plus material-side membership proves
        // each ENTIRE connected G component. A Q hole/outer boundary wholly
        // inside G must be tested separately: edge distances cannot detect it.
        for boundary in &g {
            if !boundary.hole {
                match material_at(
                    boundary.points[0],
                    actual,
                    FillRule::PositiveCoverage,
                    work,
                    cancel,
                )? {
                    Some(true) => {}
                    Some(false) => return Ok(None),
                    None => {
                        return Err(error(
                            GeometryErrorCode::AnalysisIndeterminate,
                            "inner-material component membership unresolved",
                        ));
                    }
                }
            }
        }
        for boundary in &q {
            match material_at(
                boundary.points[0],
                eligible,
                FillRule::PositiveCoverage,
                work,
                cancel,
            )? {
                Some(false) => {}
                Some(true) => return Ok(None),
                None => {
                    return Err(error(
                        GeometryErrorCode::AnalysisIndeterminate,
                        "inner-material enclosed source boundary unresolved",
                    ));
                }
            }
        }
        Ok(Some(Interval::point(minimum).sub(required).lo))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_housing_upper_fillet_reference_proves_closed_material_and_protected_hole() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(128_000_000);
        let definition = crate::from_step_bytes_with_cancel(include_bytes!(
            "../../../../../fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step"), &cancel).unwrap();
        let occurrence: spiling_contracts::geometry::OccurrenceRecord =
            serde_json::from_slice(include_bytes!(
                "../../../../../fixtures/manufacturing/rounded-housing-bed-occurrence.json"
            ))
            .unwrap();
        let prepared = crate::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let interval = [8.014650488771121, 8.311510884034451];
        let sample = 8.163080686402786;
        let analysis = prepared
            .analyze_placed_slab(
                &crate::PlacedSlabRequest {
                    pose: occurrence.pose,
                    world_height_interval_mm: interval,
                    sample_world_z_mm: sample,
                    native_shape_error_mm: 0.1,
                },
                crate::MAX_PREPARED_SECTION_SOURCE_BYTES - prepared.retained_bytes(),
                &mut work,
                &cancel,
            )
            .unwrap();
        assert_eq!(
            analysis
                .section
                .loops
                .iter()
                .filter(|boundary| boundary.is_hole)
                .count(),
            1
        );
        // Manufacturing consumes a planar XY candidate, not rewritten native
        // geometry. Preserve the actual source sample and its separate plane
        // residual while copying its XY exactly onto the common candidate Z.
        assert!(
            analysis
                .section
                .loops
                .iter()
                .flat_map(|boundary| &boundary.points_mm)
                .all(
                    |point| (point[2] - sample).abs() <= analysis.section.achieved_plane_error_mm()
                )
        );
        assert!(
            analysis.section.achieved_plane_error_mm()
                <= spiling_contracts::geometry::SECTION_PLANE_TOLERANCE_MM
        );
        let actual: Vec<_> = analysis
            .section
            .loops
            .iter()
            .map(|boundary| SectionLoopMm {
                is_hole: boundary.is_hole,
                points_mm: boundary
                    .points_mm
                    .iter()
                    .map(|point| [point[0], point[1], sample])
                    .collect(),
            })
            .collect();
        let actual = &actual;
        let leaf = certify_native_source_leaf(
            &SourceLeafRequest {
                source: &prepared,
                occurrence_id: &occurrence.occurrence_id,
                pose: occurrence.pose,
                analysis: &analysis,
                actual_used_source_loops: actual,
                policy: NativeUnionPolicy {
                    candidate_coordinate_error_mm: 0.0,
                },
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        let prism = leaf
            .capture_constant_inner_prism()
            .expect("complete closed source radial material authority");
        let candidate = |min: [f64; 2], max: [f64; 2]| {
            vec![SectionLoopMm {
                is_hole: false,
                points_mm: vec![
                    [min[0], min[1], sample],
                    [max[0], min[1], sample],
                    [max[0], max[1], sample],
                    [min[0], max[1], sample],
                    [min[0], min[1], sample],
                ],
            }]
        };
        let shift = occurrence.pose.translation_mm;
        let inside = candidate(
            [shift[0] + 4., shift[1] + 4.],
            [shift[0] + 7., shift[1] + 7.],
        );
        let hole = candidate(
            [shift[0] + 11., shift[1] + 8.],
            [shift[0] + 13., shift[1] + 10.],
        );
        assert!(
            prism
                .certify_region(
                    &leaf.source,
                    actual,
                    &inside,
                    interval,
                    0.225,
                    crate::MAX_PREPARED_SECTION_SOURCE_BYTES - prepared.retained_bytes(),
                    &mut work,
                    &cancel
                )
                .unwrap()
                .is_some()
        );
        assert!(
            prism
                .certify_region(
                    &leaf.source,
                    actual,
                    &hole,
                    interval,
                    0.225,
                    crate::MAX_PREPARED_SECTION_SOURCE_BYTES - prepared.retained_bytes(),
                    &mut work,
                    &cancel
                )
                .unwrap()
                .is_none()
        );
        assert!(work.consumed() <= 128_000_000);
    }

    #[test]
    fn original_housing_inner_prism_preserves_holes_scope_and_binding() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(128_000_000);
        let definition = crate::from_step_bytes_with_cancel(include_bytes!(
            "../../../../../fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step"
        ), &cancel).unwrap();
        let prepared = crate::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let analysis = prepared
            .analyze_placed_slab(
                &crate::PlacedSlabRequest {
                    pose: RigidPoseMm::IDENTITY,
                    world_height_interval_mm: [4.2, 8.0],
                    sample_world_z_mm: 6.1,
                    native_shape_error_mm: 0.1,
                },
                crate::MAX_PREPARED_SECTION_SOURCE_BYTES - prepared.retained_bytes(),
                &mut work,
                &cancel,
            )
            .unwrap();
        let actual: Vec<_> = analysis
            .section
            .loops
            .iter()
            .map(|boundary| SectionLoopMm {
                is_hole: boundary.is_hole,
                points_mm: boundary
                    .points_mm
                    .iter()
                    .map(|p| [p[0], p[1], 6.1])
                    .collect(),
            })
            .collect();
        let leaf = certify_native_source_leaf(
            &SourceLeafRequest {
                source: &prepared,
                occurrence_id: &OccurrenceId::new(1).unwrap(),
                pose: RigidPoseMm::IDENTITY,
                analysis: &analysis,
                actual_used_source_loops: &actual,
                policy: NativeUnionPolicy {
                    candidate_coordinate_error_mm: 0.0,
                },
            },
            &mut work,
            &cancel,
        )
        .unwrap();
        let capture = leaf
            .capture_constant_inner_prism()
            .expect("actual complete direct radial origin");
        let candidate = |outer_scale: f64, inner_scale: f64, holes: bool| {
            actual
                .iter()
                .filter(|boundary| holes || !boundary.is_hole)
                .map(|boundary| {
                    let scale = if boundary.is_hole {
                        inner_scale
                    } else {
                        outer_scale
                    };
                    SectionLoopMm {
                        is_hole: boundary.is_hole,
                        points_mm: boundary
                            .points_mm
                            .iter()
                            .map(|p| {
                                [
                                    12.0 + (p[0] - 12.0) * scale,
                                    9.0 + (p[1] - 9.0) * scale,
                                    6.1,
                                ]
                            })
                            .collect(),
                    }
                })
                .collect::<Vec<_>>()
        };
        // Propose a small box halfway between actual role-ray crossings.
        // These coordinates are search data; only certify_region admits it.
        let center = leaf
            .radial_product
            .unwrap()
            .center
            .map(|r| r[0] * 0.5 + r[1] * 0.5);
        let ray_crossing = |hole: bool| {
            actual
                .iter()
                .filter(|r| r.is_hole == hole)
                .flat_map(|r| r.points_mm.windows(2))
                .filter_map(|edge| {
                    let (a, b) = (edge[0], edge[1]);
                    if a[1] == b[1] || center[1] < a[1].min(b[1]) || center[1] > a[1].max(b[1]) {
                        return None;
                    }
                    let x = a[0] + (center[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
                    (x > center[0]).then_some(x)
                })
                .fold(f64::INFINITY, f64::min)
        };
        let outer = ray_crossing(false);
        let inner = ray_crossing(true);
        let x = outer * 0.5 + inner * 0.5;
        let h = (outer - inner) / 16.0;
        let good = vec![SectionLoopMm {
            is_hole: false,
            points_mm: vec![
                [x - h, center[1] - h, 6.1],
                [x + h, center[1] - h, 6.1],
                [x + h, center[1] + h, 6.1],
                [x - h, center[1] + h, 6.1],
                [x - h, center[1] - h, 6.1],
            ],
        }];
        assert!(
            capture
                .certify_region(
                    capture.source(),
                    &actual,
                    &good,
                    [4.4, 4.7],
                    0.225,
                    MAX_SCRATCH,
                    &mut work,
                    &cancel
                )
                .unwrap()
                .is_some()
        );
        // Deliberately omit the REAL native hole. Its complete boundary is
        // inside G, not intersecting G's outer boundary.
        let hole_cover = candidate(0.95, 1.0, false);
        let hole = actual.iter().find(|boundary| boundary.is_hole).unwrap();
        for point in &hole.points_mm {
            assert_eq!(
                material_at(
                    [point[0], point[1]],
                    &hole_cover,
                    FillRule::PositiveCoverage,
                    &mut work,
                    &cancel
                )
                .unwrap(),
                Some(true)
            );
        }
        // Every pair is strictly separated, so only the filled-set/hole test
        // can reject this candidate; an edge-distance-only query would admit it.
        for a in hole_cover.iter().flat_map(|ring| ring.points_mm.windows(2)) {
            for b in actual.iter().flat_map(|ring| ring.points_mm.windows(2)) {
                assert!(
                    segment_distance_lower(
                        [a[0][0], a[0][1]],
                        [a[1][0], a[1][1]],
                        [b[0][0], b[0][1]],
                        [b[1][0], b[1][1]]
                    )
                    .unwrap()
                        > capture.native_displacement_mm + capture.coordinate_displacement_mm
                );
            }
        }
        assert!(
            capture
                .certify_region(
                    capture.source(),
                    &actual,
                    &hole_cover,
                    [4.4, 4.7],
                    0.0,
                    MAX_SCRATCH,
                    &mut work,
                    &cancel
                )
                .unwrap()
                .is_none()
        );
        // Boundary-reaching is not admitted by equality or a geometry epsilon.
        assert!(
            capture
                .certify_region(
                    capture.source(),
                    &actual,
                    &actual,
                    [4.4, 4.7],
                    0.225,
                    MAX_SCRATCH,
                    &mut work,
                    &cancel
                )
                .unwrap()
                .is_none()
        );
        assert!(
            capture
                .certify_region(
                    capture.source(),
                    &actual,
                    &good,
                    [4.2_f64.next_down(), 4.7],
                    0.225,
                    MAX_SCRATCH,
                    &mut work,
                    &cancel
                )
                .unwrap()
                .is_none()
        );
        let changed = NativeUnionSourceCapture {
            identity: Arc::new(NativeUnionSourceIdentity {
                occurrence_id: capture.source.occurrence_id(),
                definition_id: capture.source.definition_id().clone(),
                material_realization: capture.source.material_realization_identity(),
                pose: RigidPoseMm {
                    translation_mm: [1.0, 0.0, 0.0],
                    ..RigidPoseMm::IDENTITY
                },
            }),
            native_shape_error_policy_mm: capture.source.native_shape_error_policy_mm(),
        };
        assert_eq!(
            capture
                .certify_region(
                    &changed,
                    &actual,
                    &good,
                    [4.4, 4.7],
                    0.225,
                    MAX_SCRATCH,
                    &mut work,
                    &cancel
                )
                .unwrap_err()
                .code,
            GeometryErrorCode::InvalidGeometry
        );
        let mut changed_actual: Vec<_> = actual
            .iter()
            .map(|boundary| SectionLoopMm {
                is_hole: boundary.is_hole,
                points_mm: boundary.points_mm.clone(),
            })
            .collect();
        changed_actual[0].points_mm[1][0] += 0.01;
        assert_eq!(
            capture
                .certify_region(
                    capture.source(),
                    &changed_actual,
                    &good,
                    [4.4, 4.7],
                    0.225,
                    MAX_SCRATCH,
                    &mut work,
                    &cancel
                )
                .unwrap_err()
                .code,
            GeometryErrorCode::InvalidGeometry
        );
        assert_eq!(
            capture
                .certify_region(
                    capture.source(),
                    &actual,
                    &good,
                    [4.4, 4.7],
                    0.225,
                    0,
                    &mut work,
                    &cancel
                )
                .unwrap_err()
                .code,
            GeometryErrorCode::ResourceLimit
        );
        // Removing the strong origin cannot be repaired by an otherwise valid
        // general closed metric or the complete radial topology alone.
        let mut without_origin = leaf.clone();
        without_origin
            .radial_product
            .as_mut()
            .unwrap()
            .inner_displacement_mm = None;
        assert!(without_origin.capture_constant_inner_prism().is_none());
    }
}
