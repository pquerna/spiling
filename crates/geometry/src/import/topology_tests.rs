// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! STEP topology representations from real exporters, authored here entity by
//! entity: solids with void shells, arcs between vertices nearer than the
//! file's uncertainty, and faces whose seam is not at their surface's zero.
//! Each is imported and sliced through the tolerant section model.

use super::*;
use crate::from_step_bytes_with_cancel;
use crate::tolerant::{TolerantModel, TolerantPolicy};
use std::f64::consts::PI;
use std::fmt::Write as _;

type V = [f64; 3];

/// Writes a STEP file one entity at a time, in millimetres and radians.
#[derive(Default)]
struct W {
    s: String,
    n: u64,
}

impl W {
    fn add(&mut self, body: impl AsRef<str>) -> u64 {
        self.n += 1;
        let _ = writeln!(self.s, "#{}={};", self.n, body.as_ref());
        self.n
    }
    fn pt(&mut self, p: V) -> u64 {
        self.add(format!(
            "CARTESIAN_POINT('',({:.17E},{:.17E},{:.17E}))",
            p[0], p[1], p[2]
        ))
    }
    fn dir(&mut self, d: V) -> u64 {
        self.add(format!(
            "DIRECTION('',({:.17E},{:.17E},{:.17E}))",
            d[0], d[1], d[2]
        ))
    }
    fn axis(&mut self, o: V, z: V, x: V) -> u64 {
        let (o, z, x) = (self.pt(o), self.dir(z), self.dir(x));
        self.add(format!("AXIS2_PLACEMENT_3D('',#{o},#{z},#{x})"))
    }
    fn vertex(&mut self, p: V) -> u64 {
        let p = self.pt(p);
        self.add(format!("VERTEX_POINT('',#{p})"))
    }
    fn line(&mut self, a: (u64, V), b: (u64, V)) -> u64 {
        let d = [b.1[0] - a.1[0], b.1[1] - a.1[1], b.1[2] - a.1[2]];
        let m = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let (o, d) = (self.pt(a.1), self.dir(d.map(|x| x / m)));
        let v = self.add(format!("VECTOR('',#{d},{m:.17E})"));
        let l = self.add(format!("LINE('',#{o},#{v})"));
        self.add(format!("EDGE_CURVE('',#{},#{},#{l},.T.)", a.0, b.0))
    }
    /// A circle about +z through `centre`, from `a` to `b` counter-clockwise.
    fn arc(&mut self, centre: V, r: f64, a: u64, b: u64) -> u64 {
        let frame = self.axis(centre, [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
        let c = self.add(format!("CIRCLE('',#{frame},{r:.17E})"));
        self.add(format!("EDGE_CURVE('',#{a},#{b},#{c},.T.)"))
    }
    fn oe(&mut self, edge: u64, forward: bool) -> u64 {
        self.add(format!(
            "ORIENTED_EDGE('',*,*,#{edge},{})",
            if forward { ".T." } else { ".F." }
        ))
    }
    fn face(&mut self, loops: &[&[(u64, bool)]], surface: u64, same_sense: bool) -> u64 {
        let mut bounds = Vec::new();
        for (i, uses) in loops.iter().enumerate() {
            let oes: Vec<String> = uses
                .iter()
                .map(|&(e, f)| format!("#{}", self.oe(e, f)))
                .collect();
            let l = self.add(format!("EDGE_LOOP('',({}))", oes.join(",")));
            let kind = if i == 0 {
                "FACE_OUTER_BOUND"
            } else {
                "FACE_BOUND"
            };
            bounds.push(format!("#{}", self.add(format!("{kind}('',#{l},.T.)"))));
        }
        self.add(format!(
            "ADVANCED_FACE('',({}),#{surface},{})",
            bounds.join(","),
            if same_sense { ".T." } else { ".F." }
        ))
    }
    fn plane(&mut self, o: V, z: V, x: V) -> u64 {
        let a = self.axis(o, z, x);
        self.add(format!("PLANE('',#{a})"))
    }
    /// The whole file, `solid` its one representation item.
    fn file(mut self, solid: u64, uncertainty: f64) -> Vec<u8> {
        let l = self.add("(LENGTH_UNIT()NAMED_UNIT(*)SI_UNIT(.MILLI.,.METRE.))");
        let a = self.add("(NAMED_UNIT(*)PLANE_ANGLE_UNIT()SI_UNIT($,.RADIAN.))");
        let s = self.add("(NAMED_UNIT(*)SI_UNIT($,.STERADIAN.)SOLID_ANGLE_UNIT())");
        let u = self.add(format!(
            "UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE({uncertainty:.17E}),#{l},'distance_accuracy_value','')"
        ));
        let c = self.add(format!(
            "(GEOMETRIC_REPRESENTATION_CONTEXT(3)GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT((#{u}))GLOBAL_UNIT_ASSIGNED_CONTEXT((#{l},#{a},#{s}))REPRESENTATION_CONTEXT('',''))"
        ));
        let rep = self.add(format!(
            "ADVANCED_BREP_SHAPE_REPRESENTATION('',(#{solid}),#{c})"
        ));
        let ctx = self.add("APPLICATION_CONTEXT('automotive design')");
        let pctx = self.add(format!("PRODUCT_CONTEXT('',#{ctx},'mechanical')"));
        let product = self.add(format!("PRODUCT('part','part','',(#{pctx}))"));
        let formation = self.add(format!("PRODUCT_DEFINITION_FORMATION('','',#{product})"));
        let dctx = self.add(format!(
            "PRODUCT_DEFINITION_CONTEXT('part definition',#{ctx},'design')"
        ));
        let definition = self.add(format!(
            "PRODUCT_DEFINITION('design','',#{formation},#{dctx})"
        ));
        let shape = self.add(format!("PRODUCT_DEFINITION_SHAPE('','',#{definition})"));
        self.add(format!("SHAPE_DEFINITION_REPRESENTATION(#{shape},#{rep})"));
        format!(
            "ISO-10303-21;\nHEADER;\nFILE_DESCRIPTION((''),'2;1');\nFILE_NAME('topology.step','2026-01-01T00:00:00',('Spiling contributors'),('Spiling'),'','','OSL-3.0');\nFILE_SCHEMA(('AUTOMOTIVE_DESIGN'));\nENDSEC;\nDATA;\n{}ENDSEC;\nEND-ISO-10303-21;\n",
            self.s
        )
        .into_bytes()
    }
}

/// The six faces of the box from `lo` to `hi`, normals out of it (or, `inward`, into it).
fn box_faces(w: &mut W, lo: V, hi: V, inward: bool) -> Vec<u64> {
    let corner = |i: usize| -> V {
        [
            if i & 1 == 0 { lo[0] } else { hi[0] },
            if i & 2 == 0 { lo[1] } else { hi[1] },
            if i & 4 == 0 { lo[2] } else { hi[2] },
        ]
    };
    let vs: Vec<(u64, V)> = (0..8).map(|i| (w.vertex(corner(i)), corner(i))).collect();
    let mut edges = std::collections::BTreeMap::new();
    // Each face's corners counter-clockwise about its outward normal.
    let quads: [([usize; 4], usize, f64); 6] = [
        ([0, 2, 3, 1], 2, -1.0),
        ([4, 5, 7, 6], 2, 1.0),
        ([0, 1, 5, 4], 1, -1.0),
        ([2, 6, 7, 3], 1, 1.0),
        ([0, 4, 6, 2], 0, -1.0),
        ([1, 3, 7, 5], 0, 1.0),
    ];
    let mut faces = Vec::new();
    for (q, axis, sign) in quads {
        let mut uses = Vec::new();
        for k in 0..4 {
            let (a, b) = (q[k], q[(k + 1) % 4]);
            let key = (a.min(b), a.max(b));
            let e = match edges.get(&key) {
                Some(&e) => e,
                None => {
                    let e = w.line(vs[key.0], vs[key.1]);
                    edges.insert(key, e);
                    e
                }
            };
            uses.push((e, a < b));
        }
        if inward {
            uses.reverse();
            for u in &mut uses {
                u.1 = !u.1;
            }
        }
        let mut z = [0.0; 3];
        z[axis] = sign;
        let x = if axis == 0 {
            [0.0, 1.0, 0.0]
        } else {
            [1.0, 0.0, 0.0]
        };
        let plane = w.plane(corner(q[0]), z, x);
        faces.push(w.face(&[&uses], plane, !inward));
    }
    faces
}

fn cancelled() -> AtomicBool {
    AtomicBool::new(false)
}

fn model(definition: &Definition) -> TolerantModel {
    let model = TolerantModel::build(definition, TolerantPolicy::default(), &cancelled()).unwrap();
    assert_eq!(model.failed_faces, 0);
    model
}

/// A 20 x 10 x 8 box with a 4 x 4 x 4 cavity, as AP214 writes it: BREP_WITH_VOIDS
/// with the cavity's own closed shell used reversed.
fn box_with_void(void_orientation: &str, void_shell: bool) -> Vec<u8> {
    let mut w = W::default();
    let outer = box_faces(&mut w, [0.0; 3], [20.0, 10.0, 8.0], false);
    let outer = w.add(format!(
        "CLOSED_SHELL('',({}))",
        outer
            .iter()
            .map(|f| format!("#{f}"))
            .collect::<Vec<_>>()
            .join(",")
    ));
    let inner = box_faces(&mut w, [8.0, 3.0, 2.0], [12.0, 7.0, 6.0], false);
    let inner = w.add(format!(
        "CLOSED_SHELL('',({}))",
        inner
            .iter()
            .map(|f| format!("#{f}"))
            .collect::<Vec<_>>()
            .join(",")
    ));
    let void = if void_shell {
        w.add(format!(
            "ORIENTED_CLOSED_SHELL('',*,#{inner},{void_orientation})"
        ))
    } else {
        inner
    };
    let solid = w.add(format!("BREP_WITH_VOIDS('cavity',#{outer},(#{void}))"));
    w.file(solid, 1e-6)
}

#[test]
fn a_void_shell_is_material_outside_it_and_inside_the_outer_shell() {
    let bytes = box_with_void(".F.", true);
    let bodies = structure::source_bodies(&bytes, &cancelled()).unwrap();
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0].name, "cavity");
    let definition = from_step_bytes_with_cancel(&bytes, &cancelled()).unwrap();
    assert_eq!(definition.faces().len(), 12);
    // The infinite region, the material and the cavity, each with its shells.
    assert_eq!(definition.brep().regions().len(), 3);
    // A void face's use is turned with its shell: its material side faces the cavity.
    let (outer, void) = definition.faces().split_at(6);
    for (a, b) in outer.iter().zip(void) {
        assert_ne!(a.orientation, b.orientation);
    }
    for face in &definition.faces()[6..] {
        let source = definition
            .original_source()
            .face(definition.face_ids()[definition.faces().iter().position(|f| f == face).unwrap()])
            .unwrap();
        assert_eq!(source.material_orientation(), face.orientation);
    }
    let model = model(&definition);
    for (z, cavity) in [(1.0, false), (4.0, true), (7.0, false)] {
        let section = model.section(z).unwrap();
        let mut areas: Vec<f64> = section.loops.iter().map(|l| l.area).collect();
        areas.sort_by(f64::total_cmp);
        let expected: &[f64] = if cavity { &[-16.0, 200.0] } else { &[200.0] };
        assert_eq!(areas.len(), expected.len(), "z {z}: {areas:?}");
        for (a, e) in areas.iter().zip(expected) {
            assert!((a - e).abs() < 1e-6, "z {z}: {areas:?}");
        }
    }
}

#[test]
fn a_void_must_be_an_oriented_closed_shell() {
    let error =
        from_step_bytes_with_cancel(&box_with_void(".F.", false), &cancelled()).unwrap_err();
    assert_eq!(error.code, GeometryErrorCode::InvalidGeometry);
}

#[test]
fn a_reversed_outer_shell_is_read_with_its_faces_turned() {
    // The box's faces written facing in, used through a reversed oriented shell.
    let mut w = W::default();
    let faces = box_faces(&mut w, [0.0; 3], [20.0, 10.0, 8.0], true);
    let shell = w.add(format!(
        "CLOSED_SHELL('',({}))",
        faces
            .iter()
            .map(|f| format!("#{f}"))
            .collect::<Vec<_>>()
            .join(",")
    ));
    let oriented = w.add(format!("ORIENTED_CLOSED_SHELL('',*,#{shell},.F.)"));
    let solid = w.add(format!("MANIFOLD_SOLID_BREP('',#{oriented})"));
    let definition = from_step_bytes_with_cancel(&w.file(solid, 1e-6), &cancelled()).unwrap();
    assert!(definition.faces().iter().all(|f| f.orientation));
    let section = model(&definition).section(4.0).unwrap();
    assert_eq!(section.loops.len(), 1);
    assert!((section.loops[0].area - 200.0).abs() < 1e-6);
}

/// A rod of radius 5 and height 8 with one side face and its seam at angle 0, its top circle
/// written as a long arc and a short one between vertices `gap` mm apart; the file's
/// uncertainty `uncertainty` mm.
fn rod_with_short_arc(gap: f64, uncertainty: f64) -> Vec<u8> {
    let (r, h) = (5.0, 8.0);
    let mut w = W::default();
    let b = w.vertex([r, 0.0, 0.0]);
    let t = w.vertex([r, 0.0, h]);
    let a = gap / r;
    let near = w.vertex([r * a.cos(), -r * a.sin(), h]);
    let bottom = w.arc([0.0; 3], r, b, b);
    let seam = w.line((b, [r, 0.0, 0.0]), (t, [r, 0.0, h]));
    let long = w.arc([0.0, 0.0, h], r, t, near);
    let short = w.arc([0.0, 0.0, h], r, near, t);
    let frame = w.axis([0.0; 3], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    let cylinder = w.add(format!("CYLINDRICAL_SURFACE('',#{frame},{r:.17E})"));
    let side = w.face(
        &[&[
            (bottom, true),
            (seam, true),
            (short, false),
            (long, false),
            (seam, false),
        ]],
        cylinder,
        true,
    );
    let floor = w.plane([0.0; 3], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    let floor = w.face(&[&[(bottom, false)]], floor, false);
    let roof = w.plane([0.0, 0.0, h], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    let roof = w.face(&[&[(long, true), (short, true)]], roof, true);
    let shell = w.add(format!("CLOSED_SHELL('',(#{floor},#{side},#{roof}))"));
    let solid = w.add(format!("MANIFOLD_SOLID_BREP('rod',#{shell})"));
    w.file(solid, uncertainty)
}

#[test]
fn an_arc_between_vertices_nearer_than_the_uncertainty_is_the_arc_its_loops_need() {
    // A 0.001 mm arc under a 0.01 mm uncertainty: its vertices are distinct but within the
    // tolerance, so it is either that arc or a full turn; the side's loop decides.
    for (gap, uncertainty) in [(1e-3, 1e-2), (1e-3, 1e-6), (1.0, 1e-2)] {
        let definition =
            from_step_bytes_with_cancel(&rod_with_short_arc(gap, uncertainty), &cancelled())
                .unwrap_or_else(|e| panic!("gap {gap}, uncertainty {uncertainty}: {e:?}"));
        assert_eq!(definition.faces().len(), 3);
        let section = model(&definition).section(4.0).unwrap();
        assert_eq!(section.loops.len(), 1);
        // Within the profile tolerance along the whole perimeter.
        let allowance = 2.0 * PI * 5.0 * TolerantPolicy::default().profile_mm;
        assert!(
            (section.loops[0].area - PI * 25.0).abs() < allowance,
            "{}",
            section.loops[0].area
        );
    }
}

/// A ring of half-disk section: the inner half of a torus tube (major radius 10, minor 2)
/// closed by the cylinder of radius 10 through its rims. Both faces' seams are at angle π of
/// their surfaces, and the torus face's loop is written starting on its seam.
fn half_torus_ring() -> Vec<u8> {
    let (big, small) = (10.0, 2.0);
    let mut w = W::default();
    let top = w.vertex([-big, 0.0, small]);
    let bottom = w.vertex([-big, 0.0, -small]);
    let rim_top = w.arc([0.0, 0.0, small], big, top, top);
    let rim_bottom = w.arc([0.0, 0.0, -small], big, bottom, bottom);
    // The torus seam: the inner half of the tube's circle at angle π, from the top rim down.
    let frame = w.axis([-big, 0.0, 0.0], [0.0, 1.0, 0.0], [-1.0, 0.0, 0.0]);
    let circle = w.add(format!("CIRCLE('',#{frame},{small:.17E})"));
    let tube_seam = w.add(format!("EDGE_CURVE('',#{top},#{bottom},#{circle},.T.)"));
    let wall_seam = w.line((bottom, [-big, 0.0, -small]), (top, [-big, 0.0, small]));
    let frame = w.axis([0.0; 3], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    let torus = w.add(format!(
        "TOROIDAL_SURFACE('',#{frame},{big:.17E},{small:.17E})"
    ));
    let frame = w.axis([0.0; 3], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);
    let cylinder = w.add(format!("CYLINDRICAL_SURFACE('',#{frame},{big:.17E})"));
    // Material inside the tube's inner half: both faces' normals are their surfaces' own,
    // out of the tube and away from the axis; each loop runs counter-clockwise in (u, v).
    let tube = w.face(
        &[&[
            (tube_seam, true),
            (rim_bottom, false),
            (tube_seam, false),
            (rim_top, true),
        ]],
        torus,
        true,
    );
    let wall = w.face(
        &[&[
            (rim_bottom, true),
            (wall_seam, true),
            (rim_top, false),
            (wall_seam, false),
        ]],
        cylinder,
        true,
    );
    let shell = w.add(format!("CLOSED_SHELL('',(#{tube},#{wall}))"));
    let solid = w.add(format!("MANIFOLD_SOLID_BREP('ring',#{shell})"));
    w.file(solid, 1e-6)
}

#[test]
fn a_face_whose_seam_is_not_at_its_surface_zero_meshes_and_slices() {
    let definition = from_step_bytes_with_cancel(&half_torus_ring(), &cancelled()).unwrap();
    assert_eq!(definition.faces().len(), 2);
    let model = model(&definition);
    // At height z the ring spans radii 10 - sqrt(4 - z^2) to 10.
    for z in [-1.5, 0.0, 1.0] {
        let section = model.section(z).unwrap();
        let mut areas: Vec<f64> = section.loops.iter().map(|l| l.area).collect();
        areas.sort_by(f64::total_cmp);
        let inner: f64 = 10.0 - (4.0f64 - z * z).sqrt();
        let expected = [-PI * inner * inner, PI * 100.0];
        assert_eq!(areas.len(), 2, "z {z}: {areas:?}");
        for (a, e) in areas.iter().zip(expected) {
            assert!(
                (a - e).abs() < 1e-2 * e.abs(),
                "z {z}: {areas:?} vs {expected:?}"
            );
        }
    }
}
