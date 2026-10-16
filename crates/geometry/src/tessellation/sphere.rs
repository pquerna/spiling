// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! A regular physical chart for native spherical faces, including pole uses.
//! The proposed chart is accepted only after whole-material-domain enclosure.
use super::{PolygonMesh, coverage, invalid};
use crate::{
    GeometryWorkBudget,
    kernel::{Definition, PreparedFaceTrimDomain, TrimClassification},
    parametric::{self, Interval, PreparedCurveEnclosure},
};
use monstertruck_modeling::{BoundedCurve, Point3, SearchParameter, Surface, TrimJointKind};
use spiling_contracts::geometry::{AnalyticSurfaceDescription, GeometryError, GeometryErrorCode};
use std::{collections::HashMap, sync::atomic::AtomicBool};
type Corner = coverage::Corner<[Interval; 2]>;

/// Bind by the original positional occurrence and verify the retained identity.
/// Repeated seam uses must not be resolved by shared edge ID or nearest geometry.
fn original_edge<'a>(
    face: &'a monstertruck_modeling::Face,
    wire: usize,
    coedge: usize,
    retained: &crate::kernel::BoundaryCoedgeRef,
) -> Result<&'a monstertruck_modeling::Edge, GeometryError> {
    let edge = face
        .absolute_boundaries()
        .get(wire)
        .and_then(|wire| wire.get(coedge))
        .ok_or_else(|| invalid("spherical source coedge occurrence is missing"))?;
    if edge.id() != retained.edge || edge.orientation() != retained.orientation {
        return Err(invalid(
            "spherical source coedge identity or orientation changed",
        ));
    }
    Ok(edge)
}

pub(super) struct SphereChart<'a> {
    trim: PreparedFaceTrimDomain<'a>,
    center: [f64; 3],
    radius: f64,
    x: [f64; 3],
    y: [f64; 3],
    normal: [f64; 3],
    inverse_lipschitz_mm: f64,
    carrier_error_mm: f64,
    pole_error_mm: f64,
    hemisphere_lower_mm: f64,
    reserved_bytes: usize,
}
impl SphereChart<'_> {
    pub(super) fn native_retained_bytes(&self) -> usize {
        self.trim.retained_bytes()
    }
}

impl<'a> SphereChart<'a> {
    pub(super) fn prepare(
        definition: &'a Definition,
        ordinal: usize,
        polygon: &PolygonMesh,
        extra_retained_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        let Some(AnalyticSurfaceDescription::Sphere { frame, radius_mm }) =
            definition.faces[ordinal].geometry.analytic.as_ref()
        else {
            return Ok(None);
        };
        let reserved_bytes =
            coverage::proof_memory_bound(polygon)?.saturating_add(extra_retained_bytes);
        coverage::check_memory(reserved_bytes)?;
        let trim = crate::kernel::prepare_face_trim_domain_with_byte_limit(
            definition,
            ordinal,
            coverage::PROOF_BYTES.saturating_sub(reserved_bytes),
            work,
            cancel,
        )?;
        let center = frame.origin_mm;
        // Original native topology, not display samples, proposes the chart.
        let mut normal = [0.0; 3];
        for (wire_index, wire) in definition.face_boundaries[ordinal].wires.iter().enumerate() {
            for (coedge_index, coedge) in wire.iter().enumerate() {
                work.charge(1, cancel)?;
                let edge = original_edge(trim.native().face(), wire_index, coedge_index, coedge)?;
                let point = edge.front().point();
                for axis in 0..3 {
                    normal[axis] += [point.x, point.y, point.z][axis] - center[axis];
                }
            }
        }
        let dominant = (0..3)
            .max_by(|&a, &b| normal[a].abs().total_cmp(&normal[b].abs()))
            .unwrap();
        let scale = normal[dominant].abs();
        if !scale.is_finite() || scale == 0.0 {
            return Err(unresolved(
                "spherical face needs more than one certified hemispherical chart",
            ));
        }
        normal = normal.map(|value| value / scale);
        let axes = [(dominant + 1) % 3, (dominant + 2) % 3];
        let mut x = [0.0; 3];
        let mut y = [0.0; 3];
        x[axes[0]] = 1.0;
        y[axes[1]] = 1.0;
        let determinant = dot(
            cross(x.map(Interval::point), y.map(Interval::point)),
            normal,
        );
        if determinant.hi < 0.0 {
            std::mem::swap(&mut x, &mut y);
        }
        let carrier_error_mm = trim
            .native()
            .face()
            .with_surface(|surface| native_sphere_error(surface, center, *radius_mm))?;
        let inverse_lipschitz_mm = inverse_lipschitz(x, y, normal, *radius_mm)?;
        let mut chart = Self {
            trim,
            center,
            radius: *radius_mm,
            x,
            y,
            normal,
            inverse_lipschitz_mm,
            carrier_error_mm,
            pole_error_mm: 0.0,
            hemisphere_lower_mm: f64::INFINITY,
            reserved_bytes,
        };
        chart.prove_hemisphere(work, cancel)?;
        for joint in chart.trim.native().joints() {
            if let TrimJointKind::SphericalPole {
                world_enclosure, ..
            } = &joint.kind
            {
                let point = [
                    joint.vertex_point.x,
                    joint.vertex_point.y,
                    joint.vertex_point.z,
                ];
                let displacement: [Interval; 3] = std::array::from_fn(|axis| {
                    Interval {
                        lo: world_enclosure[axis].lower,
                        hi: world_enclosure[axis].upper,
                    }
                    .sub(Interval::point(point[axis]))
                });
                chart.pole_error_mm = chart.pole_error_mm.max(norm_upper(displacement)?);
            }
        }
        Ok(Some(chart))
    }

    fn prove_hemisphere(
        &mut self,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        let mut stack = [([[0.0; 2]; 2], 0_u32); 25];
        stack[0] = (self.trim.uv_bounds(), 0);
        let mut count = 1;
        while count != 0 {
            count -= 1;
            let (region, depth) = stack[count];
            work.charge(1, cancel)?;
            if self.trim.classify(region, work, cancel)? == TrimClassification::Outside {
                continue;
            }
            let jet = self.trim.native().face().with_surface(|surface| {
                parametric::surface_enclosure(surface, region, work, cancel)
            })?;
            let displacement = std::array::from_fn(|axis| {
                jet.position[axis].sub(Interval::point(self.center[axis]))
            });
            let denominator = dot(displacement, self.normal);
            if denominator.lo > 0.0 {
                self.hemisphere_lower_mm = self.hemisphere_lower_mm.min(denominator.lo);
                continue;
            }
            if denominator.hi <= 0.0 || depth == 24 {
                return Err(unresolved(
                    "whole spherical face does not fit the proposed regular projective chart",
                ));
            }
            let axis = usize::from(region[1][1] - region[1][0] > region[0][1] - region[0][0]);
            let middle = region[axis][0] * 0.5 + region[axis][1] * 0.5;
            if middle == region[axis][0] || middle == region[axis][1] {
                return Err(unresolved(
                    "spherical chart subdivision exhausted floating point separation",
                ));
            }
            let mut left = region;
            let mut right = region;
            left[axis][1] = middle;
            right[axis][0] = middle;
            stack[count] = (right, depth + 1);
            stack[count + 1] = (left, depth + 1);
            count += 2;
        }
        Ok(())
    }

    /// Collapse only an explicitly witnessed native pole stratum. The complete
    /// source chart tube's displacement to the ORIGINAL shared vertex is charged.
    pub(super) fn canonicalize_poles(
        &self,
        polygon: &mut PolygonMesh,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        let mut bindings = HashMap::new();
        for triangle in polygon.tri_faces() {
            for vertex in triangle {
                work.charge(1, cancel)?;
                let uv = vertex
                    .uv
                    .and_then(|i| polygon.uv_coords().get(i))
                    .ok_or_else(|| invalid("pole quotient requires native mesh parameters"))?;
                for (index, joint) in self.trim.native().joints().iter().enumerate() {
                    if !matches!(&joint.kind, TrimJointKind::SphericalPole { .. }) {
                        continue;
                    }
                    if (0..2).all(|axis| {
                        joint.enclosure[axis].lower <= [uv.x, uv.y][axis]
                            && [uv.x, uv.y][axis] <= joint.enclosure[axis].upper
                    }) {
                        if !bindings.contains_key(&vertex.pos)
                            && bindings.len() == bindings.capacity()
                        {
                            coverage::check_memory(
                                self.reserved_bytes
                                    .saturating_add(self.trim.retained_bytes())
                                    .saturating_add(coverage::map_bytes::<usize, usize>(
                                        bindings.capacity().saturating_mul(2).max(8),
                                    )),
                            )?;
                            work.charge(1, cancel)?;
                            bindings.try_reserve(1).map_err(|_| {
                                super::resource("native pole binding allocation failed")
                            })?;
                        }
                        if bindings.insert(vertex.pos, index).is_some_and(|old| {
                            self.trim.native().joints()[old].vertex_id != joint.vertex_id
                        }) {
                            return Err(invalid(
                                "mesh pole parameter binds distinct original native vertices",
                            ));
                        }
                    }
                }
            }
        }
        let editor = polygon.uncheck_editor();
        for (&position, &joint) in &bindings {
            let original = editor
                .attributes
                .positions
                .get_mut(position)
                .ok_or_else(|| invalid("pole quotient position is out of range"))?;
            let target = self.trim.native().joints()[joint].vertex_point;
            let delta: [Interval; 3] = std::array::from_fn(|axis| {
                Interval::point([original.x, original.y, original.z][axis])
                    .sub(Interval::point([target.x, target.y, target.z][axis]))
            });
            if norm_upper(delta)? > self.pole_error_mm {
                return Err(invalid(
                    "mesh pole lies outside the retained whole-stratum native residual",
                ));
            }
            *original = target;
        }
        if !editor.faces.quad_faces().is_empty() || !editor.faces.other_faces().is_empty() {
            return Err(invalid(
                "spherical pole quotient requires triangle-only native mesh",
            ));
        }
        let removed = |triangle: &[monstertruck_meshing::prelude::StandardVertex; 3]| {
            let bound = triangle.map(|vertex| bindings.get(&vertex.pos).copied());
            [(0, 1), (1, 2), (2, 0)].into_iter().any(|(a, b)| {
                bound[a].is_some()
                    && bound[a].is_some_and(|left| {
                        bound[b].is_some_and(|right| {
                            self.trim.native().joints()[left].vertex_id
                                == self.trim.native().joints()[right].vertex_id
                        })
                    })
            })
        };
        let mut retained = 0_usize;
        for triangle in editor.faces.tri_faces() {
            work.charge(1, cancel)?;
            if !removed(triangle) {
                retained += 1;
            }
        }
        if retained != editor.faces.tri_faces().len() {
            // The public mesh API exposes only a mutable slice. Account for the
            // replacement vector while the original face storage is still live.
            coverage::check_memory(
                self.reserved_bytes
                    .saturating_add(self.trim.retained_bytes())
                    .saturating_add(coverage::map_bytes::<usize, usize>(bindings.capacity()))
                    .saturating_add(retained.saturating_mul(std::mem::size_of::<
                        [monstertruck_meshing::prelude::StandardVertex; 3],
                    >())),
            )?;
            work.charge(1, cancel)?;
            let mut triangles = Vec::new();
            triangles
                .try_reserve_exact(retained)
                .map_err(|_| super::resource("native pole triangle allocation failed"))?;
            for triangle in editor.faces.tri_faces() {
                work.charge(1, cancel)?;
                if !removed(triangle) {
                    triangles.push(*triangle);
                }
            }
            *editor.faces = monstertruck_meshing::prelude::Faces::from_tri_and_quad_faces(
                triangles,
                Vec::new(),
            );
        }
        Ok(())
    }

    fn project(&self, point: [f64; 3], position: usize) -> Result<Corner, GeometryError> {
        let delta = std::array::from_fn(|axis| {
            Interval::point(point[axis]).sub(Interval::point(self.center[axis]))
        });
        let denominator = dot(delta, self.normal);
        if denominator.lo <= 0.0 {
            return Err(unresolved(
                "mesh vertex crosses the certified spherical chart horizon",
            ));
        }
        let bounds = [
            dot(delta, self.x).div(denominator)?,
            dot(delta, self.y).div(denominator)?,
        ];
        Ok(Corner {
            uv: bounds.map(Interval::mid),
            bounds,
            position,
        })
    }

    pub(super) fn certify(
        &self,
        definition: &Definition,
        ordinal: usize,
        polygon: &PolygonMesh,
        extra_retained_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<f64, GeometryError> {
        let reserved = self
            .reserved_bytes
            .saturating_add(extra_retained_bytes)
            .saturating_add(self.trim.retained_bytes());
        coverage::check_memory(reserved)?;
        let circuits = coverage::triangulated_boundary_in_chart(
            polygon,
            work,
            cancel,
            reserved,
            None,
            |_, point, position| self.project(point, position),
        )?;
        let wire_count = definition.face_boundaries[ordinal].wires.len();
        let coedge_count = self.trim.native().coedges().len();
        let correspondence_bytes =
            coverage::correspondence_bytes(&circuits, coedge_count, wire_count);
        coverage::check_memory(reserved.saturating_add(correspondence_bytes))?;
        let endpoint_count = circuits.iter().map(|wire| wire.len() - 1).sum();
        work.charge(1, cancel)?;
        let mut endpoints = Vec::with_capacity(endpoint_count);
        for (wire, circuit) in circuits.iter().enumerate() {
            endpoints.extend((0..circuit.len() - 1).map(|vertex| (wire, vertex)));
        }
        endpoints.sort_unstable_by(|&(a, b), &(c, d)| {
            circuits[a][b].uv[0]
                .total_cmp(&circuits[c][d].uv[0])
                .then_with(|| (a, b).cmp(&(c, d)))
        });
        work.charge(circuits.len() as u64 + 3, cancel)?;
        let mut used: Vec<_> = circuits
            .iter()
            .map(|wire| vec![false; wire.len() - 1])
            .collect();
        let mut loops = HashMap::with_capacity(wire_count);
        let mut vertices = HashMap::with_capacity(coedge_count);
        let mut alignment = 0_i8;
        let mut maximum = 0.0_f64;
        for (wire_index, wire) in definition.face_boundaries[ordinal].wires.iter().enumerate() {
            for (coedge_index, coedge) in wire.iter().enumerate() {
                let native_edge =
                    original_edge(self.trim.native().face(), wire_index, coedge_index, coedge)?;
                native_edge.with_curve(|curve| -> Result<(), GeometryError> {
                    let (a, b) = curve.range_tuple();
                    let remaining = coverage::PROOF_BYTES
                        .saturating_sub(reserved)
                        .saturating_sub(correspondence_bytes);
                    let prepared = definition.prepare_boundary_curve(
                        native_edge.id(),
                        curve,
                        (a, b),
                        remaining,
                        work,
                        cancel,
                    )?;
                    let start = self.curve_jet(&prepared, (a, a), work, cancel)?.position;
                    let end = self.curve_jet(&prepared, (b, b), work, cancel)?.position;
                    let first =
                        coverage::find_endpoint(&circuits, &endpoints, start, work, cancel)?;
                    let last = coverage::find_endpoint(&circuits, &endpoints, end, work, cancel)?;
                    if first.0 != last.0 {
                        return Err(invalid("spherical native coedge spans distinct mesh loops"));
                    }
                    for (id, binding) in [
                        (native_edge.absolute_front().id(), first),
                        (native_edge.absolute_back().id(), last),
                    ] {
                        if vertices
                            .insert(id, binding)
                            .is_some_and(|old| old != binding)
                        {
                            return Err(invalid(
                                "spherical mesh fails original shared native vertex identity",
                            ));
                        }
                    }
                    if loops
                        .insert(wire_index, first.0)
                        .is_some_and(|old| old != first.0)
                        || loops
                            .iter()
                            .any(|(&other, &circuit)| other != wire_index && circuit == first.0)
                    {
                        return Err(invalid(
                            "spherical native and mesh trim loops are not bijective",
                        ));
                    }
                    let circuit = &circuits[first.0];
                    let count = circuit.len() - 1;
                    let mut current = first.1;
                    let mut continuation: Option<(i8, f64)> = None;
                    for (direction, next) in [
                        (1_i8, (current + 1) % count),
                        (-1_i8, (current + count - 1) % count),
                    ] {
                        let parameter = if next == last.1 {
                            Some(b)
                        } else {
                            work.charge(100, cancel)?;
                            curve.search_parameter(
                                Point3::from(circuit[next].point(polygon)),
                                (a, b),
                                100,
                            )
                        };
                        if let Some(parameter) = parameter
                            && parameter > a
                            && parameter <= b
                            && parameter.is_finite()
                            && continuation.is_none_or(|old| parameter < old.1)
                        {
                            continuation = Some((direction, parameter));
                        }
                    }
                    let direction = continuation
                        .ok_or_else(|| {
                            unresolved("spherical native coedge has no ordered mesh continuation")
                        })?
                        .0;
                    let winding = direction * if native_edge.orientation() { 1 } else { -1 };
                    if alignment == 0 {
                        alignment = winding;
                    }
                    if alignment != winding {
                        return Err(invalid("spherical trim material winding is inconsistent"));
                    }
                    let mut parameter = a;
                    let mut traversed = 0;
                    loop {
                        let next = if direction > 0 {
                            (current + 1) % count
                        } else {
                            (current + count - 1) % count
                        };
                        let next_parameter = if next == last.1 {
                            b
                        } else {
                            work.charge(100, cancel)?;
                            curve
                                .search_parameter(
                                    Point3::from(circuit[next].point(polygon)),
                                    (parameter, b),
                                    100,
                                )
                                .ok_or_else(|| {
                                    unresolved(
                                        "spherical mesh boundary has no native edge parameter",
                                    )
                                })?
                        };
                        if !next_parameter.is_finite()
                            || next_parameter <= parameter
                            || next_parameter > b
                        {
                            return Err(unresolved(
                                "spherical native edge correspondence is nonmonotone",
                            ));
                        }
                        let edge = if direction > 0 { current } else { next };
                        if used[first.0][edge] {
                            return Err(invalid(
                                "spherical mesh boundary edge is multiply covered",
                            ));
                        }
                        maximum = maximum.max(self.span_error(
                            &prepared,
                            (parameter, next_parameter),
                            [circuit[current], circuit[next]],
                            polygon,
                            work,
                            cancel,
                        )?);
                        used[first.0][edge] = true;
                        current = next;
                        parameter = next_parameter;
                        traversed += 1;
                        if current == last.1 {
                            break;
                        }
                        if traversed >= count {
                            return Err(invalid(
                                "spherical native edge does not close its mesh boundary",
                            ));
                        }
                    }
                    Ok(())
                })?;
            }
        }
        if loops.len() != circuits.len() || used.iter().flatten().any(|value| !value) {
            return Err(invalid(
                "spherical source coverage has omitted trim loops or boundary segments",
            ));
        }
        let source_error = Interval::point(self.trim.source_chart_error_mm())
            .add(Interval::point(self.pole_error_mm))
            .hi;
        let normal_norm = norm_upper(self.normal.map(Interval::point))?;
        let lower = Interval::point(self.hemisphere_lower_mm)
            .sub(Interval::point(source_error).mul(Interval::point(normal_norm)));
        if lower.lo <= 0.0 {
            return Err(unresolved(
                "source trim tube crosses the spherical chart horizon",
            ));
        }
        let radial_upper = Interval::point(self.radius)
            .add(Interval::point(self.carrier_error_mm))
            .add(Interval::point(source_error));
        let axis_norm = norm_upper([
            Interval::point(norm_upper(self.x.map(Interval::point))?),
            Interval::point(norm_upper(self.y.map(Interval::point))?),
        ])?;
        let projection_lipschitz = Interval::point(axis_norm).mul(
            Interval::point(1.0).div(lower)?.add(
                radial_upper
                    .mul(Interval::point(normal_norm))
                    .div(lower.square())?,
            ),
        );
        let source_chart_error = projection_lipschitz.mul(Interval::point(source_error));
        Ok(Interval::point(maximum)
            .add(source_chart_error)
            .mul(Interval::point(self.inverse_lipschitz_mm))
            .add(Interval::point(self.carrier_error_mm))
            .hi)
    }

    fn curve_jet(
        &self,
        curve: &PreparedCurveEnclosure<'_>,
        range: (f64, f64),
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<parametric::ChartCurveEnclosure, GeometryError> {
        curve.gnomonic_jet(
            range,
            &crate::parametric::GnomonicFrame {
                center_mm: self.center,
                x_axis: self.x,
                y_axis: self.y,
                normal: self.normal,
            },
            work,
            cancel,
        )
    }

    fn span_error(
        &self,
        curve: &PreparedCurveEnclosure<'_>,
        range: (f64, f64),
        corners: [Corner; 2],
        polygon: &PolygonMesh,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<f64, GeometryError> {
        let chord = corners.map(|corner| corner.chart_bounds());
        let direction: [Interval; 2] =
            std::array::from_fn(|axis| chord[1][axis].sub(chord[0][axis]));
        let whole_smooth = curve.is_smooth_on(range, work, cancel)?;
        let perpendicular = if whole_smooth {
            let radial = corners.map(|corner| {
                std::array::from_fn(|axis| {
                    Interval::point(corner.point(polygon)[axis])
                        .sub(Interval::point(self.center[axis]))
                })
            });
            let world_normal = cross(radial[0], radial[1]);
            let proposed_normal = world_normal.map(Interval::mid);
            let normal_error: [Interval; 3] = std::array::from_fn(|axis| {
                world_normal[axis].sub(Interval::point(proposed_normal[axis]))
            });
            let determinant = dot(
                cross(
                    self.y.map(Interval::point),
                    self.normal.map(Interval::point),
                ),
                self.x,
            );
            let dual_x = cross(
                self.y.map(Interval::point),
                self.normal.map(Interval::point),
            );
            let dual_y = cross(
                self.normal.map(Interval::point),
                self.x.map(Interval::point),
            );
            let line_coefficient = |dual: [Interval; 3]| -> Result<Interval, GeometryError> {
                let numerator = world_normal
                    .into_iter()
                    .zip(dual)
                    .fold(Interval::point(0.0), |sum, (a, b)| sum.add(a.mul(b)));
                numerator.div(determinant)
            };
            let coefficients = [line_coefficient(dual_x)?, line_coefficient(dual_y)?];
            let squared = coefficients
                .into_iter()
                .fold(Interval::point(0.0), |sum, value| sum.add(value.square()));
            let line_norm = Interval {
                lo: squared.lo.max(0.0),
                hi: squared.hi,
            }
            .sqrt()?;
            if line_norm.lo <= 0.0 {
                return Err(unresolved(
                    "spherical chart chord has unresolved projective direction",
                ));
            }
            Some((proposed_normal, normal_error, line_norm))
        } else {
            None
        };
        let mut parameter_error = 0.0_f64;
        let mut perpendicular_error = 0.0_f64;
        let mut monotone = whole_smooth;
        let mut covered = range.0;
        for (a, b) in curve.parameter_intervals() {
            let left = a.max(range.0);
            let right = b.min(range.1);
            if left >= right {
                continue;
            }
            if left > covered {
                return Err(unresolved(
                    "prepared native edge has a parameter coverage gap",
                ));
            }
            covered = covered.max(right);
            let jet = self.curve_jet(curve, (left, right), work, cancel)?;
            if let Some((proposed_normal, normal_error, line_norm)) = &perpendicular
                && monotone
            {
                let progress = (0..2).fold(Interval::point(0.0), |sum, axis| {
                    sum.add(jet.derivative[axis].mul(direction[axis]))
                });
                monotone = progress.lo > 0.0;
                if monotone {
                    let spatial = curve.jet((left, right), work, cancel)?;
                    let displacement: [Interval; 3] = std::array::from_fn(|axis| {
                        spatial.position[axis].sub(Interval::point(self.center[axis]))
                    });
                    let denominator = dot(displacement, self.normal);
                    let projected = curve.projected_position_enclosure(
                        (left, right),
                        self.center,
                        *proposed_normal,
                        work,
                        cancel,
                    )?;
                    let defect = normal_error
                        .iter()
                        .zip(displacement)
                        .fold(projected, |sum, (error, value)| sum.add(error.mul(value)));
                    if denominator.lo <= 0.0 {
                        return Err(unresolved(
                            "native boundary interval crosses spherical chart horizon",
                        ));
                    }
                    perpendicular_error = perpendicular_error.max(
                        Interval::point(defect.abs_upper())
                            .div(denominator.mul(*line_norm))?
                            .hi,
                    );
                }
            }
            let fraction = |t| {
                Interval::point(t)
                    .sub(Interval::point(range.0))
                    .div(Interval::point(range.1).sub(Interval::point(range.0)))
            };
            let interpolate = |fraction: Interval| {
                std::array::from_fn(|axis| {
                    chord[0][axis].add(chord[1][axis].sub(chord[0][axis]).mul(fraction))
                })
            };
            let endpoints: [[Interval; 2]; 2] =
                [interpolate(fraction(left)?), interpolate(fraction(right)?)];
            let error: [Interval; 2] =
                if whole_smooth || curve.is_smooth_on((left, right), work, cancel)? {
                    let start = self.curve_jet(curve, (left, left), work, cancel)?.position;
                    let end = self
                        .curve_jet(curve, (right, right), work, cancel)?
                        .position;
                    let duration = Interval::point(right).sub(Interval::point(left));
                    std::array::from_fn(|axis| {
                        let endpoint = start[axis]
                            .sub(endpoints[0][axis])
                            .abs_upper()
                            .max(end[axis].sub(endpoints[1][axis]).abs_upper());
                        Interval::point(endpoint).add(
                            Interval::point(jet.second_derivative[axis].abs_upper())
                                .mul(duration.square())
                                .mul(Interval::point(0.125)),
                        )
                    })
                } else {
                    std::array::from_fn(|axis| {
                        let linear = Interval {
                            lo: endpoints[0][axis].lo.min(endpoints[1][axis].lo),
                            hi: endpoints[0][axis].hi.max(endpoints[1][axis].hi),
                        };
                        // Complete-image residual, independent of any C1 premise.
                        jet.position[axis].sub(linear)
                    })
                };
            parameter_error = parameter_error.max(norm_upper(error)?);
        }
        if covered < range.1 {
            return Err(unresolved(
                "prepared native edge does not cover complete chord interval",
            ));
        }
        // A complete native C1 qualification supplies continuity; strict progress
        // then proves onto correspondence to the finite chord, not just its line.
        if monotone {
            let start = self
                .curve_jet(curve, (range.0, range.0), work, cancel)?
                .position;
            let end = self
                .curve_jet(curve, (range.1, range.1), work, cancel)?
                .position;
            let endpoint_error = norm_upper(std::array::from_fn::<_, 2, _>(|axis| {
                start[axis].sub(chord[0][axis])
            }))?
            .max(norm_upper(std::array::from_fn::<_, 2, _>(|axis| {
                end[axis].sub(chord[1][axis])
            }))?);
            Ok(parameter_error.min(
                Interval::point(perpendicular_error)
                    .add(Interval::point(endpoint_error))
                    .hi,
            ))
        } else {
            Ok(parameter_error)
        }
    }

    pub(super) fn triangle_error(&self, points: [[f64; 3]; 3]) -> Result<f64, GeometryError> {
        // For every chart point in a projected triangle, its ray intersects the
        // actual planar triangle. Supporting-plane distance is a conservative
        // lower bound on every point's radius, not a sampled closest point.
        let points: [[Interval; 3]; 3] = points.map(|point| {
            std::array::from_fn(|axis| {
                Interval::point(point[axis]).sub(Interval::point(self.center[axis]))
            })
        });
        let e0 = std::array::from_fn(|axis| points[1][axis].sub(points[0][axis]));
        let e1 = std::array::from_fn(|axis| points[2][axis].sub(points[0][axis]));
        let normal = cross(e0, e1);
        let normal_norm = norm_upper(normal)?;
        if normal_norm == 0.0 {
            return Err(unresolved(
                "spherical physical triangle has unresolved area",
            ));
        }
        let plane = normal
            .into_iter()
            .zip(points[0])
            .fold(Interval::point(0.0), |sum, (a, b)| sum.add(a.mul(b)));
        let absolute = if plane.lo > 0.0 {
            plane.lo
        } else if plane.hi < 0.0 {
            -plane.hi
        } else {
            0.0
        };
        let lower = parametric::down(absolute / normal_norm).max(0.0);
        let mut upper = 0.0_f64;
        for point in points {
            upper = upper.max(norm_upper(point)?);
        }
        Ok(parametric::up(
            (self.radius - lower).max(upper - self.radius).max(0.0) + self.carrier_error_mm,
        ))
    }
}

fn native_sphere_error(
    surface: &Surface,
    center: [f64; 3],
    radius: f64,
) -> Result<f64, GeometryError> {
    let Surface::SphericalSurface(processor) = surface else {
        return Err(invalid(
            "spherical certificate requires the actual native spherical carrier",
        ));
    };
    let matrix = processor.transform();
    if [matrix.x.w, matrix.y.w, matrix.z.w] != [0.0; 3] || matrix.w.w != 1.0 {
        return Err(unresolved(
            "projective native sphere needs a different radial chart proof",
        ));
    }
    let columns = [
        [matrix.x.x, matrix.x.y, matrix.x.z],
        [matrix.y.x, matrix.y.y, matrix.y.z],
        [matrix.z.x, matrix.z.y, matrix.z.z],
    ];
    let local = processor.entity().center();
    let local = [local.x, local.y, local.z];
    let actual_center: [Interval; 3] = std::array::from_fn(|axis| {
        (0..3)
            .fold(
                Interval::point([matrix.w.x, matrix.w.y, matrix.w.z][axis]),
                |sum, j| sum.add(Interval::point(columns[j][axis]).mul(Interval::point(local[j]))),
            )
            .sub(Interval::point(center[axis]))
    });
    let gram: [[Interval; 3]; 3] = std::array::from_fn(|i| {
        std::array::from_fn(|j| dot(columns[i].map(Interval::point), columns[j]))
    });
    let mut lower = f64::INFINITY;
    let mut upper = 0.0_f64;
    for (i, row) in gram.iter().enumerate() {
        let off = row
            .iter()
            .enumerate()
            .filter(|&(j, _)| i != j)
            .fold(Interval::point(0.0), |sum, (_, value)| {
                sum.add(Interval::point(value.abs_upper()))
            });
        lower = lower.min(row[i].sub(off).lo);
        upper = upper.max(row[i].add(off).hi);
    }
    if lower <= 0.0 {
        return Err(unresolved(
            "native sphere affine pose cannot prove positive definite metric",
        ));
    }
    let native_radius = Interval {
        lo: lower,
        hi: upper,
    }
    .sqrt()?
    .mul(Interval::point(processor.entity().radius()));
    if native_radius.lo <= norm_upper(actual_center)? || radius <= 0.0 {
        return Err(unresolved(
            "spherical radial origin cannot be proved inside the actual native carrier",
        ));
    }
    Ok(parametric::up(
        native_radius.sub(Interval::point(radius)).abs_upper() + norm_upper(actual_center)?,
    ))
}
fn inverse_lipschitz(
    x: [f64; 3],
    y: [f64; 3],
    n: [f64; 3],
    radius: f64,
) -> Result<f64, GeometryError> {
    let first = cross(y.map(Interval::point), n.map(Interval::point));
    let second = cross(n.map(Interval::point), x.map(Interval::point));
    let determinant = dot(first, x);
    if determinant.contains_zero() {
        return Err(unresolved("spherical chart frame is not invertible"));
    }
    let mut squared = Interval::point(0.0);
    for value in first.into_iter().chain(second) {
        squared = squared.add(value.div(determinant)?.square());
    }
    let dual = Interval {
        lo: squared.lo.max(0.0),
        hi: squared.hi,
    }
    .sqrt()?;
    Ok(dual
        .mul(Interval::point(norm_upper(n.map(Interval::point))?))
        .mul(Interval::point(radius))
        .hi)
}
fn dot(values: [Interval; 3], direction: [f64; 3]) -> Interval {
    values
        .into_iter()
        .zip(direction)
        .fold(Interval::point(0.0), |sum, (value, component)| {
            sum.add(value.mul(Interval::point(component)))
        })
}
fn cross(a: [Interval; 3], b: [Interval; 3]) -> [Interval; 3] {
    [
        a[1].mul(b[2]).sub(a[2].mul(b[1])),
        a[2].mul(b[0]).sub(a[0].mul(b[2])),
        a[0].mul(b[1]).sub(a[1].mul(b[0])),
    ]
}
fn norm_upper<const N: usize>(values: [Interval; N]) -> Result<f64, GeometryError> {
    let squared = values
        .into_iter()
        .fold(Interval::point(0.0), |sum, value| sum.add(value.square()));
    Ok(Interval {
        lo: squared.lo.max(0.0),
        hi: squared.hi,
    }
    .sqrt()?
    .hi)
}
fn unresolved(message: &str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::AnalysisIndeterminate, message)
}
