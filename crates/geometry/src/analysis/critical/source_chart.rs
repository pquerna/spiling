// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Geometric regularity of an original carrier is distinct from the rank of
//! its covering coordinates. Only source coefficients and original Native pole
//! incidence can complete a collapsed spherical boundary chart here.

use super::*;
use monstertruck_modeling::{
    SphericalPole, TrimJointKind, ValidatedGeometryBudget, ValidatedInterval,
};

/// For x = A q + c, the oriented normal is sign * cof(A) q. Invertibility
/// proves a regular ellipsoid, including the poles of its spherical chart.
/// No approximate primitive recognition or replacement carrier participates.
pub(super) struct SphereChart {
    center: [Interval; 3],
    cofactors: [[Interval; 3]; 3],
    determinant: Interval,
    reversed: bool,
    outward: bool,
}

impl SphereChart {
    fn coefficients(surface: &Surface, orientation: bool) -> Option<Self> {
        let Surface::SphericalSurface(processor) = surface else {
            return None;
        };
        let matrix = processor.transform();
        let sphere = processor.entity();
        if !sphere.radius().is_finite()
            || sphere.radius() <= 0.0
            || (0..4).any(|column| (0..4).any(|row| !matrix[column][row].is_finite()))
            || matrix.x.w != 0.0
            || matrix.y.w != 0.0
            || matrix.z.w != 0.0
            || matrix.w.w != 1.0
        {
            return None;
        }
        let local = [sphere.center().x, sphere.center().y, sphere.center().z];
        if !local.into_iter().all(f64::is_finite) {
            return None;
        }
        let columns: [[Interval; 3]; 3] = std::array::from_fn(|column| {
            std::array::from_fn(|row| Interval::point(matrix[column][row]))
        });
        let cofactors = [
            cross(columns[1], columns[2]),
            cross(columns[2], columns[0]),
            cross(columns[0], columns[1]),
        ];
        let determinant = dot(columns[0], cofactors[0]);
        if !determinant.lo.is_finite() || !determinant.hi.is_finite() || determinant.contains_zero()
        {
            return None;
        }
        let center = std::array::from_fn(|row| {
            (0..3).fold(Interval::point(matrix[3][row]), |sum, column| {
                add(
                    sum,
                    mul(columns[column][row], Interval::point(local[column])),
                )
            })
        });
        if center
            .iter()
            .any(|value| !value.lo.is_finite() || !value.hi.is_finite())
        {
            return None;
        }
        let reversed = orientation != processor.orientation();
        Some(Self {
            center,
            cofactors,
            determinant,
            reversed,
            outward: (determinant.lo > 0.0) != reversed,
        })
    }

    /// The quotient is admitted only on one original chart sheet. A source
    /// pole joint must account for the ENTIRE overrun strip, not merely lie
    /// near its UV bounds. Retained source-map margins are never snapped away.
    pub(super) fn new(
        surface: &Surface,
        domain: &PreparedFaceTrimDomain<'_>,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        work.charge(1, cancel)?;
        let Some(chart) = Self::coefficients(surface, domain.native().face().orientation()) else {
            return Ok(None);
        };
        let Surface::SphericalSurface(processor) = surface else {
            unreachable!();
        };
        let polar = usize::from(!processor.orientation());
        let angular = 1 - polar;
        let bounds = domain.uv_bounds();
        if !azimuth_is_single_sheet(bounds[angular])? {
            return Ok(None);
        }
        for (pole, boundary, overrun) in [
            (SphericalPole::North, 0.0, bounds[polar][0] <= 0.0),
            (
                SphericalPole::South,
                down(std::f64::consts::PI),
                bounds[polar][1] >= down(std::f64::consts::PI),
            ),
        ] {
            if !overrun {
                continue;
            }
            let mut strip = bounds;
            match pole {
                SphericalPole::North => strip[polar][1] = strip[polar][1].min(boundary),
                SphericalPole::South => strip[polar][0] = strip[polar][0].max(boundary),
            }
            if !pole_strip(surface, domain, pole, polar, strip, &chart, work, cancel)? {
                return Ok(None);
            }
        }
        Ok(Some(chart))
    }

    pub(super) fn normal(&self, position: [Interval; 3]) -> Result<[Interval; 3], GeometryError> {
        let relative = std::array::from_fn(|axis| position[axis].sub(self.center[axis]));
        let mut local = [Interval::point(0.0); 3];
        for axis in 0..3 {
            local[axis] = dot(self.cofactors[axis], relative).div(self.determinant)?;
        }
        let normal: [Interval; 3] = std::array::from_fn(|row| {
            (0..3).fold(Interval::point(0.0), |sum, column| {
                add(sum, mul(self.cofactors[column][row], local[column]))
            })
        });
        Ok(if self.reversed {
            normal.map(Interval::neg)
        } else {
            normal
        })
    }

    /// The implicit quadratic has positive-definite Hessian A^-T A^-1.
    /// Hence the outward second fundamental form is strictly negative in
    /// every nonzero tangent direction, including any planar meridian fiber.
    pub(super) fn convex(&self) -> bool {
        self.outward
    }
}

fn azimuth_is_single_sheet(range: [f64; 2]) -> Result<bool, GeometryError> {
    let width = Interval::new(range[0], range[1])?.width();
    Ok(up(width) < down(std::f64::consts::TAU))
}

fn contains(outer: [[f64; 2]; 2], inner: [[f64; 2]; 2]) -> bool {
    (0..2).all(|axis| outer[axis][0] <= inner[axis][0] && inner[axis][1] <= outer[axis][1])
}

#[allow(clippy::too_many_arguments)]
fn pole_strip(
    surface: &Surface,
    domain: &PreparedFaceTrimDomain<'_>,
    pole: SphericalPole,
    polar: usize,
    strip: [[f64; 2]; 2],
    chart: &SphereChart,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let mut qualified = false;
    for joint in domain.native().joints() {
        work.charge(1, cancel)?;
        match joint.kind {
            TrimJointKind::SphericalPole {
                polar_axis,
                pole: declared,
                ..
            } if polar_axis == polar && declared == pole => {}
            TrimJointKind::EndpointUncertainty => {}
            _ => continue,
        }
        if pole_joint_strip(
            surface, domain, joint, pole, polar, strip, chart, work, cancel,
        )? {
            // Count successful ORIGINAL connectors, not nearby UV candidates.
            // Two completed fans at one pole do not establish degree one.
            if qualified {
                return Ok(false);
            }
            qualified = true;
        }
    }
    Ok(qualified)
}

#[allow(clippy::too_many_arguments)]
fn pole_joint_strip(
    surface: &Surface,
    domain: &PreparedFaceTrimDomain<'_>,
    joint: &monstertruck_modeling::PreparedTrimJoint,
    pole: SphericalPole,
    polar: usize,
    strip: [[f64; 2]; 2],
    chart: &SphereChart,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, GeometryError> {
    let angular = 1 - polar;
    let margin = domain.native().uncertainty_uv();
    let mut supported = joint.enclosure.map(|value| [value.lower, value.upper]);
    // This only proposes a whole connection to Native. The saved ordinary
    // endpoint label grants no pole authority, and neither original endpoint
    // evidence nor any source cell is moved to the proposed pole.
    let exact_pole = match pole {
        SphericalPole::North => [0.0; 2],
        SphericalPole::South => [down(std::f64::consts::PI), up(std::f64::consts::PI)],
    };
    supported[polar][0] = supported[polar][0].min(exact_pole[0]);
    supported[polar][1] = supported[polar][1].max(exact_pole[1]);
    let mut endpoints = [[[0.0; 2]; 2]; 2];
    for (slot, use_) in [joint.incoming, joint.outgoing].into_iter().enumerate() {
        work.charge(domain.native().coedges().len() as u64, cancel)?;
        let Some((index, coedge)) =
            domain
                .native()
                .coedges()
                .iter()
                .enumerate()
                .find(|(_, coedge)| {
                    let identity = coedge.identity;
                    identity.face_id == use_.face_id
                        && identity.edge_id == use_.edge_id
                        && identity.wire_index == use_.wire_index
                        && identity.coedge_index == use_.coedge_index
                        && identity.orientation == use_.orientation
                })
        else {
            return Ok(false);
        };
        let face = domain.native().face();
        let edge = &face.absolute_boundaries()[use_.wire_index][use_.coedge_index];
        let endpoint = if (slot == 0) == face.orientation() {
            edge.back()
        } else {
            edge.front()
        };
        if endpoint.id() != joint.vertex_id {
            return Ok(false);
        }
        let parameter = coedge.oriented_range[1 - slot];
        endpoints[slot] =
            domain.boundary_position_enclosure(index, [parameter; 2], work, cancel)?;
        // The joint keeps canonical nominal endpoint expressions; a native
        // jet at that SAME directed endpoint also retains evaluation and
        // source-map rounding. Both are original support, not guessed padding.
        for axis in 0..2 {
            supported[axis][0] = supported[axis][0].min(endpoints[slot][axis][0]);
            supported[axis][1] = supported[axis][1].max(endpoints[slot][axis][1]);
        }
    }
    // Native applies its covering margin after the complete original
    // enclosure hull. Marginalizing only the canonical joint would omit
    // the boundary endpoint's independently retained evaluation rounding.
    for axis in 0..2 {
        let value = ValidatedInterval {
            lower: supported[axis][0],
            upper: supported[axis][1],
        } + ValidatedInterval {
            lower: -margin[axis],
            upper: margin[axis],
        };
        supported[axis] = [value.lower, value.upper];
    }
    // Check association only after gathering the complete original endpoint
    // tubes, and BEFORE extending any proposed connection to the search strip.
    if !contains(supported, strip) {
        return Ok(false);
    }
    for endpoint in &mut endpoints {
        endpoint[polar][0] = endpoint[polar][0]
            .min(strip[polar][0])
            .min(joint.enclosure[polar].lower);
        endpoint[polar][1] = endpoint[polar][1]
            .max(strip[polar][1])
            .max(joint.enclosure[polar].upper);
    }
    let lower = if endpoints[0][angular][1] < endpoints[1][angular][0] {
        0
    } else if endpoints[1][angular][1] < endpoints[0][angular][0] {
        1
    } else {
        return Ok(false);
    };
    endpoints[lower][angular][0] = endpoints[lower][angular][0]
        .min(strip[angular][0])
        .min(joint.enclosure[angular].lower);
    endpoints[1 - lower][angular][1] = endpoints[1 - lower][angular][1]
        .max(strip[angular][1])
        .max(joint.enclosure[angular].upper);
    if domain.source_chart_error_mm() <= 0.0 {
        return Ok(false);
    }
    let mut native =
        ValidatedGeometryBudget::new(usize::try_from(work.remaining()).unwrap_or(usize::MAX));
    let result = monstertruck_modeling::certified_surface_parameter_joint(
        surface,
        joint.vertex_point,
        endpoints.map(|endpoint| {
            endpoint.map(|value| ValidatedInterval {
                lower: value[0],
                upper: value[1],
            })
        }),
        [0.0; 2],
        domain.source_chart_error_mm(),
        &mut native,
        cancel,
    );
    crate::parametric::debit_native_work(work, native.consumed(), cancel)?;
    let kind = match result {
        Ok(kind) => kind,
        Err(monstertruck_modeling::TrimDomainError::Geometry(error)) => {
            return Err(crate::parametric::native_error(error));
        }
        Err(monstertruck_modeling::TrimDomainError::ResourceLimit) => {
            return Err(GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "critical original pole connection capacity exhausted",
            ));
        }
        Err(_) => return Ok(false),
    };
    let Some(TrimJointKind::SphericalPole {
        polar_axis,
        pole: declared,
        polar_coordinate,
        world_enclosure,
        ..
    }) = kind
    else {
        return Ok(false);
    };
    let mut receipt = std::array::from_fn(|axis| {
        [
            endpoints[0][axis][0].min(endpoints[1][axis][0]),
            endpoints[0][axis][1].max(endpoints[1][axis][1]),
        ]
    });
    receipt[polar] = [polar_coordinate.lower, polar_coordinate.upper];
    if polar_axis != polar || declared != pole || !contains(receipt, strip) {
        return Ok(false);
    }
    let world = world_enclosure.map(|value| Interval {
        lo: value.lower,
        hi: value.upper,
    });
    let rank = length(chart.normal(world)?)?;
    Ok(rank.lo > 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use monstertruck_modeling::{Matrix4, Point3, Processor, Sphere, base::Invertible};

    fn sphere(matrix: Matrix4, inverted: bool) -> Surface {
        let mut processor =
            Processor::with_transform(Sphere::new(Point3::new(1.0, -2.0, 3.0), 2.0), matrix);
        if inverted {
            processor.invert();
        }
        Surface::SphericalSurface(processor)
    }

    #[test]
    fn original_affine_sphere_normals_agree_with_regular_oriented_jets() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(1_000_000);
        for reflection in [-1.0, 1.0] {
            // Native matrices are column-major; keep the actual affine last
            // row explicit rather than accidentally testing a projective map.
            let matrix = Matrix4::from_cols(
                [reflection * 2.0, 0.5, 0.0, 0.0].into(),
                [0.0, 3.0, 0.25, 0.0].into(),
                [0.0, 0.0, 4.0, 0.0].into(),
                [10.0, 11.0, 12.0, 1.0].into(),
            );
            for inverted in [false, true] {
                let surface = sphere(matrix, inverted);
                for face_orientation in [false, true] {
                    let chart = SphereChart::coefficients(&surface, face_orientation).unwrap();
                    assert_eq!(
                        chart.convex(),
                        (reflection > 0.0) == (face_orientation != inverted)
                    );
                    for polar in [0.1, 0.7, 1.6, 3.0] {
                        for azimuth in [-1.2, 0.0, 2.3] {
                            let coordinates = if inverted {
                                [azimuth, polar]
                            } else {
                                [polar, azimuth]
                            };
                            let jet = surface_enclosure(
                                &surface,
                                coordinates.map(|v| [v; 2]),
                                &mut work,
                                &cancel,
                            )
                            .unwrap();
                            let geometric = chart.normal(jet.position).unwrap();
                            let parametric = normal(&jet, face_orientation);
                            assert!(dot(geometric, parametric).lo > 0.0);
                            // These independent source constructions enclose
                            // an identical tangent plane, not just a direction
                            // selected by a normal sample or a fitted sphere.
                            assert!(dot(geometric, jet.du).contains_zero());
                            assert!(dot(geometric, jet.dv).contains_zero());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn geometric_rank_persists_at_both_collapsed_chart_boundaries() {
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(100_000);
        let surface = sphere(Matrix4::from_scale(1.0), false);
        let chart = SphereChart::coefficients(&surface, true).unwrap();
        for polar in [
            [0.0, 0.0],
            [down(std::f64::consts::PI), up(std::f64::consts::PI)],
        ] {
            let jet =
                surface_enclosure(&surface, [polar, [-0.2, 1.7]], &mut work, &cancel).unwrap();
            assert!(length(normal(&jet, true)).unwrap().lo <= 0.0);
            assert!(length(chart.normal(jet.position).unwrap()).unwrap().lo > 0.0);
            assert!(chart.convex());
        }
    }

    #[test]
    fn singular_projective_nonfinite_and_nonpositive_source_coefficients_refuse() {
        for matrix in [
            Matrix4::from_nonuniform_scale(1.0, 0.0, 1.0),
            Matrix4::from_nonuniform_scale(1.0, f64::INFINITY, 1.0),
            Matrix4::from_cols(
                [1.0, 0.0, 0.0, 0.0].into(),
                [0.0, 1.0, 0.0, 0.0].into(),
                [0.0, 0.0, 1.0, 0.25].into(),
                [0.0, 0.0, 0.0, 1.0].into(),
            ),
        ] {
            assert!(SphereChart::coefficients(&sphere(matrix, false), true).is_none());
        }
        for radius in [-1.0, 0.0, f64::NAN] {
            let surface = Surface::SphericalSurface(Processor::new(Sphere::new(
                Point3::new(0.0, 0.0, 0.0),
                radius,
            )));
            assert!(SphereChart::coefficients(&surface, true).is_none());
        }
    }

    #[test]
    fn independent_housing_original_pole_has_geometric_rank_without_changing_cell() {
        let cancel = AtomicBool::new(false);
        let definition = crate::import_step(include_bytes!(
            "../../../../../fixtures/geometry/independent-exporters/occt-rounded-housing-mm.step"
        ), &cancel).unwrap();
        let mut work = GeometryWorkBudget::new(30_000_000);
        let domain = prepare_face_trim_domain_with_byte_limit(
            &definition,
            5,
            MAX_SCRATCH_BYTES,
            &mut work,
            &cancel,
        )
        .unwrap();
        assert_eq!(definition.faces[5].surface_face_entity, "461");
        assert!(
            domain.native().joints().iter().any(|joint| {
                matches!(joint.kind, TrimJointKind::EndpointUncertainty)
                    && joint.vertex_point == monstertruck_modeling::Point3::new(2.0, 2.0, 0.0)
            }),
            "the original rounded pole endpoint is not already Native pole authority"
        );
        domain.native().face().with_surface(|surface| {
            let chart = SphereChart::new(surface, &domain, &mut work, &cancel)
                .unwrap()
                .unwrap_or_else(|| panic!(
                    "original connector must receive Native pole authority: bounds={:?}, physical_limit={}, joints={:?}",
                    domain.uv_bounds(), domain.source_chart_error_mm(), domain.native().joints()
                ));
            assert!(domain.native().joints().iter().any(|joint| {
                matches!(joint.kind, TrimJointKind::EndpointUncertainty)
                    && joint.vertex_point == monstertruck_modeling::Point3::new(2.0, 2.0, 0.0)
            }), "fresh pole authority does not relabel the original endpoint evidence");
            let uv = [
                [3.141592630183209, 3.1415926535898984],
                [1.5707963033883152, 1.5707963267950045],
            ];
            let jet = surface_enclosure(surface, uv, &mut work, &cancel).unwrap();
            assert!(
                length(normal(&jet, domain.native().face().orientation()))
                    .unwrap()
                    .lo
                    <= 0.0
            );
            let geometric = chart.normal(jet.position).unwrap();
            assert!(length(geometric).unwrap().lo > 0.0);
            let relative = std::array::from_fn(|axis| {
                jet.position[axis].sub(Interval::point([12.0, 9.0, 5.0][axis]))
            });
            assert!(dot(geometric, relative).lo > 0.0);
            assert!(chart.convex());
            // No chart clipping, snapping or replacement-position query.
            let unchanged = surface_enclosure(surface, uv, &mut work, &cancel).unwrap();
            for axis in 0..3 {
                assert_eq!(
                    [jet.position[axis].lo, jet.position[axis].hi],
                    [unchanged.position[axis].lo, unchanged.position[axis].hi]
                );
            }
            let wrong_axis = 1;
            assert!(
                !pole_strip(
                    surface,
                    &domain,
                    SphericalPole::South,
                    wrong_axis,
                    uv,
                    &chart,
                    &mut work,
                    &cancel
                )
                .unwrap()
            );
            let mut interior_fold = domain.uv_bounds();
            interior_fold[0] = [std::f64::consts::PI - 0.1, std::f64::consts::PI + 0.1];
            assert!(
                !pole_strip(
                    surface,
                    &domain,
                    SphericalPole::South,
                    0,
                    interior_fold,
                    &chart,
                    &mut work,
                    &cancel
                )
                .unwrap()
            );
            assert!(
                !pole_strip(
                    surface,
                    &domain,
                    SphericalPole::North,
                    0,
                    uv,
                    &chart,
                    &mut work,
                    &cancel
                )
                .unwrap()
            );
            let cancelled = AtomicBool::new(true);
            assert!(matches!(
                SphereChart::new(surface, &domain, &mut work, &cancelled),
                Err(error) if error.code == GeometryErrorCode::Cancelled
            ));
            let mut exhausted = GeometryWorkBudget::new(0);
            assert!(matches!(
                SphereChart::new(surface, &domain, &mut exhausted, &cancel),
                Err(error) if error.code == GeometryErrorCode::ResourceLimit
            ));
        });
    }

    #[test]
    fn one_sheet_gate_rejects_period_aliases_without_narrowing_the_chart() {
        let tau = std::f64::consts::TAU;
        for range in [[0.0, tau], [-tau, tau], [1.0, 1.0 + 2.0 * tau]] {
            assert!(!azimuth_is_single_sheet(range).unwrap());
        }
        for range in [[0.0, 1.5], [-2.0, 3.0], [10.0, 11.0]] {
            assert!(azimuth_is_single_sheet(range).unwrap());
        }
    }
}
