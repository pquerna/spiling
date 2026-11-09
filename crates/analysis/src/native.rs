// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use serde::{Deserialize, Serialize};
use spiling_contracts::geometry::{AnalyticSurfaceDescription, FaceId, OccurrenceId, RigidPoseMm};
use spiling_geometry::{Definition, GeometryWorkBudget, PlacedSlabRequest, SectionLoopMm};
use std::sync::atomic::AtomicBool;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub ends: [[f64; 2]; 2],
    pub face: FaceId,
}
#[derive(Debug, Clone)]
pub struct NativePrism {
    pub(crate) loops: Vec<Vec<[f64; 2]>>,
    pub(crate) edges: Vec<Edge>,
    pub(crate) height: [f64; 2],
    pub(crate) shape_error_mm: f64,
    pub(crate) native_sample_hash: [u8; 32],
}

pub fn capture(
    definition: &Definition,
    occurrence: OccurrenceId,
    cancel: &AtomicBool,
) -> Result<NativePrism, String> {
    let mut work = GeometryWorkBudget::query();
    let mut endpoints = Vec::new();
    let mut rectangles = std::collections::BTreeMap::new();
    for face in definition.faces() {
        let Some(AnalyticSurfaceDescription::Plane { origin_mm, normal }) = face.geometry.analytic
        else {
            return Err(
                "UnsupportedModel: orthogonal whole-prism candidate requires native planar faces"
                    .into(),
            );
        };
        if normal.iter().filter(|value| value.abs() > 1e-10).count() != 1 {
            return Err("UnsupportedModel: non-orthogonal native plane".into());
        }
        if normal[2].abs() > 0.99 {
            endpoints.push(origin_mm[2]);
        } else {
            let rectangle = definition
                .vertical_rectangular_face_with_work(&face.face_id, &mut work, cancel)
                .map_err(|error| {
                    format!("UnmappedLoad: complete native side-face trim: {error:?}")
                })?;
            rectangles.insert(face.face_id.clone(), rectangle);
        }
    }
    endpoints.sort_by(f64::total_cmp);
    endpoints.dedup();
    if endpoints.len() != 2 {
        return Err(
            "UnsupportedModel: whole source has other than two candidate cap levels".into(),
        );
    }
    if rectangles.values().any(|rectangle| {
        (rectangle.min_z_mm - endpoints[0]).abs() > 1e-7
            || (rectangle.max_z_mm - endpoints[1]).abs() > 1e-7
    }) {
        return Err(
            "UnmappedLoad: complete native side-face trim does not span both source cap levels"
                .into(),
        );
    }
    let source = spiling_geometry::prepare_solid(definition, 32 * 1024 * 1024, &mut work, cancel)
        .map_err(|error| format!("native preparation: {error:?}"))?;
    let slab = source
        .analyze_placed_slab(
            &PlacedSlabRequest {
                pose: RigidPoseMm::IDENTITY,
                world_height_interval_mm: [endpoints[0], endpoints[1]],
                sample_world_z_mm: (endpoints[0] + endpoints[1]) * 0.5,
                native_shape_error_mm: 0.001,
            },
            32 * 1024 * 1024,
            &mut work,
            cancel,
        )
        .map_err(|error| format!("complete native slab: {error:?}"))?;
    let used = slab
        .section()
        .loops()
        .iter()
        .map(|boundary| SectionLoopMm {
            is_hole: boundary.is_hole,
            points_mm: boundary.points_mm.clone(),
        })
        .collect::<Vec<_>>();
    let leaf = spiling_geometry::certify_native_source_leaf(
        &spiling_geometry::SourceLeafRequest {
            source: &source,
            occurrence_id: &occurrence,
            pose: RigidPoseMm::IDENTITY,
            analysis: &slab,
            actual_used_source_loops: &used,
            policy: spiling_geometry::NativeUnionPolicy {
                candidate_coordinate_error_mm: 1e-7,
            },
        },
        &mut work,
        cancel,
    )
    .map_err(|error| format!("whole closed source leaf: {error:?}"))?;
    let shape_error_mm = leaf
        .closed_shape_error_mm()
        .filter(|error| *error <= 0.001)
        .ok_or("UnsupportedModel: whole closed source receipt unavailable")?;
    if !leaf.is_positive() {
        return Err("UnsupportedModel: empty native material".into());
    }
    let mut loops = Vec::new();
    let mut edges = Vec::new();
    for (index, boundary) in used.iter().enumerate() {
        let spans = slab
            .section()
            .spans(index)
            .ok_or("UnmappedLoad: absent native section spans")?;
        if boundary.points_mm.len() != spans.len() + 1 || spans.len() < 4 {
            return Err("UnsupportedModel: section chords do not map one-to-one to complete native straight spans".into());
        }
        let points = boundary
            .points_mm
            .iter()
            .map(|point| [point[0], point[1]])
            .collect::<Vec<_>>();
        let mut native_bounds = Vec::new();
        for span in spans {
            let spiling_geometry::SectionCurve::SurfacePlaneGraph {
                independent_axis,
                independent,
                dependent,
            } = span.curve
            else {
                return Err(
                    "UnsupportedModel: side ownership needs a complete native graph interval"
                        .into(),
                );
            };
            let mut uv = [[0.0; 2]; 2];
            uv[independent_axis as usize] = independent;
            uv[1 - independent_axis as usize] = dependent;
            let surface = definition
                .face_surface(span.face)
                .ok_or("UnmappedLoad: absent native span surface")?;
            let bounds = spiling_geometry::parametric::evaluate_analytic_surface_jet(
                surface, uv, &mut work, cancel,
            )
            .map_err(|error| format!("native full-span enclosure: {error:?}"))?;
            let scale = definition.source_units_scale_to_mm();
            native_bounds.push(
                bounds
                    .position
                    .map(|interval| [interval.lo * scale, interval.hi * scale]),
            );
        }
        let mut used_spans = std::collections::BTreeSet::new();
        for pair in points.windows(2) {
            let matches = native_bounds
                .iter()
                .enumerate()
                .filter(|(_, bounds)| {
                    (0..2).all(|axis| {
                        (bounds[axis][0] - pair[0][axis].min(pair[1][axis])).abs() < 1e-7
                            && (bounds[axis][1] - pair[0][axis].max(pair[1][axis])).abs() < 1e-7
                    })
                })
                .map(|(index, _)| index)
                .collect::<Vec<_>>();
            if matches.len() != 1 || !used_spans.insert(matches[0]) {
                return Err(
                    "UnmappedLoad: polygon chord lacks one complete native graph enclosure owner"
                        .into(),
                );
            }
            let span = &spans[matches[0]];
            let face = definition
                .faces()
                .get(span.source_face_ordinal)
                .ok_or("UnmappedLoad: native span face ordinal")?;
            let changed = (0..2)
                .filter(|axis| (pair[1][*axis] - pair[0][*axis]).abs() > 1e-8)
                .count();
            if changed != 1 || span.maximum_chord_error_mm > 1e-7 {
                return Err(
                    "UnsupportedModel: native boundary is not a certified orthogonal straight edge"
                        .into(),
                );
            }
            if definition.face_ids().get(span.source_face_ordinal) != Some(&span.face) {
                return Err("UnmappedLoad: native span face identity differs".into());
            }
            let rectangle = rectangles
                .get(&face.face_id)
                .ok_or("UnmappedLoad: complete native side-face trim is absent")?;
            let ends: [[f64; 2]; 2] = [
                [rectangle.origin_mm[0], rectangle.origin_mm[1]],
                std::array::from_fn(|axis| {
                    rectangle.origin_mm[axis]
                        + rectangle.width_mm * rectangle.horizontal_direction[axis]
                }),
            ];
            if (rectangle.min_z_mm - endpoints[0]).abs() > 1e-7
                || (rectangle.max_z_mm - endpoints[1]).abs() > 1e-7
                || !(0..2).any(|start| {
                    (0..2).all(|endpoint| {
                        (0..2).all(|axis| {
                            (pair[endpoint][axis] - ends[(start + endpoint) % 2][axis]).abs()
                                <= 1e-7
                        })
                    })
                })
            {
                return Err(
                    "UnmappedLoad: native trimmed face does not own the complete extruded side"
                        .into(),
                );
            }
            edges.push(Edge {
                ends: [pair[0], pair[1]],
                face: face.face_id.clone(),
            });
        }
        loops.push(points);
    }
    Ok(NativePrism {
        loops,
        edges,
        height: [endpoints[0], endpoints[1]],
        shape_error_mm,
        native_sample_hash: *leaf.native_sample_sha256(),
    })
}

impl NativePrism {
    pub fn loops(&self) -> &[Vec<[f64; 2]>] {
        &self.loops
    }
    pub fn edges(&self) -> &[Edge] {
        &self.edges
    }
    pub fn contains(&self, point: [f64; 3]) -> bool {
        if point[2] < self.height[0] || point[2] > self.height[1] {
            return false;
        }
        let mut inside = false;
        for boundary in &self.loops {
            for edge in boundary.windows(2) {
                let [first, second] = [edge[0], edge[1]];
                if (first[1] > point[1]) != (second[1] > point[1])
                    && point[0]
                        < (second[0] - first[0]) * (point[1] - first[1]) / (second[1] - first[1])
                            + first[0]
                {
                    inside = !inside;
                }
            }
        }
        inside
    }
}
