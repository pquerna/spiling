// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::*;
use spiling_contracts::analysis::*;

pub(super) enum Action {
    Saved(String),
    Attach(SourceHash),
    RunAndAttach(String, Arc<mechanics::AnalysisRunSettings>),
    Return(String),
    Run(String, Arc<mechanics::AnalysisRunSettings>),
    Case(String, OccurrenceId, AnalysisMode),
    Load(String, AnalysisFaceLoad),
    Fix(String, FaceId),
    Edit(AnalysisEdit),
}

pub(super) fn parse(
    flag: &str,
    args: &mut impl Iterator<Item = OsString>,
) -> Result<Action, Error> {
    Ok(match flag {
        "--analysis-saved" => Action::Saved(text(args, "saved analysis case name")?),
        "--analysis-attach" => {
            Action::Attach(SourceHash::parse(text(args, "analysis input hash")?)?)
        }
        "--analysis-return" => Action::Return(text(args, "retained analysis operation name")?),
        "--analysis-run" | "--analysis-run-and-attach" => {
            let name = text(args, "saved analysis case name")?;
            let settings: mechanics::AnalysisRunSettings = appearance_json(args, flag)?;
            settings.validate()?;
            if flag == "--analysis-run-and-attach" {
                Action::RunAndAttach(name, Arc::new(settings))
            } else {
                Action::Run(name, Arc::new(settings))
            }
        }
        "--analysis-case" => {
            let name = text(args, "analysis case name")?;
            let occurrence = OccurrenceId::new(text(args, "analysis occurrence")?.parse()?)?;
            let mode = match text(args, "analysis mode")?.as_str() {
                "basic" => AnalysisMode::Basic,
                "enhanced" => AnalysisMode::Enhanced,
                _ => {
                    return Err(
                        "analysis mode must be basic or enhanced; reference is not a user mode"
                            .into(),
                    );
                }
            };
            Action::Case(name, occurrence, mode)
        }
        "--analysis-load" => {
            let name = text(args, "analysis case name")?;
            let face_id = FaceId::parse(text(args, "analysis whole source face")?)?;
            let mut components = [0.0_f64; 6];
            for component in &mut components {
                *component = text(args, "analysis force N / moment N.mm component")?.parse()?;
                if !component.is_finite() {
                    return Err("analysis wrench must be finite".into());
                }
            }
            Action::Load(
                name,
                AnalysisFaceLoad {
                    face_id,
                    force_n: [components[0], components[1], components[2]],
                    moment_n_mm: [components[3], components[4], components[5]],
                },
            )
        }
        "--analysis-fix" => Action::Fix(
            text(args, "analysis case name")?,
            FaceId::parse(text(args, "analysis whole source face")?)?,
        ),
        "--analysis-edit" => {
            let json = text(args, "analysis edit JSON")?;
            if json.len() > MAX_ANALYSIS_DOCUMENT_BYTES {
                return Err("analysis edit exceeds control bound".into());
            }
            let edit: AnalysisEdit = serde_json::from_str(&json)?;
            edit.validate()?;
            Action::Edit(edit)
        }
        _ => return Err(format!("unknown analysis option {flag}").into()),
    })
}

pub(super) async fn execute(
    client: &mut EngineClient,
    session: &SessionId,
    action: Action,
) -> Result<Option<serde_json::Value>, Error> {
    let captured = info(client).await?;
    if let Action::Attach(input_hash) = action {
        let response = client
            .project(ProjectCommand::AttachAnalysisResult {
                session_id: session.clone(),
                base_revision: captured.revision,
                input_hash,
            })
            .await?;
        complete(client, response).await?;
        return Ok(None);
    }
    if let Action::Saved(name) = action {
        let record = captured
            .analysis
            .cases
            .iter()
            .find(|record| record.case.layout_id == captured.layout_id && record.case.name == name)
            .ok_or("saved analysis case is absent from selected plate")?;
        let response = client
            .project(ProjectCommand::GetAnalysisResult {
                session_id: session.clone(),
                base_revision: captured.revision,
                case_id: record.case.id.clone(),
            })
            .await?;
        let ProjectResponse::OperationAccepted { operation } = response else {
            return Err("saved analysis read did not return an operation".into());
        };
        let name = operation.name.clone();
        let result = geometry::wait_operation(client, session, operation).await?;
        return Ok(Some(inspect_result(client, &captured, name, result).await?));
    }
    if let Action::Return(name) = action {
        let operation = client.native_operation(&name).await?;
        if !operation.done || operation.status != JobStatus::Completed {
            return Err("retained analysis operation is not a completed success".into());
        }
        return Ok(Some(
            inspect_result(
                client,
                &captured,
                name,
                operation.result.ok_or("retained operation has no result")?,
            )
            .await?,
        ));
    }
    let attach = matches!(&action, Action::RunAndAttach(..));
    if let Action::Run(name, settings) | Action::RunAndAttach(name, settings) = action {
        let record = captured
            .analysis
            .cases
            .iter()
            .find(|record| record.case.layout_id == captured.layout_id && record.case.name == name)
            .ok_or("saved analysis case is absent from the selected plate")?;
        let response = client
            .project(ProjectCommand::RunAnalysis {
                session_id: session.clone(),
                base_revision: captured.revision,
                case_id: record.case.id.clone(),
                settings,
            })
            .await?;
        let ProjectResponse::OperationAccepted { operation } = response else {
            return Err("analysis did not return an operation".into());
        };
        let name = operation.name.clone();
        let result = geometry::wait_operation(client, session, operation).await?;
        if attach {
            let JobResult::AnalysisCompleted { input_hash, .. } = &result else {
                return Err("analysis did not complete".into());
            };
            let response = client
                .project(ProjectCommand::AttachAnalysisResult {
                    session_id: session.clone(),
                    base_revision: captured.revision,
                    input_hash: input_hash.clone(),
                })
                .await?;
            complete(client, response).await?;
        }
        return Ok(Some(inspect_result(client, &captured, name, result).await?));
    }
    let edit = match action {
        Action::Edit(edit) => edit,
        Action::Case(name, occurrence_id, mode) => {
            let scene = geometry::scene(client, session).await?;
            let (definitions, occurrences) = geometry::records(client, &scene).await?;
            let occurrence = occurrences
                .iter()
                .find(|occurrence| occurrence.occurrence_id == occurrence_id)
                .ok_or("analysis occurrence is not in the selected print layout")?;
            let definition = definitions
                .iter()
                .find(|definition| definition.definition_id == occurrence.definition_id)
                .ok_or("analysis source definition is missing")?;
            let existing = captured.analysis.cases.iter().find(|record| {
                record.case.layout_id == captured.layout_id && record.case.name == name
            });
            let mut case = existing
                .map(|record| record.case.clone())
                .unwrap_or_else(|| AnalysisCase {
                    id: AnalysisCaseId::new(),
                    name,
                    layout_id: captured.layout_id.clone(),
                    occurrence_id,
                    definition_id: occurrence.definition_id.clone(),
                    source_hash: definition.provenance.source_hash.clone(),
                    mode,
                    loads: vec![],
                    restraints: vec![],
                });
            if case.occurrence_id != occurrence_id {
                return Err("existing analysis name belongs to a different subject; create a different case".into());
            }
            case.mode = mode;
            AnalysisEdit::Put { case }
        }
        action => {
            let name = match &action {
                Action::Load(name, _) | Action::Fix(name, _) => name,
                _ => unreachable!(),
            };
            let mut case = captured.analysis.cases.iter().find(|record| record.case.layout_id == captured.layout_id && record.case.name == *name)
                .ok_or("analysis case is absent from the selected print layout; use --analysis-case first")?.case.clone();
            match action {
                Action::Load(_, load) => {
                    if let Some(existing) = case
                        .loads
                        .iter_mut()
                        .find(|existing| existing.face_id == load.face_id)
                    {
                        *existing = load;
                    } else {
                        case.loads.push(load);
                    }
                }
                Action::Fix(_, face_id) => {
                    let restraint = AnalysisFaceRestraint {
                        face_id,
                        fixed_translation: [true; 3],
                    };
                    if let Some(existing) = case
                        .restraints
                        .iter_mut()
                        .find(|existing| existing.face_id == restraint.face_id)
                    {
                        *existing = restraint;
                    } else {
                        case.restraints.push(restraint);
                    }
                }
                _ => unreachable!(),
            }
            AnalysisEdit::Put { case }
        }
    };
    let response = client
        .project(ProjectCommand::EditAnalysis {
            session_id: session.clone(),
            base_revision: captured.revision,
            edit,
        })
        .await?;
    complete(client, response).await?;
    Ok(None)
}

async fn inspect_result(
    client: &mut EngineClient,
    captured: &ProjectInfo,
    name: String,
    result: JobResult,
) -> Result<serde_json::Value, Error> {
    let (project_id, input_hash, captured_revision, resource) = match result {
        JobResult::AnalysisResultPrepared {
            project_id,
            read_revision,
            record,
            resource,
        } => {
            if read_revision != captured.revision {
                return Err("saved analysis read revision differs".into());
            }
            (
                project_id,
                record.input_hash,
                record.captured_revision,
                resource,
            )
        }
        JobResult::AnalysisCompleted {
            project_id,
            input_hash,
            captured_revision,
            resource,
        } => (project_id, input_hash, captured_revision, resource),
        _ => return Err("operation did not return analysis fields".into()),
    };
    if project_id != captured.project_id {
        return Err("retained analysis result belongs to another project".into());
    }
    let bytes = client
        .fetch_analysis_fields(&input_hash, captured_revision, &resource)
        .await?;
    let fields = fields::AnalysisFields::decode(&bytes)?;
    let current = info(client).await?;
    let session = client.hello().session_id.clone();
    let scene = geometry::scene(client, &session).await?;
    let (_, occurrences) = geometry::records(client, &scene).await?;
    let input_matches = current.project_id == project_id
        && current.analysis.cases.contains(&fields.summary.case)
        && current.surface_overlays.layers.is_empty()
        && current.process_document.layers.is_empty()
        && occurrences.iter().any(|occurrence| {
            occurrence.occurrence_id == fields.summary.case.case.occurrence_id
                && occurrence.definition_id == fields.summary.case.case.definition_id
                && occurrence.pose == fields.summary.pose
        });
    let samples=(0..fields.element_count.min(16)).map(|index|{let field=fields.element(index).unwrap();let region=["wall","skin","assumed_effective_core"][field.role as usize];serde_json::json!({"element":index,"center_mm":field.center_mm,"region":region,"centroid_macro_stress_mpa":field.stress_mpa,"energy_n_mm":field.energy_n_mm,"regional_energy_concentration":field.concentration})}).collect::<Vec<_>>();
    Ok(
        serde_json::json!({"operation":name,"resource":resource,"capture_matches_project_revision":current.revision==captured_revision,"capture_matches_current_inputs":input_matches,"summary":fields.summary,"field_samples":samples,"scope":"assumed-law planned tied macro continuum; not resolved roads, discretization/calibration/strength qualification or a shipped Basic release; explicit attachment followed by save retains portable result bytes"}),
    )
}
