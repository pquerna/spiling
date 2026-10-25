// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Original STEP operands and use genealogy, separate from generated native trims.
//! Candidate associations are not correspondence or material certificates.

use solidcore::FaceId;
use solidcraft_step::geom::{Curve, Pcurve, Surface};
use spiling_contracts::geometry::{GeometryError, GeometryErrorCode, StepEntityId};
use std::sync::Arc;

#[derive(Debug)]
pub(crate) struct OriginalSourceTopology {
    pub(crate) faces: Vec<OriginalFaceSource>,
    pub(crate) coedges: Vec<OriginalCoedgeSource>,
}

#[derive(Debug)]
pub(crate) struct OriginalFaceSource {
    pub(crate) native_face: FaceId,
    /// Canonical source shell face use, including an ORIENTED_FACE wrapper.
    pub(crate) source_face: StepEntityId,
    /// Resolved original ADVANCED_FACE/FACE_SURFACE entity.
    pub(crate) face_entity: StepEntityId,
    pub(crate) carrier_id: StepEntityId,
    /// Literal original parameters: lengths in mm, angles in radians.
    pub(crate) carrier: Arc<Surface>,
    pub(crate) same_sense: bool,
    /// Orientation of the canonical shell face use relative to ADVANCED_FACE.
    pub(crate) face_use_forward: bool,
    pub(crate) wires: Vec<OriginalWireSource>,
}

impl OriginalFaceSource {
    /// Literal source-use sense; this metadata grants no geometric authority.
    pub(crate) fn material_orientation(&self) -> bool {
        self.same_sense == self.face_use_forward
    }
}

#[derive(Debug)]
pub(crate) struct OriginalWireSource {
    pub(crate) bound_id: StepEntityId,
    pub(crate) loop_id: StepEntityId,
    /// Explicit FACE_OUTER_BOUND marker. Generic FACE_BOUND is not a hole proof.
    pub(crate) declared_outer: bool,
    pub(crate) forward: bool,
    /// Original EDGE_LOOP order, without tolerance-driven repair or reordering.
    pub(crate) coedges: Vec<usize>,
    /// Literal collapsed loop, retaining the original vertex identity and mm point.
    pub(crate) vertex_loop: Option<(StepEntityId, [f64; 3])>,
}

#[derive(Debug)]
pub(crate) struct OriginalCoedgeSource {
    pub(crate) source_face: StepEntityId,
    pub(crate) bound_id: StepEntityId,
    pub(crate) loop_id: StepEntityId,
    pub(crate) oriented_edge_id: StepEntityId,
    pub(crate) edge_curve_id: StepEntityId,
    pub(crate) curve_id: StepEntityId,
    pub(crate) vertex_ids: [StepEntityId; 2],
    pub(crate) vertex_points_mm: [[f64; 3]; 2],
    /// ORIENTED_EDGE traversal relative to the original EDGE_CURVE.
    pub(crate) forward: bool,
    /// EDGE_CURVE's original relationship to its declared geometric curve.
    pub(crate) same_sense: bool,
    /// Includes literal TRIMMED_CURVE selectors and SURFACE_CURVE preferred master.
    pub(crate) spatial_curve: Arc<Curve>,
    pub(crate) pcurves: Vec<OriginalPcurveSource>,
}

#[derive(Debug)]
pub(crate) struct OriginalPcurveSource {
    pub(crate) source_pcurve: StepEntityId,
    pub(crate) carrier_id: StepEntityId,
    /// Index in original associated_geometry, not a fitted branch selection.
    pub(crate) association_index: usize,
    pub(crate) operands: Pcurve,
}

impl OriginalSourceTopology {
    pub(crate) fn face(&self, native: FaceId) -> Result<&OriginalFaceSource, GeometryError> {
        self.faces
            .iter()
            .find(|face| face.native_face == native)
            .ok_or_else(|| {
                GeometryError::new(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    "native face has no original source operand binding",
                )
            })
    }

    /// All original coedge candidates of this exact source-bound native face.
    /// Geometric/gauge and shared-endpoint correspondence must still be proved.
    pub(crate) fn candidates(
        &self,
        native_face: FaceId,
    ) -> impl Iterator<Item = (usize, &OriginalCoedgeSource)> {
        self.faces
            .iter()
            .filter(move |face| face.native_face == native_face)
            .flat_map(|face| &face.wires)
            .flat_map(|wire| &wire.coedges)
            .filter_map(|&index| self.coedges.get(index).map(|coedge| (index, coedge)))
    }

    pub(crate) fn coedge(&self, index: usize) -> Result<&OriginalCoedgeSource, GeometryError> {
        self.coedges.get(index).ok_or_else(|| {
            GeometryError::new(
                GeometryErrorCode::InvalidGeometry,
                "original source coedge index is outside its captured census",
            )
        })
    }
}
