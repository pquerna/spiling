// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Original-use, bounded material transport. These receipts never assert C1 or
//! an exact common carrier normal. The immutable recipe is source-relative;
//! changing a query's radial witness cannot change its boundary realization.

use super::super::source_incidence::CoedgeUse;
use super::*;
use crate::parametric::{ChartCurveEnclosure, CurveEnclosure};

mod coverage;
mod fiber;
mod transport;
mod vertex_fan;
mod vertex_transport;
mod wall;
pub(super) use coverage::{ClosedRadialLinks, MAX_CORRECTION_BYTES};
pub(crate) use transport::{
    SourceTransportOwner, SourceTransportSupport, source_transport_supports,
};
pub(crate) use wall::prove_source_wall_component;

pub(super) struct MaterialLinkCell {
    pub position: [Interval; 3],
    pub source_error_mm: f64,
    pub convex_link: bool,
    outward: [Interval; 3],
    second_outward: [Interval; 3],
}

impl MaterialLinkCell {
    pub(super) fn regular_flow_rate(
        &self,
        tangent: [Interval; 3],
        height: [Interval; 3],
        vertical: [Interval; 3],
    ) -> Result<Option<f64>, GeometryError> {
        let mut rate = material_edge_flow_rate(tangent, height, vertical)?;
        include_flow_rate(
            &mut rate,
            material_face_flow_rate(self.outward, height, vertical)?,
        );
        include_flow_rate(
            &mut rate,
            material_face_flow_rate(self.second_outward, height, vertical)?,
        );
        Ok(rate)
    }
}

/// Actual cross-ribbon position and first jet at ONE original native parameter.
/// All fields use the unchanged original coefficient/map operands.
#[derive(Debug, Clone, Copy)]
struct CollarBoundaryJet {
    uv_offset: [Interval; 2],
    uv: [Interval; 2],
    position: [Interval; 3],
    derivative: [Interval; 3],
    boundary_displacement: [Interval; 3],
    constant: bool,
}

fn exact_difference(a: Interval, b: Interval) -> Interval {
    if a.lo == a.hi && b.lo == b.hi && a.lo == b.lo {
        Interval::point(0.0)
    } else {
        a.sub(b)
    }
}

fn rotate(vector: [Interval; 2], inward: f64) -> [Interval; 2] {
    [
        mul(vector[1].neg(), Interval::point(inward)),
        mul(vector[0], Interval::point(inward)),
    ]
}
fn determinant(a: [Interval; 2], b: [Interval; 2]) -> Interval {
    add(mul(a[0], b[1]), mul(a[1], b[0]).neg())
}
fn chart_jet(
    domain: &PreparedFaceTrimDomain<'_>,
    coedge: usize,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<ChartCurveEnclosure, GeometryError> {
    domain.original_coedge_uv_jet(coedge, range, work, cancel)
}
fn add_margin(position: [Interval; 2], margin: [f64; 2]) -> [Interval; 2] {
    std::array::from_fn(|axis| {
        add(
            position[axis],
            Interval {
                lo: -margin[axis],
                hi: margin[axis],
            },
        )
    })
}
fn uv_box(position: [Interval; 2]) -> [[f64; 2]; 2] {
    position.map(|value| [value.lo, value.hi])
}
fn compose_first(jet: &SurfaceEnclosure, tangent: [Interval; 2]) -> [Interval; 3] {
    std::array::from_fn(|axis| add(mul(jet.du[axis], tangent[0]), mul(jet.dv[axis], tangent[1])))
}
fn source_allowance(definition: &Definition) -> Result<f64, GeometryError> {
    definition
        .provenance
        .uncertainty_mm
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or_else(|| {
            failure(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "source has no finite admitted material-realization allowance",
                0,
                (),
            )
        })
}

/// Pointwise scalar law for an entire realized face, not an embedding receipt.
///
/// The source carrier, every original boundary curve and every NativeV must
/// have the SAME exact scalar. Then the collar's affine blend and the selected
/// conical completion preserve that scalar identically. An interval merely
/// containing the candidate height is insufficient, even at roundoff scale.
pub(crate) fn constant_material_face_height(
    definition: &Definition,
    face_index: usize,
    functional: HeightFunctional,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<f64>, GeometryError> {
    let Some(height) = super::super::slab::constant_source_face_height(
        definition, face_index, functional, work, cancel,
    )?
    else {
        return Ok(None);
    };
    if !height.lo.is_finite() || height.lo != height.hi {
        return Ok(None);
    }
    let face = definition
        .solid
        .face_iter()
        .nth(face_index)
        .ok_or_else(|| {
            failure(
                GeometryErrorCode::InvalidGeometry,
                "material scalar law lost its original face",
                face_index,
                (),
            )
        })?;
    for wire in face.absolute_boundaries() {
        for edge in wire {
            work.charge(1, cancel)?;
            let curve_height = edge.with_curve(|curve| {
                super::super::slab::constant_native_curve_height(curve, functional, work, cancel)
            })?;
            if curve_height.is_none_or(|value| value.lo != height.lo || value.hi != height.hi) {
                return Ok(None);
            }
        }
    }
    for vertex in face.vertex_iter() {
        work.charge(1, cancel)?;
        let point: [f64; 3] = vertex.point().into();
        let value = functional.vertex(point);
        if value.lo != height.lo || value.hi != height.hi {
            return Ok(None);
        }
    }
    Ok(Some(height.lo))
}

/// One source-only choice of a complete coedge collar. The range, transverse
/// field and width depend exclusively on retained original coefficients. Query
/// refinement cannot select a different realization at a shared height.
#[derive(Clone, Copy)]
enum CarrierTransport {
    Exact,
    Collar { width: f64, inward: f64 },
}
struct OriginalCollar<'a, 'd> {
    domain: &'a PreparedFaceTrimDomain<'d>,
    use_: CoedgeUse,
    range: [f64; 2],
    transport: CarrierTransport,
    margin: [f64; 2],
    position_error_mm: f64,
    uv_univalent: bool,
}
impl<'a, 'd> OriginalCollar<'a, 'd> {
    fn new(
        domain: &'a PreparedFaceTrimDomain<'d>,
        use_: CoedgeUse,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        Self::build(domain, use_, true, work, cancel)
    }

    /// Evaluate the immutable recipe for bounds, without asserting that this
    /// entire UV ribbon is an embedded material chart.
    fn for_enclosure(
        domain: &'a PreparedFaceTrimDomain<'d>,
        use_: CoedgeUse,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        Self::build(domain, use_, false, work, cancel)
    }

    fn build(
        domain: &'a PreparedFaceTrimDomain<'d>,
        use_: CoedgeUse,
        require_univalence: bool,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        work.charge(1, cancel)?;
        let proof = domain.original_coedge_correspondence(use_.coedge)?;
        if require_univalence
            && proof.provenance()
                == crate::kernel::OriginalCorrespondenceProvenance::CompletedPoleGraph
        {
            return Ok(None);
        }
        let range = proof.native_range();
        let margin = proof.margin_uv();
        if proof.provenance() == crate::kernel::OriginalCorrespondenceProvenance::ExactCoefficients
            && proof.position_error_mm() == 0.0
        {
            return Ok(Some(Self {
                domain,
                use_,
                range,
                transport: CarrierTransport::Exact,
                margin,
                position_error_mm: 0.0,
                uv_univalent: true,
            }));
        }
        let width =
            (range[1] - range[0]) * domain.material_realization().collar_parameter_fraction();
        if !width.is_finite() || width <= 0.0 {
            return Ok(None);
        }
        let midpoint = range[0] * 0.5 + range[1] * 0.5;
        let middle = chart_jet(domain, use_.coedge, [midpoint; 2], work, cancel)?;
        let offset =
            rotate(middle.derivative, 1.0).map(|value| mul(value, Interval::point(width * 0.125)));
        let mut inside = None;
        for sign in [1.0, -1.0] {
            let probe = std::array::from_fn(|axis| {
                add(
                    middle.position[axis],
                    mul(offset[axis], Interval::point(sign)),
                )
            });
            if domain.classify(uv_box(add_margin(probe, margin)), work, cancel)?
                == TrimClassification::Inside
            {
                if inside.is_some() {
                    return Ok(None);
                }
                inside = Some(sign);
            }
        }
        let Some(inward) = inside else {
            return Ok(None);
        };
        if !require_univalence {
            return Ok(Some(Self {
                domain,
                use_,
                range,
                transport: CarrierTransport::Collar { width, inward },
                margin,
                position_error_mm: proof.position_error_mm(),
                uv_univalent: false,
            }));
        }
        // Global univalence of the ORIGINAL UV collar, not a sampled direction.
        // With a monotone primary coordinate, the two-dimensional Jacobian is
        // a P-matrix after fixed row/column signs. The endpoint bubble is strictly
        // positive in the open range and collapses only at original endpoints.
        let whole = chart_jet(domain, use_.coedge, range, work, cancel)?;
        let primary =
            usize::from(middle.derivative[1].abs_upper() > middle.derivative[0].abs_upper());
        let direction = if middle.derivative[primary].lo > 0.0 {
            1.0
        } else if middle.derivative[primary].hi < 0.0 {
            -1.0
        } else {
            return Ok(None);
        };
        let v = rotate(whole.derivative, inward);
        let vt = rotate(whole.second_derivative, inward);
        let beta = Interval { lo: 0.0, hi: 0.25 };
        let beta_t = Interval { lo: -1.0, hi: 1.0 }
            .div(Interval::point(range[1]).sub(Interval::point(range[0])))?;
        let perturbation = add(mul(beta_t, v[primary]), mul(beta, vt[primary]));
        let primary_derivative = add(
            whole.derivative[primary],
            mul(Interval { lo: 0.0, hi: width }, perturbation),
        );
        let primary_positive = mul(primary_derivative, Interval::point(direction)).lo > 0.0;
        let base = determinant(whole.derivative, v);
        let bend = mul(
            mul(Interval { lo: 0.0, hi: width }, beta),
            determinant(vt, v),
        );
        let oriented = mul(add(base, bend), Interval::point(inward));
        if !primary_positive || oriented.lo <= 0.0 {
            return Ok(None);
        }
        Ok(Some(Self {
            domain,
            use_,
            range,
            transport: CarrierTransport::Collar { width, inward },
            margin,
            position_error_mm: proof.position_error_mm(),
            uv_univalent: true,
        }))
    }

    fn bubble(&self, range: [f64; 2]) -> Result<(Interval, Interval), GeometryError> {
        let span = Interval::point(self.range[1]).sub(Interval::point(self.range[0]));
        let normalized = Interval::new(range[0], range[1])?
            .sub(Interval::point(self.range[0]))
            .div(span)?;
        // The interval expression is intersected with the independent exact
        // range bound of x(1-x), not a rounding or geometric tolerance clamp.
        let beta = intersection(
            mul(normalized, Interval::point(1.0).sub(normalized)),
            Interval { lo: 0.0, hi: 0.25 },
        )
        .ok_or_else(|| {
            failure(
                GeometryErrorCode::ParameterCorrespondenceUnavailable,
                "material collar parameter leaves its original range",
                self.use_.face,
                range,
            )
        })?;
        let beta_t = Interval::point(1.0)
            .sub(mul(Interval::point(2.0), normalized))
            .div(span)?;
        Ok((beta, beta_t))
    }

    /// Position-only covering for activity and regular-fiber exclusion. This
    /// grants no embedding or material authority.
    fn swept_enclosure(
        &self,
        range: [f64; 2],
        curve: &CurveEnclosure,
        allowance: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[Interval; 3], GeometryError> {
        let CarrierTransport::Collar { width, inward } = self.transport else {
            return Ok(curve.position.map(|value| {
                add(
                    value,
                    Interval {
                        lo: -allowance,
                        hi: allowance,
                    },
                )
            }));
        };
        let uv = chart_jet(self.domain, self.use_.coedge, range, work, cancel)?;
        let (beta, _) = self.bubble(range)?;
        let v = rotate(uv.derivative, inward);
        let tube = std::array::from_fn(|axis| {
            add(
                uv.position[axis],
                mul(Interval { lo: 0.0, hi: width }, mul(beta, v[axis])),
            )
        });
        self.domain.native().face().with_surface(|surface| {
            Ok(crate::parametric::surface_piecewise_enclosure(
                surface,
                uv_box(add_margin(tube, self.margin)),
                work,
                cancel,
            )?
            .position
            .map(|value| {
                add(
                    value,
                    Interval {
                        lo: -allowance,
                        hi: allowance,
                    },
                )
            }))
        })
    }

    fn other_collar_enclosure(
        &self,
        coedge: usize,
        range: [f64; 2],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[Interval; 2], GeometryError> {
        let position = self
            .domain
            .boundary_position_enclosure(coedge, range, work, cancel)?
            .map(|value| Interval {
                lo: value[0],
                hi: value[1],
            });
        let proof = self.domain.original_coedge_correspondence(coedge)?;
        if proof.provenance() == crate::kernel::OriginalCorrespondenceProvenance::ExactCoefficients
            && proof.position_error_mm() == 0.0
        {
            return Ok(position);
        }
        let original = &self.domain.native().coedges()[coedge];
        let mut native = monstertruck_modeling::ValidatedGeometryBudget::new(
            usize::try_from(work.remaining()).unwrap_or(usize::MAX),
        );
        let result = monstertruck_modeling::validated_curve2d_jet(
            original.curve,
            range,
            &mut native,
            cancel,
        );
        crate::parametric::debit_native_work(work, native.consumed(), cancel)?;
        let jet = result.map_err(crate::parametric::native_error)?;
        let derivative = jet.first.map(|value| {
            mul(
                Interval {
                    lo: value.lower,
                    hi: value.upper,
                },
                proof.uv_parameter_derivative(),
            )
        });
        let v = rotate(derivative, 1.0);
        let native_range = proof.native_range();
        let width = (native_range[1] - native_range[0])
            * self
                .domain
                .material_realization()
                .collar_parameter_fraction();
        // The other original inward choice is not guessed. Both signs cover
        // its already fixed source-only recipe; this is exclusion only.
        Ok(std::array::from_fn(|axis| {
            add(
                position[axis],
                mul(
                    Interval {
                        lo: -width * 0.25,
                        hi: width * 0.25,
                    },
                    v[axis],
                ),
            )
        }))
    }

    fn boundary_jet(
        &self,
        parameter: f64,
        across: [f64; 2],
        curve: &CurveEnclosure,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<CollarBoundaryJet, GeometryError> {
        work.charge(1, cancel)?;
        if !across[0].is_finite()
            || !across[1].is_finite()
            || across[0] < 0.0
            || across[1] > 1.0
            || across[0] > across[1]
        {
            return Err(failure(
                GeometryErrorCode::InvalidGeometry,
                "source collar cross-section leaves its fixed recipe",
                self.use_.face,
                across,
            ));
        }
        let across = Interval::new(across[0], across[1])?;
        let uv = chart_jet(self.domain, self.use_.coedge, [parameter; 2], work, cancel)?;
        let CarrierTransport::Collar { width, inward } = self.transport else {
            return Ok(CollarBoundaryJet {
                uv_offset: [Interval::point(0.0); 2],
                uv: uv.position,
                position: curve.position,
                derivative: [Interval::point(0.0); 3],
                boundary_displacement: [Interval::point(0.0); 3],
                constant: true,
            });
        };
        let (beta, _) = self.bubble([parameter; 2])?;
        let offset = rotate(uv.derivative, inward)
            .map(|value| mul(mul(Interval::point(width), beta), value));
        let at = std::array::from_fn(|axis| add(uv.position[axis], mul(across, offset[axis])));
        self.domain.native().face().with_surface(|surface| {
            let boundary = surface_enclosure(
                surface,
                uv_box(add_margin(uv.position, self.margin)),
                work,
                cancel,
            )?;
            let base =
                surface_enclosure(surface, uv_box(add_margin(at, self.margin)), work, cancel)?;
            let displacement = std::array::from_fn(|axis| {
                intersection(
                    exact_difference(curve.position[axis], boundary.position[axis]),
                    Interval {
                        lo: -self.position_error_mm,
                        hi: self.position_error_mm,
                    },
                )
            });
            let [Some(dx), Some(dy), Some(dz)] = displacement else {
                return Err(failure(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "original collar endpoint contradicts retained forward bound",
                    self.use_.face,
                    parameter,
                ));
            };
            let displacement = [dx, dy, dz];
            // Across was validated in [0,1] before this independently known
            // affine range is used; this never clips unproved coordinates.
            let weight = intersection(
                Interval::point(1.0).sub(across),
                Interval { lo: 0.0, hi: 1.0 },
            )
            .ok_or_else(|| {
                failure(
                    GeometryErrorCode::InvalidGeometry,
                    "source collar weight has no unit image",
                    self.use_.face,
                    across,
                )
            })?;
            let position = std::array::from_fn(|axis| {
                add(base.position[axis], mul(weight, displacement[axis]))
            });
            let first = compose_first(&base, offset);
            let derivative =
                std::array::from_fn(|axis| exact_difference(first[axis], displacement[axis]));
            Ok(CollarBoundaryJet {
                uv_offset: offset,
                uv: at,
                position,
                derivative,
                boundary_displacement: displacement,
                constant: false,
            })
        })
    }

    fn cell(
        &self,
        range: [f64; 2],
        curve: &CurveEnclosure,
        prism: &SamplePrism,
        allowance: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<MaterialLinkCell>, GeometryError> {
        let Some(cell) = self.geometry_cell(range, curve, allowance, work, cancel)? else {
            return Ok(None);
        };
        let Some((radial, _)) = prism.radial_projection_frame(cell.position)? else {
            return Ok(None);
        };
        if dot(cell.outward, radial).lo <= 0.0 {
            return Ok(None);
        }
        Ok(Some(cell))
    }

    /// The actual source-owned ribbon, independent of a certification field.
    /// A caller must still prove its physical projection and global degree.
    fn geometry_cell(
        &self,
        range: [f64; 2],
        curve: &CurveEnclosure,
        allowance: f64,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<MaterialLinkCell>, GeometryError> {
        if !self.uv_univalent {
            return Ok(None);
        }
        let (collar_width, inward) = match self.transport {
            CarrierTransport::Exact => {
                let normal = coedge_normal_enclosure(
                    self.domain,
                    self.use_.coedge,
                    curve.position,
                    work,
                    cancel,
                )?;
                if length(normal)?.lo <= 0.0 {
                    return Ok(None);
                }
                return Ok(Some(MaterialLinkCell {
                    position: curve.position,
                    source_error_mm: 0.0,
                    convex_link: true,
                    outward: normal,
                    second_outward: normal,
                }));
            }
            CarrierTransport::Collar { width, inward } => (width, inward),
        };
        work.charge(1, cancel)?;
        let uv = chart_jet(self.domain, self.use_.coedge, range, work, cancel)?;
        let (beta, beta_t) = self.bubble(range)?;
        if beta.lo <= 0.0 {
            return Ok(None);
        }
        let v = rotate(uv.derivative, inward);
        let vt = rotate(uv.second_derivative, inward);
        let width = Interval::point(collar_width);
        let offset = v.map(|value| mul(mul(width, beta), value));
        let tube = std::array::from_fn(|axis| {
            add(
                uv.position[axis],
                mul(Interval { lo: 0.0, hi: 1.0 }, offset[axis]),
            )
        });
        let owned_tube = add_margin(tube, self.margin);
        // Every other original trim must miss the complete collar. The owning
        // boundary itself is covered by the global UV univalence proof above.
        for (index, _) in self.domain.native().coedges().iter().enumerate() {
            if index == self.use_.coedge {
                continue;
            }
            for span in self.domain.boundary_parameter_intervals(index)? {
                let mut stack = [([0.0; 2], 0_u8); MAX_DEPTH as usize + 1];
                stack[0] = (span, 0);
                let mut pending = 1;
                while pending != 0 {
                    pending -= 1;
                    let (span, depth) = stack[pending];
                    work.charge(1, cancel)?;
                    let other = self.other_collar_enclosure(index, span, work, cancel)?;
                    if (0..2).any(|axis| {
                        other[axis].hi < owned_tube[axis].lo || other[axis].lo > owned_tube[axis].hi
                    }) {
                        continue;
                    }
                    let mid = span[0] * 0.5 + span[1] * 0.5;
                    if depth >= MAX_DEPTH || mid <= span[0] || mid >= span[1] {
                        return Ok(None);
                    }
                    stack[pending] = ([span[0], mid], depth + 1);
                    stack[pending + 1] = ([mid, span[1]], depth + 1);
                    pending += 2;
                }
            }
        }
        // This interior anchor is a WHOLE strip, not an inverse point sample.
        // No other trim can occur between it and its owning original boundary.
        let inner = std::array::from_fn(|axis| {
            add(
                uv.position[axis],
                mul(Interval { lo: 0.5, hi: 1.0 }, offset[axis]),
            )
        });
        if self
            .domain
            .classify(uv_box(add_margin(inner, self.margin)), work, cancel)?
            != TrimClassification::Inside
        {
            return Ok(None);
        }
        self.domain.native().face().with_surface(|surface| {
            let boundary = surface_enclosure(
                surface,
                uv_box(add_margin(uv.position, self.margin)),
                work,
                cancel,
            )?;
            let patch = surface_enclosure(surface, uv_box(owned_tube), work, cancel)?;
            let boundary_t = compose_first(&boundary, uv.derivative);
            // Retained whole-forward error is an independent coefficient proof.
            // Intersect it with reevaluated component differences so subdivision
            // contracts derivatives without losing the original global bound.
            let displacement = std::array::from_fn(|axis| {
                intersection(
                    exact_difference(curve.position[axis], boundary.position[axis]),
                    Interval {
                        lo: -self.position_error_mm,
                        hi: self.position_error_mm,
                    },
                )
            });
            let [Some(dx), Some(dy), Some(dz)] = displacement else {
                return Ok(None);
            };
            let displacement = [dx, dy, dz];
            let displacement_t: [Interval; 3] = std::array::from_fn(|axis| {
                exact_difference(curve.derivative[axis], boundary_t[axis])
            });
            let cost = length(displacement)?.hi.min(self.position_error_mm);
            if !cost.is_finite() || cost > allowance {
                return Ok(None);
            }
            let at = std::array::from_fn(|axis| {
                add(
                    uv.derivative[axis],
                    mul(
                        Interval {
                            lo: 0.0,
                            hi: collar_width,
                        },
                        add(mul(beta_t, v[axis]), mul(beta, vt[axis])),
                    ),
                )
            });
            let aw = v.map(|value| mul(mul(width, beta), value));
            let ft = compose_first(&patch, at);
            let fw = compose_first(&patch, aw);
            // lambda ranges over the ENTIRE transport homotopy; no computed
            // near-zero dihedral is replaced by equality or convexity.
            let gt = std::array::from_fn(|axis| {
                add(
                    ft[axis],
                    mul(Interval { lo: 0.0, hi: 1.0 }, displacement_t[axis]),
                )
            });
            let gw = std::array::from_fn(|axis| {
                fw[axis].sub(mul(Interval { lo: 0.0, hi: 1.0 }, displacement[axis]))
            });
            let mut normal = cross(gt, gw);
            if !self.use_.orientation {
                normal = normal.map(Interval::neg);
            }
            let position = std::array::from_fn(|axis| {
                add(
                    patch.position[axis],
                    mul(Interval { lo: 0.0, hi: 1.0 }, displacement[axis]),
                )
            });
            if length(normal)?.lo <= 0.0 {
                return Ok(None);
            }
            Ok(Some(MaterialLinkCell {
                position,
                source_error_mm: cost,
                convex_link: false,
                outward: normal,
                second_outward: normal,
            }))
        })
    }
}

/// Paired original ownership and two complete source-only collar recipes.
/// A successful cell proves a two-sided local radial link, never a dihedral or
/// global convexity fact. The complete closed-boundary consumer separately
/// supplies endpoint fans, caps and degree-one global coverage.
pub(super) struct PairedMaterialLink<'a, 'd> {
    first: OriginalCollar<'a, 'd>,
    second: OriginalCollar<'a, 'd>,
    allowance: f64,
}
impl<'a, 'd> PairedMaterialLink<'a, 'd> {
    pub fn new(
        definition: &Definition,
        first_domain: &'a PreparedFaceTrimDomain<'d>,
        first: CoedgeUse,
        second_domain: &'a PreparedFaceTrimDomain<'d>,
        second: CoedgeUse,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, GeometryError> {
        work.charge(1, cancel)?;
        if first.orientation == second.orientation {
            return Ok(None);
        }
        let a = first_domain.original_coedge_correspondence(first.coedge)?;
        let b = second_domain.original_coedge_correspondence(second.coedge)?;
        let (Some(a_binding), Some(b_binding)) = (a.binding(), b.binding()) else {
            return Ok(None);
        };
        if a_binding.edge != b_binding.edge
            || a.native_range() != b.native_range()
            || a_binding.owner != [first.face, first.wire, first.index]
            || b_binding.owner != [second.face, second.wire, second.index]
        {
            return Ok(None);
        }
        let allowance = source_allowance(definition)?;
        let Some(first) = OriginalCollar::new(first_domain, first, work, cancel)? else {
            return Ok(None);
        };
        let Some(second) = OriginalCollar::new(second_domain, second, work, cancel)? else {
            return Ok(None);
        };
        Ok(Some(Self {
            first,
            second,
            allowance,
        }))
    }
    pub fn swept_enclosure(
        &self,
        range: [f64; 2],
        curve: &CurveEnclosure,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<[Interval; 3], GeometryError> {
        let a = self
            .first
            .swept_enclosure(range, curve, self.allowance, work, cancel)?;
        let b = self
            .second
            .swept_enclosure(range, curve, self.allowance, work, cancel)?;
        Ok(std::array::from_fn(|axis| a[axis].hull(b[axis])))
    }
    pub fn cell(
        &self,
        range: [f64; 2],
        curve: &CurveEnclosure,
        prism: &SamplePrism,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<MaterialLinkCell>, GeometryError> {
        let Some(first) = self
            .first
            .cell(range, curve, prism, self.allowance, work, cancel)?
        else {
            return Ok(None);
        };
        let Some(second) = self
            .second
            .cell(range, curve, prism, self.allowance, work, cancel)?
        else {
            return Ok(None);
        };
        let tangent = if self.first.use_.orientation {
            curve.derivative
        } else {
            curve.derivative.map(Interval::neg)
        };
        let convex_link = first.convex_link
            && second.convex_link
            && dot(second.outward, cross(first.outward, tangent)).hi <= 0.0;
        Ok(Some(MaterialLinkCell {
            position: std::array::from_fn(|axis| first.position[axis].hull(second.position[axis])),
            source_error_mm: first.source_error_mm.max(second.source_error_mm),
            convex_link,
            outward: first.outward,
            second_outward: second.outward,
        }))
    }
}

#[cfg(test)]
mod scalar_law_tests {
    use super::*;

    fn pocket_floor() -> (Definition, usize) {
        let (_, solid, _) = crate::parity_fixtures::recipes()
            .into_iter()
            .find(|(name, _, _)| *name == "native-parity-blind-pocket.step")
            .expect("original native pocket recipe");
        let source = crate::parity_fixtures::original("material-floor-scalar-law.step", solid);
        let definition = crate::import_step(source.as_bytes(), &AtomicBool::new(false))
            .expect("unchanged original coefficients");
        let mut work = GeometryWorkBudget::new(20_000_000);
        let functional = HeightFunctional {
            direction: [0.0, 0.0, 1.0].map(Interval::point),
            offset: Interval::point(0.0),
        };
        let floor = (0..definition.solid.face_iter().count())
            .find(|&face| {
                super::super::super::slab::constant_source_face_height(
                    &definition,
                    face,
                    functional,
                    &mut work,
                    &AtomicBool::new(false),
                )
                .unwrap()
                .is_some_and(|height| height.lo == 4.0 && height.hi == 4.0)
            })
            .expect("coefficient-backed interior floor");
        (definition, floor)
    }

    #[test]
    fn actual_generic_transport_floor_preserves_complete_exact_scalar_law() {
        let (definition, floor) = pocket_floor();
        let functional = HeightFunctional {
            direction: [0.0, 0.0, 1.0].map(Interval::point),
            offset: Interval::point(0.0),
        };
        let mut work = GeometryWorkBudget::new(20_000_000);
        assert!(
            !definition
                .material_transport_is_proved_zero(&mut work, &AtomicBool::new(false),)
                .unwrap()
        );
        assert_eq!(
            constant_material_face_height(
                &definition,
                floor,
                functional,
                &mut work,
                &AtomicBool::new(false),
            )
            .unwrap(),
            Some(4.0)
        );
    }

    #[test]
    fn complete_material_scalar_law_rejects_sub_roundoff_functional_tilt() {
        let (definition, floor) = pocket_floor();
        let functional = HeightFunctional {
            direction: [1.0e-18, 0.0, 1.0].map(Interval::point),
            offset: Interval::point(0.0),
        };
        let mut work = GeometryWorkBudget::new(20_000_000);
        assert_eq!(
            constant_material_face_height(
                &definition,
                floor,
                functional,
                &mut work,
                &AtomicBool::new(false),
            )
            .unwrap(),
            None,
            "rounded point heights cannot establish a scalar identity"
        );
    }
}
