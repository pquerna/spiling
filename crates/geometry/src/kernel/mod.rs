// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

mod cylinder;
mod trim;
mod wall;
pub(crate) use trim::{
    PreparedFaceTrimDomain, TrimClassification, certified_curve2d_is_straight_regular,
    prepare_face_trim_domain_with_byte_limit,
};

use monstertruck_io::step::load::step_geometry::{CurveParameterMap, SurfaceParameterMap};
use monstertruck_modeling::Solid;
use spiling_contracts::geometry::{AabbMm, DefinitionId, FaceInfo, SourceProvenance};

/// Supporting extrema for narrow finish certificates use the same native
/// all-family proof as other consumers, with the caller's cumulative work.
fn finish_support_extent(
    definition: &Definition,
    direction: [f64; 3],
    work: &mut crate::GeometryWorkBudget,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<[f64; 2], crate::GeometryError> {
    use crate::parametric::Interval;
    let magnitude = direction
        .into_iter()
        .fold(Interval::point(0.0), |sum, value| {
            sum.add(Interval::point(value).square())
        })
        .sqrt()?;
    let proof = crate::projected_bounds(
        definition,
        direction,
        crate::BoundsPolicy {
            maximum_endpoint_uncertainty_mm: 1.0e-8 / magnitude.hi,
        },
        work,
        cancel,
    )?;
    let minimum = Interval {
        lo: proof.minimum[0],
        hi: proof.minimum[1],
    }
    .mul(magnitude);
    let maximum = Interval {
        lo: proof.maximum[0],
        hi: proof.maximum[1],
    }
    .mul(magnitude);
    if minimum.width() > 2.5e-8 || maximum.width() > 2.5e-8 {
        return Err(crate::GeometryError::new(
            spiling_contracts::geometry::GeometryErrorCode::ConditioningFailure,
            "native finish support cannot meet its fixed residual under coordinate conditioning",
        ));
    }
    Ok([minimum.lo, maximum.hi])
}

/// Immutable native definition. Kernel representation never leaves this crate.
pub struct Definition {
    pub(crate) solid: Solid,
    pub(crate) id: DefinitionId,
    pub(crate) provenance: SourceProvenance,
    pub(crate) faces: Vec<FaceInfo>,
    pub(crate) bounds: AabbMm,
    pub(crate) source_bytes: u32,
    pub(crate) boundary_curves: Vec<BoundaryCurveMm>,
    pub(crate) face_boundaries: Vec<FaceBoundaryRefs>,
    /// Exact decoded-source/native maps in native edge order. Original STEP
    /// selectors and genealogy live in source_topology; provenance owns mm scale.
    pub(crate) boundary_parameters: Vec<Option<CurveParameterMap>>,
    /// Original implicit source branches, shared by all oriented uses and
    /// occurrences. These are not explicit Identity parameter maps.
    pub(crate) boundary_continuations: Vec<Option<OriginalBoundaryContinuation>>,
    pub(crate) original_continuations: Vec<monstertruck_modeling::PreparedValidatedCurve<'static>>,
    pub(crate) continuation_bytes: usize,
    pub(crate) boundary_curve_indices: std::collections::HashMap<
        monstertruck_topology::EdgeId<monstertruck_modeling::Curve>,
        usize,
    >,
    pub(crate) source_topology: monstertruck_io::step::load::SourceShellProvenance,
    pub(crate) trimmed_solid: monstertruck_topology::trimmed::TrimmedSolid<
        monstertruck_modeling::Point3,
        monstertruck_modeling::Curve,
        monstertruck_modeling::Surface,
        monstertruck_modeling::Curve2D,
    >,
}

pub(crate) struct OriginalBoundaryContinuation {
    pub(crate) prepared: usize,
    pub(crate) source_range: (f64, f64),
    pub(crate) original_range: (f64, f64),
    pub(crate) source_edge: Option<u64>,
    pub(crate) raw_step_gauge: monstertruck_io::step::load::step_geometry::SourceParameterGauge,
    pub(crate) source_domain: monstertruck_io::step::load::step_geometry::CurveParameterDomain,
    pub(crate) scale_to_mm: f64,
    pub(crate) edge: Option<monstertruck_topology::EdgeId<monstertruck_modeling::Curve>>,
}

/// Positional trim uses borrow native geometry. Their witness indices also
/// select the shared source/native curve maps; no second coedge index is needed.
pub(crate) struct FaceBoundaryRefs {
    pub(crate) face: monstertruck_topology::FaceId<monstertruck_modeling::Surface>,
    pub(crate) wires: Vec<Vec<BoundaryCoedgeRef>>,
    pub(crate) parameter: FaceParameterWitness,
}

/// Whole-source covering-chart authority. A pole graph does not prove a
/// regular longitude inverse; its physical and UV bounds remain on the coedge.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SourceChartCertificate {
    Unproved,
    InverseOrExact,
    PoleMeridianGraph {
        latitude_axis: usize,
        pole_endpoints: [bool; 2],
    },
}

impl SourceChartCertificate {
    pub(crate) fn is_certified(self) -> bool {
        !matches!(self, Self::Unproved)
    }

    pub(crate) fn has_regular_inverse(self) -> bool {
        matches!(self, Self::InverseOrExact)
    }
}

pub(crate) struct BoundaryCoedgeRef {
    pub(crate) witness: usize,
    pub(crate) orientation: bool,
    pub(crate) edge: monstertruck_topology::EdgeId<monstertruck_modeling::Curve>,
    /// Whole-coedge source-chart uncertainty, not a guessed glue tolerance.
    pub(crate) source_chart_margin_uv: [f64; 2],
    pub(crate) source_chart_error_mm: f64,
    /// UV trim parameters are independent of rational native edge parameters.
    pub(crate) source_chart_parameter_range: Option<(f64, f64)>,
    /// An unproved original declared trim remains retained without authorizing
    /// operations that need a proved source/native covering chart.
    pub(crate) source_chart_certificate: SourceChartCertificate,
    /// Retained original source PCURVE/literal parameter expression. A derived
    /// inverse or small physical residual alone never supplies this authority.
    pub(crate) source_chart_declared: bool,
}

pub(crate) struct FaceParameterWitness {
    /// Source UV and native UV remain distinct even after physical mm scaling.
    pub(crate) conversion: SurfaceParameterMap,
    /// Original bounded spline carrier domain, transported by exact conversion.
    /// A lowered sweep's incidental zero-to-one net is not this witness.
    pub(crate) native_uv_domain: Option<[[f64; 2]; 2]>,
}
/// Exact analytic fast paths plus native endpoint witnesses for general curves.
/// `Native` is not a line chord and never authorizes analytic extrema/finishes.
#[derive(Debug, Clone)]
pub(crate) enum BoundaryCurveMm {
    Line {
        start_mm: [f64; 3],
        end_mm: [f64; 3],
    },
    CircleArc {
        center_mm: [f64; 3],
        u_mm: [f64; 3],
        v_mm: [f64; 3],
        start_angle: f64,
        end_angle: f64,
    },
    Native {
        start_mm: [f64; 3],
        end_mm: [f64; 3],
    },
}

impl BoundaryCurveMm {
    pub(crate) fn endpoints(&self) -> ([f64; 3], [f64; 3]) {
        match self {
            Self::Line { start_mm, end_mm } | Self::Native { start_mm, end_mm } => {
                (*start_mm, *end_mm)
            }
            Self::CircleArc {
                center_mm,
                u_mm,
                v_mm,
                start_angle,
                end_angle,
            } => {
                let point = |angle: f64| {
                    std::array::from_fn(|i| {
                        center_mm[i] + angle.cos() * u_mm[i] + angle.sin() * v_mm[i]
                    })
                };
                (point(*start_angle), point(*end_angle))
            }
        }
    }
}

impl Definition {
    pub(crate) fn prepare_boundary_curve<'a>(
        &'a self,
        edge: monstertruck_topology::EdgeId<monstertruck_modeling::Curve>,
        curve: &'a monstertruck_modeling::Curve,
        range: (f64, f64),
        remaining_bytes: usize,
        work: &mut crate::GeometryWorkBudget,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<crate::parametric::PreparedCurveEnclosure<'a>, crate::GeometryError> {
        if let Some(prepared) = self.original_boundary_curve(edge, range, work, cancel)? {
            return Ok(prepared);
        }
        crate::parametric::prepare_curve_enclosure(curve, range, remaining_bytes, work, cancel)
    }
    pub fn id(&self) -> &DefinitionId {
        &self.id
    }
    pub fn provenance(&self) -> &SourceProvenance {
        &self.provenance
    }
    pub fn faces(&self) -> &[FaceInfo] {
        &self.faces
    }
    pub fn bounds_mm(&self) -> AabbMm {
        self.bounds
    }
    pub fn source_bytes(&self) -> u32 {
        self.source_bytes
    }
    /// Pinned native query-proof capacities, counted once per definition.
    /// Includes actual source/proof/positional storage and the conservative
    /// capacity-derived native edge-index table bound, not the BREP or RSS.
    pub fn retained_query_proof_bytes(&self) -> usize {
        self.continuation_bytes
    }
}

impl std::fmt::Debug for Definition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Definition")
            .field("id", &self.id)
            .field("face_count", &self.faces.len())
            .field("bounds_mm", &self.bounds)
            .finish_non_exhaustive()
    }
}
