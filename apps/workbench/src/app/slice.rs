// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! One Slice request: check the setup, slice and verify, then show Review.
//! Each step is the ordinary command; the run stops at the first refusal or
//! failure and leaves its explanation where that step reports it.

use super::*;
use spiling_contracts::manufacturing::PreflightDisposition;
use spiling_contracts::project::ProjectRevision;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Settings,
    Preflight,
    Compile,
    Verify,
}

#[derive(Debug, Default)]
pub(super) struct SliceRun {
    /// The last step issued and the host serial and revision it was issued at.
    last: Option<(Step, u64, Option<ProjectRevision>)>,
}

impl SliceRun {
    /// A run that first waits for the print settings request just queued.
    pub(super) fn after_settings(serial: u64) -> Self {
        Self {
            last: Some((Step::Settings, serial, None)),
        }
    }
}

impl Workbench {
    pub(super) fn start_slice(&mut self) {
        self.slice = Some(SliceRun::default());
    }

    pub(super) fn slicing(&self) -> bool {
        self.slice.is_some()
    }

    /// The plate's primary action: one Slice that checks, slices and verifies.
    pub(super) fn slice_button(&mut self, ui: &mut egui::Ui) {
        if self.slicing() {
            ui::primary(ui, "Slicing…", Some(""));
            return;
        }
        if ui::primary(ui, "Slice", self.disabled(Action::Preflight)) {
            self.start_slice();
        }
    }

    /// Advances the active Slice run by at most one command per frame.
    pub(super) fn advance_slice(&mut self) {
        let Some(run) = &self.slice else {
            return;
        };
        let idle = self.state.phase == Phase::Running
            && !self.state.busy
            && !self.state.manufacturing.loading
            && self.awaiting_serial.is_none()
            && self.queued.is_none()
            && self.library.queued.is_none();
        if !idle {
            return;
        }
        let last = run.last;
        if let Some((_, serial, _)) = last
            && self.state.serial <= serial
        {
            return;
        }
        let Some(project) = self.state.project.clone() else {
            self.slice = None;
            return;
        };
        let m = self.state.manufacturing.clone();
        let next = if m.intent.is_none() {
            None
        } else if self.manufacturing_current() {
            self.slice = None;
            self.show_panel(Panel::Review);
            return;
        } else if m.artifact.is_some() {
            // Edits retire the artifact, so a present one is the current output.
            Some(Step::Verify)
        } else {
            match m.preflight.as_ref() {
                Some(p) if p.input_revision == project.revision => {
                    if p.disposition == PreflightDisposition::Admitted {
                        Some(Step::Compile)
                    } else {
                        None
                    }
                }
                _ => Some(Step::Preflight),
            }
        };
        // A step that did not move the job forward stops the run: its own
        // report (refusal, problem) is already on screen.
        let repeated = next.is_some_and(|n| {
            last.is_some_and(|(s, _, rev)| s == n && rev == Some(project.revision))
        });
        let action = match next {
            Some(Step::Preflight) => Action::Preflight,
            Some(Step::Compile) => Action::Compile,
            Some(Step::Verify) => Action::Verify,
            Some(Step::Settings) | None => {
                self.slice = None;
                return;
            }
        };
        if repeated || self.disabled(action).is_some() {
            self.slice = None;
            return;
        }
        self.slice = Some(SliceRun {
            last: Some((
                next.expect("step"),
                self.state.serial,
                Some(project.revision),
            )),
        });
        self.queued = Some(action);
    }
}
