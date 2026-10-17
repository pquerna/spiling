// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Compact completed authority. Only the complete original F/G atlas and the
//! identical physical all-face cylinder census may construct this receipt.

use super::*;
use crate::analysis::placed::{ActualMaterialCoordinates, MaterialCoordinatesCapture};
use sha2::{Digest, Sha256};
use spiling_contracts::geometry::RigidPoseMm;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::analysis) enum RadialRegionKind {
    Disk,
    Annulus,
}

/// A failed boundary theorem selects only a refinement goal, never authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::analysis) enum BoundaryVisit {
    Accepted,
    RefineSpatial,
    RefineActualXy,
}

/// Raw F retains its original carrier and UV box for correlated scalar bounds.
/// Swept G/link images deliberately carry no replacement carrier.
#[derive(Clone, Copy)]
pub(in crate::analysis) struct BoundaryCell<'a> {
    pub position: [Interval; 3],
    pub original_face: Option<(&'a Surface, [[f64; 2]; 2])>,
}

#[derive(Debug, Clone, Copy)]
pub(in crate::analysis) struct ClosedRadialMaterialProduct {
    coordinates: MaterialCoordinatesCapture,
    kind: RadialRegionKind,
    source_sha256: [u8; 32],
    identity_sha256: [u8; 32],
}

fn source_identity(definition: &Definition) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"spiling-closed-radial-material-source-1\0");
    hash.update(definition.id().as_str().as_bytes());
    hash.update(crate::KERNEL_REVISION.as_bytes());
    hash.update(crate::kernel::MaterialRealizationIdentity::new(definition).identity_bytes());
    hash.finalize().into()
}

impl ClosedRadialMaterialProduct {
    pub(in crate::analysis) fn coordinates(&self) -> &MaterialCoordinatesCapture {
        &self.coordinates
    }

    pub(in crate::analysis) fn kind(&self) -> RadialRegionKind { self.kind }

    pub(in crate::analysis) fn identity_sha256(&self) -> [u8; 32] { self.identity_sha256 }

    pub(in crate::analysis) fn retained_bytes(&self) -> usize { std::mem::size_of::<Self>() }

    pub(in crate::analysis) fn matches_world(
        &self,
        definition: &Definition,
        pose: RigidPoseMm,
        interval: [f64; 2],
        sample: f64,
    ) -> bool {
        self.source_sha256 == source_identity(definition)
            && self.coordinates.matches_world(definition, pose, interval, sample)
    }
}

pub(in crate::analysis) fn certify_closed_radial_material_product(
    source: OriginalTrimSource<'_, '_>,
    request: &SlabRequest,
    coordinates: &ActualMaterialCoordinates<'_>,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Option<ClosedRadialMaterialProduct>, GeometryError> {
    certify_closed_radial_material_product_with_boundary_visitor(
        source, request, coordinates, remaining_bytes, work, cancel,
        &mut |_: BoundaryCell<'_>, _, _, _, _| Ok(BoundaryVisit::Accepted),
    )
}

/// The visitor may impose an independent stricter boundary theorem. Its
/// refinement goal schedules subdivision; success replaces no atlas/census premise.
/// Boxes conservatively cover original F and all-lambda actual G, not an exact
/// partition. Only cells meeting the closed scope reach the visitor.
/// The third argument is `can_refine`: false means this exact active cell has
/// reached the frozen depth cap or has no representable subdivision. A metric
/// consumer may preserve its own terminal ShapeErrorExceeded without changing
/// an unrelated atlas refusal into a metric diagnosis.
#[allow(clippy::too_many_arguments)]
pub(in crate::analysis) fn certify_closed_radial_material_product_with_boundary_visitor(
    source: OriginalTrimSource<'_, '_>,
    request: &SlabRequest,
    coordinates: &ActualMaterialCoordinates<'_>,
    remaining_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    visitor: &mut impl for<'s> FnMut(
        BoundaryCell<'s>, bool, bool, &mut GeometryWorkBudget, &AtomicBool,
    ) -> Result<BoundaryVisit, GeometryError>,
) -> Result<Option<ClosedRadialMaterialProduct>, GeometryError> {
    work.charge(1, cancel)?;
    let available = remaining_bytes.min(MAX_SCRATCH_BYTES)
        .checked_sub(std::mem::size_of::<ClosedRadialMaterialProduct>())
        .ok_or_else(|| GeometryError::new(GeometryErrorCode::ResourceLimit,
            "closed radial product metadata exceeds remaining byte allowance"))?;
    let Some(kind) = atlas::certify(source, request, coordinates, available, work, cancel, visitor)?
        else { return Ok(None); };
    // No constructor runs until every closed-atlas and calibrated material
    // premise holds. The hash identifies that authority; it cannot create it.
    let capture = coordinates.capture();
    let source_sha256 = source_identity(source.definition());
    let mut hash = Sha256::new();
    hash.update(b"spiling-closed-radial-material-product-1\0");
    hash.update(source_sha256);
    hash.update(capture.axis_sha256());
    hash.update([match kind { RadialRegionKind::Disk => 0, RadialRegionKind::Annulus => 1 }]);
    for value in capture.world_center_enclosure().into_iter().flatten()
        .chain(capture.world_interval()).chain(std::iter::once(capture.sample_world_z()))
    {
        hash.update(value.to_bits().to_le_bytes());
    }
    Ok(Some(ClosedRadialMaterialProduct {
        coordinates: capture, kind, source_sha256, identity_sha256: hash.finalize().into(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn original_box() -> Definition {
        let bytes = std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/geometry/box-mm.step")).unwrap();
        crate::import_step(&bytes, &AtomicBool::new(false)).unwrap()
    }

    fn request(definition: &Definition) -> SlabRequest {
        let bounds = definition.bounds_mm();
        let z = bounds.max[2] - bounds.min[2];
        SlabRequest {
            frame: PlaneFrameMm {
                origin_mm: [0.0; 3],
                x_axis: [1.0, 0.0, 0.0],
                y_axis: [0.0, 1.0, 0.0],
                z_axis: [0.0, 0.0, 1.0],
            },
            height_interval_mm: [bounds.min[2] + 0.25 * z, bounds.min[2] + 0.75 * z],
            sample_height_mm: bounds.min[2] + 0.37 * z,
            native_shape_error_mm: 0.1,
        }
    }

    #[test]
    fn complete_original_box_product_is_not_source_capture_world_authority() {
        let definition = original_box();
        let request = request(&definition);
        let cancel = AtomicBool::new(false);
        let rows = ActualMaterialCoordinates::source_rows(&request.frame).unwrap();
        let mut work = GeometryWorkBudget::new(20_000_000);
        let coordinates = ActualMaterialCoordinates::source(&definition, &request.frame, &rows,
            request.height_interval_mm, request.sample_height_mm, &mut work, &cancel).unwrap();
        let product = certify_closed_radial_material_product(
            OriginalTrimSource::standalone(&definition), &request, &coordinates,
            MAX_SCRATCH_BYTES, &mut work, &cancel,
        ).unwrap().expect("complete central box walls form a closed disk product");
        assert_eq!(product.kind(), RadialRegionKind::Disk);
        assert!(product.coordinates().matches_source(&definition, &request.frame,
            request.height_interval_mm, request.sample_height_mm));
        assert!(!product.matches_world(&definition, RigidPoseMm::IDENTITY,
            request.height_interval_mm, request.sample_height_mm));
        assert!(!product.coordinates().matches_source(&definition, &request.frame,
            request.height_interval_mm, up(request.sample_height_mm)));
    }

    #[test]
    fn product_refuses_closed_vertex_cut_and_propagates_terminal_errors() {
        let definition = original_box();
        let mut request = request(&definition);
        request.height_interval_mm[0] = definition.bounds_mm().min[2];
        let rows = ActualMaterialCoordinates::source_rows(&request.frame).unwrap();
        let cancel = AtomicBool::new(false);
        let mut work = GeometryWorkBudget::new(20_000_000);
        let coordinates = ActualMaterialCoordinates::source(&definition, &request.frame, &rows,
            request.height_interval_mm, request.sample_height_mm, &mut work, &cancel).unwrap();
        let source = OriginalTrimSource::standalone(&definition);
        assert!(certify_closed_radial_material_product(source, &request, &coordinates,
            MAX_SCRATCH_BYTES, &mut work, &cancel).unwrap().is_none());
        let error = certify_closed_radial_material_product(source, &request, &coordinates,
            0, &mut work, &cancel).unwrap_err();
        assert_eq!(error.code, GeometryErrorCode::ResourceLimit);
        let error = certify_closed_radial_material_product(source, &request, &coordinates,
            MAX_SCRATCH_BYTES, &mut work, &AtomicBool::new(true)).unwrap_err();
        assert_eq!(error.code, GeometryErrorCode::Cancelled);
    }
}
