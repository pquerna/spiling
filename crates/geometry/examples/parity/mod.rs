// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Genuine closed native constructions, not substitutions of STEP entity names.
mod rational;
use monstertruck_io::step::{
    load::step_geometry::{
        Conic3D, Curve3D, ElementarySurface, Surface, SweepSurface, re_exports::*,
    },
    save::{CompleteStepDisplay, StepHeaderDescriptor, StepModel},
};
use monstertruck_modeling::builder;
use monstertruck_topology::{Face, Solid, Wire};
use serde_json::{Value, json};

pub type StepSolid = Solid<Point3, Curve3D, Surface>;
type StepWire = Wire<Point3, Curve3D>;

pub fn rectangle(width: f64, depth: f64) -> StepWire {
    let vertices = builder::vertices([
        (0., 0., 0.),
        (width, 0., 0.),
        (width, depth, 0.),
        (0., depth, 0.),
    ]);
    (0..4)
        .map(|i| builder::line::<Curve3D>(&vertices[i], &vertices[(i + 1) % 4]))
        .collect()
}

pub fn circle(x: f64, y: f64, radius: f64) -> StepWire {
    builder::revolve(
        &builder::vertex((x + radius, y, 0.)),
        Point3::new(x, y, 0.),
        Vector3::unit_z(),
        builder::SweepAngle::Closed,
        2,
    )
}

fn analytic(surface: &Surface) -> Option<Surface> {
    let Surface::SweepSurface(SweepSurface::ExtrusionSurface(extrusion)) = surface else {
        return Some(surface.clone());
    };
    let vector = extrusion.extruding_vector();
    match extrusion.entity_curve().as_ref() {
        Curve3D::Line(line) => Some(Surface::ElementarySurface(ElementarySurface::Plane(
            Plane::new(line.0, line.1, line.0 + vector),
        ))),
        Curve3D::Conic(Conic3D::Ellipse(c)) => {
            let center = c.transform().transform_point(Point3::origin());
            let u = c.transform().transform_vector(Vector3::unit_x());
            let v = c.transform().transform_vector(Vector3::unit_y());
            if (u.magnitude() - v.magnitude()).abs() > 1e-10
                || u.dot(v).abs() > 1e-10
                || u.dot(vector).abs() > 1e-10
                || v.dot(vector).abs() > 1e-10
            {
                return None;
            }
            let point = extrusion.subs(0.37, 0.73);
            let axis = vector.normalize();
            let delta = point - center;
            let radial = delta - delta.dot(axis) * axis;
            let expected_outward = extrusion.normal(0.37, 0.73).dot(radial) > 0.0;
            let line = Line(center + u, center + u + vector);
            let mut processor =
                Processor::new(RevolutionSurface::by_revolution(line, center, axis));
            let p = processor.subs(0.37, 0.73);
            let d = p - center;
            let outward = processor.normal(0.37, 0.73).dot(d - d.dot(axis) * axis) > 0.0;
            if outward != expected_outward {
                processor.invert();
            }
            Some(Surface::ElementarySurface(
                ElementarySurface::CylindricalSurface(processor),
            ))
        }
        _ => None,
    }
}

pub fn extruded(wires: Vec<StepWire>, thickness: f64) -> StepSolid {
    let face: Face<Point3, Curve3D, Surface> =
        builder::try_attach_plane(wires).expect("original planar loops");
    let solid: StepSolid = builder::extrude(&face, thickness * Vector3::unit_z());
    let solid = solid
        .try_mapped(|p| Some(*p), |c| Some(c.clone()), analytic)
        .expect("original analytic extrusion carriers");
    for shell in solid.boundaries() {
        shell
            .check_solid_boundary()
            .expect("original closed manifold recipe");
    }
    solid
}

pub fn original(name: &str, solid: StepSolid) -> String {
    let compressed = solid.compress();
    let header = StepHeaderDescriptor {
        file_name: name.into(),
        time_stamp: "2026-01-01T00:00:00".into(),
        authors: vec!["Spiling contributors".into()],
        organization: vec!["Spiling".into()],
        organization_system: "Spiling original analytic corpus".into(),
        authorization: "OSL-3.0".into(),
    };
    // An authored SOURCE pcurve/seam curve must remain the actual edge geometry,
    // not become a leader hidden under a second generated SURFACE_CURVE wrapper.
    let authored_parameter_curves = solid
        .edge_iter()
        .any(|edge| edge.with_curve(|curve| matches!(curve, Curve3D::SurfaceCurve(_))));
    let model = if authored_parameter_curves {
        StepModel::from_curve3d_only_solid(&compressed)
    } else {
        StepModel::from(&compressed)
    };
    format!("{}", CompleteStepDisplay::new(model, header))
}

fn revolved_wire(mut wire: StepWire) -> StepSolid {
    // An axis-to-axis meridian is an OPEN generating wire. Its straight
    // axis closure encloses the 2D profile but sweeps no part of the solid's
    // boundary. Sweeping that closure creates an extra collapsed sheet and
    // disconnected vertex links, even after revolve_wire collapses endpoints.
    // Keep off-axis closures: they generate genuine inner walls and holes.
    if wire.is_closed()
        && wire.back().is_some_and(|edge| {
            edge.with_curve(|curve| {
                matches!(curve, Curve3D::Line(line)
                    if line.0.x == 0.0 && line.0.y == 0.0
                        && line.1.x == 0.0 && line.1.y == 0.0)
            })
        })
    {
        wire.pop_back();
    }
    // These meridians run from the lower side up the exterior in the r/Z
    // plane. The native profile-by-angle cross product points inward.
    // Reverse the assembled shell, not merely the shared coedge views, so
    // both the boundary senses and oriented carriers bound positive material.
    let mut shell = builder::revolve_wire::<Curve3D, Surface>(
        &wire,
        Point3::origin(),
        Vector3::unit_z(),
        builder::SweepAngle::Closed,
        4,
    );
    for face in shell.iter_mut() {
        face.invert();
    }
    Solid::new(vec![shell])
}

fn revolved_profile(points: &[(f64, f64)]) -> StepSolid {
    let vertices = builder::vertices(points.iter().map(|&(r, z)| (r, 0.0, z)));
    let wire: StepWire = (0..vertices.len())
        .map(|i| builder::line::<Curve3D>(&vertices[i], &vertices[(i + 1) % vertices.len()]))
        .collect();
    // Declare the full native carrier from the original profile itself.
    // Quadrant placement belongs to the retained trim topology, not a rounded
    // sin/cos processor frame that an exact STEP producer cannot represent.
    revolved_wire(wire)
        .try_mapped(
            |point| Some(*point),
            |curve| Some(curve.clone()),
            |surface| {
                let Surface::SweepSurface(SweepSurface::RevolutionSurface(processor)) = surface
                else {
                    return Some(surface.clone());
                };
                let profile = processor.entity().entity_curve().clone();
                let replacement = Surface::SweepSurface(SweepSurface::RevolutionSurface(
                    Processor::new(RevolutionSurface::by_revolution(
                        profile,
                        Point3::origin(),
                        Vector3::unit_z(),
                    )),
                ));
                Some(oriented(surface, replacement))
            },
        )
        .expect("original source-profile revolution carriers in exact coordinate-axis frames")
}

fn oriented(surface: &Surface, mut replacement: Surface) -> Surface {
    // Source construction knows the exact replacement carrier. This test only
    // chooses its sign; numerical admission/certification is still exercised.
    let uv = replacement
        .search_parameter(surface.subs(0.31, 0.43), None, 100)
        .expect("exact original replacement carrier contains native source point");
    if surface
        .normal(0.31, 0.43)
        .dot(replacement.normal(uv.0, uv.1))
        < 0.0
    {
        replacement.invert();
    }
    replacement
}

fn cone() -> StepSolid {
    let solid = revolved_profile(&[(0.0, 0.0), (5.0, 0.0), (3.0, 8.0), (0.0, 8.0)]);
    solid
        .try_mapped(
            |p| Some(*p),
            |c| Some(c.clone()),
            |s| {
                let Surface::SweepSurface(SweepSurface::RevolutionSurface(revolution)) = s else {
                    return Some(s.clone());
                };
                let Curve3D::Line(line) = revolution.entity().entity_curve().as_ref() else {
                    return Some(s.clone());
                };
                let delta = line.1 - line.0;
                if delta.z.abs() < 1e-10 {
                    return Some(oriented(
                        s,
                        Surface::ElementarySurface(ElementarySurface::Plane(Plane::new(
                            line.0,
                            line.1,
                            line.0 + Vector3::unit_y(),
                        ))),
                    ));
                }
                let processor = Processor::new(RevolutionSurface::by_revolution(
                    Line(Point3::new(5.0, 0.0, 0.0), Point3::new(4.75, 0.0, 1.0)),
                    Point3::origin(),
                    Vector3::unit_z(),
                ));
                Some(oriented(
                    s,
                    Surface::ElementarySurface(ElementarySurface::ConicalSurface(processor)),
                ))
            },
        )
        .expect("exact original frustum carriers")
}

fn torus() -> StepSolid {
    let circle: StepWire = builder::revolve(
        &builder::vertex((7.0, 0.0, 0.0)),
        Point3::new(5.0, 0.0, 0.0),
        Vector3::unit_y(),
        builder::SweepAngle::Closed,
        4,
    );
    let shell = builder::revolve(
        &circle,
        Point3::origin(),
        Vector3::unit_z(),
        builder::SweepAngle::Closed,
        4,
    );
    let solid = Solid::new(vec![shell]);
    solid
        .try_mapped(
            |p| Some(*p),
            |c: &Curve3D| Some(c.clone()),
            |s: &Surface| {
                Some(oriented(
                    s,
                    Surface::ElementarySurface(ElementarySurface::ToroidalSurface(Processor::new(
                        Torus::new(Point3::origin(), 5.0, 2.0),
                    ))),
                ))
            },
        )
        .expect("exact original ring torus carriers")
}

fn sphere_belt() -> StepSolid {
    let vertices = builder::vertices([
        (0.0, 0.0, -3.0),
        (4.0, 0.0, -3.0),
        (4.0, 0.0, 3.0),
        (0.0, 0.0, 3.0),
    ]);
    let wire: StepWire = vec![
        builder::line(&vertices[0], &vertices[1]),
        builder::circle_arc(&vertices[1], &vertices[2], Point3::new(5.0, 0.0, 0.0)),
        builder::line(&vertices[2], &vertices[3]),
        builder::line(&vertices[3], &vertices[0]),
    ]
    .into();
    revolved_wire(wire)
        .try_mapped(
            |p| Some(*p),
            |c| Some(c.clone()),
            |s| {
                let Surface::SweepSurface(SweepSurface::RevolutionSurface(revolution)) = s else {
                    return Some(s.clone());
                };
                match revolution.entity().entity_curve().as_ref() {
                    Curve3D::Line(line) => Some(oriented(
                        s,
                        Surface::ElementarySurface(ElementarySurface::Plane(Plane::new(
                            line.0,
                            line.1,
                            line.0 + Vector3::unit_y(),
                        ))),
                    )),
                    _ => Some(oriented(
                        s,
                        Surface::ElementarySurface(ElementarySurface::Sphere(Processor::new(
                            monstertruck_io::step::load::step_geometry::Sphere(
                                monstertruck_modeling::Sphere::new(Point3::origin(), 5.0),
                            ),
                        ))),
                    )),
                }
            },
        )
        .expect("exact original regular sphere belt carriers")
}

fn sphere() -> StepSolid {
    let radius = 5.0;
    let diagonal = radius / 2.0_f64.sqrt();
    let vertices = builder::vertices([(0.0, 0.0, -radius), (radius, 0.0, 0.0), (0.0, 0.0, radius)]);
    // Separate native hemispherical faces keep each eventual regular chart
    // strictly inside one geometric hemisphere, including its physical pole.
    let wire: StepWire = vec![
        builder::circle_arc(
            &vertices[0],
            &vertices[1],
            Point3::new(diagonal, 0.0, -diagonal),
        ),
        builder::circle_arc(
            &vertices[1],
            &vertices[2],
            Point3::new(diagonal, 0.0, diagonal),
        ),
        builder::line(&vertices[2], &vertices[0]),
    ]
    .into();
    revolved_wire(wire)
        .try_mapped(
            |point| Some(*point),
            |curve| Some(curve.clone()),
            |surface| {
                Some(oriented(
                    surface,
                    Surface::ElementarySurface(ElementarySurface::Sphere(Processor::new(
                        monstertruck_io::step::load::step_geometry::Sphere(
                            monstertruck_modeling::Sphere::new(Point3::origin(), radius),
                        ),
                    ))),
                ))
            },
        )
        .expect("exact original complete sphere carriers and collapsed pole topology")
}

fn spline_box(rational: bool) -> StepSolid {
    extruded(vec![rectangle(20.0, 10.0)], 8.0)
        .try_mapped(
            |p| Some(*p),
            |c| Some(c.clone()),
            |surface| {
                let Surface::ElementarySurface(ElementarySurface::Plane(plane)) = surface else {
                    return None;
                };
                let spline = BsplineSurface::<Point3>::from(*plane);
                Some(if rational {
                    Surface::NurbsSurface(NurbsSurface::new(BsplineSurface::lift_up(spline)))
                } else {
                    Surface::BsplineSurface(spline)
                })
            },
        )
        .expect("exact original bounded polynomial/rational box carriers")
}

fn spline_extrusion(hole: bool) -> StepSolid {
    let vertices = builder::vertices([
        (0.0, 0.0, 0.0),
        (20.0, 0.0, 0.0),
        (20.0, 10.0, 0.0),
        (0.0, 10.0, 0.0),
    ]);
    let wire: StepWire = vec![
        builder::bezier(
            &vertices[0],
            &vertices[1],
            vec![Point3::new(5.0, -2.0, 0.0), Point3::new(15.0, -2.0, 0.0)],
        ),
        builder::line(&vertices[1], &vertices[2]),
        builder::line(&vertices[2], &vertices[3]),
        builder::line(&vertices[3], &vertices[0]),
    ]
    .into();
    let mut wires = vec![wire];
    if hole {
        wires.push(circle(10.0, 5.0, 2.0).inverse());
    }
    let face: Face<Point3, Curve3D, Surface> =
        builder::try_attach_plane(wires).expect("original closed Bezier profile");
    builder::extrude(&face, 8.0 * Vector3::unit_z())
}

/// Genuine sewn B-rep: the cavity reuses the original plug rim edges in the
/// outer top ring. Its floor and walls are the reversed original plug faces.
pub fn blind_pocket() -> StepSolid {
    let outer = extruded(vec![rectangle(20., 20.)], 8.);
    let plug = extruded(vec![circle(10., 10., 3.)], 4.)
        .try_mapped(
            |point| Some(*point + 4. * Vector3::unit_z()),
            |curve| Some(curve.transformed(Matrix4::from_translation(4. * Vector3::unit_z()))),
            |surface| Some(surface.transformed(Matrix4::from_translation(4. * Vector3::unit_z()))),
        )
        .expect("exact original pocket plug translation");
    let is_top = |face: &Face<Point3, Curve3D, Surface>| {
        face.vertex_iter().all(|vertex| vertex.point().z == 8.)
    };
    let plug_top = plug
        .face_iter()
        .find(|face| is_top(face))
        .expect("original plug top");
    let mut faces = Vec::new();
    for face in outer.face_iter() {
        if is_top(face) {
            let mut boundaries = face.boundaries();
            boundaries.extend(plug_top.boundaries().into_iter().map(|wire| wire.inverse()));
            faces.push(Face::new(boundaries, face.oriented_surface()));
        } else {
            faces.push(face.clone());
        }
    }
    faces.extend(
        plug.face_iter()
            .filter(|face| !is_top(face))
            .map(|face| face.inverse()),
    );
    let shell = faces
        .into_iter()
        .collect::<monstertruck_topology::Shell<_, _, _>>();
    shell
        .check_solid_boundary()
        .expect("original sewn blind pocket manifold");
    Solid::new(vec![shell])
}

/// Same exact pocket, with a genuinely bounded rational tensor floor instead
/// of an elementary plane. Integer homogeneous controls cover the complete disk.
pub fn blind_pocket_nurbs_floor() -> StepSolid {
    blind_pocket()
        .try_mapped(
            |point| Some(*point),
            |curve| Some(curve.clone()),
            |surface| {
                let Surface::ElementarySurface(ElementarySurface::Plane(plane)) = surface else {
                    return Some(surface.clone());
                };
                if plane.origin().z != 4. {
                    return Some(surface.clone());
                }
                let knots = KnotVector::from(vec![0., 0., 1., 1.]);
                let spline = BsplineSurface::new(
                    (knots.clone(), knots),
                    vec![
                        vec![Vector4::new(7., 7., 4., 1.), Vector4::new(7., 13., 4., 1.)],
                        vec![
                            Vector4::new(13., 7., 4., 1.),
                            Vector4::new(13., 13., 4., 1.),
                        ],
                    ],
                );
                let mut replacement = Surface::NurbsSurface(NurbsSurface::new(spline));
                if plane.normal().z < 0. {
                    replacement.invert();
                }
                Some(replacement)
            },
        )
        .expect("exact original bounded rational pocket floor")
}

pub fn recipes() -> Vec<(&'static str, StepSolid, Value)> {
    vec![
        (
            "native-parity-cone-frustum.step",
            cone(),
            json!({"section_z_mm":4,"section_area_expression":"16*pi","family":"cone"}),
        ),
        (
            "native-parity-sphere-belt.step",
            sphere_belt(),
            json!({"section_z_mm":0,"section_area_expression":"25*pi","family":"sphere","regular_domain":true}),
        ),
        (
            "native-parity-full-sphere.step",
            sphere(),
            json!({"section_z_mm":0,"section_area_expression":"25*pi","family":"sphere","physical_bounds_mm":{"min":[-5,-5,-5],"max":[5,5,5]},"pole_heights_mm":[-5,5]}),
        ),
        (
            "native-parity-ring-torus.step",
            torus(),
            json!({"section_z_mm":0,"section_loop_count":2,"section_area_expression":"40*pi","family":"torus"}),
        ),
        (
            "native-parity-bspline-box.step",
            spline_box(false),
            json!({"section_z_mm":4,"section_area_mm2":200,"family":"bspline"}),
        ),
        (
            "native-parity-nurbs-cylinder.step",
            rational::nurbs_cylinder(),
            json!({"section_z_mm":4,"section_area_expression":"25*pi","family":"nurbs","face_count":3,"physical_bounds_mm":{"min":[-5,-5,0],"max":[5,5,8]}}),
        ),
        (
            "native-parity-nurbs-torus.step",
            rational::nurbs_torus(),
            json!({"section_z_mm":0,"section_loop_count":2,"section_area_expression":"48*pi","family":"nurbs","face_count":1,"physical_bounds_mm":{"min":[-8,-8,-2],"max":[8,8,2]}}),
        ),
        (
            "native-parity-nurbs-torus-independent-parameters.step",
            rational::nurbs_torus_independent_parameters(),
            json!({"section_z_mm":0,"section_loop_count":2,"section_area_expression":"48*pi","family":"nurbs","face_count":1,"independent_curve_parameter":true,"physical_bounds_mm":{"min":[-8,-8,-2],"max":[8,8,2]}}),
        ),
        (
            "native-parity-nurbs-box.step",
            spline_box(true),
            json!({"section_z_mm":4,"section_area_mm2":200,"family":"nurbs"}),
        ),
        (
            "native-parity-bezier-extrusion.step",
            spline_extrusion(false),
            json!({"section_z_mm":4,"section_area_mm2":221,"family":"linear_extrusion"}),
        ),
        (
            "native-parity-bezier-extrusion-bore.step",
            spline_extrusion(true),
            json!({"section_z_mm":4,"section_loop_count":2,"section_area_expression":"221-4*pi","family":"linear_extrusion"}),
        ),
        (
            "native-parity-rotated-bezier-bore.step",
            {
                let rotation = Matrix4::from_cols(
                    Vector4::new(0.0, 1.0, 0.0, 0.0),
                    Vector4::new(-1.0, 0.0, 0.0, 0.0),
                    Vector4::new(0.0, 0.0, 1.0, 0.0),
                    Vector4::new(0.0, 0.0, 0.0, 1.0),
                );
                spline_extrusion(true)
                    .try_mapped(
                        |point| Some(rotation.transform_point(*point)),
                        |curve| Some(curve.transformed(rotation)),
                        |surface| Some(surface.transformed(rotation)),
                    )
                    .expect("original exactly rotated trimmed Bezier mechanical part")
            },
            json!({"section_z_mm":4,"section_loop_count":2,"section_area_expression":"221-4*pi",
            "family":"linear_extrusion","physical_bounds_mm":{"min":[-10,0,0],"max":[1.5,20,8]}}),
        ),
        (
            "native-parity-revolution.step",
            revolved_profile(&[(2.0, 0.0), (5.0, 0.0), (3.0, 8.0), (2.0, 8.0)]),
            json!({"section_z_mm":4,"section_area_expression":"12*pi","family":"revolution"}),
        ),
        (
            "native-parity-blind-pocket.step",
            blind_pocket(),
            json!({"section_z_mm":6,
            "section_loop_count":2,"section_area_expression":"400-9*pi","family":"plane-cylinder",
            "face_count":9,"floor_height_mm":4,"physical_bounds_mm":{"min":[0,0,0],"max":[20,20,8]}}),
        ),
        (
            "native-parity-blind-pocket-nurbs-floor.step",
            blind_pocket_nurbs_floor(),
            json!({"section_z_mm":6,
            "section_loop_count":2,"section_area_expression":"400-9*pi","family":"nurbs-plane-cylinder",
            "face_count":9,"floor_height_mm":4,"physical_bounds_mm":{"min":[0,0,0],"max":[20,20,8]}}),
        ),
    ]
}

/// These are ORIGINAL independent-exporter sources, not this kernel's roundtrip
/// recipes. Consumers read their unchanged checked-in bytes and source hashes.
pub fn independent_recipes() -> [(&'static str, Value); 7] {
    [
        (
            "independent-exporters/occt-box-mm.step",
            json!({"face_count":6,"section_z_mm":4,
            "section_area_mm2":200,"physical_bounds_mm":{"min":[0,0,0],"max":[20,10,8]}}),
        ),
        (
            "independent-exporters/occt-cylinder-mm.step",
            json!({"face_count":3,"section_z_mm":4,
            "section_area_expression":"25*pi","physical_bounds_mm":{"min":[-5,-5,0],"max":[5,5,8]}}),
        ),
        (
            "independent-exporters/occt-through-hole-mm.step",
            json!({"face_count":7,"section_z_mm":4,
            "section_loop_count":2,"section_area_expression":"400-9*pi",
            "physical_bounds_mm":{"min":[0,0,0],"max":[20,20,8]}}),
        ),
        (
            "independent-exporters/occt-tapered-fitting-mm.step",
            json!({"section_z_mm":6,
            "section_loop_count":2,"section_area_mm2":65.97344572538566,
            "physical_bounds_mm":{"min":[-6,-6,0],"max":[6,6,12]}}),
        ),
        (
            "independent-exporters/occt-rounded-housing-mm.step",
            json!({"section_z_mm":4.16,
            "section_loop_count":2,"section_area_mm2":400.29203673205103,"floor_height_mm":4.13,
            "physical_bounds_mm":{"min":[0,0,0],"max":[24,18,10]}}),
        ),
        (
            "independent-exporters/occt-spline-housing-mm.step",
            json!({"section_z_mm":4.16,
            "section_loop_count":2,"section_area_mm2":400.29203673205103,"floor_height_mm":4.13,
            "physical_bounds_mm":{"min":[0,0,0],"max":[24,18,10]}}),
        ),
        (
            "independent-exporters/occt-narrow-pocket-mm.step",
            json!({"section_z_mm":1.1,
            "section_loop_count":2,"section_area_mm2":115.2,"floor_height_mm":1.07,
            "physical_bounds_mm":{"min":[0,0,0],"max":[12,10,6]}}),
        ),
    ]
}

/// A denominator zero between entire opposite-sign rows, not an importer enum case.
pub fn conditioning_source() -> String {
    let solid = spline_box(true)
        .try_mapped(
            |p| Some(*p),
            |c| Some(c.clone()),
            |s| {
                let Surface::NurbsSurface(nurbs) = s else {
                    return Some(s.clone());
                };
                let mut nurbs = nurbs.clone();
                for u in 0..2 {
                    *nurbs.control_point_mut(u, 1) *= -1.0;
                }
                Some(Surface::NurbsSurface(nurbs))
            },
        )
        .expect("original denominator-conditioning source recipe");
    original("native-parity-denominator-zero.step", solid)
}

/// Polynomial vanishing at every eighth, including midpoint. Its squared
/// oscillation lies strictly outside the ordinary rectangle between samples.
pub fn aliasing_source() -> String {
    let mut polynomial = vec![1.0];
    for i in 0..=8 {
        let root = i as f64 / 8.0;
        let mut next = vec![0.0; polynomial.len() + 1];
        for (k, value) in polynomial.iter().enumerate() {
            next[k] -= root * value;
            next[k + 1] += value;
        }
        polynomial = next;
    }
    let mut squared = vec![0.0; 19];
    for (i, a) in polynomial.iter().enumerate() {
        for (j, b) in polynomial.iter().enumerate() {
            squared[i + j] -= 1e9 * a * b;
        }
    }
    let choose = |n: usize, k: usize| -> f64 {
        (0..k).fold(1.0, |value, i| value * (n - i) as f64 / (i + 1) as f64)
    };
    let controls: Vec<_> = (1..18)
        .map(|i| {
            let y = (0..=i)
                .map(|k| squared[k] * choose(i, k) / choose(18, k))
                .sum::<f64>();
            Point3::new(20.0 * i as f64 / 18.0, y, 0.0)
        })
        .collect();
    let vertices = builder::vertices([
        (0.0, 0.0, 0.0),
        (20.0, 0.0, 0.0),
        (20.0, 10.0, 0.0),
        (0.0, 10.0, 0.0),
    ]);
    let wire: StepWire = vec![
        builder::bezier(&vertices[0], &vertices[1], controls),
        builder::line(&vertices[1], &vertices[2]),
        builder::line(&vertices[2], &vertices[3]),
        builder::line(&vertices[3], &vertices[0]),
    ]
    .into();
    let face: Face<Point3, Curve3D, Surface> =
        builder::try_attach_plane(vec![wire]).expect("original aliasing polynomial profile");
    original(
        "native-parity-aliasing-extrusion.step",
        builder::extrude(&face, 8.0 * Vector3::unit_z()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn parity_originals_are_closed_manifolds() {
        for (name, solid, expectation) in recipes() {
            assert_eq!(solid.boundaries().len(), 1, "{name}");
            for shell in solid.boundaries() {
                shell.check_solid_boundary().expect(name);
            }
            if let Some(count) = expectation["face_count"].as_u64() {
                assert_eq!(solid.face_iter().count() as u64, count, "{name}");
            }
        }
    }

    #[test]
    fn axis_profiles_retain_single_manifold_poles_without_axis_sheets() {
        for (solid, lower, upper) in [
            (cone(), 0.0, 8.0),
            (sphere_belt(), -3.0, 3.0),
            (sphere(), -5.0, 5.0),
        ] {
            let vertices: HashSet<_> = solid.vertex_iter().map(|vertex| vertex.id()).collect();
            let edges: HashSet<_> = solid.edge_iter().map(|edge| edge.id()).collect();
            assert_eq!(vertices.len() + solid.face_iter().count(), edges.len() + 2);
            for height in [lower, upper] {
                let pole_vertices: HashSet<_> = solid
                    .vertex_iter()
                    .filter(|vertex| vertex.point() == Point3::new(0.0, 0.0, height))
                    .map(|vertex| vertex.id())
                    .collect();
                assert_eq!(pole_vertices.len(), 1);
            }
            assert!(solid.edge_iter().all(|edge| {
                let front = edge.front().point();
                let back = edge.back().point();
                front.x != 0.0 || front.y != 0.0 || back.x != 0.0 || back.y != 0.0
            }));
        }
    }
}
