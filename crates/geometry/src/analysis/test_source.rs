// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Test operands authored as real SolidCraft source geometry and topology.
use crate::Definition;
use solidcraft_geom::brep as b;
use std::sync::atomic::AtomicBool;

mod analytic;
mod rational;

pub(crate) fn nurbs_cylinder() -> b::Brep {
    rational::nurbs_cylinder()
}
pub(crate) fn nurbs_torus() -> b::Brep {
    rational::nurbs_torus()
}
pub(crate) fn nurbs_torus_independent_parameters() -> SourceBrep {
    rational::nurbs_torus_independent_parameters()
}

pub(crate) fn ring_torus() -> b::Brep {
    analytic::ring_torus()
}
pub(crate) fn full_sphere() -> b::Brep {
    analytic::full_sphere()
}
pub(crate) fn sphere_belt() -> SourceBrep {
    analytic::sphere_belt()
}

pub(crate) struct SourceBrep {
    pub record: b::Brep,
    pub edge_curves: Vec<Option<solidcraft_step::geom::Curve>>,
    pub face_surfaces: Vec<Option<solidcraft_step::geom::Surface>>,
}

impl From<b::Brep> for SourceBrep {
    fn from(record: b::Brep) -> Self {
        Self {
            record,
            edge_curves: vec![],
            face_surfaces: vec![],
        }
    }
}

pub(crate) fn original_definition(source: impl Into<SourceBrep>) -> Definition {
    use solidcraft_step::p21::Param as P;
    use solidcraft_step::write::{GeomWriter, Writer, topology};
    let mut source = source.into();
    if source.edge_curves.is_empty()
        && source.face_surfaces.is_empty()
        && source
            .record
            .edges
            .iter()
            .any(|edge| edge.nurbs.is_some() && matches!(edge.curve, b::Curve::Circle { .. }))
    {
        // Analytic source circles keep their angular law. The portable
        // record's rational native charts are not authored analytic PCURVEs.
        let carriers = source
            .record
            .faces
            .iter()
            .map(|face| {
                topology::surface(&face.surface)
                    .expect("actual analytic source carrier")
                    .0
            })
            .collect();
        source = source_carriers(source.record, carriers);
    }
    let mut writer = Writer::new();
    let solids = if source.edge_curves.is_empty() && source.face_surfaces.is_empty() {
        topology::brep(
            &mut GeomWriter::new(&mut writer),
            &source.record,
            "analysis-source",
        )
        .expect("actual native source topology exports")
        .solids
    } else {
        write_authored_source(&mut writer, &source)
    };
    let mm = writer.add_complex(&[
        ("LENGTH_UNIT", vec![]),
        ("NAMED_UNIT", vec![P::Derived]),
        (
            "SI_UNIT",
            vec![P::Enum("MILLI".into()), P::Enum("METRE".into())],
        ),
    ]);
    let rad = writer.add_complex(&[
        ("NAMED_UNIT", vec![P::Derived]),
        ("PLANE_ANGLE_UNIT", vec![]),
        ("SI_UNIT", vec![P::Null, P::Enum("RADIAN".into())]),
    ]);
    let sr = writer.add_complex(&[
        ("NAMED_UNIT", vec![P::Derived]),
        ("SI_UNIT", vec![P::Null, P::Enum("STERADIAN".into())]),
        ("SOLID_ANGLE_UNIT", vec![]),
    ]);
    let uncertainty = writer.add(
        "UNCERTAINTY_MEASURE_WITH_UNIT",
        &[
            P::Typed(
                "LENGTH_MEASURE".into(),
                vec![P::Real(source.record.tolerance)],
            ),
            P::Ref(mm),
            P::Str("distance_accuracy_value".into()),
            P::Str(String::new()),
        ],
    );
    let context = writer.add_complex(&[
        ("GEOMETRIC_REPRESENTATION_CONTEXT", vec![P::Int(3)]),
        (
            "GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT",
            vec![P::List(vec![P::Ref(uncertainty)])],
        ),
        (
            "GLOBAL_UNIT_ASSIGNED_CONTEXT",
            vec![P::List(vec![P::Ref(mm), P::Ref(rad), P::Ref(sr)])],
        ),
        (
            "REPRESENTATION_CONTEXT",
            vec![P::Str(String::new()), P::Str(String::new())],
        ),
    ]);
    writer.add(
        "ADVANCED_BREP_SHAPE_REPRESENTATION",
        &[
            P::Str(String::new()),
            P::List(solids.into_iter().map(P::Ref).collect()),
            P::Ref(context),
        ],
    );
    crate::from_step_bytes_with_cancel(
        writer
            .file("analysis-source.step", "AUTOMOTIVE_DESIGN")
            .as_bytes(),
        &AtomicBool::new(false),
    )
    .expect("actual closed native source imports")
}

/// The normal portable writer is used unless the source deliberately retains
/// a different parameter law. This branch writes those authored STEP operands
/// directly, with the same topology/sense convention as SolidCraft's writer.
fn write_authored_source(
    writer: &mut solidcraft_step::write::Writer,
    source: &SourceBrep,
) -> Vec<u64> {
    use solidcraft_step::{
        geom as s,
        p21::Param as P,
        write::{GeomWriter, topology},
    };
    use std::sync::Arc;
    let empty = || P::Str(String::new());
    let boolean = |v| P::Enum(if v { "T" } else { "F" }.into());
    let refs = |ids: Vec<u64>| P::List(ids.into_iter().map(P::Ref).collect());
    let record = &source.record;
    let surfaces: Vec<_> = record
        .faces
        .iter()
        .enumerate()
        .map(
            |(i, face)| match source.face_surfaces.get(i).and_then(Option::as_ref) {
                Some(surface) => (Arc::new(surface.clone()), topology::Map::IDENTITY),
                None => {
                    let (surface, map) =
                        topology::surface(&face.surface).expect("actual source carrier exports");
                    (Arc::new(surface), map)
                }
            },
        )
        .collect();
    let vertices: Vec<_> = record
        .vertices
        .iter()
        .map(|vertex| {
            let point = GeomWriter::new(writer).point(vertex.point);
            writer.add_distinct("VERTEX_POINT", &[empty(), P::Ref(point)])
        })
        .collect();
    let edges: Vec<_> = record
        .edges
        .iter()
        .enumerate()
        .map(|(i, edge)| {
            let curve = match source.edge_curves.get(i).and_then(Option::as_ref) {
                Some(curve) => curve.clone(),
                None => {
                    let spatial =
                        topology::curve(&edge.curve).expect("actual spatial source curve exports");
                    let associated: Vec<_> = edge
                        .uses
                        .iter()
                        .filter_map(|used| {
                            let face = &record.faces[used.face as usize];
                            if face.nurbs.is_some() {
                                return None;
                            }
                            let b::Loop::Edges { edges } = &face.loops[used.r#loop as usize] else {
                                panic!("edge use belongs to an edge loop");
                            };
                            let (surface, map) = &surfaces[used.face as usize];
                            Some(s::Associated::Pcurve(s::Pcurve {
                                surface: surface.clone(),
                                curve: Arc::new(
                                    topology::pcurve(&edges[used.position as usize].pcurve, *map)
                                        .expect("actual source PCURVE exports"),
                                ),
                            }))
                        })
                        .collect();
                    if associated.is_empty() {
                        spatial
                    } else {
                        let seam = edge.uses.len() == 2 && edge.uses[0].face == edge.uses[1].face;
                        s::Curve::SurfaceCurve {
                            kind: if seam {
                                s::SurfaceCurveKind::Seam
                            } else {
                                s::SurfaceCurveKind::Surface
                            },
                            curve_3d: Arc::new(spatial),
                            associated,
                            master: s::PreferredRep::Curve3d,
                        }
                    }
                }
            };
            let geometry = GeomWriter::new(writer).curve(&curve);
            writer.add_distinct(
                "EDGE_CURVE",
                &[
                    empty(),
                    P::Ref(vertices[edge.vertices[0] as usize]),
                    P::Ref(vertices[edge.vertices[1] as usize]),
                    P::Ref(geometry),
                    boolean(true),
                ],
            )
        })
        .collect();
    let mut shells = Vec::new();
    for shell in &record.shells {
        let b::Shell::Faces { faces, wires } = shell else {
            panic!("closed face-shell fixture");
        };
        assert!(wires.is_empty());
        let mut face_ids = Vec::new();
        for used in faces {
            let face = &record.faces[used.face as usize];
            let (surface, map) = &surfaces[used.face as usize];
            let geometry = GeomWriter::new(writer).surface(surface);
            let mut bounds = Vec::new();
            for (i, boundary) in face.loops.iter().enumerate() {
                let boundary = match boundary {
                    b::Loop::Edges { edges: ring } => {
                        let mut oriented = Vec::new();
                        let order: Box<dyn Iterator<Item = &b::OrientedEdge>> = if used.forward {
                            Box::new(ring.iter())
                        } else {
                            Box::new(ring.iter().rev())
                        };
                        for edge in order {
                            oriented.push(writer.add_distinct(
                                "ORIENTED_EDGE",
                                &[
                                    empty(),
                                    P::Derived,
                                    P::Derived,
                                    P::Ref(edges[edge.edge as usize]),
                                    boolean(edge.forward == used.forward),
                                ],
                            ));
                        }
                        writer.add_distinct("EDGE_LOOP", &[empty(), refs(oriented)])
                    }
                    b::Loop::Vertex { vertex } => writer.add_distinct(
                        "VERTEX_LOOP",
                        &[empty(), P::Ref(vertices[*vertex as usize])],
                    ),
                };
                bounds.push(writer.add_distinct(
                    if i == 0 {
                        "FACE_OUTER_BOUND"
                    } else {
                        "FACE_BOUND"
                    },
                    &[empty(), P::Ref(boundary), boolean(true)],
                ));
            }
            face_ids.push(writer.add_distinct(
                "ADVANCED_FACE",
                &[
                    empty(),
                    refs(bounds),
                    P::Ref(geometry),
                    boolean(used.forward == map.keeps_normal()),
                ],
            ));
        }
        shells.push(writer.add_distinct("CLOSED_SHELL", &[empty(), refs(face_ids)]));
    }
    record
        .regions
        .iter()
        .skip(1)
        .filter(|region| !region.void)
        .map(|region| {
            let outer = shells[region.shells[0] as usize];
            if region.shells.len() == 1 {
                writer.add_distinct("MANIFOLD_SOLID_BREP", &[empty(), P::Ref(outer)])
            } else {
                let voids = region.shells[1..]
                    .iter()
                    .map(|&shell| {
                        writer.add_distinct(
                            "ORIENTED_CLOSED_SHELL",
                            &[
                                empty(),
                                P::Derived,
                                P::Ref(shells[shell as usize]),
                                boolean(true),
                            ],
                        )
                    })
                    .collect();
                writer.add_distinct("BREP_WITH_VOIDS", &[empty(), P::Ref(outer), refs(voids)])
            }
        })
        .collect()
}

fn solid(
    vertices: Vec<b::Vertex>,
    edges: Vec<b::Edge>,
    faces: Vec<b::Face>,
    senses: Vec<bool>,
) -> b::Brep {
    let mut source = b::Brep {
        version: b::VERSION,
        tolerance: 1.0e-6,
        regions: vec![
            b::Region {
                shells: vec![0],
                void: false,
            },
            b::Region {
                shells: vec![0],
                void: false,
            },
        ],
        shells: vec![b::Shell::Faces {
            faces: senses
                .into_iter()
                .enumerate()
                .map(|(face, forward)| b::OrientedFace {
                    face: face as u32,
                    forward,
                })
                .collect(),
            wires: vec![],
        }],
        vertices,
        edges,
        faces,
    };
    rebuild_uses(&mut source);
    source
}

pub(crate) fn rebuild_uses(source: &mut b::Brep) {
    for edge in &mut source.edges {
        edge.uses.clear();
    }
    for (face, record) in source.faces.iter().enumerate() {
        for (ring, boundary) in record.loops.iter().enumerate() {
            if let b::Loop::Edges { edges } = boundary {
                for (position, edge) in edges.iter().enumerate() {
                    source.edges[edge.edge as usize].uses.push(b::EdgeUseRef {
                        face: face as u32,
                        r#loop: ring as u32,
                        position: position as u32,
                        upper: !edge.forward,
                    });
                }
            }
        }
    }
}

fn line_uv(a: [f64; 2], z: [f64; 2]) -> b::Curve2 {
    b::Curve2::Spline {
        knots: vec![0., 0., 1., 1.],
        points: vec![a, z],
    }
}
fn line_edge(a: u32, z: u32, vertices: &[b::Vertex]) -> b::Edge {
    b::Edge {
        curve: b::Curve::Line {
            start: vertices[a as usize].point,
            end: vertices[z as usize].point,
        },
        nurbs: None,
        interval: [0., 1.],
        vertices: [a, z],
        tolerance: 1.0e-6,
        uses: vec![],
    }
}
fn oe(edge: usize, forward: bool, a: [f64; 2], z: [f64; 2]) -> b::OrientedEdge {
    b::OrientedEdge {
        edge: edge as u32,
        forward,
        pcurve: line_uv(a, z),
    }
}
fn plane(
    origin: [f64; 3],
    u_axis: [f64; 3],
    v_axis: [f64; 3],
    domain: [[f64; 2]; 2],
    edges: Vec<b::OrientedEdge>,
) -> b::Face {
    b::Face {
        surface: b::Surface::Plane {
            origin,
            u_axis,
            v_axis,
        },
        nurbs: None,
        domain,
        loops: vec![b::Loop::Edges { edges }],
    }
}

pub(crate) fn polygon_prism(points: &[[f64; 2]], height: f64) -> b::Brep {
    let n = points.len();
    assert!(n >= 3 && height > 0.);
    let vertices: Vec<_> = [0., height]
        .into_iter()
        .flat_map(|z| {
            points.iter().map(move |p| b::Vertex {
                point: [p[0], p[1], z],
                tolerance: 1.0e-6,
            })
        })
        .collect();
    let mut edges = Vec::new();
    for base in [0, n] {
        for i in 0..n {
            edges.push(line_edge(
                (base + i) as u32,
                (base + (i + 1) % n) as u32,
                &vertices,
            ));
        }
    }
    for i in 0..n {
        edges.push(line_edge(i as u32, (n + i) as u32, &vertices));
    }
    let domain = std::array::from_fn(|axis| {
        [
            points.iter().map(|p| p[axis]).fold(f64::INFINITY, f64::min),
            points
                .iter()
                .map(|p| p[axis])
                .fold(f64::NEG_INFINITY, f64::max),
        ]
    });
    let mut faces = Vec::new();
    for (base, z) in [(0, 0.), (n, height)] {
        faces.push(plane(
            [0., 0., z],
            [1., 0., 0.],
            [0., 1., 0.],
            domain,
            (0..n)
                .map(|i| oe(base + i, true, points[i], points[(i + 1) % n]))
                .collect(),
        ));
    }
    for i in 0..n {
        let next = (i + 1) % n;
        let a = points[i];
        let z = points[next];
        faces.push(plane(
            [a[0], a[1], 0.],
            [z[0] - a[0], z[1] - a[1], 0.],
            [0., 0., height],
            [[0., 1.]; 2],
            vec![
                oe(i, true, [0., 0.], [1., 0.]),
                oe(2 * n + next, true, [1., 0.], [1., 1.]),
                oe(n + i, false, [0., 1.], [1., 1.]),
                oe(2 * n + i, false, [0., 0.], [0., 1.]),
            ],
        ));
    }
    let mut senses = vec![true; faces.len()];
    senses[0] = false;
    solid(vertices, edges, faces, senses)
}

pub(crate) fn box_source(width: f64, depth: f64, height: f64) -> b::Brep {
    polygon_prism(
        &[[0., 0.], [width, 0.], [width, depth], [0., depth]],
        height,
    )
}

const EXACT_CIRCLE: [[f64; 3]; 9] = [
    [1., 0., 1.],
    [1., 1., 1.],
    [0., 2., 2.],
    [-2., 2., 2.],
    [-4., 0., 4.],
    [-4., -4., 4.],
    [0., -8., 8.],
    [8., -8., 8.],
    [16., 0., 16.],
];

pub(crate) fn circular_prism(center: [f64; 2], radius: f64, height: f64) -> b::Brep {
    let tensor = b::Surface::RationalSpline {
        u_knots: vec![0., 0., 0., 1., 1., 2., 2., 3., 3., 4., 4., 4.],
        v_knots: vec![0., 0., height, height],
        homogeneous: EXACT_CIRCLE
            .iter()
            .map(|&[x, y, w]| {
                vec![
                    [
                        center[0] * w + radius * x,
                        center[1] * w + radius * y,
                        0.,
                        w,
                    ],
                    [
                        center[0] * w + radius * x,
                        center[1] * w + radius * y,
                        height * w,
                        w,
                    ],
                ]
            })
            .collect(),
    };
    let xy = [
        [center[0] + radius, center[1]],
        [center[0] - radius, center[1]],
    ];
    let vertices: Vec<_> = [0., height]
        .into_iter()
        .flat_map(|z| {
            xy.iter().map(move |p| b::Vertex {
                point: [p[0], p[1], z],
                tolerance: 1.0e-6,
            })
        })
        .collect();
    let mut edges = Vec::new();
    for (base, z) in [(0, 0.), (2, height)] {
        for i in 0..2 {
            edges.push(b::Edge {
                curve: b::Curve::Circle {
                    frame: b::Frame {
                        origin: [center[0], center[1], z],
                        x_axis: [1., 0., 0.],
                        y_axis: [0., 1., 0.],
                    },
                    radius,
                },
                nurbs: Some(b::Curve::RationalSpline {
                    knots: vec![
                        2. * i as f64,
                        2. * i as f64,
                        2. * i as f64,
                        2. * i as f64 + 1.,
                        2. * i as f64 + 1.,
                        2. * i as f64 + 2.,
                        2. * i as f64 + 2.,
                        2. * i as f64 + 2.,
                    ],
                    homogeneous: EXACT_CIRCLE[4 * i..=4 * i + 4]
                        .iter()
                        .map(|&[x, y, w]| {
                            [
                                center[0] * w + radius * x,
                                center[1] * w + radius * y,
                                z * w,
                                w,
                            ]
                        })
                        .collect(),
                }),
                interval: [2. * i as f64, 2. * (i + 1) as f64],
                vertices: [(base + i) as u32, (base + (i + 1) % 2) as u32],
                tolerance: 1.0e-6,
                uses: vec![],
            });
        }
    }
    for i in 0..2 {
        edges.push(line_edge(i as u32, (2 + i) as u32, &vertices));
    }
    let circle_uv = |i: usize| {
        // Exact integer homogeneous circle quarters, X²+Y²=W².
        let homogeneous = EXACT_CIRCLE[4 * i..=4 * i + 4]
            .iter()
            .map(|p| {
                [
                    center[0] * p[2] + radius * p[0],
                    center[1] * p[2] + radius * p[1],
                    p[2],
                ]
            })
            .collect();
        let a = 2. * i as f64;
        b::Curve2::RationalSpline {
            knots: vec![a, a, a, a + 1., a + 1., a + 2., a + 2., a + 2.],
            homogeneous,
        }
    };
    let mut faces = Vec::new();
    for (base, z) in [(0, 0.), (2, height)] {
        faces.push(plane(
            [0., 0., z],
            [1., 0., 0.],
            [0., 1., 0.],
            [
                [center[0] - radius, center[0] + radius],
                [center[1] - radius, center[1] + radius],
            ],
            (0..2)
                .map(|i| b::OrientedEdge {
                    edge: (base + i) as u32,
                    forward: true,
                    pcurve: circle_uv(i),
                })
                .collect(),
        ));
    }
    for i in 0..2 {
        let a = 2. * i as f64;
        let z = 2. * (i + 1) as f64;
        let angular = |v| b::Curve2::Spline {
            knots: vec![a, a, z, z],
            points: vec![[a, v], [z, v]],
        };
        faces.push(b::Face {
            surface: b::Surface::Cylinder {
                frame: b::Frame {
                    origin: [center[0], center[1], 0.],
                    x_axis: [1., 0., 0.],
                    y_axis: [0., 1., 0.],
                },
                radius,
            },
            nurbs: Some(tensor.clone()),
            domain: [[a, z], [0., height]],
            loops: vec![b::Loop::Edges {
                edges: vec![
                    b::OrientedEdge {
                        edge: i as u32,
                        forward: true,
                        pcurve: angular(0.),
                    },
                    oe(4 + (i + 1) % 2, true, [z, 0.], [z, height]),
                    b::OrientedEdge {
                        edge: (2 + i) as u32,
                        forward: false,
                        pcurve: angular(height),
                    },
                    oe(4 + i, false, [a, 0.], [a, height]),
                ],
            }],
        });
    }
    solid(vertices, edges, faces, vec![false, true, true, true])
}

pub(crate) fn translated(source: b::Brep, translation: [f64; 3]) -> b::Brep {
    source.moved(&b::placement::Rigid {
        rotation: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        translation,
    })
}

fn horizontal_face(source: &b::Brep, face: usize, height: f64) -> bool {
    source.faces[face].loops.iter().all(|ring| match ring {
        b::Loop::Edges { edges } => edges.iter().all(|edge| {
            source.edges[edge.edge as usize]
                .vertices
                .iter()
                .all(|&vertex| source.vertices[vertex as usize].point[2] == height)
        }),
        b::Loop::Vertex { vertex } => source.vertices[*vertex as usize].point[2] == height,
    })
}

/// Sew an inverted original prism into the exterior. The opening reuses its
/// literal rim edges; it is not a sampled boolean or a certificate mutation.
fn subtract_open_prism(outer: &mut b::Brep, mut plug: b::Brep, openings: &[f64]) {
    let vertex_base = outer.vertices.len() as u32;
    let edge_base = outer.edges.len() as u32;
    for edge in &mut plug.edges {
        edge.vertices = edge.vertices.map(|vertex| vertex + vertex_base);
    }
    // Determine cap identities before rebasing their vertex indices.
    let original_vertices = &plug.vertices;
    let cap_at = |face: &b::Face, height: f64| {
        face.loops.iter().all(|ring| match ring {
            b::Loop::Edges { edges } => edges.iter().all(|edge| {
                plug.edges[edge.edge as usize]
                    .vertices
                    .iter()
                    .all(|&vertex| {
                        original_vertices[(vertex - vertex_base) as usize].point[2] == height
                    })
            }),
            b::Loop::Vertex { vertex } => original_vertices[*vertex as usize].point[2] == height,
        })
    };
    let omitted: Vec<_> = openings
        .iter()
        .map(|&height| {
            plug.faces
                .iter()
                .position(|face| cap_at(face, height))
                .expect("actual prism opening cap")
        })
        .collect();
    for (&height, &rim) in openings.iter().zip(&omitted) {
        let cap = (0..outer.faces.len())
            .find(|&face| horizontal_face(outer, face, height))
            .expect("actual exterior opening cap");
        let b::Surface::Plane {
            origin,
            u_axis,
            v_axis,
        } = outer.faces[cap].surface
        else {
            panic!("coordinate cap");
        };
        assert_eq!([origin[0], origin[1]], [0., 0.]);
        assert_eq!(u_axis, [1., 0., 0.]);
        assert_eq!(v_axis, [0., 1., 0.]);
        for ring in &plug.faces[rim].loops {
            let b::Loop::Edges { edges } = ring else {
                panic!("noncollapsed original rim");
            };
            outer.faces[cap].loops.push(b::Loop::Edges {
                edges: edges
                    .iter()
                    .rev()
                    .map(|edge| b::OrientedEdge {
                        edge: edge.edge + edge_base,
                        forward: !edge.forward,
                        pcurve: edge.pcurve.clone(),
                    })
                    .collect(),
            });
        }
    }
    let b::Shell::Faces {
        faces: plug_senses, ..
    } = &plug.shells[0]
    else {
        panic!("prism face shell");
    };
    let senses: Vec<_> = plug_senses
        .iter()
        .map(|used| (used.face as usize, !used.forward))
        .collect();
    outer.vertices.append(&mut plug.vertices);
    outer.edges.append(&mut plug.edges);
    let b::Shell::Faces {
        faces: outer_senses,
        ..
    } = &mut outer.shells[0]
    else {
        panic!("exterior face shell");
    };
    for (i, mut face) in plug.faces.into_iter().enumerate() {
        if omitted.contains(&i) {
            continue;
        }
        for ring in &mut face.loops {
            match ring {
                b::Loop::Edges { edges } => {
                    for edge in edges {
                        edge.edge += edge_base;
                    }
                }
                b::Loop::Vertex { vertex } => *vertex += vertex_base,
            }
        }
        let forward = senses.iter().find(|&&(face, _)| face == i).unwrap().1;
        outer_senses.push(b::OrientedFace {
            face: outer.faces.len() as u32,
            forward,
        });
        outer.faces.push(face);
    }
    rebuild_uses(outer);
}

pub(crate) fn blind_pocket() -> b::Brep {
    let mut outer = box_source(20., 20., 8.);
    let plug = translated(circular_prism([10., 10.], 3., 4.), [0., 0., 4.]);
    subtract_open_prism(&mut outer, plug, &[8.]);
    outer
}

pub(crate) fn opposing_pockets() -> b::Brep {
    let floor = 129. / 32.;
    let ceiling = 127. / 32.;
    let mut outer = box_source(40., 20., 8.);
    subtract_open_prism(
        &mut outer,
        translated(circular_prism([10., 10.], 3., 8. - floor), [0., 0., floor]),
        &[8.],
    );
    subtract_open_prism(&mut outer, circular_prism([30., 10.], 3., ceiling), &[0.]);
    outer
}

pub(crate) fn protected_bore(rational: bool) -> b::Brep {
    let (mut outer, plug) = if rational {
        (
            polygon_prism(&[[-10., -10.], [10., -10.], [10., 10.], [-10., 10.]], 8.),
            nurbs_cylinder(),
        )
    } else {
        (box_source(20., 20., 8.), circular_prism([10., 10.], 3., 8.))
    };
    subtract_open_prism(&mut outer, plug, &[0., 8.]);
    outer
}

pub(crate) fn thin_sheared_rod() -> b::Brep {
    let mut source = box_source(1. / 1024., 1. / 1024., 8.);
    let map = |mut p: [f64; 3]| {
        p[0] += p[2] / 8.;
        p
    };
    for vertex in &mut source.vertices {
        vertex.point = map(vertex.point);
    }
    for edge in &mut source.edges {
        let b::Curve::Line { start, end } = &mut edge.curve else {
            panic!("original rod line");
        };
        *start = map(*start);
        *end = map(*end);
    }
    for face in &mut source.faces {
        let b::Surface::Plane {
            origin,
            u_axis,
            v_axis,
        } = &mut face.surface
        else {
            panic!("original rod plane");
        };
        *origin = map(*origin);
        *u_axis = map(*u_axis);
        *v_axis = map(*v_axis);
    }
    source
}

fn spline_plane(face: &mut b::Face, rational: bool) {
    let b::Surface::Plane {
        origin,
        u_axis,
        v_axis,
    } = face.surface
    else {
        panic!("actual coordinate plane");
    };
    let points: Vec<Vec<_>> = face.domain[0]
        .iter()
        .map(|&u| {
            face.domain[1]
                .iter()
                .map(|&v| {
                    std::array::from_fn(|axis| origin[axis] + u * u_axis[axis] + v * v_axis[axis])
                })
                .collect()
        })
        .collect();
    let u_knots = vec![
        face.domain[0][0],
        face.domain[0][0],
        face.domain[0][1],
        face.domain[0][1],
    ];
    let v_knots = vec![
        face.domain[1][0],
        face.domain[1][0],
        face.domain[1][1],
        face.domain[1][1],
    ];
    face.surface = if rational {
        b::Surface::RationalSpline {
            u_knots,
            v_knots,
            homogeneous: points
                .into_iter()
                .map(|row| {
                    row.into_iter()
                        .map(|p: [f64; 3]| [p[0], p[1], p[2], 1.])
                        .collect()
                })
                .collect(),
        }
    } else {
        b::Surface::Spline {
            u_knots,
            v_knots,
            points,
        }
    };
}

pub(crate) fn spline_box(rational: bool) -> b::Brep {
    let mut source = box_source(20., 10., 8.);
    for face in &mut source.faces {
        spline_plane(face, rational);
    }
    source
}

pub(crate) fn blind_pocket_nurbs_floor() -> b::Brep {
    let mut source = blind_pocket();
    let floor = (0..source.faces.len())
        .find(|&face| horizontal_face(&source, face, 4.))
        .expect("actual pocket floor");
    spline_plane(&mut source.faces[floor], true);
    source
}

pub(crate) fn cone_frustum() -> b::Brep {
    let mut source = circular_prism([0., 0.], 5., 8.);
    let taper = |p: [f64; 3]| {
        let s = (5. - p[2] / 4.) / 5.;
        [p[0] * s, p[1] * s, p[2]]
    };
    for vertex in &mut source.vertices {
        vertex.point = taper(vertex.point);
    }
    for edge in &mut source.edges {
        match &mut edge.curve {
            b::Curve::Circle { frame, radius } => *radius = 5. - frame.origin[2] / 4.,
            b::Curve::Line { start, end } => {
                *start = taper(*start);
                *end = taper(*end);
            }
            _ => panic!("actual cone rim or generator"),
        }
        if let Some(b::Curve::RationalSpline { homogeneous, .. }) = &mut edge.nurbs {
            for p in homogeneous {
                let s = (5. - p[2] / p[3] / 4.) / 5.;
                p[0] *= s;
                p[1] *= s;
            }
        }
    }
    for (i, face) in source.faces.iter_mut().enumerate() {
        if i < 2 {
            let s = if i == 0 { 1. } else { 3. / 5. };
            for domain in &mut face.domain {
                for coordinate in domain {
                    *coordinate *= s;
                }
            }
            for ring in &mut face.loops {
                let b::Loop::Edges { edges } = ring else {
                    panic!("actual cone cap");
                };
                for edge in edges {
                    let b::Curve2::RationalSpline { homogeneous, .. } = &mut edge.pcurve else {
                        panic!("exact circular cap");
                    };
                    for p in homogeneous {
                        p[0] *= s;
                        p[1] *= s;
                    }
                }
            }
        } else {
            face.surface = b::Surface::Cone {
                frame: b::Frame {
                    origin: [0.; 3],
                    x_axis: [1., 0., 0.],
                    y_axis: [0., 1., 0.],
                },
                radius_at_origin: 5.,
                half_angle: -(0.25_f64).atan(),
            };
            let Some(b::Surface::RationalSpline { homogeneous, .. }) = &mut face.nurbs else {
                panic!("literal cone tensor");
            };
            for row in homogeneous {
                for p in row {
                    let s = (5. - p[2] / p[3] / 4.) / 5.;
                    p[0] *= s;
                    p[1] *= s;
                }
            }
        }
    }
    source
}

fn source_carriers(record: b::Brep, carriers: Vec<solidcraft_step::geom::Surface>) -> SourceBrep {
    use solidcraft_step::{geom as s, write::topology};
    use std::sync::Arc;
    let carriers: Vec<_> = carriers.into_iter().map(Arc::new).collect();
    let edge_curves = record
        .edges
        .iter()
        .map(|edge| {
            let associated = edge
                .uses
                .iter()
                .map(|used| s::Associated::Surface(carriers[used.face as usize].clone()))
                .collect();
            Some(s::Curve::SurfaceCurve {
                kind: s::SurfaceCurveKind::Surface,
                curve_3d: Arc::new(
                    topology::curve(&edge.curve).expect("actual source edge carrier"),
                ),
                associated,
                master: s::PreferredRep::Curve3d,
            })
        })
        .collect();
    SourceBrep {
        record,
        edge_curves,
        face_surfaces: carriers
            .into_iter()
            .map(|surface| Some((*surface).clone()))
            .collect(),
    }
}

pub(crate) fn bezier_extrusion(hole: bool, rotated: bool) -> SourceBrep {
    use solidcraft_step::{geom as s, write::topology};
    use std::sync::Arc;
    let mut source = box_source(20., 10., 8.);
    let controls = [[0., 0.], [5., -2.], [15., -2.], [20., 0.]];
    let knots = vec![0., 0., 0., 0., 1., 1., 1., 1.];
    for (edge, z) in [(0, 0.), (4, 8.)] {
        source.edges[edge].curve = b::Curve::Spline {
            knots: knots.clone(),
            points: controls.map(|p| [p[0], p[1], z]).to_vec(),
        };
    }
    for face in &mut source.faces[..2] {
        face.domain[1][0] = -2.;
        let b::Loop::Edges { edges } = &mut face.loops[0] else {
            panic!("actual cap");
        };
        edges[0].pcurve = b::Curve2::Spline {
            knots: knots.clone(),
            points: controls.to_vec(),
        };
    }
    source.faces[2].surface = b::Surface::Spline {
        u_knots: knots,
        v_knots: vec![0., 0., 1., 1.],
        points: controls
            .map(|p| vec![[p[0], p[1], 0.], [p[0], p[1], 8.]])
            .to_vec(),
    };
    if hole {
        subtract_open_prism(&mut source, circular_prism([10., 5.], 2., 8.), &[0., 8.]);
    }
    if rotated {
        source = source.moved(&b::placement::Rigid {
            rotation: [[0., -1., 0.], [1., 0., 0.], [0., 0., 1.]],
            translation: [0.; 3],
        });
    }
    let carriers = source
        .faces
        .iter()
        .enumerate()
        .map(|(i, face)| {
            if i < 2 {
                return topology::surface(&face.surface)
                    .expect("original planar cap")
                    .0;
            }
            let b::Loop::Edges { edges } = &face.loops[0] else {
                panic!("original extrusion side");
            };
            let bottom = &source.edges[edges[0].edge as usize].curve;
            // All side faces have the original lower profile as their first use;
            // the cavity's orientation is carried by its shell, not a profile flip.
            s::Surface::Extrusion {
                profile: Arc::new(
                    topology::curve(bottom).expect("literal original extrusion profile"),
                ),
                direction: [0., 0., 1.],
                magnitude: 8.,
            }
        })
        .collect();
    source_carriers(source, carriers)
}

pub(crate) fn revolution() -> SourceBrep {
    use solidcraft_step::{geom as s, write::topology};
    use std::sync::Arc;
    let mut source = cone_frustum();
    subtract_open_prism(&mut source, circular_prism([0., 0.], 2., 8.), &[0., 8.]);
    let carriers = source
        .faces
        .iter()
        .enumerate()
        .map(|(i, _)| {
            let (start, end) = match i {
                0 => ([5., 0., 0.], [2., 0., 0.]),
                1 => ([3., 0., 8.], [2., 0., 8.]),
                2 | 3 => ([5., 0., 0.], [3., 0., 8.]),
                _ => ([2., 0., 0.], [2., 0., 8.]),
            };
            s::Surface::Revolution {
                profile: Arc::new(
                    topology::curve(&b::Curve::Line { start, end })
                        .expect("literal original revolution meridian"),
                ),
                axis: s::Axis1 {
                    origin: [0.; 3],
                    direction: [0., 0., 1.],
                },
            }
        })
        .collect();
    source_carriers(source, carriers)
}

pub(crate) fn recipe(name: &str) -> Definition {
    match name {
        "box-mm.step" => crate::from_step_bytes_with_cancel(
            include_bytes!("../../../../fixtures/geometry/box-mm.step"),
            &AtomicBool::new(false),
        )
        .unwrap(),
        "cylinder.step" => crate::from_step_bytes_with_cancel(
            include_bytes!("../../../../fixtures/geometry/cylinder.step"),
            &AtomicBool::new(false),
        )
        .unwrap(),
        "native-parity-bspline-box.step" => original_definition(spline_box(false)),
        "native-parity-nurbs-box.step" => original_definition(spline_box(true)),
        "native-parity-cone-frustum.step" => original_definition(cone_frustum()),
        "native-parity-sphere-belt.step" => original_definition(sphere_belt()),
        "native-parity-full-sphere.step" => original_definition(full_sphere()),
        "native-parity-ring-torus.step" => original_definition(ring_torus()),
        "native-parity-bezier-extrusion.step" => {
            original_definition(bezier_extrusion(false, false))
        }
        "native-parity-bezier-extrusion-bore.step" => {
            original_definition(bezier_extrusion(true, false))
        }
        "native-parity-rotated-bezier-bore.step" => {
            original_definition(bezier_extrusion(true, true))
        }
        "native-parity-revolution.step" => original_definition(revolution()),
        "native-parity-blind-pocket.step" => original_definition(blind_pocket()),
        "native-parity-blind-pocket-nurbs-floor.step" => {
            original_definition(blind_pocket_nurbs_floor())
        }
        "native-parity-nurbs-cylinder.step" => original_definition(nurbs_cylinder()),
        "native-parity-nurbs-torus.step" => original_definition(nurbs_torus()),
        "native-parity-nurbs-torus-independent-parameters.step" => {
            original_definition(nurbs_torus_independent_parameters())
        }
        _ => panic!("unknown original native analysis source recipe: {name}"),
    }
}
