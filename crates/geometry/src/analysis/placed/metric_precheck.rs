// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::kernel::TrimClassification;
use crate::parametric::{surface_parameter_boxes, surface_piecewise_enclosure};

const DIRECTIONS: usize = 16;
const MAX_DEPTH: usize = 12;
const MAX_CELLS: usize = 512;
const SCRATCH_BYTES: usize = 128 * 1024
    + super::super::slab::HEIGHT_CONTRACTION_SCRATCH_BYTES
    + crate::parametric::CORRELATED_SURFACE_FIRST_SCRATCH_BYTES;

#[derive(Debug, Clone, Copy)]
struct Support {
    direction: [Interval; 2],
    upper: f64,
    norm_upper: f64,
}

fn optional<T>(result: Result<T, GeometryError>) -> Result<Option<T>, GeometryError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(failure)
            if matches!(
                failure.code,
                GeometryErrorCode::AnalysisIndeterminate
                    | GeometryErrorCode::ConditioningFailure
                    | GeometryErrorCode::ParameterCorrespondenceUnavailable
                    | GeometryErrorCode::SingularGeometry
            ) =>
        {
            Ok(None)
        }
        Err(failure) => Err(failure),
    }
}

fn supports(
    loops: &[crate::SectionLoopMm],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<[Support; DIRECTIONS]>, GeometryError> {
    let mut result = [Support {
        direction: [Interval::point(0.0); 2],
        upper: f64::NEG_INFINITY,
        norm_upper: 0.0,
    }; DIRECTIONS];
    if loops.is_empty() {
        return Ok(None);
    }
    for (index, support) in result.iter_mut().enumerate() {
        work.charge(1, cancel)?;
        let angle = std::f64::consts::TAU * index as f64 / DIRECTIONS as f64;
        support.direction = [angle.cos(), angle.sin()].map(Interval::point);
        support.norm_upper = length([
            support.direction[0],
            support.direction[1],
            Interval::point(0.0),
        ])?
        .hi;
        for boundary in loops {
            work.charge(1, cancel)?;
            if boundary.points_mm.len() < 4
                || boundary.points_mm.first() != boundary.points_mm.last()
            {
                return Ok(None);
            }
            for point in &boundary.points_mm {
                work.charge(1, cancel)?;
                if point.iter().any(|coordinate| !coordinate.is_finite()) {
                    return Ok(None);
                }
                let projection = add(
                    mul(support.direction[0], Interval::point(point[0])),
                    mul(support.direction[1], Interval::point(point[1])),
                );
                support.upper = support.upper.max(projection.hi);
            }
        }
        if !support.upper.is_finite()
            || !support.norm_upper.is_finite()
            || support.norm_upper <= 0.0
        {
            return Ok(None);
        }
    }
    Ok(Some(result))
}

fn separation(
    supports: &[Support; DIRECTIONS],
    position: [Interval; 3],
    target: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<f64>, GeometryError> {
    for support in supports {
        work.charge(1, cancel)?;
        let projection = add(
            mul(support.direction[0], position[0]),
            mul(support.direction[1], position[1]),
        );
        let lower = div(
            sub(projection, Interval::point(support.upper)),
            Interval::point(support.norm_upper),
        )?
        .lo;
        if lower.is_finite() && lower > target {
            return Ok(Some(lower));
        }
    }
    Ok(None)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn reject_separated_candidate(
    source: super::super::material_source::MaterialSourceView<'_, '_>,
    request: &PlacedSlabRequest,
    section: &NativeSection,
    candidate: &WorldSampleCandidate,
    placement: &SamplePlacement<'_>,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    optional((|| {
    work.charge(1, cancel)?;
    if !section.matches_native_material(source.definition()) || !section.is_native_intersection() {
        return Err(error(
            GeometryErrorCode::InvalidGeometry,
            "negative metric query requires the original native material sample",
        ));
    }
    let available = remaining_bytes.checked_sub(SCRATCH_BYTES).ok_or_else(|| {
        error(
            GeometryErrorCode::ResourceLimit,
            "negative metric query exceeds coexisting scratch allowance",
        )
    })?;
    let Some(supports) = supports(&candidate.loops, work, cancel)? else {
        return Ok(());
    };
    let mut plane_return = 0.0_f64;
    for boundary in &candidate.loops {
        for point in &boundary.points_mm {
            work.charge(1, cancel)?;
            plane_return = plane_return
                .max(sub(Interval::point(point[2]), Interval::point(placement.sample)).abs_upper());
        }
    }
    let approximation = add(
        mul(
            Interval::point(placement.sigma),
            add(
                add(
                    Interval::point(section.achieved_chord_error_mm()),
                    Interval::point(section.achieved_plane_error_mm()),
                ),
                Interval::point(section.boolean_tolerance_mm),
            ),
        ),
        add(
            Interval::point(candidate.arithmetic),
            Interval::point(plane_return),
        ),
    );
    let target = add(
        Interval::point(request.native_shape_error_mm),
        mul(Interval::point(3.0), approximation),
    )
    .hi;
    if !target.is_finite() {
        return Ok(());
    }
    let tolerance = source.trim_source().effective_tolerance_mm();
    let height = HeightFunctional {
        direction: placement.map.matrix[2],
        offset: Interval::point(request.pose.translation_mm[2]),
    };
    let mut cells = 0;
    for ordinal in 0..source.definition().face_ids().len() {
        let domain = source.face_domain(ordinal, available, work, cancel)?;
        if !domain.is_native_material() || !std::ptr::eq(domain.definition(), source.definition()) {
            return Err(error(
                GeometryErrorCode::InvalidGeometry,
                "negative metric query changed its native face owner",
            ));
        }
        let surface = domain.surface_operand();
        let Some(spans) = optional(surface_parameter_boxes(
            &surface,
            domain.uv_domain(),
            work,
            cancel,
        ))?
        else {
            continue;
        };
        for span in spans {
            let mut stack = [([[0.0; 2]; 2], 0usize); MAX_DEPTH + 1];
            stack[0] = (span, 0);
            let mut count = 1;
            while count > 0 {
                work.charge(1, cancel)?;
                if cells == MAX_CELLS {
                    return Ok(());
                }
                cells += 1;
                count -= 1;
                let (uv, depth) = stack[count];
                let Some(Some((uv, jet))) = optional(super::super::slab::clip_height_box(
                    &surface,
                    uv,
                    height,
                    request.world_height_interval_mm,
                    work,
                    cancel,
                ))?
                else {
                    continue;
                };
                if optional(domain.classify_with_byte_limit(uv, available, work, cancel))?
                    == Some(TrimClassification::Outside)
                {
                    continue;
                }
                let point_uv = uv.map(|range| [range[0] * 0.5 + range[1] * 0.5; 2]);
                if let Some(point) = optional(surface_piecewise_enclosure(
                    &surface, point_uv, work, cancel,
                ))? {
                    let position = transformed_position(
                        placement.map.matrix,
                        request.pose,
                        point.position.map(|value| add(value, symmetric(tolerance))),
                    )?;
                    if request.world_height_interval_mm[0] <= position[2].lo
                        && position[2].hi <= request.world_height_interval_mm[1]
                        && let Some(gap) = separation(&supports, position, target, work, cancel)?
                        && optional(
                            domain.classify_with_byte_limit(point_uv, available, work, cancel),
                        )? == Some(TrimClassification::Inside)
                    {
                        return Err(GeometryError::new(
                            GeometryErrorCode::ShapeErrorExceeded,
                            format!(
                                "retained native face point exceeds actual world candidate support: face={ordinal} uv={point_uv:?} separation_lower_mm={gap} rejection_target_mm={target} interval={:?}",
                                request.world_height_interval_mm,
                            ),
                        ));
                    }
                }
                if depth == MAX_DEPTH {
                    continue;
                }
                let widths = [jet.du, jet.dv].map(|derivative| {
                    length(apply(placement.map.matrix, derivative)).map(|norm| norm.hi)
                });
                let [width_u, width_v] = widths;
                let widths = [width_u?, width_v?];
                let axis = usize::from(
                    widths[1] * (uv[1][1] - uv[1][0]) > widths[0] * (uv[0][1] - uv[0][0]),
                );
                let middle = uv[axis][0] * 0.5 + uv[axis][1] * 0.5;
                if middle <= uv[axis][0] || middle >= uv[axis][1] {
                    continue;
                }
                let mut children = [uv; 2];
                children[0][axis][1] = middle;
                children[1][axis][0] = middle;
                for child in children {
                    stack[count] = (child, depth + 1);
                    count += 1;
                }
            }
        }
    }
    Ok(())
    })())
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tilted_split_cap_candidate_reaches_proved_metric_refinement() {
        let cancel = AtomicBool::new(false);
        let definition = crate::from_step_bytes_with_cancel(
            include_bytes!("../../../../../fixtures/geometry/cylinder.step"),
            &cancel,
        )
        .unwrap();
        let mut work = GeometryWorkBudget::new(100_000_000);
        let source = crate::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let half_angle = 0.5 * 1.0_f64.to_radians();
        let component = half_angle.sin() / 2.0_f64.sqrt();
        let failure = analyze_placed_slab(
            &source,
            &PlacedSlabRequest {
                pose: RigidPoseMm {
                    translation_mm: [0.0, 0.0, 0.08726261625534897],
                    rotation_xyzw: [component, component, 0.0, half_angle.cos()],
                },
                world_height_interval_mm: [0.0, 0.02555904155621155],
                sample_world_z_mm: 0.012779520778105775,
                native_shape_error_mm: 0.1,
            },
            crate::MAX_PREPARED_SECTION_BYTES - source.retained_bytes(),
            &mut work,
            &cancel,
        )
        .unwrap_err();
        assert_eq!(
            failure.code,
            GeometryErrorCode::ShapeErrorExceeded,
            "{failure:?}"
        );
        assert!(
            failure.message.contains("actual world candidate support"),
            "{failure:?}"
        );
    }

    #[test]
    fn negative_metric_preserves_native_owner_world_scope_and_terminal_limits() {
        let cancel = AtomicBool::new(false);
        let definition = crate::from_step_bytes_with_cancel(
            include_bytes!("../../../../../fixtures/geometry/cylinder.step"),
            &cancel,
        )
        .unwrap();
        let mut work = GeometryWorkBudget::new(100_000_000);
        let source = crate::prepare_solid(
            &definition,
            crate::MAX_PREPARED_SECTION_SOURCE_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        let remaining = crate::MAX_PREPARED_SECTION_BYTES - source.retained_bytes();
        let half_angle = 0.5 * 1.0_f64.to_radians();
        let component = half_angle.sin() / 2.0_f64.sqrt();
        let request = PlacedSlabRequest {
            pose: RigidPoseMm {
                translation_mm: [10000.0, -10000.0, 0.08726261625534897],
                rotation_xyzw: [component, component, 0.0, half_angle.cos() * (1.0 + 2e-11)],
            },
            world_height_interval_mm: [0.0, 0.02555904155621155],
            sample_world_z_mm: 0.012779520778105775,
            native_shape_error_mm: 0.1,
        };
        let map = HeightMap::new(request.pose, [10.0; 3]).unwrap();
        let sigma = spectral_bound(map.matrix).unwrap();
        assert!(sigma > 1.0);
        let range = map.source_range(request.world_height_interval_mm).unwrap();
        let sample = div(
            sub(
                Interval::point(request.sample_world_z_mm),
                Interval::point(request.pose.translation_mm[2]),
            ),
            Interval::point(map.scale),
        )
        .unwrap()
        .mid();
        let mut prepared = super::super::super::slab::prepare_slab_sample(
            &source,
            SlabRequest {
                frame: map.frame,
                height_interval_mm: range,
                sample_height_mm: sample,
                native_shape_error_mm: div(
                    Interval::point(request.native_shape_error_mm),
                    Interval::point(sigma),
                )
                .unwrap()
                .lo,
            },
            remaining,
            &mut work,
            &cancel,
        )
        .unwrap();
        let placement = SamplePlacement {
            map: &map,
            pose: request.pose,
            sample: request.sample_world_z_mm,
            sigma,
            extent: [10.0; 3],
        };
        let candidate = WorldSampleCandidate::prepare(
            &prepared.section,
            &placement,
            remaining,
            &mut work,
            &cancel,
        )
        .unwrap();
        let allowance = remaining - prepared.section.retained_bytes() - candidate.retained_bytes();
        let view = source.source_view().unwrap();
        let failure = reject_separated_candidate(
            view,
            &request,
            &prepared.section,
            &candidate,
            &placement,
            allowance,
            &mut work,
            &cancel,
        )
        .unwrap_err();
        assert_eq!(failure.code, GeometryErrorCode::ShapeErrorExceeded);
        assert!(failure.message.contains("actual world candidate support"));
        for (limit, budget, cancelled, expected) in [
            (0, 100_000_000, false, GeometryErrorCode::ResourceLimit),
            (allowance, 0, false, GeometryErrorCode::ResourceLimit),
            (allowance, 100_000_000, true, GeometryErrorCode::Cancelled),
        ] {
            assert_eq!(
                reject_separated_candidate(
                    view,
                    &request,
                    &prepared.section,
                    &candidate,
                    &placement,
                    limit,
                    &mut GeometryWorkBudget::new(budget),
                    &AtomicBool::new(cancelled)
                )
                .unwrap_err()
                .code,
                expected
            );
        }
        let original_id = prepared.section.certificate.definition_id.clone();
        prepared.section.certificate.definition_id =
            spiling_contracts::geometry::DefinitionId::from_source_sha256(&[0; 32]);
        assert_eq!(
            reject_separated_candidate(
                view,
                &request,
                &prepared.section,
                &candidate,
                &placement,
                allowance,
                &mut work,
                &cancel
            )
            .unwrap_err()
            .code,
            GeometryErrorCode::InvalidGeometry
        );
        prepared.section.certificate.definition_id = original_id;
        for changed in [
            PlacedSlabRequest {
                native_shape_error_mm: 10.0,
                ..request
            },
            PlacedSlabRequest {
                world_height_interval_mm: [-1.0, -0.5],
                ..request
            },
        ] {
            let before = work.consumed();
            reject_separated_candidate(
                view,
                &changed,
                &prepared.section,
                &candidate,
                &placement,
                allowance,
                &mut work,
                &cancel,
            )
            .unwrap();
            assert!(work.consumed() > before);
        }
    }

    #[test]
    fn optional_metric_conditioning_falls_through_without_hiding_terminal_errors() {
        assert!(
            optional(length([Interval::point(1e155); 3]))
                .unwrap()
                .is_none()
        );
        for code in [
            GeometryErrorCode::Cancelled,
            GeometryErrorCode::ResourceLimit,
            GeometryErrorCode::InvalidGeometry,
            GeometryErrorCode::ShapeErrorExceeded,
        ] {
            assert_eq!(
                optional::<()>(Err(error(code, "retained cause")))
                    .unwrap_err()
                    .code,
                code
            );
        }
    }

    fn polygon(points: &[[f64; 2]], hole: bool) -> crate::SectionLoopMm {
        crate::SectionLoopMm {
            is_hole: hole,
            points_mm: points
                .iter()
                .map(|point| [point[0], point[1], 0.0])
                .collect(),
        }
    }

    #[test]
    fn whole_vertex_support_covers_concavity_holes_and_every_component() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let mut loops = vec![
            polygon(
                &[
                    [0.0, 0.0],
                    [2.0, 0.0],
                    [1.0, 1.0],
                    [2.0, 2.0],
                    [0.0, 2.0],
                    [0.0, 0.0],
                ],
                false,
            ),
            polygon(&[[0.1, 0.1], [0.1, 0.2], [0.2, 0.2], [0.1, 0.1]], true),
        ];
        let bounds = supports(&loops, &mut work, &cancel).unwrap().unwrap();
        assert!(
            separation(
                &bounds,
                [1.9, 1.0, 0.0].map(Interval::point),
                0.01,
                &mut work,
                &cancel
            )
            .unwrap()
            .is_none()
        );
        assert!(
            separation(
                &bounds,
                [3.0, 1.0, -1000.0].map(Interval::point),
                0.9,
                &mut work,
                &cancel
            )
            .unwrap()
            .is_some()
        );
        loops.push(polygon(
            &[[2.5, 0.0], [3.5, 0.0], [3.5, 2.0], [2.5, 2.0], [2.5, 0.0]],
            false,
        ));
        let bounds = supports(&loops, &mut work, &cancel).unwrap().unwrap();
        assert!(
            separation(
                &bounds,
                [3.0, 1.0, 1000.0].map(Interval::point),
                0.01,
                &mut work,
                &cancel
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn support_rejection_is_strict_physical_and_cancellable() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let loops = [polygon(
            &[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0], [0.0, 0.0]],
            false,
        )];
        let bounds = supports(&loops, &mut work, &cancel).unwrap().unwrap();
        assert!(
            separation(
                &bounds,
                [2.0, 0.5, 0.0].map(Interval::point),
                1.0,
                &mut work,
                &cancel
            )
            .unwrap()
            .is_none()
        );
        assert!(
            separation(
                &bounds,
                [2.0, 0.5, 0.0].map(Interval::point),
                0.99,
                &mut work,
                &cancel
            )
            .unwrap()
            .is_some()
        );
        assert!(
            separation(
                &bounds,
                [1.0, 0.25, 0.0].map(Interval::point),
                0.1,
                &mut work,
                &cancel
            )
            .unwrap()
            .is_none()
        );
        assert_eq!(
            supports(&loops, &mut GeometryWorkBudget::new(0), &cancel)
                .unwrap_err()
                .code,
            GeometryErrorCode::ResourceLimit
        );
        assert_eq!(
            supports(&loops, &mut work, &AtomicBool::new(true))
                .unwrap_err()
                .code,
            GeometryErrorCode::Cancelled
        );
    }
}
