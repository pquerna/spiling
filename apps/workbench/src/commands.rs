// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::host::{Phase, Snapshot};

#[cfg(test)]
mod tests {
    #[test]
    fn local_navigation_survives_startup_and_busy_engine_but_respects_native_picker() {
        let mut state = crate::host::Snapshot::default();
        for action in [
            super::Action::PrepareWorkspace,
            super::Action::ReviewWorkspace,
            super::Action::AnalyzeWorkspace,
            super::Action::Surface,
        ] {
            assert!(action.disabled(&state).is_none());
            state.busy = true;
            assert!(action.disabled(&state).is_none());
            state.picker_open = true;
            assert!(action.disabled(&state).is_some());
            state.picker_open = false;
        }
        assert!(super::Action::Compile.disabled(&state).is_some());
    }
    #[test]
    fn save_as_names_the_project_file_not_a_directory() {
        assert_eq!(super::Action::SaveAs.label(), "Save As .spil…");
    }
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Workspace {
    #[default]
    Prepare,
    Review,
    Analyze,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    View(crate::viewport::ViewAction),
    PrepareWorkspace,
    ReviewWorkspace,
    AnalyzeWorkspace,
    Properties,
    Printer,
    Material,
    Settings,
    Placement,
    Infill,
    Surface,
    Study,
    ReviewLayers,
    Compare,
    Analysis,
    FocusSelection,
    RevealSelection,
    Arrange,
    Library,
    New,
    Open,
    OpenReadOnly,
    Recover,
    Save,
    SaveAs,
    Import,
    LoadIntent,
    LoadMaterial,
    LoadMaterialCatalog,
    LoadAnalysisModel,
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
        Self::View(crate::viewport::ViewAction::Snap(
            crate::viewport::StandardView::Isometric,
        )),
        Self::View(crate::viewport::ViewAction::Snap(
            crate::viewport::StandardView::Top,
        )),
        Self::View(crate::viewport::ViewAction::Snap(
            crate::viewport::StandardView::Bottom,
        )),
        Self::View(crate::viewport::ViewAction::Snap(
            crate::viewport::StandardView::Front,
        )),
        Self::View(crate::viewport::ViewAction::Snap(
            crate::viewport::StandardView::Back,
        )),
        Self::View(crate::viewport::ViewAction::Snap(
            crate::viewport::StandardView::Left,
        )),
        Self::View(crate::viewport::ViewAction::Snap(
            crate::viewport::StandardView::Right,
        )),
        Self::View(crate::viewport::ViewAction::Fit),
        Self::View(crate::viewport::ViewAction::FitParts),
        Self::View(crate::viewport::ViewAction::FitPlate),
        Self::View(crate::viewport::ViewAction::Projection),
        Self::View(crate::viewport::ViewAction::Normal),
        Self::View(crate::viewport::ViewAction::OppositeNormal),
        Self::View(crate::viewport::ViewAction::RollLeft),
        Self::View(crate::viewport::ViewAction::RollRight),
        Self::View(crate::viewport::ViewAction::Previous),
        Self::View(crate::viewport::ViewAction::DIAGONALS[0]),
        Self::View(crate::viewport::ViewAction::DIAGONALS[1]),
        Self::View(crate::viewport::ViewAction::DIAGONALS[2]),
        Self::View(crate::viewport::ViewAction::DIAGONALS[3]),
        Self::View(crate::viewport::ViewAction::DIAGONALS[4]),
        Self::View(crate::viewport::ViewAction::DIAGONALS[5]),
        Self::View(crate::viewport::ViewAction::DIAGONALS[6]),
        Self::View(crate::viewport::ViewAction::DIAGONALS[7]),
        Self::View(crate::viewport::ViewAction::DIAGONALS[8]),
        Self::View(crate::viewport::ViewAction::DIAGONALS[9]),
        Self::View(crate::viewport::ViewAction::DIAGONALS[10]),
        Self::View(crate::viewport::ViewAction::DIAGONALS[11]),
        Self::View(crate::viewport::ViewAction::DIAGONALS[12]),
        Self::View(crate::viewport::ViewAction::DIAGONALS[13]),
        Self::View(crate::viewport::ViewAction::DIAGONALS[14]),
        Self::View(crate::viewport::ViewAction::DIAGONALS[15]),
        Self::View(crate::viewport::ViewAction::DIAGONALS[16]),
        Self::View(crate::viewport::ViewAction::DIAGONALS[17]),
        Self::View(crate::viewport::ViewAction::DIAGONALS[18]),
        Self::View(crate::viewport::ViewAction::DIAGONALS[19]),
        Self::PrepareWorkspace,
        Self::ReviewWorkspace,
        Self::AnalyzeWorkspace,
        Self::Properties,
        Self::Printer,
        Self::Material,
        Self::Settings,
        Self::Placement,
        Self::Infill,
        Self::Surface,
        Self::Study,
        Self::ReviewLayers,
        Self::Compare,
        Self::Analysis,
        Self::FocusSelection,
        Self::RevealSelection,
        Self::Arrange,
        Self::Library,
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
        Self::LoadMaterialCatalog,
        Self::LoadAnalysisModel,
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
            Self::View(action) => action.label(),
            Self::PrepareWorkspace => "Prepare",
            Self::ReviewWorkspace => "Review",
            Self::AnalyzeWorkspace => "Analyze",
            Self::Properties => "Properties",
            Self::Printer => "Printer",
            Self::Material => "Material",
            Self::Settings => "Settings",
            Self::Placement => "Position",
            Self::Infill => "Infill",
            Self::Surface => "Surface finish",
            Self::Study => "Color study",
            Self::ReviewLayers => "Layers",
            Self::Compare => "Compare settings",
            Self::Analysis => "Section analysis",
            Self::FocusSelection => "Focus on selection",
            Self::RevealSelection => "Reveal selection",
            Self::Arrange => "Arrange…",
            Self::Library => "Library",
            Self::New => "New project",
            Self::Open => "Open project…",
            Self::OpenReadOnly => "Open read-only…",
            Self::Recover => "Recover previous checkpoint…",
            Self::Save => "Save",
            Self::SaveAs => "Save As .spil…",
            Self::Import => "Import STEP…",
            Self::LoadIntent => "Load intent JSON…",
            Self::LoadMaterial => "Load material profile…",
            Self::LoadMaterialCatalog => "Browse material catalog…",
            Self::LoadAnalysisModel => "Choose analysis model…",
            Self::Preflight => "Check setup",
            Self::Compile => "Slice plate",
            Self::Verify => "Verify program",
            Self::Export => "Export plate…",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::Restart => "Restart engine",
            Self::Close => "Close Spiling",
            Self::Cancel => "Cancel operation",
            Self::Resume => "Resume observation",
            Self::License => "License",
            Self::Gallery => "Component gallery",
        }
    }
    pub fn navigation(self) -> bool {
        matches!(
            self,
            Self::PrepareWorkspace
                | Self::View(_)
                | Self::ReviewWorkspace
                | Self::AnalyzeWorkspace
                | Self::Properties
                | Self::Printer
                | Self::Material
                | Self::Settings
                | Self::Placement
                | Self::Infill
                | Self::Surface
                | Self::Study
                | Self::ReviewLayers
        )
    }
    pub fn disabled(self, state: &Snapshot) -> Option<&'static str> {
        if matches!(self, Self::License | Self::Gallery | Self::Analysis) {
            return None;
        }
        if state.confirmation.is_some() {
            return Some("Resolve the discard confirmation first");
        }
        if self.navigation() {
            return state
                .picker_open
                .then_some("Finish or cancel the native file dialog first");
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
            Self::Compare if m.artifact.is_none() => {
                Some("Slice and freshly verify the current plate first")
            }
            Self::Arrange if state.scene.as_ref().is_none_or(|s| s.occurrence_count == 0) => {
                Some("Import a part first")
            }
            Self::LoadMaterial | Self::LoadMaterialCatalog
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
