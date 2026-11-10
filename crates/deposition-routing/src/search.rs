// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Editable complete-route algorithm with independently checked output.
mod portals;
mod recovery;
use crate::{planner::Session, *};
pub(crate) fn solve(session: &mut Session<'_>) -> Result<Route, Error> {
    recovery::solve(session)
}
