// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Narrow whole-boundary recognition, sharing the cylinder physical trim proof.
//! No section, display mesh, Boolean, source label or filename grants authority.

use super::Definition;
use crate::{BoredPlate, GeometryError, GeometryWorkBudget, check_cancel};
use solidcore::nurbs::b_spline_surface_n_lib::p2::OrientType;
use solidcore::surface::surface::p2::SurfaceKind;
use solidcore::topology::{EdgeuseOwner, LoopuseStart};
use spiling_contracts::geometry::{AnalyticSurfaceDescription, GeometryErrorCode};
use std::{collections::HashMap, sync::atomic::AtomicBool};

const MM: f64 = 1e-7;
const ANGULAR: f64 = 1e-10;
fn unsupported(message: &str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::UnsupportedGeometry, message)
}
fn malformed(message: &str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::KernelFailure, message)
}
fn native<T>(r: solidcore::error::Result<T>) -> Result<T, GeometryError> {
    r.map_err(|_| malformed("invalid native bored-plate incidence"))
}
fn orientation(o: OrientType) -> Result<bool, GeometryError> {
    match o {
        OrientType::Same => Ok(true),
        OrientType::Opposite => Ok(false),
        _ => Err(malformed("unoriented native bored-plate use")),
    }
}
fn unit(a: [f64; 3]) -> Result<[f64; 3], GeometryError> {
    let r = a[0].hypot(a[1]).hypot(a[2]);
    if !r.is_finite() || r == 0.0 {
        return Err(malformed("invalid bored-plate plane direction"));
    }
    Ok(a.map(|x| x / r))
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| a[i] * b[i]).sum()
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

// Boundary role, independent of STEP/native face ordering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    Bore,
    Cap(usize),
    Outer(usize),
}
#[derive(Clone, Copy)]
struct Use {
    face: usize,
    role: Role,
    forward: bool,
}

impl Definition {
    pub fn bored_plate(&self) -> Result<BoredPlate, GeometryError> {
        self.bored_plate_with_work(&mut GeometryWorkBudget::query(), &AtomicBool::new(false))
    }

    /// Certify the complete boundary of one source-Z rectangular plate with one
    /// circular through bore. Inward split faces are combined by shared native
    /// seam incidence, never by geometric proximity alone. Separate closed-slab
    /// and source-leaf qualification remains the consumer's obligation.
    pub fn bored_plate_with_work(
        &self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<BoredPlate, GeometryError> {
        check_cancel(cancel)?;
        // Conservative coexistence bound for hash maps, cycle inventories and
        // angular/face output. Native admission separately bounds its arenas.
        self.brep()
            .edgeuses()
            .len()
            .checked_mul(512)
            .and_then(|n| n.checked_add(self.retained_query_proof_bytes()))
            .filter(|&n| n <= crate::analysis::MAX_SCRATCH_ALLOCATION_BYTES)
            .ok_or_else(|| {
                GeometryError::new(
                    GeometryErrorCode::ResourceLimit,
                    "bored-plate incidence and retained input exceed the native scratch cap",
                )
            })?;
        let mut bore_faces = Vec::new();
        for info in self.faces() {
            work.charge(1, cancel)?;
            if matches!(
                info.geometry.analytic,
                Some(AnalyticSurfaceDescription::Cylinder { .. })
            ) {
                bore_faces.push(info.face_id.clone());
            }
        }
        // Every cylinder belongs to this one bore; stepped/second holes cannot
        // be dropped from the census. Coefficient disagreement is terminal.
        let (bore, bore_face_angles) =
            self.cylinder_faces_with_work(&bore_faces, true, work, cancel)?;
        let height = [bore.min_z_mm, bore.max_z_mm];
        let x = super::finish_support_extent(self, [1.0, 0.0, 0.0], work, cancel)?;
        let y = super::finish_support_extent(self, [0.0, 1.0, 0.0], work, cancel)?;
        let xy = [[x[0], y[0]], [x[1], y[1]]];
        for axis in [0, 1] {
            if !xy[0][axis].is_finite()
                || !xy[1][axis].is_finite()
                || bore.center_mm[axis] - bore.radius_mm <= xy[0][axis] + MM
                || bore.center_mm[axis] + bore.radius_mm >= xy[1][axis] - MM
            {
                return Err(unsupported(
                    "bore must lie strictly inside the rectangular plate",
                ));
            }
        }
        let brep = self.brep();
        let mut caps = [None; 2];
        let mut outer = [None; 4];
        let mut roles = Vec::with_capacity(self.faces().len());
        for (index, info) in self.faces().iter().enumerate() {
            work.charge(1, cancel)?;
            if bore_faces.contains(&info.face_id) {
                roles.push(Role::Bore);
                continue;
            }
            let id = self.face_ids()[index];
            let original = self.original_source().face(id)?;
            let solidcraft_step::geom::Surface::Plane { position: source } =
                original.carrier.as_ref()
            else {
                return Err(unsupported(
                    "bored plate has an additional nonplanar boundary",
                ));
            };
            if original.material_orientation() != info.orientation
                || info.surface_face_entity.parse::<u64>().ok() != Some(original.source_face.0)
            {
                return Err(malformed(
                    "bored-plate plane source identity or orientation disagrees",
                ));
            }
            let source_normal = unit(source.axis)?.map(|x| if info.orientation { x } else { -x });
            let Some(AnalyticSurfaceDescription::Plane { origin_mm, normal }) =
                info.geometry.analytic
            else {
                return Err(unsupported(
                    "bored plate requires analytic planar outer and cap faces",
                ));
            };
            let admitted_normal = unit(normal)?.map(|x| if info.orientation { x } else { -x });
            if dot(source_normal, admitted_normal) < 1.0 - ANGULAR {
                return Err(malformed(
                    "bored-plate admitted/source plane coefficients disagree",
                ));
            }
            let face = native(brep.face(id))?;
            let fu = native(
                brep.faceuse(
                    face.faceuse
                        .ok_or_else(|| malformed("missing plate faceuse"))?,
                ),
            )?;
            if !orientation(fu.orientation)? {
                return Err(malformed("plate chart owner is not the SAME use"));
            }
            let forward =
                crate::analysis::source_incidence::face_orientation(self, index, work, cancel)?;
            let SurfaceKind::Plane(plane) = self
                .face_surface(id)
                .ok_or_else(|| malformed("missing plate surface"))?
                .kind()
            else {
                return Err(malformed(
                    "native plate plane differs from original carrier",
                ));
            };
            let p = plane.position();
            let o = p.origin();
            let a = p.x_axis();
            let b = p.y_axis();
            let scale = plane.uv_scale();
            if ![scale.x, scale.y]
                .iter()
                .all(|x| x.is_finite() && *x != 0.0)
            {
                return Err(malformed("invalid native cap plane parameter scales"));
            }
            let n = unit(cross([a.x, a.y, a.z], [b.x, b.y, b.z]))?.map(|x| {
                x * if scale.x.signum() == scale.y.signum() {
                    1.0
                } else {
                    -1.0
                } * if forward { 1.0 } else { -1.0 }
            });
            let native_origin = [o.x, o.y, o.z];
            for origin in [source.origin, origin_mm, native_origin] {
                if !origin.iter().all(|x| x.is_finite())
                    || dot(
                        std::array::from_fn(|i| origin[i] - source.origin[i]),
                        source_normal,
                    )
                    .abs()
                        > MM
                {
                    return Err(malformed("bored-plate plane positions disagree"));
                }
            }
            if dot(n, source_normal) < 1.0 - ANGULAR {
                return Err(malformed(
                    "native/source bored-plate material normal disagrees",
                ));
            }
            if n[0].abs() <= ANGULAR && n[1].abs() <= ANGULAR {
                let side = if n[2] < 0.0 { 0 } else { 1 };
                if (native_origin[2] - height[side]).abs() > MM
                    || caps[side].replace(index).is_some()
                {
                    return Err(unsupported(
                        "plate requires exactly one outward cap at each bore endpoint",
                    ));
                }
                roles.push(Role::Cap(side));
            } else {
                let wall = self.vertical_wall_face_with_work(&info.face_id, work, cancel)?;
                let axis = if wall.outward_normal[0].abs() >= 1.0 - ANGULAR {
                    0
                } else if wall.outward_normal[1].abs() >= 1.0 - ANGULAR {
                    1
                } else {
                    return Err(unsupported("plate exterior is not source-XY rectangular"));
                };
                let side = usize::from(wall.outward_normal[axis] > 0.0);
                let slot = 2 * axis + side;
                let other = 1 - axis;
                if (wall.origin_mm[axis] - xy[side][axis]).abs() > MM
                    || (wall.origin_mm[other] - xy[0][other]).abs() > MM
                    || (wall.width_mm - (xy[1][other] - xy[0][other])).abs() > MM
                    || (wall.min_z_mm - height[0]).abs() > MM
                    || (wall.max_z_mm - height[1]).abs() > MM
                    || outer[slot].replace(index).is_some()
                {
                    return Err(unsupported(
                        "plate outer rectangles do not cover the same complete box",
                    ));
                }
                roles.push(Role::Outer(slot));
            }
        }
        let caps = caps
            .map(|x| x.ok_or_else(|| unsupported("bored plate is missing a cap")))
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        let outer = outer
            .map(|x| x.ok_or_else(|| unsupported("bored plate is missing an outer rectangle")))
            .into_iter()
            .collect::<Result<Vec<_>, _>>()?;
        if roles.len() != bore_faces.len() + 6 {
            return Err(unsupported("bored plate has additional boundary faces"));
        }
        let mut census: HashMap<solidcore::EdgeId, Vec<Use>> = HashMap::new();
        let mut cap_cycles = [Vec::new(), Vec::new()];
        // Loopuse/Edgeuse cycles, never Loop.edges, supply actual ownership.
        for (index, &role) in roles.iter().enumerate() {
            work.charge(1, cancel)?;
            let face = native(brep.face(self.face_ids()[index]))?;
            let fu_id = face
                .faceuse
                .ok_or_else(|| malformed("missing plate faceuse"))?;
            let fu = native(brep.faceuse(fu_id))?;
            if !orientation(fu.orientation)? {
                return Err(malformed("plate chart owner is not the SAME use"));
            }
            let face_forward =
                crate::analysis::source_incidence::face_orientation(self, index, work, cancel)?;
            if face.loops.is_empty() || face.loops.len() != fu.loopuses.len() {
                return Err(malformed("plate loop/faceuse census disagrees"));
            }
            for (&loop_id, &lu_id) in face.loops.iter().zip(&fu.loopuses) {
                work.charge(1, cancel)?;
                let lu = native(brep.loopuse(lu_id))?;
                if lu.loop_ != Some(loop_id) || lu.faceuse != Some(fu_id) {
                    return Err(malformed("foreign plate loopuse"));
                }
                let LoopuseStart::Edgeuse(Some(first)) = lu.start else {
                    return Err(unsupported("plate contains an empty trim cycle"));
                };
                let mut current = first;
                let mut cycle = Vec::new();
                for count in 0..brep.edgeuses().len() {
                    work.charge(1, cancel)?;
                    let eu = native(brep.edgeuse(current))?;
                    if eu.owner != EdgeuseOwner::Loopuse(Some(lu_id)) {
                        return Err(malformed("foreign plate coedge"));
                    }
                    let edge = eu
                        .edge
                        .ok_or_else(|| malformed("plate coedge has no edge"))?;
                    let forward = orientation(eu.orientation)?;
                    let next = eu.ccw.ok_or_else(|| malformed("open plate trim cycle"))?;
                    let next_use = native(brep.edgeuse(next))?;
                    let next_edge = next_use
                        .edge
                        .ok_or_else(|| malformed("missing next plate edge"))?;
                    let (a, b) = native(solidcore::topology::edge::start_end_vertices(brep, edge))?;
                    let (c, d) = native(solidcore::topology::edge::start_end_vertices(
                        brep, next_edge,
                    ))?;
                    let back = if forward { b } else { a };
                    let front = if orientation(next_use.orientation)? {
                        c
                    } else {
                        d
                    };
                    if back.is_none() || back != front {
                        return Err(malformed("plate trim cycle lacks shared vertex incidence"));
                    }
                    let uses = census.entry(edge).or_default();
                    if uses.len() >= 2 {
                        return Err(unsupported("nonmanifold plate boundary edge"));
                    }
                    uses.push(Use {
                        face: index,
                        role,
                        forward: forward == face_forward,
                    });
                    cycle.push(edge);
                    current = next;
                    if current == first {
                        break;
                    }
                    if count + 1 == brep.edgeuses().len() {
                        return Err(malformed("plate trim exceeds admitted native census"));
                    }
                }
                if let Role::Cap(side) = role {
                    cap_cycles[side].push(cycle);
                }
            }
        }
        for uses in census.values() {
            work.charge(1, cancel)?;
            if uses.len() != 2 || uses[0].forward == uses[1].forward {
                return Err(unsupported(
                    "plate boundary edges require two opposite material uses",
                ));
            }
            match (uses[0].role, uses[1].role) {
                (Role::Bore, Role::Bore) => {}
                (Role::Bore, Role::Cap(_)) | (Role::Cap(_), Role::Bore) => {}
                (Role::Outer(a), Role::Outer(b)) if a / 2 != b / 2 => {}
                (Role::Outer(_), Role::Cap(_)) | (Role::Cap(_), Role::Outer(_)) => {}
                _ => {
                    return Err(unsupported(
                        "plate contains an unexpected boundary adjacency",
                    ));
                }
            }
        }
        for (side, cycles) in cap_cycles.iter().enumerate() {
            if cycles.len() != 2 {
                return Err(unsupported(
                    "plate cap must have exactly rectangle and aperture cycles",
                ));
            }
            let mut circle = 0;
            let mut rectangle = 0;
            for cycle in cycles {
                work.charge(1, cancel)?;
                let neighbors: Vec<_> = cycle
                    .iter()
                    .map(|id| {
                        census[id]
                            .iter()
                            .find(|u| u.face != caps[side])
                            .map(|u| u.role)
                    })
                    .collect();
                if !neighbors.is_empty() && neighbors.iter().all(|r| *r == Some(Role::Bore)) {
                    circle += 1;
                } else if neighbors.len() == 4
                    && (0..4).all(|slot| {
                        neighbors
                            .iter()
                            .filter(|r| **r == Some(Role::Outer(slot)))
                            .count()
                            == 1
                    })
                {
                    rectangle += 1;
                } else {
                    return Err(unsupported(
                        "cap rim does not share its actual complete bore/rectangle edges",
                    ));
                }
            }
            if circle != 1 || rectangle != 1 {
                return Err(unsupported("plate cap boundary is not one annulus"));
            }
        }
        check_cancel(cancel)?;
        Ok(BoredPlate {
            center_mm: bore.center_mm,
            radius_mm: bore.radius_mm,
            height_mm: height,
            outer_xy_mm: xy,
            bore_faces,
            cap_faces: std::array::from_fn(|i| self.faces()[caps[i]].face_id.clone()),
            outer_faces: std::array::from_fn(|i| self.faces()[outer[i]].face_id.clone()),
            bore_face_angles,
        })
    }
}
