// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

mod cylinder;
mod wall;

use monstertruck_modeling::Solid;
use spiling_contracts::geometry::{AabbMm, DefinitionId, FaceInfo, SourceProvenance};

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
}

/// Positional coedge associations retained before analytic STEP curves become
/// modeled NURBS. IDs borrow identity, not a second copy of native geometry.
pub(crate) struct FaceBoundaryRefs {
    pub(crate) face: monstertruck_topology::FaceId<monstertruck_modeling::Surface>,
    pub(crate) wires: Vec<Vec<BoundaryCoedgeRef>>,
}

pub(crate) struct BoundaryCoedgeRef {
    pub(crate) witness: usize,
    pub(crate) orientation: bool,
    pub(crate) edge: monstertruck_topology::EdgeId<monstertruck_modeling::Curve>,
}

/// Exact line/circle arc witnesses of the admitted native boundary, in millimetres.
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
}

impl BoundaryCurveMm {
    pub(crate) fn endpoints(&self) -> ([f64; 3], [f64; 3]) {
        match self {
            Self::Line { start_mm, end_mm } => (*start_mm, *end_mm),
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
