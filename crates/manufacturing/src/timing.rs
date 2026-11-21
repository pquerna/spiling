use std::time::Instant;

pub(crate) struct Clock {
    scope: &'static str,
    started: Option<Instant>,
    segment: Option<Instant>,
}

impl Clock {
    pub(crate) fn new(scope: &'static str) -> Self {
        let started = std::env::var_os("SPILING_TRACE_SLICE_PHASES")
            .is_some()
            .then(Instant::now);
        Self {
            scope,
            started,
            segment: started,
        }
    }

    pub(crate) fn mark(&mut self, phase: &'static str) {
        if let Some(segment) = self.segment {
            let now = Instant::now();
            eprintln!(
                "{}",
                serde_json::json!({
                    "event": "slice_phase", "scope": self.scope,
                    "phase": phase, "seconds": now.duration_since(segment).as_secs_f64(),
                })
            );
            self.segment = Some(Instant::now());
        }
    }
}

impl Drop for Clock {
    fn drop(&mut self) {
        self.mark("tail");
        if let Some(started) = self.started {
            eprintln!(
                "{}",
                serde_json::json!({
                    "event": "slice_phase", "scope": self.scope,
                    "phase": "total", "seconds": started.elapsed().as_secs_f64(),
                })
            );
        }
    }
}

/// Accumulated sub-phase seconds inside a loop, reported once on drop as
/// `slice_phase` events under its own scope. Inert unless tracing is enabled.
pub(crate) struct Split {
    scope: &'static str,
    enabled: bool,
    entries: Vec<(&'static str, std::time::Duration, usize)>,
}

impl Split {
    pub(crate) fn new(scope: &'static str) -> Self {
        Self {
            scope,
            enabled: std::env::var_os("SPILING_TRACE_SLICE_PHASES").is_some(),
            entries: Vec::new(),
        }
    }

    pub(crate) fn start(&self) -> Option<Instant> {
        self.enabled.then(Instant::now)
    }

    pub(crate) fn stop(&mut self, phase: &'static str, started: Option<Instant>) {
        let Some(started) = started else {
            return;
        };
        let elapsed = started.elapsed();
        match self.entries.iter_mut().find(|(name, _, _)| *name == phase) {
            Some((_, total, calls)) => {
                *total += elapsed;
                *calls += 1;
            }
            None => self.entries.push((phase, elapsed, 1)),
        }
    }
}

impl Drop for Split {
    fn drop(&mut self) {
        for (phase, total, calls) in &self.entries {
            eprintln!(
                "{}",
                serde_json::json!({
                    "event": "slice_phase", "scope": self.scope,
                    "phase": phase, "seconds": total.as_secs_f64(), "calls": calls,
                })
            );
        }
    }
}
