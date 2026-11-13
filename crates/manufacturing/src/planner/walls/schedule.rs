// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;
use geo::kernels::{Kernel, Orientation, RobustKernel};
use spiling_geometry::{Interval, IntervalVec2};

pub(in crate::planner) struct SourceCommandHeightLimit {
    pub(in crate::planner) maximum_mm: f64,
}

impl SourceCommandHeightLimit {
    pub(in crate::planner) fn first_violating(
        &self,
        schedule: &[super::super::LayerElevation],
        range: std::ops::Range<u32>,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<Option<u32>, ManufacturingError> {
        for index in range {
            super::super::preflight::charge_capture_proposal_work(work, 1, cancel)?;
            let layer = &schedule[index as usize];
            if layer.z - layer.lower_z > self.maximum_mm {
                return Ok(Some(index));
            }
        }
        Ok(None)
    }
}

pub(in crate::planner) fn source_command_height_limit(
    material: &MultiPolygon<f64>,
    nozzle_diameter_mm: f64,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<SourceCommandHeightLimit>, ManufacturingError> {
    let uncertainty = program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM;
    let extent = Interval::point(nozzle_diameter_mm)
        .mul(Interval::point(0.25))
        .sub(Interval::point(super::super::OFFSET_TOLERANCE_MM * 0.5))
        .sub(Interval::point(uncertainty + 3.0 * FIT_ERROR_MM));
    let mut maximum = f64::INFINITY;
    let conditioning =
        |failure| super::super::native_error(failure, "source wall command-height bound");
    for polygon in &material.0 {
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            let points = &ring.0;
            if points.len() < 4 || points.first() != points.last() {
                return Err(unsupported(
                    "source wall height bound requires closed source rings",
                ));
            }
            let count = points.len() - 1;
            for index in 0..count {
                super::super::preflight::charge_capture_proposal_work(work, 1, cancel)?;
                let previous = points[(index + count - 1) % count];
                let vertex = points[index];
                let next = points[(index + 1) % count];
                if RobustKernel::orient2d(previous, vertex, next) != Orientation::CounterClockwise {
                    continue;
                }
                let at = IntervalVec2::point(vertex.x, vertex.y);
                let incoming = at.sub(IntervalVec2::point(previous.x, previous.y));
                let outgoing = IntervalVec2::point(next.x, next.y).sub(at);
                let cosine = incoming
                    .dot(outgoing)
                    .div(
                        incoming
                            .norm()
                            .map_err(conditioning)?
                            .mul(outgoing.norm().map_err(conditioning)?),
                    )
                    .map_err(conditioning)?;
                let sine = Interval::new(
                    ((1.0 + cosine.lo).next_down() * 0.5).max(0.0),
                    ((1.0 + cosine.hi).next_up() * 0.5).min(1.0),
                )
                .map_err(conditioning)?
                .sqrt()
                .map_err(conditioning)?;
                let gap = Interval::point(1.0)
                    .div(sine)
                    .map_err(conditioning)?
                    .sub(Interval::point(1.0));
                if gap.hi <= 0.0 {
                    continue;
                }
                let width = extent
                    .mul(Interval::point(2.0))
                    .div(Interval::point(gap.hi))
                    .map_err(conditioning)?;
                maximum = maximum.min((width.lo - 2.0 * uncertainty).next_down());
            }
        }
    }
    if maximum == f64::INFINITY {
        return Ok(None);
    }
    if !maximum.is_finite() || maximum <= 0.0 {
        return Err(unsupported(
            "source corners leave no positive command-admissible layer height",
        ));
    }
    Ok(Some(SourceCommandHeightLimit {
        maximum_mm: maximum,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::polygon;

    #[test]
    fn source_corner_height_limit_precedes_wall_commands_without_changing_caller_height() {
        let material = MultiPolygon(vec![polygon![
            (x: 0.0, y: 0.0), (x: 20.0, y: 0.0),
            (x: 20.0, y: 10.0), (x: 0.0, y: 10.0),
        ]]);
        let cancel = AtomicBool::new(false);
        let limit = source_command_height_limit(&material, 0.4, &mut 0, &cancel)
            .unwrap()
            .unwrap();
        assert!(0.4 < limit.maximum_mm && limit.maximum_mm < 0.5);
        let mut recipe = crate::planner::tests::recipe(1.0);
        recipe.layer_height_mm = 0.6;
        let required = region::RequiredMaterial::from_section(&material);
        assert_eq!(
            crate::planner::tests::wall_plan(&material, &required, &recipe, 5, &cancel)
                .err()
                .unwrap()
                .code,
            ManufacturingErrorCode::UnsupportedGeometry,
        );
    }

    #[test]
    fn source_corner_height_scan_preserves_work_and_cancellation_limits() {
        let material = MultiPolygon(vec![polygon![
            (x: 0.0, y: 0.0), (x: 20.0, y: 0.0),
            (x: 20.0, y: 10.0), (x: 0.0, y: 10.0),
        ]]);
        assert_eq!(
            source_command_height_limit(&material, 0.4, &mut 0, &AtomicBool::new(true))
                .err()
                .unwrap()
                .code,
            ManufacturingErrorCode::Cancelled,
        );
        let mut exhausted_work = usize::MAX;
        assert_eq!(
            source_command_height_limit(
                &material,
                0.4,
                &mut exhausted_work,
                &AtomicBool::new(false)
            )
            .err()
            .unwrap()
            .code,
            ManufacturingErrorCode::ResourceLimit,
        );
    }

    #[test]
    fn source_corner_height_limit_checks_every_interval_in_a_reused_native_span() {
        let limit = SourceCommandHeightLimit { maximum_mm: 0.45 };
        let schedule = [[0.0, 0.3], [0.3, 0.6], [0.6, 1.2]].map(|[lower_z, z]| {
            super::super::super::LayerElevation {
                lower_z,
                z,
                height: z - lower_z,
                sample: lower_z + (z - lower_z) * 0.5,
                course: 0,
            }
        });
        assert_eq!(
            limit
                .first_violating(&schedule, 0..3, &mut 0, &AtomicBool::new(false))
                .unwrap(),
            Some(2),
        );
        assert_eq!(
            limit
                .first_violating(&schedule, 0..2, &mut 0, &AtomicBool::new(false))
                .unwrap(),
            None,
        );
    }
}
