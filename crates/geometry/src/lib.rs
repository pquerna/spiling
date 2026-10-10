// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Native geometry admission and inspection, independent of sessions and applications.

mod import;
mod kernel;
mod section;
mod tessellation;

#[cfg(test)]
mod tests;

pub use kernel::Definition;
pub use spiling_contracts::display::EncodedMesh as MeshArtifact;
pub use spiling_contracts::display::MeshBudget;
use spiling_contracts::geometry::GeometryErrorCode;
pub use spiling_contracts::geometry::{
    DisplayProfile, FaceId, FaceInfo, GeometryError, PlaneFrameMm, PlaneMm,
};
use std::sync::atomic::{AtomicBool, Ordering};

/// One native section boundary, closed and canonically ordered in the plane frame.
#[derive(Debug)]
pub struct SectionLoopMm {
    pub is_hole: bool,
    pub points_mm: Vec<[f64; 3]>,
}

/// Owned native section; loop identities are local to this result, never source faces.
#[derive(Debug)]
pub struct NativeSection {
    pub plane: PlaneMm,
    pub frame: PlaneFrameMm,
    pub loops: Vec<SectionLoopMm>,
    pub sampling_tolerance_mm: f64,
    pub boolean_tolerance_mm: f64,
}

pub fn import_step(bytes: &[u8], cancel: &AtomicBool) -> Result<Definition, GeometryError> {
    import_step_with_face_limit(bytes, cancel, spiling_contracts::geometry::MAX_NATIVE_FACES)
}

/// Admit within the engine's remaining live/staged native-face capacity.
pub fn import_step_with_face_limit(
    bytes: &[u8],
    cancel: &AtomicBool,
    face_limit: u32,
) -> Result<Definition, GeometryError> {
    import::import_step(bytes, cancel, face_limit)
}

pub fn tessellate(
    definition: &Definition,
    profile: DisplayProfile,
    cancel: &AtomicBool,
) -> Result<MeshArtifact, GeometryError> {
    tessellate_with_budget(definition, profile, cancel, MeshBudget::FROZEN)
}

/// Pack within remaining scene capacity; full-cap standalone consumers use `tessellate`.
pub fn tessellate_with_budget(
    definition: &Definition,
    profile: DisplayProfile,
    cancel: &AtomicBool,
    budget: MeshBudget,
) -> Result<MeshArtifact, GeometryError> {
    tessellation::tessellate(definition, profile, cancel, budget)
}

pub fn section(
    definition: &Definition,
    plane_in_definition_mm: PlaneMm,
    cancel: &AtomicBool,
) -> Result<NativeSection, GeometryError> {
    section_with_byte_limit(
        definition,
        plane_in_definition_mm,
        cancel,
        spiling_contracts::geometry::MAX_SECTION_BYTES,
    )
}

/// Bound owned sampled native loops before growth, including SPLS-equivalent overhead.
pub fn section_with_byte_limit(
    definition: &Definition,
    plane_in_definition_mm: PlaneMm,
    cancel: &AtomicBool,
    byte_limit: u32,
) -> Result<NativeSection, GeometryError> {
    section::section(definition, plane_in_definition_mm, cancel, byte_limit)
}

pub fn inspect_face(definition: &Definition, face: &FaceId) -> Result<FaceInfo, GeometryError> {
    definition
        .faces
        .iter()
        .find(|info| &info.face_id == face)
        .cloned()
        .ok_or_else(|| {
            GeometryError::new(
                GeometryErrorCode::UnknownHandle,
                "source face is not in this definition",
            )
        })
}

pub(crate) fn check_cancel(cancel: &AtomicBool) -> Result<(), GeometryError> {
    if cancel.load(Ordering::Acquire) {
        Err(GeometryError::new(
            GeometryErrorCode::Cancelled,
            "geometry operation cancelled",
        ))
    } else {
        Ok(())
    }
}
