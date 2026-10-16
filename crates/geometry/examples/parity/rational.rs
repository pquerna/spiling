// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Original rational STEP sources, including the original oriented seam branches.
//!
//! These recipes use only generic NURBS carriers. Their independent geometric
//! oracles are the circle identity X² + Y² = W² and its tensor product, not
//! recognition of an analytic surface by the importer. Export with the shared
//! `original` helper's explicit-source-curve path so the authored `SEAM_CURVE`
//! and `PCURVE` associations remain the edge geometry, not a nested leader.

use super::StepSolid;
use monstertruck_io::step::load::step_geometry::{
    Curve2D, Curve3D, ElementarySurface, StepParameterCurve, Surface, SurfaceCurve3D,
    SurfaceCurveAssociatedGeometry, SurfaceCurveKind, SurfaceCurveRepresentation, re_exports::*,
};
use monstertruck_topology::{Edge, Face, Solid, Vertex, Wire};

// Each unit quarter starts with homogeneous Bernstein controls
// (1,0,1), (1,1,1), (0,2,2): X=1-t², Y=2t, W=1+t².
// Rotate subsequent quarters and multiply all three of their controls by
// 2, 4 and 8 respectively. Shared homogeneous endpoints then agree exactly.
// Double internal knots retain the original piecewise rational parameter;
// in particular, this is not a rounded sqrt(1/2)-weighted approximation.
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

fn circle_knots() -> KnotVector {
    KnotVector::from(vec![
        0.0, 0.0, 0.0, 1.0, 1.0, 2.0, 2.0, 3.0, 3.0, 4.0, 4.0, 4.0,
    ])
}

fn half_knots(half: usize) -> KnotVector {
    let start = 2.0 * half as f64;
    let middle = start + 1.0;
    let end = start + 2.0;
    KnotVector::from(vec![start, start, start, middle, middle, end, end, end])
}

fn linear_knots(start: f64, end: f64) -> KnotVector {
    KnotVector::from(vec![start, start, end, end])
}

fn uv_line(start: f64, end: f64, front: [f64; 2], back: [f64; 2]) -> Curve2D {
    // Unlike an unbounded STEP LINE, this spline declares its own exact
    // parameter interval. It need not equal the associated 3D leader's domain.
    Curve2D::BsplineCurve(BsplineCurve::new(
        linear_knots(start, end),
        vec![
            Point2::new(front[0], front[1]),
            Point2::new(back[0], back[1]),
        ],
    ))
}

fn associated(surface: &Surface, curve: Curve2D) -> SurfaceCurveAssociatedGeometry {
    let pcurve = StepParameterCurve::new(Box::new(curve), Box::new(surface.clone()));
    SurfaceCurveAssociatedGeometry::ParameterCurve(pcurve)
}

fn source_curve(
    kind: SurfaceCurveKind,
    leader: Curve3D,
    geometry: Vec<SurfaceCurveAssociatedGeometry>,
) -> Curve3D {
    Curve3D::SurfaceCurve(SurfaceCurve3D::new(
        kind,
        Box::new(leader),
        geometry,
        SurfaceCurveRepresentation::Curve3D,
    ))
}

fn seam(leader: Curve3D, surface: &Surface, positive: Curve2D, negative: Curve2D) -> Curve3D {
    // The first original branch belongs to the forward edge use; the second
    // belongs to the inverse use. Both pcurves retain increasing own t:
    // reversal is topology, not a second reparameterization of the pcurve.
    source_curve(
        SurfaceCurveKind::SeamCurve,
        leader,
        vec![associated(surface, positive), associated(surface, negative)],
    )
}

fn rational_half(half: usize, controls: impl Iterator<Item = Vector4>) -> Curve3D {
    Curve3D::NurbsCurve(NurbsCurve::new(BsplineCurve::new(
        half_knots(half),
        controls.collect(),
    )))
}

fn cap(z: f64) -> Surface {
    // Unit XY axes make the original planar PCURVE coordinates exactly (x,y).
    let origin = Point3::new(0.0, 0.0, z);
    Surface::ElementarySurface(ElementarySurface::Plane(Plane::new(
        origin,
        origin + Vector3::unit_x(),
        origin + Vector3::unit_y(),
    )))
}

/// A radius-5, height-8 cylinder with one rational side and two planar caps.
///
/// Independent oracle: 3 faces, 4 vertices, 5 edges; at z=4 one region with
/// one exterior loop and area 25π mm². Physical bounds are [-5,5]² × [0,8].
/// The side's original source domain is [0,4] × [0,8], with explicit u=4/u=0
/// axial seam branches. Its native/source parameters are not angles.
pub(super) fn nurbs_cylinder() -> StepSolid {
    const RADIUS: f64 = 5.0;
    const HEIGHT: f64 = 8.0;
    let side = Surface::NurbsSurface(NurbsSurface::new(BsplineSurface::new(
        (circle_knots(), linear_knots(0.0, HEIGHT)),
        CIRCLE
            .iter()
            .map(|&[x, y, w]| {
                vec![
                    Vector4::new(RADIUS * x, RADIUS * y, 0.0, w),
                    Vector4::new(RADIUS * x, RADIUS * y, HEIGHT * w, w),
                ]
            })
            .collect(),
    )));
    let bottom = cap(0.0);
    let top = cap(HEIGHT);
    let vertices = [
        Vertex::new(Point3::new(RADIUS, 0.0, 0.0)),
        Vertex::new(Point3::new(-RADIUS, 0.0, 0.0)),
        Vertex::new(Point3::new(RADIUS, 0.0, HEIGHT)),
        Vertex::new(Point3::new(-RADIUS, 0.0, HEIGHT)),
    ];
    let rim = |half: usize, z: f64, plane: &Surface, front: usize, back: usize| {
        let controls = &CIRCLE[4 * half..=4 * half + 4];
        let leader = rational_half(
            half,
            controls
                .iter()
                .map(|&[x, y, w]| Vector4::new(RADIUS * x, RADIUS * y, z * w, w)),
        );
        let planar_curve = Curve2D::NurbsCurve(NurbsCurve::new(BsplineCurve::new(
            half_knots(half),
            controls
                .iter()
                .map(|&[x, y, w]| Vector3::new(RADIUS * x, RADIUS * y, w))
                .collect(),
        )));
        let start = 2.0 * half as f64;
        let end = start + 2.0;
        Edge::new(
            &vertices[front],
            &vertices[back],
            source_curve(
                SurfaceCurveKind::SurfaceCurve,
                leader,
                vec![
                    associated(&side, uv_line(start, end, [start, z], [end, z])),
                    associated(plane, planar_curve),
                ],
            ),
        )
    };
    let lower = [rim(0, 0.0, &bottom, 0, 1), rim(1, 0.0, &bottom, 1, 0)];
    let upper = [rim(0, HEIGHT, &top, 2, 3), rim(1, HEIGHT, &top, 3, 2)];
    let axial_leader = Curve3D::NurbsCurve(NurbsCurve::new(BsplineCurve::new(
        linear_knots(0.0, HEIGHT),
        vec![
            Vector4::new(RADIUS, 0.0, 0.0, 1.0),
            Vector4::new(RADIUS, 0.0, HEIGHT, 1.0),
        ],
    )));
    let axial = Edge::new(
        &vertices[0],
        &vertices[2],
        seam(
            axial_leader,
            &side,
            uv_line(0.0, HEIGHT, [4.0, 0.0], [4.0, HEIGHT]),
            uv_line(0.0, HEIGHT, [0.0, 0.0], [0.0, HEIGHT]),
        ),
    );
    let side_boundary: Wire<_, _> = vec![
        lower[0].clone(),
        lower[1].clone(),
        axial.clone(),
        upper[1].inverse(),
        upper[0].inverse(),
        axial.inverse(),
    ]
    .into();
    let bottom_boundary: Wire<_, _> = vec![lower[0].clone(), lower[1].clone()].into();
    let top_boundary: Wire<_, _> = vec![upper[0].clone(), upper[1].clone()].into();
    Solid::new(vec![
        vec![
            Face::new(vec![side_boundary], side),
            Face::new(vec![bottom_boundary], bottom).inverse(),
            Face::new(vec![top_boundary], top),
        ]
        .into(),
    ])
}

/// An exact rational ring torus, major radius 6 and minor radius 2 mm.
///
/// Independent oracle: 1 face, 3 vertices, 4 edges (Euler characteristic 0).
/// At z=0 its one annular region has two loops, radii 8 and 4, and area
/// π(8²−4²)=48π mm². Physical bounds are [-8,8]² × [-2,2]. Both chart
/// directions retain their two original seam branches on [0,4]².
pub(super) fn nurbs_torus() -> StepSolid {
    torus_source(false)
}

/// The same exact torus with independently parameterized original seam leaders.
///
/// Every 3D leader uses t3D=10+2*tUV; original UV pcurves and their per-use
/// branches are unchanged. The source intervals are therefore [10,14] and
/// [14,18], independently of the pcurve intervals [0,2] and [2,4].
pub(super) fn nurbs_torus_independent_parameters() -> StepSolid {
    torus_source(true)
}

fn torus_source(independent_parameters: bool) -> StepSolid {
    const MAJOR: f64 = 6.0;
    const MINOR: f64 = 2.0;
    // Htheta=(X,Y,W), Hphi=(A,B,V). Tensor Bernstein coefficients:
    // ((R*V+r*A)*X, (R*V+r*A)*Y, r*B*W, V*W).
    // Every coefficient and every dehomogenized STEP control is dyadic.
    let tensor = |[x, y, w]: [f64; 3], [a, b, v]: [f64; 3]| {
        let radial = MAJOR * v + MINOR * a;
        Vector4::new(radial * x, radial * y, MINOR * b * w, v * w)
    };
    let surface = Surface::NurbsSurface(NurbsSurface::new(BsplineSurface::new(
        (circle_knots(), circle_knots()),
        CIRCLE
            .iter()
            .map(|&theta| CIRCLE.iter().map(|&phi| tensor(theta, phi)).collect())
            .collect(),
    )));
    // A closed STEP curve is split at t=2 so every kernel edge has distinct
    // endpoint vertices. This changes neither the carrier nor its seam chart.
    let corner = Vertex::new(Point3::new(8.0, 0.0, 0.0));
    let theta_middle = Vertex::new(Point3::new(-8.0, 0.0, 0.0));
    let phi_middle = Vertex::new(Point3::new(4.0, 0.0, 0.0));
    let leader_curve = |half: usize, controls: Vec<Vector4>| {
        let mut curve = NurbsCurve::new(BsplineCurve::new(half_knots(half), controls));
        if independent_parameters {
            curve.parameter_transform(2.0, 10.0);
        }
        Curve3D::NurbsCurve(curve)
    };
    let theta_edge = |half: usize, front: &Vertex<Point3>, back: &Vertex<Point3>| {
        let start = 2.0 * half as f64;
        let end = start + 2.0;
        let leader = leader_curve(
            half,
            CIRCLE[4 * half..=4 * half + 4]
                .iter()
                .map(|&theta| tensor(theta, CIRCLE[0]))
                .collect(),
        );
        Edge::new(
            front,
            back,
            seam(
                leader,
                &surface,
                uv_line(start, end, [start, 0.0], [end, 0.0]),
                uv_line(start, end, [start, 4.0], [end, 4.0]),
            ),
        )
    };
    let phi_edge = |half: usize, front: &Vertex<Point3>, back: &Vertex<Point3>| {
        let start = 2.0 * half as f64;
        let end = start + 2.0;
        let leader = leader_curve(
            half,
            CIRCLE[4 * half..=4 * half + 4]
                .iter()
                .map(|&phi| tensor(CIRCLE[0], phi))
                .collect(),
        );
        Edge::new(
            front,
            back,
            seam(
                leader,
                &surface,
                uv_line(start, end, [4.0, start], [4.0, end]),
                uv_line(start, end, [0.0, start], [0.0, end]),
            ),
        )
    };
    let theta = [
        theta_edge(0, &corner, &theta_middle),
        theta_edge(1, &theta_middle, &corner),
    ];
    let phi = [
        phi_edge(0, &corner, &phi_middle),
        phi_edge(1, &phi_middle, &corner),
    ];
    // Counterclockwise boundary of the original UV rectangle, before seam
    // identification. Branch order above follows each use's actual sign.
    let boundary: Wire<_, _> = vec![
        theta[0].clone(),
        theta[1].clone(),
        phi[0].clone(),
        phi[1].clone(),
        theta[1].inverse(),
        theta[0].inverse(),
        phi[1].inverse(),
        phi[0].inverse(),
    ]
    .into();
    Solid::new(vec![vec![Face::new(vec![boundary], surface)].into()])
}
