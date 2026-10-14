// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Allocation-free scalar field; native carrier lifting remains shape-specific.
#[derive(Clone, Copy)]
pub(super) struct PeriodicProfile {
    depth: f64,
    frequency: f64,
    phase: f64,
}
impl PeriodicProfile {
    pub(super) fn new(depth: f64, frequency: f64, phase_radians: f64) -> Self {
        Self {
            depth,
            frequency,
            phase: phase_radians,
        }
    }
    pub(super) fn value(self, parameter: f64) -> f64 {
        self.depth * 0.5 * (1.0 - (self.frequency * parameter - self.phase).cos())
    }
    pub(super) fn maximum(self) -> f64 {
        self.depth
    }
    pub(super) fn first_derivative_bound(self) -> f64 {
        self.depth * self.frequency.abs() * 0.5
    }
    pub(super) fn second_derivative_bound(self) -> f64 {
        self.depth * self.frequency * self.frequency * 0.5
    }
}
