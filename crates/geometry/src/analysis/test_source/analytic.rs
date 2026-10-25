// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Native-authored analytic counterparts of the original parity constructions.
//! Quadrant faces, radial cap spokes and collapsed physical pole topology are
//! retained. Curves and PCURVEs are actual source operands; none is a polygonal
//! proxy or a substitute used only to obtain an analytic certificate.

use solidcraft_geom::brep as b;
use solidcraft_step::{geom as s, write::topology};
use std::sync::Arc;
use std::f64::consts::FRAC_PI_2;

const RADIAL: [[f64; 3]; 4] = [[1.0,0.0,0.0],[0.0,1.0,0.0],[-1.0,0.0,0.0],[0.0,-1.0,0.0]];
fn angle(i: usize) -> f64 { i as f64 * FRAC_PI_2 }
fn point(radius: f64, quadrant: usize, z: f64) -> [f64; 3] {
    let r = RADIAL[quadrant % 4];
    [radius*r[0],radius*r[1],z]
}
fn vertex(point: [f64; 3]) -> b::Vertex { b::Vertex { point, tolerance: 1.0e-6 } }
fn edge(curve: b::Curve, interval: [f64; 2], vertices: [u32; 2]) -> b::Edge {
    b::Edge { curve, nurbs: None, interval, vertices, tolerance: 1.0e-6, uses: vec![] }
}
fn uv(range: [f64; 2], a: [f64; 2], z: [f64; 2]) -> b::Curve2 {
    b::Curve2::Spline { knots: vec![range[0],range[0],range[1],range[1]], points: vec![a,z] }
}
fn use_(edge: usize, forward: bool, range: [f64; 2], a: [f64; 2], z: [f64; 2]) -> b::OrientedEdge {
    b::OrientedEdge { edge: edge as u32, forward, pcurve: uv(range,a,z) }
}
fn face(surface: b::Surface, domain: [[f64; 2]; 2], edges: Vec<b::OrientedEdge>) -> b::Face {
    b::Face { surface, nurbs: None, domain, loops: vec![b::Loop::Edges { edges }] }
}
fn frame(origin: [f64; 3], x_axis: [f64; 3], y_axis: [f64; 3]) -> b::Frame {
    b::Frame { origin,x_axis,y_axis }
}
fn xy_circle(radius: f64, z: f64) -> b::Curve {
    b::Curve::Circle { frame: frame([0.0,0.0,z],[1.0,0.0,0.0],[0.0,1.0,0.0]),radius }
}
fn sphere() -> b::Surface {
    b::Surface::Sphere { frame: frame([0.0;3],[1.0,0.0,0.0],[0.0,1.0,0.0]),radius:5.0 }
}
fn plane(z: f64) -> b::Surface {
    b::Surface::Plane { origin:[0.0,0.0,z],u_axis:[1.0,0.0,0.0],v_axis:[0.0,1.0,0.0] }
}

/// Original radius-5 sphere between z=-3 and z=3, closed by four sectors on
/// each planar cap. Retaining the two axis vertices and eight radial spokes
/// gives ten vertices, twenty edges and twelve faces, as the revolved recipe.
pub(super) fn sphere_belt() -> super::SourceBrep {
    let latitude = 3.0_f64.atan2(4.0);
    let source_meridian_range = [-latitude,latitude];
    // Exact rational 3-4-5 meridian: tangent intersection x=25/4 with
    // middle weight 4/5. A common homogeneous scale of 5 makes every
    // coefficient integral, without changing any source analytic operand.
    const MERIDIAN: [[f64;3];3] = [[20.0,-15.0,5.0],[25.0,0.0,4.0],[20.0,15.0,5.0]];
    const CIRCLE: [[f64;3];9] = [[1.0,0.0,1.0],[1.0,1.0,1.0],[0.0,2.0,2.0],
        [-2.0,2.0,2.0],[-4.0,0.0,4.0],[-4.0,-4.0,4.0],[0.0,-8.0,8.0],[8.0,-8.0,8.0],[16.0,0.0,16.0]];
    let quarter_knots = |i:usize| vec![i as f64,i as f64,i as f64,(i+1) as f64,(i+1) as f64,(i+1) as f64];
    let meridian_knots = || vec![0.0,0.0,0.0,1.0,1.0,1.0];
    let native_sphere = b::Surface::RationalSpline {
        u_knots:vec![0.0,0.0,0.0,1.0,1.0,2.0,2.0,3.0,3.0,4.0,4.0,4.0],
        v_knots:meridian_knots(),
        homogeneous:CIRCLE.iter().map(|&[x,y,w]| MERIDIAN.iter().map(|&[a,z,v]| [a*x,a*y,z*w,v*w]).collect()).collect(),
    };
    let mut vertices = vec![vertex([0.0,0.0,-3.0])];
    for z in [-3.0,3.0] { for i in 0..4 { vertices.push(vertex(point(4.0,i,z))); } }
    vertices.push(vertex([0.0,0.0,3.0]));
    let mut edges = Vec::with_capacity(20);
    for (z,base) in [(-3.0,1),(3.0,5)] { for i in 0..4 {
        let mut rim = edge(xy_circle(4.0,z),[i as f64,(i+1) as f64],[(base+i) as u32,(base+(i+1)%4) as u32]);
        rim.nurbs = Some(b::Curve::RationalSpline { knots:quarter_knots(i),
            homogeneous:CIRCLE[2*i..=2*i+2].iter().map(|&[x,y,w]| [4.0*x,4.0*y,z*w,w]).collect() });
        edges.push(rim);
    } }
    for (center,base) in [(0,1),(9,5)] { for i in 0..4 {
        edges.push(edge(b::Curve::Line { start:vertices[center].point,end:vertices[base+i].point },
            [0.0,1.0],[center as u32,(base+i) as u32]));
    } }
    for i in 0..4 {
        let mut meridian = edge(b::Curve::Circle { frame:frame([0.0;3],RADIAL[i],[0.0,0.0,1.0]),radius:5.0 },
            [0.0,1.0],[(1+i) as u32,(5+i) as u32]);
        meridian.nurbs = Some(b::Curve::RationalSpline { knots:meridian_knots(),
            homogeneous:MERIDIAN.iter().map(|&[a,z,w]| [a*RADIAL[i][0],a*RADIAL[i][1],z,w]).collect() });
        edges.push(meridian);
    }
    let cap_pcurve = |i:usize| b::Curve2::RationalSpline { knots:quarter_knots(i),
        homogeneous:CIRCLE[2*i..=2*i+2].iter().map(|&[x,y,w]| [4.0*x,4.0*y,w]).collect() };
    let native_spheres = [native_sphere.clone(),native_sphere.clone(),native_sphere.clone(),native_sphere];
    let mut faces = Vec::with_capacity(12);
    let mut senses = Vec::with_capacity(12);
    for (i,native_sphere) in native_spheres.into_iter().enumerate() {
        let next = (i+1)%4;
        let range = [i as f64,(i+1) as f64];
        let a = point(4.0,i,0.0); let z = point(4.0,next,0.0);
        let xy_a = [a[0],a[1]]; let xy_z = [z[0],z[1]];
        faces.push(face(plane(-3.0),[[-4.0,4.0];2],vec![
            use_(8+i,true,[0.0,1.0],[0.0;2],xy_a),
            b::OrientedEdge { edge:i as u32,forward:true,pcurve:cap_pcurve(i) },
            use_(8+next,false,[0.0,1.0],[0.0;2],xy_z),
        ]));
        senses.push(false);
        let mut side = face(sphere(),[range,[0.0,1.0]],vec![
            use_(i,true,range,[range[0],0.0],[range[1],0.0]),
            use_(16+next,true,[0.0,1.0],[range[1],0.0],[range[1],1.0]),
            use_(4+i,false,range,[range[0],1.0],[range[1],1.0]),
            use_(16+i,false,[0.0,1.0],[range[0],0.0],[range[0],1.0]),
        ]);
        side.nurbs = Some(native_sphere);
        faces.push(side);
        senses.push(true);
        faces.push(face(plane(3.0),[[-4.0,4.0];2],vec![
            use_(12+i,true,[0.0,1.0],[0.0;2],xy_a),
            b::OrientedEdge { edge:(4+i) as u32,forward:true,pcurve:cap_pcurve(i) },
            use_(12+next,false,[0.0,1.0],[0.0;2],xy_z),
        ]));
        senses.push(true);
    }
    let record = super::solid(vertices,edges,faces,senses);
    let face_surfaces:Vec<_> = record.faces.iter().map(|face|
        Some(topology::surface(&face.surface).expect("actual analytic source surface exports").0)).collect();
    let mut edge_curves = vec![None;record.edges.len()];
    for index in (0..8).chain(16..20) {
        let edge = &record.edges[index];
        let source_range = if index < 8 { [angle(index%4),angle(index%4+1)] } else { source_meridian_range };
        let associated = edge.uses.iter().map(|use_| {
            let surface = face_surfaces[use_.face as usize].as_ref().expect("actual source face is retained").clone();
            let curve = if use_.face % 3 == 1 {
                let quadrant = use_.face as usize/3;
                let curve = if index < 8 {
                    let v = if index < 4 { -latitude } else { latitude };
                    uv(source_range,[source_range[0],v],[source_range[1],v])
                } else {
                    let meridian = index-16;
                    let u = angle(if meridian == quadrant { quadrant } else { quadrant+1 });
                    uv(source_range,[u,-latitude],[u,latitude])
                };
                topology::pcurve(&curve,topology::Map::IDENTITY).expect("actual spherical angle pcurve exports")
            } else {
                trimmed(s::Curve::Circle { position:s::Placement {
                    origin:[0.0;3],axis:[0.0,0.0,1.0],ref_direction:[1.0,0.0,0.0],
                },radius:4.0 },source_range)
            };
            s::Associated::Pcurve(s::Pcurve { surface:Arc::new(surface),curve:Arc::new(curve) })
        }).collect();
        edge_curves[index] = Some(s::Curve::SurfaceCurve {
            kind:s::SurfaceCurveKind::Surface,
            curve_3d:Arc::new(trimmed(topology::curve(&edge.curve).expect("actual analytic source circle exports"),source_range)),
            associated,master:s::PreferredRep::Curve3d,
        });
    }
    super::SourceBrep { record,edge_curves,face_surfaces }
}


fn trimmed(basis:s::Curve, range:[f64;2]) -> s::Curve {
    s::Curve::Trimmed { basis:Arc::new(basis),trim_1:vec![s::Trim::Parameter(range[0])],
        trim_2:vec![s::Trim::Parameter(range[1])],sense:true,preference:s::TrimPreference::Parameter }
}

/// Original complete radius-5 sphere: four azimuth sectors times two
/// hemispheres, six physical vertices and twelve edges. Pole-adjacent faces
/// retain triangular physical circuits; no fake degenerate pole edge is added.
pub(super) fn full_sphere() -> b::Brep {
    let mut vertices = vec![vertex([0.0,0.0,-5.0])];
    for i in 0..4 { vertices.push(vertex(point(5.0,i,0.0))); }
    vertices.push(vertex([0.0,0.0,5.0]));
    let mut edges = Vec::with_capacity(12);
    for i in 0..4 { edges.push(edge(xy_circle(5.0,0.0),[angle(i),angle(i+1)],[(1+i) as u32,(1+(i+1)%4) as u32])); }
    for i in 0..4 { edges.push(edge(b::Curve::Circle {
        frame:frame([0.0;3],[0.0,0.0,-1.0],RADIAL[i]),radius:5.0 },[0.0,FRAC_PI_2],[0,(1+i) as u32])); }
    for i in 0..4 { edges.push(edge(b::Curve::Circle {
        frame:frame([0.0;3],RADIAL[i],[0.0,0.0,1.0]),radius:5.0 },[0.0,FRAC_PI_2],[(1+i) as u32,5])); }
    let mut faces = Vec::with_capacity(8);
    for i in 0..4 {
        let next = (i+1)%4;
        let range = [angle(i),angle(i+1)];
        faces.push(face(sphere(),[range,[-FRAC_PI_2,0.0]],vec![
            use_(4+i,false,[0.0,FRAC_PI_2],[range[0],-FRAC_PI_2],[range[0],0.0]),
            use_(4+next,true,[0.0,FRAC_PI_2],[range[1],-FRAC_PI_2],[range[1],0.0]),
            use_(i,false,range,[range[0],0.0],[range[1],0.0]),
        ]));
        faces.push(face(sphere(),[range,[0.0,FRAC_PI_2]],vec![
            use_(i,true,range,[range[0],0.0],[range[1],0.0]),
            use_(8+next,true,[0.0,FRAC_PI_2],[range[1],0.0],[range[1],FRAC_PI_2]),
            use_(8+i,false,[0.0,FRAC_PI_2],[range[0],0.0],[range[0],FRAC_PI_2]),
        ]));
    }
    super::solid(vertices,edges,faces,vec![true;8])
}

/// Original analytic R=5, r=2 ring torus, with all sixteen quadrant faces,
/// sixteen vertices and thirty-two edges. The generating minor-circle sweep
/// runs toward negative Z, as the original revolution about +Y did.
pub(super) fn ring_torus() -> b::Brep {
    let radii = [7.0,5.0,3.0,5.0];
    let heights = [0.0,-2.0,0.0,2.0];
    let index = |i:usize,j:usize| (4*(i%4)+j%4) as u32;
    let mut vertices = Vec::with_capacity(16);
    for i in 0..4 { for j in 0..4 { vertices.push(vertex(point(radii[j],i,heights[j]))); } }
    let mut edges = Vec::with_capacity(32);
    for i in 0..4 { for j in 0..4 {
        edges.push(edge(xy_circle(radii[j],heights[j]),[angle(i),angle(i+1)],[index(i,j),index(i+1,j)]));
    } }
    for i in 0..4 { for j in 0..4 {
        edges.push(edge(b::Curve::Circle { frame:frame(point(5.0,i,0.0),RADIAL[i],[0.0,0.0,-1.0]),radius:2.0 },
            [angle(j),angle(j+1)],[index(i,j),index(i,j+1)]));
    } }
    let major_edge = |i:usize,j:usize| 4*(i%4)+j%4;
    let minor_edge = |i:usize,j:usize| 16+4*(i%4)+j%4;
    let surface = b::Surface::Torus { frame:frame([0.0;3],[1.0,0.0,0.0],[0.0,1.0,0.0]),major_radius:5.0,minor_radius:2.0 };
    let mut faces = Vec::with_capacity(16);
    for i in 0..4 { for j in 0..4 {
        let u = [angle(i),angle(i+1)]; let a = [angle(j),angle(j+1)];
        let v = [-a[1],-a[0]];
        faces.push(face(surface.clone(),[u,v],vec![
            use_(major_edge(i,j+1),true,u,[u[0],v[0]],[u[1],v[0]]),
            use_(minor_edge(i+1,j),false,a,[u[1],v[1]],[u[1],v[0]]),
            use_(major_edge(i,j),false,u,[u[0],v[1]],[u[1],v[1]]),
            use_(minor_edge(i,j),true,a,[u[0],v[1]],[u[0],v[0]]),
        ]));
    } }
    super::solid(vertices,edges,faces,vec![true;16])
}
