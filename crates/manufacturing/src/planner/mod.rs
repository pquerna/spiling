// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

mod region;
use crate::{CompilerInput, check_cancel, error};
use geo::{Area, BooleanOps, BoundingRect, MultiPolygon};
use spiling_contracts::geometry::{
    AabbMm, FaceCarrier, GeometryErrorCode, PlaneMm, RigidPoseMm, SECTION_SAMPLING_TOLERANCE_MM,
};
use spiling_contracts::manufacturing::*;
use std::{collections::BTreeSet, sync::atomic::AtomicBool};

pub const LAYER_QUANTIZATION_MM: f64 = 0.000001;
pub const OFFSET_TOLERANCE_MM: f64 = 0.005;
const MAX_BOOLEAN_PAIRS: usize = 20_000_000;

pub(crate) fn plan(
    input: &CompilerInput<'_>,
    cancel: &AtomicBool,
) -> Result<NormalizedPrintPlan, ManufacturingError> {
    check_cancel(cancel)?;
    input.intent.validate()?;
    let bed = input.intent.printer.build_envelope.min[2];
    let height = input.intent.recipe.layer_height_mm;
    let alignment_tolerance = LAYER_QUANTIZATION_MM.min(height * 1e-6);
    let mut slab_boundaries = BTreeSet::new();
    let mut reuse_slabs = true;
    let mut bounds: Option<AabbMm> = None;
    let mut occurrences = input.occurrences.iter().collect::<Vec<_>>();
    occurrences.sort_by_key(|o| o.occurrence_id);
    for occurrence in &occurrences {
        check_cancel(cancel)?;
        occurrence
            .pose
            .validate()
            .map_err(|e| error(ManufacturingErrorCode::InvalidSpecification, e.to_string()))?;
        let definition = input
            .definitions
            .get(&occurrence.definition_id)
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::InvalidSpecification,
                    "occurrence definition is missing",
                )
            })?;
        // Vertical analytic carriers plus layer-aligned horizontal faces make
        // midpoint sampling exhaustive for admitted layer geometry, rather than
        // silently missing sub-layer shelves, sloped roofs or tilted cylinders.
        for face in definition.faces() {
            check_cancel(cancel)?;
            match &face.carrier {
                FaceCarrier::Plane { origin_mm, normal } => {
                    let n = rotate(&occurrence.pose, *normal);
                    if n[2].abs() <= 1e-10 {
                        reuse_slabs &= n[2] == 0.0;
                        continue;
                    }
                    if n[0].hypot(n[1]) > 1e-10 {
                        return Err(error(
                            ManufacturingErrorCode::UnsupportedGeometry,
                            "sloped planar carriers require unsupported within-layer/support analysis",
                        ));
                    }
                    reuse_slabs &= n[0] == 0.0 && n[1] == 0.0;
                    let elevation = forward(&occurrence.pose, *origin_mm)[2];
                    let grid = ((elevation - bed) / height).round();
                    if (bed + grid * height - elevation).abs() > alignment_tolerance {
                        return Err(error(
                            ManufacturingErrorCode::UnsupportedGeometry,
                            "horizontal native boundary is not aligned to fixed layer quantization",
                        ));
                    }
                    if grid >= 0.0 && grid <= MAX_PRINT_LAYERS as f64 {
                        slab_boundaries.insert(grid as u32);
                    }
                }
                FaceCarrier::Cylinder { axis_direction, .. } => {
                    let axis = rotate(&occurrence.pose, *axis_direction);
                    if axis[0].hypot(axis[1]) > 1e-10 {
                        return Err(error(
                            ManufacturingErrorCode::UnsupportedGeometry,
                            "nonvertical cylindrical carriers require unsupported support analysis",
                        ));
                    }
                    reuse_slabs &= axis[0] == 0.0 && axis[1] == 0.0;
                }
            }
        }
        let local = definition.bounds_mm();
        for mask in 0..8 {
            let point = forward(
                &occurrence.pose,
                std::array::from_fn(|a| {
                    if mask & (1 << a) == 0 {
                        local.min[a]
                    } else {
                        local.max[a]
                    }
                }),
            );
            let envelope = input.intent.printer.build_envelope;
            if (0..3).any(|a| {
                point[a] < envelope.min[a] - LAYER_QUANTIZATION_MM
                    || point[a] > envelope.max[a] + LAYER_QUANTIZATION_MM
            }) {
                return Err(error(
                    ManufacturingErrorCode::UnsupportedGeometry,
                    "placed native certified bounds exceed printer build envelope",
                ));
            }
            if let Some(b) = &mut bounds {
                b.include(point).map_err(|e| {
                    error(ManufacturingErrorCode::UnsupportedGeometry, e.to_string())
                })?;
            } else {
                bounds = Some(AabbMm {
                    min: point,
                    max: point,
                });
            }
        }
    }
    let bounds = bounds.ok_or_else(|| {
        error(
            ManufacturingErrorCode::EmptyProject,
            "no placed native geometry",
        )
    })?;
    if (bounds.min[2] - bed).abs() > LAYER_QUANTIZATION_MM {
        return Err(error(
            ManufacturingErrorCode::UnsupportedGeometry,
            "geometry must begin on the declared bed plane; generated supports are unavailable",
        ));
    }
    let count = ((bounds.max[2] - bed) / height).round();
    if !count.is_finite() || count < 1.0 || count > MAX_PRINT_LAYERS as f64 {
        return Err(error(
            ManufacturingErrorCode::ResourceLimit,
            "fixed layer count exceeds support limits",
        ));
    }
    if (bed + count * height - bounds.max[2]).abs() > LAYER_QUANTIZATION_MM {
        return Err(error(
            ManufacturingErrorCode::UnsupportedGeometry,
            "top height is not an integral fixed layer height; partial top layers are unsupported",
        ));
    }
    let mut layers = Vec::with_capacity(count as usize);
    let mut section = MultiPolygon(Vec::new());
    let mut templates: [Option<Vec<DepositionPath>>; 2] = [None, None];
    let mut segments = 0usize;
    for index in 0..count as u32 {
        check_cancel(cancel)?;
        let z = bed + (index as f64 + 1.0) * height;
        let sample = bed + (index as f64 + 0.5) * height;
        // Exact world-vertical carriers have invariant XY between horizontal
        // faces. Residual floating tilt admitted by carrier tolerances still
        // uses fresh native sections; it cannot silently grow a reuse error.
        if index == 0 || !reuse_slabs || slab_boundaries.contains(&index) {
            let mut next = MultiPolygon(Vec::new());
            for occurrence in &occurrences {
                check_cancel(cancel)?;
                let definition = &input.definitions[&occurrence.definition_id];
                let plane = PlaneMm {
                    origin_mm: inverse(&occurrence.pose, [0.0, 0.0, sample]),
                    normal: rotate_inverse(&occurrence.pose, [0.0, 0.0, 1.0]),
                };
                let native = spiling_geometry::section_with_byte_limit(
                    definition,
                    plane,
                    cancel,
                    4 * 1024 * 1024,
                )
                .map_err(|e| {
                    let code = match e.code {
                        GeometryErrorCode::Cancelled => ManufacturingErrorCode::Cancelled,
                        GeometryErrorCode::ResourceLimit => ManufacturingErrorCode::ResourceLimit,
                        _ => ManufacturingErrorCode::UnsupportedGeometry,
                    };
                    error(code, format!("native section at layer {index}: {e}"))
                })?;
                let placed = region::from_native(native, &occurrence.pose, sample, cancel)?;
                if placed.0.is_empty() {
                    continue;
                }
                if next.0.is_empty() {
                    next = placed;
                } else {
                    boolean_budget(&next, &placed)?;
                    next = next.union(&placed);
                }
                check_cancel(cancel)?;
                region::check_size(&next)?;
            }
            region::canonicalize(&mut next);
            if next.0.is_empty() || next.unsigned_area() <= 0.0 {
                return Err(error(
                    ManufacturingErrorCode::UnsupportedGeometry,
                    "empty aggregate layer would require unsupported suspension/support",
                ));
            }
            if !section.0.is_empty() {
                let supported =
                    region::offset(&section, 2.0 * SECTION_SAMPLING_TOLERANCE_MM, cancel)?;
                boolean_budget(&next, &supported)?;
                let unsupported_area = next.difference(&supported).unsigned_area();
                check_cancel(cancel)?;
                if unsupported_area > OFFSET_TOLERANCE_MM * OFFSET_TOLERANCE_MM {
                    return Err(error(
                        ManufacturingErrorCode::UnsupportedGeometry,
                        "section expands beyond previous layer; supports/bridging are unsupported",
                    ));
                }
            }
            section = next;
            templates = [None, None];
        }
        let parity = index as usize % 2;
        if templates[parity].is_none() {
            let mut template_segments = 0;
            templates[parity] = Some(region::paths(
                &section,
                z,
                index,
                &input.intent.recipe,
                &mut template_segments,
                cancel,
            )?);
        }
        let template = templates[parity]
            .as_ref()
            .expect("current parity template was initialized");
        let additional = template
            .iter()
            .map(|p| p.points_mm.len() - 1)
            .sum::<usize>();
        segments = segments
            .checked_add(additional)
            .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "deposition segment limit exceeded",
                )
            })?;
        let mut paths = Vec::with_capacity(template.len());
        for path in template {
            check_cancel(cancel)?;
            paths.push(DepositionPath {
                kind: path.kind,
                points_mm: path.points_mm.iter().map(|p| [p[0], p[1], z]).collect(),
            });
        }
        layers.push(PrintLayer {
            index,
            z_mm: z,
            section_z_mm: sample,
            paths,
        });
    }
    let result = NormalizedPrintPlan {
        schema_version: 1,
        layers,
        sampling_tolerance_mm: SECTION_SAMPLING_TOLERANCE_MM,
        offset_tolerance_mm: OFFSET_TOLERANCE_MM,
        layer_quantization_mm: LAYER_QUANTIZATION_MM,
    };
    result.validate(input.intent)?;
    Ok(result)
}

pub(super) fn boolean_budget(
    a: &MultiPolygon<f64>,
    b: &MultiPolygon<f64>,
) -> Result<(), ManufacturingError> {
    let mut pairs = 0usize;
    for left in &a.0 {
        let Some(l) = left.bounding_rect() else {
            continue;
        };
        let left_points =
            left.exterior().0.len() + left.interiors().iter().map(|r| r.0.len()).sum::<usize>();
        for right in &b.0 {
            let Some(r) = right.bounding_rect() else {
                continue;
            };
            if l.min().x > r.max().x
                || r.min().x > l.max().x
                || l.min().y > r.max().y
                || r.min().y > l.max().y
            {
                continue;
            }
            let right_points = right.exterior().0.len()
                + right.interiors().iter().map(|r| r.0.len()).sum::<usize>();
            pairs = pairs
                .checked_add(left_points.checked_mul(right_points).ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "Boolean work overflow",
                    )
                })?)
                .filter(|n| *n <= MAX_BOOLEAN_PAIRS)
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "planar Boolean work budget exceeded",
                    )
                })?;
        }
    }
    Ok(())
}

pub(super) fn rotate(pose: &RigidPoseMm, p: [f64; 3]) -> [f64; 3] {
    let [x, y, z, w] = pose.rotation_xyzw;
    let t = [
        2.0 * (y * p[2] - z * p[1]),
        2.0 * (z * p[0] - x * p[2]),
        2.0 * (x * p[1] - y * p[0]),
    ];
    [
        p[0] + w * t[0] + y * t[2] - z * t[1],
        p[1] + w * t[1] + z * t[0] - x * t[2],
        p[2] + w * t[2] + x * t[1] - y * t[0],
    ]
}
pub(super) fn forward(pose: &RigidPoseMm, p: [f64; 3]) -> [f64; 3] {
    let r = rotate(pose, p);
    std::array::from_fn(|a| r[a] + pose.translation_mm[a])
}
fn rotate_inverse(pose: &RigidPoseMm, p: [f64; 3]) -> [f64; 3] {
    let [x, y, z, w] = pose.rotation_xyzw;
    rotate(
        &RigidPoseMm {
            translation_mm: [0.0; 3],
            rotation_xyzw: [-x, -y, -z, w],
        },
        p,
    )
}
fn inverse(pose: &RigidPoseMm, p: [f64; 3]) -> [f64; 3] {
    rotate_inverse(pose, std::array::from_fn(|a| p[a] - pose.translation_mm[a]))
}
