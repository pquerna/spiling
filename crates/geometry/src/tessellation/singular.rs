// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Display-only quotient authority. Shared indices propose ownership; original
//! native endpoints, an exactly collapsed law and a one-sided normal prove it.

use super::{audit::norm_upper, invalid, native_error, resource, shape};
use crate::{
    GeometryWorkBudget,
    kernel::Definition,
    parametric::{self, Interval},
};
use solidcore::nurbs::b_spline_surface_n_lib::p2::OrientType;
use solidcore::{curve::curve::Curve, surface::surface::Surface, topology::edgeuse::p1 as eu};
use solidcore_mesh::{EdgeSet, FaceMesh};
use spiling_contracts::geometry::{
    GeometryError, MESH_CARRIER_TOLERANCE_MM, NORMAL_NORM_TOLERANCE,
};
use std::{mem::size_of, sync::atomic::AtomicBool};

pub(super) struct Junction {
    pub shared: u32,
    pub source_loop: usize,
    pub uv: [[f64; 2]; 2],
    pub position: [f64; 3],
    pub normal: [f64; 3],
}

pub(super) struct Inventory {
    pub junctions: Vec<Junction>,
    locals: Vec<Option<usize>>,
    pub carrier_error: f64,
}
impl Inventory {
    pub fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            .saturating_add(
                self.junctions
                    .capacity()
                    .saturating_mul(size_of::<Junction>()),
            )
            .saturating_add(
                self.locals
                    .capacity()
                    .saturating_mul(size_of::<Option<usize>>()),
            )
    }
    pub fn at(&self, local: u32) -> Option<&Junction> {
        self.locals
            .get(local as usize)
            .copied()
            .flatten()
            .map(|j| &self.junctions[j])
    }
    pub fn node_count(&self, mesh: &FaceMesh) -> usize {
        mesh.positions.len() + 2 * self.junctions.len()
    }
    pub fn uv(&self, mesh: &FaceMesh, node: u32) -> [f64; 2] {
        if (node as usize) < mesh.uvs.len() {
            let q = mesh.uvs[node as usize];
            [q.x, q.y]
        } else {
            let offset = node as usize - mesh.uvs.len();
            self.junctions[offset / 2].uv[offset % 2]
        }
    }
    pub fn position(&self, mesh: &FaceMesh, node: u32) -> [f64; 3] {
        if (node as usize) < mesh.positions.len() {
            let p = mesh.positions[node as usize];
            [p.x, p.y, p.z]
        } else {
            self.junctions[(node as usize - mesh.positions.len()) / 2].position
        }
    }
    pub fn shared(&self, mesh: &FaceMesh, node: u32) -> Option<u32> {
        if (node as usize) < mesh.shared.len() {
            mesh.shared[node as usize]
        } else {
            Some(self.junctions[(node as usize - mesh.shared.len()) / 2].shared)
        }
    }
    /// Proof-only source chart endpoint. No mesh vertex is moved or published.
    pub fn source_endpoint(
        &self,
        mesh: &FaceMesh,
        local: u32,
        curve: &Curve,
        upper: bool,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<u32, GeometryError> {
        let j = self.locals[local as usize]
            .ok_or_else(|| invalid("source endpoint is not a proved singular replica"))?;
        let interval = curve.natural_interval();
        let t = if upper {
            interval.max()
        } else {
            interval.min()
        };
        let q = endpoint_uv(curve, t, work, cancel)?;
        let endpoint = self.junctions[j]
            .uv
            .iter()
            .position(|p| *p == q)
            .ok_or_else(|| {
                invalid("singular source interval endpoint changed its original junction")
            })?;
        u32::try_from(mesh.positions.len() + 2 * j + endpoint)
            .map_err(|_| resource("singular chart endpoint index overflow"))
    }
}

fn endpoint_uv(
    curve: &Curve,
    t: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[f64; 2], GeometryError> {
    let p = parametric::curve_enclosure(curve, (t, t), work, cancel)?.position;
    let q = [p[0].mid(), p[1].mid()];
    for (axis, &coordinate) in q.iter().enumerate() {
        if !parametric::curve_endpoint_coordinate_equal(curve, t, axis, coordinate, work, cancel)? {
            return Err(invalid(
                "singular display endpoint lacks exact native coefficient coordinates",
            ));
        }
    }
    Ok(q)
}

pub(super) fn inventory(
    definition: &Definition,
    mesh: &FaceMesh,
    edges: &EdgeSet,
    maximum: usize,
    outside: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Inventory, GeometryError> {
    let brep = definition.brep();
    let face = brep.face(mesh.face).map_err(native_error)?;
    let surface = face
        .surface
        .as_ref()
        .ok_or_else(|| invalid("singular display has no carrier"))?;
    let fu = brep
        .faceuse(
            face.faceuse
                .ok_or_else(|| invalid("singular display has no faceuse"))?,
        )
        .map_err(native_error)?;
    let mut result = Inventory {
        junctions: Vec::new(),
        locals: Vec::new(),
        carrier_error: 0.0,
    };
    let natural = surface.natural_uv_domain();
    let polar = parametric::analytic_eval::collapsed_step_sphere_position(
        surface,
        natural.v_min(),
        work,
        cancel,
    )?
    .is_some()
        || parametric::analytic_eval::collapsed_step_sphere_position(
            surface,
            natural.v_max(),
            work,
            cancel,
        )?
        .is_some();
    if !polar {
        return Ok(result);
    }
    super::check_memory(
        outside
            .saturating_add(size_of::<Inventory>())
            .saturating_add(maximum.saturating_mul(size_of::<Junction>()))
            .saturating_add(
                mesh.positions
                    .len()
                    .saturating_mul(size_of::<Option<usize>>()),
            ),
    )?;
    result
        .junctions
        .try_reserve_exact(maximum)
        .map_err(|_| resource("singular display inventory allocation failed"))?;
    super::check_memory(
        outside
            .saturating_add(result.retained_bytes())
            .saturating_add(
                mesh.positions
                    .len()
                    .saturating_mul(size_of::<Option<usize>>()),
            ),
    )?;
    result
        .locals
        .try_reserve_exact(mesh.positions.len())
        .map_err(|_| resource("singular display identity allocation failed"))?;
    result.locals.resize(mesh.positions.len(), None);
    super::check_memory(outside.saturating_add(result.retained_bytes()))?;
    for (source_loop, &lu) in fu.loopuses.iter().enumerate() {
        let uses = eu::loopuse_edgeuses(brep, lu).map_err(native_error)?;
        super::check_memory(
            outside
                .saturating_add(result.retained_bytes())
                .saturating_add(
                    uses.capacity()
                        .saturating_mul(size_of::<solidcore::topology::EdgeuseId>()),
                ),
        )?;
        for index in 0..uses.len() {
            work.charge(1, cancel)?;
            let endpoint = |use_, end: bool| -> Result<_, GeometryError> {
                let edge = eu::edge(brep, use_)
                    .map_err(native_error)?
                    .ok_or_else(|| invalid("singular endpoint has no edge"))?;
                let reverse = match eu::orientation(brep, use_).map_err(native_error)? {
                    OrientType::Same => false,
                    OrientType::Opposite => true,
                    _ => return Err(invalid("singular endpoint orientation is unresolved")),
                };
                let upper = end != reverse;
                let (a, b) = solidcore::topology::edge::start_end_vertices(brep, edge)
                    .map_err(native_error)?;
                let vertex = (if upper { b } else { a })
                    .ok_or_else(|| invalid("singular endpoint has no original vertex"))?;
                let poly = edges
                    .polyline(edge)
                    .ok_or_else(|| invalid("singular endpoint has no edge polyline"))?;
                let shared = *(if upper {
                    poly.vertices.last()
                } else {
                    poly.vertices.first()
                })
                .ok_or_else(|| invalid("singular endpoint polyline is empty"))?;
                let uv = eu::uv_trim_curve(brep, use_)
                    .map_err(native_error)?
                    .0
                    .map(|curve| {
                        let range = curve.natural_interval();
                        (curve, if upper { range.max() } else { range.min() })
                    });
                Ok((vertex, shared, uv))
            };
            let (av, a, auv) = endpoint(uses[index], true)?;
            let (bv, b, buv) = endpoint(uses[(index + 1) % uses.len()], false)?;
            if av != bv || a != b {
                return Err(invalid(
                    "source display junction changed original native vertex ownership",
                ));
            }
            let (Some((ac, at)), Some((bc, bt))) = (auv, buv) else {
                continue;
            };
            let aq = parametric::curve_enclosure(ac, (at, at), work, cancel)?.position;
            let bq = parametric::curve_enclosure(bc, (bt, bt), work, cancel)?.position;
            if aq[0].lo <= bq[0].hi && bq[0].lo <= aq[0].hi {
                continue;
            }
            let natural = surface.natural_uv_domain();
            let mut selected = None;
            for v in [natural.v_min(), natural.v_max()] {
                if parametric::analytic_eval::collapsed_step_sphere_position(
                    surface, v, work, cancel,
                )?
                .is_some()
                    && parametric::curve_endpoint_coordinate_equal(ac, at, 1, v, work, cancel)?
                    && parametric::curve_endpoint_coordinate_equal(bc, bt, 1, v, work, cancel)?
                {
                    selected = Some(v);
                    break;
                }
            }
            let Some(v) = selected else {
                continue;
            };
            let uv = [
                endpoint_uv(ac, at, work, cancel)?,
                endpoint_uv(bc, bt, work, cancel)?,
            ];
            let domain = [[uv[0][0].min(uv[1][0]), uv[0][0].max(uv[1][0])], [v, v]];
            let image = parametric::surface_position_enclosure(surface, domain, work, cancel)?;
            let collapsed = parametric::analytic_eval::collapsed_step_sphere_position(
                surface, v, work, cancel,
            )?
            .ok_or_else(|| invalid("singular coefficient law lost pole authority"))?;
            let p = edges
                .positions()
                .get(a as usize)
                .ok_or_else(|| invalid("singular shared vertex index is absent"))?;
            let original = brep.vertex(av).map_err(native_error)?.point;
            if *p != original {
                return Err(invalid(
                    "singular shared index does not bind its actual native vertex",
                ));
            }
            let position = [p.x, p.y, p.z];
            let error = norm_upper(std::array::from_fn(|k| {
                image[k].sub(Interval::point(position[k]))
            }))
            .max(norm_upper(std::array::from_fn(|k| {
                collapsed[k].sub(Interval::point(position[k]))
            })));
            // The invertible cofactor field extends continuously from the natural
            // interior to this row. The one-sided closed strip proves a finite,
            // unique unit enclosure, independently of candidate normal estimates.
            let inward = if v == natural.v_min() {
                parametric::up(v)
            } else {
                parametric::down(v)
            };
            let enclosure = parametric::surface_geometric_normal_enclosure(
                surface,
                [domain[0], [v.min(inward), v.max(inward)]],
                work,
                cancel,
            )?;
            let center = enclosure.map(Interval::mid);
            let length = center.iter().map(|c| c * c).sum::<f64>().sqrt();
            let normal = center.map(|c| c / length);
            if !length.is_finite()
                || length <= 0.0
                || norm_upper(std::array::from_fn(|k| {
                    enclosure[k].sub(Interval::point(normal[k]))
                })) > NORMAL_NORM_TOLERANCE
            {
                return Err(invalid(
                    "singular display has no unique one-sided coefficient normal",
                ));
            }
            if !error.is_finite() || error > MESH_CARRIER_TOLERANCE_MM {
                return Err(shape(
                    "singular chart row exceeds display carrier allowance",
                ));
            }
            if result.junctions.iter().any(|j| j.shared == a) {
                return Err(invalid(
                    "singular native vertex has ambiguous source junction ownership",
                ));
            }
            for (local, shared) in mesh.shared.iter().enumerate() {
                work.charge(1, cancel)?;
                if *shared != Some(a) {
                    continue;
                }
                let q = mesh.uvs[local];
                if q.y != v
                    || q.x < domain[0][0]
                    || q.x > domain[0][1]
                    || mesh.positions[local] != *p
                {
                    return Err(invalid(
                        "shared singular alias is not its proved collapsed native chart point",
                    ));
                }
                result.locals[local] = Some(result.junctions.len());
            }
            result.carrier_error = result.carrier_error.max(error);
            result.junctions.push(Junction {
                shared: a,
                source_loop,
                uv,
                position,
                normal,
            });
        }
    }
    u32::try_from(result.node_count(mesh))
        .map_err(|_| resource("singular display chart node count overflow"))?;
    Ok(result)
}

/// A positive UV fan is real coverage even though its chord is a retained line.
/// This is direct carrier transport to that SAME retained spoke, not a second
/// serial trim displacement. Disjoint fans and ordinary carrier patches combine
/// by maximum; the owning audit still adds the independently proved source trim.
pub(super) fn fan(
    surface: &Surface,
    mesh: &FaceMesh,
    inventory: &Inventory,
    triangle: [u32; 3],
    sense: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<f64, GeometryError> {
    work.charge(1, cancel)?;
    let uv = triangle.map(|i| inventory.uv(mesh, i));
    if super::trim::orient(uv[0], uv[1], uv[2])
        .mul(Interval::point(sense))
        .lo
        <= 0.0
    {
        return Err(invalid(
            "singular display fan has reversed, overlapping or uncertain chart incidence",
        ));
    }
    let points = triangle.map(|i| inventory.position(mesh, i));
    let error =
        parametric::analytic_eval::display_fan_transport(surface, &uv, &points, work, cancel)?;
    if !error.is_finite() || error > MESH_CARRIER_TOLERANCE_MM {
        return Err(shape(format!(
            "complete singular chart fan carrier={error} exceeds display carrier allowance"
        )));
    }
    Ok(error)
}
