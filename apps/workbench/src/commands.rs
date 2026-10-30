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
    LoadIntent,
    LoadMaterial,
    Preflight,
    Compile,
    Verify,
    Export,
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
    pub const ALL: &[Self] = &[
        Self::New,
        Self::Open,
        Self::OpenReadOnly,
        Self::Recover,
        Self::Save,
        Self::SaveAs,
        Self::Import,
        Self::Undo,
        Self::Redo,
        Self::LoadIntent,
        Self::LoadMaterial,
        Self::Preflight,
        Self::Compile,
        Self::Verify,
        Self::Export,
        Self::Cancel,
        Self::Resume,
        Self::Restart,
        Self::License,
        Self::Gallery,
        Self::Close,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::New => "New project",
            Self::Open => "Open project…",
            Self::OpenReadOnly => "Open read-only…",
            Self::Recover => "Recover previous checkpoint…",
            Self::Save => "Save",
            Self::SaveAs => "Save to new directory…",
            Self::Import => "Import STEP…",
            Self::LoadIntent => "Load intent JSON…",
            Self::LoadMaterial => "Load material profile…",
            Self::Preflight => "Preflight current inputs",
            Self::Compile => "Compile current inputs",
            Self::Verify => "Fresh Verify and load motion",
            Self::Export => "Export verified output…",
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
            Self::Save
                | Self::SaveAs
                | Self::Import
                | Self::LoadMaterial
                | Self::Undo
                | Self::Redo
                | Self::Compile
        ) && project.read_only
        {
            return Some("This project is read-only");
        }
        let m = &state.manufacturing;
        match self {
            Self::LoadMaterial
                if !m.intent.as_ref().is_some_and(|i| {
                    matches!(
                        i.printer.workflow,
                        spiling_contracts::manufacturing::JobWorkflow::CompleteMarlin { .. }
                    )
                }) =>
            {
                Some("Apply a complete single-tool Marlin intent first")
            }
            Self::Preflight if m.intent.is_none() => {
                Some("Apply a complete manufacturing intent first")
            }
            Self::Compile
                if !m.preflight.as_ref().is_some_and(|p| {
                    p.input_revision == project.revision
                        && p.disposition
                            == spiling_contracts::manufacturing::PreflightDisposition::Admitted
                }) =>
            {
                Some("Preflight must admit the current revision first")
            }
            Self::Verify if m.artifact.is_none() => Some("Compile a manufacturing artifact first"),
            Self::Export
                if m.export_busy
                    || !state.hello.as_ref().is_some_and(|h| {
                        m.current_capture(project, &h.session_id)
                            .is_some_and(|r| r.report.verified)
                    }) =>
            {
                Some("Fresh passing verification is required for the current output")
            }
            Self::Undo if !project.can_undo => Some("No project transaction to undo"),
            Self::Redo if !project.can_redo => Some("No project transaction to redo"),
            _ => None,
        }
    }
}
