// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Editable complete-route algorithm with independently checked output.
mod lazy;
mod oropt;
mod portals;
mod recovery;
use crate::{planner::Session, *};
pub(crate) fn solve(session: &mut Session<'_>) -> Result<Route, Error> {
    let incumbent = match lazy::solve(session)? {
        Some(route) => route,
        None => recovery::solve(session)?,
    };
    oropt::improve(session, incumbent)
}
