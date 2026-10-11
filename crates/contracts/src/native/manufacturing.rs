// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Checked typed manufacturing RPC adapters. Bundle JSON remains a persistence format.
use crate::{geometry::SourceHash, manufacturing::*, project::ProjectRevision, rpc};
use std::sync::Arc;

fn required<T>(value: Option<T>, field: &str) -> Result<T, String> {
    value.ok_or_else(|| format!("missing {field}"))
}
fn valid<T>(
    value: T,
    check: impl FnOnce(&T) -> Result<(), ManufacturingError>,
) -> Result<T, String> {
    check(&value).map_err(|e| e.to_string())?;
    Ok(value)
}

#[expect(
    clippy::large_enum_variant,
    reason = "A one-shot bounded RPC dispatch value does not need a separate heap allocation"
)]
#[derive(Debug, Clone)]
pub enum ManufacturingCall {
    Execute(rpc::ManufacturingRequest),
    Compile(rpc::CompileManufacturingRequest),
    Verify(rpc::VerifyManufacturingRequest),
}

pub fn manufacturing_call(
    command: ManufacturingCommand,
    request_id: &str,
) -> Result<ManufacturingCall, String> {
    super::request_id(request_id)?;
    match command {
        command @ ManufacturingCommand::Compile { .. } => {
            let mut request = rpc::CompileManufacturingRequest::try_from(command)?;
            request.request_id = request_id.to_owned();
            Ok(ManufacturingCall::Compile(request))
        }
        command @ ManufacturingCommand::Inspect { .. } => {
            let mut request = rpc::VerifyManufacturingRequest::try_from(command)?;
            request.request_id = request_id.to_owned();
            Ok(ManufacturingCall::Verify(request))
        }
        command => Ok(ManufacturingCall::Execute(command.try_into()?)),
    }
}

impl TryFrom<ManufacturingCommand> for rpc::ManufacturingRequest {
    type Error = String;
    fn try_from(value: ManufacturingCommand) -> Result<Self, String> {
        use rpc::manufacturing_request::Command;
        let (session_id, command) = match value {
            ManufacturingCommand::Get { session_id } => {
                (session_id, Command::Get(rpc::GetManufacturingRequest {}))
            }
            ManufacturingCommand::SetIntent {
                session_id,
                base_revision,
                intent,
            } => (
                session_id,
                Command::SetIntent(rpc::SetManufacturingIntentRequest {
                    base_revision: base_revision.get(),
                    intent: Some(Arc::unwrap_or_clone(intent).try_into()?),
                }),
            ),
            _ => return Err("long manufacturing command requires its dedicated RPC".into()),
        };
        let parent = format!("sessions/{}", session_id.as_str());
        super::session_parent(&parent)?;
        Ok(Self {
            parent,
            command: Some(command),
        })
    }
}
impl TryFrom<rpc::ManufacturingRequest> for ManufacturingCommand {
    type Error = String;
    fn try_from(value: rpc::ManufacturingRequest) -> Result<Self, String> {
        use rpc::manufacturing_request::Command;
        let session_id = super::session_parent(&value.parent)?;
        match required(value.command, "manufacturing command")? {
            Command::Get(_) => Ok(Self::Get { session_id }),
            Command::SetIntent(request) => Ok(Self::SetIntent {
                session_id,
                base_revision: ProjectRevision(request.base_revision),
                intent: Arc::new(required(request.intent, "manufacturing intent")?.try_into()?),
            }),
        }
    }
}
impl TryFrom<ManufacturingCommand> for rpc::CompileManufacturingRequest {
    type Error = String;
    fn try_from(value: ManufacturingCommand) -> Result<Self, String> {
        let ManufacturingCommand::Compile {
            session_id,
            base_revision,
        } = value
        else {
            return Err("expected manufacturing compile command".into());
        };
        let parent = format!("sessions/{}", session_id.as_str());
        super::session_parent(&parent)?;
        Ok(Self {
            parent,
            request_id: String::new(),
            base_revision: base_revision.get(),
        })
    }
}
impl TryFrom<rpc::CompileManufacturingRequest> for ManufacturingCommand {
    type Error = String;
    fn try_from(value: rpc::CompileManufacturingRequest) -> Result<Self, String> {
        super::request_id(&value.request_id)?;
        Ok(Self::Compile {
            session_id: super::session_parent(&value.parent)?,
            base_revision: ProjectRevision(value.base_revision),
        })
    }
}
impl TryFrom<ManufacturingCommand> for rpc::VerifyManufacturingRequest {
    type Error = String;
    fn try_from(value: ManufacturingCommand) -> Result<Self, String> {
        let ManufacturingCommand::Inspect {
            session_id,
            base_revision,
        } = value
        else {
            return Err("expected manufacturing inspect command".into());
        };
        let parent = format!("sessions/{}", session_id.as_str());
        super::session_parent(&parent)?;
        Ok(Self {
            parent,
            request_id: String::new(),
            base_revision: base_revision.get(),
        })
    }
}
impl TryFrom<rpc::VerifyManufacturingRequest> for ManufacturingCommand {
    type Error = String;
    fn try_from(value: rpc::VerifyManufacturingRequest) -> Result<Self, String> {
        super::request_id(&value.request_id)?;
        Ok(Self::Inspect {
            session_id: super::session_parent(&value.parent)?,
            base_revision: ProjectRevision(value.base_revision),
        })
    }
}

impl TryFrom<rpc::PrinterCoordinateFrame> for PrinterCoordinateFrame {
    type Error = String;
    fn try_from(value: rpc::PrinterCoordinateFrame) -> Result<Self, String> {
        match value {
            rpc::PrinterCoordinateFrame::RightHandedMillimetres => Ok(Self::RightHandedMillimetres),
            _ => Err("unsupported printer coordinate frame".into()),
        }
    }
}
impl TryFrom<PrinterCoordinateFrame> for rpc::PrinterCoordinateFrame {
    type Error = String;
    fn try_from(value: PrinterCoordinateFrame) -> Result<Self, String> {
        match value {
            PrinterCoordinateFrame::RightHandedMillimetres => Ok(Self::RightHandedMillimetres),
        }
    }
}
impl TryFrom<rpc::MotionSpace> for MotionSpace {
    type Error = String;
    fn try_from(value: rpc::MotionSpace) -> Result<Self, String> {
        match value {
            rpc::MotionSpace::CartesianXyz => Ok(Self::CartesianXyz),
            _ => Err("unsupported motion space".into()),
        }
    }
}
impl TryFrom<MotionSpace> for rpc::MotionSpace {
    type Error = String;
    fn try_from(value: MotionSpace) -> Result<Self, String> {
        match value {
            MotionSpace::CartesianXyz => Ok(Self::CartesianXyz),
        }
    }
}
impl TryFrom<rpc::OutputDialect> for OutputDialect {
    type Error = String;
    fn try_from(value: rpc::OutputDialect) -> Result<Self, String> {
        match value {
            rpc::OutputDialect::CartesianAbsoluteGcodeV1 => Ok(Self::CartesianAbsoluteGcodeV1),
            _ => Err("unsupported output dialect".into()),
        }
    }
}
impl TryFrom<OutputDialect> for rpc::OutputDialect {
    type Error = String;
    fn try_from(value: OutputDialect) -> Result<Self, String> {
        match value {
            OutputDialect::CartesianAbsoluteGcodeV1 => Ok(Self::CartesianAbsoluteGcodeV1),
        }
    }
}
impl TryFrom<rpc::PrinterCapabilities> for PrinterCapabilities {
    type Error = String;
    fn try_from(value: rpc::PrinterCapabilities) -> Result<Self, String> {
        valid(
            Self {
                motion_space: rpc::MotionSpace::try_from(value.motion_space)
                    .map_err(|e| e.to_string())?
                    .try_into()?,
                extruders: value.extruders,
                output_dialect: rpc::OutputDialect::try_from(value.output_dialect)
                    .map_err(|e| e.to_string())?
                    .try_into()?,
                generated_supports: value.generated_supports,
            },
            Self::validate,
        )
    }
}
impl TryFrom<PrinterCapabilities> for rpc::PrinterCapabilities {
    type Error = String;
    fn try_from(value: PrinterCapabilities) -> Result<Self, String> {
        value.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            motion_space: rpc::MotionSpace::try_from(value.motion_space)? as i32,
            extruders: value.extruders,
            output_dialect: rpc::OutputDialect::try_from(value.output_dialect)? as i32,
            generated_supports: value.generated_supports,
        })
    }
}
impl TryFrom<rpc::MachineComponentRole> for MachineComponentRole {
    type Error = String;
    fn try_from(value: rpc::MachineComponentRole) -> Result<Self, String> {
        match value {
            rpc::MachineComponentRole::Bed => Ok(Self::Bed),
            rpc::MachineComponentRole::Toolhead => Ok(Self::Toolhead),
            rpc::MachineComponentRole::Fixture => Ok(Self::Fixture),
            _ => Err("unsupported machine component role".into()),
        }
    }
}
impl TryFrom<MachineComponentRole> for rpc::MachineComponentRole {
    type Error = String;
    fn try_from(value: MachineComponentRole) -> Result<Self, String> {
        Ok(match value {
            MachineComponentRole::Bed => Self::Bed,
            MachineComponentRole::Toolhead => Self::Toolhead,
            MachineComponentRole::Fixture => Self::Fixture,
        })
    }
}
fn validate_shape(value: &MachineComponentShape) -> Result<(), String> {
    match value {
        MachineComponentShape::Box { bounds_mm } => {
            bounds_mm.validate().map_err(|e| e.to_string())?;
            if (0..3).any(|i| {
                bounds_mm.min[i] >= bounds_mm.max[i]
                    || !(bounds_mm.max[i] - bounds_mm.min[i]).is_finite()
            }) {
                return Err("component box must have finite positive dimensions".into());
            }
        }
        MachineComponentShape::Cylinder {
            radius_mm,
            height_mm,
        } => {
            if [*radius_mm, *height_mm]
                .iter()
                .any(|v| !v.is_finite() || *v <= 0.0)
            {
                return Err("component cylinder must have finite positive dimensions".into());
            }
        }
    }
    Ok(())
}
impl TryFrom<rpc::MachineComponentShape> for MachineComponentShape {
    type Error = String;
    fn try_from(value: rpc::MachineComponentShape) -> Result<Self, String> {
        use rpc::machine_component_shape::Shape;
        let shape = match required(value.shape, "machine component shape")? {
            Shape::Box(bounds) => Self::Box {
                bounds_mm: bounds.try_into()?,
            },
            Shape::Cylinder(cylinder) => Self::Cylinder {
                radius_mm: cylinder.radius_mm,
                height_mm: cylinder.height_mm,
            },
        };
        validate_shape(&shape)?;
        Ok(shape)
    }
}
impl TryFrom<MachineComponentShape> for rpc::MachineComponentShape {
    type Error = String;
    fn try_from(value: MachineComponentShape) -> Result<Self, String> {
        use rpc::machine_component_shape::Shape;
        validate_shape(&value)?;
        Ok(Self {
            shape: Some(match value {
                MachineComponentShape::Box { bounds_mm } => Shape::Box(bounds_mm.try_into()?),
                MachineComponentShape::Cylinder {
                    radius_mm,
                    height_mm,
                } => Shape::Cylinder(rpc::CylinderComponentShape {
                    radius_mm,
                    height_mm,
                }),
            }),
        })
    }
}
impl TryFrom<rpc::MachineComponent> for MachineComponent {
    type Error = String;
    fn try_from(value: rpc::MachineComponent) -> Result<Self, String> {
        valid(
            Self {
                id: value.id,
                role: rpc::MachineComponentRole::try_from(value.role)
                    .map_err(|e| e.to_string())?
                    .try_into()?,
                pose: required(value.pose, "component pose")?.try_into()?,
                shape: required(value.shape, "component shape")?.try_into()?,
            },
            Self::validate,
        )
    }
}
impl TryFrom<MachineComponent> for rpc::MachineComponent {
    type Error = String;
    fn try_from(value: MachineComponent) -> Result<Self, String> {
        value.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            id: value.id,
            role: rpc::MachineComponentRole::try_from(value.role)? as i32,
            pose: Some(value.pose.try_into()?),
            shape: Some(value.shape.try_into()?),
        })
    }
}
impl TryFrom<rpc::PrinterSpecification> for PrinterSpecification {
    type Error = String;
    fn try_from(value: rpc::PrinterSpecification) -> Result<Self, String> {
        if value.components.len() > MAX_MACHINE_COMPONENTS as usize {
            return Err("machine component limit exceeded".into());
        }
        valid(
            Self {
                schema_version: value.schema_version,
                id: value.id,
                revision: value.revision,
                name: value.name,
                coordinate_frame: rpc::PrinterCoordinateFrame::try_from(value.coordinate_frame)
                    .map_err(|e| e.to_string())?
                    .try_into()?,
                build_envelope: required(value.build_envelope, "build envelope")?.try_into()?,
                components: value
                    .components
                    .into_iter()
                    .map(TryInto::try_into)
                    .collect::<Result<_, String>>()?,
                capabilities: required(value.capabilities, "printer capabilities")?.try_into()?,
                nozzle_diameter_mm: value.nozzle_diameter_mm,
                max_feed_mm_s: value.max_feed_mm_s,
                max_volumetric_flow_mm3_s: value.max_volumetric_flow_mm3_s,
            },
            Self::validate,
        )
    }
}
impl TryFrom<PrinterSpecification> for rpc::PrinterSpecification {
    type Error = String;
    fn try_from(value: PrinterSpecification) -> Result<Self, String> {
        value.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            schema_version: value.schema_version,
            id: value.id,
            revision: value.revision,
            name: value.name,
            coordinate_frame: rpc::PrinterCoordinateFrame::try_from(value.coordinate_frame)? as i32,
            build_envelope: Some(value.build_envelope.try_into()?),
            components: value
                .components
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, String>>()?,
            capabilities: Some(value.capabilities.try_into()?),
            nozzle_diameter_mm: value.nozzle_diameter_mm,
            max_feed_mm_s: value.max_feed_mm_s,
            max_volumetric_flow_mm3_s: value.max_volumetric_flow_mm3_s,
        })
    }
}
impl TryFrom<rpc::PlanarPrintRecipe> for PlanarPrintRecipe {
    type Error = String;
    fn try_from(value: rpc::PlanarPrintRecipe) -> Result<Self, String> {
        valid(
            Self {
                id: value.id,
                revision: value.revision,
                material_id: value.material_id,
                material_revision: value.material_revision,
                filament_diameter_mm: value.filament_diameter_mm,
                layer_height_mm: value.layer_height_mm,
                bead_width_mm: value.bead_width_mm,
                perimeter_count: value.perimeter_count,
                infill_fraction: value.infill_fraction,
                print_speed_mm_s: value.print_speed_mm_s,
                travel_speed_mm_s: value.travel_speed_mm_s,
                flow_multiplier: value.flow_multiplier,
            },
            Self::validate,
        )
    }
}
impl TryFrom<PlanarPrintRecipe> for rpc::PlanarPrintRecipe {
    type Error = String;
    fn try_from(value: PlanarPrintRecipe) -> Result<Self, String> {
        value.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            id: value.id,
            revision: value.revision,
            material_id: value.material_id,
            material_revision: value.material_revision,
            filament_diameter_mm: value.filament_diameter_mm,
            layer_height_mm: value.layer_height_mm,
            bead_width_mm: value.bead_width_mm,
            perimeter_count: value.perimeter_count,
            infill_fraction: value.infill_fraction,
            print_speed_mm_s: value.print_speed_mm_s,
            travel_speed_mm_s: value.travel_speed_mm_s,
            flow_multiplier: value.flow_multiplier,
        })
    }
}
impl TryFrom<rpc::ManufacturingIntent> for ManufacturingIntent {
    type Error = String;
    fn try_from(value: rpc::ManufacturingIntent) -> Result<Self, String> {
        valid(
            Self {
                printer: required(value.printer, "printer specification")?.try_into()?,
                recipe: required(value.recipe, "planar recipe")?.try_into()?,
            },
            Self::validate,
        )
    }
}
impl TryFrom<ManufacturingIntent> for rpc::ManufacturingIntent {
    type Error = String;
    fn try_from(value: ManufacturingIntent) -> Result<Self, String> {
        value.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            printer: Some(value.printer.try_into()?),
            recipe: Some(value.recipe.try_into()?),
        })
    }
}
impl TryFrom<rpc::ManufacturingSummary> for ManufacturingSummary {
    type Error = String;
    fn try_from(value: rpc::ManufacturingSummary) -> Result<Self, String> {
        valid(
            Self {
                layers: value.layers,
                paths: value.paths,
                deposition_segments: value.deposition_segments,
                deposited_volume_mm3: value.deposited_volume_mm3,
                filament_length_mm: value.filament_length_mm,
                software_only: value.software_only,
            },
            Self::validate,
        )
    }
}
impl TryFrom<ManufacturingSummary> for rpc::ManufacturingSummary {
    type Error = String;
    fn try_from(value: ManufacturingSummary) -> Result<Self, String> {
        value.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            layers: value.layers,
            paths: value.paths,
            deposition_segments: value.deposition_segments,
            deposited_volume_mm3: value.deposited_volume_mm3,
            filament_length_mm: value.filament_length_mm,
            software_only: value.software_only,
        })
    }
}
impl TryFrom<rpc::ManufacturingArtifactRecord> for ManufacturingArtifactRecord {
    type Error = String;
    fn try_from(value: rpc::ManufacturingArtifactRecord) -> Result<Self, String> {
        valid(
            Self {
                hash: SourceHash::parse(value.hash).map_err(|e| e.to_string())?,
                input_hash: SourceHash::parse(value.input_hash).map_err(|e| e.to_string())?,
                byte_count: value.byte_count,
                summary: required(value.summary, "manufacturing summary")?.try_into()?,
            },
            Self::validate,
        )
    }
}
impl TryFrom<ManufacturingArtifactRecord> for rpc::ManufacturingArtifactRecord {
    type Error = String;
    fn try_from(value: ManufacturingArtifactRecord) -> Result<Self, String> {
        value.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            hash: value.hash.into(),
            input_hash: value.input_hash.into(),
            byte_count: value.byte_count,
            summary: Some(value.summary.try_into()?),
        })
    }
}
impl TryFrom<rpc::VerificationReport> for VerificationReport {
    type Error = String;
    fn try_from(value: rpc::VerificationReport) -> Result<Self, String> {
        valid(
            Self {
                verified: value.verified,
                coverage: value.coverage,
                limitations: value.limitations,
                deposition_segments: value.deposition_segments,
                travel_segments: value.travel_segments,
                deposited_volume_mm3: value.deposited_volume_mm3,
                filament_length_mm: value.filament_length_mm,
                max_position_error_mm: value.max_position_error_mm,
                max_extrusion_error_mm: value.max_extrusion_error_mm,
            },
            Self::validate,
        )
    }
}
impl TryFrom<VerificationReport> for rpc::VerificationReport {
    type Error = String;
    fn try_from(value: VerificationReport) -> Result<Self, String> {
        value.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            verified: value.verified,
            coverage: value.coverage,
            limitations: value.limitations,
            deposition_segments: value.deposition_segments,
            travel_segments: value.travel_segments,
            deposited_volume_mm3: value.deposited_volume_mm3,
            filament_length_mm: value.filament_length_mm,
            max_position_error_mm: value.max_position_error_mm,
            max_extrusion_error_mm: value.max_extrusion_error_mm,
        })
    }
}

/// Match the immutable ByteStream descriptor to the domain bundle record.
pub fn validate_resource(
    record: &ManufacturingArtifactRecord,
    resource: &rpc::Artifact,
) -> Result<(), String> {
    record.validate().map_err(|e| e.to_string())?;
    super::validate_artifact(resource, MAX_MANUFACTURING_BUNDLE_BYTES as u64)?;
    if resource.sha256 != record.hash.as_str()
        || resource.size_bytes != u64::from(record.byte_count)
        || resource.media_type != "application/x-spiling-manufacturing-bundle"
    {
        return Err("manufacturing descriptor disagrees with artifact record".into());
    }
    Ok(())
}

impl TryFrom<ManufacturingResponse> for rpc::ManufacturingReply {
    type Error = String;
    fn try_from(value: ManufacturingResponse) -> Result<Self, String> {
        let ManufacturingResponse::Status {
            info,
            intent,
            artifact,
            resource,
        } = value
        else {
            return Err(
                "only manufacturing Status is a short success reply; failures use canonical Status"
                    .into(),
            );
        };
        let resource = resource
            .map(|v| super::artifact_from_view(v, MAX_MANUFACTURING_BUNDLE_BYTES as u64))
            .transpose()?;
        match (&artifact, &resource) {
            (Some(record), Some(resource)) => validate_resource(record, resource)?,
            (None, None) => {}
            _ => {
                return Err(
                    "manufacturing artifact and descriptor must be present together".into(),
                );
            }
        }
        Ok(Self {
            status: Some(rpc::ManufacturingStatus {
                info: Some(info.try_into()?),
                intent: intent
                    .map(|v| rpc::ManufacturingIntent::try_from(Arc::unwrap_or_clone(v)))
                    .transpose()?,
                artifact: artifact.map(TryInto::try_into).transpose()?,
                resource,
            }),
        })
    }
}
impl TryFrom<rpc::ManufacturingReply> for ManufacturingResponse {
    type Error = String;
    fn try_from(value: rpc::ManufacturingReply) -> Result<Self, String> {
        let status = required(value.status, "manufacturing status")?;
        let artifact: Option<ManufacturingArtifactRecord> =
            status.artifact.map(TryInto::try_into).transpose()?;
        match (&artifact, &status.resource) {
            (Some(record), Some(resource)) => validate_resource(record, resource)?,
            (None, None) => {}
            _ => {
                return Err(
                    "manufacturing artifact and descriptor must be present together".into(),
                );
            }
        }
        Ok(Self::Status {
            info: required(status.info, "project info")?.try_into()?,
            intent: status
                .intent
                .map(|v| v.try_into().map(Arc::new))
                .transpose()?,
            artifact,
            resource: status
                .resource
                .map(|v| super::artifact_to_view(v, MAX_MANUFACTURING_BUNDLE_BYTES as u64))
                .transpose()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::SessionId;

    fn intent() -> ManufacturingIntent {
        decode_intent(include_bytes!(
            "../../../../fixtures/manufacturing/solid-fill.intent.json"
        ))
        .unwrap()
    }

    #[test]
    fn intent_round_trip_reapplies_domain_admission() {
        let domain = intent();
        let wire = rpc::ManufacturingIntent::try_from(domain.clone()).unwrap();
        assert_eq!(ManufacturingIntent::try_from(wire.clone()).unwrap(), domain);
        let mut invalid = wire.clone();
        invalid.recipe.as_mut().unwrap().flow_multiplier = f64::NAN;
        assert!(ManufacturingIntent::try_from(invalid).is_err());
        let mut invalid = wire.clone();
        invalid.printer.as_mut().unwrap().coordinate_frame = 0;
        assert!(ManufacturingIntent::try_from(invalid).is_err());
        let mut invalid = wire.clone();
        invalid
            .printer
            .as_mut()
            .unwrap()
            .capabilities
            .as_mut()
            .unwrap()
            .motion_space = i32::MAX;
        assert!(ManufacturingIntent::try_from(invalid).is_err());
        let mut invalid = wire.clone();
        invalid.printer.as_mut().unwrap().components =
            vec![rpc::MachineComponent::default(); MAX_MACHINE_COMPONENTS as usize + 1];
        assert!(ManufacturingIntent::try_from(invalid).is_err());
        let mut invalid = wire;
        invalid.recipe.as_mut().unwrap().infill_fraction = 0.5;
        assert!(ManufacturingIntent::try_from(invalid).is_err());
    }

    #[test]
    fn typed_dispatch_preserves_project_revision_and_request_id() {
        let session_id = SessionId::new();
        let command = ManufacturingCommand::SetIntent {
            session_id: session_id.clone(),
            base_revision: ProjectRevision(17),
            intent: Arc::new(intent()),
        };
        let ManufacturingCall::Execute(request) = manufacturing_call(command.clone(), "").unwrap()
        else {
            panic!("wrong RPC")
        };
        assert_eq!(ManufacturingCommand::try_from(request).unwrap(), command);
        let command = ManufacturingCommand::Inspect {
            session_id,
            base_revision: ProjectRevision(19),
        };
        let request_id = "3d21ddeb-0e96-4c9e-a2e2-7f37642366f1";
        let ManufacturingCall::Verify(request) =
            manufacturing_call(command.clone(), request_id).unwrap()
        else {
            panic!("wrong RPC")
        };
        assert_eq!(request.request_id, request_id);
        assert_eq!(
            ManufacturingCommand::try_from(request.clone()).unwrap(),
            command
        );
        let mut invalid = request;
        invalid.parent = "sessions/not-a-session".into();
        assert!(ManufacturingCommand::try_from(invalid).is_err());
        assert!(manufacturing_call(command, "invalid").is_err());
        assert!(
            ManufacturingCommand::try_from(rpc::ManufacturingRequest {
                parent: "sessions/not-a-session".into(),
                command: None
            })
            .is_err()
        );
    }

    #[test]
    fn artifact_descriptors_and_summaries_are_checked() {
        let record = ManufacturingArtifactRecord {
            hash: SourceHash::from_bytes(b"bundle"),
            input_hash: SourceHash::from_bytes(b"input"),
            byte_count: 6,
            summary: ManufacturingSummary {
                layers: 1,
                paths: 1,
                deposition_segments: 2,
                deposited_volume_mm3: 1.0,
                filament_length_mm: 1.0,
                software_only: true,
            },
        };
        let wire = rpc::ManufacturingArtifactRecord::try_from(record.clone()).unwrap();
        assert_eq!(
            ManufacturingArtifactRecord::try_from(wire.clone()).unwrap(),
            record
        );
        let mut invalid = wire.clone();
        invalid.hash = "not-sha256".into();
        assert!(ManufacturingArtifactRecord::try_from(invalid).is_err());
        let mut invalid = wire;
        invalid.summary.as_mut().unwrap().software_only = false;
        assert!(ManufacturingArtifactRecord::try_from(invalid).is_err());
        let descriptor = rpc::Artifact {
            name: format!("artifacts/{}", record.hash.as_str()),
            size_bytes: 6,
            sha256: record.hash.as_str().to_owned(),
            media_type: "application/x-spiling-manufacturing-bundle".into(),
        };
        validate_resource(&record, &descriptor).unwrap();
        let mut wrong_class = descriptor.clone();
        wrong_class.media_type = "application/json".into();
        assert!(validate_resource(&record, &wrong_class).is_err());
        let mut invalid = descriptor;
        invalid.size_bytes += 1;
        assert!(validate_resource(&record, &invalid).is_err());
    }

    #[test]
    fn reports_reject_unbounded_descriptions_and_nonfinite_measurements() {
        let report = VerificationReport {
            verified: true,
            coverage: vec!["software replay".into()],
            limitations: vec!["not machine ready".into()],
            deposition_segments: 2,
            travel_segments: 1,
            deposited_volume_mm3: 1.0,
            filament_length_mm: 1.0,
            max_position_error_mm: 0.0,
            max_extrusion_error_mm: 0.0,
        };
        let wire = rpc::VerificationReport::try_from(report.clone()).unwrap();
        assert_eq!(VerificationReport::try_from(wire.clone()).unwrap(), report);
        let mut invalid = wire.clone();
        invalid.coverage = vec!["coverage".into(); 65];
        assert!(VerificationReport::try_from(invalid).is_err());
        let mut invalid = wire;
        invalid.max_position_error_mm = f64::INFINITY;
        assert!(VerificationReport::try_from(invalid).is_err());
        assert!(
            rpc::ManufacturingReply::try_from(ManufacturingResponse::Error {
                error: ManufacturingError::new(
                    ManufacturingErrorCode::InvalidSpecification,
                    "invalid intent"
                ),
            })
            .is_err()
        );
        assert!(ManufacturingResponse::try_from(rpc::ManufacturingReply::default()).is_err());
    }
}
