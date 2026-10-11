// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Software-only replay of actual program bytes, independent of the backend.

use std::sync::atomic::{AtomicBool, Ordering};

use spiling_contracts::manufacturing::{
    MAX_MANUFACTURING_BUNDLE_BYTES, ManufacturingError, ManufacturingErrorCode,
    ManufacturingIntent, NormalizedPrintPlan, VerificationReport,
};

const XYZ_HALF_STEP: f64 = 0.000_000_5;
const E_HALF_STEP: f64 = 0.000_000_005;
const F_HALF_STEP: f64 = 0.000_000_5;
const MAX_LINE_BYTES: usize = 512;

fn failed(message: impl AsRef<str>) -> ManufacturingError {
    ManufacturingError::new(ManufacturingErrorCode::VerificationFailed, message)
}

fn cancelled(cancel: &AtomicBool) -> Result<(), ManufacturingError> {
    if cancel.load(Ordering::Relaxed) {
        Err(ManufacturingError::new(
            ManufacturingErrorCode::Cancelled,
            "emitted-program verification cancelled",
        ))
    } else {
        Ok(())
    }
}

fn arithmetic_slack(value: f64) -> f64 {
    32.0 * f64::EPSILON * value.abs().max(1.0)
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1]).hypot(a[2] - b[2])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Millimetres,
    AbsolutePosition,
    AbsoluteExtrusion,
    ResetExtrusion,
    Travel,
    Deposit,
}

struct Decoded {
    command: Command,
    fields: [Option<f64>; 5], // X, Y, Z, E, F
}

// Deliberately not a general firmware parser. Only the declared inert dialect
// has meaning here; omitted axes/feeds, checksums and firmware extensions do not.
fn decode(line: &str) -> Result<Option<Decoded>, &'static str> {
    let code = line.split_once(';').map_or(line, |(code, _)| code);
    if !code.is_ascii()
        || code
            .bytes()
            .any(|byte| byte.is_ascii_control() && byte != b'\t')
    {
        return Err("non-ASCII or control byte in command");
    }
    let code = code.trim();
    if code.is_empty() {
        return Ok(None);
    }
    let mut words = code.split_ascii_whitespace();
    let command = match words.next() {
        Some("G21") => Command::Millimetres,
        Some("G90") => Command::AbsolutePosition,
        Some("M82") => Command::AbsoluteExtrusion,
        Some("G92") => Command::ResetExtrusion,
        Some("G0") => Command::Travel,
        Some("G1") => Command::Deposit,
        _ => return Err("unsupported or malformed command"),
    };
    let mut fields = [None; 5];
    for word in words {
        let (index, precision) = match word.as_bytes().first() {
            Some(b'X') => (0, 6),
            Some(b'Y') => (1, 6),
            Some(b'Z') => (2, 6),
            Some(b'E') => (3, 8),
            Some(b'F') => (4, 6),
            _ => return Err("unsupported command field"),
        };
        if fields[index].is_some() {
            return Err("duplicate command field");
        }
        fields[index] = Some(decimal(&word[1..], precision)?);
    }
    let present = fields.map(|field| field.is_some());
    let permitted = match command {
        Command::Millimetres | Command::AbsolutePosition | Command::AbsoluteExtrusion => [false; 5],
        Command::ResetExtrusion => [false, false, false, true, false],
        Command::Travel => [true, true, true, false, true],
        Command::Deposit => [true; 5],
    };
    if present != permitted {
        return Err("missing or unsupported fields for command");
    }
    if command == Command::ResetExtrusion && fields[3] != Some(0.0) {
        return Err("extrusion reset must be exactly E0");
    }
    Ok(Some(Decoded { command, fields }))
}

fn decimal(text: &str, precision: usize) -> Result<f64, &'static str> {
    let unsigned = text
        .strip_prefix('+')
        .or_else(|| text.strip_prefix('-'))
        .unwrap_or(text);
    let (integer, fractional) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    if integer.is_empty()
        || !integer.bytes().all(|byte| byte.is_ascii_digit())
        || !fractional.bytes().all(|byte| byte.is_ascii_digit())
        || fractional.len() > precision
        || (unsigned.contains('.') && fractional.is_empty())
    {
        return Err("malformed number or unsupported numeric precision");
    }
    let value = text.parse::<f64>().map_err(|_| "malformed number")?;
    if !value.is_finite() {
        return Err("nonfinite number");
    }
    Ok(value)
}

/// Replays a bounded actual program against borrowed normalized path semantics.
/// A successful result is software validation, never machine-ready certification.
pub fn verify(
    program: &str,
    plan: &NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    cancel: &AtomicBool,
) -> Result<VerificationReport, ManufacturingError> {
    cancelled(cancel)?;
    if program.len() > MAX_MANUFACTURING_BUNDLE_BYTES as usize {
        return Err(ManufacturingError::new(
            ManufacturingErrorCode::ResourceLimit,
            "emitted program exceeds manufacturing byte limit",
        ));
    }
    plan.validate(intent)?;

    let recipe = &intent.recipe;
    let printer = &intent.printer;
    let bead_area = (recipe.bead_width_mm - recipe.layer_height_mm) * recipe.layer_height_mm
        + std::f64::consts::PI * (recipe.layer_height_mm / 2.0).powi(2);
    let filament_area = std::f64::consts::PI * (recipe.filament_diameter_mm / 2.0).powi(2);
    let volume_per_mm = bead_area * recipe.flow_multiplier;
    if !volume_per_mm.is_finite()
        || volume_per_mm <= 0.0
        || !filament_area.is_finite()
        || filament_area <= 0.0
    {
        return Err(failed(
            "nominal extrusion arithmetic is not finite and positive",
        ));
    }

    // One borrowed point at a time. A path start denotes travel even when its
    // position coincides with the preceding path's endpoint.
    let mut expected = plan.layers.iter().flat_map(|layer| {
        layer.paths.iter().flat_map(|path| {
            path.points_mm
                .iter()
                .enumerate()
                .map(move |(index, point)| {
                    (
                        point,
                        index
                            .checked_sub(1)
                            .map(|previous| &path.points_mm[previous]),
                    )
                })
        })
    });
    let setup = [
        Command::Millimetres,
        Command::AbsolutePosition,
        Command::AbsoluteExtrusion,
        Command::ResetExtrusion,
    ];
    let mut setup_index = 0;
    let mut previous_position = None;
    let mut actual_e = 0.0;
    let mut expected_e = 0.0;
    let mut deposition_segments = 0_u32;
    let mut travel_segments = 0_u32;
    let mut max_position_error_mm: f64 = 0.0;
    let mut max_extrusion_error_mm: f64 = 0.0;

    for (line_index, line) in program.lines().enumerate() {
        cancelled(cancel)?;
        if line.len() > MAX_LINE_BYTES {
            return Err(ManufacturingError::new(
                ManufacturingErrorCode::ResourceLimit,
                format!("program line {} exceeds byte limit", line_index + 1),
            ));
        }
        let decoded = decode(line)
            .map_err(|reason| failed(format!("program line {}: {reason}", line_index + 1)))?;
        let Some(decoded) = decoded else { continue };
        if setup_index < setup.len() {
            if decoded.command != setup[setup_index] {
                return Err(failed(
                    "program must establish G21, G90, M82, G92 E0 in order",
                ));
            }
            setup_index += 1;
            continue;
        }
        let Some((target, source)) = expected.next() else {
            return Err(failed("extra command after final normalized motion"));
        };
        let required_command = if source.is_some() {
            Command::Deposit
        } else {
            Command::Travel
        };
        if decoded.command != required_command {
            return Err(failed(
                "command kind or mode does not match normalized motion",
            ));
        }
        // decode's exact field masks establish these values, not firmware defaults.
        let position = [
            decoded.fields[0].ok_or_else(|| failed("missing X"))?,
            decoded.fields[1].ok_or_else(|| failed("missing Y"))?,
            decoded.fields[2].ok_or_else(|| failed("missing Z"))?,
        ];
        for axis in 0..3 {
            let error = (position[axis] - target[axis]).abs();
            if error > XYZ_HALF_STEP + arithmetic_slack(target[axis]) {
                return Err(failed("command position differs from normalized path"));
            }
            if position[axis] < printer.build_envelope.min[axis]
                || position[axis] > printer.build_envelope.max[axis]
            {
                return Err(failed(
                    "command position lies outside printer build envelope",
                ));
            }
        }
        max_position_error_mm = max_position_error_mm.max(distance(position, *target));
        let feed = decoded.fields[4].ok_or_else(|| failed("missing feed"))?;
        let expected_feed = 60.0
            * if source.is_some() {
                recipe.print_speed_mm_s
            } else {
                recipe.travel_speed_mm_s
            };
        if feed <= 0.0
            || !expected_feed.is_finite()
            || (feed - expected_feed).abs() > F_HALF_STEP + arithmetic_slack(expected_feed)
            || feed / 60.0
                > printer.max_feed_mm_s
                    + F_HALF_STEP / 60.0
                    + arithmetic_slack(printer.max_feed_mm_s)
        {
            return Err(failed(
                "command feed violates expected speed or printer limit",
            ));
        }

        if let Some(source) = source {
            let from = previous_position.ok_or_else(|| failed("deposition before positioning"))?;
            let length = distance(from, position);
            let nominal_length = distance(*source, *target);
            let next_e = decoded.fields[3].ok_or_else(|| failed("missing extrusion"))?;
            let delta_e = next_e - actual_e;
            let nominal_e = nominal_length * volume_per_mm / filament_area;
            expected_e += nominal_e;
            if !expected_e.is_finite()
                || length <= 0.0
                || nominal_length <= 0.0
                || !nominal_e.is_finite()
                || nominal_e <= 0.0
                || delta_e <= 0.0
            {
                return Err(failed("zero-length, nonpositive or nonfinite deposition"));
            }
            let cumulative_error = (next_e - expected_e).abs();
            let segment_error = (delta_e - nominal_e).abs();
            max_extrusion_error_mm = max_extrusion_error_mm
                .max(cumulative_error)
                .max(segment_error);
            if cumulative_error > E_HALF_STEP + arithmetic_slack(expected_e)
                || segment_error > 2.0 * E_HALF_STEP + arithmetic_slack(expected_e)
            {
                return Err(failed(
                    "absolute or incremental extrusion differs from nominal volume",
                ));
            }
            let speed = feed / 60.0;
            let flow = delta_e * filament_area * speed / length;
            // E deltas have two rounded endpoints; a segment's decoded length
            // differs from its ideal by at most two XYZ rounding-vector norms.
            let position_delta_bound =
                2.0 * 3.0_f64.sqrt() * XYZ_HALF_STEP + arithmetic_slack(nominal_length);
            let rounding_flow_slack = (printer.max_volumetric_flow_mm3_s * position_delta_bound
                + 2.0 * E_HALF_STEP * filament_area * speed)
                / length
                + volume_per_mm * F_HALF_STEP / 60.0;
            if !flow.is_finite()
                || !rounding_flow_slack.is_finite()
                || flow
                    > printer.max_volumetric_flow_mm3_s
                        + rounding_flow_slack
                        + arithmetic_slack(flow)
            {
                return Err(failed("decoded volumetric flow exceeds printer limit"));
            }
            actual_e = next_e;
            deposition_segments += 1;
        } else {
            travel_segments += 1;
        }
        previous_position = Some(position);
    }
    cancelled(cancel)?;
    if setup_index != setup.len() || expected.next().is_some() || deposition_segments == 0 {
        return Err(failed("program is missing setup or normalized motions"));
    }
    let deposited_volume_mm3 = actual_e * filament_area;
    if !deposited_volume_mm3.is_finite() {
        return Err(failed("decoded deposited volume is nonfinite"));
    }
    Ok(VerificationReport {
        verified: true,
        coverage: vec![
            "Independent decoding of actual program bytes; strict G21/G90/M82/G92 E0 then absolute XYZ/E G0/G1 state transitions".into(),
            "Ordered travel/deposition completeness; XYZ6 <=0.0000005 mm/axis, E8 <=0.000000005 mm absolute / 0.00000001 mm delta, F6 <=0.0000005 mm/min; scale-aware floating roundoff only".into(),
            "Command-space build envelope, explicit feeds, printer speed and nominal volumetric flow limits".into(),
            "Rounded nominal bead area (width-height)*height + pi*(height/2)^2, explicit flow multiplier and actual decoded extrusion totals".into(),
        ],
        limitations: vec![
            "SOFTWARE VALIDATION ONLY, NOT MACHINE READY; no physical printer support or printability certification".into(),
            "No physics, structural/mechanical or material deposition validation".into(),
            "No collisions or component/fixture clearance validation; component shapes are metadata only".into(),
            "No firmware behavior, execution, calibration or hardware accuracy validation".into(),
            "No startup, homing or known physical initial position; first travel checks only commanded endpoint/feed".into(),
            "No temperature, thermal behavior, heating or cooling validation".into(),
            "No generated or physical support certification; normalized geometry policy is not a physical guarantee".into(),
        ],
        deposition_segments,
        travel_segments,
        deposited_volume_mm3,
        filament_length_mm: actual_e,
        max_position_error_mm,
        max_extrusion_error_mm,
    })
}

#[cfg(test)]
mod tests;
