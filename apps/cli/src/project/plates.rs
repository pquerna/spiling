// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
//! Explicit ordered print-job operations through the shared project service.
use super::*;
use spiling_contracts::project::{PrintLayoutEdit, PrintLayoutId};
#[derive(Clone)]
pub(super) enum Action {
    Select(String),
    Create(String),
    Rename(String),
    Duplicate(String),
    Remove,
    Transfer {
        target: String,
        occurrence: OccurrenceId,
        copy: bool,
    },
    Edit(PrintLayoutEdit),
}
fn resolve(info: &ProjectInfo, value: &str) -> Result<PrintLayoutId, Error> {
    if let Ok(index) = value.parse::<usize>() {
        return index
            .checked_sub(1)
            .and_then(|i| info.layouts.get(i))
            .map(|l| l.layout_id.clone())
            .ok_or_else(|| "plate index is outside this project".into());
    }
    let id = PrintLayoutId::parse(value)?;
    info.layouts
        .iter()
        .find(|l| l.layout_id == id)
        .map(|l| l.layout_id.clone())
        .ok_or_else(|| "unknown plate ID".into())
}
pub(super) async fn execute(client: &mut EngineClient, action: Action) -> Result<(), Error> {
    let p = info(client).await?;
    let session_id = client.hello().session_id.clone();
    let command = match action {
        Action::Select(value) => ProjectCommand::SelectPrintLayout {
            session_id,
            base_revision: p.revision,
            layout_id: resolve(&p, &value)?,
        },
        action => {
            let edit = match action {
                Action::Create(name) => PrintLayoutEdit::Create {
                    name,
                    copy_setup_from: Some(p.layout_id),
                },
                Action::Rename(name) => PrintLayoutEdit::Rename {
                    name,
                    layout_id: p.layout_id,
                },
                Action::Duplicate(name) => PrintLayoutEdit::Duplicate {
                    name,
                    source: p.layout_id,
                },
                Action::Remove => PrintLayoutEdit::Remove {
                    layout_id: p.layout_id,
                },
                Action::Transfer {
                    target,
                    occurrence,
                    copy,
                } => PrintLayoutEdit::Transfer {
                    target: resolve(&p, &target)?,
                    source: p.layout_id,
                    occurrences: vec![occurrence],
                    copy,
                },
                Action::Edit(edit) => edit,
                Action::Select(_) => unreachable!(),
            };
            edit.validate()?;
            ProjectCommand::EditPrintLayout {
                session_id,
                base_revision: p.revision,
                edit,
            }
        }
    };
    client.project(command).await?;
    Ok(())
}
