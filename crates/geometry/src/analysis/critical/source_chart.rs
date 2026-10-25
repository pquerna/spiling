// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! A carrier identity, a regular quotient and source-joint authority are three
//! different obligations. Original STEP providers retain radians and literal
//! placement expressions; native explicit STEP wrappers retain degrees. Neither
//! may borrow the other's UV gauge. Bare native spheres select rational NURBS:
//! their collapsed boundaries require actual homogeneous coefficient proofs,
//! never an analytic family label or an inferred implicit quadric.

use super::*;
use solidcore::surface::surface::{Surface, p2::SurfaceKind};
use solidcore::topology::{EdgeuseId, VertexId};

use crate::kernel::trim::SphericalPole;
use crate::parametric::SurfaceEvaluator;
use crate::kernel::trim::{OriginalCoedgeCorrespondence, PreparedTrimJoint, PreparedTrimSurface, TrimJointKind};

#[derive(Clone, Copy)]
struct SphereGauge {
    scale: Interval,
    north: Interval,
    period_lower: f64,
}
impl SphereGauge {
    fn native_step() -> Option<Self> {
        Some(Self { scale: degree().ok()?, north: Interval::point(90.0), period_lower: 360.0 })
    }
    fn original() -> Self {
        Self { scale: Interval::point(1.0),
            north: Interval { lo: down(std::f64::consts::FRAC_PI_2), hi: up(std::f64::consts::FRAC_PI_2) },
            period_lower: down(std::f64::consts::TAU) }
    }
    fn pole(self, pole: SphericalPole) -> Interval {
        if pole == SphericalPole::North { self.north } else { self.north.neg() }
    }
    fn single_sheet(self, range: [f64; 2]) -> Result<bool, GeometryError> {
        Ok(Interval::new(range[0], range[1])?.width() < self.period_lower)
    }
}

fn selected_provider(surface: &(impl SurfaceEvaluator + ?Sized), domain: &PreparedFaceTrimDomain<'_>) -> bool {
    match (surface.native_surface(), surface.original_surface(), domain.surface_operand()) {
        (Some(candidate), None, PreparedTrimSurface::Native(selected)) => std::ptr::eq(candidate, selected),
        (None, Some(candidate), PreparedTrimSurface::Original(selected)) => std::ptr::eq(candidate, selected.source()),
        _ => false,
    }
}

fn selected_bounds(domain: &PreparedFaceTrimDomain<'_>) -> [[f64; 2]; 2] {
    match domain.surface_operand() {
        PreparedTrimSurface::Native(_) => domain.uv_domain(),
        PreparedTrimSurface::Original(original) => original.active_domain(),
    }
}

/// Literal affine image x = center + A q of the unit sphere. The columns
/// include the positive literal radius. No orthonormal-placement assumption is
/// required: cof(A) and det(A) retain their full outward enclosures.
pub(super) struct SphereChart {
    center: [Interval; 3],
    columns: [[Interval; 3]; 3],
    cofactors: [[Interval; 3]; 3],
    determinant: Interval,
    reversed: bool,
    outward: bool,
    gauge: SphereGauge,
}

fn unavailable(message: &'static str) -> GeometryError {
    GeometryError::new(GeometryErrorCode::AnalysisIndeterminate, message)
}

/// Unwrapping CurveBounded preserves the selected law. Unwrapping a bare
/// Sphere and treating natural UV as angles would change it, so is forbidden.
fn explicit_step_sphere(surface: &Surface) -> Option<&solidcore::surface::sphere::Sphere> {
    let mut source = surface;
    let mut explicit = false;
    for _ in 0..32 {
        match source.kind() {
            SurfaceKind::CurveBounded(bound) => source = bound.basis(),
            SurfaceKind::Step(step) => {
                source = step.surface()?;
                explicit = true;
            }
            SurfaceKind::Sphere(sphere) if explicit => return Some(sphere),
            _ => return None,
        }
    }
    None
}

impl SphereChart {
    fn coefficients(surface: &(impl SurfaceEvaluator + ?Sized), orientation: bool) -> Option<Self> {
        let (center, axes, radius, gauge) = match (surface.native_surface(), surface.original_surface()) {
            (Some(native), None) => {
                let sphere = explicit_step_sphere(native)?;
                let placement = sphere.base().position();
                let p = placement.origin(); let x = placement.x_axis();
                let y = placement.y_axis(); let z = placement.z_axis();
                let center = [p.x, p.y, p.z];
                let axes = [[x.x, x.y, x.z], [y.x, y.y, y.z], [z.x, z.y, z.z]];
                if center.into_iter().chain(axes.into_iter().flatten()).any(|v| !v.is_finite()) { return None; }
                (center.map(Interval::point), axes.map(|axis| axis.map(Interval::point)),
                    sphere.radius(), SphereGauge::native_step()?)
            }
            (None, Some(mut original)) => {
                for _ in 0..32 {
                    match original {
                        solidcraft_step::geom::Surface::RectangularTrimmed { basis, .. } => original = basis,
                        _ => break,
                    }
                }
                let solidcraft_step::geom::Surface::Sphere { position, radius } = original else { return None; };
                if position.origin.into_iter().chain(position.axis).chain(position.ref_direction).any(|v| !v.is_finite()) { return None; }
                let x = position.ref_direction.map(Interval::point);
                let z = position.axis.map(Interval::point);
                // The original operator defines Y by this cross product. Never
                // round Placement::y() before bounding its source coefficients.
                (position.origin.map(Interval::point), [x, cross(z, x), z], *radius, SphereGauge::original())
            }
            _ => return None,
        };
        if !radius.is_finite() || radius <= 0.0 { return None; }
        let columns = axes.map(|column| column.map(|v| v.mul(Interval::point(radius))));
        let cofactors = [cross(columns[1], columns[2]), cross(columns[2], columns[0]), cross(columns[0], columns[1])];
        let determinant = dot(columns[0], cofactors[0]);
        if !determinant.lo.is_finite() || !determinant.hi.is_finite() || determinant.contains_zero() { return None; }
        // Explicit STEP uses U longitude, V latitude, independently of the
        // internal NURBS swap_uv/inside_out flags. dU x dV is the outward law.
        let reversed = !orientation;
        Some(Self { center, columns, cofactors, determinant, gauge,
            reversed, outward: (determinant.lo > 0.0) != reversed })
    }

    /// Qualify every collapsed boundary from the complete genuine source joint
    /// inventory. Each whole overrun strip needs exactly one freshly proved
    /// source fan; neither a pole label nor a saved smaller image supplies it.
    pub(super) fn new(
        surface: &(impl SurfaceEvaluator + ?Sized), domain: &PreparedFaceTrimDomain<'_>,
        work: &mut GeometryWorkBudget, cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        work.charge(1, cancel)?;
        if !selected_provider(surface, domain) {
            return Err(unavailable("source sphere query is not bound to its actual selected face operand"));
        }
        let Some(chart) = Self::coefficients(surface, domain.orientation()) else { return Ok(None); };
        let bounds = selected_bounds(domain);
        if !chart.gauge.single_sheet(bounds[0])? { return Ok(None); }
        for (pole, boundary, overrun) in [
            (SphericalPole::South, -chart.gauge.north.lo, bounds[1][0] <= -chart.gauge.north.lo),
            (SphericalPole::North, chart.gauge.north.lo, bounds[1][1] >= chart.gauge.north.lo),
        ] {
            if !overrun { continue; }
            let mut strip = bounds;
            if pole == SphericalPole::South { strip[1][1] = strip[1][1].min(boundary); }
            else { strip[1][0] = strip[1][0].max(boundary); }
            let mut qualified = false;
            for joint in domain.original_joints()? {
                if let Some((declared, _)) = qualify_joint_under_chart(domain, joint, strip, &chart, work, cancel)? {
                    if declared != pole { continue; }
                    if qualified { return Ok(None); }
                    qualified = true;
                }
            }
            if !qualified { return Ok(None); }
        }
        Ok(Some(chart))
    }

    /// Same carrier proof, with the complete original source joint inventory.
    /// Exactly one successfully qualified physical connector must cover each
    /// complete overrun strip; two separate fans do not establish degree one.
    pub(super) fn with_source_joints(
        surface: &(impl SurfaceEvaluator + ?Sized), domain: &PreparedFaceTrimDomain<'_>, joints: &[SourcePoleJoint<'_>],
        work: &mut GeometryWorkBudget, cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        work.charge(1, cancel)?;
        if !selected_provider(surface, domain) {
            return Err(unavailable("source pole inventory is not bound to its actual selected face operand"));
        }
        let Some(chart) = Self::coefficients(surface, domain.orientation()) else { return Ok(None); };
        let bounds = selected_bounds(domain);
        if !chart.gauge.single_sheet(bounds[0])? { return Ok(None); }
        for (pole, boundary, overrun) in [
            (SphericalPole::South, -chart.gauge.north.lo, bounds[1][0] <= -chart.gauge.north.lo),
            (SphericalPole::North, chart.gauge.north.lo, bounds[1][1] >= chart.gauge.north.lo),
        ] {
            if !overrun { continue; }
            let mut strip = bounds;
            if pole == SphericalPole::South { strip[1][1] = strip[1][1].min(boundary); }
            else { strip[1][0] = strip[1][0].max(boundary); }
            let mut qualified = false;
            for joint in joints {
                if pole_joint_strip(surface, domain, joint, pole, strip, &chart, work, cancel)? {
                    if qualified { return Ok(None); }
                    qualified = true;
                }
            }
            if !qualified { return Ok(None); }
        }
        Ok(Some(chart))
    }

    pub(super) fn normal(&self, position: [Interval; 3]) -> Result<[Interval; 3], GeometryError> {
        let relative = std::array::from_fn(|i| position[i].sub(self.center[i]));
        let mut local = [Interval::point(0.0); 3];
        for (i, value) in local.iter_mut().enumerate() { *value = dot(self.cofactors[i], relative).div(self.determinant)?; }
        let n: [Interval; 3] = std::array::from_fn(|row| (0..3).fold(Interval::point(0.0), |s, column|
            s.add(self.cofactors[column][row].mul(local[column]))));
        Ok(if self.reversed { n.map(Interval::neg) } else { n })
    }

    pub(super) fn convex(&self) -> bool { self.outward }
}

/// Actual hemisphere graph of an explicitly selected STEP sphere law. Natural
/// rational charts never use this trigonometric parameter correspondence.
#[derive(Clone, Copy)]
pub(super) struct PoleChart {
    center: [Interval; 3],
    columns: [[Interval; 3]; 3],
    inverse: [[Interval; 3]; 3],
    sign: f64,
    gauge: SphereGauge,
}

impl PoleChart {
    pub(super) fn coefficients(surface: &(impl SurfaceEvaluator + ?Sized), pole: SphericalPole) -> Result<Option<Self>, GeometryError> {
        let Some(chart) = SphereChart::coefficients(surface, true) else { return Ok(None); };
        let mut inverse = [[Interval::point(0.0); 3]; 3];
        for (axis, row) in inverse.iter_mut().enumerate() {
            for (column, value) in row.iter_mut().enumerate() { *value = chart.cofactors[axis][column].div(chart.determinant)?; }
        }
        Ok(Some(Self { center: chart.center, columns: chart.columns, inverse, gauge: chart.gauge,
            sign: if pole == SphericalPole::North { 1.0 } else { -1.0 } }))
    }
    pub(super) fn polar_axis(&self) -> usize { 1 }
    pub(super) fn angular_axis(&self) -> usize { 0 }
    /// A proposal only; proofs retain the complete pole-coordinate enclosure.
    pub(super) fn pole_coordinate(&self) -> f64 { self.pole_coordinate_enclosure().mid() }
    pub(super) fn pole_coordinate_enclosure(&self) -> Interval {
        if self.sign > 0.0 { self.gauge.north } else { self.gauge.north.neg() }
    }
    pub(super) fn principal_polar(&self, latitude: Interval) -> bool {
        latitude.lo > -self.gauge.north.lo && latitude.hi < self.gauge.north.lo
    }
    pub(super) fn project(&self, position: [Interval; 3]) -> [Interval; 3] {
        let relative = std::array::from_fn(|i| position[i].sub(self.center[i]));
        self.inverse.map(|row| dot(row, relative))
    }
    pub(super) fn project_vector(&self, vector: [Interval; 3]) -> [Interval; 3] { self.inverse.map(|row| dot(row, vector)) }
    pub(super) fn fiber_parameters(&self, target: [Interval; 2]) -> ([f64; 3], [[Interval; 3]; 2], [Interval; 2], [Interval; 3], f64) {
        let origin = self.center.map(Interval::mid);
        let residual = std::array::from_fn(|i| Interval::point(origin[i]).sub(self.center[i]));
        let rows = [self.inverse[0], self.inverse[1]];
        let offsets = std::array::from_fn(|i| dot(rows[i], residual).sub(target[i]));
        let forward = self.inverse[2].map(|v| v.mul(Interval::point(self.sign)));
        let min_forward = dot(forward, residual).neg().hi;
        (origin, rows, offsets, forward, min_forward)
    }
    pub(super) fn same_hemisphere(&self, p: [Interval; 3]) -> bool { self.project(p)[2].mul(Interval::point(self.sign)).lo > 0.0 }
    pub(super) fn opposite_hemisphere(&self, p: [Interval; 3]) -> bool { self.project(p)[2].mul(Interval::point(self.sign)).hi <= 0.0 }
    fn vector(&self, local: [Interval; 3]) -> [Interval; 3] {
        std::array::from_fn(|row| (0..3).fold(Interval::point(0.0), |sum, column|
            sum.add(self.columns[column][row].mul(local[column]))))
    }

    /// U is longitude and V latitude in the selected provider's actual gauge.
    /// Chain derivatives retain its scale, including second-order factors.
    pub(super) fn covering_jet(
        &self, uv: [Interval; 2], first: [Interval; 2], second: [Interval; 2],
        work: &mut GeometryWorkBudget, cancel: &AtomicBool,
    ) -> Result<([Interval; 2], [Interval; 2], [Interval; 2]), GeometryError> {
        work.charge(1, cancel)?;
        let scale = self.gauge.scale;
        let (sa, ca) = crate::parametric::sin_cos_interval(uv[0].mul(scale));
        let (sp, cp) = crate::parametric::sin_cos_interval(uv[1].mul(scale));
        let q = [cp.mul(ca), cp.mul(sa)];
        let qa = [cp.mul(sa).neg(), cp.mul(ca)];
        let qp = [sp.mul(ca).neg(), sp.mul(sa).neg()];
        let qpa = [sp.mul(sa), sp.mul(ca).neg()];
        let a = first[0].mul(scale); let p = first[1].mul(scale);
        let aa = second[0].mul(scale); let pp = second[1].mul(scale);
        let derivative = std::array::from_fn(|i| qa[i].mul(a).add(qp[i].mul(p)));
        let acceleration = std::array::from_fn(|i| q[i].neg().mul(a.square().add(p.square()))
            .add(Interval::point(2.0).mul(qpa[i]).mul(p).mul(a)).add(qa[i].mul(aa)).add(qp[i].mul(pp)));
        Ok((q, derivative, acceleration))
    }

    pub(super) fn graph_jet(&self, q: [Interval; 2], direction: [Interval; 2])
        -> Result<([Interval; 3], [Interval; 3], [Interval; 3]), GeometryError> {
        let radicand = Interval::point(1.0).sub(q[0].square()).sub(q[1].square());
        if radicand.lo <= 0.0 { return Err(unavailable("original spherical pole comparison leaves its open hemisphere")); }
        let z = radicand.sqrt()?.mul(Interval::point(self.sign));
        let slope = q[0].mul(direction[0]).add(q[1].mul(direction[1]));
        let speed2 = direction[0].square().add(direction[1].square());
        let dz = slope.neg().div(z)?;
        let ddz = speed2.neg().div(z)?.sub(slope.square().div(z.mul(z).mul(z))?);
        let position = self.vector([q[0], q[1], z]);
        Ok((std::array::from_fn(|i| self.center[i].add(position[i])),
            self.vector([direction[0], direction[1], dz]),
            self.vector([Interval::point(0.0), Interval::point(0.0), ddz])))
    }
}

fn degree() -> Result<Interval, GeometryError> {
    Interval { lo: down(std::f64::consts::PI), hi: up(std::f64::consts::PI) }.div(Interval::point(180.0))
}
fn contains(outer: [[f64; 2]; 2], inner: [[f64; 2]; 2]) -> bool {
    (0..2).all(|axis| outer[axis][0] <= inner[axis][0] && inner[axis][1] <= outer[axis][1])
}

/// These are ORIGINAL source operands, not an inferred native endpoint joint.
/// The owner must supply the complete immutable source inventory. An ordinary
/// uncertain endpoint can acquire fresh pole authority only through the whole
/// physical/UV proof below; its retained original classification is unchanged.
pub(in crate::analysis) struct SourcePoleJoint<'a> {
    pub(in crate::analysis) incoming: EdgeuseId,
    pub(in crate::analysis) outgoing: EdgeuseId,
    pub(in crate::analysis) vertex: VertexId,
    /// Fixed realized common vertex V. Original declared vertex coordinates
    /// may differ; their complete residual to V remains in the world receipt.
    pub(in crate::analysis) vertex_point: [f64; 3],
    pub(in crate::analysis) enclosure: [[f64; 2]; 2],
    pub(in crate::analysis) uncertainty_uv: [f64; 2],
    pub(in crate::analysis) physical_limit_mm: f64,
    /// Retained original classification, not fresh pole authority.
    pub(in crate::analysis) declared_pole: Option<SphericalPole>,
    /// Complete ORIGINAL endpoint tubes in the selected provider's gauge,
    /// including canonical expressions and original endpoint-jet rounding.
    /// Native degree/NURBS endpoint boxes are not original-radian evidence.
    pub(in crate::analysis) endpoint_uv: &'a [[[f64; 2]; 2]; 2],
    /// Required for the Original provider: sealed selected-source trace
    /// receipts, in incoming/outgoing order. Native chart orientation is not
    /// original spatial/material endpoint orientation.
    pub(in crate::analysis) endpoint_correspondence: Option<[&'a OriginalCoedgeCorrespondence; 2]>,
}

/// Fresh complete physical/UV qualification of one actual prepared source
/// joint. The requested scope, both actual connectors and stored endpoint
/// operands all remain in the proof. The source recipe's admitted ceiling is
/// not itself evidence, and a measured residual is never used as a budget.
pub(super) fn qualify_original_pole_joint(
    surface: &(impl SurfaceEvaluator + ?Sized), domain: &PreparedFaceTrimDomain<'_>,
    joint: &PreparedTrimJoint, requested: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget, cancel: &AtomicBool,
) -> Result<Option<[Interval; 3]>, GeometryError> {
    work.charge(1, cancel)?;
    if !selected_provider(surface, domain) {
        return Err(unavailable("source joint qualification selected a different face operand"));
    }
    let mut admitted = false;
    for original in domain.original_joints()? {
        work.charge(1, cancel)?;
        if std::ptr::eq(original, joint) { admitted = true; break; }
    }
    if !admitted {
        return Err(unavailable("pole joint does not belong to the complete prepared source inventory"));
    }
    let Some(chart) = SphereChart::coefficients(surface, domain.orientation()) else { return Ok(None); };
    Ok(qualify_joint_under_chart(domain, joint, requested, &chart, work, cancel)?.map(|(_,world)| world))
}

fn qualify_joint_under_chart(
    domain: &PreparedFaceTrimDomain<'_>, joint: &PreparedTrimJoint,
    requested: [[f64; 2]; 2], chart: &SphereChart,
    work: &mut GeometryWorkBudget, cancel: &AtomicBool,
) -> Result<Option<(SphericalPole, [Interval; 3])>, GeometryError> {
    work.charge(1, cancel)?;
    let maximum = match domain.definition().provenance().uncertainty_mm {
        Some(value) if value.is_finite() && value >= 0.0 => value,
        Some(_) => return Err(unavailable("source realization has an invalid admitted physical ceiling")),
        None => return Ok(None),
    };
    for range in requested { Interval::new(range[0],range[1])?; }
    // PreparedTrimJoint has private construction state and is borrowed from
    // the admitted original inventory. Its incoming/outgoing source endpoint
    // tubes are already material-oriented, not generated native UV samples.
    for use_ in [joint.incoming,joint.outgoing] {
        let mut found = false;
        for coedge in domain.coedges() {
            work.charge(1,cancel)?;
            if coedge.edgeuse != use_.edgeuse { continue; }
            if coedge.identity() != use_ || !coedge.vertices.contains(&joint.vertex_id) {
                return Err(unavailable("prepared source pole joint lost its actual native incidence"));
            }
            found = true;
            break;
        }
        if !found { return Err(unavailable("prepared source pole joint lost an actual coedge use")); }
    }
    let declared = match joint.kind {
        TrimJointKind::EndpointUncertainty => None,
        TrimJointKind::SphericalPole { polar_axis, pole, .. } => {
            if polar_axis != 1 { return Ok(None); }
            Some(pole)
        }
    };
    let enclosure = joint.enclosure.map(|value| [value.lo,value.hi]);
    let actual_connector = crate::parametric::curve2d_piecewise_jet(
        &joint.connector,[0.0,1.0],work,cancel)?.position;
    let mut receipt = None;
    for pole in [SphericalPole::North,SphericalPole::South] {
        if declared.is_some_and(|original| original != pole) { continue; }
        // The selected endpoint expression tubes retain all arithmetic/root
        // uncertainty. Zero here means NO additional arbitrary UV expansion,
        // not a zero-uncertainty or zero-source-error certificate.
        let Some(mut scope) = supported_connector(enclosure,joint.endpoint_uv,1,
            chart.gauge.pole(pole),requested,[0.0;2])? else { continue; };
        for axis in 0..2 {
            scope[axis][0] = scope[axis][0].min(actual_connector[axis].lo);
            scope[axis][1] = scope[axis][1].max(actual_connector[axis].hi);
        }
        let mut saved_world = None;
        let mut saved_residual = None;
        if let TrimJointKind::SphericalPole { connector, polar_coordinate, world_enclosure, vertex_residual, .. } = joint.kind {
            let image = crate::parametric::curve2d_piecewise_jet(&connector,[0.0,1.0],work,cancel)?.position;
            for axis in 0..2 {
                scope[axis][0] = scope[axis][0].min(image[axis].lo);
                scope[axis][1] = scope[axis][1].max(image[axis].hi);
            }
            Interval::new(polar_coordinate.lo,polar_coordinate.hi)?;
            scope[1][0] = scope[1][0].min(polar_coordinate.lo);
            scope[1][1] = scope[1][1].max(polar_coordinate.hi);
            saved_world = Some(world_enclosure);
            saved_residual = Some(vertex_residual);
        }
        if !chart.gauge.single_sheet(scope[0])? { continue; }
        let mut world = sphere_world(chart,scope)?;
        for axis in 0..3 {
            world[axis] = world[axis].hull(Interval::new(joint.vertex_point[axis],joint.vertex_point[axis])?);
            if let Some(saved) = saved_world {
                world[axis] = world[axis].hull(Interval::new(saved[axis].lo,saved[axis].hi)?);
            }
        }
        let residual = std::array::from_fn(|axis| world[axis].sub(Interval::point(joint.vertex_point[axis])));
        if length(residual)?.hi > maximum { continue; }
        // Preserve the actual saved displacement vector as its own physical
        // obligation; do not guess its origin or turn it into a new point.
        if let Some(saved) = saved_residual {
            for value in saved { Interval::new(value.lo,value.hi)?; }
            if length(saved)?.hi > maximum { continue; }
        }
        if length(chart.normal(world)?)?.lo <= 0.0 { continue; }
        if receipt.is_some() { return Ok(None); }
        receipt = Some((pole,world));
    }
    Ok(receipt)
}

fn supported_connector(
    enclosure: [[f64;2];2], endpoints: [[[f64;2];2];2], polar: usize,
    boundary: Interval, requested: [[f64;2];2], additional_margin: [f64;2],
) -> Result<Option<[[f64;2];2]>,GeometryError> {
    let mut supported = enclosure;
    for axis in 0..2 {
        Interval::new(enclosure[axis][0],enclosure[axis][1])?;
        Interval::new(requested[axis][0],requested[axis][1])?;
        if !additional_margin[axis].is_finite() || additional_margin[axis] < 0.0 { return Ok(None); }
        for endpoint in endpoints {
            Interval::new(endpoint[axis][0],endpoint[axis][1])?;
            supported[axis][0] = supported[axis][0].min(endpoint[axis][0]);
            supported[axis][1] = supported[axis][1].max(endpoint[axis][1]);
        }
    }
    supported[polar][0] = supported[polar][0].min(boundary.lo);
    supported[polar][1] = supported[polar][1].max(boundary.hi);
    for axis in 0..2 {
        let value = Interval::new(supported[axis][0],supported[axis][1])?
            .add(Interval { lo:-additional_margin[axis],hi:additional_margin[axis] });
        supported[axis] = [value.lo,value.hi];
    }
    if !contains(supported,requested) { return Ok(None); }
    let angular = 1-polar;
    if !(endpoints[0][angular][1] < endpoints[1][angular][0]
        || endpoints[1][angular][1] < endpoints[0][angular][0]) { return Ok(None); }
    Ok(Some(std::array::from_fn(|axis| [
        supported[axis][0].min(requested[axis][0]),
        supported[axis][1].max(requested[axis][1]),
    ])))
}

#[allow(clippy::too_many_arguments)]
fn pole_joint_strip(
    surface: &(impl SurfaceEvaluator + ?Sized), domain: &PreparedFaceTrimDomain<'_>, joint: &SourcePoleJoint<'_>,
    pole: SphericalPole, strip: [[f64; 2]; 2], chart: &SphereChart,
    work: &mut GeometryWorkBudget, cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    work.charge(1, cancel)?;
    if !selected_provider(surface, domain) {
        return Err(unavailable("source pole connector selected a different face operand"));
    }
    if joint.declared_pole.is_some_and(|declared| declared != pole) { return Ok(false); }
    let boundary = chart.gauge.pole(pole);
    let Some(connector) = source_connector(domain, joint, 1, boundary, strip, work, cancel)? else {
        return Ok(false);
    };
    if !chart.gauge.single_sheet(connector[0])? { return Ok(false); }
    // A complete rectangular hull is conservative for the entire source
    // connector, including every endpoint tube. No endpoint-only residual or
    // sampled closeness can authorize the pole. Overruns remain actual operands.
    let world = retain_declared_vertices(sphere_world(chart, connector)?, joint)?;
    let residual = std::array::from_fn(|i| world[i].sub(Interval::point(joint.vertex_point[i])));
    if length(residual)?.hi > joint.physical_limit_mm { return Ok(false); }
    Ok(length(chart.normal(world)?)?.lo > 0.0)
}

fn sphere_world(chart: &SphereChart, uv: [[f64; 2]; 2]) -> Result<[Interval; 3], GeometryError> {
    let scale = chart.gauge.scale;
    let (su, cu) = crate::parametric::sin_cos_interval(Interval::new(uv[0][0], uv[0][1])?.mul(scale));
    let (sv, cv) = crate::parametric::sin_cos_interval(Interval::new(uv[1][0], uv[1][1])?.mul(scale));
    let local = [cv.mul(cu), cv.mul(su), sv];
    Ok(std::array::from_fn(|row| (0..3).fold(chart.center[row], |sum, column|
        sum.add(chart.columns[column][row].mul(local[column])))))
}

/// A shared native vertex ID is not equality of declared source coordinates.
/// Keep each actual original P in the receipt before bounding it relative to
/// the fixed realization V; no coefficient equality or tolerance snap is used.
fn retain_declared_vertices(mut world: [Interval; 3], joint: &SourcePoleJoint<'_>)
    -> Result<[Interval; 3], GeometryError> {
    if let Some(receipts) = joint.endpoint_correspondence.as_ref() {
        for (slot, receipt) in receipts.iter().enumerate() {
            let binding = receipt.binding().ok_or_else(||
                unavailable("original pole world receipt lost its sealed endpoint binding"))?;
            let spatial_high = binding.material_forward == (slot == 0);
            let point = binding.source_vertex_points_mm[binding.native_endpoint_index(spatial_high)];
            for axis in 0..3 {
                world[axis] = world[axis].hull(Interval::new(point[axis], point[axis])?);
            }
        }
    }
    Ok(world)
}

/// Requalify an original pole connector after enlarging its root cover. The
/// original endpoint support is gathered before extension, and the entire
/// enlarged connection receives a fresh physical receipt under the SAME
/// original allowance. A retained pole classification may not change poles.
pub(in crate::analysis) fn certify_source_pole_connection(
    surface: &(impl SurfaceEvaluator + ?Sized), domain: &PreparedFaceTrimDomain<'_>, joint: &SourcePoleJoint<'_>,
    root: [[f64; 2]; 2], polar_axis: usize,
    work: &mut GeometryWorkBudget, cancel: &AtomicBool,
) -> Result<Option<([[f64; 2]; 2], [Interval; 3])>, GeometryError> {
    work.charge(1, cancel)?;
    if !selected_provider(surface, domain) {
        return Err(unavailable("expanded source pole connection selected a different face operand"));
    }
    if polar_axis != 1 { return Ok(None); }
    let Some(chart) = SphereChart::coefficients(surface, domain.orientation()) else { return Ok(None); };
    for interval in root { Interval::new(interval[0], interval[1])?; }
    let mut receipt = None;
    for pole in [SphericalPole::North, SphericalPole::South] {
        if joint.declared_pole.is_some_and(|declared| declared != pole) { continue; }
        let boundary = chart.gauge.pole(pole);
        let Some(mut connection) = source_connector(domain, joint, 1, boundary, joint.enclosure, work, cancel)? else {
            continue;
        };
        for axis in 0..2 {
            connection[axis][0] = connection[axis][0].min(root[axis][0]);
            connection[axis][1] = connection[axis][1].max(root[axis][1]);
        }
        if !chart.gauge.single_sheet(connection[0])? { continue; }
        let world = retain_declared_vertices(sphere_world(&chart, connection)?, joint)?;
        let residual = std::array::from_fn(|i| world[i].sub(Interval::point(joint.vertex_point[i])));
        if length(residual)?.hi > joint.physical_limit_mm || length(chart.normal(world)?)?.lo <= 0.0 {
            continue;
        }
        if receipt.is_some() { return Ok(None); }
        receipt = Some((connection, world));
    }
    Ok(receipt)
}

/// Complete source support, before any requested strip extension. Shared by
/// actual STEP and rational natural laws; there is no angular interpretation.
fn source_connector(
    domain: &PreparedFaceTrimDomain<'_>, joint: &SourcePoleJoint<'_>,
    polar: usize, boundary: Interval, strip: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget, cancel: &AtomicBool,
) -> Result<Option<[[f64; 2]; 2]>, GeometryError> {
    if !joint.physical_limit_mm.is_finite() || joint.physical_limit_mm <= 0.0
        || joint.uncertainty_uv.iter().any(|m| !m.is_finite() || *m < 0.0)
        || joint.vertex_point.iter().any(|p| !p.is_finite()) { return Ok(None); }
    let Some(ceiling) = domain.definition().provenance().uncertainty_mm else { return Ok(None); };
    if !ceiling.is_finite() || ceiling < 0.0 || joint.physical_limit_mm > ceiling { return Ok(None); }
    let mut margin = joint.uncertainty_uv;
    if let Some(receipts) = joint.endpoint_correspondence.as_ref() {
        for receipt in receipts {
            let admitted = receipt.margin_uv();
            for axis in 0..2 { margin[axis] = margin[axis].max(admitted[axis]); }
        }
    }
    let mut endpoints = *joint.endpoint_uv;
    for (slot, id) in [joint.incoming, joint.outgoing].into_iter().enumerate() {
        let mut found = None;
        for (index, coedge) in domain.coedges().iter().enumerate() {
            work.charge(1, cancel)?;
            if coedge.edgeuse == id { found = Some((index, coedge)); break; }
        }
        let Some((index, coedge)) = found else { return Ok(None); };
        let endpoint = match domain.surface_operand() {
            PreparedTrimSurface::Native(_) => {
                if slot == 0 { usize::from(coedge.forward) } else { usize::from(!coedge.forward) }
            }
            PreparedTrimSurface::Original(_) => {
                let receipts = joint.endpoint_correspondence.as_ref().ok_or_else(||
                    unavailable("original pole endpoint lacks its sealed source trace receipt"))?;
                let binding = receipts[slot].binding().ok_or_else(||
                    unavailable("original pole endpoint lost its sealed material/source binding"))?;
                if binding.face != domain.face() || binding.edgeuse != id
                    || binding.edge != coedge.edge || binding.endpoints != coedge.vertices
                { return Ok(None); }
                let spatial_high = binding.material_forward == (slot == 0);
                binding.native_endpoint_index(spatial_high)
            }
        };
        if coedge.vertices[endpoint] != joint.vertex { return Ok(None); }
        let retained = match domain.surface_operand() {
            PreparedTrimSurface::Native(_) => {
                let parameter = coedge.uv_range[endpoint];
                Some(domain.boundary_position_enclosure(index, [parameter; 2], work, cancel)?)
            }
            // Original endpoint tubes were evaluated by the original-source
            // inventory. A native endpoint box belongs to another gauge.
            PreparedTrimSurface::Original(_) => None,
        };
        for axis in 0..2 {
            Interval::new(endpoints[slot][axis][0], endpoints[slot][axis][1])?;
            if let Some(retained) = retained {
                endpoints[slot][axis][0] = endpoints[slot][axis][0].min(retained[axis][0]);
                endpoints[slot][axis][1] = endpoints[slot][axis][1].max(retained[axis][1]);
            }
        }
    }
    // Source association precedes extension. Every retained endpoint tube and
    // actual admitted margin participates in the shared whole-scope guard.
    supported_connector(joint.enclosure,endpoints,polar,boundary,strip,margin)
}

/// A coefficient-certified collapsed boundary of the ACTUAL natural NURBS law.
/// This is not a sphere certificate or a source-joint certificate. Positive
/// weights and exact homogeneous proportionality prove every point of the
/// entire boundary is the same physical point, including all original spans.
#[derive(Clone, Copy)]
pub(super) struct NativePoleBoundary {
    pub(super) axis: usize,
    pub(super) coordinate: f64,
    pub(super) world: [Interval; 3],
}

pub(super) fn native_collapsed_boundary(
    surface: &Surface, axis: usize, upper: bool,
    work: &mut GeometryWorkBudget, cancel: &AtomicBool,
) -> Result<Option<NativePoleBoundary>, GeometryError> {
    work.charge(1, cancel)?;
    if axis > 1 { return Ok(None); }
    let Some(n) = crate::parametric::native_surface_nurbs(surface)? else { return Ok(None); };
    let (p, q) = n.degrees();
    for (knots, degree, count) in [(n.u_knots(), p, n.n() + 1), (n.v_knots(), q, n.m() + 1)] {
        if count <= degree || knots.len() != count + degree + 1 { return Ok(None); }
        for pair in knots.windows(2) {
            work.charge(1, cancel)?;
            if !pair[0].is_finite() || !pair[1].is_finite() || pair[0] > pair[1] { return Ok(None); }
        }
        if knots[degree] >= knots[count] { return Ok(None); }
    }
    let (knots, degree, count, other) = if axis == 0 { (n.u_knots(), p, n.n() + 1, n.m() + 1) }
        else { (n.v_knots(), q, n.m() + 1, n.n() + 1) };
    if degree == 0 || count <= degree || knots.len() != count + degree + 1 { return Ok(None); }
    let coordinate = knots[if upper { count } else { degree }];
    let domain = surface.natural_uv_domain();
    let native_boundary = if upper { [domain.max().x, domain.max().y][axis] }
        else { [domain.min().x, domain.min().y][axis] };
    if coordinate != native_boundary { return Ok(None); }
    // Only a genuinely clamped end is represented by this row of controls.
    let end = if upper { &knots[count..] } else { &knots[..=degree] };
    if end.iter().any(|k| *k != coordinate) { return Ok(None); }
    let row = if upper { count - 1 } else { 0 };
    let control = |j| -> Result<[f64; 4], GeometryError> {
        let (u, v) = if axis == 0 { (row, j) } else { (j, row) };
        let h = n.cpt(u, v).map_err(|_| unavailable("native collapsed boundary control is absent"))?;
        if h.z == solidcore::nurbs::math::NOZ
            || (h.w != solidcore::nurbs::math::NOW) != n.is_rational()
        { return Err(unavailable("inconsistent native collapsed-boundary coefficient markers")); }
        Ok([h.x, h.y, h.z, if n.is_rational() { h.w } else { 1.0 }])
    };
    let first = control(0)?;
    if first.iter().any(|v| !v.is_finite()) || first[3] <= 0.0 { return Ok(None); }
    for j in 0..other {
        work.charge(1, cancel)?;
        let h = control(j)?;
        if h.iter().any(|v| !v.is_finite()) || h[3] <= 0.0 { return Ok(None); }
        for direction in [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]] {
            if !exact_homogeneous_height_identity(first, h, direction, work, cancel)? { return Ok(None); }
        }
    }
    let mut world = [Interval::point(0.0); 3];
    for i in 0..3 { world[i] = Interval::point(first[i]).div(Interval::point(first[3]))?; }
    Ok(Some(NativePoleBoundary { axis, coordinate, world }))
}

/// Whole physical/UV guard for a coefficient-proven collapsed natural boundary.
/// The receipt grants only that connector image, not a spherical graph, a
/// one-sheet face, or the regularity of the quotient. Those obligations remain
/// independent. In particular this operation never interprets native UV as STEP.
pub(super) fn native_pole_joint_strip(
    surface: &Surface, domain: &PreparedFaceTrimDomain<'_>, joint: &SourcePoleJoint<'_>,
    axis: usize, upper: bool, strip: [[f64; 2]; 2],
    work: &mut GeometryWorkBudget, cancel: &AtomicBool,
) -> Result<Option<NativePoleBoundary>, GeometryError> {
    work.charge(1, cancel)?;
    if !selected_provider(surface, domain) {
        return Err(unavailable("native pole connector selected a different native carrier"));
    }
    let Some(boundary) = native_collapsed_boundary(surface, axis, upper, work, cancel)? else {
        return Ok(None);
    };
    let Some(connector) = source_connector(domain, joint, axis, Interval::point(boundary.coordinate), strip, work, cancel)? else {
        return Ok(None);
    };
    let image = retain_declared_vertices(
        crate::parametric::surface_piecewise_enclosure(surface, connector, work, cancel)?.position, joint)?;
    let residual = std::array::from_fn(|i| image[i].sub(Interval::point(joint.vertex_point[i])));
    if length(residual)?.hi > joint.physical_limit_mm { return Ok(None); }
    let pole_residual = std::array::from_fn(|i| boundary.world[i].sub(Interval::point(joint.vertex_point[i])));
    if length(pole_residual)?.hi > joint.physical_limit_mm { return Ok(None); }
    Ok(Some(boundary))
}

#[cfg(test)]
mod tests {
    use super::*;

    use solidcore::surface::{sphere::Sphere, surface::p1::StepSurface};
    use solidcore::geom::axis2_placement::Axis2Placement;
    use solidcore::nurbs::{srf_basic, b_spline_surface::BSplineSurface, math::CPoint};

    fn step_sphere() -> Surface {
        let kernel = crate::make_kernel_context();
        let sphere = Sphere::create_canonical(&kernel, &Axis2Placement::IDENTITY, 2.0).unwrap();
        StepSurface::new(sphere.into(), true).unwrap().into()
    }

    #[test]
    fn original_provider_keeps_literal_placement_and_radian_chain_law() {
        use crate::kernel::source_operand::PreparedOriginalSurface;
        use solidcraft_step::geom::{Surface as OriginalSurface, Placement};
        let source = OriginalSurface::Sphere { position: Placement {
            origin: [1.0,-2.0,3.0], axis: [0.0,0.0,1.0], ref_direction: [2.0,0.25,0.0],
        }, radius: 2.0 };
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let operand = PreparedOriginalSurface::new(&source,[[-0.5,0.5];2],usize::MAX,&mut work,&cancel).unwrap();
        let sphere = SphereChart::coefficients(&operand,true).unwrap();
        let pole = PoleChart::coefficients(&operand,SphericalPole::North).unwrap().unwrap();
        let uv = [Interval::point(0.2),Interval::point(0.3)];
        let jet = surface_enclosure(&operand,uv.map(|v| [v.lo,v.hi]),&mut work,&cancel).unwrap();
        let n = sphere.normal(jet.position).unwrap();
        assert!(dot(n,normal(&jet,true)).lo > 0.0);
        assert!(dot(n,jet.du).contains_zero());
        assert!(dot(n,jet.dv).contains_zero());
        let projected = pole.project(jet.position);
        let zero = Interval::point(0.0);
        let (q,_,_) = pole.covering_jet(uv,[zero;2],[zero;2],&mut work,&cancel).unwrap();
        assert!(projected[0].sub(q[0]).contains_zero());
        assert!(projected[1].sub(q[1]).contains_zero());
        let (_,first,second) = pole.covering_jet([zero;2],[Interval::point(1.0),zero],[zero;2],&mut work,&cancel).unwrap();
        assert!(first[1].contains(1.0));
        assert!(second[0].contains(-1.0));
        assert!(pole.principal_polar(Interval::point(0.3)));
        assert!(!pole.principal_polar(pole.pole_coordinate_enclosure()));
        assert!(!sphere.gauge.single_sheet([0.0,std::f64::consts::TAU]).unwrap());
        assert!(sphere.gauge.single_sheet([0.0,4.0]).unwrap());
        assert!(!sphere.gauge.single_sheet([0.0,90.0]).unwrap());
    }

    #[test]
    fn original_step_sphere_normals_agree_with_regular_oriented_native_jets() {
        let surface = step_sphere();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        for orientation in [false, true] {
            let chart = SphereChart::coefficients(&surface, orientation).unwrap();
            assert_eq!(chart.convex(), orientation);
            for latitude in [-80.0, -30.0, 0.0, 70.0] {
                for longitude in [0.0, 60.0, 170.0, 290.0] {
                    let jet = surface_enclosure(&surface, [[longitude; 2], [latitude; 2]], &mut work, &cancel).unwrap();
                    let n = chart.normal(jet.position).unwrap();
                    assert!(dot(n, normal(&jet, orientation)).lo > 0.0);
                    assert!(dot(n, jet.du).contains_zero());
                    assert!(dot(n, jet.dv).contains_zero());
                }
            }
        }
    }

    #[test]
    fn geometric_rank_persists_at_both_explicit_step_poles_without_changing_the_query() {
        let surface = step_sphere();
        let chart = SphereChart::coefficients(&surface, true).unwrap();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        for latitude in [-90.0, 90.0] {
            let uv = [[10.0, 40.0], [latitude; 2]];
            let jet = surface_enclosure(&surface, uv, &mut work, &cancel).unwrap();
            assert!(length(normal(&jet, true)).unwrap().lo <= 0.0);
            assert!(length(chart.normal(jet.position).unwrap()).unwrap().lo > 0.0);
            let unchanged = surface_enclosure(&surface, uv, &mut work, &cancel).unwrap();
            assert_eq!(jet.position, unchanged.position);
        }
    }

    fn collapsed_surface(perturbation: f64) -> Surface {
        let raw = srf_basic::Surface::new(vec![
            vec![CPoint::new(2.0, -4.0, 6.0, 2.0), CPoint::new(2.0, 0.0, 0.0, 2.0)],
            vec![CPoint::new(4.0 + perturbation, -8.0, 12.0, 4.0), CPoint::new(0.0, 4.0, 0.0, 4.0)],
        ], 1, 1, vec![0.0, 0.0, 1.0, 1.0], vec![0.0, 0.0, 1.0, 1.0]).unwrap();
        BSplineSurface::from_nurb(raw, true).into()
    }

    #[test]
    fn natural_boundary_collapse_uses_exact_homogeneous_coefficients_not_a_sphere_label() {
        let surface = collapsed_surface(0.0);
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        let receipt = native_collapsed_boundary(&surface, 1, false, &mut work, &cancel).unwrap().unwrap();
        assert_eq!(receipt.axis, 1);
        assert_eq!(receipt.coordinate, 0.0);
        for (interval, point) in receipt.world.into_iter().zip([1.0, -2.0, 3.0]) {
            assert!(interval.contains(point));
        }
        assert!(SphereChart::coefficients(&surface, true).is_none());
        assert!(native_collapsed_boundary(&collapsed_surface(up(4.0) - 4.0), 1, false, &mut work, &cancel).unwrap().is_none());
        assert!(native_collapsed_boundary(&surface, 1, true, &mut work, &cancel).unwrap().is_none());
        assert!(matches!(native_collapsed_boundary(&surface, 1, false, &mut work, &AtomicBool::new(true)),
            Err(error) if error.code == GeometryErrorCode::Cancelled));
        assert!(matches!(native_collapsed_boundary(&surface, 1, false, &mut GeometryWorkBudget::new(0), &cancel),
            Err(error) if error.code == GeometryErrorCode::ResourceLimit));
    }

    #[test]
    fn bare_native_sphere_does_not_borrow_the_explicit_step_identity() {
        let kernel = crate::make_kernel_context();
        let surface: Surface = Sphere::create_canonical(&kernel, &Axis2Placement::IDENTITY, 2.0).unwrap().into();
        assert!(SphereChart::coefficients(&surface, true).is_none());
        assert!(PoleChart::coefficients(&surface, SphericalPole::North).unwrap().is_none());
    }
    #[test]
    fn native_step_one_sheet_uses_degrees_not_old_radians() {
        let gauge = SphereGauge::native_step().unwrap();
        for r in [[0.0, 360.0], [-360.0, 360.0], [10.0, 730.0]] { assert!(!gauge.single_sheet(r).unwrap()); }
        for r in [[0.0, 90.0], [-90.0, 120.0], [720.0, 810.0]] { assert!(gauge.single_sheet(r).unwrap()); }
    }

    fn chart(sign: f64) -> PoleChart {
        let zero = Interval::point(0.0); let one = Interval::point(1.0);
        let id = [[one, zero, zero], [zero, one, zero], [zero, zero, one]];
        PoleChart { center: [zero; 3], columns: id, inverse: id, sign, gauge: SphereGauge::native_step().unwrap() }
    }

    #[test]
    fn covering_jet_retains_actual_degree_chain_factors() {
        let mut work = GeometryWorkBudget::new(1000); let cancel = AtomicBool::new(false);
        let chart = chart(1.0); let zero = Interval::point(0.0);
        let (q, first, second) = chart.covering_jet([zero, zero], [Interval::point(1.0), zero], [zero; 2], &mut work, &cancel).unwrap();
        assert!(q[0].lo <= 1.0 && q[0].hi >= 1.0);
        let d = std::f64::consts::PI / 180.0;
        assert!(first[1].lo <= d && first[1].hi >= d);
        assert!(second[0].lo <= -d*d && second[0].hi >= -d*d);
    }

    #[test]
    fn graph_derivatives_remain_on_the_actual_hemisphere() {
        let zero = Interval::point(0.0); let one = Interval::point(1.0);
        for sign in [-1.0, 1.0] {
            let (p, first, second) = chart(sign).graph_jet([zero; 2], [one, zero]).unwrap();
            assert!(p[2].lo <= sign && p[2].hi >= sign);
            assert!(first[0].lo <= 1.0 && first[0].hi >= 1.0);
            assert!(second[2].lo <= -sign && second[2].hi >= -sign);
        }
        assert!(chart(1.0).graph_jet([one, one], [one, zero]).is_err());
    }

    #[test]
    fn whole_strip_support_retains_endpoint_tubes_and_refuses_foreign_scope() {
        let pole = SphereGauge::original().north;
        let error = 1.0e-9;
        let enclosure = [[0.0,1.0],[pole.lo-error,pole.hi+error]];
        let endpoints = [
            [[0.0;2],[pole.lo-error,pole.hi]],
            [[1.0;2],[pole.lo,pole.hi+error]],
        ];
        let requested = [[0.0,1.0],[pole.lo,pole.hi+error]];
        let scope = supported_connector(enclosure,endpoints,1,pole,requested,[0.0;2]).unwrap().unwrap();
        assert!(contains(scope,enclosure));
        assert!(contains(scope,requested));
        let foreign = [[-0.25,1.0],requested[1]];
        assert!(supported_connector(enclosure,endpoints,1,pole,foreign,[0.0;2]).unwrap().is_none());
        let overlapping = [
            [[0.0,0.8],enclosure[1]],
            [[0.2,1.0],enclosure[1]],
        ];
        assert!(supported_connector(enclosure,overlapping,1,pole,requested,[0.0;2]).unwrap().is_none());
    }

    #[test]
    fn wider_pole_scope_keeps_its_actual_physical_displacement() {
        let surface = step_sphere();
        let chart = SphereChart::coefficients(&surface,true).unwrap();
        let pole = [Interval::point(0.0),Interval::point(0.0),Interval::point(2.0)];
        let narrow = sphere_world(&chart,[[0.0,90.0],[90.0;2]]).unwrap();
        let wider = sphere_world(&chart,[[0.0,90.0],[89.0,90.0]]).unwrap();
        let distance = |world:[Interval;3]| length(std::array::from_fn(|i| world[i].sub(pole[i]))).unwrap().hi;
        assert!(distance(narrow) < 1.0e-9);
        assert!(distance(wider) > 0.01);
    }
}
