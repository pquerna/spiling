// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Deposition scheduling: the order of admitted deposition tasks, their entry
//! direction or seam, and the tool sequence. It adds, removes and alters no
//! deposition, and holds no material, command or machine authority.
#![forbid(unsafe_code)]

mod check;
mod cost;
mod seam;
mod search;
pub use spiling_contracts::manufacturing::SeamPolicy;

pub use check::{CheckError, check};
pub use cost::{Cost, gap_seconds};
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

pub type PointMm = [f64; 3];

/// Where a task may be entered and where it then ends.
#[derive(Clone, Copy, Debug)]
pub enum Ends<'a> {
    /// Deposited first-to-last, or last-to-first when `reversible`.
    Open {
        first: PointMm,
        last: PointMm,
        reversible: bool,
    },
    /// A closed path entered and left at one of these permitted vertices.
    Loop { seams: &'a [PointMm] },
}

/// One admitted deposition path. The scheduler sees only what ordering needs.
#[derive(Clone, Copy, Debug)]
pub struct Task<'a> {
    pub ends: Ends<'a>,
    pub tool: u32,
    /// Every task with a lower barrier completes first. The caller decides what
    /// a barrier is (today one physical layer); it is a constraint, not a type.
    pub barrier: u32,
    /// Indexes of tasks that must be deposited earlier.
    pub prerequisites: &'a [u32],
    /// Modelled deposition seconds, used only for the schedule clock.
    pub duration_s: f64,
    /// Authored wall policy. Other loops allow the search to choose their seam.
    pub seam_policy: Option<SeamPolicy>,
    /// Actual occupied height, independent of the caller's barrier index.
    pub physical_z_mm: [f64; 2],
}

/// Declared travel behaviour between deposition paths. Not a calibration.
#[derive(Clone, Copy, Debug)]
pub struct Travel {
    pub speed_mm_s: f64,
    /// Every non-zero travel rises and descends this far (a safe plane or hop).
    pub lift_mm: f64,
    /// Travels at least `minimum_mm` long pay `seconds` once (retract/recover).
    pub retraction: Option<Retraction>,
}
#[derive(Clone, Copy, Debug)]
pub struct Retraction {
    pub minimum_mm: f64,
    pub seconds: f64,
    pub lift_mm: f64,
}

/// When a tool visits its station (waste lane, prime tower, dock).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    /// Every tool is serviced in every barrier, even without tasks.
    EveryBarrier,
    /// Only tools that have tasks in a barrier appear in it.
    WhenUsed,
}

/// Per-machine declared tool-change behaviour. Values are modelled, not measured.
#[derive(Clone, Debug)]
pub struct ToolChange {
    /// Seconds for one tool selection, excluding travel priced separately.
    pub swap_s: f64,
    /// Start heating an incoming tool this long before its selection.
    pub prewarm_lead_s: Option<f64>,
    /// Seconds each station visit takes (for example one waste stroke).
    pub service_s: f64,
    pub service: Service,
    /// Per tool: XY where the tool's group begins after its station visit.
    pub stations_xy: Vec<Option<[f64; 2]>>,
}

#[derive(Clone, Debug)]
pub struct Machine {
    /// Known position before the first task, if any.
    pub start: Option<PointMm>,
    /// Tool selected before the first group; `None` prices the first selection.
    pub initial_tool: Option<u32>,
    pub tools: u32,
    pub travel: Travel,
    pub change: ToolChange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolOrder {
    /// Ascending tool index in every barrier.
    Fixed,
    /// The scheduler picks each barrier's tool order to reduce selections.
    Free,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Visit {
    pub task: u32,
    /// Open tasks only.
    pub reverse: bool,
    /// Loop tasks only: index into `Ends::Loop::seams`.
    pub seam: u32,
}

/// One tool's contiguous work inside one barrier. May hold no visits.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub barrier: u32,
    pub tool: u32,
    pub visits: Range<usize>,
    /// A tool selection precedes this group.
    pub selected: bool,
    /// Schedule-clock time to start heating this tool, when a lead is declared.
    pub prewarm_at_s: Option<f64>,
    /// Schedule-clock time this group begins (after any selection).
    pub begins_at_s: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Schedule {
    pub visits: Vec<Visit>,
    pub groups: Vec<Group>,
    pub cost: Cost,
    /// Charged optional-improvement work.
    pub work: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidInput(&'static str),
    /// Prerequisites, barriers and tool order cannot all hold.
    Inconsistent,
    Cancelled,
    /// The search produced a schedule the frozen checker refused: a defect.
    Rejected(CheckError),
}

pub(crate) fn entry_exit(task: &Task<'_>, visit: Visit) -> (PointMm, PointMm) {
    match task.ends {
        Ends::Open { first, last, .. } => {
            if visit.reverse {
                (last, first)
            } else {
                (first, last)
            }
        }
        Ends::Loop { seams } => (seams[visit.seam as usize], seams[visit.seam as usize]),
    }
}

fn finite(p: &PointMm) -> bool {
    p.iter().all(|v| v.is_finite())
}

fn validate(tasks: &[Task<'_>], machine: &Machine) -> Result<(), Error> {
    let t = &machine.travel;
    if u32::try_from(tasks.len()).is_err() {
        return Err(Error::InvalidInput("too many tasks"));
    }
    if !(t.speed_mm_s.is_finite() && t.speed_mm_s > 0.)
        || !(t.lift_mm.is_finite() && t.lift_mm >= 0.)
        || t.retraction.is_some_and(|r| {
            !(r.minimum_mm.is_finite() && r.minimum_mm >= 0.)
                || !(r.seconds.is_finite() && r.seconds >= 0.)
                || !(r.lift_mm.is_finite() && r.lift_mm >= 0.)
        })
    {
        return Err(Error::InvalidInput("travel model"));
    }
    let c = &machine.change;
    if machine.tools == 0
        || c.stations_xy.len() != machine.tools as usize
        || !(c.swap_s.is_finite() && c.swap_s >= 0.)
        || !(c.service_s.is_finite() && c.service_s >= 0.)
        || c.prewarm_lead_s
            .is_some_and(|s| !(s.is_finite() && s >= 0.))
        || c.stations_xy
            .iter()
            .flatten()
            .flatten()
            .any(|v| !v.is_finite())
        || machine.start.is_some_and(|p| !finite(&p))
        || machine
            .initial_tool
            .is_some_and(|tool| tool >= machine.tools)
    {
        return Err(Error::InvalidInput("machine model"));
    }
    for (i, task) in tasks.iter().enumerate() {
        let ends_valid = match task.ends {
            Ends::Open { first, last, .. } => finite(&first) && finite(&last),
            Ends::Loop { seams } => {
                !seams.is_empty() && u32::try_from(seams.len()).is_ok() && seams.iter().all(finite)
            }
        };
        let [lower, upper] = task.physical_z_mm;
        let planar = match task.ends {
            Ends::Open { first, last, .. } => first[2] == upper && last[2] == upper,
            Ends::Loop { seams } => seams.iter().all(|p| p[2] == upper),
        };
        let policy_valid = match task.seam_policy {
            Some(SeamPolicy::PreferredBuildDirection { xy }) => {
                xy.iter().all(|v| v.is_finite()) && xy != [0., 0.]
            }
            _ => true,
        };
        if !policy_valid
            || !planar
            || !lower.is_finite()
            || !upper.is_finite()
            || lower >= upper
            || task.seam_policy.is_some() && !matches!(task.ends, Ends::Loop { .. })
            || !ends_valid
            || task.tool >= machine.tools
            || !(task.duration_s.is_finite() && task.duration_s >= 0.)
            || task
                .prerequisites
                .iter()
                .any(|q| *q as usize >= tasks.len() || *q as usize == i)
        {
            return Err(Error::InvalidInput("task"));
        }
    }
    Ok(())
}

/// Schedule every task exactly once. The result has passed the frozen checker.
pub fn schedule(
    tasks: &[Task<'_>],
    machine: &Machine,
    order: ToolOrder,
    cancel: &AtomicBool,
) -> Result<Schedule, Error> {
    validate(tasks, machine)?;
    let mut context = search::Context {
        tasks,
        machine,
        order,
        cancel,
        work: 0,
    };
    let (visits, groups) = search::solve(&mut context)?;
    let work = context.work;
    finish(tasks, machine, order, visits, groups, work, cancel)
}

/// The caller's own order, grouped and priced: the comparison control.
pub fn schedule_input_order(
    tasks: &[Task<'_>],
    machine: &Machine,
    cancel: &AtomicBool,
) -> Result<Schedule, Error> {
    validate(tasks, machine)?;
    let mut context = search::Context {
        tasks,
        machine,
        order: ToolOrder::Fixed,
        cancel,
        work: 0,
    };
    let (visits, groups) = search::input_order(&mut context)?;
    finish(tasks, machine, ToolOrder::Fixed, visits, groups, 0, cancel)
}

fn finish(
    tasks: &[Task<'_>],
    machine: &Machine,
    order: ToolOrder,
    mut visits: Vec<Visit>,
    groups: Vec<(u32, u32, Range<usize>)>,
    work: u64,
    cancel: &AtomicBool,
) -> Result<Schedule, Error> {
    // Check raw search references before indexing borrowed input or expanding
    // seams. Full inventory/precedence/annotation admission still follows.
    for visit in &visits {
        let Some(task) = tasks.get(visit.task as usize) else {
            return Err(Error::Rejected(CheckError::Inventory));
        };
        let allowed = match task.ends {
            Ends::Open { reversible, .. } => visit.seam == 0 && (!visit.reverse || reversible),
            Ends::Loop { seams } => !visit.reverse && (visit.seam as usize) < seams.len(),
        };
        if !allowed {
            return Err(Error::Rejected(CheckError::Permission));
        }
    }
    if groups.iter().any(|(_, tool, range)| {
        *tool >= machine.tools || range.start > range.end || range.end > visits.len()
    }) {
        return Err(Error::Rejected(CheckError::Grouping));
    }
    cancelled(cancel)?;
    seam::apply(tasks, machine, &mut visits, &groups, Some(cancel))?;
    let mut schedule = cost::annotate(tasks, machine, visits, groups);
    schedule.work = work;
    let checked = check(tasks, machine, order, &schedule);
    cancelled(cancel)?;
    match checked {
        Ok(_) => Ok(schedule),
        Err(CheckError::Prerequisite) => Err(Error::Inconsistent),
        Err(other) => Err(Error::Rejected(other)),
    }
}

pub(crate) fn cancelled(cancel: &AtomicBool) -> Result<(), Error> {
    if cancel.load(Ordering::Relaxed) {
        Err(Error::Cancelled)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
