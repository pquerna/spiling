// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Original rational source recipes, not recognized analytic replacements.
//! The integer homogeneous circle coefficients satisfy X² + Y² = W² exactly.
//! Seam branches retain their authored increasing UV parameters; reversal is a
//! topological edge-use property. Independent spatial parameters are retained in
//! explicit STEP source operands, separately from the portable record's own-t
//! pcurves.

use solidcraft_geom::brep as b;
use solidcraft_step::{geom as s, write::topology};
use std::sync::Arc;

// A quarter has Bernstein controls (1,0,1), (1,1,1), (0,2,2), hence
// X=1-t², Y=2t, W=1+t². Subsequent quarters rotate and scale by 2,4,8,
// preserving literal shared homogeneous endpoints without sqrt(1/2) rounding.
const CIRCLE: [[f64; 3]; 9] = [
    [1.0, 0.0, 1.0],
    [1.0, 1.0, 1.0],
    [0.0, 2.0, 2.0],
    [-2.0, 2.0, 2.0],
    [-4.0, 0.0, 4.0],
    [-4.0, -4.0, 4.0],
    [0.0, -8.0, 8.0],
    [8.0, -8.0, 8.0],
    [16.0, 0.0, 16.0],
];

fn circle_knots() -> Vec<f64> {
    vec![0.0, 0.0, 0.0, 1.0, 1.0, 2.0, 2.0, 3.0, 3.0, 4.0, 4.0, 4.0]
}
fn half_knots(half: usize) -> Vec<f64> {
    let start = 2.0 * half as f64;
    let middle = start + 1.0;
    let end = start + 2.0;
    vec![start, start, start, middle, middle, end, end, end]
}
fn linear_knots(start: f64, end: f64) -> Vec<f64> {
    vec![start, start, end, end]
}
fn uv_line(start: f64, end: f64, front: [f64; 2], back: [f64; 2]) -> b::Curve2 {
    b::Curve2::Spline {
        knots: linear_knots(start, end),
        points: vec![front, back],
    }
}
fn edge(curve: b::Curve, interval: [f64; 2], vertices: [u32; 2]) -> b::Edge {
    b::Edge {
        curve,
        nurbs: None,
        interval,
        vertices,
        tolerance: 1.0e-6,
        uses: vec![],
    }
}
fn use_(edge: usize, forward: bool, pcurve: b::Curve2) -> b::OrientedEdge {
    b::OrientedEdge {
        edge: edge as u32,
        forward,
        pcurve,
    }
}
fn face(surface: b::Surface, domain: [[f64; 2]; 2], edges: Vec<b::OrientedEdge>) -> b::Face {
    b::Face {
        surface,
        nurbs: None,
        domain,
        loops: vec![b::Loop::Edges { edges }],
    }
}
fn vertex(point: [f64; 3]) -> b::Vertex {
    b::Vertex {
        point,
        tolerance: 1.0e-6,
    }
}
fn cap(z: f64) -> b::Surface {
    b::Surface::Plane {
        origin: [0.0, 0.0, z],
        u_axis: [1.0, 0.0, 0.0],
        v_axis: [0.0, 1.0, 0.0],
    }
}

/// Radius 5, height 8, three faces, four vertices and five edges. The side's
/// actual source parameters are [0,4] × [0,8], not analytic angles. Its one
/// axial seam has both original u=4 and u=0 branches.
pub(super) fn nurbs_cylinder() -> b::Brep {
    const R: f64 = 5.0;
    const H: f64 = 8.0;
    let side = b::Surface::RationalSpline {
        u_knots: circle_knots(),
        v_knots: linear_knots(0.0, H),
        homogeneous: CIRCLE
            .iter()
            .map(|&[x, y, w]| vec![[R * x, R * y, 0.0, w], [R * x, R * y, H * w, w]])
            .collect(),
    };
    let vertices = vec![
        vertex([R, 0.0, 0.0]),
        vertex([-R, 0.0, 0.0]),
        vertex([R, 0.0, H]),
        vertex([-R, 0.0, H]),
    ];
    let mut edges = Vec::with_capacity(5);
    for (z, base) in [(0.0, 0), (H, 2)] {
        for half in 0..2 {
            let start = 2.0 * half as f64;
            edges.push(edge(
                b::Curve::RationalSpline {
                    knots: half_knots(half),
                    homogeneous: CIRCLE[4 * half..=4 * half + 4]
                        .iter()
                        .map(|&[x, y, w]| [R * x, R * y, z * w, w])
                        .collect(),
                },
                [start, start + 2.0],
                [(base + half) as u32, (base + 1 - half) as u32],
            ));
        }
    }
    edges.push(edge(
        b::Curve::RationalSpline {
            knots: linear_knots(0.0, H),
            homogeneous: vec![[R, 0.0, 0.0, 1.0], [R, 0.0, H, 1.0]],
        },
        [0.0, H],
        [0, 2],
    ));
    let side_rim = |index: usize, forward: bool, z: f64| {
        let start = 2.0 * (index % 2) as f64;
        use_(
            index,
            forward,
            uv_line(start, start + 2.0, [start, z], [start + 2.0, z]),
        )
    };
    let side = face(
        side,
        [[0.0, 4.0], [0.0, H]],
        vec![
            side_rim(0, true, 0.0),
            side_rim(1, true, 0.0),
            use_(4, true, uv_line(0.0, H, [4.0, 0.0], [4.0, H])),
            side_rim(3, false, H),
            side_rim(2, false, H),
            use_(4, false, uv_line(0.0, H, [0.0, 0.0], [0.0, H])),
        ],
    );
    let planar_rim = |index: usize| {
        let half = index % 2;
        use_(
            index,
            true,
            b::Curve2::RationalSpline {
                knots: half_knots(half),
                homogeneous: CIRCLE[4 * half..=4 * half + 4]
                    .iter()
                    .map(|&[x, y, w]| [R * x, R * y, w])
                    .collect(),
            },
        )
    };
    let bottom = face(cap(0.0), [[-R, R]; 2], vec![planar_rim(0), planar_rim(1)]);
    let top = face(cap(H), [[-R, R]; 2], vec![planar_rim(2), planar_rim(3)]);
    super::solid(
        vertices,
        edges,
        vec![side, bottom, top],
        vec![true, false, true],
    )
}

/// Exact rational ring torus, major radius 6 and minor radius 2; one face,
/// three vertices and four edges. Both source seam directions have two branches
/// on [0,4]². At z=0 its annulus has radii 8 and 4, area 48π mm².
pub(super) fn nurbs_torus() -> b::Brep {
    torus_record(false)
}

/// Same source, with every original 3D leader using t3D=10+2*tUV. The portable
/// record remains valid in its own edge parameter; whole original STEP curve
/// overrides preserve the independent original PCURVEs on [0,2] and [2,4].
pub(super) fn nurbs_torus_independent_parameters() -> super::SourceBrep {
    let record = torus_record(true);
    let (surface, map) =
        topology::surface(&record.faces[0].surface).expect("exact rational torus exports");
    let surface = Arc::new(surface);
    let edge_curves = record
        .edges
        .iter()
        .enumerate()
        .map(|(index, edge)| {
            let half = index % 2;
            let start = 2.0 * half as f64;
            let end = start + 2.0;
            let branches = if index < 2 {
                [
                    uv_line(start, end, [start, 0.0], [end, 0.0]),
                    uv_line(start, end, [start, 4.0], [end, 4.0]),
                ]
            } else {
                [
                    uv_line(start, end, [4.0, start], [4.0, end]),
                    uv_line(start, end, [0.0, start], [0.0, end]),
                ]
            };
            let associated = branches
                .iter()
                .map(|branch| {
                    s::Associated::Pcurve(s::Pcurve {
                        surface: Arc::clone(&surface),
                        curve: Arc::new(
                            topology::pcurve(branch, map)
                                .expect("original linear source pcurve exports"),
                        ),
                    })
                })
                .collect();
            Some(s::Curve::SurfaceCurve {
                kind: s::SurfaceCurveKind::Seam,
                curve_3d: Arc::new(
                    topology::curve(&edge.curve)
                        .expect("exact independently parameterized source leader exports"),
                ),
                associated,
                master: s::PreferredRep::Curve3d,
            })
        })
        .collect();
    super::SourceBrep {
        record,
        edge_curves,
        face_surfaces: vec![None],
    }
}

fn torus_record(independent: bool) -> b::Brep {
    const MAJOR: f64 = 6.0;
    const MINOR: f64 = 2.0;
    let tensor = |[x, y, w]: [f64; 3], [a, b, v]: [f64; 3]| {
        let radial = MAJOR * v + MINOR * a;
        [radial * x, radial * y, MINOR * b * w, v * w]
    };
    let surface = b::Surface::RationalSpline {
        u_knots: circle_knots(),
        v_knots: circle_knots(),
        homogeneous: CIRCLE
            .iter()
            .map(|&theta| CIRCLE.iter().map(|&phi| tensor(theta, phi)).collect())
            .collect(),
    };
    let vertices = vec![
        vertex([8.0, 0.0, 0.0]),
        vertex([-8.0, 0.0, 0.0]),
        vertex([4.0, 0.0, 0.0]),
    ];
    let parameter = |t: f64| if independent { 10.0 + 2.0 * t } else { t };
    let mut edges = Vec::with_capacity(4);
    for direction in 0..2 {
        for half in 0..2 {
            let homogeneous = CIRCLE[4 * half..=4 * half + 4]
                .iter()
                .map(|&h| {
                    if direction == 0 {
                        tensor(h, CIRCLE[0])
                    } else {
                        tensor(CIRCLE[0], h)
                    }
                })
                .collect();
            let start = 2.0 * half as f64;
            let middle_vertex = if direction == 0 { 1 } else { 2 };
            edges.push(edge(
                b::Curve::RationalSpline {
                    knots: half_knots(half).into_iter().map(parameter).collect(),
                    homogeneous,
                },
                [parameter(start), parameter(start + 2.0)],
                if half == 0 {
                    [0, middle_vertex]
                } else {
                    [middle_vertex, 0]
                },
            ));
        }
    }
    // Native record pcurves use the edge's own t. Their surface coordinates
    // remain authored u/v, and the explicit STEP overrides above retain the
    // original independent pcurve parameter instead of conflating the two.
    let theta = |half: usize, forward: bool, v: f64| {
        let start = 2.0 * half as f64;
        use_(
            half,
            forward,
            uv_line(
                parameter(start),
                parameter(start + 2.0),
                [start, v],
                [start + 2.0, v],
            ),
        )
    };
    let phi = |half: usize, forward: bool, u: f64| {
        let start = 2.0 * half as f64;
        use_(
            2 + half,
            forward,
            uv_line(
                parameter(start),
                parameter(start + 2.0),
                [u, start],
                [u, start + 2.0],
            ),
        )
    };
    let face = face(
        surface,
        [[0.0, 4.0]; 2],
        vec![
            theta(0, true, 0.0),
            theta(1, true, 0.0),
            phi(0, true, 4.0),
            phi(1, true, 4.0),
            theta(1, false, 4.0),
            theta(0, false, 4.0),
            phi(1, false, 0.0),
            phi(0, false, 0.0),
        ],
    );
    super::solid(vertices, edges, vec![face], vec![true])
}
