// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::geometry::JobId;
use parking_lot::{Condvar, Mutex};
use spiling_contracts::geometry::{JOB_CANCEL_AFTER_MS, JOB_CANCEL_GRACE_MS};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

struct Active {
    id: JobId,
    accepted: Instant,
    cancel_at: Option<Instant>,
    committing: bool,
    cancel: Arc<AtomicBool>,
}
struct State {
    active: Option<Active>,
    stopped: bool,
}
struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

/// Captures the job whose durable commit must serialize against cancellation.
#[derive(Clone)]
pub struct CommitToken {
    shared: Arc<Shared>,
    id: JobId,
}
impl CommitToken {
    pub fn begin_commit(&self) -> bool {
        let mut state = self.shared.state.lock();
        let active = state.active.as_mut().expect("active job");
        assert_eq!(active.id, self.id);
        if active.cancel.load(Ordering::Acquire) {
            return false;
        }
        active.committing = true;
        true
    }
}

/// One deadline owner, independent of stdin and the non-preemptible kernel thread.
pub struct Watchdog {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Watchdog {
    pub fn new() -> Self {
        Self::with_deadlines(
            Duration::from_millis(JOB_CANCEL_AFTER_MS.into()),
            Duration::from_millis(JOB_CANCEL_GRACE_MS.into()),
        )
    }
    fn with_deadlines(cancel_after: Duration, grace: Duration) -> Self {
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                active: None,
                stopped: false,
            }),
            changed: Condvar::new(),
        });
        let owner = shared.clone();
        let thread = std::thread::spawn(move || {
            let mut state = owner.state.lock();
            loop {
                if state.stopped {
                    break;
                }
                let deadline = if let Some(active) = &mut state.active {
                    let now = Instant::now();
                    let automatic = active.accepted + cancel_after;
                    if active.cancel_at.is_none() && now >= automatic {
                        active.cancel_at = Some(now);
                        if !active.committing {
                            active.cancel.store(true, Ordering::Release);
                        }
                    }
                    if let Some(cancel_at) = active.cancel_at {
                        let deadline = cancel_at + grace;
                        if now >= deadline {
                            crate::diagnostic(
                                "error",
                                "job_cancel_deadline",
                                "native worker did not acknowledge cancellation/promotion within grace",
                            );
                            std::process::exit(70);
                        }
                        Some(deadline)
                    } else {
                        Some(automatic)
                    }
                } else {
                    None
                };
                if let Some(deadline) = deadline {
                    owner.changed.wait_for(
                        &mut state,
                        deadline.saturating_duration_since(Instant::now()),
                    );
                } else {
                    owner.changed.wait(&mut state);
                }
            }
        });
        Self {
            shared,
            thread: Some(thread),
        }
    }
    pub fn start(&self, id: JobId, cancel: Arc<AtomicBool>) {
        let mut state = self.shared.state.lock();
        assert!(state.active.is_none());
        state.active = Some(Active {
            id,
            accepted: Instant::now(),
            cancel_at: None,
            committing: false,
            cancel,
        });
        self.shared.changed.notify_all();
    }
    pub fn cancel(&self, id: JobId) -> bool {
        let mut state = self.shared.state.lock();
        if let Some(active) = &mut state.active
            && active.id == id
        {
            active.cancel_at.get_or_insert_with(Instant::now);
            if !active.committing {
                active.cancel.store(true, Ordering::Release);
            }
            self.shared.changed.notify_all();
            return active.committing;
        }
        false
    }
    pub fn commit_token(&self, id: JobId) -> CommitToken {
        CommitToken {
            shared: self.shared.clone(),
            id,
        }
    }
    /// This lock is the cancellation linearization point. Promotion may not roll back afterward.
    pub fn begin_commit(&self, id: JobId) -> bool {
        self.commit_token(id).begin_commit()
    }
    pub fn finish(&self, id: JobId) {
        let mut state = self.shared.state.lock();
        assert_eq!(state.active.as_ref().map(|a| a.id), Some(id));
        state.active = None;
        self.shared.changed.notify_all();
    }
}
impl Drop for Watchdog {
    fn drop(&mut self) {
        self.shared.state.lock().stopped = true;
        self.shared.changed.notify_all();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_linearizes_before_but_not_after_commit() {
        let owner = Watchdog::new();
        let id = JobId::new(1).unwrap();
        let flag = Arc::new(AtomicBool::new(false));
        owner.start(id, flag.clone());
        let token = owner.commit_token(id);
        assert!(!owner.cancel(id));
        assert!(flag.load(Ordering::Acquire));
        assert!(!owner.begin_commit(id));
        assert!(!token.clone().begin_commit());
        owner.finish(id);
        let id = JobId::new(2).unwrap();
        let flag = Arc::new(AtomicBool::new(false));
        owner.start(id, flag.clone());
        let token = owner.commit_token(id);
        assert!(token.clone().begin_commit());
        assert!(owner.begin_commit(id));
        assert!(owner.cancel(id));
        assert!(!flag.load(Ordering::Acquire));
        owner.finish(id);
    }

    // Isolated subprocess exercises the real terminating thread, not a mocked
    // successful native cancellation. Short timings exist only in these tests.
    #[test]
    fn deadline_child() {
        let Ok(mode) = std::env::var("SPILING_WATCHDOG_TEST_CHILD") else {
            return;
        };
        let owner = Watchdog::with_deadlines(Duration::from_millis(50), Duration::from_millis(50));
        let id = JobId::new(1).unwrap();
        owner.start(id, Arc::new(AtomicBool::new(false)));
        if mode == "committing" {
            assert!(owner.begin_commit(id));
            owner.cancel(id);
        }
        std::thread::sleep(Duration::from_secs(2));
        panic!("watchdog failed to terminate a missing ACK");
    }

    #[test]
    fn idle_and_unacknowledged_promotion_deadlines_terminate_process() {
        for mode in ["idle", "committing"] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "session::tests::deadline_child", "--nocapture"])
                .env("SPILING_WATCHDOG_TEST_CHILD", mode)
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(70));
            assert!(
                std::str::from_utf8(&output.stderr)
                    .unwrap()
                    .contains("job_cancel_deadline")
            );
        }
    }
}
