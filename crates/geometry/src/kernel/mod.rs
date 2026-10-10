// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

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
