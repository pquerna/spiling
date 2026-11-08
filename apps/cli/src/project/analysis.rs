// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::*;
use spiling_contracts::analysis::*;

pub(super) enum Action {
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
) -> Result<(), Error> {
    let captured = info(client).await?;
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
    complete(client, response).await
}
