// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::required;
use crate::{
    geometry::SourceHash,
    manufacturing::*,
    project::{ProjectId, ProjectRevision},
    rpc,
};

impl TryFrom<rpc::VerificationOutput> for VerificationOutput {
    type Error = String;
    fn try_from(value: rpc::VerificationOutput) -> Result<Self, String> {
        match value {
            rpc::VerificationOutput::ReportOnly => Ok(Self::ReportOnly),
            rpc::VerificationOutput::ReportAndMotion => Ok(Self::ReportAndMotion),
            _ => Err("verification output must be explicitly selected".into()),
        }
    }
}
impl TryFrom<VerificationOutput> for rpc::VerificationOutput {
    type Error = String;
    fn try_from(value: VerificationOutput) -> Result<Self, String> {
        Ok(match value {
            VerificationOutput::ReportOnly => Self::ReportOnly,
            VerificationOutput::ReportAndMotion => Self::ReportAndMotion,
        })
    }
}
impl TryFrom<rpc::ManufacturingInspectionProvenance> for ManufacturingInspectionProvenance {
    type Error = String;
    fn try_from(v: rpc::ManufacturingInspectionProvenance) -> Result<Self, String> {
        let value = Self {
            project_id: ProjectId::parse(v.project_id).map_err(|e| e.to_string())?,
            input_revision: ProjectRevision(v.input_revision),
            input_hash: SourceHash::parse(v.input_hash).map_err(|e| e.to_string())?,
            surface_overlays: required(v.surface_overlays, "inspection surface overlays")?
                .try_into()?,
            kernel: required(v.kernel, "inspection kernel")?.try_into()?,
            planner_version: v.planner_version,
            emitter_version: v.emitter_version,
            verifier_version: v.verifier_version,
        };
        value.validate().map_err(|e| e.to_string())?;
        Ok(value)
    }
}
impl TryFrom<ManufacturingInspectionProvenance> for rpc::ManufacturingInspectionProvenance {
    type Error = String;
    fn try_from(v: ManufacturingInspectionProvenance) -> Result<Self, String> {
        v.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            project_id: v.project_id.as_str().into(),
            input_revision: v.input_revision.get(),
            input_hash: v.input_hash.into(),
            surface_overlays: Some(v.surface_overlays.try_into()?),
            kernel: Some(v.kernel.try_into()?),
            planner_version: v.planner_version,
            emitter_version: v.emitter_version,
            verifier_version: v.verifier_version,
        })
    }
}
impl TryFrom<rpc::ReplayDisplayInfo> for ReplayDisplayInfo {
    type Error = String;
    fn try_from(v: rpc::ReplayDisplayInfo) -> Result<Self, String> {
        let value = Self {
            schema_version: v.schema_version,
            bundle_hash: SourceHash::parse(v.bundle_hash).map_err(|e| e.to_string())?,
            input_hash: SourceHash::parse(v.input_hash).map_err(|e| e.to_string())?,
            program_hash: SourceHash::parse(v.program_hash).map_err(|e| e.to_string())?,
            verifier_version: v.verifier_version,
            frame: rpc::PrinterCoordinateFrame::try_from(v.frame)
                .map_err(|_| "unknown replay frame")?
                .try_into()?,
            event_count: v.event_count,
            deposition_count: v.deposition_count,
            travel_count: v.travel_count,
            layer_count: v.layer_count,
            lifecycle_count: v.lifecycle_count,
            process_count: v.process_count,
            bounds_mm: required(v.bounds_mm, "replay bounds")?.try_into()?,
        };
        value.validate().map_err(|e| e.to_string())?;
        Ok(value)
    }
}
impl TryFrom<ReplayDisplayInfo> for rpc::ReplayDisplayInfo {
    type Error = String;
    fn try_from(v: ReplayDisplayInfo) -> Result<Self, String> {
        v.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            schema_version: v.schema_version,
            bundle_hash: v.bundle_hash.into(),
            input_hash: v.input_hash.into(),
            program_hash: v.program_hash.into(),
            verifier_version: v.verifier_version,
            frame: rpc::PrinterCoordinateFrame::try_from(v.frame)? as i32,
            event_count: v.event_count,
            deposition_count: v.deposition_count,
            travel_count: v.travel_count,
            layer_count: v.layer_count,
            lifecycle_count: v.lifecycle_count,
            process_count: v.process_count,
            bounds_mm: Some(v.bounds_mm.try_into()?),
        })
    }
}
impl TryFrom<rpc::ReplayDisplayDescriptor> for ReplayDisplayDescriptor {
    type Error = String;
    fn try_from(v: rpc::ReplayDisplayDescriptor) -> Result<Self, String> {
        let value = Self {
            info: required(v.info, "replay info")?.try_into()?,
            resource: super::artifact_to_view(
                required(v.resource, "replay resource")?,
                MAX_REPLAY_DISPLAY_BYTES as u64,
            )?,
        };
        value.validate().map_err(|e| e.to_string())?;
        Ok(value)
    }
}
impl TryFrom<ReplayDisplayDescriptor> for rpc::ReplayDisplayDescriptor {
    type Error = String;
    fn try_from(v: ReplayDisplayDescriptor) -> Result<Self, String> {
        v.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            info: Some(v.info.try_into()?),
            resource: Some(super::artifact_from_view(
                v.resource,
                MAX_REPLAY_DISPLAY_BYTES as u64,
            )?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{AabbMm, KernelIdentity};

    fn display() -> ReplayDisplayDescriptor {
        let hash = SourceHash::from_bytes(b"replay");
        ReplayDisplayDescriptor {
            info: ReplayDisplayInfo {
                schema_version: REPLAY_SCHEMA_VERSION,
                bundle_hash: SourceHash::from_bytes(b"bundle"),
                input_hash: SourceHash::from_bytes(b"input"),
                program_hash: SourceHash::from_bytes(b"program"),
                verifier_version: "1".into(),
                frame: PrinterCoordinateFrame::RightHandedMillimetres,
                event_count: 4,
                deposition_count: 2,
                travel_count: 2,
                layer_count: 1,
                lifecycle_count: 0,
                process_count: 0,
                bounds_mm: AabbMm::new([0.0; 3], [1.0; 3]).unwrap(),
            },
            resource: crate::ArtifactView {
                name: format!("artifacts/{}", hash.as_str()),
                sha256: hash.as_str().into(),
                size_bytes: "288".into(),
                media_type: REPLAY_MEDIA_TYPE.into(),
            },
        }
    }
    #[test]
    fn replay_wire_rejects_unspecified_output_frames_sizes_hashes_and_counts() {
        assert!(VerificationOutput::try_from(rpc::VerificationOutput::Unspecified).is_err());
        let command = crate::manufacturing::ManufacturingCommand::Inspect {
            session_id: crate::geometry::SessionId::new(),
            base_revision: ProjectRevision(7),
            output: VerificationOutput::ReportAndMotion,
        };
        let wire = rpc::VerifyManufacturingRequest::try_from(command.clone()).unwrap();
        assert_eq!(
            crate::manufacturing::ManufacturingCommand::try_from(wire.clone()).unwrap(),
            command
        );
        for output in [0, i32::MAX] {
            let mut invalid = wire.clone();
            invalid.output = output;
            assert!(crate::manufacturing::ManufacturingCommand::try_from(invalid).is_err());
        }
        let descriptor = display();
        let wire = rpc::ReplayDisplayDescriptor::try_from(descriptor.clone()).unwrap();
        assert_eq!(
            ReplayDisplayDescriptor::try_from(wire.clone()).unwrap(),
            descriptor
        );
        let mut invalid = wire.clone();
        invalid.info.as_mut().unwrap().frame = 0;
        assert!(ReplayDisplayDescriptor::try_from(invalid).is_err());
        let mut invalid = wire.clone();
        invalid.info.as_mut().unwrap().program_hash = "BAD".into();
        assert!(ReplayDisplayDescriptor::try_from(invalid).is_err());
        let mut invalid = wire.clone();
        invalid.info.as_mut().unwrap().event_count = 200_001;
        assert!(ReplayDisplayDescriptor::try_from(invalid).is_err());
        let mut invalid = wire.clone();
        invalid.resource.as_mut().unwrap().size_bytes += 1;
        assert!(ReplayDisplayDescriptor::try_from(invalid).is_err());
        let mut invalid = wire;
        invalid.resource.as_mut().unwrap().media_type = "application/json".into();
        assert!(ReplayDisplayDescriptor::try_from(invalid).is_err());
        let mut limit = descriptor.info.clone();
        limit.event_count = 200_000;
        limit.deposition_count = 100_000;
        limit.travel_count = 100_000;
        assert_eq!(limit.byte_count().unwrap(), 9_600_096);
        limit.travel_count += 1;
        assert!(limit.validate().is_err());
    }

    #[test]
    fn inspection_provenance_is_checked_and_bound_to_record() {
        let descriptor = display();
        let record = ManufacturingArtifactRecord {
            hash: descriptor.info.bundle_hash.clone(),
            input_hash: descriptor.info.input_hash.clone(),
            byte_count: 10,
            summary: ManufacturingSummary {
                layers: 1,
                native_geometry: NativeGeometryCoverageSummary {
                    requested_shape_error_mm: 0.1,
                    achieved_shape_error_mm: 0.0,
                    height_analysis_count: 1,
                    slab_count: 1,
                    certified_layer_count: 1,
                    event_count: 2,
                    geometry_work: 1,
                    evidence: PreflightEvidence::Certified,
                    source_feature_coverage: NativeFeatureCoverageSummary {
                        disjoint_source_layer_count: 1,
                        invariant_source_union_layer_count: 0,
                        native_interface_complex_layer_count: 0,
                        owned_critical_slab_count: 0,
                    },
                    skin_coverage: SkinCoverageScope::ScheduledMaterialDistance,
                    support_clearance: SupportClearanceScope::ContinuousTargetOuterOccupancy,
                },
                paths: 2,
                deposition_segments: 2,
                deposited_volume_mm3: 1.0,
                filament_length_mm: 1.0,
                software_only: true,
                support_deposition_segments: 0,
                support_deposited_volume_mm3: 0.0,
                support_filament_length_mm: 0.0,
                surface_resolution: SurfaceResolutionTotals::empty(),
            },
        };
        descriptor.validate_for_record(&record).unwrap();
        let provenance = ManufacturingInspectionProvenance {
            project_id: ProjectId::new(),
            input_revision: ProjectRevision(9),
            input_hash: record.input_hash.clone(),
            surface_overlays: crate::surface::SurfaceOverlays::empty(),
            kernel: KernelIdentity {
                name: "monstertruck".into(),
                version: "0.4.1".into(),
                revision: "rev".into(),
            },
            planner_version: "1".into(),
            emitter_version: "1".into(),
            verifier_version: "1".into(),
        };
        let wire = rpc::ManufacturingInspectionProvenance::try_from(provenance.clone()).unwrap();
        assert_eq!(
            ManufacturingInspectionProvenance::try_from(wire.clone()).unwrap(),
            provenance
        );
        let mut invalid = wire;
        invalid.verifier_version = "x".repeat(MAX_MANUFACTURING_LABEL_BYTES + 1);
        assert!(ManufacturingInspectionProvenance::try_from(invalid).is_err());
        let result = crate::geometry::JobResult::ManufacturingVerified {
            record: record.clone(),
            provenance: provenance.clone(),
            replay_display: Some(descriptor.clone()),
            report: VerificationReport {
                verified: true,
                coverage: vec!["actual replay".into()],
                limitations: vec!["software only".into()],
                deposition_segments: 2,
                travel_segments: 2,
                deposited_volume_mm3: 1.0,
                filament_length_mm: 1.0,
                max_position_error_mm: 0.0,
                max_extrusion_error_mm: 0.0,
                lifecycle_motion_count: 0,
                priming_filament_length_mm: 0.0,
                support_deposition_segments: 0,
                support_deposited_volume_mm3: 0.0,
                support_filament_length_mm: 0.0,
                retraction_count: 0,
                retracted_filament_mm: 0.0,
            },
            resource: crate::ArtifactView {
                name: format!("artifacts/{}", record.hash.as_str()),
                sha256: record.hash.as_str().into(),
                size_bytes: "10".into(),
                media_type: "application/x-spiling-manufacturing-bundle".into(),
            },
        };
        let result_wire = rpc::NativeOperationResult::try_from(result.clone()).unwrap();
        assert_eq!(
            crate::geometry::JobResult::try_from(result_wire.clone()).unwrap(),
            result
        );
        let rpc::native_operation_result::Result::ManufacturingVerified(mut verified) =
            result_wire.result.unwrap()
        else {
            panic!("wrong native result")
        };
        verified
            .replay_display
            .as_mut()
            .unwrap()
            .info
            .as_mut()
            .unwrap()
            .verifier_version = "other".into();
        assert!(
            crate::geometry::JobResult::try_from(rpc::NativeOperationResult {
                result: Some(rpc::native_operation_result::Result::ManufacturingVerified(
                    verified.clone()
                ))
            })
            .is_err()
        );
        verified.replay_display = None;
        verified.provenance = None;
        assert!(
            crate::geometry::JobResult::try_from(rpc::NativeOperationResult {
                result: Some(rpc::native_operation_result::Result::ManufacturingVerified(
                    verified
                ))
            })
            .is_err()
        );
        let mut wrong_record = record;
        wrong_record.input_hash = SourceHash::from_bytes(b"other");
        assert!(descriptor.validate_for_record(&wrong_record).is_err());
        assert!(provenance.validate_for_record(&wrong_record).is_err());
    }
}
