// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Frozen admission of a schedule. Search output is never trusted.
use crate::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CheckError {
    Input,
    Inventory,
    Permission,
    Prerequisite,
    Barrier,
    Grouping,
    ToolOrder,
    Annotation,
}

/// Every task once, permitted entries, prerequisites and barriers in order,
/// contiguous tool groups in the declared service pattern, and a cost and
/// clock equal to a fresh evaluation.
pub fn check(
    tasks: &[Task<'_>],
    machine: &Machine,
    order: ToolOrder,
    schedule: &Schedule,
) -> Result<Cost, CheckError> {
    validate(tasks, machine).map_err(|_| CheckError::Input)?;
    structure(tasks, machine, order, schedule)?;
    let groups: Vec<_> = schedule
        .groups
        .iter()
        .map(|g| (g.barrier, g.tool, g.visits.clone()))
        .collect();
    let mut authored = schedule.visits.clone();
    crate::seam::apply(tasks, machine, &mut authored, &groups, None)
        .map_err(|_| CheckError::Permission)?;
    if authored != schedule.visits {
        return Err(CheckError::Permission);
    }
    let fresh = cost::annotate(tasks, machine, schedule.visits.clone(), groups);
    if fresh.groups != schedule.groups || fresh.cost != schedule.cost {
        return Err(CheckError::Annotation);
    }
    finite_cost(&fresh.cost)?;
    Ok(fresh.cost)
}

fn finite_cost(cost: &Cost) -> Result<(), CheckError> {
    if cost.total_s.is_finite() && cost.travel_mm.is_finite() {
        Ok(())
    } else {
        Err(CheckError::Annotation)
    }
}

/// The scheduler's own admission. Its input was validated, its wall seams were
/// authored and its annotation computed by the same frozen functions [`check`]
/// would re-run, so only the search's order and grouping remain to admit.
pub(crate) fn admit(
    tasks: &[Task<'_>],
    machine: &Machine,
    order: ToolOrder,
    schedule: &Schedule,
) -> Result<Cost, CheckError> {
    structure(tasks, machine, order, schedule)?;
    finite_cost(&schedule.cost)?;
    Ok(schedule.cost)
}

/// Inventory, permitted entries, prerequisites, barriers and grouping.
fn structure(
    tasks: &[Task<'_>],
    machine: &Machine,
    order: ToolOrder,
    schedule: &Schedule,
) -> Result<(), CheckError> {
    if schedule.visits.len() != tasks.len() {
        return Err(CheckError::Inventory);
    }
    let mut position = vec![usize::MAX; tasks.len()];
    for (at, visit) in schedule.visits.iter().enumerate() {
        let task = tasks
            .get(visit.task as usize)
            .ok_or(CheckError::Inventory)?;
        if position[visit.task as usize] != usize::MAX {
            return Err(CheckError::Inventory);
        }
        position[visit.task as usize] = at;
        let permitted = match task.ends {
            Ends::Open { reversible, .. } => visit.seam == 0 && (reversible || !visit.reverse),
            Ends::Loop { seams } => !visit.reverse && (visit.seam as usize) < seams.len(),
        };
        if !permitted {
            return Err(CheckError::Permission);
        }
    }
    for (index, task) in tasks.iter().enumerate() {
        if task
            .prerequisites
            .iter()
            .any(|q| position[*q as usize] >= position[index])
        {
            return Err(CheckError::Prerequisite);
        }
    }
    let mut next = 0;
    let mut previous: Option<&Group> = None;
    let mut seen = vec![false; machine.tools as usize];
    for group in &schedule.groups {
        if group.visits.start != next
            || group.visits.end < next
            || group.visits.end > tasks.len()
            || group.tool >= machine.tools
        {
            return Err(CheckError::Grouping);
        }
        next = group.visits.end;
        for visit in &schedule.visits[group.visits.clone()] {
            let task = &tasks[visit.task as usize];
            if task.tool != group.tool || task.barrier != group.barrier {
                return Err(CheckError::Grouping);
            }
        }
        match previous {
            Some(p) if p.barrier > group.barrier => return Err(CheckError::Barrier),
            Some(p) if p.barrier == group.barrier => {
                if order == ToolOrder::Fixed && p.tool >= group.tool {
                    return Err(CheckError::ToolOrder);
                }
            }
            _ => seen.fill(false),
        }
        // One group per tool per barrier.
        if std::mem::replace(&mut seen[group.tool as usize], true) {
            return Err(CheckError::Grouping);
        }
        if group.visits.is_empty() && machine.change.service == Service::WhenUsed {
            return Err(CheckError::Grouping);
        }
        previous = Some(group);
    }
    if next != tasks.len() {
        return Err(CheckError::Grouping);
    }
    if machine.change.service == Service::EveryBarrier {
        // Every tool is serviced in every barrier that has any work.
        let mut barriers: Vec<u32> = tasks.iter().map(|t| t.barrier).collect();
        barriers.sort_unstable();
        barriers.dedup();
        if schedule.groups.len() != barriers.len() * machine.tools as usize
            || schedule
                .groups
                .chunks(machine.tools as usize)
                .zip(&barriers)
                .any(|(chunk, barrier)| chunk.iter().any(|g| g.barrier != *barrier))
        {
            return Err(CheckError::Grouping);
        }
    }
    Ok(())
}
