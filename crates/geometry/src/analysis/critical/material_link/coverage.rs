// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;

pub(in crate::analysis::critical) const MAX_CORRECTION_BYTES: usize = 4 * 1024 * 1024;

pub(in crate::analysis::critical) fn native_face_point_is_retained(
    source: MaterialSourceView<'_, '_>,
    domain: &PreparedFaceTrimDomain<'_>,
    face: usize,
    uv: [[f64; 2]; 2],
    remaining: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    if !std::ptr::eq(domain.definition(), source.definition())
        || !domain.is_native_material()
        || !std::ptr::eq(source.face_domain(face, remaining, work, cancel)?, domain)
    {
        return Err(GeometryError::new(
            GeometryErrorCode::InvalidGeometry,
            "native point belongs to another material face owner",
        ));
    }
    Ok(domain.classify_with_byte_limit(uv, remaining, work, cancel)? == TrimClassification::Inside)
}

fn native_vertex_support(point: [f64; 3], tolerance: f64) -> [Interval; 3] {
    point.map(|value| {
        Interval::point(value).add(Interval {
            lo: -tolerance,
            hi: tolerance,
        })
    })
}

fn fiber_misses_support(fiber: &fiber::NativeFiber, support: [Interval; 3]) -> bool {
    let relative =
        std::array::from_fn(|axis| support[axis].sub(Interval::point(fiber.center[axis])));
    fiber
        .rows
        .iter()
        .zip(fiber.offsets)
        .any(|(row, offset)| !dot(*row, relative).add(offset).contains_zero())
}

/// A raw-face census has no native vertex coincidence-zone authority. Exclude
/// every complete native vertex support from this EXACT candidate fiber family
/// before using that census as material degree. No prism or source transport is
/// constructed here; endpoint-germ qualification is a separate obligation.
pub(super) fn native_fiber_misses_vertex_supports(
    source: MaterialSourceView<'_, '_>,
    incidence: &SourceEdgeIncidence<'_>,
    fiber: &fiber::NativeFiber,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    let vertices = incidence.vertices().ok_or_else(|| {
        GeometryError::new(
            GeometryErrorCode::InvalidGeometry,
            "native fiber exclusion requires complete vertex incidence",
        )
    })?;
    let tolerance = source.trim_source().effective_tolerance_mm();
    for vertex in vertices {
        work.charge(1, cancel)?;
        let point = source
            .trim_source()
            .native_vertex_point(vertex, work, cancel)?;
        if !fiber_misses_support(fiber, native_vertex_support(point, tolerance)) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Actual native vertex tolerance zones. They are not added material, changed
/// source support, or an original-STEP displacement law. Calibration fibers
/// must miss these ambiguous zero-dimensional coincidence zones.
pub(in crate::analysis::critical) struct ClosedRadialLinks<'d> {
    definition: &'d Definition,
    supports: Vec<[Interval; 3]>,
    pub effective_tolerance_mm: f64,
    all_vertices_outside: bool,
}
impl<'d> ClosedRadialLinks<'d> {
    pub fn new(
        source: MaterialSourceView<'_, 'd>,
        incidence: &SourceEdgeIncidence<'_>,
        prism: &SamplePrism,
        request: &SlabRequest,
        remaining: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        let mut result = Self {
            definition: source.definition(),
            supports: Vec::new(),
            effective_tolerance_mm: source.trim_source().effective_tolerance_mm(),
            all_vertices_outside: true,
        };
        let vertices = incidence.vertices().ok_or_else(|| {
            GeometryError::new(
                GeometryErrorCode::InvalidGeometry,
                "native radial links require complete vertex incidence",
            )
        })?;
        for vertex in vertices {
            work.charge(1, cancel)?;
            let point = source
                .trim_source()
                .native_vertex_point(vertex, work, cancel)?;
            let support = native_vertex_support(point, result.effective_tolerance_mm);
            if !incidence.vertex_link_is_cycle(source.definition(), vertex, work, cancel)? {
                return Err(GeometryError::new(
                    GeometryErrorCode::AnalysisIndeterminate,
                    "native material vertex link is not one cycle",
                ));
            }
            if overlaps(
                prism.source_height.height(support),
                request.height_interval_mm,
            ) {
                result.all_vertices_outside = false;
                let Some((radial, _)) = prism.radial_projection_frame(support)? else {
                    return Err(GeometryError::new(
                        GeometryErrorCode::AnalysisIndeterminate,
                        "native vertex radial map is singular",
                    ));
                };
                let rows = projection_rows(radial)?;
                if !vertex_fan::qualify_native_vertex(
                    source,
                    incidence,
                    vertex,
                    rows,
                    remaining
                        .checked_sub(result.allocated_bytes())
                        .ok_or_else(resource)?,
                    work,
                    cancel,
                )? {
                    return Err(GeometryError::new(
                        GeometryErrorCode::AnalysisIndeterminate,
                        "native vertex radial fan has no complete degree-one projection",
                    ));
                }
                if prism.boundary_distance(support, request.native_shape_error_mm, work, cancel)?
                    > request.native_shape_error_mm
                {
                    return Err(GeometryError::new(
                        GeometryErrorCode::ShapeErrorExceeded,
                        "complete native vertex zone exceeds candidate-prism shape policy",
                    ));
                }
            }
            result.record_support(support, remaining)?;
        }
        Ok(result)
    }
    pub fn all_vertex_supports_outside_closed_slab(&self) -> bool {
        self.all_vertices_outside
    }
    pub fn vertex_supports(&self) -> impl Iterator<Item = &[Interval; 3]> {
        self.supports.iter()
    }
    pub(super) fn allocated_bytes(&self) -> usize {
        self.supports
            .capacity()
            .saturating_mul(std::mem::size_of::<[Interval; 3]>())
    }
    fn record_support(
        &mut self,
        position: [Interval; 3],
        remaining: usize,
    ) -> Result<(), GeometryError> {
        if self.supports.len() == self.supports.capacity() {
            let capacity = self.supports.capacity().saturating_mul(2).max(16);
            if capacity.saturating_mul(std::mem::size_of::<[Interval; 3]>())
                > remaining.min(MAX_CORRECTION_BYTES)
            {
                return Err(resource());
            }
            self.supports
                .try_reserve_exact(capacity - self.supports.len())
                .map_err(|_| resource())?;
            if self.allocated_bytes() > remaining.min(MAX_CORRECTION_BYTES) {
                return Err(resource());
            }
        }
        self.supports.push(position);
        Ok(())
    }
    pub fn record(
        &mut self,
        cell: &MaterialLinkCell,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        work.charge(1, cancel)?;
        self.effective_tolerance_mm = self.effective_tolerance_mm.max(cell.effective_tolerance_mm);
        Ok(())
    }
    pub fn degree_one(
        &self,
        source: MaterialSourceView<'_, '_>,
        prism: &SamplePrism,
        request: &SlabRequest,
        remaining: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        if !std::ptr::eq(self.definition, source.definition())
            || prism.frame.z_axis != request.frame.z_axis
        {
            return Err(GeometryError::new(
                GeometryErrorCode::InvalidGeometry,
                "native radial calibration changed its material/height owner",
            ));
        }
        let center_height = prism
            .source_height
            .height(prism.center.map(Interval::point));
        let margin = down(
            (center_height.lo - request.height_interval_mm[0])
                .min(request.height_interval_mm[1] - center_height.hi),
        );
        if margin <= 0.0 {
            return Err(GeometryError::new(
                GeometryErrorCode::AnalysisIndeterminate,
                "native radial calibration meets a closed cap",
            ));
        }
        let source_box =
            super::super::super::placed::material_position_bounds(source, remaining, work, cancel)?;
        let p = project(source_box, prism.frame);
        let extent =
            up(length([p[0], p[1], Interval::point(0.0)])?.hi + prism.core_radius.unwrap_or(0.0));
        let height_rows = prism.dual.map(|column| prism.source_height.dot(column));
        let meridian_extent = mul(
            Interval::point(height_rows[2].abs_upper()),
            Interval::point(extent),
        )
        .hi;
        let [x, y, z] = [prism.frame.x_axis, prism.frame.y_axis, prism.frame.z_axis]
            .map(|r| r.map(Interval::point));
        for alpha in [0.0, 1.0, -1.0, 0.5, -0.5, 2.0, -2.0, 0.2718281828459045] {
            let a = Interval::point(alpha);
            let angular = std::array::from_fn(|axis| y[axis].sub(a.mul(x[axis])));
            let normalization = add(Interval::point(1.0), square(a)).sqrt()?;
            let drift = mul(
                Interval::point(add(height_rows[0], a.mul(height_rows[1])).abs_upper()),
                Interval::point(p[0].abs_upper()),
            )
            .hi;
            let available_margin = Interval::point(margin).sub(Interval::point(drift)).lo;
            if available_margin <= 0.0 || meridian_extent <= 0.0 {
                continue;
            }
            let beta_bound = Interval::point(available_margin)
                .div(Interval::point(meridian_extent))?
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
                if add(
                    Interval::point(drift),
                    mul(
                        Interval::point(beta.abs()),
                        Interval::point(meridian_extent),
                    ),
                )
                .hi >= margin
                {
                    continue;
                }
                let rows = [
                    angular,
                    std::array::from_fn(|axis| {
                        z[axis].sub(mul(Interval::point(beta), normalization).mul(x[axis]))
                    }),
                ];
                let forward = cross(rows[0], rows[1]);
                if length(forward)?.lo <= 0.0 {
                    continue;
                }
                let fiber = fiber::NativeFiber {
                    center: prism.center,
                    rows,
                    offsets: [
                        Interval::point(0.0),
                        Interval::point(prism.core_radius.unwrap_or(0.0))
                            .mul(Interval::point(beta)),
                    ],
                    forward,
                    forward_offset: Interval::point(0.0),
                    min_forward: 0.0,
                    core: prism.core_radius.map(|radius| fiber::FiberCore {
                        frame: prism.frame,
                        radius,
                    }),
                };
                if self.misses_all_supports(&fiber, work, cancel)?
                    && fiber::native_fiber_degree_one(
                        source,
                        &fiber,
                        request,
                        prism.source_height,
                        remaining,
                        work,
                        cancel,
                    )?
                    .is_some()
                {
                    return Ok(());
                }
            }
        }
        Err(GeometryError::new(
            GeometryErrorCode::AnalysisIndeterminate,
            "native closed radial boundary has no complete degree-one regular fiber",
        ))
    }
    pub(super) fn misses_all_supports(
        &self,
        fiber: &fiber::NativeFiber,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<bool, GeometryError> {
        for support in &self.supports {
            work.charge(1, cancel)?;
            if !fiber_misses_support(fiber, *support) {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

pub(super) fn projection_rows(radial: [Interval; 3]) -> Result<[[Interval; 3]; 2], GeometryError> {
    let axis = (0..3)
        .min_by(|&a, &b| radial[a].abs_upper().total_cmp(&radial[b].abs_upper()))
        .unwrap_or(0);
    let basis = std::array::from_fn(|i| Interval::point(if i == axis { 1.0 } else { 0.0 }));
    let first = cross(radial, basis);
    let second = cross(radial, first);
    if length(first)?.lo <= 0.0 || length(second)?.lo <= 0.0 {
        return Err(GeometryError::new(
            GeometryErrorCode::AnalysisIndeterminate,
            "native radial projection has no separated transverse rows",
        ));
    }
    Ok([first, second])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calibration_fiber() -> fiber::NativeFiber {
        let rows = [[0.0, 1.0, 0.0], [0.0, 0.0, 1.0]].map(|row| row.map(Interval::point));
        fiber::NativeFiber {
            center: [0.0; 3],
            rows,
            offsets: [Interval::point(0.0); 2],
            forward: cross(rows[0], rows[1]),
            forward_offset: Interval::point(0.0),
            min_forward: 0.0,
            core: None,
        }
    }

    #[test]
    fn calibration_fiber_must_miss_the_whole_native_vertex_zone() {
        let fiber = calibration_fiber();
        let point = [1.0, 1e-6, 1e-6];
        assert!(
            fiber_misses_support(&fiber, point.map(Interval::point)),
            "the literal vertex misses this fiber"
        );
        assert!(
            !fiber_misses_support(&fiber, native_vertex_support(point, 2e-6)),
            "missing the literal vertex does not exclude its native tolerance zone"
        );
        assert!(
            fiber_misses_support(&fiber, native_vertex_support([1.0, 3e-6, 0.0], 2e-6)),
            "one strictly separated affine equation excludes the entire zone"
        );
    }

    #[test]
    fn native_zone_exclusion_uses_the_exact_offset_and_row_families() {
        let mut fiber = calibration_fiber();
        fiber.center = [10.0, 20.0, 30.0];
        fiber.offsets = [
            Interval { lo: -0.3, hi: -0.1 },
            Interval { lo: -0.6, hi: -0.4 },
        ];
        let support = native_vertex_support([11.0, 20.25, 30.5], 2e-6);
        assert!(!fiber_misses_support(&fiber, support));
        let midpoint = fiber::NativeFiber {
            offsets: fiber.offsets.map(|offset| Interval::point(offset.mid())),
            ..fiber
        };
        assert!(
            fiber_misses_support(&midpoint, support),
            "a disjoint midpoint fiber cannot certify its whole offset family"
        );
        let mut fiber = calibration_fiber();
        fiber.rows[0] = [
            Interval { lo: 0.0, hi: 1.0 },
            Interval::point(0.0),
            Interval::point(0.0),
        ];
        let support = native_vertex_support([1.0, 0.0, 0.0], 2e-6);
        assert!(!fiber_misses_support(&fiber, support));
        fiber.rows[0][0] = Interval::point(0.5);
        assert!(
            fiber_misses_support(&fiber, support),
            "a disjoint midpoint covector cannot certify its whole row family"
        );
    }
}
