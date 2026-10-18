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
    Preflight(rpc::PreflightManufacturingRequest),
    Verify(rpc::VerifyManufacturingRequest),
}

pub fn manufacturing_call(
    command: ManufacturingCommand,
    request_id: &str,
) -> Result<ManufacturingCall, String> {
    super::request_id(request_id)?;
    match command {
        command @ ManufacturingCommand::Preflight { .. } => {
            let mut request = rpc::PreflightManufacturingRequest::try_from(command)?;
            request.request_id = request_id.to_owned();
            Ok(ManufacturingCall::Preflight(request))
        }
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
            output,
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
            output: rpc::VerificationOutput::try_from(output)? as i32,
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
            output: rpc::VerificationOutput::try_from(value.output)
                .map_err(|_| "unknown verification output")?
                .try_into()?,
        })
    }
}
impl TryFrom<ManufacturingCommand> for rpc::PreflightManufacturingRequest {
    type Error = String;
    fn try_from(value: ManufacturingCommand) -> Result<Self, String> {
        let ManufacturingCommand::Preflight {
            session_id,
            base_revision,
        } = value
        else {
            return Err("expected manufacturing preflight command".into());
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
impl TryFrom<rpc::PreflightManufacturingRequest> for ManufacturingCommand {
    type Error = String;
    fn try_from(value: rpc::PreflightManufacturingRequest) -> Result<Self, String> {
        super::request_id(&value.request_id)?;
        Ok(Self::Preflight {
            session_id: super::session_parent(&value.parent)?,
            base_revision: ProjectRevision(value.base_revision),
        })
    }
}
impl TryFrom<rpc::RetractionPolicy> for RetractionPolicy {
    type Error = String;
    fn try_from(value: rpc::RetractionPolicy) -> Result<Self, String> {
        use rpc::retraction_policy::Policy;
        let policy = match required(value.policy, "retraction policy")? {
            Policy::Disabled(_) => Self::Disabled,
            Policy::Enabled(p) => Self::Enabled {
                distance_mm: p.distance_mm,
                speed_mm_s: p.speed_mm_s,
                minimum_travel_mm: p.minimum_travel_mm,
                z_hop_mm: p.z_hop_mm,
            },
        };
        policy.validate().map_err(|e| e.to_string())?;
        Ok(policy)
    }
}
impl TryFrom<RetractionPolicy> for rpc::RetractionPolicy {
    type Error = String;
    fn try_from(value: RetractionPolicy) -> Result<Self, String> {
        value.validate().map_err(|e| e.to_string())?;
        use rpc::retraction_policy::Policy;
        Ok(Self {
            policy: Some(match value {
                RetractionPolicy::Disabled => Policy::Disabled(rpc::RetractionDisabled {}),
                RetractionPolicy::Enabled {
                    distance_mm,
                    speed_mm_s,
                    minimum_travel_mm,
                    z_hop_mm,
                } => Policy::Enabled(rpc::RetractionEnabled {
                    distance_mm,
                    speed_mm_s,
                    minimum_travel_mm,
                    z_hop_mm,
                }),
            }),
        })
    }
}
impl TryFrom<rpc::ProcessPolicy> for ProcessPolicy {
    type Error = String;
    fn try_from(value: rpc::ProcessPolicy) -> Result<Self, String> {
        valid(
            Self {
                first_layer_speed_factor: value.first_layer_speed_factor,
                first_layer_flow_factor: value.first_layer_flow_factor,
                fan_first_layers: value
                    .fan_first_layers
                    .try_into()
                    .map_err(|_| "fan PWM exceeds 255")?,
                fan_after_first_layers: value
                    .fan_after_first_layers
                    .try_into()
                    .map_err(|_| "fan PWM exceeds 255")?,
                bridge_speed_factor: value.bridge_speed_factor,
                bridge_flow_factor: value.bridge_flow_factor,
                bridge_fan_pwm: value
                    .bridge_fan_pwm
                    .try_into()
                    .map_err(|_| "fan PWM exceeds 255")?,
                max_bridge_span_mm: value.max_bridge_span_mm,
                retraction: required(value.retraction, "retraction policy")?.try_into()?,
            },
            Self::validate,
        )
    }
}
impl TryFrom<ProcessPolicy> for rpc::ProcessPolicy {
    type Error = String;
    fn try_from(value: ProcessPolicy) -> Result<Self, String> {
        value.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            first_layer_speed_factor: value.first_layer_speed_factor,
            first_layer_flow_factor: value.first_layer_flow_factor,
            fan_first_layers: value.fan_first_layers.into(),
            fan_after_first_layers: value.fan_after_first_layers.into(),
            bridge_speed_factor: value.bridge_speed_factor,
            bridge_flow_factor: value.bridge_flow_factor,
            bridge_fan_pwm: value.bridge_fan_pwm.into(),
            max_bridge_span_mm: value.max_bridge_span_mm,
            retraction: Some(value.retraction.try_into()?),
        })
    }
}
impl TryFrom<rpc::SupportPolicy> for SupportPolicy {
    type Error = String;
    fn try_from(value: rpc::SupportPolicy) -> Result<Self, String> {
        use rpc::support_policy::Policy;
        valid(
            match required(value.policy, "support policy")? {
                Policy::Disabled(_) => Self::Disabled,
                Policy::Breakaway(p) => Self::Breakaway {
                    density: p.density,
                    xy_clearance_mm: p.xy_clearance_mm,
                    z_gap_mm: p.z_gap_mm,
                    interface_thickness_mm: p.interface_thickness_mm,
                },
            },
            Self::validate,
        )
    }
}
impl TryFrom<SupportPolicy> for rpc::SupportPolicy {
    type Error = String;
    fn try_from(value: SupportPolicy) -> Result<Self, String> {
        value.validate().map_err(|e| e.to_string())?;
        use rpc::support_policy::Policy;
        Ok(Self {
            policy: Some(match value {
                SupportPolicy::Disabled => Policy::Disabled(rpc::SupportsDisabled {}),
                SupportPolicy::Breakaway {
                    density,
                    xy_clearance_mm,
                    z_gap_mm,
                    interface_thickness_mm,
                } => Policy::Breakaway(rpc::BreakawaySupports {
                    density,
                    xy_clearance_mm,
                    z_gap_mm,
                    interface_thickness_mm,
                }),
            }),
        })
    }
}
impl TryFrom<rpc::PreflightReport> for PreflightReport {
    type Error = String;
    fn try_from(value: rpc::PreflightReport) -> Result<Self, String> {
        if value.checks.len() > 256 {
            return Err("preflight check limit exceeded".into());
        }
        valid(
            Self {
                input_revision: ProjectRevision(value.input_revision),
                disposition: match rpc::PreflightDisposition::try_from(value.disposition) {
                    Ok(rpc::PreflightDisposition::PreflightAdmitted) => {
                        PreflightDisposition::Admitted
                    }
                    Ok(rpc::PreflightDisposition::PreflightRefused) => {
                        PreflightDisposition::Refused
                    }
                    Ok(rpc::PreflightDisposition::PreflightUnresolved) => {
                        PreflightDisposition::Unresolved
                    }
                    _ => return Err("unknown preflight disposition".into()),
                },
                software_only: value.software_only,
                checks: value
                    .checks
                    .into_iter()
                    .map(|check| {
                        Ok(PreflightCheck {
                            code: check.code,
                            message: check.message,
                            evidence: match rpc::PreflightEvidence::try_from(check.evidence) {
                                Ok(rpc::PreflightEvidence::PreflightNotEvaluated) => {
                                    PreflightEvidence::NotEvaluated
                                }
                                Ok(rpc::PreflightEvidence::PreflightCertified) => {
                                    PreflightEvidence::Certified
                                }
                                Ok(rpc::PreflightEvidence::PreflightUnsupported) => {
                                    PreflightEvidence::Unsupported
                                }
                                Ok(rpc::PreflightEvidence::PreflightIndeterminate) => {
                                    PreflightEvidence::Indeterminate
                                }
                                Ok(rpc::PreflightEvidence::PreflightFailed) => {
                                    PreflightEvidence::Failed
                                }
                                _ => return Err("unknown preflight evidence".into()),
                            },
                            required: required(check.required, "preflight check required policy")?,
                            occurrence_id: check
                                .occurrence_id
                                .map(crate::geometry::OccurrenceId::new)
                                .transpose()
                                .map_err(|e| e.to_string())?,
                            face_id: check
                                .face_id
                                .map(crate::geometry::FaceId::parse)
                                .transpose()
                                .map_err(|e| e.to_string())?,
                            severity: match rpc::PreflightSeverity::try_from(check.severity) {
                                Ok(rpc::PreflightSeverity::PreflightError) => {
                                    PreflightSeverity::Error
                                }
                                Ok(rpc::PreflightSeverity::PreflightWarning) => {
                                    PreflightSeverity::Warning
                                }
                                Ok(rpc::PreflightSeverity::PreflightInfo) => {
                                    PreflightSeverity::Info
                                }
                                _ => return Err("unknown preflight severity".into()),
                            },
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?,
            },
            Self::validate,
        )
    }
}
impl TryFrom<PreflightReport> for rpc::PreflightReport {
    type Error = String;
    fn try_from(value: PreflightReport) -> Result<Self, String> {
        value.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            input_revision: value.input_revision.get(),
            disposition: match value.disposition {
                PreflightDisposition::Admitted => rpc::PreflightDisposition::PreflightAdmitted,
                PreflightDisposition::Refused => rpc::PreflightDisposition::PreflightRefused,
                PreflightDisposition::Unresolved => rpc::PreflightDisposition::PreflightUnresolved,
            } as i32,
            software_only: value.software_only,
            checks: value
                .checks
                .into_iter()
                .map(|check| rpc::PreflightCheck {
                    code: check.code,
                    message: check.message,
                    evidence: match check.evidence {
                        PreflightEvidence::NotEvaluated => {
                            rpc::PreflightEvidence::PreflightNotEvaluated
                        }
                        PreflightEvidence::Certified => rpc::PreflightEvidence::PreflightCertified,
                        PreflightEvidence::Unsupported => {
                            rpc::PreflightEvidence::PreflightUnsupported
                        }
                        PreflightEvidence::Indeterminate => {
                            rpc::PreflightEvidence::PreflightIndeterminate
                        }
                        PreflightEvidence::Failed => rpc::PreflightEvidence::PreflightFailed,
                    } as i32,
                    required: Some(check.required),
                    occurrence_id: check.occurrence_id.map(|id| id.get()),
                    face_id: check.face_id.map(|id| id.as_str().to_owned()),
                    severity: match check.severity {
                        PreflightSeverity::Error => rpc::PreflightSeverity::PreflightError,
                        PreflightSeverity::Warning => rpc::PreflightSeverity::PreflightWarning,
                        PreflightSeverity::Info => rpc::PreflightSeverity::PreflightInfo,
                    } as i32,
                })
                .collect(),
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
            rpc::OutputDialect::Marlin2SingleExtruderGcodeV1 => {
                Ok(Self::Marlin2SingleExtruderGcodeV1)
            }
            _ => Err("unsupported output dialect".into()),
        }
    }
}
impl TryFrom<OutputDialect> for rpc::OutputDialect {
    type Error = String;
    fn try_from(value: OutputDialect) -> Result<Self, String> {
        match value {
            OutputDialect::CartesianAbsoluteGcodeV1 => Ok(Self::CartesianAbsoluteGcodeV1),
            OutputDialect::Marlin2SingleExtruderGcodeV1 => Ok(Self::Marlin2SingleExtruderGcodeV1),
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
impl TryFrom<rpc::JobWorkflow> for JobWorkflow {
    type Error = String;
    fn try_from(value: rpc::JobWorkflow) -> Result<Self, String> {
        use rpc::job_workflow::Workflow;
        let result = match required(value.workflow, "job workflow selection")? {
            Workflow::DiagnosticBodyOnly(_) => Self::DiagnosticBodyOnly,
            Workflow::CompleteMarlin(job) => {
                if rpc::HomingCoordinateContract::try_from(job.homing_contract)
                    .map_err(|e| e.to_string())?
                    != rpc::HomingCoordinateContract::MarlinNativeCartesian
                {
                    return Err("unsupported homing coordinate contract".into());
                }
                Self::CompleteMarlin {
                    job: SingleExtruderJob {
                        machine_id: job.machine_id,
                        firmware_version: job.firmware_version,
                        configuration_hash: SourceHash::parse(job.configuration_hash)
                            .map_err(|e| e.to_string())?,
                        calibration_record_hash: SourceHash::parse(job.calibration_record_hash)
                            .map_err(|e| e.to_string())?,
                        homing_contract: HomingCoordinateContract::MarlinNativeCartesian,
                        home_position_mm: required(job.home_position_mm, "home position")?
                            .try_into()?,
                        nozzle_temperature_c: job.nozzle_temperature_c,
                        bed_temperature_c: job.bed_temperature_c,
                        minimum_extrusion_temperature_c: job.minimum_extrusion_temperature_c,
                        maximum_nozzle_temperature_c: job.maximum_nozzle_temperature_c,
                        maximum_bed_temperature_c: job.maximum_bed_temperature_c,
                        prime_start_mm: required(job.prime_start_mm, "prime start")?.try_into()?,
                        prime_end_mm: required(job.prime_end_mm, "prime end")?.try_into()?,
                        prime_filament_length_mm: job.prime_filament_length_mm,
                        prime_speed_mm_s: job.prime_speed_mm_s,
                        park_position_mm: required(job.park_position_mm, "park position")?
                            .try_into()?,
                    },
                }
            }
        };
        if let Self::CompleteMarlin { job } = &result {
            job.validate_declaration().map_err(|e| e.to_string())?;
        }
        Ok(result)
    }
}
impl TryFrom<JobWorkflow> for rpc::JobWorkflow {
    type Error = String;
    fn try_from(value: JobWorkflow) -> Result<Self, String> {
        use rpc::job_workflow::Workflow;
        if let JobWorkflow::CompleteMarlin { job } = &value {
            job.validate_declaration().map_err(|e| e.to_string())?;
        }
        Ok(Self {
            workflow: Some(match value {
                JobWorkflow::DiagnosticBodyOnly => {
                    Workflow::DiagnosticBodyOnly(rpc::DiagnosticBodyOnly {})
                }
                JobWorkflow::CompleteMarlin { job } => {
                    Workflow::CompleteMarlin(rpc::SingleExtruderJob {
                        machine_id: job.machine_id,
                        firmware_version: job.firmware_version,
                        configuration_hash: job.configuration_hash.as_str().into(),
                        calibration_record_hash: job.calibration_record_hash.as_str().into(),
                        homing_contract: rpc::HomingCoordinateContract::MarlinNativeCartesian
                            as i32,
                        home_position_mm: Some(job.home_position_mm.try_into()?),
                        nozzle_temperature_c: job.nozzle_temperature_c,
                        bed_temperature_c: job.bed_temperature_c,
                        minimum_extrusion_temperature_c: job.minimum_extrusion_temperature_c,
                        maximum_nozzle_temperature_c: job.maximum_nozzle_temperature_c,
                        maximum_bed_temperature_c: job.maximum_bed_temperature_c,
                        prime_start_mm: Some(job.prime_start_mm.try_into()?),
                        prime_end_mm: Some(job.prime_end_mm.try_into()?),
                        prime_filament_length_mm: job.prime_filament_length_mm,
                        prime_speed_mm_s: job.prime_speed_mm_s,
                        park_position_mm: Some(job.park_position_mm.try_into()?),
                    })
                }
            }),
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
                workflow: required(value.workflow, "job workflow")?.try_into()?,
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
            workflow: Some(value.workflow.try_into()?),
        })
    }
}
impl TryFrom<rpc::SeamPolicy> for SeamPolicy {
    type Error = String;
    fn try_from(value: rpc::SeamPolicy) -> Result<Self, String> {
        use rpc::seam_policy::Policy;
        valid(
            match required(value.policy, "seam policy selection")? {
                Policy::Nearest(_) => Self::Nearest,
                Policy::PreferredBuildDirection(direction) => Self::PreferredBuildDirection {
                    xy: [direction.x, direction.y],
                },
            },
            Self::validate,
        )
    }
}
impl TryFrom<SeamPolicy> for rpc::SeamPolicy {
    type Error = String;
    fn try_from(value: SeamPolicy) -> Result<Self, String> {
        use rpc::seam_policy::Policy;
        value.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            policy: Some(match value {
                SeamPolicy::Nearest => Policy::Nearest(rpc::NearestSeam {}),
                SeamPolicy::PreferredBuildDirection { xy } => {
                    Policy::PreferredBuildDirection(rpc::PreferredBuildDirectionSeam {
                        x: xy[0],
                        y: xy[1],
                    })
                }
            }),
        })
    }
}
impl TryFrom<rpc::RectilinearAxisPolicy> for RectilinearAxisPolicy {
    type Error = String;
    fn try_from(value: rpc::RectilinearAxisPolicy) -> Result<Self, String> {
        match value {
            rpc::RectilinearAxisPolicy::RectilinearX => Ok(Self::X),
            rpc::RectilinearAxisPolicy::RectilinearY => Ok(Self::Y),
            rpc::RectilinearAxisPolicy::AlternatingXy => Ok(Self::AlternatingXY),
            rpc::RectilinearAxisPolicy::AlternatingYx => Ok(Self::AlternatingYX),
            _ => Err("unsupported rectilinear axis policy".into()),
        }
    }
}
impl TryFrom<RectilinearAxisPolicy> for rpc::RectilinearAxisPolicy {
    type Error = String;
    fn try_from(value: RectilinearAxisPolicy) -> Result<Self, String> {
        Ok(match value {
            RectilinearAxisPolicy::X => Self::RectilinearX,
            RectilinearAxisPolicy::Y => Self::RectilinearY,
            RectilinearAxisPolicy::AlternatingXY => Self::AlternatingXy,
            RectilinearAxisPolicy::AlternatingYX => Self::AlternatingYx,
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
                native_shape_error_mm: value.native_shape_error_mm,
                bead_width_mm: value.bead_width_mm,
                perimeter_count: value.perimeter_count,
                infill_fraction: value.infill_fraction,
                print_speed_mm_s: value.print_speed_mm_s,
                travel_speed_mm_s: value.travel_speed_mm_s,
                flow_multiplier: value.flow_multiplier,
                top_skin_thickness_mm: value.top_skin_thickness_mm,
                bottom_skin_thickness_mm: value.bottom_skin_thickness_mm,
                process: required(value.process, "process policy")?.try_into()?,
                supports: required(value.supports, "support policy")?.try_into()?,
                seam_policy: required(value.seam_policy, "seam policy")?.try_into()?,
                rectilinear_axis: rpc::RectilinearAxisPolicy::try_from(value.rectilinear_axis)
                    .map_err(|_| "unknown rectilinear axis policy")?
                    .try_into()?,
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
            native_shape_error_mm: value.native_shape_error_mm,
            bead_width_mm: value.bead_width_mm,
            perimeter_count: value.perimeter_count,
            infill_fraction: value.infill_fraction,
            print_speed_mm_s: value.print_speed_mm_s,
            travel_speed_mm_s: value.travel_speed_mm_s,
            flow_multiplier: value.flow_multiplier,
            top_skin_thickness_mm: value.top_skin_thickness_mm,
            bottom_skin_thickness_mm: value.bottom_skin_thickness_mm,
            process: Some(value.process.try_into()?),
            supports: Some(value.supports.try_into()?),
            seam_policy: Some(value.seam_policy.try_into()?),
            rectilinear_axis: rpc::RectilinearAxisPolicy::try_from(value.rectilinear_axis)? as i32,
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
impl TryFrom<rpc::NativeFeatureCoverageSummary> for NativeFeatureCoverageSummary {
    type Error = String;
    fn try_from(value: rpc::NativeFeatureCoverageSummary) -> Result<Self, String> {
        valid(
            Self {
                disjoint_source_layer_count: required(
                    value.disjoint_source_layer_count,
                    "disjoint source layer count",
                )?,
                invariant_source_union_layer_count: required(
                    value.invariant_source_union_layer_count,
                    "invariant source union layer count",
                )?,
                native_interface_complex_layer_count: required(
                    value.native_interface_complex_layer_count,
                    "native interface complex layer count",
                )?,
                owned_critical_slab_count: required(
                    value.owned_critical_slab_count,
                    "owned critical slab count",
                )?,
            },
            Self::validate,
        )
    }
}
impl TryFrom<NativeFeatureCoverageSummary> for rpc::NativeFeatureCoverageSummary {
    type Error = String;
    fn try_from(value: NativeFeatureCoverageSummary) -> Result<Self, String> {
        value.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            disjoint_source_layer_count: Some(value.disjoint_source_layer_count),
            invariant_source_union_layer_count: Some(value.invariant_source_union_layer_count),
            native_interface_complex_layer_count: Some(value.native_interface_complex_layer_count),
            owned_critical_slab_count: Some(value.owned_critical_slab_count),
        })
    }
}
impl TryFrom<rpc::NativeGeometryCoverageSummary> for NativeGeometryCoverageSummary {
    type Error = String;
    fn try_from(value: rpc::NativeGeometryCoverageSummary) -> Result<Self, String> {
        valid(
            Self {
                requested_shape_error_mm: value.requested_shape_error_mm,
                achieved_shape_error_mm: value.achieved_shape_error_mm,
                height_analysis_count: value.height_analysis_count,
                slab_count: value.slab_count,
                certified_layer_count: value.certified_layer_count,
                event_count: value.event_count,
                geometry_work: value.geometry_work,
                evidence: match rpc::PreflightEvidence::try_from(value.evidence) {
                    Ok(rpc::PreflightEvidence::PreflightCertified) => PreflightEvidence::Certified,
                    _ => return Err("native geometry coverage must be certified".into()),
                },
                source_feature_coverage: required(
                    value.source_feature_coverage,
                    "native source feature coverage",
                )?
                .try_into()?,
                skin_coverage: match rpc::SkinCoverageScope::try_from(value.skin_coverage) {
                    Ok(rpc::SkinCoverageScope::ScheduledMaterialDistance) => {
                        SkinCoverageScope::ScheduledMaterialDistance
                    }
                    _ => return Err("unknown native skin coverage scope".into()),
                },
                support_clearance: match rpc::SupportClearanceScope::try_from(
                    value.support_clearance,
                ) {
                    Ok(rpc::SupportClearanceScope::ContinuousTargetOuterOccupancy) => {
                        SupportClearanceScope::ContinuousTargetOuterOccupancy
                    }
                    _ => return Err("unknown native support clearance scope".into()),
                },
            },
            Self::validate,
        )
    }
}
impl TryFrom<NativeGeometryCoverageSummary> for rpc::NativeGeometryCoverageSummary {
    type Error = String;
    fn try_from(value: NativeGeometryCoverageSummary) -> Result<Self, String> {
        value.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            requested_shape_error_mm: value.requested_shape_error_mm,
            achieved_shape_error_mm: value.achieved_shape_error_mm,
            height_analysis_count: value.height_analysis_count,
            slab_count: value.slab_count,
            certified_layer_count: value.certified_layer_count,
            event_count: value.event_count,
            geometry_work: value.geometry_work,
            evidence: rpc::PreflightEvidence::PreflightCertified as i32,
            source_feature_coverage: Some(value.source_feature_coverage.try_into()?),
            skin_coverage: match value.skin_coverage {
                SkinCoverageScope::ScheduledMaterialDistance => {
                    rpc::SkinCoverageScope::ScheduledMaterialDistance
                }
            } as i32,
            support_clearance: match value.support_clearance {
                SupportClearanceScope::ContinuousTargetOuterOccupancy => {
                    rpc::SupportClearanceScope::ContinuousTargetOuterOccupancy
                }
            } as i32,
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
                support_deposition_segments: value.support_deposition_segments,
                support_deposited_volume_mm3: value.support_deposited_volume_mm3,
                support_filament_length_mm: value.support_filament_length_mm,
                surface_resolution: required(
                    value.surface_resolution,
                    "surface resolution totals",
                )?
                .try_into()?,
                native_geometry: required(value.native_geometry, "native geometry coverage")?
                    .try_into()?,
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
            support_deposition_segments: value.support_deposition_segments,
            support_deposited_volume_mm3: value.support_deposited_volume_mm3,
            support_filament_length_mm: value.support_filament_length_mm,
            surface_resolution: Some(value.surface_resolution.try_into()?),
            native_geometry: Some(value.native_geometry.try_into()?),
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
                lifecycle_motion_count: value.lifecycle_motion_count,
                priming_filament_length_mm: value.priming_filament_length_mm,
                support_deposition_segments: value.support_deposition_segments,
                support_deposited_volume_mm3: value.support_deposited_volume_mm3,
                support_filament_length_mm: value.support_filament_length_mm,
                retraction_count: value.retraction_count,
                retracted_filament_mm: value.retracted_filament_mm,
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
            lifecycle_motion_count: value.lifecycle_motion_count,
            priming_filament_length_mm: value.priming_filament_length_mm,
            support_deposition_segments: value.support_deposition_segments,
            support_deposited_volume_mm3: value.support_deposited_volume_mm3,
            support_filament_length_mm: value.support_filament_length_mm,
            retraction_count: value.retraction_count,
            retracted_filament_mm: value.retracted_filament_mm,
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
    fn consumed_recipe_wire_rejects_missing_unknown_and_invalid_policies() {
        use prost::Message;
        let wire = rpc::ManufacturingIntent::try_from(intent()).unwrap();
        for seam_policy in [
            None,
            Some(rpc::SeamPolicy { policy: None }),
            // Unknown oneof arm is discarded by Prost, never a nearest-policy default.
            Some(rpc::SeamPolicy::decode([0x1a, 0x00].as_slice()).unwrap()),
        ] {
            let mut missing = wire.clone();
            missing.recipe.as_mut().unwrap().seam_policy = seam_policy;
            assert!(ManufacturingIntent::try_from(missing).is_err());
        }
        for rectilinear_axis in [0, -1, i32::MAX] {
            let mut invalid = wire.clone();
            invalid.recipe.as_mut().unwrap().rectilinear_axis = rectilinear_axis;
            assert!(ManufacturingIntent::try_from(invalid).is_err());
        }
        for xy in [
            [0.0, 0.0],
            [2.0, 0.0],
            [f64::NAN, 1.0],
            [1.0, f64::INFINITY],
            [f64::NEG_INFINITY, 0.0],
            [f64::MAX, f64::MAX],
        ] {
            let mut invalid = wire.clone();
            invalid.recipe.as_mut().unwrap().seam_policy = Some(rpc::SeamPolicy {
                policy: Some(rpc::seam_policy::Policy::PreferredBuildDirection(
                    rpc::PreferredBuildDirectionSeam { x: xy[0], y: xy[1] },
                )),
            });
            assert!(ManufacturingIntent::try_from(invalid).is_err(), "{xy:?}");
            let mut invalid = intent();
            invalid.recipe.seam_policy = SeamPolicy::PreferredBuildDirection { xy };
            assert!(
                rpc::ManufacturingIntent::try_from(invalid).is_err(),
                "{xy:?}"
            );
        }
    }

    #[test]
    fn preflight_request_report_and_operation_round_trip_are_checked() {
        let session = SessionId::new();
        let command = ManufacturingCommand::Preflight {
            session_id: session,
            base_revision: ProjectRevision(7),
        };
        let ManufacturingCall::Preflight(request) =
            manufacturing_call(command.clone(), "").unwrap()
        else {
            panic!("wrong preflight dispatch");
        };
        assert_eq!(
            ManufacturingCommand::try_from(request.clone()).unwrap(),
            command
        );
        let mut invalid = request;
        invalid.request_id = "not-a-uuid".into();
        assert!(ManufacturingCommand::try_from(invalid).is_err());
        let report = PreflightReport {
            input_revision: ProjectRevision(7), disposition: PreflightDisposition::Admitted, software_only: true,
            checks: vec![PreflightCheck {
                code: "compile_analysis_required".into(), severity: PreflightSeverity::Info,
                evidence: PreflightEvidence::NotEvaluated, required: false,
                message: "Native layer/support/bridge/offset analysis occurs during Compile; physical qualification absent".into(),
                occurrence_id: Some(crate::geometry::OccurrenceId::new(1).unwrap()),
                face_id: Some(crate::geometry::FaceId::from_step_entity(9007199254740993).unwrap()),
            }],
        };
        let wire = rpc::PreflightReport::try_from(report.clone()).unwrap();
        assert_eq!(PreflightReport::try_from(wire.clone()).unwrap(), report);
        let result = crate::geometry::JobResult::ManufacturingPreflight { report };
        assert_eq!(
            crate::geometry::JobResult::try_from(
                rpc::NativeOperationResult::try_from(result.clone()).unwrap()
            )
            .unwrap(),
            result
        );
        for severity in [0, i32::MAX] {
            let mut invalid = wire.clone();
            invalid.checks[0].severity = severity;
            assert!(PreflightReport::try_from(invalid).is_err());
        }
        for invalid_value in [0, -1, i32::MAX] {
            let mut invalid = wire.clone();
            invalid.disposition = invalid_value;
            assert!(PreflightReport::try_from(invalid).is_err());
            let mut invalid = wire.clone();
            invalid.checks[0].evidence = invalid_value;
            assert!(PreflightReport::try_from(invalid).is_err());
        }
        let mut missing = wire.clone();
        missing.checks[0].required = None;
        assert!(PreflightReport::try_from(missing).is_err());
        let mut unresolved = wire.clone();
        unresolved.checks[0].required = Some(true);
        assert!(PreflightReport::try_from(unresolved.clone()).is_err());
        unresolved.disposition = rpc::PreflightDisposition::PreflightUnresolved as i32;
        let mut domain = PreflightReport::try_from(unresolved.clone()).unwrap();
        domain.disposition = PreflightDisposition::Admitted;
        assert!(rpc::PreflightReport::try_from(domain).is_err());
        for evidence in [
            rpc::PreflightEvidence::PreflightNotEvaluated,
            rpc::PreflightEvidence::PreflightIndeterminate,
            rpc::PreflightEvidence::PreflightCertified,
            rpc::PreflightEvidence::PreflightUnsupported,
            rpc::PreflightEvidence::PreflightFailed,
        ] {
            let disposition = match evidence {
                rpc::PreflightEvidence::PreflightCertified => {
                    rpc::PreflightDisposition::PreflightAdmitted
                }
                rpc::PreflightEvidence::PreflightUnsupported
                | rpc::PreflightEvidence::PreflightFailed => {
                    rpc::PreflightDisposition::PreflightRefused
                }
                _ => rpc::PreflightDisposition::PreflightUnresolved,
            };
            let mut checked = unresolved.clone();
            checked.disposition = disposition as i32;
            checked.checks[0].evidence = evidence as i32;
            let domain = PreflightReport::try_from(checked.clone()).unwrap();
            assert_eq!(rpc::PreflightReport::try_from(domain).unwrap(), checked);
        }
        let mut invalid = wire.clone();
        invalid.checks[0].occurrence_id = Some(0);
        assert!(PreflightReport::try_from(invalid).is_err());
        let mut invalid = wire.clone();
        invalid.checks[0].face_id = Some("step:01".into());
        assert!(PreflightReport::try_from(invalid).is_err());
        let mut invalid = wire;
        invalid.software_only = false;
        assert!(PreflightReport::try_from(invalid).is_err());
    }

    #[test]
    fn intent_round_trip_reapplies_domain_admission() {
        let domain = intent();
        let wire = rpc::ManufacturingIntent::try_from(domain.clone()).unwrap();
        assert_eq!(ManufacturingIntent::try_from(wire.clone()).unwrap(), domain);
        for value in [0.0, -0.001, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut invalid = wire.clone();
            invalid.recipe.as_mut().unwrap().native_shape_error_mm = value;
            assert!(ManufacturingIntent::try_from(invalid).is_err());
            let mut invalid = domain.clone();
            invalid.recipe.native_shape_error_mm = value;
            assert!(rpc::ManufacturingIntent::try_from(invalid).is_err());
        }
        let mut missing = wire.clone();
        missing.recipe.as_mut().unwrap().process = None;
        assert!(ManufacturingIntent::try_from(missing).is_err());
        let mut missing = wire.clone();
        missing.recipe.as_mut().unwrap().supports = None;
        assert!(ManufacturingIntent::try_from(missing).is_err());
        let mut invalid_pwm = wire.clone();
        invalid_pwm
            .recipe
            .as_mut()
            .unwrap()
            .process
            .as_mut()
            .unwrap()
            .bridge_fan_pwm = 256;
        assert!(ManufacturingIntent::try_from(invalid_pwm).is_err());
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
        invalid.recipe.as_mut().unwrap().infill_fraction = 0.0;
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
            output: VerificationOutput::ReportOnly,
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
                support_deposition_segments: 0,
                support_deposited_volume_mm3: 0.0,
                support_filament_length_mm: 0.0,
                surface_resolution: SurfaceResolutionTotals::empty(),
                native_geometry: NativeGeometryCoverageSummary {
                    requested_shape_error_mm: 0.02,
                    achieved_shape_error_mm: 0.001,
                    height_analysis_count: 1,
                    slab_count: 1,
                    certified_layer_count: 1,
                    event_count: 0,
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
        let mut missing = wire.clone();
        missing.summary.as_mut().unwrap().native_geometry = None;
        assert!(ManufacturingArtifactRecord::try_from(missing).is_err());
        let coverage = wire
            .summary
            .as_ref()
            .unwrap()
            .native_geometry
            .as_ref()
            .unwrap()
            .clone();
        for (disjoint, invariant, interface) in [(3, 0, 0), (0, 3, 0), (0, 0, 3), (1, 1, 1)] {
            let mut expected = record.summary.native_geometry.clone();
            expected.certified_layer_count = 3;
            expected.slab_count = 3;
            expected.source_feature_coverage = NativeFeatureCoverageSummary {
                disjoint_source_layer_count: disjoint,
                invariant_source_union_layer_count: invariant,
                native_interface_complex_layer_count: interface,
                owned_critical_slab_count: 1,
            };
            let encoded = rpc::NativeGeometryCoverageSummary::try_from(expected.clone()).unwrap();
            assert_eq!(
                NativeGeometryCoverageSummary::try_from(encoded).unwrap(),
                expected
            );
        }
        let mut missing = coverage.clone();
        missing.source_feature_coverage = None;
        assert!(NativeGeometryCoverageSummary::try_from(missing).is_err());
        for field in 0..4 {
            let mut missing = coverage.clone();
            let feature = missing.source_feature_coverage.as_mut().unwrap();
            match field {
                0 => feature.disjoint_source_layer_count = None,
                1 => feature.invariant_source_union_layer_count = None,
                2 => feature.native_interface_complex_layer_count = None,
                _ => feature.owned_critical_slab_count = None,
            }
            assert!(NativeGeometryCoverageSummary::try_from(missing).is_err());
        }
        for (disjoint, invariant, interface, endpoints) in [
            (0, 0, 0, 0),
            (1, 1, 0, 0),
            (1, 0, 1, 0),
            (1, 0, 0, 2),
            (u32::MAX, 0, 1, 0),
        ] {
            let mut invalid = coverage.clone();
            invalid.source_feature_coverage = Some(rpc::NativeFeatureCoverageSummary {
                disjoint_source_layer_count: Some(disjoint),
                invariant_source_union_layer_count: Some(invariant),
                native_interface_complex_layer_count: Some(interface),
                owned_critical_slab_count: Some(endpoints),
            });
            assert!(NativeGeometryCoverageSummary::try_from(invalid).is_err());
            let mut invalid = record.summary.native_geometry.clone();
            invalid.source_feature_coverage = NativeFeatureCoverageSummary {
                disjoint_source_layer_count: disjoint,
                invariant_source_union_layer_count: invariant,
                native_interface_complex_layer_count: interface,
                owned_critical_slab_count: endpoints,
            };
            assert!(rpc::NativeGeometryCoverageSummary::try_from(invalid).is_err());
        }
        for value in [0, -1, i32::MAX] {
            let mut invalid = coverage.clone();
            invalid.evidence = value;
            assert!(NativeGeometryCoverageSummary::try_from(invalid).is_err());
            let mut invalid = coverage.clone();
            invalid.skin_coverage = value;
            assert!(NativeGeometryCoverageSummary::try_from(invalid).is_err());
            let mut invalid = coverage.clone();
            invalid.support_clearance = value;
            assert!(NativeGeometryCoverageSummary::try_from(invalid).is_err());
        }
        for evidence in [
            rpc::PreflightEvidence::PreflightNotEvaluated,
            rpc::PreflightEvidence::PreflightUnsupported,
            rpc::PreflightEvidence::PreflightIndeterminate,
            rpc::PreflightEvidence::PreflightFailed,
        ] {
            let mut invalid = coverage.clone();
            invalid.evidence = evidence as i32;
            assert!(NativeGeometryCoverageSummary::try_from(invalid).is_err());
        }
        for achieved in [-0.001, 0.021, f64::NAN, f64::INFINITY] {
            let mut invalid = coverage.clone();
            invalid.achieved_shape_error_mm = achieved;
            assert!(NativeGeometryCoverageSummary::try_from(invalid).is_err());
        }
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
            lifecycle_motion_count: 0,
            priming_filament_length_mm: 0.0,
            support_deposition_segments: 0,
            support_deposited_volume_mm3: 0.0,
            support_filament_length_mm: 0.0,
            retraction_count: 0,
            retracted_filament_mm: 0.0,
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

#[cfg(test)]
mod job_wire_tests {
    use super::*;
    #[test]
    fn complete_job_wire_round_trip_and_declaration_rejection() {
        let mut intent = decode_intent(include_bytes!(
            "../../../../fixtures/manufacturing/solid-fill.intent.json"
        ))
        .unwrap();
        let low = intent.printer.build_envelope.min;
        intent.printer.capabilities.output_dialect = OutputDialect::Marlin2SingleExtruderGcodeV1;
        intent.printer.workflow = JobWorkflow::CompleteMarlin {
            job: SingleExtruderJob {
                machine_id: "unqualified-wire-test".into(),
                firmware_version: "2.1.2.5".into(),
                configuration_hash: SourceHash::from_bytes(b"declared test config"),
                calibration_record_hash: SourceHash::from_bytes(b"declared test record"),
                homing_contract: HomingCoordinateContract::MarlinNativeCartesian,
                home_position_mm: low,
                nozzle_temperature_c: 200.0,
                bed_temperature_c: 60.0,
                minimum_extrusion_temperature_c: 170.0,
                maximum_nozzle_temperature_c: 250.0,
                maximum_bed_temperature_c: 100.0,
                prime_start_mm: [low[0] + 1.0, low[1] + 1.0, low[2] + 0.2],
                prime_end_mm: [low[0] + 11.0, low[1] + 1.0, low[2] + 0.2],
                prime_filament_length_mm: 1.0,
                prime_speed_mm_s: 5.0,
                park_position_mm: [low[0], low[1], low[2] + 10.0],
            },
        };
        let wire = rpc::ManufacturingIntent::try_from(intent.clone()).unwrap();
        assert_eq!(ManufacturingIntent::try_from(wire.clone()).unwrap(), intent);
        let mut missing = wire.clone();
        missing.printer.as_mut().unwrap().workflow = None;
        assert!(ManufacturingIntent::try_from(missing).is_err());
        let mut invalid = wire.clone();
        if let Some(rpc::job_workflow::Workflow::CompleteMarlin(job)) = &mut invalid
            .printer
            .as_mut()
            .unwrap()
            .workflow
            .as_mut()
            .unwrap()
            .workflow
        {
            job.homing_contract = 0;
        }
        assert!(ManufacturingIntent::try_from(invalid).is_err());
        let mut invalid = wire;
        if let Some(rpc::job_workflow::Workflow::CompleteMarlin(job)) = &mut invalid
            .printer
            .as_mut()
            .unwrap()
            .workflow
            .as_mut()
            .unwrap()
            .workflow
        {
            job.nozzle_temperature_c = f64::NAN;
        }
        assert!(ManufacturingIntent::try_from(invalid).is_err());
    }
}

impl TryFrom<rpc::SurfaceResolutionTotals> for SurfaceResolutionTotals {
    type Error = String;
    fn try_from(v: rpc::SurfaceResolutionTotals) -> Result<Self, String> {
        let totals = Self {
            evaluated_witnesses: v.evaluated_witnesses,
            resolved: v.resolved,
            ambiguous: v.ambiguous,
            below_nominal_resolution: v.below_nominal_resolution,
            unavailable: v.unavailable,
            coverage_complete: v.coverage_complete,
        };
        totals.validate().map_err(|e| e.to_string())?;
        Ok(totals)
    }
}
impl TryFrom<SurfaceResolutionTotals> for rpc::SurfaceResolutionTotals {
    type Error = String;
    fn try_from(v: SurfaceResolutionTotals) -> Result<Self, String> {
        v.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            evaluated_witnesses: v.evaluated_witnesses,
            resolved: v.resolved,
            ambiguous: v.ambiguous,
            below_nominal_resolution: v.below_nominal_resolution,
            unavailable: v.unavailable,
            coverage_complete: v.coverage_complete,
        })
    }
}
