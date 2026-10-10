// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use serde::Deserialize;
use serde_json::Value;
use spiling_contracts::{
    Request,
    geometry::{GeometryError, PlaneMm, RigidPoseMm},
    manufacturing::{ManufacturingError, ManufacturingErrorCode},
};

pub enum Decoded {
    Request(Request),
    Domain(GeometryError),
    ManufacturingDomain(ManufacturingError),
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPose {
    translation_mm: [f64; 3],
    rotation_xyzw: [f64; 4],
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPlane {
    origin_mm: [f64; 3],
    normal: [f64; 3],
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBounds {
    min: [f64; 3],
    max: [f64; 3],
}

fn normalize_bounds(value: &mut Value) -> Result<Option<GeometryError>, serde_json::Error> {
    let raw: RawBounds = serde_json::from_value(value.clone())?;
    let bounds = spiling_contracts::geometry::AabbMm {
        min: raw.min,
        max: raw.max,
    };
    if let Err(error) = bounds.validate() {
        *value = serde_json::to_value(spiling_contracts::geometry::AabbMm {
            min: [0.0; 3],
            max: [1.0; 3],
        })?;
        Ok(Some(error))
    } else {
        Ok(None)
    }
}
fn normalize_pose(value: &mut Value) -> Result<Option<GeometryError>, serde_json::Error> {
    let raw: RawPose = serde_json::from_value(value.clone())?;
    if let Err(error) = (RigidPoseMm {
        translation_mm: raw.translation_mm,
        rotation_xyzw: raw.rotation_xyzw,
    })
    .validate()
    {
        *value = serde_json::to_value(RigidPoseMm::IDENTITY)?;
        Ok(Some(error))
    } else {
        Ok(None)
    }
}

/// Checked DTOs reject semantic invalidity during deserialization. Validate the same
/// strict envelope with only a structurally valid but invalid pose/plane normalized
/// temporarily, so those errors remain domain failures, never weakened control parsing.
pub fn decode_request(bytes: &[u8]) -> Result<Decoded, serde_json::Error> {
    let mut value: Value = serde_json::from_slice(bytes)?;
    let mut failure = None;
    if value.get("type").and_then(Value::as_str) == Some("geometry")
        && let Some(command) = value.get_mut("command")
    {
        let pose_key = match command.get("op").and_then(Value::as_str) {
            Some("import_part") => Some("initial_pose"),
            Some("add_instance" | "set_instance_pose") => Some("pose"),
            _ => None,
        };
        if let Some(key) = pose_key
            && let Some(pose) = command.get_mut(key)
        {
            let raw: RawPose = serde_json::from_value(pose.clone())?;
            if let Err(error) = (RigidPoseMm {
                translation_mm: raw.translation_mm,
                rotation_xyzw: raw.rotation_xyzw,
            })
            .validate()
            {
                failure = Some(error);
                *pose = serde_json::to_value(RigidPoseMm::IDENTITY)?;
            }
        }
        if command.get("op").and_then(Value::as_str) == Some("start_section")
            && let Some(plane) = command.get_mut("plane")
        {
            let raw: RawPlane = serde_json::from_value(plane.clone())?;
            if let Err(error) = (PlaneMm {
                origin_mm: raw.origin_mm,
                normal: raw.normal,
            })
            .validate()
            {
                failure = Some(error);
                *plane = serde_json::to_value(PlaneMm {
                    origin_mm: [0.0; 3],
                    normal: [0.0, 0.0, 1.0],
                })?;
            }
        }
    }
    let mut manufacturing_failure = None;
    if value.get("type").and_then(Value::as_str) == Some("manufacturing")
        && let Some(command) = value.get_mut("command")
        && command.get("op").and_then(Value::as_str) == Some("set_intent")
        && let Some(printer) = command
            .get_mut("intent")
            .and_then(|intent| intent.get_mut("printer"))
    {
        if let Some(bounds) = printer.get_mut("build_envelope") {
            manufacturing_failure = normalize_bounds(bounds)?;
        }
        if let Some(components) = printer.get_mut("components").and_then(Value::as_array_mut) {
            for component in components {
                if let Some(pose) = component.get_mut("pose") {
                    manufacturing_failure = normalize_pose(pose)?.or(manufacturing_failure);
                }
                if let Some(shape) = component.get_mut("shape")
                    && shape.get("kind").and_then(Value::as_str) == Some("box")
                    && let Some(bounds) = shape.get_mut("bounds_mm")
                {
                    manufacturing_failure = normalize_bounds(bounds)?.or(manufacturing_failure);
                }
            }
        }
    }
    let request = serde_json::from_value(value)?;
    Ok(if let Some(error) = manufacturing_failure {
        Decoded::ManufacturingDomain(ManufacturingError::new(
            ManufacturingErrorCode::InvalidSpecification,
            error.message,
        ))
    } else if let Some(error) = failure {
        Decoded::Domain(error)
    } else {
        Decoded::Request(request)
    })
}
