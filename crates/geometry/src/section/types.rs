// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use crate::{GeometryError, PlaneFrameMm, PlaneMm};
use solidcore::topology::EdgeuseId;
use solidcore::{EdgeId, FaceId, VertexId};
use spiling_contracts::geometry::{DefinitionId, GeometryErrorCode, SECTION_SAMPLING_TOLERANCE_MM};

#[derive(Debug)]
pub struct SectionLoopMm {
    pub is_hole: bool,
    pub points_mm: Vec<[f64; 3]>,
}

/// Identity, not a proximity bucket. Roots are ordered in the native 3D edge's
/// parameter direction; a vertex is never guessed from a nearby coordinate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SectionEndpoint {
    EdgeRoot { edge: EdgeId, root: u32 },
    Vertex(VertexId),
}

/// The exact curve is the zero set of the section plane functional on the
/// original native surface, restricted to this uniquely solved graph chart.
/// The independent/dependent intervals are enclosures, not rounded exact trim
/// limits. Exact limits are the two directed roots of `trim_edgeuses` below.
/// This is neither a fitted spline nor a sampled intersection curve.
#[derive(Debug, Clone, Copy)]
pub enum SectionCurve {
    SurfacePlaneGraph {
        independent_axis: u8,
        independent: [f64; 2],
        dependent: [f64; 2],
    },
}
#[derive(Debug, Clone)]
pub struct SectionSpan {
    pub face: FaceId,
    pub source_face_ordinal: usize,
    /// Immutable plane in the definition's native coefficient coordinates.
    pub definition_plane: PlaneMm,
    /// Placement of this native expression in the returned section frame.
    pub section_pose: spiling_contracts::geometry::RigidPoseMm,
    pub curve: SectionCurve,
    pub endpoints: [SectionEndpoint; 2],
    /// Ordered in increasing independent parameter, regardless of `reversed`.
    pub trim_edgeuses: [EdgeuseId; 2],
    /// Root isolators in the native edgeuse normalized parameter law [0,1].
    pub trim_parameters: [[f64; 2]; 2],
    pub native_edge_parameters: [[f64; 2]; 2],
    pub reversed: bool,
    pub maximum_chord_error_mm: f64,
    pub maximum_plane_error_mm: f64,
}
#[derive(Debug)]
pub struct NativeSection {
    pub(crate) plane: PlaneMm,
    pub(crate) frame: PlaneFrameMm,
    pub(crate) loops: Vec<SectionLoopMm>,
    pub(crate) loop_source_faces: Vec<Vec<usize>>,
    pub(crate) loop_spans: Vec<Vec<SectionSpan>>,
    pub(crate) sampling_tolerance_mm: f64,
    pub(crate) boolean_tolerance_mm: f64,
    pub(crate) certificate: NativeSectionCertificate,
}
#[derive(Debug, Clone)]
pub(crate) struct NativeSectionCertificate {
    pub(crate) definition_id: DefinitionId,
    pub(crate) authority: SectionAuthority,
    pub(crate) maximum_chord_error_mm: f64,
    pub(crate) maximum_plane_error_mm: f64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SectionAuthority {
    NativeIntersection,
    ConservativeSlabEnvelope,
}
impl NativeSection {
    pub fn plane(&self) -> PlaneMm {
        self.plane
    }
    pub fn frame(&self) -> PlaneFrameMm {
        self.frame
    }
    pub fn loops(&self) -> &[SectionLoopMm] {
        &self.loops
    }
    pub fn spans(&self, loop_index: usize) -> Option<&[SectionSpan]> {
        self.loop_spans.get(loop_index).map(Vec::as_slice)
    }
    pub fn sampling_tolerance_mm(&self) -> f64 {
        self.sampling_tolerance_mm
    }
    pub fn boolean_tolerance_mm(&self) -> f64 {
        self.boolean_tolerance_mm
    }
    pub fn into_loops(self) -> Vec<SectionLoopMm> {
        self.loops
    }
    pub fn definition_id(&self) -> &DefinitionId {
        &self.certificate.definition_id
    }
    pub fn is_native_intersection(&self) -> bool {
        self.certificate.authority == SectionAuthority::NativeIntersection
    }
    pub fn achieved_chord_error_mm(&self) -> f64 {
        self.certificate.maximum_chord_error_mm
    }
    pub fn achieved_plane_error_mm(&self) -> f64 {
        self.certificate.maximum_plane_error_mm
    }
    pub(crate) fn loop_source_face_ordinals(&self, index: usize) -> Option<&[usize]> {
        if !self.is_native_intersection() || self.loop_source_faces.len() != self.loops.len() {
            return None;
        }
        self.loop_source_faces
            .get(index)
            .filter(|faces| !faces.is_empty())
            .map(Vec::as_slice)
    }
    pub(crate) fn charge_reference_displacement(
        &mut self,
        displacement_mm: f64,
    ) -> Result<(), GeometryError> {
        let total = (self.achieved_chord_error_mm() + displacement_mm).next_up();
        if !self.is_native_intersection()
            || !displacement_mm.is_finite()
            || displacement_mm < 0.0
            || total > SECTION_SAMPLING_TOLERANCE_MM
        {
            return Err(GeometryError::new(
                GeometryErrorCode::AnalysisIndeterminate,
                "reference displacement exceeds native curve authority",
            ));
        }
        self.certificate.maximum_chord_error_mm = total;
        Ok(())
    }
    pub fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.certificate.definition_id.allocated_bytes()
            + self.loops.capacity() * std::mem::size_of::<SectionLoopMm>()
            + self
                .loops
                .iter()
                .map(|l| l.points_mm.capacity() * std::mem::size_of::<[f64; 3]>())
                .sum::<usize>()
            + self.loop_source_faces.capacity() * std::mem::size_of::<Vec<usize>>()
            + self
                .loop_source_faces
                .iter()
                .map(|f| f.capacity() * std::mem::size_of::<usize>())
                .sum::<usize>()
            + self.loop_spans.capacity() * std::mem::size_of::<Vec<SectionSpan>>()
            + self
                .loop_spans
                .iter()
                .map(|s| s.capacity() * std::mem::size_of::<SectionSpan>())
                .sum::<usize>()
    }
}
