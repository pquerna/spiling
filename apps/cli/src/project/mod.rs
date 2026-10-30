// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::{
    Command, Error, Options, geometry,
    output::{OutputDirectory, OutputFile},
    set_once,
};
use spiling_contracts::appearance::{
    AppearanceReport, AppearanceReportRecord, AppearanceTarget, SearchLimits,
};
use spiling_contracts::optical::{
    MAX_OPTICAL_REQUEST_BYTES, OpticalReport, OpticalReportRecord, OpticalRequest, OpticalSelection,
};
use spiling_contracts::process::{
    MAX_PROCESS_DOCUMENT_BYTES, ProcessEdit, ProcessInterfacePermission, ProcessSelector,
    ProcessValue,
};
use spiling_contracts::surface::{
    ImageReliefParameters, MAX_SURFACE_DOCUMENT_BYTES, SurfaceFinish, SurfaceLayerId, SurfaceScope,
    SurfaceScopeKind,
};
use spiling_contracts::{ArtifactView, geometry::*, manufacturing::*, project::*};
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
enum OpticalChoice {
    Requested,
    Best,
    Candidate(u32),
}
enum Operation {
    AppearancePalette(NativePath),
    AppearanceTarget(AppearanceTarget),
    AppearanceLimits(SearchLimits),
    AppearanceCompare,
    AppearanceAccept(Option<String>),
    AppearanceReport,
    OpticalStudy(OpticalRequest),
    OpticalCompare,
    OpticalAccept(Option<OpticalChoice>),
    OpticalReport,
    ApplyOptical(u16, [OpticalMaterialBinding; 2]),
    Import(NativePath),
    Add(DefinitionId, RigidPoseMm),
    Pose(OccurrenceId, RigidPoseMm),
    PrepareBed(OccurrenceId, bool),
    Remove(OccurrenceId),
    SurfaceFinish(OccurrenceId, FaceId, SurfaceScopeKind, SurfaceFinish),
    SurfaceImage(
        OccurrenceId,
        FaceId,
        SurfaceScopeKind,
        NativePath,
        ImageReliefParameters,
    ),
    ExportSurfaceImage(SourceHash),
    RemoveSurfaceFinish(SurfaceLayerId),
    SurfaceFinishEnabled(SurfaceLayerId, bool),
    ProcessSet(OccurrenceId, SurfaceScopeKind, ProcessValue),
    ProcessSetBox {
        occurrence: OccurrenceId,
        scope: SurfaceScopeKind,
        min: [f64; 3],
        max: [f64; 3],
        fraction: f64,
        interface: Option<ProcessInterfacePermission>,
    },
    ProcessReset(OccurrenceId, SurfaceScopeKind),
    ProcessEdit(ProcessEdit),
    ProcessResolve(OccurrenceId),
    Intent(Arc<ManufacturingIntent>),
    MaterialProfile(MaterialProfileSelection),
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
fn appearance_json<T: serde::de::DeserializeOwned>(
    args: &mut impl Iterator<Item = OsString>,
    flag: &str,
) -> Result<T, Error> {
    let source = args
        .next()
        .ok_or_else(|| format!("{flag} requires a native JSON path"))?;
    NativePath::from_os_str(&source)?;
    let mut bytes = Vec::new();
    File::open(PathBuf::from(source))?
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 64 * 1024 {
        return Err("appearance input JSON exceeds byte limit".into());
    }
    Ok(serde_json::from_slice(&bytes)?)
}
fn optical_json(args: &mut impl Iterator<Item = OsString>) -> Result<OpticalRequest, Error> {
    let source = args
        .next()
        .ok_or("--optical-study requires a native JSON path")?;
    NativePath::from_os_str(&source)?;
    let mut bytes = Vec::new();
    File::open(PathBuf::from(source))?
        .take(MAX_OPTICAL_REQUEST_BYTES as u64 + 1)
        .read_to_end(&mut bytes)?;
    Ok(OpticalRequest::decode(&bytes)?)
}
fn process_scope_kind(
    args: &mut impl Iterator<Item = OsString>,
    name: &str,
) -> Result<SurfaceScopeKind, Error> {
    match text(args, name)?.as_str() {
        "definition" => Ok(SurfaceScopeKind::Definition),
        "occurrence" => Ok(SurfaceScopeKind::Occurrence),
        _ => Err(format!("{name} must be definition or occurrence").into()),
    }
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
    let mut surface_image_output = None;
    let mut appearance_report_output = None;
    let mut optical_report_output = None;
    let mut inspections = Vec::new();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--optical-study") => {
                operations.push(Operation::OpticalStudy(optical_json(&mut args)?))
            }
            Some("--apply-optical") => {
                let index =
                    text(&mut args, "--apply-optical construction index")?.parse::<u16>()?;
                let bindings = appearance_json::<[OpticalMaterialBinding; 2]>(
                    &mut args,
                    "--apply-optical bindings",
                )?;
                operations.push(Operation::ApplyOptical(index, bindings));
            }
            Some("--optical-compare") => operations.push(Operation::OpticalCompare),
            Some("--optical-accept") => {
                let choice = match text(&mut args, "--optical-accept")?.as_str() {
                    "requested" => OpticalChoice::Requested,
                    "best" => OpticalChoice::Best,
                    index => OpticalChoice::Candidate(index.parse()?),
                };
                operations.push(Operation::OpticalAccept(Some(choice)));
            }
            Some("--optical-accept-report") => operations.push(Operation::OpticalAccept(None)),
            Some("--optical-report") => {
                let destination = args
                    .next()
                    .ok_or("--optical-report requires a native destination")?;
                NativePath::from_os_str(&destination)?;
                set_once(
                    &mut optical_report_output,
                    PathBuf::from(destination),
                    "--optical-report",
                )?;
                operations.push(Operation::OpticalReport);
            }
            Some("--appearance-palette") => operations.push(Operation::AppearancePalette(path(
                &mut args,
                "--appearance-palette",
            )?)),
            Some("--appearance-target") => {
                let target: AppearanceTarget = appearance_json(&mut args, "--appearance-target")?;
                target.validate()?;
                operations.push(Operation::AppearanceTarget(target));
            }
            Some("--appearance-limits") => {
                let limits: SearchLimits = appearance_json(&mut args, "--appearance-limits")?;
                limits.validate()?;
                operations.push(Operation::AppearanceLimits(limits));
            }
            Some("--appearance-compare") => operations.push(Operation::AppearanceCompare),
            Some("--appearance-accept") => {
                let key = text(&mut args, "--appearance-accept")?;
                operations.push(Operation::AppearanceAccept(Some(key)));
            }
            Some("--appearance-accept-report") => {
                operations.push(Operation::AppearanceAccept(None))
            }
            Some("--appearance-report") => {
                let destination = args
                    .next()
                    .ok_or("--appearance-report requires a native destination")?;
                NativePath::from_os_str(&destination)?;
                set_once(
                    &mut appearance_report_output,
                    PathBuf::from(destination),
                    "--appearance-report",
                )?;
                operations.push(Operation::AppearanceReport);
            }
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
            Some("--surface-image") => {
                let occurrence =
                    OccurrenceId::new(text(&mut args, "--surface-image occurrence")?.parse()?)?;
                let face = FaceId::parse(text(&mut args, "--surface-image face")?)?;
                let scope = match text(&mut args, "--surface-image scope")?.as_str() {
                    "definition" => SurfaceScopeKind::Definition,
                    "occurrence" => SurfaceScopeKind::Occurrence,
                    _ => {
                        return Err("--surface-image scope must be definition or occurrence".into());
                    }
                };
                let source = path(&mut args, "--surface-image source")?;
                let json = text(&mut args, "--surface-image parameters JSON")?;
                if json.len() > MAX_SURFACE_DOCUMENT_BYTES {
                    return Err("surface image parameters exceed the document byte limit".into());
                }
                let parameters: ImageReliefParameters = serde_json::from_str(&json)?;
                parameters.validate()?;
                operations.push(Operation::SurfaceImage(
                    occurrence, face, scope, source, parameters,
                ));
            }
            Some("--surface-image-out") => {
                let hash = SourceHash::parse(text(&mut args, "--surface-image-out hash")?)?;
                let destination = args
                    .next()
                    .ok_or("--surface-image-out requires a native destination")?;
                NativePath::from_os_str(&destination)?;
                set_once(
                    &mut surface_image_output,
                    PathBuf::from(destination),
                    "--surface-image-out",
                )?;
                operations.push(Operation::ExportSurfaceImage(hash));
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
            Some("--process-set") => {
                let occurrence =
                    OccurrenceId::new(text(&mut args, "--process-set occurrence")?.parse()?)?;
                let scope = process_scope_kind(&mut args, "--process-set scope")?;
                let value = ProcessValue::SparseFraction {
                    fraction: text(&mut args, "--process-set fraction")?.parse()?,
                };
                value.validate()?;
                operations.push(Operation::ProcessSet(occurrence, scope, value));
            }
            Some("--process-set-box") => {
                let occurrence =
                    OccurrenceId::new(text(&mut args, "--process-set-box occurrence")?.parse()?)?;
                let scope = process_scope_kind(&mut args, "--process-set-box scope")?;
                let min = [
                    text(&mut args, "--process-set-box min X mm")?.parse()?,
                    text(&mut args, "--process-set-box min Y mm")?.parse()?,
                    text(&mut args, "--process-set-box min Z mm")?.parse()?,
                ];
                let max = [
                    text(&mut args, "--process-set-box max X mm")?.parse()?,
                    text(&mut args, "--process-set-box max Y mm")?.parse()?,
                    text(&mut args, "--process-set-box max Z mm")?.parse()?,
                ];
                ProcessSelector::SourceBoxMm { min, max }.validate()?;
                let fraction = text(&mut args, "--process-set-box fraction")?.parse()?;
                ProcessValue::SparseFraction { fraction }.validate()?;
                let permission = text(&mut args, "--process-set-box separator permission")?;
                let interface = match permission.as_str() {
                    "none" => None,
                    "separator-wall" => Some(ProcessInterfacePermission::SeparatorWall),
                    permission => {
                        let thickness = permission
                            .strip_prefix("horizontal-diaphragm:")
                            .ok_or(
                                "--process-set-box requires explicit none, separator-wall or horizontal-diaphragm:MM permission",
                            )?;
                        let interface = ProcessInterfacePermission::HorizontalDiaphragm {
                            thickness_mm: thickness.parse()?,
                        };
                        interface.validate()?;
                        Some(interface)
                    }
                };
                operations.push(Operation::ProcessSetBox {
                    occurrence,
                    scope,
                    min,
                    max,
                    fraction,
                    interface,
                });
            }
            Some("--process-reset") => {
                let occurrence =
                    OccurrenceId::new(text(&mut args, "--process-reset occurrence")?.parse()?)?;
                let scope = process_scope_kind(&mut args, "--process-reset scope")?;
                operations.push(Operation::ProcessReset(occurrence, scope));
            }
            Some("--process-edit") => {
                let json = text(&mut args, "--process-edit JSON")?;
                if json.len() > MAX_PROCESS_DOCUMENT_BYTES {
                    return Err("process edit JSON exceeds the document byte limit".into());
                }
                operations.push(Operation::ProcessEdit(serde_json::from_str(&json)?));
            }
            Some("--process-resolve") => operations.push(Operation::ProcessResolve(
                OccurrenceId::new(text(&mut args, "--process-resolve occurrence")?.parse()?)?,
            )),
            Some("--intent") => operations.push(Operation::Intent(intent(&mut args)?.into())),
            Some("--material-profile") => {
                let file = args
                    .next()
                    .ok_or("--material-profile requires a native JSON path")?;
                NativePath::from_os_str(&file)?;
                let mut bytes = Vec::new();
                File::open(PathBuf::from(file))?
                    .take(MAX_PROFILE_JSON_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)?;
                if bytes.len() > MAX_PROFILE_JSON_BYTES as usize {
                    return Err("material profile exceeds profile JSON byte limit".into());
                }
                let selection: MaterialProfileSelection = serde_json::from_slice(&bytes)?;
                selection.validate()?;
                operations.push(Operation::MaterialProfile(selection));
            }
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
                        | Operation::ExportSurfaceImage(_)
                        | Operation::ProcessResolve(_)
                        | Operation::AppearanceCompare
                        | Operation::AppearanceReport
                        | Operation::OpticalCompare
                        | Operation::OpticalReport
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
            surface_image_output,
            appearance_report_output,
            optical_report_output,
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
async fn process_scope(
    client: &mut EngineClient,
    occurrence_id: OccurrenceId,
    kind: SurfaceScopeKind,
) -> Result<SurfaceScope, Error> {
    let session = client.hello().session_id.clone();
    let summary = geometry::scene(client, &session).await?;
    let (_, occurrences) = geometry::records(client, &summary).await?;
    let occurrence = occurrences
        .into_iter()
        .find(|row| row.occurrence_id == occurrence_id)
        .ok_or("process target occurrence is not in the current project")?;
    Ok(match kind {
        SurfaceScopeKind::Definition => SurfaceScope::Definition {
            definition_id: occurrence.definition_id,
        },
        SurfaceScopeKind::Occurrence => SurfaceScope::Occurrence {
            occurrence_id,
            expected_definition_id: occurrence.definition_id,
        },
    })
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
            info,
            intent,
            artifact,
            ..
        } => {
            let mut status = serde_json::json!({
                "artifact": artifact, "current_revision": info.revision,
                "software_only": true, "not_machine_ready": true
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
    surface_image_output: Option<&mut OutputFile>,
    appearance_report_output: Option<&mut OutputFile>,
    optical_report_output: Option<&mut OutputFile>,
) -> Result<serde_json::Value, Error> {
    match execute(
        client,
        workflow,
        plane,
        output,
        manufacturing_output,
        replay_output,
        surface_image_output,
        appearance_report_output,
        optical_report_output,
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
    mut surface_image_output: Option<&mut OutputFile>,
    mut appearance_report_output: Option<&mut OutputFile>,
    mut optical_report_output: Option<&mut OutputFile>,
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
    let mut image_evidence = None;
    let mut process_inspections = Vec::new();
    let mut preflight_report = None;
    let mut appearance_comparison: Option<(
        ProjectRevision,
        AppearanceReportRecord,
        AppearanceReport,
    )> = None;
    let mut appearance_evidence = Vec::new();
    let mut optical_comparison: Option<(ProjectRevision, OpticalReportRecord, OpticalReport)> =
        None;
    let mut optical_evidence = Vec::new();
    for operation in workflow.operations {
        let base_revision = geometry::scene(client, &session).await?.revision;
        let label;
        let mut process_resolution = None;
        match operation {
            Operation::ApplyOptical(construction_index, bindings) => {
                label = "apply_optical";
                let captured = info(client).await?;
                let study = captured
                    .appearance_study
                    .optical
                    .as_ref()
                    .ok_or("application requires a retained optical study")?;
                let record = study
                    .report
                    .as_ref()
                    .ok_or("application requires an accepted optical report")?;
                let selection = study
                    .selection
                    .ok_or("application requires an explicitly accepted optical selection")?;
                let response = client
                    .project(ProjectCommand::ApplyOpticalConstruction {
                        session_id: session.clone(),
                        base_revision: captured.revision,
                        application: ApplyOpticalConstruction {
                            construction_index,
                            report_hash: record.hash.clone(),
                            selection,
                            bindings,
                        },
                    })
                    .await?;
                let ProjectResponse::OperationAccepted { operation } = response else {
                    return Err("optical application did not return its native operation".into());
                };
                match geometry::wait_operation(client,&session,operation).await? {
                    JobResult::OpticalConstructionApplied {info,construction_index:index}
                        if index==construction_index && info.revision.0==captured.revision.0+1 => {},
                    _=>return Err("optical application result does not bind the requested construction/revision".into()),
                }
            }
            Operation::OpticalStudy(request) => {
                label = "optical_study";
                let captured = info(client).await?;
                let response = client
                    .project(ProjectCommand::SetOpticalStudy {
                        session_id: session.clone(),
                        base_revision: captured.revision,
                        request,
                    })
                    .await?;
                complete(client, response).await?;
            }
            Operation::OpticalAccept(choice) => {
                label = "optical_accept";
                let captured = info(client).await?;
                let (revision, record, report) = optical_comparison
                    .as_ref()
                    .ok_or("optical accept requires an earlier complete comparison")?;
                let selection = choice
                    .map(|choice| match choice {
                        OpticalChoice::Requested => Ok(OpticalSelection::Requested),
                        OpticalChoice::Best => report
                            .alternatives
                            .first()
                            .copied()
                            .map(|index| OpticalSelection::Candidate { index })
                            .ok_or("optical search retained no assessed alternative"),
                        OpticalChoice::Candidate(index) => {
                            Ok(OpticalSelection::Candidate { index })
                        }
                    })
                    .transpose()?;
                if *revision != captured.revision
                    || !report.is_complete()
                    || selection
                        .as_ref()
                        .is_some_and(|selection| !report.has_selection(selection))
                {
                    return Err("optical acceptance does not match the latest complete current-revision comparison".into());
                }
                let response = client
                    .project(ProjectCommand::AcceptOpticalReport {
                        session_id: session.clone(),
                        base_revision: captured.revision,
                        report_hash: record.hash.clone(),
                        selection,
                    })
                    .await?;
                complete(client, response).await?;
            }
            Operation::OpticalCompare | Operation::OpticalReport => {
                let export = matches!(operation, Operation::OpticalReport);
                label = if export {
                    "optical_report"
                } else {
                    "optical_compare"
                };
                let captured = info(client).await?;
                let study = captured
                    .appearance_study
                    .optical
                    .as_ref()
                    .ok_or("optical comparison requires a study request")?;
                let palette = captured
                    .appearance_study
                    .palette
                    .as_ref()
                    .ok_or("optical comparison requires a palette")?;
                let command = if export {
                    ProjectCommand::GetOpticalReport {
                        session_id: session.clone(),
                        base_revision: captured.revision,
                    }
                } else {
                    ProjectCommand::CompareOptical {
                        session_id: session.clone(),
                        base_revision: captured.revision,
                    }
                };
                let ProjectResponse::OperationAccepted { operation } =
                    client.project(command).await?
                else {
                    return Err("optical comparison did not return an operation".into());
                };
                match geometry::wait_operation(client, &session, operation).await? {
                    JobResult::OpticalCompared {
                        project_id,
                        captured_revision,
                        record,
                        resource,
                    } => {
                        if project_id != captured.project_id
                            || captured_revision != captured.revision
                            || export && study.report.as_ref() != Some(&record)
                        {
                            return Err(
                                "optical report disagrees with captured project/study".into()
                            );
                        }
                        let report = client.fetch_optical_report(&record, &resource).await?;
                        report.matches_inputs(palette, study.request.as_ref())?;
                        if export {
                            optical_report_output
                                .as_deref_mut()
                                .ok_or("optical report output was not admitted")?
                                .write(&report.encode()?)?;
                        }
                        optical_evidence.push(serde_json::json!({
                            "operation": label, "project_id": project_id, "captured_revision": captured_revision,
                            "record": record, "resource": resource, "report": report,
                        }));
                        if !export {
                            optical_comparison = Some((captured_revision, record, report));
                        }
                    }
                    _ => return Err("unexpected optical comparison result".into()),
                }
            }
            Operation::AppearancePalette(source) => {
                label = "appearance_palette";
                let project_revision = info(client).await?.revision;
                let response = client
                    .project(ProjectCommand::ImportAppearancePalette {
                        session_id: session.clone(),
                        base_revision: project_revision,
                        source,
                    })
                    .await?;
                let ProjectResponse::OperationAccepted { operation } = response else {
                    return Err("palette import did not return an operation".into());
                };
                match geometry::wait_operation(client, &session, operation).await? {
                    JobResult::AppearancePaletteImported {
                        info: receipt,
                        palette,
                    } => {
                        palette.validate()?;
                        if info(client).await? != receipt {
                            return Err(
                                "palette receipt disagrees with authoritative project".into()
                            );
                        }
                    }
                    _ => return Err("unexpected palette import result".into()),
                }
            }
            Operation::AppearanceTarget(_) | Operation::AppearanceLimits(_) => {
                label = "appearance_target";
                let captured = info(client).await?;
                let (target, limits) = match operation {
                    Operation::AppearanceTarget(target) => {
                        (target, captured.appearance_study.limits.clone())
                    }
                    Operation::AppearanceLimits(limits) => (
                        captured
                            .appearance_study
                            .target
                            .clone()
                            .ok_or("appearance limits require a target first")?,
                        limits,
                    ),
                    _ => unreachable!(),
                };
                let response = client
                    .project(ProjectCommand::SetAppearanceTarget {
                        session_id: session.clone(),
                        base_revision: captured.revision,
                        target,
                        limits,
                    })
                    .await?;
                complete(client, response).await?;
            }
            Operation::AppearanceAccept(selection) => {
                label = "appearance_accept";
                let captured = info(client).await?;
                let (revision, record, report) = appearance_comparison
                    .as_ref()
                    .ok_or("appearance accept requires an earlier complete comparison")?;
                if *revision != captured.revision
                    || !report.is_complete()
                    || selection
                        .as_ref()
                        .is_some_and(|key| !report.has_selection(key))
                {
                    return Err("appearance acceptance does not match the latest complete current-revision comparison".into());
                }
                let response = client
                    .project(ProjectCommand::AcceptAppearanceReport {
                        session_id: session.clone(),
                        base_revision: captured.revision,
                        report_hash: record.hash.clone(),
                        selection,
                    })
                    .await?;
                complete(client, response).await?;
            }
            Operation::AppearanceCompare | Operation::AppearanceReport => {
                let export = matches!(operation, Operation::AppearanceReport);
                label = if export {
                    "appearance_report"
                } else {
                    "appearance_compare"
                };
                let captured = info(client).await?;
                let command = if export {
                    ProjectCommand::GetAppearanceReport {
                        session_id: session.clone(),
                        base_revision: captured.revision,
                    }
                } else {
                    ProjectCommand::CompareAppearance {
                        session_id: session.clone(),
                        base_revision: captured.revision,
                    }
                };
                let ProjectResponse::OperationAccepted { operation } =
                    client.project(command).await?
                else {
                    return Err("appearance comparison did not return an operation".into());
                };
                match geometry::wait_operation(client, &session, operation).await? {
                    JobResult::AppearanceCompared {
                        project_id,
                        captured_revision,
                        record,
                        resource,
                    } => {
                        if project_id != captured.project_id
                            || captured_revision != captured.revision
                        {
                            return Err("appearance report disagrees with captured project".into());
                        }
                        let report = client.fetch_appearance_report(&record, &resource).await?;
                        if export {
                            appearance_report_output
                                .as_deref_mut()
                                .ok_or("appearance report output was not admitted")?
                                .write(&report.encode()?)?;
                        }
                        appearance_evidence.push(serde_json::json!({
                            "operation": label, "project_id": project_id, "captured_revision": captured_revision,
                            "record": record, "resource": resource, "report": report,
                        }));
                        if !export {
                            appearance_comparison = Some((captured_revision, record, report));
                        }
                    }
                    _ => return Err("unexpected appearance comparison result".into()),
                }
            }
            Operation::SurfaceImage(occurrence_id, face_id, scope, source, parameters) => {
                label = "surface_image";
                let summary = geometry::scene(client, &session).await?;
                let (_, occurrences) = geometry::records(client, &summary).await?;
                let occurrence = occurrences
                    .iter()
                    .find(|row| row.occurrence_id == occurrence_id)
                    .ok_or("surface target occurrence is not in the current project")?;
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
                    GeometryResponse::FaceInspection { inspection }
                        if inspection.reference == reference => {}
                    _ => return Err("surface target native face inspection mismatch".into()),
                }
                let base_revision = info(client).await?.revision;
                let response = client
                    .project(ProjectCommand::ImportSurfaceImageFinish {
                        session_id: session.clone(),
                        base_revision,
                        reference,
                        scope,
                        source,
                        parameters,
                    })
                    .await?;
                match response {
                    ProjectResponse::OperationAccepted { operation } => {
                        match geometry::wait_operation(client, &session, operation).await? {
                            JobResult::SurfaceImageImported {
                                info: receipt,
                                image,
                                layer_id,
                            } => {
                                image.validate()?;
                                if info(client).await? != receipt {
                                    return Err("surface image receipt disagrees with authoritative project".into());
                                }
                                image_evidence = Some(serde_json::json!({
                                    "project": receipt, "image": image, "layer_id": layer_id,
                                }));
                            }
                            _ => return Err("unexpected surface image import result".into()),
                        }
                    }
                    _ => return Err("surface image import did not return an operation".into()),
                }
            }
            Operation::ExportSurfaceImage(image_hash) => {
                label = "surface_image_export";
                let captured = info(client).await?;
                let response = client
                    .project(ProjectCommand::GetSurfaceImage {
                        session_id: session.clone(),
                        base_revision: captured.revision,
                        image_hash: image_hash.clone(),
                    })
                    .await?;
                let ProjectResponse::OperationAccepted { operation } = response else {
                    return Err("surface image retrieval did not return an operation".into());
                };
                match geometry::wait_operation(client, &session, operation).await? {
                    JobResult::SurfaceImagePrepared {
                        project_id,
                        captured_revision,
                        record,
                        resource,
                    } => {
                        spiling_contracts::native::validate_surface_image_prepared_binding(
                            &project_id,
                            captured_revision,
                            &record,
                            &resource,
                        )
                        .map_err(spiling_engine_client::ClientError::Protocol)?;
                        if project_id != captured.project_id
                            || captured_revision != captured.revision
                            || record.image_hash != image_hash
                        {
                            return Err(
                                "surface image result disagrees with captured project".into()
                            );
                        }
                        let bytes = client.fetch_surface_image(&record, &resource).await?;
                        surface_image_output
                            .as_deref_mut()
                            .ok_or("surface image output was not admitted")?
                            .write(&bytes)?;
                        image_evidence = Some(serde_json::json!({
                            "project_id": project_id, "captured_revision": captured_revision,
                            "record": record, "resource": resource,
                        }));
                    }
                    _ => return Err("unexpected surface image retrieval result".into()),
                }
            }
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
            Operation::ProcessSet(..)
            | Operation::ProcessSetBox { .. }
            | Operation::ProcessReset(..)
            | Operation::ProcessEdit(_) => {
                let edit = match operation {
                    Operation::ProcessSet(occurrence_id, kind, value) => {
                        label = "process_set";
                        let ProcessValue::SparseFraction { fraction } = value;
                        ProcessEdit::SetSparseFraction {
                            scope: process_scope(client, occurrence_id, kind).await?,
                            fraction,
                        }
                    }
                    Operation::ProcessSetBox {
                        occurrence,
                        scope,
                        min,
                        max,
                        fraction,
                        interface,
                    } => {
                        label = "process_set_box";
                        ProcessEdit::SetBoxSparseFraction {
                            scope: process_scope(client, occurrence, scope).await?,
                            min,
                            max,
                            fraction,
                            interface,
                        }
                    }
                    Operation::ProcessReset(occurrence_id, kind) => {
                        label = "process_reset";
                        ProcessEdit::Reset {
                            scope: process_scope(client, occurrence_id, kind).await?,
                        }
                    }
                    Operation::ProcessEdit(edit) => {
                        label = "process_edit";
                        edit
                    }
                    _ => unreachable!(),
                };
                let base_revision = info(client).await?.revision;
                let response = client
                    .project(ProjectCommand::EditProcess {
                        session_id: session.clone(),
                        base_revision,
                        edit,
                    })
                    .await?;
                complete(client, response).await?;
            }
            Operation::ProcessResolve(occurrence_id) => {
                label = "process_resolve";
                let project = info(client).await?;
                let resolved = project
                    .resolved_process
                    .iter()
                    .find(|resolved| resolved.occurrence_id == occurrence_id)
                    .ok_or("process resolution requires an explicit manufacturing intent and a current occurrence")?;
                let evidence = serde_json::json!({
                    "project_revision": project.revision,
                    "base_whole_owned_resolution": resolved,
                    "finite_box_intent": project.process_document.layers.iter().filter(|layer| match &layer.scope {
                        SurfaceScope::Definition{definition_id}=>definition_id==&resolved.definition_id,
                        SurfaceScope::Occurrence{occurrence_id,expected_definition_id}=>*occurrence_id==resolved.occurrence_id && expected_definition_id==&resolved.definition_id,
                    }).filter(|layer|layer.operations.iter().any(|operation|matches!(operation.selector,ProcessSelector::SourceBoxMm{..}))).collect::<Vec<_>>(),
                    "spatial_output_authority": "Fresh compiled process_resolution/region/interface records and actual emitted replay; base resolution excludes finite boxes",
                });
                process_inspections.push(evidence.clone());
                process_resolution = Some(evidence);
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
            Operation::MaterialProfile(selection) => {
                label = "material_profile";
                let base_revision = info(client).await?.revision;
                match client
                    .manufacturing(ManufacturingCommand::ApplyMaterialProfile {
                        session_id: session.clone(),
                        base_revision,
                        selection,
                    })
                    .await?
                {
                    ManufacturingResponse::Status { .. } => {}
                    ManufacturingResponse::Error { error } => {
                        return Err(spiling_engine_client::ClientError::Manufacturing(error).into());
                    }
                    _ => return Err("unexpected material profile response".into()),
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
                        .write_manufacturing_bundle(&bundle, &report, &provenance)?;
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
                let project_revision = info(client).await?.revision;
                let response = geometry::control(
                    client,
                    GeometryCommand::SetInstancePose {
                        session_id: session.clone(),
                        base_revision,
                        project_revision,
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
                let (_, occurrences) = geometry::records(client, &summary).await?;
                let occurrence = occurrences
                    .iter()
                    .find(|row| row.occurrence_id == occurrence_id)
                    .ok_or("bed preparation occurrence not in project")?;
                let (intent, project_revision) = match client.manufacturing(ManufacturingCommand::Get { session_id: session.clone() }).await? {
                    ManufacturingResponse::Status { intent: Some(intent), info, .. } => (intent, info.revision),
                    ManufacturingResponse::Status { intent: None, .. } => return Err("bed preparation requires --intent with a complete explicit printer/recipe profile first".into()),
                    ManufacturingResponse::Error { error } => return Err(spiling_engine_client::ClientError::Manufacturing(error).into()),
                    _ => return Err("unexpected manufacturing status response".into()),
                };
                let preparation = client
                    .prepare_bed_pose(
                        &summary,
                        project_revision,
                        occurrence,
                        intent.printer.build_envelope,
                        center,
                    )
                    .await?;
                let response = geometry::control(
                    client,
                    GeometryCommand::SetInstancePose {
                        session_id: session.clone(),
                        base_revision: preparation.scene_revision,
                        project_revision: preparation.project_revision,
                        occurrence_id,
                        pose: preparation.pose,
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
        let project = info(client).await?;
        history.push(serde_json::json!({
            "operation": label, "project": project, "manufacturing": manufacturing,
            "preflight": if label == "preflight" { preflight_report.as_ref() } else { None },
            "process_resolution": process_resolution,
            "preflight_freshness": preflight_report.as_ref().map(|report|
                if report.input_revision == project.revision { "current" } else { "stale" })
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
    let mut report = if summary.occurrence_count == 0 {
        serde_json::json!({
            "hello": client.hello(), "scene": summary, "occurrences": [], "definitions": [],
            "unique_mesh_bytes": 0, "unique_mesh_chunks": 0, "unique_mesh_vertices": 0,
            "unique_mesh_triangles": 0, "section": null,
        })
    } else {
        geometry::inspect(client, plane, output).await?
    };
    report["command"] = serde_json::json!("project");
    let project = info(client).await?;
    report["project"] = serde_json::to_value(&project)?;
    report["face_inspections"] = serde_json::to_value(inspections)?;
    report["operations"] = serde_json::json!(history);
    report["process_inspections"] = serde_json::json!(process_inspections);
    report["manufacturing"] = manufacturing_status(client, true).await?;
    if let Some(preflight) = preflight_report {
        report["manufacturing"]["preflight_freshness"] =
            serde_json::json!(if preflight.input_revision == project.revision {
                "current"
            } else {
                "stale"
            });
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
    if let Some(evidence) = image_evidence {
        report["surface_image"] = evidence;
    }
    report["appearance"] = serde_json::json!(appearance_evidence);
    report["optical"] = serde_json::json!(optical_evidence);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse_args(args: &[&str]) -> Result<Option<Options>, Error> {
        parse(args.iter().map(OsString::from), None, None)
    }
    #[test]
    fn appearance_readonly_flags_and_literal_selection_keys_are_ordered() {
        let options = parse_args(&[
            "inspect",
            "project",
            "--appearance-compare",
            "--appearance-report",
            "new.json",
        ])
        .unwrap()
        .unwrap();
        let Command::Project {
            workflow,
            appearance_report_output,
            ..
        } = options.command
        else {
            panic!("project workflow");
        };
        assert!(matches!(
            workflow.operations.as_slice(),
            [Operation::AppearanceCompare, Operation::AppearanceReport,]
        ));
        assert_eq!(appearance_report_output, Some(PathBuf::from("new.json")));
        for flags in [
            vec!["--appearance-palette", "palette.json"],
            vec!["--appearance-accept", "none"],
            vec!["--appearance-accept-report"],
        ] {
            let mut args = vec!["inspect", "project"];
            args.extend(flags);
            assert!(
                parse_args(&args)
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("read-only")
            );
        }
        let options = parse_args(&[
            "open",
            "project",
            "--appearance-compare",
            "--appearance-accept",
            "none",
            "--appearance-accept-report",
            "--undo",
            "--redo",
            "--save",
        ])
        .unwrap()
        .unwrap();
        let Command::Project { workflow, .. } = options.command else {
            panic!("project workflow");
        };
        let [
            Operation::AppearanceCompare,
            Operation::AppearanceAccept(Some(key)),
            Operation::AppearanceAccept(None),
            Operation::Undo,
            Operation::Redo,
            Operation::Save(None),
        ] = workflow.operations.as_slice()
        else {
            panic!("ordered appearance operations");
        };
        assert_eq!(key, "none");
    }
    #[test]
    fn box_process_arguments_are_explicit_typed_ordered_and_readonly_safe() {
        let flags = [
            "--process-set-box",
            "1",
            "occurrence",
            "-1",
            "-2",
            "-3",
            "4",
            "5",
            "6",
            "0.6",
            "none",
        ];
        for (permission, expected) in [
            ("none", None),
            (
                "separator-wall",
                Some(ProcessInterfacePermission::SeparatorWall),
            ),
            (
                "horizontal-diaphragm:0.23",
                Some(ProcessInterfacePermission::HorizontalDiaphragm { thickness_mm: 0.23 }),
            ),
        ] {
            let mut args = vec!["open", "project"];
            args.extend(flags);
            *args.last_mut().unwrap() = permission;
            args.extend(["--undo", "--redo"]);
            let options = parse_args(&args).unwrap().unwrap();
            let Command::Project { workflow, .. } = options.command else {
                panic!("expected project command");
            };
            let [
                Operation::ProcessSetBox {
                    occurrence,
                    scope,
                    min,
                    max,
                    fraction,
                    interface,
                },
                Operation::Undo,
                Operation::Redo,
            ] = workflow.operations.as_slice()
            else {
                panic!("expected ordered typed box edit and history");
            };
            assert_eq!(occurrence.get(), 1);
            assert_eq!(*scope, SurfaceScopeKind::Occurrence);
            assert_eq!(*min, [-1.0, -2.0, -3.0]);
            assert_eq!(*max, [4.0, 5.0, 6.0]);
            assert_eq!(*fraction, 0.6);
            assert_eq!(*interface, expected);
        }
        for (index, invalid) in [
            (1, "0"),
            (2, "face"),
            (3, "NaN"),
            (4, "inf"),
            (5, "-inf"),
            (6, "-1"),
            (7, "-3"),
            (8, "-3"),
            (9, "0"),
            (9, "1.01"),
            (9, "NaN"),
            (10, "true"),
            (10, "--approve-separator"),
            (10, "horizontal-diaphragm"),
            (10, "horizontal-diaphragm:"),
            (10, "horizontal-diaphragm:0"),
            (10, "horizontal-diaphragm:-0.1"),
            (10, "horizontal-diaphragm:NaN"),
            (10, "horizontal-diaphragm:inf"),
            (10, "horizontal-diaphragm:-inf"),
            (10, "horizontal-diaphragm:abc"),
        ] {
            let mut args = vec!["open", "project"];
            let mut invalid_flags = flags;
            invalid_flags[index] = invalid;
            args.extend(invalid_flags);
            assert!(parse_args(&args).is_err(), "{args:?}");
        }
        let mut missing_permission = vec!["open", "project"];
        missing_permission.extend(&flags[..flags.len() - 1]);
        assert!(parse_args(&missing_permission).is_err());
        for mode in ["inspect", "open"] {
            let mut args = vec![mode, "project"];
            args.extend(flags);
            if mode == "open" {
                args.push("--read-only");
            }
            assert!(
                parse_args(&args)
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("read-only")
            );
        }
    }
    #[test]
    fn process_arguments_reject_invalid_authoring_and_allow_readonly_resolution() {
        for flags in [
            vec!["--process-set", "1", "definition", "0"],
            vec!["--process-set", "1", "occurrence", "1.01"],
            vec!["--process-set", "1", "occurrence", "NaN"],
            vec!["--process-set", "1", "occurrence", "inf"],
            vec!["--process-set", "0", "occurrence", "0.2"],
            vec!["--process-set", "1", "face", "0.2"],
            vec!["--process-reset", "1", "face"],
            vec![
                "--process-edit",
                r#"{"op":"add_layer","scope":{"kind":"definition","definition_id":"invalid"},"selector":{"kind":"face"},"action":{"kind":"erase"}}"#,
            ],
            vec!["--process-edit", r#"{"op":"remove_layer","layer_id":0}"#],
            vec!["--process-resolve", "0"],
        ] {
            let mut args = vec!["open", "project"];
            args.extend(flags);
            assert!(parse_args(&args).is_err(), "{args:?}");
        }
        for flags in [
            vec!["--process-set", "1", "definition", "0.2"],
            vec!["--process-reset", "1", "occurrence"],
            vec!["--process-edit", r#"{"op":"remove_layer","layer_id":1}"#],
        ] {
            for mode in ["inspect", "open"] {
                let mut args = vec![mode, "project"];
                args.extend(flags.iter().copied());
                if mode == "open" {
                    args.push("--read-only");
                }
                assert!(
                    parse_args(&args)
                        .err()
                        .unwrap()
                        .to_string()
                        .contains("read-only")
                );
            }
        }
        assert!(
            parse_args(&["inspect", "project", "--process-resolve", "1"])
                .unwrap()
                .is_some()
        );
        assert!(
            parse_args(&["open", "project", "--process-set", "1", "definition", "1"])
                .unwrap()
                .is_some()
        );
    }
    #[test]
    fn image_flags_validate_before_spawn_and_export_is_readonly_safe() {
        let parameters = r#"{"depth_mm":0.08,"u_repeats":2,"u_phase_fraction":0.137,"z_period_mm":3.7,"z_phase_mm":0.213,"edge_margin_mm":1,"vertical_error_mm":0.01}"#;
        assert!(
            parse_args(&[
                "inspect",
                "project",
                "--surface-image",
                "1",
                "step:1",
                "definition",
                "image.png",
                parameters
            ])
            .is_err()
        );
        assert!(
            parse_args(&[
                "open",
                "project",
                "--surface-image",
                "1",
                "step:1",
                "definition",
                "image.png",
                "{}"
            ])
            .is_err()
        );
        let hash = "a".repeat(64);
        assert!(
            parse_args(&[
                "inspect",
                "project",
                "--surface-image-out",
                &hash,
                "fresh.pgm"
            ])
            .unwrap()
            .is_some()
        );
        assert!(
            parse_args(&[
                "inspect",
                "project",
                "--surface-image-out",
                "invalid",
                "fresh.pgm"
            ])
            .is_err()
        );
        let options = parse_args(&[
            "open",
            "project",
            "--surface-image",
            "1",
            "step:1",
            "definition",
            "image.png",
            parameters,
            "--undo",
            "--redo",
        ])
        .unwrap()
        .unwrap();
        let Command::Project { workflow, .. } = options.command else {
            panic!("project workflow");
        };
        assert!(matches!(
            workflow.operations.as_slice(),
            [
                Operation::SurfaceImage(..),
                Operation::Undo,
                Operation::Redo
            ]
        ));
    }
    #[cfg(unix)]
    #[test]
    fn image_source_keeps_native_non_utf8_bytes() {
        use std::os::unix::ffi::{OsStrExt, OsStringExt};
        let source = OsString::from_vec(b"image-\xff.png".to_vec());
        let parameters = r#"{"depth_mm":0.08,"u_repeats":2,"u_phase_fraction":0.137,"z_period_mm":3.7,"z_phase_mm":0.213,"edge_margin_mm":1,"vertical_error_mm":0.01}"#;
        let mut args: Vec<OsString> = [
            "open",
            "project",
            "--surface-image",
            "1",
            "step:1",
            "definition",
        ]
        .into_iter()
        .map(OsString::from)
        .collect();
        args.push(source.clone());
        args.push(parameters.into());
        let options = parse(args.into_iter(), None, None).unwrap().unwrap();
        let Command::Project { workflow, .. } = options.command else {
            panic!("project workflow");
        };
        let Operation::SurfaceImage(_, _, _, native, _) = &workflow.operations[0] else {
            panic!("image operation");
        };
        assert_eq!(
            native,
            &NativePath::from_os_str(std::ffi::OsStr::from_bytes(source.as_bytes())).unwrap()
        );
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
    fn intent_json_preserves_consumed_policies_and_rejects_missing_or_invalid_choices() {
        let root = env::temp_dir().join(format!("spiling-policies-{}", SessionId::new().as_str()));
        std::fs::create_dir(&root).unwrap();
        let file = root.join("intent.json");
        let mut intent = decode_intent(include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/manufacturing/solid-fill.intent.json"
        )))
        .unwrap();
        intent.recipe.seam_policy =
            spiling_contracts::manufacturing::SeamPolicy::PreferredBuildDirection {
                xy: [0.6, -0.8],
            };
        intent.recipe.rectilinear_axis = spiling_contracts::manufacturing::RectilinearAxisPolicy::Y;
        let load = || {
            parse(
                [
                    OsString::from("open"),
                    OsString::from("project"),
                    OsString::from("--intent"),
                    file.as_os_str().to_os_string(),
                ]
                .into_iter(),
                None,
                None,
            )
        };
        let original = serde_json::to_value(&intent).unwrap();
        std::fs::write(&file, serde_json::to_vec(&intent).unwrap()).unwrap();
        let Command::Project { workflow, .. } = load().unwrap().unwrap().command else {
            panic!("project expected");
        };
        let [Operation::Intent(received)] = workflow.operations.as_slice() else {
            panic!("existing typed intent operation expected");
        };
        assert_eq!(received.as_ref(), &intent);
        for field in ["seam_policy", "rectilinear_axis"] {
            let mut missing = original.clone();
            missing["recipe"].as_object_mut().unwrap().remove(field);
            std::fs::write(&file, serde_json::to_vec(&missing).unwrap()).unwrap();
            assert!(load().is_err(), "{field}");
        }
        for (field, policy) in [
            ("seam_policy", serde_json::json!({"kind":"unknown"})),
            ("rectilinear_axis", serde_json::json!("unknown")),
        ] {
            let mut invalid = original.clone();
            invalid["recipe"][field] = policy;
            std::fs::write(&file, serde_json::to_vec(&invalid).unwrap()).unwrap();
            assert!(load().is_err(), "{field}");
        }
        std::fs::remove_dir_all(root).unwrap();
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
