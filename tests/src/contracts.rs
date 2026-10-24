// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Contracts for Spiling RFC 07 per PROJECT.md § Interface Contracts.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StepEntityId(pub u64);

impl std::fmt::Display for StepEntityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "step:{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceFaceId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceEdgeId(pub u32);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SectionCurve2d {
    Line {
        start_mm: [f64; 2],
        end_mm: [f64; 2],
    },
    CircularArc {
        center_mm: [f64; 2],
        radius_mm: f64,
        start_angle_rad: f64,
        sweep_angle_rad: f64,
    },
    EllipticArc {
        center_mm: [f64; 2],
        major_axis_mm: [f64; 2],
        minor_radius_mm: f64,
        start_param_rad: f64,
        end_param_rad: f64,
    },
    NurbsSpan {
        degree: u8,
        knots: Vec<f64>,
        control_points_mm: Vec<[f64; 2]>,
        weights: Option<Vec<f64>>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SectionSpan2d {
    pub curve: SectionCurve2d,
    pub source_face: SourceFaceId,
    pub source_edge: Option<SourceEdgeId>,
    pub achieved_plane_error_mm: f64,
}
