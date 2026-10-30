// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::host::{Phase, Snapshot};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    New,
    Open,
    OpenReadOnly,
    Recover,
    Save,
    SaveAs,
    Import,
    Undo,
    Redo,
    Restart,
    Close,
    Cancel,
    Resume,
    License,
    Gallery,
}
impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Self::New => "New project",
            Self::Open => "Open project…",
            Self::OpenReadOnly => "Open read-only…",
            Self::Recover => "Recover previous checkpoint…",
            Self::Save => "Save",
            Self::SaveAs => "Save to new directory…",
            Self::Import => "Import STEP…",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::Restart => "Restart engine",
            Self::Close => "Close Spiling",
            Self::Cancel => "Cancel native operation",
            Self::Resume => "Resume observation",
            Self::License => "License",
            Self::Gallery => "Component gallery",
        }
    }
    pub fn disabled(self, state: &Snapshot) -> Option<&'static str> {
        if matches!(self, Self::License | Self::Gallery) {
            return None;
        }
        if state.confirmation.is_some() {
            return Some("Resolve the discard confirmation first");
        }
        if matches!(self, Self::Close) {
            return None;
        }
        if state.picker_open {
            return Some("Finish or cancel the native file dialog first");
        }
        if matches!(self, Self::Restart) && state.phase != Phase::Starting {
            return None;
        }
        if state.phase != Phase::Running {
            return Some("Connect the native engine first");
        }
        if matches!(self, Self::Cancel | Self::Resume) {
            return (!state.active_operation).then_some("No native operation is active");
        }
        if state.busy {
            return Some("Wait for the current native operation or cancel it");
        }
        let Some(project) = state.project.as_ref() else {
            return Some("Project information is unavailable");
        };
        if matches!(
            self,
            Self::Save | Self::SaveAs | Self::Import | Self::Undo | Self::Redo
        ) && project.read_only
        {
            return Some("This project is read-only");
        }
        match self {
            Self::Undo if !project.can_undo => Some("No project transaction to undo"),
            Self::Redo if !project.can_redo => Some("No project transaction to redo"),
            _ => None,
        }
    }
}
