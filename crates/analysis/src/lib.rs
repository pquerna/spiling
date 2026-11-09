// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Print-aware numerical model consumers; native source authority stays in geometry.
mod element;
mod mesh;
pub mod native;
pub mod run;
mod solve;
#[cfg(test)]
mod tests;
pub use solve::{PreparedOperator, Solution, prepare, solve};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub struct Budget<'a> {
    cancel: &'a AtomicBool,
    start: Instant,
    duration: Duration,
}
impl<'a> Budget<'a> {
    pub fn new(cancel: &'a AtomicBool, seconds: u32) -> Self {
        Self {
            cancel,
            start: Instant::now(),
            duration: Duration::from_secs(seconds.into()),
        }
    }
    pub fn check(&self) -> Result<(), String> {
        if self.cancel.load(Ordering::Acquire) {
            Err("Cancelled: mechanical capture/solve was cancelled".into())
        } else if self.start.elapsed() > self.duration {
            Err("ResourceLimit: mechanical case wall deadline".into())
        } else {
            Ok(())
        }
    }
}
