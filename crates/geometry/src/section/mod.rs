// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

mod analytic;
pub(crate) use analytic::projected_extent;
mod loops;
#[cfg(test)]
mod tests;

use crate::{Definition, GeometryError, NativeSection, PlaneFrameMm, PlaneMm, check_cancel};
use monstertruck_modeling::{
    BoundingBox, Curve, Matrix4, Point3, Solid, Surface, builder, primitive,
};
use monstertruck_solid::{BooleanWorkLimits, PreparedBooleanSource, ShapeOpsError};
use spiling_contracts::geometry::{
    GeometryErrorCode, SECTION_BOOLEAN_TOLERANCE_MM, SECTION_PLANE_TOLERANCE_MM,
    SECTION_SAMPLING_TOLERANCE_MM,
};
use std::sync::atomic::AtomicBool;

const PARALLEL_TOLERANCE: f64 = 1e-10;

/// Source cache plus derived clipping seeds across one consumer's section job.
pub const MAX_PREPARED_SECTION_BYTES: usize = 64 * 1024 * 1024;
const MAX_QUERY_SEED_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PREPARED_SECTION_SOURCE_BYTES: usize =
    MAX_PREPARED_SECTION_BYTES - MAX_QUERY_SEED_BYTES;
// Search preparation is independent of renderer/packed-display admission.
const MAX_NATIVE_SEED_VERTICES: usize = 1_000_000;
const MAX_NATIVE_SEED_TRIANGLES: usize = 1_000_000;

/// Job-local search seeds borrowing one immutable native definition.
///
/// Seeds accelerate native intersections; they never certify section geometry.
/// Dropping this value releases its buffers without retaining the definition.
pub struct PreparedSectionSource<'a> {
    definition: &'a Definition,
    native: PreparedBooleanSource<'a, Curve, Surface>,
}

impl PreparedSectionSource<'_> {
    /// Owned buffer capacity, excluding borrowed BREP and allocator bookkeeping.
    pub fn retained_bytes(&self) -> usize {
        self.native.retained_bytes()
    }

    /// Apply the same analytic, native topology and sampled-output gates.
    pub fn section_with_byte_limit(
        &self,
        plane: PlaneMm,
        cancel: &AtomicBool,
        byte_limit: u32,
    ) -> Result<NativeSection, GeometryError> {
        section_using(
            self.definition,
            plane,
            cancel,
            byte_limit,
            Some(&self.native),
        )
    }
}

/// Prepare only original-tolerance native seeds within remaining job capacity.
///
/// Native tessellation is indivisible; these admission limits are not RSS limits.
pub fn prepare_section_source<'a>(
    definition: &'a Definition,
    remaining_prepared_bytes: usize,
    cancel: &AtomicBool,
) -> Result<PreparedSectionSource<'a>, GeometryError> {
    check_cancel(cancel)?;
    if remaining_prepared_bytes == 0 {
        return Err(resource_error("prepared native section capacity exhausted"));
    }
    let limits = BooleanWorkLimits {
        max_faces: spiling_contracts::geometry::MAX_NATIVE_FACES as usize + 6,
        max_edges: 4 * spiling_contracts::geometry::MAX_STEP_ENTITIES as usize,
        max_mesh_vertices: MAX_NATIVE_SEED_VERTICES,
        max_mesh_triangles: MAX_NATIVE_SEED_TRIANGLES,
        max_triangle_pairs: 20_000_000,
        max_interference_segments: 1_000_000,
        max_output_faces: spiling_contracts::geometry::MAX_NATIVE_FACES as usize + 6,
        max_output_edges: 4 * spiling_contracts::geometry::MAX_STEP_ENTITIES as usize,
        max_prepared_bytes: MAX_PREPARED_SECTION_BYTES,
        max_source_prepared_bytes: remaining_prepared_bytes.min(MAX_PREPARED_SECTION_SOURCE_BYTES),
        max_other_prepared_bytes: MAX_QUERY_SEED_BYTES,
        max_intermediate_prepared_bytes: MAX_PREPARED_SECTION_BYTES,
    };
    let native = monstertruck_solid::prepare_boolean_source(
        &definition.solid,
        SECTION_BOOLEAN_TOLERANCE_MM,
        limits,
        cancel,
    )
    .map_err(native_error)?;
    Ok(PreparedSectionSource { definition, native })
}

pub(crate) fn section(
    definition: &Definition,
    plane: PlaneMm,
    cancel: &AtomicBool,
    byte_limit: u32,
) -> Result<NativeSection, GeometryError> {
    section_using(definition, plane, cancel, byte_limit, None)
}

fn section_using(
    definition: &Definition,
    plane: PlaneMm,
    cancel: &AtomicBool,
    byte_limit: u32,
    prepared: Option<&PreparedBooleanSource<'_, Curve, Surface>>,
) -> Result<NativeSection, GeometryError> {
    check_cancel(cancel)?;
    let mut budget = loops::SectionBudget::with_limit(byte_limit)?;
    let plane = plane.normalized()?;
    let frame = PlaneFrameMm::from_plane(plane)?;
    let mut result = NativeSection {
        plane,
        frame,
        loops: Vec::new(),
        sampling_tolerance_mm: SECTION_SAMPLING_TOLERANCE_MM,
        boolean_tolerance_mm: SECTION_BOOLEAN_TOLERANCE_MM,
    };
    if !analytic::crosses_plane(definition, &plane, cancel)? {
        return Ok(result);
    }

    // The analytic witnesses establish crossing; all certified AABB corners,
    // not merely topological vertices, establish a safe finite clipping box.
    let (min, max) = clip_bounds(definition, &frame)?;
    let clip: Solid = primitive::cuboid(BoundingBox::from_iter([
        Point3::new(min[0], min[1], min[2]),
        Point3::new(max[0], max[1], max[2]),
    ]));
    let clip = builder::transformed(&clip, frame_to_definition(&frame));
    for shell in clip.boundaries() {
        shell
            .check_solid_boundary()
            .map_err(|_| kernel_error("invalid native clipping cuboid"))?;
    }
    let mut clip_carrier = None;
    for face in clip.face_iter() {
        let Surface::Plane(carrier) = face.surface() else {
            continue;
        };
        if parallel(carrier.normal().into(), plane.normal)
            && signed_distance(carrier.origin().into(), &plane)?.abs() <= SECTION_PLANE_TOLERANCE_MM
        {
            clip_carrier = Some(carrier);
            break;
        }
    }
    let clip_carrier = clip_carrier
        .ok_or_else(|| kernel_error("native clipping cuboid has no plane-side carrier"))?;
    check_cancel(cancel)?;
    let owned;
    let source = match prepared {
        Some(source) => source,
        None => {
            owned = prepare_section_source(definition, MAX_PREPARED_SECTION_SOURCE_BYTES, cancel)?;
            &owned.native
        }
    };
    let cut = monstertruck_solid::and_prepared(source, &clip, cancel).map_err(native_error)?;
    check_cancel(cancel)?;
    for shell in cut.boundaries() {
        check_cancel(cancel)?;
        shell
            .check_solid_boundary()
            .map_err(|_| kernel_error("native section boolean returned invalid solid topology"))?;
    }

    let mut boundaries = Vec::new();
    // The native loop budget narrows to remaining job cache/output capacity.
    for face in cut.face_iter() {
        check_cancel(cancel)?;
        let Surface::Plane(carrier) = face.surface() else {
            continue;
        };
        // The pinned boolean preserves input carriers when splitting faces.
        // Match the clipping carrier exactly: coincidence tolerance alone
        // could also select a retained source face only 1e-4 mm away.
        if carrier != clip_carrier {
            continue;
        }
        let normal: [f64; 3] = carrier.normal().into();
        let origin: [f64; 3] = carrier.origin().into();
        if !parallel(normal, plane.normal)
            || signed_distance(origin, &plane)?.abs() > SECTION_PLANE_TOLERANCE_MM
        {
            continue;
        }
        // Source coplanar faces were rejected before the boolean and native
        // clipping-carrier provenance identifies these caps, not source IDs.
        // Absolute wires and the raw carrier normal share an orientation.
        // Their un-inverted signs preserve outer/hole classification even on
        // an inverted face, without cloning/inverting boundary topology.
        let face_sign = dot(normal, plane.normal);
        let mut outer_count = 0;
        for wire in face.absolute_boundaries() {
            check_cancel(cancel)?;
            let boundary = loops::sample_wire(wire, &frame, face_sign, &budget, cancel)?;
            if !boundary.is_hole() {
                outer_count += 1;
            }
            budget.push_loop(boundary.point_count())?;
            boundaries.push(boundary);
            check_cancel(cancel)?;
        }
        if outer_count == 0 {
            return Err(kernel_error("native section cap has no outer boundary"));
        }
    }
    if boundaries.is_empty() {
        return Err(kernel_error(
            "native crossing section produced no planar cap",
        ));
    }
    check_cancel(cancel)?;
    boundaries.sort_by(|a, b| loops::compare_loops(a, b, &frame));
    // Canonical ordering can change complete-loop chunk boundaries. Recheck
    // the actual sorted packing budget before returning the owned result.
    let mut sorted_budget = loops::SectionBudget::with_limit(byte_limit)?;
    for boundary in &boundaries {
        check_cancel(cancel)?;
        sorted_budget.push_loop(boundary.point_count())?;
    }
    result.loops = boundaries
        .into_iter()
        .map(loops::CanonicalLoop::into_native)
        .collect();
    Ok(result)
}

fn clip_bounds(
    definition: &Definition,
    frame: &PlaneFrameMm,
) -> Result<([f64; 3], [f64; 3]), GeometryError> {
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for mask in 0..8 {
        let corner = std::array::from_fn(|axis| {
            if mask & (1 << axis) == 0 {
                definition.bounds.min[axis]
            } else {
                definition.bounds.max[axis]
            }
        });
        let local = in_frame(corner, frame)?;
        for axis in 0..3 {
            min[axis] = min[axis].min(local[axis]);
            max[axis] = max[axis].max(local[axis]);
        }
    }
    let pad = 1.0_f64.max(100.0 * SECTION_BOOLEAN_TOLERANCE_MM);
    let min = [min[0] - pad, min[1] - pad, 0.0];
    let max = [max[0] + pad, max[1] + pad, max[2] + pad];
    if (0..3).any(|axis| !min[axis].is_finite() || !max[axis].is_finite() || min[axis] >= max[axis])
    {
        return Err(kernel_error(
            "section clipping extents are not finite and ordered",
        ));
    }
    Ok((min, max))
}

fn frame_to_definition(frame: &PlaneFrameMm) -> Matrix4 {
    let [x, y, z, o] = [frame.x_axis, frame.y_axis, frame.z_axis, frame.origin_mm];
    Matrix4::new(
        x[0], x[1], x[2], 0.0, y[0], y[1], y[2], 0.0, z[0], z[1], z[2], 0.0, o[0], o[1], o[2], 1.0,
    )
}

pub(super) fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn parallel(a: [f64; 3], b: [f64; 3]) -> bool {
    let cross = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    dot(cross, cross) <= PARALLEL_TOLERANCE * PARALLEL_TOLERANCE
}

fn signed_distance(point: [f64; 3], plane: &PlaneMm) -> Result<f64, GeometryError> {
    let value = dot(
        std::array::from_fn(|axis| point[axis] - plane.origin_mm[axis]),
        plane.normal,
    );
    if !value.is_finite() {
        return Err(kernel_error("nonfinite native section signed distance"));
    }
    Ok(value)
}

fn in_frame(point: [f64; 3], frame: &PlaneFrameMm) -> Result<[f64; 3], GeometryError> {
    let offset = std::array::from_fn(|axis| point[axis] - frame.origin_mm[axis]);
    let local = [
        dot(offset, frame.x_axis),
        dot(offset, frame.y_axis),
        dot(offset, frame.z_axis),
    ];
    if local.iter().any(|value| !value.is_finite()) {
        return Err(kernel_error(
            "nonfinite native section plane-frame coordinate",
        ));
    }
    Ok(local.map(|value| if value == 0.0 { 0.0 } else { value }))
}

fn kernel_error(message: impl AsRef<str>) -> GeometryError {
    GeometryError::new(GeometryErrorCode::KernelFailure, message)
}

fn native_error(error: ShapeOpsError) -> GeometryError {
    let code = match &error {
        ShapeOpsError::Cancelled => GeometryErrorCode::Cancelled,
        ShapeOpsError::WorkLimitExceeded { .. } => GeometryErrorCode::ResourceLimit,
        _ => GeometryErrorCode::KernelFailure,
    };
    GeometryError::new(code, format!("native section boolean failed: {error}"))
}

fn resource_error(message: &'static str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::ResourceLimit, message)
}
