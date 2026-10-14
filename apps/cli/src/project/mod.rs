// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::{
    Command, Error, Options, geometry,
    output::{OutputDirectory, OutputFile},
    set_once,
};
use spiling_contracts::surface::{
    MAX_SURFACE_DOCUMENT_BYTES, SurfaceFinish, SurfaceLayerId, SurfaceScopeKind,
};
use spiling_contracts::{ArtifactView, geometry::*, manufacturing::*, project::*};
use spiling_engine_client::{EngineClient, prepare_bed_pose as bed_pose};
use std::{env, ffi::OsString, fs::File, io::Read, path::PathBuf, sync::Arc};

/// A failed checkpoint with freshly observed engine state, not an assumed rollback.
#[derive(Debug)]
pub struct SaveFailure {
    pub error: Error,
    pub project: ProjectInfo,
}
impl std::fmt::Display for SaveFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.error, formatter)
    }
}
impl std::error::Error for SaveFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.error.as_ref())
    }
}
/// A recoverable manufacturing failure with authoritative status before child cleanup.
#[derive(Debug)]
pub struct ManufacturingFailure {
    pub error: Error,
    pub project: ProjectInfo,
    pub manufacturing: serde_json::Value,
}
impl std::fmt::Display for ManufacturingFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.error, formatter)
    }
}
impl std::error::Error for ManufacturingFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.error.as_ref())
    }
}

pub struct Workflow {
    mode: String,
    path: NativePath,
    read_only: bool,
    operations: Vec<Operation>,
    inspections: Vec<(OccurrenceId, FaceId)>,
}
enum Operation {
    Import(NativePath),
    Add(DefinitionId, RigidPoseMm),
    Pose(OccurrenceId, RigidPoseMm),
    PrepareBed(OccurrenceId, bool),
    Remove(OccurrenceId),
    SurfaceFinish(OccurrenceId, FaceId, SurfaceScopeKind, SurfaceFinish),
    RemoveSurfaceFinish(SurfaceLayerId),
    SurfaceFinishEnabled(SurfaceLayerId, bool),
    Intent(Arc<ManufacturingIntent>),
    Preflight,
    Compile,
    Verify,
    ExportManufacturing,
    ExportReplay,
    Undo,
    Redo,
    Save(Option<NativePath>),
}
fn text(args: &mut impl Iterator<Item = OsString>, name: &str) -> Result<String, Error> {
    args.next()
        .ok_or_else(|| format!("{name} requires a value"))?
        .into_string()
        .map_err(|_| format!("{name} requires UTF-8").into())
}
fn path(args: &mut impl Iterator<Item = OsString>, name: &str) -> Result<NativePath, Error> {
    let value = args
        .next()
        .ok_or_else(|| format!("{name} requires a native path"))?;
    Ok(NativePath::from_os_str(&value)?)
}
fn intent(args: &mut impl Iterator<Item = OsString>) -> Result<ManufacturingIntent, Error> {
    let path = args.next().ok_or("--intent requires a native JSON path")?;
    NativePath::from_os_str(&path)?;
    let mut bytes = Vec::new();
    File::open(PathBuf::from(path))?
        .take(MAX_PROFILE_JSON_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_PROFILE_JSON_BYTES as usize {
        return Err("manufacturing intent exceeds the profile JSON byte limit".into());
    }
    // The service owns specification validation; this is only bounded inert decoding.
    Ok(serde_json::from_slice(&bytes)?)
}
pub fn parse(
    mut args: impl Iterator<Item = OsString>,
    mut engine: Option<PathBuf>,
    mut store: Option<PathBuf>,
) -> Result<Option<Options>, Error> {
    let mode = text(&mut args, "project")?;
    if !matches!(
        mode.as_str(),
        "create" | "edit" | "save" | "open" | "inspect" | "recover"
    ) {
        return Err("project requires create, edit, save, open, inspect or recover".into());
    }
    let target = path(&mut args, &mode)?;
    let mut read_only = mode == "inspect";
    let mut operations = Vec::new();
    let mut plane = None;
    let mut output = None;
    let mut manufacturing_output = None;
    let mut replay_output = None;
    let mut inspections = Vec::new();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--help" | "-h") => return Ok(None),
            Some("--engine") => set_once(
                &mut engine,
                PathBuf::from(args.next().ok_or("--engine requires a path")?),
                "--engine",
            )?,
            Some("--store") => set_once(
                &mut store,
                PathBuf::from(args.next().ok_or("--store requires a native path")?),
                "--store",
            )?,
            Some("--read-only") if !read_only => read_only = true,
            Some("--section") => set_once(
                &mut plane,
                geometry::parse_plane(&text(&mut args, "--section")?)?,
                "--section",
            )?,
            Some("--out") => set_once(
                &mut output,
                PathBuf::from(args.next().ok_or("--out requires a path")?),
                "--out",
            )?,
            Some("--inspect-face") => {
                let occurrence = OccurrenceId::new(text(&mut args, "--inspect-face")?.parse()?)?;
                let face = FaceId::parse(text(&mut args, "--inspect-face face")?)?;
                if inspections.len() >= MAX_NATIVE_FACES as usize {
                    return Err("face inspection limit exceeded".into());
                }
                inspections.push((occurrence, face));
            }
            Some("--surface-finish") => {
                let occurrence =
                    OccurrenceId::new(text(&mut args, "--surface-finish occurrence")?.parse()?)?;
                let face = FaceId::parse(text(&mut args, "--surface-finish face")?)?;
                let scope = match text(&mut args, "--surface-finish scope")?.as_str() {
                    "definition" => SurfaceScopeKind::Definition,
                    "occurrence" => SurfaceScopeKind::Occurrence,
                    _ => {
                        return Err(
                            "--surface-finish scope must be definition or occurrence".into()
                        );
                    }
                };
                let json = text(&mut args, "--surface-finish finish JSON")?;
                if json.len() > MAX_SURFACE_DOCUMENT_BYTES {
                    return Err("surface finish JSON exceeds the document byte limit".into());
                }
                let finish: SurfaceFinish = serde_json::from_str(&json)?;
                finish.validate()?;
                operations.push(Operation::SurfaceFinish(occurrence, face, scope, finish));
            }
            Some("--remove-surface-finish") => {
                operations.push(Operation::RemoveSurfaceFinish(SurfaceLayerId::new(
                    text(&mut args, "--remove-surface-finish")?.parse()?,
                )?));
            }
            Some("--surface-finish-enabled") => {
                let layer = SurfaceLayerId::new(
                    text(&mut args, "--surface-finish-enabled layer")?.parse()?,
                )?;
                let enabled = match text(&mut args, "--surface-finish-enabled boolean")?.as_str() {
                    "true" => true,
                    "false" => false,
                    _ => return Err("--surface-finish-enabled requires true or false".into()),
                };
                operations.push(Operation::SurfaceFinishEnabled(layer, enabled));
            }
            Some("--intent") => operations.push(Operation::Intent(intent(&mut args)?.into())),
            Some("--preflight") => operations.push(Operation::Preflight),
            Some("--place-on-bed" | "--center-on-bed") => {
                let center = arg == "--center-on-bed";
                let id =
                    OccurrenceId::new(text(&mut args, "--place-on-bed/--center-on-bed")?.parse()?)?;
                operations.push(Operation::PrepareBed(id, center));
            }
            Some("--compile") => operations.push(Operation::Compile),
            Some("--verify") => operations.push(Operation::Verify),
            Some("--manufacturing-out") => {
                let value = args
                    .next()
                    .ok_or("--manufacturing-out requires a native path")?;
                NativePath::from_os_str(&value)?;
                set_once(
                    &mut manufacturing_output,
                    PathBuf::from(value),
                    "--manufacturing-out",
                )?;
                operations.push(Operation::ExportManufacturing);
            }
            Some("--replay-out") => {
                if !matches!(mode.as_str(), "create" | "inspect") {
                    return Err("--replay-out is valid only with project create or inspect".into());
                }
                let value = args
                    .next()
                    .ok_or("--replay-out requires a native file path")?;
                NativePath::from_os_str(&value)?;
                set_once(&mut replay_output, PathBuf::from(value), "--replay-out")?;
                operations.push(Operation::ExportReplay);
            }
            Some("--import") => operations.push(Operation::Import(path(&mut args, "--import")?)),
            Some("--add") => {
                let id = DefinitionId::parse(text(&mut args, "--add")?)?;
                let pose: RigidPoseMm = serde_json::from_str(&text(&mut args, "--add pose")?)?;
                operations.push(Operation::Add(id, pose));
            }
            Some("--pose") => {
                let id = OccurrenceId::new(text(&mut args, "--pose")?.parse()?)?;
                let pose: RigidPoseMm = serde_json::from_str(&text(&mut args, "--pose pose")?)?;
                operations.push(Operation::Pose(id, pose));
            }
            Some("--remove") => operations.push(Operation::Remove(OccurrenceId::new(
                text(&mut args, "--remove")?.parse()?,
            )?)),
            Some("--undo") => operations.push(Operation::Undo),
            Some("--redo") => operations.push(Operation::Redo),
            Some("--save") => operations.push(Operation::Save(None)),
            Some("--save-as") => {
                operations.push(Operation::Save(Some(path(&mut args, "--save-as")?)))
            }
            _ => return Err(format!("unrecognized project argument {arg:?}").into()),
        }
        if operations.len() > 256 {
            return Err("project workflow exceeds 256 operations".into());
        }
    }
    if read_only
        && (matches!(mode.as_str(), "create" | "edit" | "save")
            || operations.iter().any(|operation| {
                !matches!(
                    operation,
                    Operation::Preflight
                        | Operation::Verify
                        | Operation::ExportManufacturing
                        | Operation::ExportReplay
                )
            }))
    {
        return Err(
            "read-only inspection cannot contain authoring, compile or save operations".into(),
        );
    }
    if mode == "create" {
        if operations
            .iter()
            .any(|operation| matches!(operation, Operation::Save(Some(_))))
        {
            return Err("create uses its PATH destination, not --save-as".into());
        }
        // First checkpoint attaches the destination; later saves use its held writer lock.
        let first_save = operations
            .iter_mut()
            .find(|operation| matches!(operation, Operation::Save(_)));
        match first_save {
            Some(Operation::Save(target_slot)) if target_slot.is_none() => {
                *target_slot = Some(target.clone())
            }
            Some(_) => return Err("create uses its PATH destination, not --save-as".into()),
            None => operations.push(Operation::Save(Some(target.clone()))),
        }
    }
    if matches!(mode.as_str(), "create" | "edit" | "save")
        && !matches!(operations.last(), Some(Operation::Save(_)))
    {
        operations.push(Operation::Save(None));
    }
    Ok(Some(Options {
        command: Command::Project {
            workflow: Workflow {
                mode,
                path: target,
                read_only,
                operations,
                inspections,
            },
            plane,
            output,
            manufacturing_output,
            replay_output,
        },
        engine: engine.unwrap_or(
            env::current_exe()?
                .with_file_name(format!("spiling-engine{}", env::consts::EXE_SUFFIX)),
        ),
        store,
    }))
}
async fn info(client: &mut EngineClient) -> Result<ProjectInfo, Error> {
    match client
        .project(ProjectCommand::Get {
            session_id: client.hello().session_id.clone(),
        })
        .await?
    {
        ProjectResponse::Status { info } => Ok(info),
        _ => Err("unexpected project status response".into()),
    }
}
async fn complete(client: &mut EngineClient, response: ProjectResponse) -> Result<(), Error> {
    match response {
        ProjectResponse::SceneChanged { .. } | ProjectResponse::Status { .. } => Ok(()),
        ProjectResponse::OperationAccepted { operation } => {
            let session = client.hello().session_id.clone();
            match geometry::wait_operation(client, &session, operation).await? {
                JobResult::Scene { .. } | JobResult::ProjectSaved { .. } => Ok(()),
                _ => Err("unexpected project job result".into()),
            }
        }
        ProjectResponse::Error { error } => {
            Err(spiling_engine_client::ClientError::Project(error).into())
        }
    }
}
async fn manufacturing_status(
    client: &mut EngineClient,
    include_intent: bool,
) -> Result<serde_json::Value, Error> {
    match client
        .manufacturing(ManufacturingCommand::Get {
            session_id: client.hello().session_id.clone(),
        })
        .await?
    {
        ManufacturingResponse::Status {
            intent, artifact, ..
        } => {
            let mut status = serde_json::json!({
                "artifact": artifact, "software_only": true, "not_machine_ready": true
            });
            // Ordered history retains artifact evidence, not 256 copies of the profile.
            if include_intent {
                status["intent"] = serde_json::to_value(intent)?;
            } else {
                status["intent_present"] = serde_json::json!(intent.is_some());
            }
            Ok(status)
        }
        ManufacturingResponse::Error { error } => {
            Err(spiling_engine_client::ClientError::Manufacturing(error).into())
        }
        _ => Err("unexpected manufacturing status response".into()),
    }
}
struct FreshInspection {
    record: ManufacturingArtifactRecord,
    report: VerificationReport,
    resource: ArtifactView,
    provenance: ManufacturingInspectionProvenance,
    replay_display: Option<ReplayDisplayDescriptor>,
}
async fn verify(
    client: &mut EngineClient,
    output: VerificationOutput,
) -> Result<FreshInspection, Error> {
    let session = client.hello().session_id.clone();
    let base_revision = info(client).await?.revision;
    match client
        .manufacturing(ManufacturingCommand::Inspect {
            session_id: session.clone(),
            base_revision,
            output,
        })
        .await?
    {
        ManufacturingResponse::Verified {
            record,
            report,
            resource,
            provenance,
            replay_display,
        } => Ok(FreshInspection {
            record,
            report,
            resource,
            provenance,
            replay_display,
        }),
        ManufacturingResponse::OperationAccepted { operation } => {
            match geometry::wait_operation(client, &session, operation).await? {
                JobResult::ManufacturingVerified {
                    record,
                    report,
                    resource,
                    provenance,
                    replay_display,
                } => Ok(FreshInspection {
                    record,
                    report,
                    resource,
                    provenance,
                    replay_display,
                }),
                _ => Err("unexpected manufacturing verification job result".into()),
            }
        }
        ManufacturingResponse::Error { error } => {
            Err(spiling_engine_client::ClientError::Manufacturing(error).into())
        }
        _ => Err("unexpected manufacturing verification response".into()),
    }
}

async fn preflight(client: &mut EngineClient) -> Result<PreflightReport, Error> {
    let session = client.hello().session_id.clone();
    let base_revision = info(client).await?.revision;
    match client
        .manufacturing(ManufacturingCommand::Preflight {
            session_id: session.clone(),
            base_revision,
        })
        .await?
    {
        ManufacturingResponse::Preflight { report } => Ok(report),
        ManufacturingResponse::OperationAccepted { operation } => {
            match geometry::wait_operation(client, &session, operation).await? {
                JobResult::ManufacturingPreflight { report } => Ok(report),
                _ => Err("unexpected manufacturing preflight job result".into()),
            }
        }
        ManufacturingResponse::Error { error } => {
            Err(spiling_engine_client::ClientError::Manufacturing(error).into())
        }
        _ => Err("unexpected manufacturing preflight response".into()),
    }
}

pub async fn run(
    client: &mut EngineClient,
    workflow: Workflow,
    plane: Option<PlaneMm>,
    output: Option<&mut OutputDirectory>,
    manufacturing_output: Option<&mut OutputDirectory>,
    replay_output: Option<&mut OutputFile>,
) -> Result<serde_json::Value, Error> {
    match execute(
        client,
        workflow,
        plane,
        output,
        manufacturing_output,
        replay_output,
    )
    .await
    {
        Ok(report) => Ok(report),
        Err(error)
            if matches!(
                error.downcast_ref::<spiling_engine_client::ClientError>(),
                Some(spiling_engine_client::ClientError::Manufacturing(_))
            ) =>
        {
            let project = info(client).await?;
            let manufacturing = manufacturing_status(client, true).await?;
            Err(Box::new(ManufacturingFailure {
                error,
                project,
                manufacturing,
            }))
        }
        Err(error) => Err(error),
    }
}
async fn execute(
    client: &mut EngineClient,
    workflow: Workflow,
    plane: Option<PlaneMm>,
    output: Option<&mut OutputDirectory>,
    mut manufacturing_output: Option<&mut OutputDirectory>,
    mut replay_output: Option<&mut OutputFile>,
) -> Result<serde_json::Value, Error> {
    let session = client.hello().session_id.clone();
    let summary = geometry::scene(client, &session).await?;
    let response = if workflow.mode == "create" {
        client
            .project(ProjectCommand::New {
                session_id: session.clone(),
                base_revision: summary.revision,
                discard_changes: false,
            })
            .await?
    } else {
        client
            .project(ProjectCommand::Open {
                session_id: session.clone(),
                base_revision: summary.revision,
                path: workflow.path,
                read_only: workflow.read_only,
                recover_previous: workflow.mode == "recover",
                discard_changes: false,
            })
            .await?
    };
    complete(client, response).await?;
    let mut history = vec![serde_json::json!({
        "operation": workflow.mode,
        "project": info(client).await?,
        "manufacturing": manufacturing_status(client, false).await?
    })];
    let mut verification = None;
    let mut export_evidence = None;
    let mut replay_evidence = None;
    let mut preflight_report = None;
    for operation in workflow.operations {
        let base_revision = geometry::scene(client, &session).await?.revision;
        let label;
        match operation {
            Operation::SurfaceFinish(occurrence_id, face_id, scope, finish) => {
                label = "surface_finish";
                let summary = geometry::scene(client, &session).await?;
                let (_, occurrences) = geometry::records(client, &summary).await?;
                let occurrence = occurrences
                    .iter()
                    .find(|row| row.occurrence_id == occurrence_id)
                    .ok_or("surface target occurrence is not in the current project")?;
                let face_ref = FaceRef {
                    session_id: session.clone(),
                    scene_revision: summary.revision,
                    occurrence_id,
                    definition_id: occurrence.definition_id.clone(),
                    face_id,
                };
                match geometry::control(
                    client,
                    GeometryCommand::InspectFace {
                        reference: face_ref.clone(),
                    },
                )
                .await?
                {
                    GeometryResponse::FaceInspection { inspection }
                        if inspection.reference == face_ref => {}
                    _ => return Err("surface target native face inspection mismatch".into()),
                }
                let base_revision = info(client).await?.revision;
                let response = client
                    .project(ProjectCommand::AddSurfaceFinish {
                        session_id: session.clone(),
                        base_revision,
                        face_ref,
                        scope,
                        finish,
                    })
                    .await?;
                complete(client, response).await?;
            }
            Operation::RemoveSurfaceFinish(layer_id) => {
                label = "remove_surface_finish";
                let base_revision = info(client).await?.revision;
                let response = client
                    .project(ProjectCommand::RemoveSurfaceFinish {
                        session_id: session.clone(),
                        base_revision,
                        layer_id,
                    })
                    .await?;
                complete(client, response).await?;
            }
            Operation::SurfaceFinishEnabled(layer_id, enabled) => {
                label = "surface_finish_enabled";
                let base_revision = info(client).await?.revision;
                let response = client
                    .project(ProjectCommand::SetSurfaceFinishEnabled {
                        session_id: session.clone(),
                        base_revision,
                        layer_id,
                        enabled,
                    })
                    .await?;
                complete(client, response).await?;
            }
            Operation::Intent(intent) => {
                label = "intent";
                let base_revision = info(client).await?.revision;
                match client
                    .manufacturing(ManufacturingCommand::SetIntent {
                        session_id: session.clone(),
                        base_revision,
                        intent,
                    })
                    .await?
                {
                    ManufacturingResponse::Status { .. } => {}
                    ManufacturingResponse::Error { error } => {
                        return Err(spiling_engine_client::ClientError::Manufacturing(error).into());
                    }
                    _ => return Err("unexpected set manufacturing intent response".into()),
                }
            }
            Operation::Preflight => {
                label = "preflight";
                preflight_report = Some(preflight(client).await?);
            }
            Operation::Compile => {
                label = "compile";
                let base_revision = info(client).await?.revision;
                match client
                    .manufacturing(ManufacturingCommand::Compile {
                        session_id: session.clone(),
                        base_revision,
                    })
                    .await?
                {
                    ManufacturingResponse::OperationAccepted { operation } => {
                        if !matches!(
                            geometry::wait_operation(client, &session, operation).await?,
                            JobResult::ManufacturingCompiled { .. }
                        ) {
                            return Err("unexpected manufacturing compile job result".into());
                        }
                    }
                    ManufacturingResponse::Error { error } => {
                        return Err(spiling_engine_client::ClientError::Manufacturing(error).into());
                    }
                    _ => return Err("unexpected manufacturing compile response".into()),
                }
            }
            Operation::Verify | Operation::ExportManufacturing => {
                let export = matches!(operation, Operation::ExportManufacturing);
                label = if export {
                    "manufacturing_export"
                } else {
                    "verify"
                };
                let inspection = verify(client, VerificationOutput::ReportOnly).await?;
                let FreshInspection {
                    record,
                    report,
                    resource,
                    provenance,
                    replay_display,
                } = inspection;
                if !report.verified {
                    return Err("manufacturing replay did not verify the emitted program".into());
                }
                if export {
                    let bundle = client
                        .fetch_manufacturing_bundle(&record, &resource)
                        .await?;
                    manufacturing_output
                        .as_deref_mut()
                        .ok_or("manufacturing export directory was not admitted")?
                        .write_manufacturing_bundle(&bundle, &report)?;
                    export_evidence = Some(serde_json::json!({
                        "artifact": record, "verification": report,
                        "software_only": true, "not_machine_ready": true
                    }));
                }
                verification = Some(FreshInspection {
                    record,
                    report,
                    resource,
                    provenance,
                    replay_display,
                });
            }
            Operation::ExportReplay => {
                label = "replay_export";
                let inspection = verify(client, VerificationOutput::ReportAndMotion).await?;
                if !inspection.report.verified {
                    return Err("manufacturing replay did not verify the emitted program".into());
                }
                let descriptor = inspection
                    .replay_display
                    .as_ref()
                    .ok_or("motion inspection completed without its replay descriptor")?;
                let bytes = client
                    .fetch_replay_display(&inspection.record, descriptor)
                    .await?;
                replay_output
                    .as_deref_mut()
                    .ok_or("replay output file was not admitted")?
                    .write(&bytes)?;
                replay_evidence = Some(serde_json::json!({
                    "artifact": inspection.record,
                    "verification": inspection.report,
                    "provenance": inspection.provenance,
                    "replay_display": descriptor,
                    "software_only": true, "not_machine_ready": true
                }));
                verification = Some(inspection);
            }
            Operation::Import(source) => {
                label = "import";
                let response = geometry::control(
                    client,
                    GeometryCommand::ImportPart {
                        session_id: session.clone(),
                        base_revision,
                        source,
                        initial_pose: RigidPoseMm::IDENTITY,
                    },
                )
                .await?;
                let GeometryResponse::OperationAccepted { operation } = response else {
                    return Err("unexpected import response".into());
                };
                if !matches!(
                    geometry::wait_operation(client, &session, operation).await?,
                    JobResult::Scene { .. }
                ) {
                    return Err("unexpected import result".into());
                }
            }
            Operation::Add(definition_id, pose) => {
                label = "add";
                let response = geometry::control(
                    client,
                    GeometryCommand::AddInstance {
                        session_id: session.clone(),
                        base_revision,
                        definition_id,
                        pose,
                    },
                )
                .await?;
                if !matches!(response, GeometryResponse::SceneChanged { .. }) {
                    return Err("unexpected add response".into());
                }
            }
            Operation::Pose(occurrence_id, pose) => {
                label = "pose";
                let response = geometry::control(
                    client,
                    GeometryCommand::SetInstancePose {
                        session_id: session.clone(),
                        base_revision,
                        occurrence_id,
                        pose,
                    },
                )
                .await?;
                if !matches!(response, GeometryResponse::SceneChanged { .. }) {
                    return Err("unexpected pose response".into());
                }
            }
            Operation::PrepareBed(occurrence_id, center) => {
                label = if center {
                    "center_on_bed"
                } else {
                    "place_on_bed"
                };
                let summary = geometry::scene(client, &session).await?;
                let (definitions, occurrences) = geometry::records(client, &summary).await?;
                let occurrence = occurrences
                    .iter()
                    .find(|row| row.occurrence_id == occurrence_id)
                    .ok_or("bed preparation occurrence not in project")?;
                let definition = definitions
                    .iter()
                    .find(|row| row.definition_id == occurrence.definition_id)
                    .ok_or("bed preparation definition not in project")?;
                let intent = match client.manufacturing(ManufacturingCommand::Get { session_id: session.clone() }).await? {
                    ManufacturingResponse::Status { intent: Some(intent), .. } => intent,
                    ManufacturingResponse::Status { intent: None, .. } => return Err("bed preparation requires --intent with a complete explicit printer/recipe profile first".into()),
                    ManufacturingResponse::Error { error } => return Err(spiling_engine_client::ClientError::Manufacturing(error).into()),
                    _ => return Err("unexpected manufacturing status response".into()),
                };
                let pose = bed_pose(
                    definition.bounds_mm,
                    occurrence.pose,
                    intent.printer.build_envelope,
                    center,
                )?;
                let response = geometry::control(
                    client,
                    GeometryCommand::SetInstancePose {
                        session_id: session.clone(),
                        base_revision: summary.revision,
                        occurrence_id,
                        pose,
                    },
                )
                .await?;
                if !matches!(response, GeometryResponse::SceneChanged { .. }) {
                    return Err("unexpected bed preparation pose response".into());
                }
            }
            Operation::Remove(occurrence_id) => {
                label = "remove";
                let response = geometry::control(
                    client,
                    GeometryCommand::RemoveInstance {
                        session_id: session.clone(),
                        base_revision,
                        occurrence_id,
                    },
                )
                .await?;
                if !matches!(response, GeometryResponse::SceneChanged { .. }) {
                    return Err("unexpected remove response".into());
                }
            }
            Operation::Undo | Operation::Redo => {
                let command = if matches!(operation, Operation::Undo) {
                    label = "undo";
                    ProjectCommand::Undo {
                        session_id: session.clone(),
                        base_revision,
                    }
                } else {
                    label = "redo";
                    ProjectCommand::Redo {
                        session_id: session.clone(),
                        base_revision,
                    }
                };
                let response = client.project(command).await?;
                complete(client, response).await?;
            }
            Operation::Save(target) => {
                label = "save";
                let result: Result<(), Error> = async {
                    let response = client
                        .project(ProjectCommand::Save {
                            session_id: session.clone(),
                            base_revision,
                            target,
                        })
                        .await?;
                    complete(client, response).await
                }
                .await;
                if let Err(error) = result {
                    if error
                        .downcast_ref::<spiling_engine_client::ClientError>()
                        .is_some_and(|error| !error.is_fatal())
                    {
                        let project = info(client).await?;
                        return Err(Box::new(SaveFailure { error, project }));
                    }
                    return Err(error);
                }
            }
        }
        let manufacturing = manufacturing_status(client, false).await?;
        if verification.as_ref().is_some_and(|inspection| {
            manufacturing["artifact"]["hash"].as_str() != Some(inspection.record.hash.as_str())
        }) {
            verification = None;
        }
        history.push(serde_json::json!({
            "operation": label, "project": info(client).await?, "manufacturing": manufacturing,
            "preflight": if label == "preflight" { preflight_report.as_ref() } else { None }
        }));
    }
    let summary = geometry::scene(client, &session).await?;
    let (_, occurrences) = geometry::records(client, &summary).await?;
    let mut inspections = Vec::new();
    for (occurrence_id, face_id) in workflow.inspections {
        let occurrence = occurrences
            .iter()
            .find(|row| row.occurrence_id == occurrence_id)
            .ok_or("inspection occurrence not in project")?;
        let reference = FaceRef {
            session_id: session.clone(),
            scene_revision: summary.revision,
            occurrence_id,
            definition_id: occurrence.definition_id.clone(),
            face_id,
        };
        match geometry::control(
            client,
            GeometryCommand::InspectFace {
                reference: reference.clone(),
            },
        )
        .await?
        {
            GeometryResponse::FaceInspection { inspection } => {
                inspection.face.validate()?;
                inspection.provenance.validate()?;
                inspection.pose.validate()?;
                if inspection.reference != reference || inspection.pose != occurrence.pose {
                    return Err("face inspection reference/placement mismatch".into());
                }
                inspections.push(inspection);
            }
            _ => return Err("unexpected face inspection response".into()),
        }
    }
    let mut report = geometry::inspect(client, plane, output).await?;
    report["command"] = serde_json::json!("project");
    report["project"] = serde_json::to_value(info(client).await?)?;
    report["face_inspections"] = serde_json::to_value(inspections)?;
    report["operations"] = serde_json::json!(history);
    report["manufacturing"] = manufacturing_status(client, true).await?;
    if let Some(preflight) = preflight_report {
        report["manufacturing"]["preflight"] = serde_json::to_value(preflight)?;
    }
    if let Some(inspection) = verification {
        report["manufacturing"]["verified_artifact_hash"] =
            serde_json::json!(inspection.record.hash);
        report["manufacturing"]["verification"] = serde_json::to_value(inspection.report)?;
        report["manufacturing"]["provenance"] = serde_json::to_value(inspection.provenance)?;
        report["manufacturing"]["replay_display"] =
            serde_json::to_value(inspection.replay_display)?;
    }
    if let Some(export_evidence) = export_evidence {
        report["manufacturing_export"] = export_evidence;
    }
    if let Some(replay_evidence) = replay_evidence {
        report["manufacturing_replay"] = replay_evidence;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse_args(args: &[&str]) -> Result<Option<Options>, Error> {
        parse(args.iter().map(OsString::from), None, None)
    }
    #[test]
    fn authoring_is_validated_before_child_spawn() {
        assert!(parse_args(&["open", "project", "--pose", "0", "{}"]).is_err());
        assert!(
            parse_args(&[
                "open",
                "project",
                "--pose",
                "1",
                r#"{"translation_mm":[0,0,0],"rotation_xyzw":[0,0,0,0]}"#
            ])
            .is_err()
        );
        assert!(parse_args(&["inspect", "project", "--undo"]).is_err());
        assert!(parse_args(&["open", "project", "--add", "bad", "{}"]).is_err());
        assert!(parse_args(&["create", "project", "--section", "0,0,0:0,0,0"]).is_err());
    }
    #[test]
    fn surface_flags_are_ordered_typed_and_rejected_read_only_before_spawn() {
        let options = parse_args(&[
            "open",
            "project",
            "--surface-finish",
            "1",
            "step:1",
            "definition",
            r#"{"kind":"smooth"}"#,
            "--surface-finish-enabled",
            "1",
            "false",
            "--remove-surface-finish",
            "1",
        ])
        .unwrap()
        .unwrap();
        let Command::Project { workflow, .. } = options.command else {
            panic!("project");
        };
        assert!(matches!(
            workflow.operations.as_slice(),
            [
                Operation::SurfaceFinish(_, _, SurfaceScopeKind::Definition, SurfaceFinish::Smooth),
                Operation::SurfaceFinishEnabled(_, false),
                Operation::RemoveSurfaceFinish(_),
            ]
        ));
        let options = parse_args(&[
            "open",
            "project",
            "--surface-finish",
            "1",
            "step:1",
            "occurrence",
            r#"{"kind":"cylindrical_ribs","depth_mm":0.1,"rib_count":12,"phase_fraction":0.25}"#,
        ])
        .unwrap()
        .unwrap();
        let Command::Project { workflow, .. } = options.command else {
            panic!("project");
        };
        assert!(matches!(
            workflow.operations.as_slice(),
            [Operation::SurfaceFinish(
                _,
                _,
                SurfaceScopeKind::Occurrence,
                SurfaceFinish::CylindricalRibs { rib_count: 12, .. }
            ),]
        ));
        for flags in [
            vec![
                "--surface-finish",
                "1",
                "step:1",
                "occurrence",
                r#"{"kind":"smooth"}"#,
            ],
            vec!["--surface-finish-enabled", "1", "true"],
            vec!["--remove-surface-finish", "1"],
        ] {
            let mut args = vec!["open", "project"];
            args.extend(flags);
            args.push("--read-only");
            assert!(parse_args(&args).is_err());
        }
        for flags in [
            vec![
                "--surface-finish",
                "0",
                "step:1",
                "occurrence",
                r#"{"kind":"smooth"}"#,
            ],
            vec![
                "--surface-finish",
                "1",
                "step:1",
                "both",
                r#"{"kind":"smooth"}"#,
            ],
            vec![
                "--surface-finish",
                "1",
                "step:1",
                "occurrence",
                r#"{"kind":"vertical_ribs","depth_mm":0,"pitch_mm":2,"edge_margin_mm":0.8,"phase_mm":0}"#,
            ],
            vec![
                "--surface-finish",
                "1",
                "step:1",
                "occurrence",
                r#"{"kind":"cylindrical_ribs","depth_mm":0.1,"rib_count":1,"phase_fraction":0}"#,
            ],
            vec![
                "--surface-finish",
                "1",
                "step:1",
                "occurrence",
                r#"{"kind":"cylindrical_ribs","depth_mm":0.1,"rib_count":12,"phase_fraction":1}"#,
            ],
            vec!["--surface-finish-enabled", "1", "yes"],
            vec!["--remove-surface-finish", "0"],
        ] {
            let mut args = vec!["open", "project"];
            args.extend(flags);
            assert!(parse_args(&args).is_err());
        }
    }
    #[test]
    fn manufacturing_flags_are_ordered_and_read_only_replay_is_allowed() {
        let options = parse_args(&[
            "open",
            "project",
            "--read-only",
            "--verify",
            "--manufacturing-out",
            "new-output",
        ])
        .unwrap()
        .unwrap();
        let Command::Project {
            workflow,
            manufacturing_output,
            ..
        } = options.command
        else {
            panic!("expected project command");
        };
        assert!(matches!(
            workflow.operations.as_slice(),
            [Operation::Verify, Operation::ExportManufacturing]
        ));
        assert_eq!(manufacturing_output.unwrap(), PathBuf::from("new-output"));
        assert!(parse_args(&["inspect", "project", "--compile"]).is_err());
        assert!(parse_args(&["open", "project", "--compile", "--read-only"]).is_err());
        assert!(
            parse_args(&[
                "inspect",
                "project",
                "--manufacturing-out",
                "one",
                "--manufacturing-out",
                "two",
            ])
            .is_err()
        );
        let options = parse_args(&["edit", "project", "--compile", "--verify"])
            .unwrap()
            .unwrap();
        let Command::Project { workflow, .. } = options.command else {
            panic!("expected project command");
        };
        assert!(matches!(
            workflow.operations.as_slice(),
            [Operation::Compile, Operation::Verify, Operation::Save(None)]
        ));
        let options = parse_args(&["inspect", "project", "--replay-out", "motion.splr"])
            .unwrap()
            .unwrap();
        let Command::Project {
            workflow,
            replay_output,
            ..
        } = options.command
        else {
            panic!("expected project command")
        };
        assert!(matches!(
            workflow.operations.as_slice(),
            [Operation::ExportReplay]
        ));
        assert_eq!(replay_output.unwrap(), PathBuf::from("motion.splr"));
        assert!(parse_args(&["inspect", "project", "--replay-out"]).is_err());
        assert!(
            parse_args(&[
                "inspect",
                "project",
                "--replay-out",
                "one",
                "--replay-out",
                "two"
            ])
            .is_err()
        );
        assert!(parse_args(&["edit", "project", "--replay-out", "motion.splr"]).is_err());
    }
    #[test]
    fn bed_preparation_preserves_orientation_and_uses_declared_envelope() {
        let bounds = AabbMm::new([-2.0, -4.0, -3.0], [8.0, 6.0, 7.0]).unwrap();
        let envelope = AabbMm::new([10.0, 20.0, 5.0], [110.0, 120.0, 105.0]).unwrap();
        let pose = RigidPoseMm {
            translation_mm: [30.0, 40.0, 50.0],
            ..RigidPoseMm::IDENTITY
        };
        let placed = bed_pose(bounds, pose, envelope, false).unwrap();
        assert_eq!(placed.translation_mm, [30.0, 40.0, 8.0]);
        assert_eq!(placed.rotation_xyzw, pose.rotation_xyzw);
        let centered = bed_pose(bounds, pose, envelope, true).unwrap();
        assert_eq!(centered.translation_mm, [57.0, 69.0, 8.0]);
        let tilted = RigidPoseMm {
            rotation_xyzw: [
                0.0,
                0.0,
                (std::f64::consts::PI / 8.0).sin(),
                (std::f64::consts::PI / 8.0).cos(),
            ],
            ..pose
        };
        assert!(bed_pose(bounds, tilted, envelope, true).is_err());
        let small = AabbMm::new([0.0; 3], [1.0; 3]).unwrap();
        assert!(bed_pose(bounds, pose, small, true).is_err());
        let rotated = RigidPoseMm {
            rotation_xyzw: [0.0, 0.0, 1.0, 0.0],
            ..pose
        };
        let centered = bed_pose(bounds, rotated, envelope, true).unwrap();
        assert_eq!(centered.rotation_xyzw, rotated.rotation_xyzw);
        assert_eq!(centered.translation_mm, [63.0, 71.0, 8.0]);
    }

    #[test]
    fn preflight_is_read_only_but_bed_preparation_is_an_ordered_edit() {
        let options = parse_args(&["inspect", "project", "--preflight"])
            .unwrap()
            .unwrap();
        let Command::Project { workflow, .. } = options.command else {
            panic!("project expected")
        };
        assert!(matches!(
            workflow.operations.as_slice(),
            [Operation::Preflight]
        ));
        for flag in ["--place-on-bed", "--center-on-bed"] {
            assert!(parse_args(&["inspect", "project", flag, "1"]).is_err());
            assert!(parse_args(&["open", "project", flag, "0"]).is_err());
            assert!(parse_args(&["open", "project", flag]).is_err());
        }
        let options = parse_args(&[
            "edit",
            "project",
            "--place-on-bed",
            "1",
            "--center-on-bed",
            "2",
            "--preflight",
            "--compile",
            "--verify",
        ])
        .unwrap()
        .unwrap();
        let Command::Project { workflow, .. } = options.command else {
            panic!("project expected")
        };
        assert!(matches!(
            workflow.operations.as_slice(),
            [
                Operation::PrepareBed(_, false),
                Operation::PrepareBed(_, true),
                Operation::Preflight,
                Operation::Compile,
                Operation::Verify,
                Operation::Save(None)
            ]
        ));
    }

    #[test]
    fn intent_json_is_bounded_before_child_spawn() {
        let root = env::temp_dir().join(format!("spiling-intent-{}", SessionId::new().as_str()));
        std::fs::create_dir(&root).unwrap();
        let file = root.join("intent.json");
        let mut contents = vec![b' '; MAX_PROFILE_JSON_BYTES as usize];
        contents.extend_from_slice(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/manufacturing/solid-fill.intent.json"
        )));
        std::fs::write(&file, contents).unwrap();
        assert!(
            parse(
                [
                    OsString::from("open"),
                    OsString::from("project"),
                    OsString::from("--intent"),
                    file.into_os_string(),
                ]
                .into_iter(),
                None,
                None
            )
            .is_err()
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
