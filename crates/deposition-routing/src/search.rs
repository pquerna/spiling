// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! The sole editable algorithm surface in a frozen routing campaign.
use crate::planner::Choice;

pub(crate) fn choose(choices: &[Choice]) -> usize {
    (0..choices.len())
        .min_by(|a, b| {
            choices[*a]
                .travel_mm
                .total_cmp(&choices[*b].travel_mm)
                .then(
                    choices[*a]
                        .following_distance_mm
                        .total_cmp(&choices[*b].following_distance_mm),
                )
                .then(a.cmp(b))
        })
        .unwrap()
}
