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
