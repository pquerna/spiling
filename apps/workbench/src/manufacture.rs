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
pub struct LoadedCatalog {
    pub nonce: u64,
    pub label: String,
    pub project: spiling_contracts::project::ProjectId,
    pub session: SessionId,
    pub catalog: Arc<MaterialCatalog>,
}
#[derive(Debug, Clone)]
pub struct MaterialReview {
    pub project: spiling_contracts::project::ProjectId,
    pub session: SessionId,
    pub request: Arc<MaterialReviewRequest>,
    pub report: MaterialReviewReport,
}
impl MaterialReview {
    pub fn current(&self, project: &ProjectInfo, session: &SessionId) -> bool {
        self.project == project.project_id
            && self.session == *session
            && self.report.input_revision == project.revision
    }
}
#[derive(Debug, Clone)]
pub struct Capture {
    pub layout_id: spiling_contracts::project::PrintLayoutId,
    pub nonce: u64,
    pub session: SessionId,
    pub observed_revision: spiling_contracts::project::ProjectRevision,
    pub record: ManufacturingArtifactRecord,
    pub report: VerificationReport,
    pub provenance: ManufacturingInspectionProvenance,
    pub resource: ArtifactView,
    pub replay: Option<ReplayDisplayDescriptor>,
}
#[derive(Debug, Clone)]
pub struct Review {
    pub capture: Arc<Capture>,
    pub bundle: Arc<ManufacturingBundle>,
    pub motion: Option<spiling_chunks::ChunkSource>,
    pub display: Option<Arc<crate::replay::Replay>>,
    /// Why a verified result has no toolpath preview, when it has none.
    pub display_note: Option<String>,
}
/// A report-only inspection carries no actual-motion document.
pub const PREVIEW_OMITTED: &str = "Toolpaths were not requested for this inspection. Verification passed and export is available.";
#[derive(Debug, Clone)]
pub struct InspectedOption {
    pub project: spiling_contracts::project::ProjectId,
    pub revision: spiling_contracts::project::ProjectRevision,
    pub session: SessionId,
    pub option: spiling_contracts::project::ManufacturingOption,
}
#[derive(Debug, Clone)]
pub struct AppliedSettings {
    pub interior_edit: Option<spiling_contracts::manufacturing::InteriorToolPolicyEdit>,
    pub nonce: u64,
    pub project: spiling_contracts::project::ProjectId,
    pub session: SessionId,
    pub base_revision: spiling_contracts::project::ProjectRevision,
    pub revision: spiling_contracts::project::ProjectRevision,
    pub intent: Arc<ManufacturingIntent>,
}
#[derive(Debug, Clone, Default)]
pub struct State {
    pub interior_opportunity: Option<spiling_contracts::manufacturing::InteriorPolicyOpportunity>,
    pub layout_id: Option<spiling_contracts::project::PrintLayoutId>,
    pub context_hash: Option<spiling_contracts::geometry::SourceHash>,
    pub alternative: Option<Arc<ManufacturingAlternativeEvaluation>>,
    pub alternative_review: Option<Arc<Review>>,
    pub reference_review: Option<Arc<Review>>,
    pub alternative_loading: bool,
    pub inspected_option: Option<Arc<InspectedOption>>,
    pub applied_settings: Option<Arc<AppliedSettings>>,
    pub revision: Option<(
        spiling_contracts::project::ProjectId,
        spiling_contracts::project::ProjectRevision,
    )>,
    pub intent: Option<Arc<ManufacturingIntent>>,
    pub artifact: Option<ManufacturingArtifactRecord>,
    pub resource: Option<ArtifactView>,
    pub loaded: Option<Arc<LoadedIntent>>,
    pub loaded_material: Option<Arc<LoadedMaterial>>,
    pub loaded_catalog: Option<Arc<LoadedCatalog>>,
    pub preflight: Option<Arc<PreflightReport>>,
    pub material_review: Option<Arc<MaterialReview>>,
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
                && c.layout_id == project.layout_id
                && self.layout_id.as_ref() == Some(&project.layout_id)
                && c.provenance.project_id == project.project_id
                && c.observed_revision == project.revision
                && self.artifact.as_ref() == Some(&c.record)
                && self.resource.as_ref() == Some(&c.resource)
        })
    }
    pub fn display_bytes(&self) -> usize {
        let reviews = [
            self.review.as_ref(),
            self.reference_review.as_ref(),
            self.alternative_review.as_ref(),
        ];
        let displays = reviews
            .iter()
            .flatten()
            .filter_map(|review| review.display.as_ref())
            .map(|display| (display, display.owned_bytes));
        self.retired_display_bytes.max(unique_bytes(displays))
    }
    pub fn invalidate(&mut self) {
        // Conservatively retain the prior GPU charge until this engine session ends.
        self.retired_display_bytes = self.display_bytes();
        self.material_review = None;
        self.capture = None;
        self.review = None;
        self.last_export = None;
        self.loading = false;
    }
}
fn unique_bytes<'value, Value: 'value>(
    values: impl Iterator<Item = (&'value Arc<Value>, usize)>,
) -> usize {
    let mut seen = Vec::new();
    let mut total = 0;
    for (value, bytes) in values {
        let pointer = Arc::as_ptr(value);
        if !seen.contains(&pointer) {
            seen.push(pointer);
            total += bytes;
        }
    }
    total
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
pub fn read_catalog(path: &NativePath) -> Result<(Arc<MaterialCatalog>, String), String> {
    let (bytes, label) = read_declaration(path)?;
    let catalog: MaterialCatalog =
        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid material catalog: {e}"))?;
    catalog.validate().map_err(|e| e.to_string())?;
    Ok((Arc::new(catalog), label))
}
pub async fn check_current(rpc: &EngineRpc, capture: &Capture) -> Result<(), ClientError> {
    capture.provenance.validate_for_record(&capture.record)?;
    match rpc
        .manufacturing(ManufacturingCommand::Get {
            layout_id: capture.layout_id.clone(),
            session_id: capture.session.clone(),
        })
        .await?
    {
        ManufacturingResponse::Status {
            info,
            artifact: Some(record),
            resource: Some(resource),
            ..
        } if info.layout_id == capture.layout_id
            && info.project_id == capture.provenance.project_id
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
    origin: glam::DVec3,
) -> Result<Arc<Review>, ClientError> {
    check_current(&rpc, &capture).await?;
    let review = load_checked_review(rpc.clone(), capture, origin).await?;
    check_current(&rpc, &review.capture).await?;
    Ok(review)
}
pub async fn load_checked_review(
    rpc: EngineRpc,
    capture: Arc<Capture>,
    origin: glam::DVec3,
) -> Result<Arc<Review>, ClientError> {
    capture.provenance.validate_for_record(&capture.record)?;
    let bundle = rpc
        .fetch_manufacturing_bundle(&capture.record, &capture.resource)
        .await?;
    if bundle.provenance.input_hash != capture.provenance.input_hash {
        return Err(invalid("Captured bundle/provenance input mismatch"));
    }
    let motion: Option<spiling_chunks::ChunkSource> = match &capture.replay {
        Some(descriptor) => Some(
            rpc.fetch_replay_document(&capture.record, descriptor)
                .await?,
        ),
        None => None,
    };
    // Display preparation failure retains exact motion and verified export.
    let (motion, display, display_note) = match (motion, &capture.replay) {
        (Some(bytes), Some(descriptor)) => {
            match crate::replay::Replay::project_document(&bytes, descriptor, origin) {
                Ok(display) => (Some(bytes), Some(Arc::new(display)), None),
                Err(error) => (
                    Some(bytes),
                    None,
                    Some(format!(
                        "Toolpath display unavailable: {error}. Verification and export remain available."
                    )),
                ),
            }
        }
        _ => (None, None, Some(PREVIEW_OMITTED.to_owned())),
    };
    Ok(Arc::new(Review {
        capture,
        bundle: Arc::new(bundle),
        motion,
        display,
        display_note,
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
    let program = rpc
        .fetch_manufacturing_program(&capture.record)
        .await
        .map_err(|e| e.to_string())?;
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
                &program,
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
    fn alternative_reference_budget_counts_each_shared_allocation_once() {
        let current = Arc::new(1);
        let candidate = Arc::new(2);
        let reference = current.clone();
        assert_eq!(
            unique_bytes([(&current, 20), (&reference, 20), (&candidate, 30)].into_iter()),
            50
        );
        let replacement_current = Arc::new(3);
        assert_eq!(
            unique_bytes(
                [
                    (&replacement_current, 40),
                    (&reference, 20),
                    (&candidate, 30)
                ]
                .into_iter()
            ),
            90
        );
    }
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
