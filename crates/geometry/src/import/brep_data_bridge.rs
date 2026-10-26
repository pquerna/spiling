// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Pure-Rust bridge from `solidcraft_geom::brep::Brep` (Schema 4) to `solidcore::Brep`.
//!
//! Adapts the topology and analytic-surface conversion rules of `solidcraft`'s
//! `import` module directly to `solidcore::topology::brep_data::BrepData` and
//! `solidcore::topology::brep::p10::make_topology_from_data`.

#![allow(dead_code)]

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::f64::consts::{FRAC_PI_2, PI, TAU};

use solidcore::curve::curve::Curve;
use solidcore::curve::ellipse::Ellipse;
use solidcore::curve::line::Line;
use solidcore::error::EntityRef;
use solidcore::geom::attribute::{AttributeStore, ObjectKey};
use solidcore::geom::axis2_placement::Axis2Placement;
use solidcore::geom::extent1d::Extent1d;
use solidcore::geom::extent2d::Extent2d;
use solidcore::geom::object::ObjectRef;
use solidcore::geom::vector2d::Point2;
use solidcore::geom::vector3d::{Point3, Vec3};
use solidcore::kernel::Kernel;
use solidcore::nurbs::b_spline_curve::p1::CanonicalData;
use solidcore::nurbs::b_spline_curve::{BSplineCurve, BSplineCurveForm, KnotType};
use solidcore::nurbs::b_spline_surface::p1::CanonicalSurfaceData;
use solidcore::nurbs::b_spline_surface::{BSplineSurface, BSplineSurfaceForm};
use solidcore::surface::cone::Cone;
use solidcore::surface::plane::Plane;
use solidcore::surface::sphere::Sphere;
use solidcore::surface::surface::Surface;
use solidcore::surface::surface::p1::StepSurface;
use solidcore::surface::torus::{Torus, TorusOptions};
use solidcore::topology::Brep;
use solidcore::topology::brep::p10::{
    HEALER_VERSION, MakeTopologyOptions, TopologyEnv, make_topology_from_data,
};
use solidcore::topology::brep_data::{
    BrepData, DataSourceType, EdgeData, EuData, FaceData, FaceuseData, Keyed, LoopData, RegionData,
    ShellData, VertexData,
};
use solidcraft_geom::brep as b;
use solidcraft_geom::brep::eval::{CurveEval, SurfaceEval};

const NO_UV_CURVE: u64 = 999_999;

/// Why a Schema 4 B-rep record cannot be converted into `solidcore::Brep`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BridgeError {
    Invalid(&'static str),
    NotYet(&'static str),
    Kernel(String),
}

impl std::fmt::Display for BridgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BridgeError::Invalid(w) => write!(f, "invalid B-rep record: {w}"),
            BridgeError::NotYet(w) => write!(f, "not supported yet: {w} in solidcore"),
            BridgeError::Kernel(msg) => write!(f, "solidcore error: {msg}"),
        }
    }
}

impl std::error::Error for BridgeError {}

impl From<solidcore::SolidError> for BridgeError {
    fn from(err: solidcore::SolidError) -> Self {
        BridgeError::Kernel(err.to_string())
    }
}

type R<T> = Result<T, BridgeError>;

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn z_axis(f: &b::Frame) -> [f64; 3] {
    cross(f.x_axis, f.y_axis)
}

fn p3(p: [f64; 3]) -> Point3 {
    Point3::new(p[0], p[1], p[2])
}

fn v3(v: [f64; 3]) -> Vec3 {
    Vec3::new(v[0], v[1], v[2])
}

fn idx(i: usize) -> R<u32> {
    u32::try_from(i).map_err(|_| BridgeError::Invalid("index"))
}

fn placement(origin: [f64; 3], z_dir: [f64; 3], x_dir: [f64; 3]) -> R<Axis2Placement> {
    let y_dir = cross(z_dir, x_dir);
    Ok(Axis2Placement::new(p3(origin), v3(x_dir), v3(y_dir)))
}

fn unique_knots_and_multiplicities(flat: &[f64]) -> (Vec<f64>, Vec<usize>) {
    let mut knots: Vec<f64> = Vec::new();
    let mut mults: Vec<usize> = Vec::new();
    for &k in flat {
        if let Some(&last) = knots.last() {
            if k == last {
                if let Some(m) = mults.last_mut() {
                    *m += 1;
                }
                continue;
            }
        }
        knots.push(k);
        mults.push(1);
    }
    (knots, mults)
}

/// Parse the original STEP exchange without changing knots, multiplicities or domains.
pub fn parse_step_exchange(step_text: &str) -> Result<solidcraft_step::p21::Exchange, String> {
    solidcraft_step::p21::parse(step_text)
}

/// Converts a validated Schema 4 [`solidcraft_geom::brep::Brep`] into a [`solidcore::topology::Brep`].
pub fn from_schema4_brep(k: &mut Kernel, brep: &b::Brep, heal: bool) -> R<Brep> {
    let mut data = to_brep_data(k, brep, true)?;
    if !heal {
        data.healer_version = HEALER_VERSION;
    }
    let mut out = Brep::new_with_infinite_region()?;
    let mut store = AttributeStore::new();
    let mut records = BTreeMap::new();
    let key_of = |e: EntityRef| -> ObjectRef {
        let (type_code, kind, i) = match e {
            EntityRef::Brep(x) => (16011, 0u64, x.index()),
            EntityRef::Region(x) => (16012, 1, x.index()),
            EntityRef::Shell(x) => (16013, 2, x.index()),
            EntityRef::Faceuse(x) => (16022, 3, x.index()),
            EntityRef::Face(x) => (16002, 4, x.index()),
            EntityRef::Loopuse(x) => (16004, 5, x.index()),
            EntityRef::Loop(x) => (16003, 6, x.index()),
            EntityRef::Edgeuse(x) => (16005, 7, x.index()),
            EntityRef::Edge(x) => (16016, 8, x.index()),
            EntityRef::Vertexuse(x) => (16006, 9, x.index()),
            EntityRef::Vertex(x) => (16021, 10, x.index()),
        };
        ObjectRef {
            type_code,
            key: ObjectKey(((kind + 1) << 32) | u64::from(i)),
        }
    };
    let mut env = TopologyEnv {
        all_attributes: &[],
        attributes: &mut store,
        key_of: &key_of,
        records: &mut records,
        next_key: 1_000_000,
    };
    let opts = MakeTopologyOptions {
        make_uv_trim_curves: true,
        // Declared analytics were already constructed by build_surface/curve.
        // Shape recognition here may rebuild a spline as a canonical analytic
        // carrier with a different parameter law and discard its original
        // knots, while retaining the old UV trims/domain. Copy exact operands.
        add_analytics: false,
        force_same_tol: true,
        check_uv_trim_curves: false,
        always_heal: heal,
        healer_enabled: heal,
    };
    make_topology_from_data(k, &mut out, &mut data, &mut env, opts)?;
    if !heal {
        let faces = solidcore::topology::brep::p1::faces(&out, None)?;
        for f in faces {
            let _ =
                solidcore::topology::face::p3::create_uv_trim_curves(k, &mut out, f, false, None);
        }
    }
    Ok(out)
}

/// Builds a [`solidcore::topology::brep_data::BrepData`] from a Schema 4 `Brep`.
pub fn to_brep_data(k: &mut Kernel, brep: &b::Brep, analytic_planes: bool) -> R<BrepData> {
    brep.validate().map_err(|e| BridgeError::Invalid(e.0))?;

    // Manifold flag 0 tells `make_topology_from_data` to use the explicit regions,
    // shells, and `relink_radial_lists` (`next_eu` / `mate_next_eu`) we construct below.
    let mut d = BrepData::new(0, DataSourceType::Step);
    d.zone_tol_3d = brep.tolerance;
    d.approx_tol_3d = brep.tolerance * 0.5;
    d.start_region = 0;
    d.num_regions = brep.regions.len() as u64;

    let mut next_geom_key = 1u64;
    let mut alloc_key = || {
        let key = ObjectKey(next_geom_key);
        next_geom_key += 1;
        key
    };

    // 1. Regions, shells, and faceuses.
    let enclosed_by: HashMap<u32, usize> = brep
        .regions
        .iter()
        .enumerate()
        .skip(1)
        .filter_map(|(r, region)| region.shells.first().map(|&s| (s, r)))
        .collect();
    let listed = |s: u32| {
        brep.regions
            .iter()
            .filter(|r| r.shells.contains(&s))
            .count()
    };

    for (r, region) in brep.regions.iter().enumerate() {
        let start_shell = d.shells.len() as u64;
        let num_shells = region.shells.len() as u64;
        d.regions.push(RegionData {
            start_shell,
            num_shells,
            is_void: i32::from(r == 0 || region.void),
            ..RegionData::zeroed()
        });
        for (k_shell, &s) in region.shells.iter().enumerate() {
            let Some(b::Shell::Faces { faces, wires }) = brep.shells.get(s as usize) else {
                return Err(BridgeError::NotYet("wire and vertex shells"));
            };
            if !wires.is_empty() {
                return Err(BridgeError::NotYet("wire edges embedded in a shell"));
            }
            let uses: Vec<(u32, bool)> = if listed(s) == 2 {
                let inside = enclosed_by
                    .get(&s)
                    .copied()
                    .ok_or(BridgeError::Invalid("closed shell encloses no region"))?;
                let inside_is_void = brep.regions.get(inside).is_some_and(|e| e.void);
                let normals_into_this = if k_shell == 0 && r > 0 {
                    inside_is_void
                } else {
                    !inside_is_void
                };
                faces
                    .iter()
                    .map(|f| (f.face, f.forward == normals_into_this))
                    .collect()
            } else {
                faces
                    .iter()
                    .flat_map(|f| [(f.face, true), (f.face, false)])
                    .collect()
            };
            let faceuse_start = d.faceuses.len() as u64;
            let num_faceuses = uses.len() as u64;
            d.shells.push(ShellData {
                shell_type: 0,
                faceuse_start,
                num_faceuses,
                ..ShellData::zeroed()
            });
            for (face, same) in uses {
                d.faceuses.push(FaceuseData {
                    face: u64::from(face),
                    orientation: i32::from(same),
                    ..FaceuseData::zeroed()
                });
            }
        }
    }

    // 2. Number edgeuses face -> loop -> position, and compute radial links.
    let mut eu_index: HashMap<(u32, u32, u32), i64> = HashMap::new();
    let mut n_eus = 0i64;
    for (fi, f) in brep.faces.iter().enumerate() {
        for (li, l) in f.loops.iter().enumerate() {
            match l {
                b::Loop::Edges { edges } => {
                    for p in 0..edges.len() {
                        eu_index.insert((idx(fi)?, idx(li)?, idx(p)?), n_eus);
                        n_eus += 1;
                    }
                }
                b::Loop::Vertex { .. } => {}
            }
        }
    }
    let total_eus = usize::try_from(n_eus).unwrap_or(0);
    let mut next_radial = vec![-1i64; total_eus];
    let mut radial_entry = vec![0i64; total_eus];
    let mut edge_prim_eu = vec![-1i64; brep.edges.len()];
    for (ei, e) in brep.edges.iter().enumerate() {
        let at = |u: &b::EdgeUseRef| {
            eu_index
                .get(&(u.face, u.r#loop, u.position))
                .copied()
                .ok_or(BridgeError::Invalid("edge use"))
        };
        if let Some(first_use) = e.uses.first() {
            let first_idx = at(first_use)?;
            edge_prim_eu[ei] = if first_use.upper {
                first_idx
            } else {
                -first_idx - 1
            };
        }
        for (k_use, u) in e.uses.iter().enumerate() {
            let here = at(u)?;
            let after = e
                .uses
                .get((k_use + 1) % e.uses.len())
                .ok_or(BridgeError::Invalid("edge use"))?;
            let slot = usize::try_from(here).map_err(|_| BridgeError::Invalid("edge use"))?;
            next_radial[slot] = at(after)?;
            radial_entry[slot] = i64::from(u.upper);
        }
    }

    // Each edge's interval and angular shift.
    let intervals: Vec<([f64; 2], f64)> = brep
        .edges
        .iter()
        .map(|e| {
            if e.nurbs.is_some() {
                (e.interval, 0.0)
            } else {
                turned(&e.curve, e.interval)
            }
        })
        .collect();

    // 3. Faces, surfaces, loops, and edgeuses.
    let mut eu_cursor = 0usize;
    for f in &brep.faces {
        let (surf, uv_domain, is_spline_face) = match &f.nurbs {
            Some(n) => {
                let (s, dom, _) = build_surface(k, n, f.domain, analytic_planes)?;
                (s, dom, true)
            }
            None => build_surface(k, &f.surface, f.domain, analytic_planes)?,
        };
        let surf_idx = d.surfaces.len() as u64;
        d.surfaces.push(Keyed {
            key: alloc_key(),
            object: surf,
        });

        let start_loop = d.loops.len() as u64;
        let num_loops = f.loops.len() as u64;
        d.faces.push(FaceData {
            surface: surf_idx,
            start_loop,
            num_loops,
            uv_domain,
            rectangular_trim: 0,
            zone_tol_3d: brep.tolerance,
            ..FaceData::zeroed()
        });

        for l in &f.loops {
            let loop_idx = d.loops.len() as u64;
            match l {
                b::Loop::Vertex { vertex } => {
                    d.loops.push(LoopData {
                        loop_type: 1,
                        vertex: u64::from(*vertex),
                        start_eu: 0,
                        num_eu: 0,
                        ..LoopData::zeroed()
                    });
                }
                b::Loop::Edges { edges } => {
                    let start_eu = d.edgeuses.len() as u64;
                    let num_eu = edges.len() as u64;
                    d.loops.push(LoopData {
                        loop_type: 0,
                        vertex: 0,
                        start_eu,
                        num_eu,
                        ..LoopData::zeroed()
                    });
                    for u in edges {
                        let uv_curve = if is_spline_face {
                            let shift = intervals.get(u.edge as usize).map_or(0.0, |i| i.1);
                            // In `solidcore` (`SmEdgeuse::NormalizedEvaluate`) and `solidcore-mesh`
                            // (`MapContext::edgeuse_points`), `uv_trim_curve` is parameterised in
                            // the 3D edge's direction (`min -> max`), NOT reversed for opposite edgeuses.
                            let c2d = build_trim_curve(&u.pcurve, true, shift)?;
                            let idx_uv = d.uv_curves.len() as u64;
                            d.uv_curves.push(Keyed {
                                key: alloc_key(),
                                object: c2d,
                            });
                            idx_uv
                        } else {
                            NO_UV_CURVE
                        };

                        // Radial links: each edgeuse record `i` builds two edgeuses in solidcore:
                        //   - `eu1` (signed index `i >= 0`): on the face's `same` (top) faceuse.
                        //   - `eu2` (signed index `-i - 1 < 0`): on the face's `mate` (bottom) faceuse.
                        // In Schema 4 (`Radial`), `radial_entry[r]` (`u.upper`) is true when the
                        // right-hand traversal around the edge enters record `r` from its `same`
                        // side (`eu1`, index `r`) and leaves from `eu2` (`-r - 1`), and false when
                        // it enters from `eu2` (`-r - 1`) and leaves from `eu1` (`r`).
                        let here_i64 = eu_cursor as i64;
                        let here_upper = radial_entry[eu_cursor] != 0;
                        let next_rec = next_radial[eu_cursor];
                        let next_upper = usize::try_from(next_rec)
                            .ok()
                            .and_then(|idx| radial_entry.get(idx))
                            .copied()
                            .unwrap_or(0)
                            != 0;
                        let target_entry = if next_upper { next_rec } else { -next_rec - 1 };
                        let (next_eu, mate_next_eu) = if here_upper {
                            // Enters at `eu1` (`here_i64`), steps to `eu2` (`-here_i64 - 1`),
                            // and exits from `eu2` to `target_entry`.
                            (-here_i64 - 1, target_entry)
                        } else {
                            // Enters at `eu2` (`-here_i64 - 1`), steps to `eu1` (`here_i64`),
                            // and exits from `eu1` to `target_entry`.
                            (target_entry, here_i64)
                        };

                        d.edgeuses.push(EuData {
                            eu_type: 0,
                            edge: u64::from(u.edge),
                            loop_: loop_idx,
                            orientation: i32::from(u.forward),
                            uv_curve,
                            next_eu,
                            mate_next_eu,
                            ..EuData::zeroed()
                        });
                        eu_cursor += 1;
                    }
                }
            }
        }
    }

    // 4. Edges and 3D curves.
    for (ei, (e, &(interval, _))) in brep.edges.iter().zip(&intervals).enumerate() {
        let (crv, nurb_interval) = match (&e.curve, &e.nurbs) {
            (_, Some(n)) => build_curve_3d(k, n, interval)?,
            (c, None) => build_curve_3d(k, c, interval)?,
        };
        let crv_idx = d.curves_3d.len() as u64;
        d.curves_3d.push(Keyed {
            key: alloc_key(),
            object: crv,
        });
        d.edges.push(EdgeData {
            curve: crv_idx,
            start_vertex: u64::from(e.vertices[0]),
            end_vertex: u64::from(e.vertices[1]),
            prim_eu: edge_prim_eu[ei],
            interval: nurb_interval,
            zone_tol_3d: e.tolerance.max(brep.tolerance),
            ..EdgeData::zeroed()
        });
    }

    // 5. Vertices.
    for v in &brep.vertices {
        d.vertices.push(VertexData {
            point: p3(v.point),
            zone_tol_3d: v.tolerance.max(brep.tolerance),
            ..VertexData::zeroed()
        });
    }

    Ok(d)
}

/// Cancel only an exact, adjacent out-and-back use of one otherwise unowned
/// edge. Distinct periodic pcurves are real seams, not null spurs. Faces,
/// carriers and all surviving curve coefficients stay in their original order.
pub(crate) fn strip_lamina_spurs(
    brep: &mut b::Brep,
    byte_limit: usize,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<(), spiling_contracts::geometry::GeometryError> {
    use super::{check_cancel, error, invalid};
    use spiling_contracts::geometry::GeometryErrorCode;

    fn pair(
        records: &[b::Edge],
        a: &b::OrientedEdge,
        z: &b::OrientedEdge,
        face: usize,
        wire: usize,
    ) -> bool {
        a.edge == z.edge
            && a.forward != z.forward
            && records.get(a.edge as usize).is_some_and(|edge| {
                edge.uses.len() == 2
                    && edge
                        .uses
                        .iter()
                        .all(|u| u.face as usize == face && u.r#loop as usize == wire)
            })
            && a.pcurve == z.pcurve
    }

    // No scratch, allocation or BREP copy for the ordinary path.
    let mut found = false;
    for (fi, face) in brep.faces.iter().enumerate() {
        for (li, wire) in face.loops.iter().enumerate() {
            let b::Loop::Edges { edges } = wire else {
                continue;
            };
            for i in 0..edges.len() {
                check_cancel(cancel)?;
                if edges.len() > 2
                    && pair(
                        &brep.edges,
                        &edges[i],
                        &edges[(i + 1) % edges.len()],
                        fi,
                        li,
                    )
                {
                    found = true;
                    break;
                }
            }
            if found {
                break;
            }
        }
        if found {
            break;
        }
    }
    if !found {
        return Ok(());
    }

    fn buffer<T: Copy>(
        count: usize,
        value: T,
        retained: &mut usize,
        limit: usize,
    ) -> Result<Vec<T>, spiling_contracts::geometry::GeometryError> {
        let resource = || {
            error(
                GeometryErrorCode::ResourceLimit,
                "normalized topology scratch allowance exhausted",
            )
        };
        let bytes = count
            .checked_mul(std::mem::size_of::<T>())
            .ok_or_else(resource)?;
        if retained.checked_add(bytes).is_none_or(|n| n > limit) {
            return Err(resource());
        }
        let mut out = Vec::new();
        out.try_reserve_exact(count).map_err(|_| resource())?;
        let actual = out
            .capacity()
            .checked_mul(std::mem::size_of::<T>())
            .ok_or_else(resource)?;
        *retained = retained.checked_add(actual).ok_or_else(resource)?;
        if *retained > limit {
            return Err(resource());
        }
        out.resize(count, value);
        Ok(out)
    }

    let mut retained = 0;
    let mut edge_new = buffer(brep.edges.len(), 0u32, &mut retained, byte_limit)?;
    let mut vertex_new = buffer(brep.vertices.len(), u32::MAX, &mut retained, byte_limit)?;
    let mut loop_count = 0usize;
    let mut use_count = 0usize;
    let mut max_wire = 0usize;
    for face in &brep.faces {
        for wire in &face.loops {
            check_cancel(cancel)?;
            loop_count = loop_count
                .checked_add(1)
                .ok_or_else(|| invalid("normalized topology loop count overflow"))?;
            if let b::Loop::Edges { edges } = wire {
                use_count = use_count
                    .checked_add(edges.len())
                    .ok_or_else(|| invalid("normalized topology use count overflow"))?;
                max_wire = max_wire.max(edges.len());
            }
        }
    }
    let mut face_start = buffer(brep.faces.len(), 0usize, &mut retained, byte_limit)?;
    let mut loop_start = buffer(loop_count, 0usize, &mut retained, byte_limit)?;
    let mut use_new = buffer(use_count, u32::MAX, &mut retained, byte_limit)?;
    let mut stack_old = buffer(max_wire, 0usize, &mut retained, byte_limit)?;
    let mut loop_cursor = 0;
    let mut use_cursor = 0;

    for (fi, face) in brep.faces.iter_mut().enumerate() {
        face_start[fi] = loop_cursor;
        let mut modified = false;
        for (li, wire) in face.loops.iter_mut().enumerate() {
            loop_start[loop_cursor] = use_cursor;
            loop_cursor += 1;
            let b::Loop::Edges { edges } = wire else {
                continue;
            };
            // An in-place stack cancels nested adjacent pairs in linear work;
            // unlike repeated drain/search, it does not rescan long wires.
            let n = edges.len();
            let mut kept = 0;
            for read in 0..n {
                check_cancel(cancel)?;
                if kept > 0
                    && kept + n - read > 2
                    && pair(&brep.edges, &edges[kept - 1], &edges[read], fi, li)
                {
                    edge_new[edges[read].edge as usize] = u32::MAX;
                    kept -= 1;
                    modified = true;
                } else {
                    edges.swap(kept, read);
                    stack_old[kept] = read;
                    kept += 1;
                }
            }
            edges.truncate(kept);
            let mut start = 0;
            let mut end = kept;
            while end - start > 2 && pair(&brep.edges, &edges[end - 1], &edges[start], fi, li) {
                check_cancel(cancel)?;
                edge_new[edges[start].edge as usize] = u32::MAX;
                start += 1;
                end -= 1;
                modified = true;
            }
            for p in start..end {
                use_new[use_cursor + stack_old[p]] = u32::try_from(p - start)
                    .map_err(|_| invalid("normalized topology use index"))?;
            }
            use_cursor += n;
            edges.truncate(end);
            edges.drain(..start);
        }
        if modified {
            face.domain = recompute_face_domain(face.domain, &face.loops, cancel)?;
        }
    }

    // Surviving edges retain their relative first-appearance order and their
    // exact primary/radial use order. Only positions within shortened loops
    // change; never replace radial order with the face walk's order.
    let mut next_edge = 0u32;
    for (old, edge) in brep.edges.iter_mut().enumerate() {
        check_cancel(cancel)?;
        if edge_new[old] == u32::MAX {
            continue;
        }
        edge_new[old] = next_edge;
        next_edge = next_edge
            .checked_add(1)
            .ok_or_else(|| invalid("normalized topology edge index overflow"))?;
        for u in &mut edge.uses {
            check_cancel(cancel)?;
            let loop_base = *face_start
                .get(u.face as usize)
                .ok_or_else(|| invalid("normalized topology face reference"))?;
            let use_base = *loop_start
                .get(loop_base + u.r#loop as usize)
                .ok_or_else(|| invalid("normalized topology loop reference"))?;
            u.position = *use_new
                .get(use_base + u.position as usize)
                .filter(|&&p| p != u32::MAX)
                .ok_or_else(|| invalid("normalized topology removed a live edge use"))?;
        }
    }
    for face in &mut brep.faces {
        for wire in &mut face.loops {
            if let b::Loop::Edges { edges } = wire {
                for u in edges {
                    check_cancel(cancel)?;
                    u.edge = *edge_new
                        .get(u.edge as usize)
                        .filter(|&&e| e != u32::MAX)
                        .ok_or_else(|| invalid("normalized topology removed a live edge"))?;
                }
            }
        }
    }
    for shell in &mut brep.shells {
        let wires = match shell {
            b::Shell::Faces { wires, .. } => wires,
            b::Shell::Wire { edges } => edges,
            b::Shell::Vertex { .. } => continue,
        };
        for edge in wires {
            check_cancel(cancel)?;
            *edge = *edge_new
                .get(*edge as usize)
                .filter(|&&e| e != u32::MAX)
                .ok_or_else(|| invalid("normalized topology removed a wire edge"))?;
        }
    }
    let mut old = 0;
    brep.edges.retain(|_| {
        let keep = edge_new[old] != u32::MAX;
        old += 1;
        keep
    });

    let mut next_vertex = 0u32;
    let mut visit = |v: &mut u32| -> Result<(), spiling_contracts::geometry::GeometryError> {
        check_cancel(cancel)?;
        let slot = vertex_new
            .get_mut(*v as usize)
            .ok_or_else(|| invalid("normalized topology vertex reference"))?;
        if *slot == u32::MAX {
            *slot = next_vertex;
            next_vertex = next_vertex
                .checked_add(1)
                .ok_or_else(|| invalid("normalized topology vertex index overflow"))?;
        }
        *v = *slot;
        Ok(())
    };
    for edge in &mut brep.edges {
        for vertex in &mut edge.vertices {
            visit(vertex)?;
        }
    }
    for face in &mut brep.faces {
        for wire in &mut face.loops {
            if let b::Loop::Vertex { vertex } = wire {
                visit(vertex)?;
            }
        }
    }
    for shell in &mut brep.shells {
        if let b::Shell::Vertex { vertex } = shell {
            visit(vertex)?;
        }
    }
    // Move, never clone, owned vertices into their new canonical slots. Dead
    // vertices are only those with no remaining edge, vertex-loop or shell use.
    let mut i = 0;
    while i < vertex_new.len() {
        check_cancel(cancel)?;
        let target = vertex_new[i];
        if target == u32::MAX {
            vertex_new.swap_remove(i);
            brep.vertices.swap_remove(i);
        } else if target as usize == i {
            i += 1;
        } else {
            vertex_new.swap(i, target as usize);
            brep.vertices.swap(i, target as usize);
        }
    }
    Ok(())
}

fn recompute_face_domain(
    previous: [[f64; 2]; 2],
    loops: &[b::Loop],
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<[[f64; 2]; 2], spiling_contracts::geometry::GeometryError> {
    let mut lo = [f64::INFINITY; 2];
    let mut hi = [f64::NEG_INFINITY; 2];
    let mut include = |p: [f64; 2]| -> Result<(), spiling_contracts::geometry::GeometryError> {
        super::check_cancel(cancel)?;
        if !p.into_iter().all(f64::is_finite) {
            return Err(super::invalid(
                "normalized topology nonfinite pcurve coefficient",
            ));
        }
        for d in 0..2 {
            lo[d] = lo[d].min(p[d]);
            hi[d] = hi[d].max(p[d]);
        }
        Ok(())
    };
    for wire in loops {
        let b::Loop::Edges { edges } = wire else {
            continue;
        };
        for u in edges {
            match &u.pcurve {
                b::Curve2::Spline { points, .. } => {
                    for &p in points {
                        include(p)?;
                    }
                }
                b::Curve2::RationalSpline { homogeneous, .. } => {
                    // A mixed/nonpositive-weight hull cannot narrow a chart.
                    // In particular, never silently omit its coefficients.
                    if homogeneous.iter().any(|h| !(h[2] > 0.0)) {
                        return Ok(previous);
                    }
                    for h in homogeneous {
                        include([h[0] / h[2], h[1] / h[2]])?;
                    }
                }
            }
        }
    }
    let mut out = previous;
    for d in 0..2 {
        let (a, z) = (lo[d], hi[d]);
        if !(a.is_finite() && z.is_finite() && a <= z) {
            return Err(super::invalid("normalized topology face without pcurves"));
        }
        // Preserve the decoder's padding convention, but only intersect its
        // existing chart. No new tolerance, natural period or wider domain.
        let pad = 1e-6 * (1.0 + (z - a)) + 1e-9 * a.abs().max(z.abs());
        out[d] = [previous[d][0].max(a - pad), previous[d][1].min(z + pad)];
        if !(out[d][0] < out[d][1]) {
            return Err(super::invalid("normalized topology empty face domain"));
        }
    }
    Ok(out)
}

fn turned(c: &b::Curve, interval: [f64; 2]) -> ([f64; 2], f64) {
    if !matches!(c, b::Curve::Circle { .. } | b::Curve::Ellipse { .. }) {
        return (interval, 0.0);
    }
    let [lo, hi] = interval;
    let shift = if hi > TAU {
        -TAU * ((hi - TAU) / TAU).ceil()
    } else if lo < -TAU {
        TAU * ((-TAU - lo) / TAU).ceil()
    } else {
        0.0
    };
    ([lo + shift, hi + shift], shift)
}

fn build_surface(
    k: &Kernel,
    s: &b::Surface,
    domain: [[f64; 2]; 2],
    analytic_planes: bool,
) -> R<(Surface, Extent2d, bool)> {
    let [[u0, u1], [v0, v1]] = domain;
    match s {
        b::Surface::Plane {
            origin,
            u_axis,
            v_axis,
        } => build_plane(*origin, *u_axis, *v_axis, domain, analytic_planes),
        b::Surface::PlaneThrough {
            origin,
            u_point,
            v_point,
        } => build_plane(
            *origin,
            sub(*u_point, *origin),
            sub(*v_point, *origin),
            domain,
            analytic_planes,
        ),
        b::Surface::Cylinder { frame, radius } => {
            let mut f = frame.clone();
            let mut u_range = [u0, u1];
            if u0 != 0.0 {
                analytic::turn_frame_in_place(&mut f, u0);
                u_range = [0.0, u1 - u0];
            }
            let ax = placement(f.origin, z_axis(&f), f.x_axis)?;
            let cyl = Cone::create_canonical_cylinder(k, &ax, *radius)?;
            let u0_deg = u_range[0].to_degrees();
            let u1_deg = u_range[1].to_degrees();
            let dom = Extent2d::from_bounds(u0_deg, v0, u1_deg, v1);
            Ok((
                Surface::from(StepSurface::new(Surface::from(cyl), true)?),
                dom,
                false,
            ))
        }
        b::Surface::Cone {
            frame,
            radius_at_origin,
            half_angle,
        } => {
            let mut f = frame.clone();
            let mut u_range = [u0, u1];
            if u0 != 0.0 {
                analytic::turn_frame_in_place(&mut f, u0);
                u_range = [0.0, u1 - u0];
            }
            let ax = placement(f.origin, z_axis(&f), f.x_axis)?;
            let cone = Cone::create_canonical_with_semi_angle(
                k,
                &ax,
                *radius_at_origin,
                half_angle.to_degrees(),
                None,
                false,
                false,
            )?;
            let u0_deg = u_range[0].to_degrees();
            let u1_deg = u_range[1].to_degrees();
            let dom = Extent2d::from_bounds(u0_deg, v0, u1_deg, v1);
            Ok((
                Surface::from(StepSurface::new(Surface::from(cone), true)?),
                dom,
                false,
            ))
        }
        b::Surface::Sphere { frame, radius } => {
            let mut f = frame.clone();
            let mut u_range = [u0, u1];
            if u0 != 0.0 {
                analytic::turn_frame_in_place(&mut f, u0);
                u_range = [0.0, u1 - u0];
            }
            let ax = placement(f.origin, z_axis(&f), f.x_axis)?;
            let sph = Sphere::create_canonical(k, &ax, *radius)?;
            let u0_deg = u_range[0].to_degrees();
            let u1_deg = u_range[1].to_degrees();
            let v0_deg = v0.to_degrees();
            let v1_deg = v1.to_degrees();
            let dom = Extent2d::from_bounds(u0_deg, v0_deg, u1_deg, v1_deg);
            Ok((
                Surface::from(StepSurface::new(Surface::from(sph), true)?),
                dom,
                false,
            ))
        }
        b::Surface::Torus {
            frame,
            major_radius,
            minor_radius,
        } => {
            let mut f = frame.clone();
            let mut u_range = [u0, u1];
            if u0 != 0.0 {
                analytic::turn_frame_in_place(&mut f, u0);
                u_range = [0.0, u1 - u0];
            }
            let v_range = analytic::first_turn([v0, v1]);
            let ax = placement(f.origin, z_axis(&f), f.x_axis)?;
            let v_base_deg = (v_range[0].to_degrees() / 360.0).floor() * 360.0;
            let anal_dom = Extent2d::from_bounds(0.0, v_base_deg, 360.0, v_base_deg + 360.0);
            let tor = Torus::create_canonical(
                k,
                &ax,
                *major_radius,
                *minor_radius,
                TorusOptions::default(),
                Some(&anal_dom),
            )?;
            let u0_deg = u_range[0].to_degrees();
            let u1_deg = u_range[1].to_degrees();
            let v0_deg = v_range[0].to_degrees();
            let v1_deg = v_range[1].to_degrees();
            let dom = Extent2d::from_bounds(u0_deg, v0_deg, u1_deg, v1_deg);
            Ok((
                Surface::from(StepSurface::new(Surface::from(tor), true)?),
                dom,
                false,
            ))
        }
        b::Surface::Spline {
            u_knots,
            v_knots,
            points,
        } => {
            let nu = points.len();
            let nv = points.first().map_or(0, Vec::len);
            let cps: Vec<Point3> = points.iter().flatten().map(|&p| p3(p)).collect();
            let srf = build_bspline_surface(u_knots, v_knots, nu, nv, &cps, None)?;
            let dom = Extent2d::from_bounds(u0, v0, u1, v1);
            Ok((Surface::from(srf), dom, true))
        }
        b::Surface::RationalSpline {
            u_knots,
            v_knots,
            homogeneous,
        } => {
            let nu = homogeneous.len();
            let nv = homogeneous.first().map_or(0, Vec::len);
            let mut cps = Vec::with_capacity(nu * nv);
            let mut weights = Vec::with_capacity(nu * nv);
            for h in homogeneous.iter().flatten() {
                cps.push(Point3::new(h[0] / h[3], h[1] / h[3], h[2] / h[3]));
                weights.push(h[3]);
            }
            let srf = build_bspline_surface(u_knots, v_knots, nu, nv, &cps, Some(&weights))?;
            let dom = Extent2d::from_bounds(u0, v0, u1, v1);
            Ok((Surface::from(srf), dom, true))
        }
        b::Surface::PlacedRevolution {
            profile,
            origin,
            axis,
            matrix,
            forward,
        } => build_revolution_surface(profile, *origin, *axis, matrix, *forward, domain),
    }
}

fn build_plane(
    origin: [f64; 3],
    u: [f64; 3],
    v: [f64; 3],
    domain: [[f64; 2]; 2],
    analytic: bool,
) -> R<(Surface, Extent2d, bool)> {
    let [[u0, u1], [v0, v1]] = domain;
    let unit = |x: [f64; 3]| (dot(x, x).sqrt() - 1.0).abs() <= 1e-12;
    if !analytic && !(unit(u) && unit(v) && dot(u, v).abs() <= 1e-12) {
        let at = |s: f64, t: f64| p3(add(origin, add(scale(u, s), scale(v, t))));
        let cps = [at(u0, v0), at(u0, v1), at(u1, v0), at(u1, v1)];
        let srf = build_bspline_surface(&[u0, u0, u1, u1], &[v0, v0, v1, v1], 2, 2, &cps, None)?;
        let dom = Extent2d::from_bounds(u0, v0, u1, v1);
        return Ok((Surface::from(srf), dom, false));
    }
    let n = cross(u, v);
    let (lu, ln) = (dot(u, u).sqrt(), dot(n, n).sqrt());
    if !(lu > 0.0 && ln > 0.0 && lu.is_finite() && ln.is_finite()) {
        return Err(BridgeError::Invalid("plane axes"));
    }
    let (x, z) = (scale(u, 1.0 / lu), scale(n, 1.0 / ln));
    let y = cross(z, x);
    let corners = [[u0, v0], [u0, v1], [u1, v0], [u1, v1]].map(|[s, t]| {
        let d = add(scale(u, s), scale(v, t));
        [dot(d, x), dot(d, y)]
    });
    let lo = |k: usize| corners.iter().map(|c| c[k]).fold(f64::INFINITY, f64::min);
    let hi = |k: usize| {
        corners
            .iter()
            .map(|c| c[k])
            .fold(f64::NEG_INFINITY, f64::max)
    };
    let ax = placement(origin, z, x)?;
    let pl = Plane::create_canonical(&ax)?;
    let dom = Extent2d::from_bounds(lo(0), lo(1), hi(0), hi(1));
    Ok((Surface::from(pl), dom, false))
}

fn build_bspline_surface(
    u_knots_flat: &[f64],
    v_knots_flat: &[f64],
    nu: usize,
    nv: usize,
    cps: &[Point3],
    weights: Option<&[f64]>,
) -> R<BSplineSurface> {
    let u_order = u_knots_flat
        .len()
        .checked_sub(nu)
        .filter(|&o| o >= 2)
        .ok_or(BridgeError::Invalid("spline u order"))?;
    let v_order = v_knots_flat
        .len()
        .checked_sub(nv)
        .filter(|&o| o >= 2)
        .ok_or(BridgeError::Invalid("spline v order"))?;
    let (u_knots, u_mults) = unique_knots_and_multiplicities(u_knots_flat);
    let (v_knots, v_mults) = unique_knots_and_multiplicities(v_knots_flat);
    let mut surface = BSplineSurface::create_canonical(&CanonicalSurfaceData {
        u_degree: u_order - 1,
        v_degree: v_order - 1,
        control_points: cps,
        form: BSplineSurfaceForm::Unspecified,
        u_multiplicities: &u_mults,
        v_multiplicities: &v_mults,
        u_knots: &u_knots,
        v_knots: &v_knots,
        knot_type: KnotType::Unspecified,
        weights,
        uv_domain: None,
    })
    .map_err(BridgeError::from)?;

    // CreateCanonical routes through IGES clamping, which discards unused
    // exterior slots even after the decoder's original-vector restoration.
    // Keep all canonical validation; undo ONLY that proven inactive-slot
    // replacement, after checking the entire resulting coefficient owner.
    let native = surface.get_or_create_gw_nurb()?;
    let original = [u_knots_flat, v_knots_flat];
    let degrees = [u_order - 1, v_order - 1];
    let counts = [nu, nv];
    let native_knots = [native.u_knots(), native.v_knots()];
    let changed = [
        native_knots[0] != original[0],
        native_knots[1] != original[1],
    ];
    if !changed.into_iter().any(|changed| changed) {
        return Ok(surface);
    }
    let weight = |i: usize| weights.map_or(1.0, |w| w[i]);
    let equal_control = |a: usize, b: usize| {
        cps[a].x == cps[b].x
            && cps[a].y == cps[b].y
            && cps[a].z == cps[b].z
            && weight(a) == weight(b)
    };
    let equal_end_rows = [
        (0..nv).all(|j| equal_control(j, (nu - 1) * nv + j)),
        (0..nu).all(|i| equal_control(i * nv, i * nv + nv - 1)),
    ];
    let restore = [0, 1].map(|d| {
        changed[d]
            && equal_end_rows[d]
            && super::closed_spline_decode::same_active_basis_with_inactive_end_slots(
                original[d],
                native_knots[d],
                degrees[d],
                counts[d],
            )
    });
    if !restore.into_iter().any(|restore| restore) {
        return Ok(surface);
    }
    let indices = native.indices();
    if native.degrees() != (degrees[0], degrees[1])
        || indices.n.checked_add(1) != Some(nu)
        || indices.m.checked_add(1) != Some(nv)
        || (0..2).any(|d| changed[d] && !restore[d])
    {
        return Err(BridgeError::Invalid(
            "canonical spline changed its original active basis",
        ));
    }
    // IGES stores exactly constant-weight nets as nonrational. This exact
    // homogeneous gauge is allowed; near-constant or altered weights are not.
    let constant_weights = weights.is_none_or(|w| w.iter().all(|x| *x == w[0]));
    for i in 0..nu {
        for j in 0..nv {
            let index = i * nv + j;
            let c = native.cpt(i, j)?;
            let p = cps[index];
            let w = if c.w == solidcore::nurbs::math::NOW && constant_weights {
                1.0
            } else {
                let w = weight(index);
                if c.w != w {
                    return Err(BridgeError::Invalid(
                        "canonical spline changed an original weight",
                    ));
                }
                w
            };
            if c.x != p.x * w || c.y != p.y * w || c.z != p.z * w {
                return Err(BridgeError::Invalid(
                    "canonical spline changed an original control coefficient",
                ));
            }
        }
    }
    if restore[0] {
        let knots = native.u_knots_mut();
        knots[0] = u_knots_flat[0];
        let last = knots.len() - 1;
        knots[last] = u_knots_flat[last];
    }
    if restore[1] {
        let knots = native.v_knots_mut();
        knots[0] = v_knots_flat[0];
        let last = knots.len() - 1;
        knots[last] = v_knots_flat[last];
    }
    Ok(surface)
}

fn quarter_param(phi: f64) -> f64 {
    let c = std::f64::consts::FRAC_1_SQRT_2;
    let angle = |s: f64| {
        let (b0, b1, b2) = ((1.0 - s) * (1.0 - s), 2.0 * s * (1.0 - s) * c, s * s);
        (b1 + b2).atan2(b0 + b1)
    };
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..64 {
        let mid = 0.5 * (lo + hi);
        if angle(mid) < phi {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

fn circle_param(angle: f64) -> f64 {
    let q = FRAC_PI_2;
    let a = angle.clamp(0.0, 4.0 * q);
    let k = (a / q).floor().min(3.0);
    k + quarter_param(a - k * q)
}

fn build_revolution_surface(
    profile: &b::Curve,
    origin: [f64; 3],
    axis: [f64; 3],
    m: &[[f64; 4]; 4],
    forward: bool,
    domain: [[f64; 2]; 2],
) -> R<(Surface, Extent2d, bool)> {
    let (knots, pts): (Vec<f64>, Vec<([f64; 3], f64)>) = match profile {
        b::Curve::Line { start, end } => {
            (vec![0.0, 0.0, 1.0, 1.0], vec![(*start, 1.0), (*end, 1.0)])
        }
        b::Curve::Spline { knots, points } => {
            (knots.clone(), points.iter().map(|p| (*p, 1.0)).collect())
        }
        b::Curve::RationalSpline { knots, homogeneous } => (
            knots.clone(),
            homogeneous
                .iter()
                .map(|h| ([h[0] / h[3], h[1] / h[3], h[2] / h[3]], h[3]))
                .collect(),
        ),
        _ => {
            return Err(BridgeError::NotYet(
                "revolutions of conic, polyline or intersection profiles",
            ));
        }
    };
    let len = dot(axis, axis).sqrt();
    if !(len > 0.0) {
        return Err(BridgeError::Invalid("revolution axis"));
    }
    let z = scale(axis, 1.0 / len);
    let ([p0, p1], [a0, a1]) = if forward {
        (domain[0], domain[1])
    } else {
        (domain[1], domain[0])
    };
    let t1 = circle_param(a1 - a0);
    let place =
        |p: [f64; 3]| [0, 1, 2].map(|r| m[0][r] * p[0] + m[1][r] * p[1] + m[2][r] * p[2] + m[3][r]);
    let det = {
        let col = |c: usize| [m[c][0], m[c][1], m[c][2]];
        dot(cross(col(0), col(1)), col(2))
    };
    let c = std::f64::consts::FRAC_1_SQRT_2;
    let mut net: Vec<Vec<(Point3, f64)>> = Vec::with_capacity(pts.len());
    for (p, w) in &pts {
        let r = sub(*p, origin);
        let centre = add(origin, scale(z, dot(r, z)));
        let x0 = sub(*p, centre);
        let y0 = cross(z, x0);
        let (s0, c0) = a0.sin_cos();
        let (x, y) = (
            add(scale(x0, c0), scale(y0, s0)),
            sub(scale(y0, c0), scale(x0, s0)),
        );
        let row = (0..9)
            .map(|k| {
                let (sk, ck) = (k as f64 * std::f64::consts::FRAC_PI_4).sin_cos();
                let (f, wk) = if k % 2 == 1 { (1.0 / c, c) } else { (1.0, 1.0) };
                (
                    p3(place(add(
                        centre,
                        scale(add(scale(x, ck), scale(y, sk)), f),
                    ))),
                    w * wk,
                )
            })
            .collect();
        net.push(row);
    }
    let circle_knots = [0.0, 0.0, 0.0, 1.0, 1.0, 2.0, 2.0, 3.0, 3.0, 4.0, 4.0, 4.0];
    let profile_first = forward == (det > 0.0);
    if profile_first {
        let mut cps = Vec::with_capacity(pts.len() * 9);
        let mut weights = Vec::with_capacity(pts.len() * 9);
        for (pt, w) in net.iter().flatten() {
            cps.push(*pt);
            weights.push(*w);
        }
        let srf = build_bspline_surface(&knots, &circle_knots, pts.len(), 9, &cps, Some(&weights))?;
        let dom = Extent2d::from_bounds(p0, 0.0, p1, t1);
        Ok((Surface::from(srf), dom, false))
    } else {
        let mut cps = Vec::with_capacity(pts.len() * 9);
        let mut weights = Vec::with_capacity(pts.len() * 9);
        for k_idx in 0..9 {
            for row in &net {
                if let Some(&(pt, w)) = row.get(k_idx) {
                    cps.push(pt);
                    weights.push(w);
                }
            }
        }
        let srf = build_bspline_surface(&circle_knots, &knots, 9, pts.len(), &cps, Some(&weights))?;
        let dom = Extent2d::from_bounds(0.0, p0, t1, p1);
        Ok((Surface::from(srf), dom, false))
    }
}

fn build_trim_curve(c: &b::Curve2, forward: bool, shift: f64) -> R<Curve> {
    let (knots, mut pts): (&[f64], Vec<([f64; 2], f64)>) = match c {
        b::Curve2::Spline { knots, points } => (knots, points.iter().map(|p| (*p, 1.0)).collect()),
        b::Curve2::RationalSpline { knots, homogeneous } => (
            knots,
            homogeneous
                .iter()
                .map(|h| ([h[0] / h[2], h[1] / h[2]], h[2]))
                .collect(),
        ),
    };
    let order = knots
        .len()
        .checked_sub(pts.len())
        .filter(|&o| o >= 2)
        .ok_or(BridgeError::Invalid("pcurve order"))?;
    let mut k_flat: Vec<f64> = knots.iter().map(|t| t + shift).collect();
    if !forward {
        pts.reverse();
        k_flat = k_flat.iter().rev().map(|t| -t).collect();
    }
    let cps: Vec<Point3> = pts
        .iter()
        .map(|&(p, _)| Point3::new(p[0], p[1], 0.0))
        .collect();
    let weights: Vec<f64> = pts.iter().map(|&(_, w)| w).collect();
    let (uk, mults) = unique_knots_and_multiplicities(&k_flat);
    let bsc = BSplineCurve::create_canonical(&CanonicalData {
        dim: 2,
        degree: order - 1,
        control_points: &cps,
        form: BSplineCurveForm::Unspecified,
        knot_multiplicities: &mults,
        knots: &uk,
        knot_type: KnotType::Unspecified,
        weights: Some(&weights),
        trim: None,
    })?;
    Ok(Curve::from(bsc))
}

fn build_curve_3d(k: &Kernel, c: &b::Curve, interval: [f64; 2]) -> R<(Curve, Extent1d)> {
    match c {
        b::Curve::Line { start, end } => {
            let dir = sub(*end, *start);
            let line = Line::create_canonical(p3(*start), v3(dir));
            let t0 = line.convert_t_from_step_to_nurbs(interval[0]);
            let t1 = line.convert_t_from_step_to_nurbs(interval[1]);
            Ok((Curve::from(line), Extent1d::from_raw(t0, t1)))
        }
        b::Curve::Circle { frame, radius } => {
            let ax = placement(frame.origin, z_axis(frame), frame.x_axis)?;
            let step_ivl =
                Extent1d::from_min_max(interval[0].to_degrees(), interval[1].to_degrees());
            let circle =
                Ellipse::create_canonical_circle(k, &ax, *radius, Some(&step_ivl), Some(false))?;
            let curve = Curve::from(circle);
            let nurb_ivl = curve.natural_interval();
            Ok((curve, nurb_ivl))
        }
        b::Curve::Ellipse { frame, radii } => {
            let ax = placement(frame.origin, z_axis(frame), frame.x_axis)?;
            let step_ivl =
                Extent1d::from_min_max(interval[0].to_degrees(), interval[1].to_degrees());
            let ell = Ellipse::create_canonical(
                k,
                &ax,
                radii[0],
                radii[1],
                Some(&step_ivl),
                Some(false),
            )?;
            let curve = Curve::from(ell);
            let nurb_ivl = curve.natural_interval();
            Ok((curve, nurb_ivl))
        }
        b::Curve::Polyline { points } => {
            let n = points.len();
            let mut k_flat = vec![0.0];
            k_flat.extend((0..n).map(|i| i as f64));
            k_flat.push(n.saturating_sub(1) as f64);
            let cps: Vec<Point3> = points.iter().map(|&p| p3(p)).collect();
            let (uk, mults) = unique_knots_and_multiplicities(&k_flat);
            let bsc = BSplineCurve::create_canonical(&CanonicalData {
                dim: 3,
                degree: 1,
                control_points: &cps,
                form: BSplineCurveForm::PolylineForm,
                knot_multiplicities: &mults,
                knots: &uk,
                knot_type: KnotType::Unspecified,
                weights: None,
                trim: None,
            })?;
            Ok((
                Curve::from(bsc),
                Extent1d::from_raw(interval[0], interval[1]),
            ))
        }
        b::Curve::Spline { knots, points } => {
            let order = knots
                .len()
                .checked_sub(points.len())
                .filter(|&o| o >= 2)
                .ok_or(BridgeError::Invalid("spline order"))?;
            let cps: Vec<Point3> = points.iter().map(|&p| p3(p)).collect();
            let (uk, mults) = unique_knots_and_multiplicities(knots);
            let bsc = BSplineCurve::create_canonical(&CanonicalData {
                dim: 3,
                degree: order - 1,
                control_points: &cps,
                form: BSplineCurveForm::Unspecified,
                knot_multiplicities: &mults,
                knots: &uk,
                knot_type: KnotType::Unspecified,
                weights: None,
                trim: None,
            })?;
            Ok((
                Curve::from(bsc),
                Extent1d::from_raw(interval[0], interval[1]),
            ))
        }
        b::Curve::RationalSpline { knots, homogeneous } => {
            let order = knots
                .len()
                .checked_sub(homogeneous.len())
                .filter(|&o| o >= 2)
                .ok_or(BridgeError::Invalid("spline order"))?;
            let cps: Vec<Point3> = homogeneous
                .iter()
                .map(|h| Point3::new(h[0] / h[3], h[1] / h[3], h[2] / h[3]))
                .collect();
            let weights: Vec<f64> = homogeneous.iter().map(|h| h[3]).collect();
            let (uk, mults) = unique_knots_and_multiplicities(knots);
            let bsc = BSplineCurve::create_canonical(&CanonicalData {
                dim: 3,
                degree: order - 1,
                control_points: &cps,
                form: BSplineCurveForm::Unspecified,
                knot_multiplicities: &mults,
                knots: &uk,
                knot_type: KnotType::Unspecified,
                weights: Some(&weights),
                trim: None,
            })?;
            Ok((
                Curve::from(bsc),
                Extent1d::from_raw(interval[0], interval[1]),
            ))
        }
        b::Curve::SurfaceIntersection { .. } => {
            Err(BridgeError::NotYet("traced intersection curves"))
        }
    }
}

pub(crate) mod analytic {
    use super::*;
    use solidcraft_geom::Vec3 as ScVec3;

    const NOT_ANALYTIC: BridgeError = BridgeError::NotYet(
        "revolved surfaces other than planes, cylinders, cones, spheres and tori",
    );

    pub(super) fn analytic(brep: &b::Brep) -> R<Cow<'_, b::Brep>> {
        let size = extent(brep);
        let tol = brep.tolerance.max(1e-9 * size);
        let mut out: Option<b::Brep> = None;
        let mut turned = Vec::new();
        for (i, f) in brep.faces.iter().enumerate() {
            let found = match &f.surface {
                b::Surface::PlacedRevolution {
                    profile,
                    origin,
                    axis,
                    matrix,
                    forward,
                } => match Revolution::new(profile, *origin, *axis, matrix, *forward)
                    .and_then(|r| r.analytic(f.domain))
                {
                    Ok(found) => Some(found),
                    Err(BridgeError::NotYet(_)) => None,
                    Err(e) => return Err(e),
                },
                _ => None,
            };
            let Some((mut surface, mut range)) = found else {
                continue;
            };
            if !matches!(surface, b::Surface::Plane { .. }) {
                turn_frame(&mut surface, range[0][0]);
                range[0] = [0.0, range[0][1] - range[0][0]];
            }
            if matches!(surface, b::Surface::Torus { .. }) {
                range[1] = first_turn(range[1]);
            }
            let flip = agree((&f.surface, f.domain), (&surface, range), tol)?;
            let face = out
                .get_or_insert_with(|| brep.clone())
                .faces
                .get_mut(i)
                .ok_or(BridgeError::Invalid("face"))?;
            face.surface = surface;
            face.domain = range;
            if flip {
                turned.push(i);
            }
        }
        let Some(mut out) = out else {
            return Ok(Cow::Borrowed(brep));
        };
        for i in turned {
            turn_over(&mut out, i)?;
        }
        Ok(Cow::Owned(out))
    }

    pub(super) fn turn_frame_in_place(frame: &mut b::Frame, angle: f64) {
        let (x, y) = (sc_v3(frame.x_axis), sc_v3(frame.y_axis));
        let (sin, cos) = angle.sin_cos();
        frame.x_axis = arr(x * cos + y * sin);
        frame.y_axis = arr(y * cos - x * sin);
    }

    fn turn_frame(s: &mut b::Surface, angle: f64) {
        if let b::Surface::Cylinder { frame, .. }
        | b::Surface::Cone { frame, .. }
        | b::Surface::Sphere { frame, .. }
        | b::Surface::Torus { frame, .. } = s
        {
            turn_frame_in_place(frame, angle);
        }
    }

    pub(crate) fn first_turn([a, z]: [f64; 2]) -> [f64; 2] {
        let shift = (a / TAU).floor() * TAU;
        [a - shift, z - shift]
    }

    fn extruded_circle(
        u_knots: &[f64],
        v_knots: &[f64],
        net: &[Vec<[f64; 4]>],
        domain: [[f64; 2]; 2],
        tol: f64,
    ) -> Option<(b::Surface, [[f64; 2]; 2])> {
        let linear = |k: &[f64]| k.len() == 4 && k[0] == k[1] && k[2] == k[3] && k[1] < k[2];
        let (rows, arc_knots, arc_range, line_knots, line_range): (
            [Vec<[f64; 4]>; 2],
            &[f64],
            [f64; 2],
            &[f64],
            [f64; 2],
        ) = if net.iter().all(|r| r.len() == 2) && linear(v_knots) && !linear(u_knots) {
            (
                [
                    net.iter().map(|r| r[0]).collect(),
                    net.iter().map(|r| r[1]).collect(),
                ],
                u_knots,
                domain[0],
                v_knots,
                domain[1],
            )
        } else if net.len() == 2 && linear(u_knots) && !linear(v_knots) {
            (
                [net.first()?.clone(), net.get(1)?.clone()],
                v_knots,
                domain[1],
                u_knots,
                domain[0],
            )
        } else {
            return None;
        };
        let cart = |h: &[f64; 4]| ScVec3::new(h[0] / h[3], h[1] / h[3], h[2] / h[3]);
        let (first, second) = (&rows[0], &rows[1]);
        let d = cart(second.first()?) - cart(first.first()?);
        let alike = first.iter().zip(second).all(|(p, q)| {
            (p[3] - q[3]).abs() <= 1e-12 * p[3].abs() && (cart(q) - cart(p) - d).len() <= tol
        });
        if !alike || first.len() != second.len() || !(d.len() > tol) {
            return None;
        }
        let arc = CurveEval::new(&b::Curve::RationalSpline {
            knots: arc_knots.to_vec(),
            homogeneous: first.clone(),
        })
        .ok()?;
        let (center, radius, normal) = circle(&arc, arc_range)?;
        let z = d.normalized()?;
        if normal.cross(z).len() > 1e-9 {
            return None;
        }
        let x = (arc.point(arc_range[0])? - center).normalized()?;
        let y = z.cross(x);
        let mut angle = 0.0;
        let mut prev = 0.0;
        for i in 1..=32 {
            let q = arc
                .point(arc_range[0] + (arc_range[1] - arc_range[0]) * f64::from(i) / 32.0)?
                - center;
            let mut a = q.dot(y).atan2(q.dot(x));
            while a - prev > PI {
                a -= TAU;
            }
            while a - prev < -PI {
                a += TAU;
            }
            prev = a;
            angle = a;
        }
        let height = |s: f64| (s - line_knots[1]) / (line_knots[2] - line_knots[1]) * d.len();
        let sorted = |a: f64, b: f64| if a <= b { [a, b] } else { [b, a] };
        let frame = b::Frame {
            origin: arr(center),
            x_axis: arr(x),
            y_axis: arr(y),
        };
        Some((
            b::Surface::Cylinder { frame, radius },
            [
                sorted(0.0, angle),
                sorted(height(line_range[0]), height(line_range[1])),
            ],
        ))
    }

    fn extent(brep: &b::Brep) -> f64 {
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for v in &brep.vertices {
            for k in 0..3 {
                lo[k] = lo[k].min(v.point[k]);
                hi[k] = hi[k].max(v.point[k]);
            }
        }
        let d = (0..3).map(|k| (hi[k] - lo[k]).powi(2)).sum::<f64>().sqrt();
        if d.is_finite() && d > 0.0 { d } else { 1.0 }
    }

    fn sc_v3(p: [f64; 3]) -> ScVec3 {
        ScVec3::new(p[0], p[1], p[2])
    }
    fn arr(v: ScVec3) -> [f64; 3] {
        [v.x, v.y, v.z]
    }

    struct Revolution {
        profile: CurveEval,
        line: Option<[ScVec3; 2]>,
        o: ScVec3,
        z: ScVec3,
        forward: bool,
    }

    impl Revolution {
        fn new(
            profile: &b::Curve,
            origin: [f64; 3],
            axis: [f64; 3],
            m: &[[f64; 4]; 4],
            forward: bool,
        ) -> R<Revolution> {
            let col = |c: usize| ScVec3::new(m[c][0], m[c][1], m[c][2]);
            let (c0, c1, c2, t) = (col(0), col(1), col(2), col(3));
            let unit = |v: ScVec3| (v.len() - 1.0).abs() <= 1e-9;
            if !(m[0][3] == 0.0 && m[1][3] == 0.0 && m[2][3] == 0.0 && m[3][3] == 1.0)
                || !(unit(c0) && unit(c1) && unit(c2))
                || c0.dot(c1).abs() > 1e-9
                || c0.dot(c2).abs() > 1e-9
                || c1.dot(c2).abs() > 1e-9
            {
                return Err(BridgeError::NotYet(
                    "revolved surfaces placed by a matrix that is no rigid motion",
                ));
            }
            let det = c0.dot(c1.cross(c2));
            let place = |p: ScVec3| c0 * p.x + c1 * p.y + c2 * p.z + t;
            let turn = |d: ScVec3| c0 * d.x + c1 * d.y + c2 * d.z;
            let z = turn(sc_v3(axis))
                .normalized()
                .ok_or(BridgeError::Invalid("revolution axis"))?
                * det.signum();
            let placed_curve = |c: &b::Curve| -> R<b::Curve> {
                let p = |q: [f64; 3]| arr(place(sc_v3(q)));
                Ok(match c {
                    b::Curve::Line { start, end } => b::Curve::Line {
                        start: p(*start),
                        end: p(*end),
                    },
                    b::Curve::Circle { frame, radius } => b::Curve::Circle {
                        frame: b::Frame {
                            origin: p(frame.origin),
                            x_axis: arr(turn(sc_v3(frame.x_axis))),
                            y_axis: arr(turn(sc_v3(frame.y_axis))),
                        },
                        radius: *radius,
                    },
                    b::Curve::Spline { knots, points } => b::Curve::Spline {
                        knots: knots.clone(),
                        points: points.iter().map(|q| p(*q)).collect(),
                    },
                    b::Curve::RationalSpline { knots, homogeneous } => b::Curve::RationalSpline {
                        knots: knots.clone(),
                        homogeneous: homogeneous
                            .iter()
                            .map(|h| {
                                let w = h[3];
                                let q = place(ScVec3::new(h[0], h[1], h[2]) * (1.0 / w));
                                [q.x * w, q.y * w, q.z * w, w]
                            })
                            .collect(),
                    },
                    _ => return Err(NOT_ANALYTIC),
                })
            };
            let world = placed_curve(profile)?;
            let line = match &world {
                b::Curve::Line { start, end } => Some([sc_v3(*start), sc_v3(*end)]),
                _ => None,
            };
            let profile =
                CurveEval::new(&world).map_err(|_| BridgeError::Invalid("revolution profile"))?;
            Ok(Revolution {
                profile,
                line,
                o: place(sc_v3(origin)),
                z,
                forward,
            })
        }

        fn ranges(&self, domain: [[f64; 2]; 2]) -> ([f64; 2], [f64; 2]) {
            if self.forward {
                (domain[0], domain[1])
            } else {
                (domain[1], domain[0])
            }
        }

        fn split(&self, p: ScVec3) -> (ScVec3, f64, ScVec3) {
            let h = (p - self.o).dot(self.z);
            let foot = self.o + self.z * h;
            (foot, h, p - foot)
        }

        fn analytic(&self, domain: [[f64; 2]; 2]) -> R<(b::Surface, [[f64; 2]; 2])> {
            let (t, angle) = self.ranges(domain);
            let at = |s: f64| {
                self.profile
                    .point(s)
                    .ok_or(BridgeError::Invalid("revolution profile"))
            };
            let samples: Vec<ScVec3> = (0..=16)
                .map(|i| at(t[0] + (t[1] - t[0]) * f64::from(i) / 16.0))
                .collect::<R<_>>()?;
            let far = samples
                .iter()
                .map(|p| self.split(*p).2)
                .fold(
                    ScVec3::ZERO,
                    |best, r| if r.len() > best.len() { r } else { best },
                );
            let x = far.normalized().ok_or(NOT_ANALYTIC)?;
            let y = self.z.cross(x);
            let frame = |o: ScVec3| b::Frame {
                origin: arr(o),
                x_axis: arr(x),
                y_axis: arr(y),
            };
            let sorted = |a: f64, b: f64| if a <= b { [a, b] } else { [b, a] };
            if let Some([s, e]) = self.line {
                let d = e - s;
                let len = d.len();
                if !(len > 0.0) {
                    return Err(NOT_ANALYTIC);
                }
                if (s - self.o).dot(y).abs() > 1e-9 * len.max(1.0) || d.dot(y).abs() > 1e-9 * len {
                    return Err(NOT_ANALYTIC);
                }
                let (foot, _, _) = self.split(s);
                let height = |p: ScVec3| (p - foot).dot(self.z);
                let radial = |p: ScVec3| (p - foot).dot(x);
                let (h0, h1) = (height(at(t[0])?), height(at(t[1])?));
                let (dh, dr) = (d.dot(self.z), d.dot(x));
                if dh.abs() <= 1e-12 * len {
                    let (ra, rb) = (radial(at(t[0])?), radial(at(t[1])?));
                    let mut angles = vec![angle[0], angle[1]];
                    let mut q = (angle[0] / FRAC_PI_2).ceil() * FRAC_PI_2;
                    while q < angle[1] && angles.len() < 16 {
                        angles.push(q);
                        q += FRAC_PI_2;
                    }
                    let corners: Vec<[f64; 2]> = angles
                        .iter()
                        .flat_map(|a| [ra, rb].map(|r| [r * a.cos(), r * a.sin()]))
                        .collect();
                    let lo = |k: usize| corners.iter().map(|c| c[k]).fold(f64::INFINITY, f64::min);
                    let hi = |k: usize| {
                        corners
                            .iter()
                            .map(|c| c[k])
                            .fold(f64::NEG_INFINITY, f64::max)
                    };
                    let plane = b::Surface::Plane {
                        origin: arr(foot),
                        u_axis: arr(x),
                        v_axis: arr(y),
                    };
                    return Ok((plane, [[lo(0), hi(0)], [lo(1), hi(1)]]));
                }
                let r0 = radial(s) - dr / dh * height(s);
                let v = sorted(h0, h1);
                if dr.abs() <= 1e-12 * len {
                    if !(r0 > 0.0) {
                        return Err(NOT_ANALYTIC);
                    }
                    return Ok((
                        b::Surface::Cylinder {
                            frame: frame(foot),
                            radius: r0,
                        },
                        [angle, v],
                    ));
                }
                let r = |h: f64| r0 + h * dr / dh;
                if r(v[0]) < 0.0 || r(v[1]) < 0.0 {
                    return Err(NOT_ANALYTIC);
                }
                let cone = b::Surface::Cone {
                    frame: frame(foot),
                    radius_at_origin: r0,
                    half_angle: (dr / dh).atan(),
                };
                return Ok((cone, [angle, v]));
            }
            let (center, radius, normal) = circle(&self.profile, t).ok_or(NOT_ANALYTIC)?;
            let slack = 1e-9 * radius.max(1.0);
            if normal.dot(self.z).abs() > 1e-9
                || normal.dot(x).abs() > 1e-9
                || (center - self.o).dot(y).abs() > slack
            {
                return Err(NOT_ANALYTIC);
            }
            let (foot, _, off) = self.split(center);
            let meridian = |p: ScVec3| {
                let q = p - center;
                q.dot(self.z).atan2(q.dot(x))
            };
            let mut angles = Vec::with_capacity(samples.len());
            for p in &samples {
                let mut a = meridian(*p);
                if let Some(prev) = angles.last().copied() {
                    while a - prev > PI {
                        a -= TAU;
                    }
                    while a - prev < -PI {
                        a += TAU;
                    }
                }
                angles.push(a);
            }
            let (first, last) = (
                angles.first().copied().ok_or(NOT_ANALYTIC)?,
                angles.last().copied().ok_or(NOT_ANALYTIC)?,
            );
            if off.len() <= slack {
                let v = sorted(first, last);
                if v[0] < -FRAC_PI_2 - 1e-9 || v[1] > FRAC_PI_2 + 1e-9 {
                    return Err(NOT_ANALYTIC);
                }
                let v = [v[0].max(-FRAC_PI_2), v[1].min(FRAC_PI_2)];
                return Ok((
                    b::Surface::Sphere {
                        frame: frame(foot),
                        radius,
                    },
                    [angle, v],
                ));
            }
            let major = off.dot(x);
            if !(major > radius) {
                return Err(NOT_ANALYTIC);
            }
            let torus = b::Surface::Torus {
                frame: frame(foot),
                major_radius: major,
                minor_radius: radius,
            };
            Ok((torus, [angle, sorted(first, last)]))
        }
    }

    fn analytic_uv(s: &b::Surface, p: ScVec3, near: [f64; 2]) -> Option<[f64; 2]> {
        let unwrap = |mut a: f64, n: f64| {
            while a - n > PI {
                a -= TAU;
            }
            while a - n < -PI {
                a += TAU;
            }
            a
        };
        if let b::Surface::Plane {
            origin,
            u_axis,
            v_axis,
        } = s
        {
            let q = p - sc_v3(*origin);
            return Some([q.dot(sc_v3(*u_axis)), q.dot(sc_v3(*v_axis))]);
        }
        let (frame, round) = match s {
            b::Surface::Cylinder { frame, .. }
            | b::Surface::Cone { frame, .. }
            | b::Surface::Sphere { frame, .. } => (frame, 0.0),
            b::Surface::Torus {
                frame,
                major_radius,
                ..
            } => (frame, *major_radius),
            _ => return None,
        };
        let (x, y) = (sc_v3(frame.x_axis), sc_v3(frame.y_axis));
        let z = x.cross(y);
        let q = p - sc_v3(frame.origin);
        let (qx, qy, qz) = (q.dot(x), q.dot(y), q.dot(z));
        let u = unwrap(qy.atan2(qx), near[0]);
        let r = qx.hypot(qy);
        Some(match s {
            b::Surface::Sphere { .. } => [u, qz.atan2(r)],
            b::Surface::Torus { .. } => [u, unwrap(qz.atan2(r - round), near[1])],
            _ => [u, qz],
        })
    }

    fn agree(
        (record, domain): (&b::Surface, [[f64; 2]; 2]),
        (analytic, range): (&b::Surface, [[f64; 2]; 2]),
        tol: f64,
    ) -> R<bool> {
        let bad = BridgeError::Invalid("a surface its analytic form does not match");
        let s = SurfaceEval::new(record).map_err(|_| bad.clone())?;
        let a = SurfaceEval::new(analytic).map_err(|_| bad.clone())?;
        let [[u0, u1], [v0, v1]] = domain;
        let near = [
            0.5 * (range[0][0] + range[0][1]),
            0.5 * (range[1][0] + range[1][1]),
        ];
        let normals = |u: f64, w: f64| -> R<f64> {
            let (p, du, dv) = s.derivs(u, w).ok_or(bad.clone())?;
            let uv = analytic_uv(analytic, p, near).ok_or(bad.clone())?;
            let (q, au, av) = a.derivs(uv[0], uv[1]).ok_or(bad.clone())?;
            if !((p - q).len() <= tol) {
                return Err(bad.clone());
            }
            Ok(du.cross(dv).dot(au.cross(av)))
        };
        for i in 0..=4 {
            for j in 0..=4 {
                normals(
                    u0 + (u1 - u0) * f64::from(i) / 4.0,
                    v0 + (v1 - v0) * f64::from(j) / 4.0,
                )?;
            }
        }
        let d = normals(0.5 * (u0 + u1), 0.5 * (v0 + v1))?;
        if !(d.abs() > 0.0) {
            return Err(bad);
        }
        Ok(d < 0.0)
    }

    fn circle(c: &CurveEval, t: [f64; 2]) -> Option<(ScVec3, f64, ScVec3)> {
        let at = |s: f64| c.point(t[0] + (t[1] - t[0]) * s);
        let (p, q, r) = (at(0.0)?, at(1.0 / 3.0)?, at(2.0 / 3.0)?);
        let (u, w) = (q - p, r - p);
        let normal = u.cross(w);
        let nn = normal.dot(normal);
        if !(nn > 0.0) {
            return None;
        }
        let center = p + (normal.cross(u) * w.dot(w) + w.cross(normal) * u.dot(u)) * (0.5 / nn);
        let radius = (p - center).len();
        let k = normal.normalized()?;
        let slack = 1e-9 * radius.max(1.0);
        for i in 0..=64 {
            let s = at(f64::from(i) / 64.0)?;
            if ((s - center).len() - radius).abs() > slack || (s - center).dot(k).abs() > slack {
                return None;
            }
        }
        Some((center, radius, k))
    }

    fn turn_over(brep: &mut b::Brep, face: usize) -> R<()> {
        let index = |x: usize| u32::try_from(x).map_err(|_| BridgeError::Invalid("index"));
        let fi = index(face)?;
        let f = brep
            .faces
            .get_mut(face)
            .ok_or(BridgeError::Invalid("face"))?;
        let mut moved: HashMap<(u32, u32), (u32, bool)> = HashMap::new();
        for (li, l) in f.loops.iter_mut().enumerate() {
            if let b::Loop::Edges { edges } = l {
                let n = edges.len();
                edges.reverse();
                for (p, e) in edges.iter_mut().enumerate() {
                    e.forward = !e.forward;
                    moved.insert((index(li)?, index(n - 1 - p)?), (index(p)?, e.forward));
                }
            }
        }
        for e in &mut brep.edges {
            for u in e.uses.iter_mut().filter(|u| u.face == fi) {
                let &(position, forward) = moved
                    .get(&(u.r#loop, u.position))
                    .ok_or(BridgeError::Invalid("edge use"))?;
                u.position = position;
                u.upper = !forward;
            }
        }
        for s in &mut brep.shells {
            if let b::Shell::Faces { faces, .. } = s {
                for o in faces.iter_mut().filter(|o| o.face == fi) {
                    o.forward = !o.forward;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solidcore_mesh::{TessellateOptions, tessellate};

    const FIXTURES: &[(&str, &str)] = &[
        (
            "occt-rounded-housing-mm",
            include_str!(
                "../../../../fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step"
            ),
        ),
        (
            "occt-cylinder-mm",
            include_str!(
                "../../../../fixtures/geometry/independent-exporters/occt-cylinder-mm.step"
            ),
        ),
        (
            "occt-through-hole-mm",
            include_str!(
                "../../../../fixtures/geometry/independent-exporters/occt-through-hole-mm.step"
            ),
        ),
        (
            "occt-narrow-pocket-mm",
            include_str!(
                "../../../../fixtures/geometry/independent-exporters/occt-narrow-pocket-mm.step"
            ),
        ),
        (
            "occt-tapered-fitting-mm",
            include_str!(
                "../../../../fixtures/geometry/independent-exporters/occt-tapered-fitting-mm.step"
            ),
        ),
        (
            "occt-ribbed-bracket-mm",
            include_str!(
                "../../../../fixtures/geometry/independent-exporters/occt-ribbed-bracket-mm.step"
            ),
        ),
        (
            "occt-spline-housing-mm",
            include_str!(
                "../../../../fixtures/geometry/independent-exporters/occt-spline-housing-mm.step"
            ),
        ),
        (
            "perforated-plate",
            include_str!("../../../../fixtures/geometry/perforated-plate.step"),
        ),
    ];

    #[test]
    fn phase0_step_to_solidcore_tessellate_and_section_all_6_fixtures() {
        for &(name, step_text) in FIXTURES {
            let mut ex = parse_step_exchange(step_text)
                .unwrap_or_else(|e| panic!("{name}: p21 parse failed: {e}"));
            let cancel = std::sync::atomic::AtomicBool::new(false);
            let ast = step_p21::parser::parse(step_text).unwrap();
            let entities = crate::import::ast::Entities::new(&ast.data[0], &cancel).unwrap();
            let roots = entities.named("MANIFOLD_SOLID_BREP", &cancel).unwrap();
            assert_eq!(roots.len(), 1, "{name}: expected 1 solid");
            let (unit, uncertainty) =
                crate::import::units::resolve(&entities, roots[0], &cancel).unwrap();
            let mut context = solidcraft_step::units::Context::default();
            context.units.len = unit.scale_to_mm();
            context.uncertainty = uncertainty;
            let solid = crate::import::closed_spline_decode::read_solid(
                &mut ex,
                roots[0],
                context,
                crate::import::MAX_QUERY_PROOF_BYTES,
                &cancel,
            )
            .unwrap_or_else(|e| panic!("{name}: STEP extraction failed: {e}"));
            assert!(!solid.sheet, "{name}: expected closed solid");

            let mut k = Kernel::default();
            let brep = from_schema4_brep(&mut k, &solid.brep, false)
                .unwrap_or_else(|e| panic!("{name}: from_schema4_brep failed: {e}"));

            let faces = solidcore::topology::brep::p1::faces(&brep, None).unwrap();
            let edges = solidcore::topology::brep::p1::edges(&brep, None).unwrap();
            let _vertices = solidcore::topology::brep::p1::vertices(&brep, None).unwrap();
            assert_eq!(faces.len(), solid.brep.faces.len(), "{name}: face count");

            // 1. Tessellate with solidcore-mesh and check watertightness.
            let opts = TessellateOptions::default();
            let mesh = tessellate(&k, &brep, &opts)
                .unwrap_or_else(|e| panic!("{name}: tessellate failed: {e}"));
            assert!(
                mesh.failures.is_empty(),
                "{name}: mesh failures: {:?}",
                mesh.failures
            );
            let report = mesh.edge_report();
            assert!(
                report.is_watertight(),
                "{name}: mesh is not watertight: open={} non_manifold={} inconsistent={}",
                report.open.len(),
                report.non_manifold.len(),
                report.inconsistent.len()
            );

            // 2. Section at midpoint Z using Surface::global_surface_intersect.
            let mut z_min = f64::INFINITY;
            let mut z_max = f64::NEG_INFINITY;
            for v in &solid.brep.vertices {
                z_min = z_min.min(v.point[2]);
                z_max = z_max.max(v.point[2]);
            }
            let z_mid = 0.5 * (z_min + z_max);
            let plane_ax = placement([0.0, 0.0, z_mid], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]).unwrap();
            let section_plane = Surface::from(Plane::create_canonical(&plane_ax).unwrap());
            let plane_dom = Extent2d::from_bounds(-10_000.0, -10_000.0, 10_000.0, 10_000.0);
            let mut total_curves = 0usize;
            for &fid in &faces {
                let face = brep.face(fid).unwrap();
                let surf = face.surface.as_ref().unwrap();
                let dom = face.uv_domain;
                let res = surf
                    .global_surface_intersect(
                        &k,
                        &dom,
                        &section_plane,
                        &plane_dom,
                        [false, false],
                        Some(brep.tolerance()),
                        None,
                    )
                    .unwrap_or_else(|e| {
                        panic!(
                            "{name} face {}: global_surface_intersect failed: {e}",
                            fid.index()
                        )
                    });
                for c in &res.curves_3d {
                    total_curves += 1;
                    let ivl = c.natural_interval();
                    let _p_start = c.evaluate_point(&k, ivl.min()).unwrap();
                    let _p_end = c.evaluate_point(&k, ivl.max()).unwrap();
                }
            }
            assert!(
                total_curves >= 1,
                "{name}: expected >= 1 plane-face intersection curves at z={z_mid}, got {total_curves}"
            );
            eprintln!(
                "{name}: OK! faces={} edges={} tris={} watertight={} section_curves={total_curves}",
                faces.len(),
                edges.len(),
                mesh.triangles.len(),
                report.is_watertight(),
            );
        }
    }
}
