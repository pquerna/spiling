// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use std::{
    io::Read,
    path::{Path, PathBuf},
};

use serde::Serialize;
use spiling_contracts::{
    ArtifactView, NativeOperationView,
    geometry::{
        GeometryCommand, GeometryResponse, JobResult, JobStatus, MAX_DISPLAY_LABEL_BYTES,
        OccurrenceId, SECTION_PLANE_TOLERANCE_MM, ScenePageKind, SceneRevision, SessionId,
        bounded_text,
    },
    manufacturing::{
        DepositionOwner, ManufacturingArtifactRecord, ManufacturingCommand, ManufacturingIntent,
        ManufacturingResponse, ReplayDisplayDescriptor, SparseBandDisposition, SparseBandVeto,
        VerificationReport,
    },
};
use spiling_engine_client::{ClientError, export::OutputDirectory};
use tauri::{AppHandle, Manager, State};

use super::{EngineState, Supervisor, resolve_fixture_project, rpc, selection_session};

#[derive(Default)]
pub(super) struct ManufacturingSession {
    pending: Option<String>,
    verified: Option<CapturedVerification>,
    selection: Option<ExportSelection>,
    intent_selection: Option<IntentSelection>,
}
impl ManufacturingSession {
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }
    fn load_intent(
        &mut self,
        token: &str,
        session: &SessionId,
    ) -> Result<ManufacturingIntent, String> {
        let selection = self
            .intent_selection
            .as_ref()
            .filter(|selection| {
                selection.public.token == token && &selection.public.session_id == session
            })
            .ok_or("Unknown or consumed manufacturing intent token")?;
        selection
            .intent
            .validate()
            .map_err(|error| format!("{:?}: {}", error.code, error.message))?;
        Ok(self.intent_selection.take().unwrap().intent)
    }
}
#[derive(Clone)]
struct CapturedVerification {
    operation: String,
    session: SessionId,
    record: ManufacturingArtifactRecord,
    report: VerificationReport,
    resource: ArtifactView,
    replay: Option<ReplayDisplayDescriptor>,
}
struct ExportSelection {
    public: SelectedManufacturingExport,
    path: PathBuf,
}
#[derive(Clone, Serialize)]
pub struct SelectedManufacturingExport {
    token: String,
    session_id: SessionId,
    label: String,
}
#[derive(Serialize)]
pub struct ManufacturingExportReceipt {
    label: String,
    files: [&'static str; 4],
}

const MAX_INTENT_BYTES: usize = 32 * 1024;
struct IntentSelection {
    public: SelectedManufacturingIntent,
    intent: ManufacturingIntent,
}
#[derive(Clone, Serialize)]
pub struct SelectedManufacturingIntent {
    token: String,
    session_id: SessionId,
    label: String,
}
#[derive(Serialize)]
pub struct ManufacturingLayer {
    index: u32,
    z_mm: f64,
    lower_z_mm: f64,
    height_mm: f64,
    section_z_mm: f64,
    deposition_segments: usize,
    sparse_band: Option<ManufacturingSparseBand>,
    diaphragms: Vec<ManufacturingDiaphragm>,
}
#[derive(Serialize)]
pub struct ManufacturingDiaphragm {
    occurrence_id: OccurrenceId,
    provenance: spiling_contracts::process::ProcessProvenance,
    source_plane_mm: f64,
    requested_thickness_mm: f64,
    physical_z_mm: [f64; 2],
    region_index: u16,
    area_mm2: f64,
    nominal_path_volume_mm3: f64,
}

#[derive(Serialize)]
pub struct ManufacturingSparseBand {
    first_layer: u32,
    combined_regions: Vec<u16>,
    combined_global: bool,
    retained_fine: Vec<(SparseBandVeto, u32)>,
}

fn read_intent(reader: impl Read) -> Result<ManufacturingIntent, String> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_INTENT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Cannot read manufacturing intent: {error}"))?;
    if bytes.len() > MAX_INTENT_BYTES {
        return Err("Manufacturing intent exceeds the 32 KiB limit.".into());
    }
    let intent: ManufacturingIntent = serde_json::from_slice(&bytes)
        .map_err(|error| format!("Invalid inert ManufacturingIntent JSON: {error}"))?;
    intent
        .validate()
        .map_err(|error| format!("{:?}: {}", error.code, error.message))?;
    Ok(intent)
}

async fn install_intent(
    state: &EngineState,
    session: SessionId,
    path: &Path,
) -> Result<SelectedManufacturingIntent, String> {
    let file = std::fs::File::open(path)
        .map_err(|error| format!("Cannot open selected intent: {error}"))?;
    if !file
        .metadata()
        .map_err(|error| error.to_string())?
        .is_file()
    {
        return Err("Selected manufacturing intent must be a regular JSON file.".into());
    }
    let intent = read_intent(file)?;
    let public = SelectedManufacturingIntent {
        token: uuid::Uuid::new_v4().to_string(),
        session_id: session.clone(),
        label: bounded_text(
            &path.file_name().unwrap_or_default().to_string_lossy(),
            MAX_DISPLAY_LABEL_BYTES as usize,
        ),
    };
    let mut supervisor = state.active().await?;
    supervisor.refresh().await;
    if supervisor
        .client
        .as_ref()
        .map(|client| &client.hello().session_id)
        != Some(&session)
    {
        return Err("stale_revision: session changed during intent selection".into());
    }
    supervisor.manufacturing.intent_selection = Some(IntentSelection {
        public: public.clone(),
        intent,
    });
    Ok(public)
}

#[tauri::command]
pub async fn manufacturing_select_intent(
    app: AppHandle,
    state: State<'_, EngineState>,
) -> Result<Option<SelectedManufacturingIntent>, String> {
    let session = selection_session(&state).await?;
    let (send, receive) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let path = rfd::FileDialog::new()
            .set_title("Load inert manufacturing intent (32 KiB)")
            .add_filter("ManufacturingIntent JSON", &["json"])
            .pick_file();
        let _ = send.send(path);
    })
    .map_err(|error| error.to_string())?;
    match receive
        .await
        .map_err(|_| "Intent picker closed without a result")?
    {
        None => Ok(None),
        Some(path) => install_intent(&state, session, &path).await.map(Some),
    }
}

#[tauri::command]
pub async fn manufacturing_debug_select_intent(
    state: State<'_, EngineState>,
    relative_path: String,
) -> Result<SelectedManufacturingIntent, String> {
    let session = selection_session(&state).await?;
    let root = state
        .fixture_root
        .as_ref()
        .ok_or("Native fixture admission is disabled")?;
    let path = super::resolve_fixture_sources(root, vec![relative_path])?.remove(0);
    install_intent(&state, session, &path).await
}

#[tauri::command]
pub async fn manufacturing_load_intent(
    state: State<'_, EngineState>,
    token: String,
    session_id: SessionId,
) -> Result<ManufacturingIntent, String> {
    let mut supervisor = state.active().await?;
    supervisor.refresh().await;
    if supervisor
        .client
        .as_ref()
        .map(|client| &client.hello().session_id)
        != Some(&session_id)
    {
        return Err("stale_revision: old intent selection session".into());
    }
    supervisor.manufacturing.load_intent(&token, &session_id)
}

#[tauri::command]
pub async fn manufacturing_prepare_bed(
    state: State<'_, EngineState>,
    session_id: SessionId,
    base_revision: SceneRevision,
    occurrence_id: OccurrenceId,
    center: bool,
) -> Result<GeometryResponse, String> {
    let mut supervisor = state.active().await?;
    supervisor.refresh().await;
    if supervisor
        .client
        .as_ref()
        .map(|client| &client.hello().session_id)
        != Some(&session_id)
    {
        return Err("stale_revision: old bed preparation session".into());
    }
    let summary = match supervisor
        .geometry(GeometryCommand::GetScene {
            session_id: session_id.clone(),
        })
        .await?
    {
        GeometryResponse::Scene { summary } => summary,
        GeometryResponse::Error { error } => return Err(error.to_string()),
        GeometryResponse::ProjectError { error } => return Err(error.to_string()),
        _ => return Err("Unexpected native scene response for bed preparation".into()),
    };
    if summary.revision != base_revision {
        return Err("stale_revision: scene changed before bed preparation".into());
    }
    let response = supervisor
        .client
        .as_ref()
        .ok_or("engine is not running")?
        .manufacturing(ManufacturingCommand::Get {
            session_id: session_id.clone(),
        })
        .await
        .map_err(|error| error.to_string())?;
    let (intent, info) = match response {
        ManufacturingResponse::Status {
            intent: Some(intent),
            info,
            ..
        } => (intent, info),
        ManufacturingResponse::Error { error } => return Err(error.to_string()),
        _ => {
            return Err(
                "Select and Apply a full manufacturing intent before preparing the bed".into(),
            );
        }
    };
    if info.read_only {
        return Err("read_only: bed preparation cannot mutate this snapshot".into());
    }
    let mut occurrence = None;
    let mut count = 0_usize;
    let mut offset = 0;
    loop {
        let (rows, next_offset) = match supervisor
            .geometry(GeometryCommand::GetScenePage {
                session_id: session_id.clone(),
                revision: summary.revision,
                kind: ScenePageKind::Occurrences,
                offset,
            })
            .await?
        {
            GeometryResponse::ScenePage {
                definitions,
                occurrences,
                next_offset,
            } if definitions.is_empty() => (occurrences, next_offset),
            GeometryResponse::Error { error } => return Err(error.to_string()),
            GeometryResponse::ProjectError { error } => return Err(error.to_string()),
            _ => return Err("Unexpected native placement occurrence page".into()),
        };
        count = count
            .checked_add(rows.len())
            .ok_or("Native occurrence count overflow")?;
        if count > summary.occurrence_count as usize {
            return Err("Native placement page exceeds the captured scene".into());
        }
        for row in rows {
            if row.occurrence_id == occurrence_id {
                if occurrence.replace(row).is_some() {
                    return Err("Duplicate native placement occurrence".into());
                }
            }
        }
        match next_offset {
            None => break,
            Some(next) if next > offset => offset = next,
            _ => return Err("Native placement occurrence page did not advance".into()),
        }
    }
    if count != summary.occurrence_count as usize {
        return Err("Incomplete native scene for bed preparation".into());
    }
    let occurrence =
        occurrence.ok_or("Selected occurrence is absent from the current native scene")?;
    supervisor
        .geometry(GeometryCommand::PrepareBedPose {
            session_id,
            base_revision,
            project_revision: info.revision,
            occurrence_id: occurrence.occurrence_id,
            expected_definition_id: occurrence.definition_id,
            expected_pose: occurrence.pose,
            envelope_mm: intent.printer.build_envelope,
            center_xy: center,
            contact_tolerance_mm: SECTION_PLANE_TOLERANCE_MM,
        })
        .await
}

pub(super) fn observe(supervisor: &mut Supervisor, operation: &NativeOperationView) {
    if supervisor
        .client
        .as_ref()
        .map(|client| &client.hello().session_id)
        != Some(&operation.session_id)
        || supervisor.manufacturing.pending.as_deref() != Some(operation.name.as_str())
        || !operation.status.is_terminal()
    {
        return;
    }
    supervisor.manufacturing.pending = None;
    if operation.status == JobStatus::Completed
        && let Some(JobResult::ManufacturingVerified {
            record,
            report,
            resource,
            replay_display,
            ..
        }) = &operation.result
    {
        supervisor.manufacturing.verified = Some(CapturedVerification {
            operation: operation.name.clone(),
            session: operation.session_id.clone(),
            record: record.clone(),
            report: report.clone(),
            resource: resource.clone(),
            replay: replay_display.clone(),
        });
    }
}

#[tauri::command]
pub async fn manufacturing_control(
    state: State<'_, EngineState>,
    command: ManufacturingCommand,
) -> Result<ManufacturingResponse, String> {
    let mut supervisor = state.active().await?;
    supervisor.refresh().await;
    let inspecting = matches!(&command, ManufacturingCommand::Inspect { .. });
    let mutating = matches!(
        &command,
        ManufacturingCommand::SetIntent { .. } | ManufacturingCommand::Compile { .. }
    );
    let session = supervisor
        .client
        .as_ref()
        .ok_or("engine is not running")?
        .hello()
        .session_id
        .clone();
    let response = match supervisor
        .client
        .as_ref()
        .ok_or("engine is not running")?
        .manufacturing(command)
        .await
    {
        Ok(response) => response,
        Err(ClientError::Manufacturing(error)) => {
            return Ok(ManufacturingResponse::Error { error });
        }
        Err(error) if !error.is_fatal() => return Err(error.to_string()),
        Err(error) => {
            return Err(supervisor
                .disconnect(format!("manufacturing exchange failed: {error}"))
                .await);
        }
    };
    if inspecting || mutating {
        supervisor.manufacturing.verified = None;
        supervisor.manufacturing.pending = None;
        if inspecting && let ManufacturingResponse::OperationAccepted { operation } = &response {
            supervisor.manufacturing.pending = Some(operation.name.clone());
        }
        if inspecting
            && let ManufacturingResponse::Verified {
                record,
                report,
                resource,
                replay_display,
                ..
            } = &response
        {
            supervisor.manufacturing.verified = Some(CapturedVerification {
                operation: uuid::Uuid::new_v4().to_string(),
                session,
                record: record.clone(),
                report: report.clone(),
                resource: resource.clone(),
                replay: replay_display.clone(),
            });
        }
    }
    Ok(response)
}

async fn check_active(
    supervisor: &mut Supervisor,
    session: &SessionId,
    record: &ManufacturingArtifactRecord,
    operation: Option<&str>,
) -> Result<CapturedVerification, String> {
    supervisor.refresh().await;
    let client = supervisor.client.as_ref().ok_or("engine is not running")?;
    if &client.hello().session_id != session {
        return Err("stale_revision: old manufacturing session".into());
    }
    let capture = supervisor
        .manufacturing
        .verified
        .as_ref()
        .filter(|capture| {
            &capture.session == session
                && &capture.record == record
                && operation.is_none_or(|name| name == capture.operation)
        })
        .ok_or("stale_revision: verify the active bundle again before inspecting or exporting")?
        .clone();
    let status = client
        .manufacturing(ManufacturingCommand::Get {
            session_id: session.clone(),
        })
        .await
        .map_err(|error| error.to_string())?;
    if !matches!(&status, ManufacturingResponse::Status { artifact: Some(active), resource: Some(resource), .. }
        if active == record && resource == &capture.resource)
    {
        return Err("stale_revision: the freshly verified bundle is no longer active".into());
    }
    Ok(capture)
}

#[tauri::command]
pub async fn manufacturing_replay(
    state: State<'_, EngineState>,
    descriptor: ReplayDisplayDescriptor,
    record: ManufacturingArtifactRecord,
) -> Result<tauri::ipc::Response, String> {
    let (session, capture) = {
        let mut supervisor = state.active().await?;
        let session = supervisor
            .client
            .as_ref()
            .ok_or("engine is not running")?
            .hello()
            .session_id
            .clone();
        let capture = check_active(&mut supervisor, &session, &record, None).await?;
        if capture.replay.as_ref() != Some(&descriptor) {
            return Err(
                "corrupt_artifact: replay is not the captured fresh verification output".into(),
            );
        }
        (session, capture)
    };
    let bytes = rpc(&state)
        .await?
        .fetch_replay_display(&record, &descriptor)
        .await
        .map_err(|error| error.to_string())?;
    let mut supervisor = state.active().await?;
    check_active(&mut supervisor, &session, &record, Some(&capture.operation)).await?;
    Ok(tauri::ipc::Response::new(bytes))
}

#[tauri::command]
pub async fn manufacturing_layers(
    state: State<'_, EngineState>,
    record: ManufacturingArtifactRecord,
) -> Result<Vec<ManufacturingLayer>, String> {
    let (session, capture) = {
        let mut supervisor = state.active().await?;
        let session = supervisor
            .client
            .as_ref()
            .ok_or("engine is not running")?
            .hello()
            .session_id
            .clone();
        let capture = check_active(&mut supervisor, &session, &record, None).await?;
        (session, capture)
    };
    let bundle = rpc(&state)
        .await?
        .fetch_manufacturing_bundle(&record, &capture.resource)
        .await
        .map_err(|error| error.to_string())?;
    let mut supervisor = state.active().await?;
    check_active(&mut supervisor, &session, &record, Some(&capture.operation)).await?;
    drop(supervisor);
    let recipe = &bundle.provenance.intent.recipe;
    let mut layers = bundle
        .plan
        .layers
        .iter()
        .map(|layer| ManufacturingLayer {
            index: layer.index,
            z_mm: layer.z_mm,
            lower_z_mm: layer.lower_z_mm,
            height_mm: layer.height_mm,
            section_z_mm: layer.section_z_mm,
            deposition_segments: layer
                .paths
                .iter()
                .map(|path| path.points_mm.len() - 1)
                .sum(),
            sparse_band: None,
            diaphragms: layer.region_coverage.process_diaphragms.iter().map(|plate| ManufacturingDiaphragm {
                occurrence_id: plate.occurrence_id, provenance: plate.provenance,
                source_plane_mm: plate.source_plane_mm, requested_thickness_mm: plate.requested_thickness_mm,
                physical_z_mm: plate.physical_z_mm, region_index: plate.region_index, area_mm2: plate.area_mm2,
                nominal_path_volume_mm3: layer.paths.iter().filter(|path| path.owner == Some(
                    DepositionOwner::Regional { region_index: plate.region_index })).map(|path| {
                    let height = path.height_mm();
                    let area = ((path.bead_width_mm - height) * height
                        + std::f64::consts::PI * (height * 0.5).powi(2)) * recipe.flow_multiplier;
                    let factor = if path.kind == spiling_contracts::manufacturing::DepositionKind::Bridge {
                        recipe.process.bridge_flow_factor
                    } else { 1.0 } * if layer.index == 0 { recipe.process.first_layer_flow_factor } else { 1.0 };
                    path.points_mm.windows(2).map(|edge| (edge[1][0] - edge[0][0])
                        .hypot(edge[1][1] - edge[0][1]) * area * factor).sum::<f64>()
                }).sum(),
            }).collect(),
        })
        .collect::<Vec<_>>();
    // One bounded summary per terminal layer, not per-stroke JSON metadata.
    // Its earliest start also lets intermediate fine layers explain deferral.
    for band in &bundle.plan.sparse_bands {
        let view = layers[band.last_layer as usize]
            .sparse_band
            .get_or_insert_with(|| ManufacturingSparseBand {
                first_layer: band.first_layer,
                combined_regions: Vec::new(),
                combined_global: false,
                retained_fine: Vec::new(),
            });
        view.first_layer = view.first_layer.min(band.first_layer);
        match band.disposition {
            SparseBandDisposition::Combined => {
                if let Some(DepositionOwner::Regional { region_index }) = band.owner {
                    view.combined_regions.push(region_index);
                } else {
                    view.combined_global = true;
                }
            }
            SparseBandDisposition::RetainedFine { reason } => {
                if let Some((_, count)) = view
                    .retained_fine
                    .iter_mut()
                    .find(|(existing, _)| *existing == reason)
                {
                    *count += 1;
                } else {
                    view.retained_fine.push((reason, 1));
                }
            }
        }
    }
    Ok(layers)
}

async fn install_export(
    state: &EngineState,
    session: SessionId,
    path: PathBuf,
) -> Result<SelectedManufacturingExport, String> {
    if std::fs::symlink_metadata(&path).is_ok() {
        return Err("export requires a new destination directory; nothing is overwritten".into());
    }
    let parent = path
        .parent()
        .ok_or("export destination has no parent")?
        .canonicalize()
        .map_err(|error| error.to_string())?;
    if !parent.is_dir() {
        return Err("export parent is not a directory".into());
    }
    let name = path.file_name().ok_or("export destination has no name")?;
    let path = parent.join(name);
    let public = SelectedManufacturingExport {
        token: uuid::Uuid::new_v4().to_string(),
        session_id: session.clone(),
        label: bounded_text(&name.to_string_lossy(), MAX_DISPLAY_LABEL_BYTES as usize),
    };
    let mut supervisor = state.active().await?;
    supervisor.refresh().await;
    if supervisor
        .client
        .as_ref()
        .map(|client| &client.hello().session_id)
        != Some(&session)
    {
        return Err("stale_revision: session changed during destination selection".into());
    }
    supervisor.manufacturing.selection = Some(ExportSelection {
        public: public.clone(),
        path,
    });
    Ok(public)
}

#[tauri::command]
pub async fn manufacturing_select_export(
    app: AppHandle,
    state: State<'_, EngineState>,
) -> Result<Option<SelectedManufacturingExport>, String> {
    let session = selection_session(&state).await?;
    let (send, receive) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let path = rfd::FileDialog::new()
            .set_title("New manufacturing export directory")
            .set_file_name("manufacturing-export")
            .save_file();
        let _ = send.send(path);
    })
    .map_err(|error| error.to_string())?;
    match receive
        .await
        .map_err(|_| "export picker closed without a result")?
    {
        None => Ok(None),
        Some(path) => install_export(&state, session, path).await.map(Some),
    }
}

#[tauri::command]
pub async fn manufacturing_debug_select_export(
    state: State<'_, EngineState>,
    relative_path: String,
) -> Result<SelectedManufacturingExport, String> {
    let root = state
        .fixture_root
        .as_ref()
        .ok_or("manufacturing fixture selection is disabled")?;
    let path = resolve_fixture_project(
        root,
        &relative_path,
        spiling_contracts::project::ProjectPathIntent::Save,
    )?;
    install_export(&state, selection_session(&state).await?, path).await
}

#[tauri::command]
pub async fn manufacturing_export(
    state: State<'_, EngineState>,
    token: String,
    session_id: SessionId,
    record: ManufacturingArtifactRecord,
) -> Result<ManufacturingExportReceipt, String> {
    let (selection, capture, client) = {
        let mut supervisor = state.active().await?;
        let capture = check_active(&mut supervisor, &session_id, &record, None).await?;
        let selection = supervisor
            .manufacturing
            .selection
            .take()
            .ok_or("unknown export token; select a new destination")?;
        if selection.public.token != token || selection.public.session_id != session_id {
            supervisor.manufacturing.selection = Some(selection);
            return Err("unknown export token or old session".into());
        }
        let client = supervisor
            .client
            .as_ref()
            .ok_or("engine is not running")?
            .rpc();
        (selection, capture, client)
    };
    // Tokens are single-use after admission, including a failed export: no implicit retry.
    let bundle = client
        .fetch_manufacturing_bundle(&record, &capture.resource)
        .await
        .map_err(|error| error.to_string())?;
    let report = capture.report;
    let path = selection.path;
    let mut output = tauri::async_runtime::spawn_blocking(move || {
        let mut output = OutputDirectory::create(&path).map_err(|error| error.to_string())?;
        output
            .write_manufacturing_bundle(&bundle, &report)
            .map_err(|error| error.to_string())?;
        Ok::<_, String>(output)
    })
    .await
    .map_err(|error| error.to_string())??;
    // Publication owns the short lifecycle lock. A changed session/artifact removes staging.
    let mut supervisor = state.active().await?;
    check_active(
        &mut supervisor,
        &session_id,
        &record,
        Some(&capture.operation),
    )
    .await?;
    output.commit();
    Ok(ManufacturingExportReceipt {
        label: selection.public.label,
        files: [
            "plan.json",
            "program.gcode",
            "provenance.json",
            "verification.json",
        ],
    })
}

#[tauri::command]
pub async fn manufacturing_diagnostic_attach(
    app: AppHandle,
    state: State<'_, EngineState>,
) -> Result<spiling_contracts::Hello, String> {
    if !state.manufacturing_diagnostic_enabled() {
        return Err("manufacturing diagnostic attachment was not enabled at native startup".into());
    }
    let store = super::operation_store(&app)?;
    state.active().await?.start(&store).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Vec<u8> {
        include_bytes!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../fixtures/manufacturing/solid-fill.intent.json"
        ))
        .to_vec()
    }

    #[test]
    fn inert_intent_admission_is_bounded_strict_and_validated() {
        let bytes = fixture();
        let mut padded = bytes.clone();
        padded.resize(MAX_INTENT_BYTES, b' ');
        assert!(read_intent(padded.as_slice()).is_ok());
        padded.push(b' ');
        assert!(read_intent(padded.as_slice()).is_err());
        assert!(read_intent(b"export default {};".as_slice()).is_err());
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value["execute"] = serde_json::json!("arbitrary script");
        assert!(read_intent(serde_json::to_vec(&value).unwrap().as_slice()).is_err());
        value.as_object_mut().unwrap().remove("execute");
        value["recipe"]["layer_height_mm"] = serde_json::json!(0);
        assert!(read_intent(serde_json::to_vec(&value).unwrap().as_slice()).is_err());
        let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        value["recipe"].as_object_mut().unwrap().remove("process");
        assert!(read_intent(serde_json::to_vec(&value).unwrap().as_slice()).is_err());
    }

    #[test]
    fn intent_tokens_are_session_bound_single_use_and_cleared_on_lifecycle() {
        let intent = read_intent(fixture().as_slice()).unwrap();
        let session = SessionId::new();
        let mut state = ManufacturingSession {
            intent_selection: Some(IntentSelection {
                public: SelectedManufacturingIntent {
                    token: "selected".into(),
                    session_id: session.clone(),
                    label: "intent.json".into(),
                },
                intent: intent.clone(),
            }),
            ..Default::default()
        };
        assert!(state.load_intent("wrong", &session).is_err());
        assert!(state.load_intent("selected", &SessionId::new()).is_err());
        assert_eq!(state.load_intent("selected", &session).unwrap(), intent);
        assert!(state.load_intent("selected", &session).is_err());
        state.intent_selection = Some(IntentSelection {
            public: SelectedManufacturingIntent {
                token: "next".into(),
                session_id: session.clone(),
                label: "intent.json".into(),
            },
            intent,
        });
        state.clear();
        assert!(state.load_intent("next", &session).is_err());
    }
}
