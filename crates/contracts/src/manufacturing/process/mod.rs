// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::{ManufacturingError, PrinterSpecification, invalid, positive};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProcessPolicy {
    pub first_layer_speed_factor: f64,
    pub first_layer_flow_factor: f64,
    pub fan_first_layers: u8,
    pub fan_after_first_layers: u8,
    pub bridge_speed_factor: f64,
    pub bridge_flow_factor: f64,
    pub bridge_fan_pwm: u8,
    pub max_bridge_span_mm: f64,
    pub retraction: RetractionPolicy,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RetractionPolicy {
    Disabled,
    Enabled {
        distance_mm: f64,
        speed_mm_s: f64,
        minimum_travel_mm: f64,
        z_hop_mm: f64,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SupportPolicy {
    Disabled,
    Breakaway {
        density: f64,
        xy_clearance_mm: f64,
        z_gap_mm: f64,
        interface_thickness_mm: f64,
    },
}
impl ProcessPolicy {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        positive(&[
            self.first_layer_speed_factor,
            self.first_layer_flow_factor,
            self.bridge_speed_factor,
            self.bridge_flow_factor,
            self.max_bridge_span_mm,
        ])?;
        if self.first_layer_speed_factor > 1.0 || self.bridge_speed_factor > 1.0 {
            return Err(invalid("process speed factors must be in (0,1]"));
        }
        self.retraction.validate()?;
        Ok(())
    }
    pub fn validate_printer(
        &self,
        printer: &PrinterSpecification,
    ) -> Result<(), ManufacturingError> {
        self.validate()?;
        if let RetractionPolicy::Enabled {
            speed_mm_s,
            z_hop_mm,
            ..
        } = self.retraction
            && (speed_mm_s > printer.max_feed_mm_s
                || !(speed_mm_s * 60.0).is_finite()
                || (speed_mm_s * 60_000_000.0).round() / 1_000_000.0 / 60.0 > printer.max_feed_mm_s
                || z_hop_mm > printer.build_envelope.max[2] - printer.build_envelope.min[2])
        {
            return Err(invalid("retraction exceeds printer feed or Z envelope"));
        }
        Ok(())
    }
}
impl RetractionPolicy {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        if let Self::Enabled {
            distance_mm,
            speed_mm_s,
            minimum_travel_mm,
            z_hop_mm,
        } = *self
        {
            positive(&[distance_mm, speed_mm_s, minimum_travel_mm])?;
            if !z_hop_mm.is_finite()
                || z_hop_mm < 0.0
                || distance_mm < 0.00000001
                || !(speed_mm_s * 60.0).is_finite()
                || speed_mm_s * 60.0 < 0.000001
                || minimum_travel_mm < 0.000001
                || (z_hop_mm > 0.0 && z_hop_mm < 0.000001)
            {
                return Err(invalid(
                    "retraction policy exceeds serialized precision or numeric range",
                ));
            }
        }
        Ok(())
    }
}
impl SupportPolicy {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        if let Self::Breakaway {
            density,
            xy_clearance_mm,
            z_gap_mm,
            interface_thickness_mm,
        } = *self
        {
            positive(&[density, xy_clearance_mm, interface_thickness_mm])?;
            if density > 1.0 || !z_gap_mm.is_finite() || z_gap_mm < 0.0 {
                return Err(invalid("invalid breakaway support density or Z gap"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PreflightSeverity {
    Error,
    Warning,
    Info,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PreflightCheck {
    pub code: String,
    pub severity: PreflightSeverity,
    pub message: String,
    pub occurrence_id: Option<crate::geometry::OccurrenceId>,
    pub face_id: Option<crate::geometry::FaceId>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PreflightReport {
    pub input_revision: crate::project::ProjectRevision,
    pub eligible: bool,
    pub software_only: bool,
    #[serde(deserialize_with = "checks")]
    pub checks: Vec<PreflightCheck>,
}
fn checks<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<PreflightCheck>, D::Error> {
    super::bounded_vec::<D, _, 256>(d)
}
impl PreflightReport {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        if self.checks.len() > 256 {
            return Err(super::resource("preflight check budget exceeded"));
        }
        if !self.software_only
            || self.checks.is_empty()
            || self.eligible
                == self
                    .checks
                    .iter()
                    .any(|c| c.severity == PreflightSeverity::Error)
        {
            return Err(invalid(
                "invalid preflight eligibility or software qualification",
            ));
        }
        for check in &self.checks {
            super::label(&check.code)?;
            if check.message.is_empty()
                || check.message.len() > super::MAX_MANUFACTURING_ERROR_BYTES
                || check.message.chars().any(char::is_control)
            {
                return Err(invalid("invalid preflight message"));
            }
        }
        Ok(())
    }
}
