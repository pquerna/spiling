// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::{Command, Error, Options, geometry, output::OutputDirectory, set_once};
use spiling_contracts::{PROTOCOL_VERSION, geometry::*, manufacturing::*, project::*};
use spiling_engine_client::EngineClient;
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
    Remove(OccurrenceId),
    Intent(Arc<ManufacturingIntent>),
    Compile,
    Verify,
    ExportManufacturing,
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
    mut version: Option<u16>,
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
    let mut inspections = Vec::new();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--help" | "-h") => return Ok(None),
            Some("--engine") => set_once(
                &mut engine,
                PathBuf::from(args.next().ok_or("--engine requires a path")?),
                "--engine",
            )?,
            Some("--protocol-version") => set_once(
                &mut version,
                text(&mut args, "--protocol-version")?.parse()?,
                "--protocol-version",
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
            Some("--intent") => operations.push(Operation::Intent(intent(&mut args)?.into())),
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
                    Operation::Verify | Operation::ExportManufacturing
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
        },
        engine: engine.unwrap_or(
            env::current_exe()?
                .with_file_name(format!("spiling-engine{}", env::consts::EXE_SUFFIX)),
        ),
        protocol_version: version.unwrap_or(PROTOCOL_VERSION),
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
        ProjectResponse::JobAccepted { job_id } => {
            let session = client.hello().session_id.clone();
            match geometry::wait_job(client, &session, job_id).await? {
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
async fn verify(
    client: &mut EngineClient,
) -> Result<(ManufacturingArtifactRecord, VerificationReport), Error> {
    let session = client.hello().session_id.clone();
    match client
        .manufacturing(ManufacturingCommand::Inspect {
            session_id: session.clone(),
        })
        .await?
    {
        ManufacturingResponse::Verified { record, report } => Ok((record, report)),
        ManufacturingResponse::JobAccepted { job_id } => {
            match geometry::wait_job(client, &session, job_id).await? {
                JobResult::ManufacturingVerified { record, report } => Ok((record, report)),
                _ => Err("unexpected manufacturing verification job result".into()),
            }
        }
        ManufacturingResponse::Error { error } => {
            Err(spiling_engine_client::ClientError::Manufacturing(error).into())
        }
        _ => Err("unexpected manufacturing verification response".into()),
    }
}
pub async fn run(
    client: &mut EngineClient,
    workflow: Workflow,
    plane: Option<PlaneMm>,
    output: Option<&mut OutputDirectory>,
    manufacturing_output: Option<&mut OutputDirectory>,
) -> Result<serde_json::Value, Error> {
    match execute(client, workflow, plane, output, manufacturing_output).await {
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
    for operation in workflow.operations {
        let base_revision = geometry::scene(client, &session).await?.revision;
        let label;
        match operation {
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
                    ManufacturingResponse::JobAccepted { job_id } => {
                        if !matches!(
                            geometry::wait_job(client, &session, job_id).await?,
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
                let (record, report) = verify(client).await?;
                if !report.verified {
                    return Err("manufacturing replay did not verify the emitted program".into());
                }
                if export {
                    let bundle = client.fetch_manufacturing_bundle(&record).await?;
                    manufacturing_output
                        .as_deref_mut()
                        .ok_or("manufacturing export directory was not admitted")?
                        .write_manufacturing_bundle(&bundle, &report)?;
                    export_evidence = Some(serde_json::json!({
                        "artifact": record, "verification": report,
                        "software_only": true, "not_machine_ready": true
                    }));
                }
                verification = Some((record, report));
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
                let GeometryResponse::JobAccepted { job_id } = response else {
                    return Err("unexpected import response".into());
                };
                if !matches!(
                    geometry::wait_job(client, &session, job_id).await?,
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
        if verification.as_ref().is_some_and(|(record, _)| {
            manufacturing["artifact"]["hash"].as_str() != Some(record.hash.as_str())
        }) {
            verification = None;
        }
        history.push(serde_json::json!({
            "operation": label, "project": info(client).await?, "manufacturing": manufacturing
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
    if let Some((record, verification)) = verification {
        report["manufacturing"]["verified_artifact_hash"] = serde_json::json!(record.hash);
        report["manufacturing"]["verification"] = serde_json::to_value(verification)?;
    }
    if let Some(export_evidence) = export_evidence {
        report["manufacturing_export"] = export_evidence;
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
