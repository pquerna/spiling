// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Frontend orchestration of native manufacturing. No planner, verifier or G-code parser.
use spiling_contracts::{
    ArtifactView,
    geometry::{NativePath, SessionId},
    manufacturing::*,
    project::ProjectInfo,
};
use spiling_engine_client::{ClientError, EngineRpc, export::OutputDirectory};
use std::{io::Read, path::PathBuf, sync::Arc};

#[derive(Debug, Clone)]
pub struct LoadedIntent {
    pub nonce: u64,
    pub label: String,
    pub intent: Arc<ManufacturingIntent>,
}
#[derive(Debug, Clone)]
pub struct LoadedMaterial {
    pub nonce: u64,
    pub label: String,
    pub project: spiling_contracts::project::ProjectId,
    pub revision: spiling_contracts::project::ProjectRevision,
    pub session: SessionId,
    pub selection: MaterialProfileSelection,
}
#[derive(Debug, Clone)]
pub struct Capture {
    pub nonce: u64,
    pub session: SessionId,
    pub observed_revision: spiling_contracts::project::ProjectRevision,
    pub record: ManufacturingArtifactRecord,
    pub report: VerificationReport,
    pub provenance: ManufacturingInspectionProvenance,
    pub resource: ArtifactView,
    pub replay: Option<ReplayDisplayDescriptor>,
}
#[derive(Debug)]
pub struct Review {
    pub capture: Arc<Capture>,
    pub bundle: Arc<ManufacturingBundle>,
    pub motion: Option<Arc<[u8]>>,
    pub display: Option<Arc<crate::replay::Replay>>,
}
#[derive(Debug, Clone, Default)]
pub struct State {
    pub revision: Option<(
        spiling_contracts::project::ProjectId,
        spiling_contracts::project::ProjectRevision,
    )>,
    pub intent: Option<Arc<ManufacturingIntent>>,
    pub artifact: Option<ManufacturingArtifactRecord>,
    pub resource: Option<ArtifactView>,
    pub loaded: Option<Arc<LoadedIntent>>,
    pub loaded_material: Option<Arc<LoadedMaterial>>,
    pub preflight: Option<Arc<PreflightReport>>,
    pub capture: Option<Arc<Capture>>,
    pub review: Option<Arc<Review>>,
    pub retired_display_bytes: usize,
    pub loading: bool,
    pub export_busy: bool,
    pub last_export: Option<String>,
}
impl State {
    pub fn current_capture(
        &self,
        project: &ProjectInfo,
        session: &SessionId,
    ) -> Option<&Arc<Capture>> {
        self.capture.as_ref().filter(|c| {
            &c.session == session
                && c.provenance.project_id == project.project_id
                && c.observed_revision == project.revision
                && self.artifact.as_ref() == Some(&c.record)
                && self.resource.as_ref() == Some(&c.resource)
        })
    }
    pub fn display_bytes(&self) -> usize {
        self.retired_display_bytes.max(
            self.review
                .as_ref()
                .and_then(|r| r.display.as_ref())
                .map_or(0, |r| r.owned_bytes),
        )
    }
    pub fn invalidate(&mut self) {
        // Conservatively retain the prior GPU charge until this engine session ends.
        self.retired_display_bytes = self.display_bytes();
        self.capture = None;
        self.review = None;
        self.last_export = None;
        self.loading = false;
    }
}
fn invalid(message: impl Into<String>) -> ClientError {
    ClientError::InvalidRequest(message.into())
}
fn read_declaration(path: &NativePath) -> Result<(Vec<u8>, String), String> {
    let path = PathBuf::from(path.to_os_string().map_err(|e| e.to_string())?);
    let file = std::fs::File::open(&path).map_err(|e| format!("Cannot open declaration: {e}"))?;
    if !file.metadata().map_err(|e| e.to_string())?.is_file() {
        return Err("Declaration must be a regular JSON file".into());
    }
    let mut bytes = Vec::new();
    file.take(u64::from(MAX_PROFILE_JSON_BYTES) + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_PROFILE_JSON_BYTES as usize {
        return Err("Declaration exceeds the profile JSON byte limit".into());
    }
    let label = spiling_contracts::geometry::bounded_text(
        &path.file_name().unwrap_or_default().to_string_lossy(),
        256,
    );
    Ok((bytes, label))
}
pub fn read_intent(path: &NativePath) -> Result<(Arc<ManufacturingIntent>, String), String> {
    let (bytes, label) = read_declaration(path)?;
    let intent = decode_intent(&bytes).map_err(|e| e.to_string())?;
    Ok((Arc::new(intent), label))
}
pub fn read_material(path: &NativePath) -> Result<(MaterialProfileSelection, String), String> {
    let (bytes, label) = read_declaration(path)?;
    let selection: MaterialProfileSelection = serde_json::from_slice(&bytes)
        .map_err(|e| format!("Invalid material profile selection: {e}"))?;
    selection.validate().map_err(|e| e.to_string())?;
    Ok((selection, label))
}
pub async fn check_current(rpc: &EngineRpc, capture: &Capture) -> Result<(), ClientError> {
    capture.provenance.validate_for_record(&capture.record)?;
    match rpc
        .manufacturing(ManufacturingCommand::Get {
            session_id: capture.session.clone(),
        })
        .await?
    {
        ManufacturingResponse::Status {
            info,
            artifact: Some(record),
            resource: Some(resource),
            ..
        } if info.project_id == capture.provenance.project_id
            && info.revision == capture.observed_revision
            && record == capture.record
            && resource == capture.resource =>
        {
            Ok(())
        }
        _ => Err(invalid(
            "The freshly verified manufacturing output is no longer current; verify again",
        )),
    }
}
pub async fn load_review(
    rpc: EngineRpc,
    capture: Arc<Capture>,
    reserved: usize,
    origin: glam::DVec3,
) -> Result<Arc<Review>, ClientError> {
    check_current(&rpc, &capture).await?;
    let bundle = rpc
        .fetch_manufacturing_bundle(&capture.record, &capture.resource)
        .await?;
    if bundle.provenance.input_hash != capture.provenance.input_hash {
        return Err(invalid("Captured bundle/provenance input mismatch"));
    }
    let motion: Option<Arc<[u8]>> = match &capture.replay {
        Some(descriptor) => Some(
            rpc.fetch_replay_display(&capture.record, descriptor)
                .await?
                .into(),
        ),
        None => None,
    };
    let display = match (&motion, &capture.replay) {
        (Some(bytes), Some(descriptor)) => Some(Arc::new(
            crate::replay::Replay::project(bytes, descriptor, reserved, origin).map_err(invalid)?,
        )),
        _ => None,
    };
    check_current(&rpc, &capture).await?;
    Ok(Arc::new(Review {
        capture,
        bundle: Arc::new(bundle),
        motion,
        display,
    }))
}
pub struct StagedExport {
    pub output: OutputDirectory,
    pub capture: Arc<Capture>,
    pub label: String,
}
pub async fn stage_export(
    rpc: EngineRpc,
    review: Arc<Review>,
    path: NativePath,
) -> Result<StagedExport, String> {
    check_current(&rpc, &review.capture)
        .await
        .map_err(|e| e.to_string())?;
    let capture = review.capture.clone();
    let path = PathBuf::from(path.to_os_string().map_err(|e| e.to_string())?);
    let label = spiling_contracts::geometry::bounded_text(
        &path.file_name().unwrap_or_default().to_string_lossy(),
        256,
    );
    let output = tokio::task::spawn_blocking(move || {
        let mut output = OutputDirectory::create(&path).map_err(|e| e.to_string())?;
        output
            .write_manufacturing_bundle(
                &review.bundle,
                &review.capture.report,
                &review.capture.provenance,
            )
            .map_err(|e| e.to_string())?;
        Ok::<_, String>(output)
    })
    .await
    .map_err(|e| e.to_string())??;
    check_current(&rpc, &capture)
        .await
        .map_err(|e| e.to_string())?;
    Ok(StagedExport {
        output,
        capture,
        label,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn profile_files_are_bounded_strict_and_preserve_setup_and_overrides() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Matériau Ω.json");
        let native = NativePath::from_os_str(path.as_os_str()).unwrap();
        let mut selection: MaterialProfileSelection = serde_json::from_slice(include_bytes!(
            "../../../fixtures/manufacturing/material-profile-a.json"
        ))
        .unwrap();
        selection.overrides.flow_multiplier = Some(0.95);
        std::fs::write(&path, serde_json::to_vec(&selection).unwrap()).unwrap();
        let (loaded, label) = read_material(&native).unwrap();
        assert_eq!(loaded, selection);
        assert_eq!(label, "Matériau Ω.json");
        let mut unknown = serde_json::to_value(&selection).unwrap();
        unknown["implicit_default"] = serde_json::Value::Bool(true);
        std::fs::write(&path, serde_json::to_vec(&unknown).unwrap()).unwrap();
        assert!(
            read_material(&native)
                .unwrap_err()
                .contains("unknown field")
        );
        std::fs::write(&path, vec![b' '; MAX_PROFILE_JSON_BYTES as usize + 1]).unwrap();
        assert!(read_material(&native).unwrap_err().contains("byte limit"));
        selection.setup.hotend_id = "incompatible".into();
        std::fs::write(&path, serde_json::to_vec(&selection).unwrap()).unwrap();
        assert!(read_material(&native).is_err());
    }
}
