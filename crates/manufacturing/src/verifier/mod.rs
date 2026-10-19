// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Software-only replay of actual program bytes, independent of the backend.

use std::sync::atomic::{AtomicBool, Ordering};

use spiling_contracts::manufacturing::program_coordinates::{
    project_xyz6_coordinate_mm, project_xyz6_point_mm,
};
use spiling_contracts::manufacturing::*;

mod expected;
mod replay;
pub(crate) use replay::ReplaySink;

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

fn verify_region_volume(
    layer: &PrintLayer,
    volume: f64,
    support_volume: f64,
) -> Result<(), ManufacturingError> {
    let declared = layer.region_coverage.nominal_deposition_volume_mm3;
    let support_declared = layer.region_coverage.support_nominal_deposition_volume_mm3;
    if !volume.is_finite()
        || (declared - volume).abs() > 1e-9 * volume.max(1.0) + arithmetic_slack(volume)
        || !support_volume.is_finite()
        || (support_declared - support_volume).abs()
            > 1e-9 * support_volume.max(1.0) + arithmetic_slack(support_volume)
    {
        return Err(failed(
            "region nominal deposition volume disagrees with decoded path semantics",
        ));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    Millimetres,
    AbsolutePosition,
    AbsoluteExtrusion,
    ResetExtrusion,
    Travel,
    Deposit,
    VolumetricOff,
    FeedScale,
    FlowScale,
    FanOff,
    Fan,
    Recovery,
    Home,
    LevelingOff,
    WaitBed,
    WaitNozzle,
    Barrier,
    NozzleOff,
    BedOff,
    MotorsOff,
    Celsius,
}

struct Decoded {
    command: Command,
    fields: [Option<f64>; 8], // X, Y, Z, E, F, S, R, D
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
        Some("M200") => Command::VolumetricOff,
        Some("M220") => Command::FeedScale,
        Some("M221") => Command::FlowScale,
        Some("M107") => Command::FanOff,
        Some("G28") => Command::Home,
        Some("M106") => Command::Fan,
        Some("M420") => Command::LevelingOff,
        Some("M190") => Command::WaitBed,
        Some("M109") => Command::WaitNozzle,
        Some("M400") => Command::Barrier,
        Some("M104") => Command::NozzleOff,
        Some("M140") => Command::BedOff,
        Some("M84") => Command::MotorsOff,
        Some("M149") => Command::Celsius,
        _ => return Err("unsupported or malformed command"),
    };
    if command == Command::Celsius {
        if words.next() != Some("C") || words.next().is_some() {
            return Err("temperature units must be exactly M149 C");
        }
        return Ok(Some(Decoded {
            command,
            fields: [None; 8],
        }));
    }
    let mut fields = [None; 8];
    for word in words {
        let (index, precision) = match word.as_bytes().first() {
            Some(b'X') => (0, 6),
            Some(b'Y') => (1, 6),
            Some(b'Z') => (2, 6),
            Some(b'E') => (3, 8),
            Some(b'F') => (4, 6),
            Some(b'S') => (5, 6),
            Some(b'R') => (6, 6),
            Some(b'D') => (7, 6),
            _ => return Err("unsupported command field"),
        };
        if fields[index].is_some() {
            return Err("duplicate command field");
        }
        fields[index] = Some(decimal(&word[1..], precision)?);
    }
    let command = if command == Command::Deposit && fields[0..3].iter().all(Option::is_none) {
        Command::Recovery
    } else {
        command
    };
    let present = fields.map(|field| field.is_some());
    let permitted = match command {
        Command::Millimetres
        | Command::AbsolutePosition
        | Command::AbsoluteExtrusion
        | Command::FanOff
        | Command::Home
        | Command::Barrier
        | Command::MotorsOff
        | Command::Celsius => [false; 8],
        Command::ResetExtrusion => [false, false, false, true, false, false, false, false],
        Command::Travel => [true, true, true, false, true, false, false, false],
        Command::Deposit => [true, true, true, true, true, false, false, false],
        Command::Recovery => [false, false, false, true, true, false, false, false],
        Command::VolumetricOff => [false, false, false, false, false, false, false, true],
        Command::FeedScale
        | Command::FlowScale
        | Command::LevelingOff
        | Command::NozzleOff
        | Command::BedOff
        | Command::Fan => [false, false, false, false, false, true, false, false],
        Command::WaitBed | Command::WaitNozzle => {
            [false, false, false, false, false, false, true, false]
        }
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

enum Lifecycle {
    Bare(Command),
    Scalar(Command, usize, f64),
    Motion([f64; 3], f64, Option<f64>),
}
struct MotionState {
    position: Option<[f64; 3]>,
    extrusion: f64,
}

fn inspect_lifecycle(
    decoded: &Decoded,
    operation: Lifecycle,
    intent: &ManufacturingIntent,
    state: &mut MotionState,
    line: u32,
    replay: Option<&mut ReplaySink>,
    prime_exclusion: Option<PrimeExclusionBoundsMm>,
) -> Result<(f64, f64), ManufacturingError> {
    let mut position_error = 0.0;
    let mut extrusion_error = 0.0;
    match operation {
        Lifecycle::Bare(command) => {
            if decoded.command != command {
                return Err(failed("missing or reordered lifecycle state/barrier"));
            }
        }
        Lifecycle::Scalar(command, field, value) => {
            if decoded.command != command
                || decoded.fields[field].is_none_or(|actual| {
                    (actual - value).abs() > F_HALF_STEP + arithmetic_slack(value)
                })
            {
                return Err(failed(
                    "lifecycle modal or thermal command differs from specification",
                ));
            }
        }
        Lifecycle::Motion(target, speed, extrusion) => {
            let command = if extrusion.is_some() {
                Command::Deposit
            } else {
                Command::Travel
            };
            if decoded.command != command {
                return Err(failed("missing or reordered lifecycle motion"));
            }
            let position = [
                decoded.fields[0].unwrap(),
                decoded.fields[1].unwrap(),
                decoded.fields[2].unwrap(),
            ];
            let feed = decoded.fields[4].unwrap();
            let envelope = intent.printer.build_envelope;
            let projected = project_xyz6_point_mm(target)?;
            if (0..3).any(|a| {
                position[a] != projected[a]
                    || position[a] < envelope.min[a]
                    || position[a] > envelope.max[a]
            }) || feed <= 0.0
                || (feed - speed * 60.0).abs() > F_HALF_STEP + arithmetic_slack(speed * 60.0)
                || feed / 60.0
                    > intent.printer.max_feed_mm_s + F_HALF_STEP / 60.0 + arithmetic_slack(speed)
            {
                return Err(failed("unsafe or mutated lifecycle endpoint/feed"));
            }
            position_error = distance(position, target);
            if let Some(expected_e) = extrusion {
                let e = decoded.fields[3].unwrap();
                extrusion_error = (e - expected_e).abs();
                let from = state
                    .position
                    .ok_or_else(|| failed("priming before positioning"))?;
                let bounds =
                    prime_exclusion.ok_or_else(|| failed("missing priming ownership envelope"))?;
                let mut first = 0.0_f64;
                let mut last = 1.0_f64;
                let mut separated = false;
                for axis in 0..2 {
                    let direction = position[axis] - from[axis];
                    if direction == 0.0 {
                        if from[axis] < bounds[0][axis] || from[axis] > bounds[1][axis] {
                            separated = true;
                            break;
                        }
                    } else {
                        let lower = (bounds[0][axis] - from[axis]) / direction;
                        let upper = (bounds[1][axis] - from[axis]) / direction;
                        first = first.max(lower.min(upper));
                        last = last.min(lower.max(upper));
                        if first > last {
                            separated = true;
                            break;
                        }
                    }
                }
                if !separated {
                    return Err(failed(
                        "decoded prime crosses conservative object XY ownership envelope",
                    ));
                }
                let length = distance(from, position);
                let delta = e - state.extrusion;
                if length <= 0.0
                    || delta <= 0.0
                    || (e - expected_e).abs() > E_HALF_STEP + arithmetic_slack(expected_e)
                {
                    return Err(failed(
                        "priming extrusion differs from bounded specification",
                    ));
                }
                let area =
                    std::f64::consts::PI * (intent.recipe.filament_diameter_mm / 2.0).powi(2);
                let flow = delta * area * feed / 60.0 / length;
                let slack = (intent.printer.max_volumetric_flow_mm3_s
                    * 2.0
                    * 3.0_f64.sqrt()
                    * XYZ_HALF_STEP
                    + E_HALF_STEP * area * feed / 60.0)
                    / length
                    + arithmetic_slack(flow);
                if !flow.is_finite() || flow > intent.printer.max_volumetric_flow_mm3_s + slack {
                    return Err(failed("decoded priming exceeds volumetric flow limit"));
                }
                state.extrusion = e;
            }
            if let Some(sink) = replay {
                sink.push(
                    position,
                    state.extrusion,
                    feed,
                    line,
                    u16::MAX,
                    if extrusion.is_some() { 7 } else { 6 },
                )?;
            }
            state.position = Some(position);
        }
    }
    Ok((position_error, extrusion_error))
}

/// Replays a bounded actual program against borrowed normalized path semantics.
/// A successful result is software validation, never machine-ready certification.
pub fn verify(
    program: &str,
    plan: &NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    cancel: &AtomicBool,
) -> Result<VerificationReport, ManufacturingError> {
    verify_actual(program, plan, intent, cancel, None)
}

pub(crate) fn verify_actual(
    program: &str,
    plan: &NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    cancel: &AtomicBool,
    mut replay: Option<&mut ReplaySink>,
) -> Result<VerificationReport, ManufacturingError> {
    cancelled(cancel)?;
    if program.len() > MAX_MANUFACTURING_BUNDLE_BYTES as usize {
        return Err(ManufacturingError::new(
            ManufacturingErrorCode::ResourceLimit,
            "emitted program exceeds manufacturing byte limit",
        ));
    }
    let prime_exclusion = plan.validate_and_prime_exclusion(intent)?;

    let recipe = &intent.recipe;
    let printer = &intent.printer;
    let filament_area = std::f64::consts::PI * (recipe.filament_diameter_mm / 2.0).powi(2);
    if !filament_area.is_finite() || filament_area <= 0.0 {
        return Err(failed("filament area is not finite and positive"));
    }

    let initial_position = match &printer.workflow {
        JobWorkflow::CompleteMarlin { job } => Some(project_xyz6_point_mm(job.prime_end_mm)?),
        JobWorkflow::DiagnosticBodyOnly => None,
    };
    expected::validate_preferred_seams(plan, recipe, initial_position, cancel)?;
    let mut expected = expected::Expected::new(plan, recipe, initial_position).peekable();
    let setup = [
        Command::Millimetres,
        Command::AbsolutePosition,
        Command::AbsoluteExtrusion,
        Command::ResetExtrusion,
    ];
    let mut setup_index = 0;
    let mut state = MotionState {
        position: None,
        extrusion: 0.0,
    };
    let mut expected_e = 0.0;
    let mut deposition_segments = 0_u32;
    let mut travel_segments = 0_u32;
    let mut max_position_error_mm: f64 = 0.0;
    let mut max_extrusion_error_mm: f64 = 0.0;
    let job = match &printer.workflow {
        JobWorkflow::DiagnosticBodyOnly => None,
        JobWorkflow::CompleteMarlin { job } => Some(job),
    };
    let mut lifecycle_index = 0usize;
    let mut finish_index = 0usize;
    let mut priming_filament_length_mm = 0.0;
    let mut volume_layer = 0u16;
    let mut layer_nominal_volume = 0.0;
    let mut layer_support_volume = 0.0;
    let mut support_deposition_segments = 0u32;
    let mut object_filament = 0.0;
    let mut support_filament = 0.0;
    let mut retraction_count = 0u32;
    let mut retracted_filament_mm = 0.0;
    let mut retracted_endpoint = None;
    let mut fan_state = None;
    let mut recovery_moves = 0u8;

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
        if let Some(job) = job {
            let operation = if lifecycle_index < 11 {
                let op = match lifecycle_index {
                    0 => Lifecycle::Bare(Command::Celsius),
                    1 => Lifecycle::Scalar(Command::VolumetricOff, 7, 0.0),
                    2 => Lifecycle::Scalar(Command::FeedScale, 5, 100.0),
                    3 => Lifecycle::Scalar(Command::FlowScale, 5, 100.0),
                    4 => Lifecycle::Bare(Command::FanOff),
                    5 => Lifecycle::Bare(Command::Home),
                    6 => Lifecycle::Scalar(Command::LevelingOff, 5, 0.0),
                    7 => Lifecycle::Scalar(Command::WaitBed, 6, job.bed_temperature_c),
                    8 => Lifecycle::Scalar(Command::WaitNozzle, 6, job.nozzle_temperature_c),
                    9 => Lifecycle::Motion(job.prime_start_mm, recipe.travel_speed_mm_s, None),
                    _ => Lifecycle::Motion(
                        job.prime_end_mm,
                        job.prime_speed_mm_s,
                        Some(job.prime_filament_length_mm),
                    ),
                };
                lifecycle_index += 1;
                Some(op)
            } else if expected.peek().is_none() {
                let last = state
                    .position
                    .ok_or_else(|| failed("park without deposition"))?;
                let op = match finish_index {
                    0 => Lifecycle::Bare(Command::Barrier),
                    1 => Lifecycle::Motion(
                        [last[0], last[1], job.park_position_mm[2]],
                        recipe.travel_speed_mm_s,
                        None,
                    ),
                    2 => Lifecycle::Motion(job.park_position_mm, recipe.travel_speed_mm_s, None),
                    3 => Lifecycle::Bare(Command::Barrier),
                    4 => Lifecycle::Scalar(Command::NozzleOff, 5, 0.0),
                    5 => Lifecycle::Scalar(Command::BedOff, 5, 0.0),
                    6 => Lifecycle::Bare(Command::FanOff),
                    7 => Lifecycle::Bare(Command::MotorsOff),
                    _ => return Err(failed("extra command after shutdown")),
                };
                finish_index += 1;
                Some(op)
            } else {
                None
            };
            if let Some(operation) = operation {
                if finish_index == 2
                    && state.position.is_some_and(|last| {
                        decoded.fields[0] != Some(last[0])
                            || decoded.fields[1] != Some(last[1])
                            || decoded.fields[2].is_some_and(|z| z <= last[2])
                    })
                {
                    return Err(failed(
                        "final lift must be vertical with representable clearance",
                    ));
                }
                if finish_index == 3
                    && state
                        .position
                        .is_some_and(|last| decoded.fields[2] != Some(last[2]))
                {
                    return Err(failed("final XY park must preserve clearance Z"));
                }
                let (position_error, extrusion_error) = inspect_lifecycle(
                    &decoded,
                    operation,
                    intent,
                    &mut state,
                    (line_index + 1) as u32,
                    replay.as_deref_mut(),
                    prime_exclusion,
                )?;
                max_position_error_mm = max_position_error_mm.max(position_error);
                max_extrusion_error_mm = max_extrusion_error_mm.max(extrusion_error);
                if decoded.command == Command::Home {
                    state.position = Some(job.home_position_mm);
                }
                if lifecycle_index == 11 && expected_e == 0.0 {
                    priming_filament_length_mm = state.extrusion;
                    expected_e = job.prime_filament_length_mm;
                }
                continue;
            }
        }
        let Some(action) = expected.next() else {
            return Err(failed("extra command after final normalized motion"));
        };
        let (target, source, layer_index, deposition_kind, association) = match action {
            expected::Action::Fan(pwm) => {
                if decoded.command != Command::Fan || decoded.fields[5] != Some(f64::from(pwm)) {
                    return Err(failed("missing or mutated process cooling state"));
                }
                fan_state = Some(pwm);
                continue;
            }
            expected::Action::Recovery {
                retract,
                layer,
                role,
            } => {
                let RetractionPolicy::Enabled {
                    distance_mm,
                    speed_mm_s,
                    z_hop_mm,
                    ..
                } = recipe.process.retraction
                else {
                    return Err(failed("stationary extrusion without enabled retraction"));
                };
                if decoded.command != Command::Recovery
                    || state.position.is_none()
                    || retract == retracted_endpoint.is_some()
                {
                    return Err(failed(
                        "missing, reordered or nonstationary retraction/recovery",
                    ));
                }
                let pwm = if role == DepositionKind::Bridge {
                    recipe.process.bridge_fan_pwm
                } else if layer == 0 {
                    recipe.process.fan_first_layers
                } else {
                    recipe.process.fan_after_first_layers
                };
                if fan_state != Some(pwm)
                    || (!retract && recovery_moves != if z_hop_mm > 0.0 { 3 } else { 1 })
                {
                    return Err(failed(
                        "recovery without required queued fan or complete travel/hop sequence",
                    ));
                }
                let next_e = decoded.fields[3].unwrap();
                let feed = decoded.fields[4].unwrap();
                let delta = next_e - state.extrusion;
                let desired = if retract { -distance_mm } else { distance_mm };
                let wanted_e = if retract {
                    expected_e - distance_mm
                } else {
                    expected_e
                };
                if (next_e - wanted_e).abs() > E_HALF_STEP + arithmetic_slack(wanted_e)
                    || (delta - desired).abs() > 2.0 * E_HALF_STEP + arithmetic_slack(expected_e)
                    || (!retract && retracted_endpoint != Some(next_e))
                    || (retract && delta >= 0.0)
                    || (!retract && delta <= 0.0)
                    || (feed - speed_mm_s * 60.0).abs() > F_HALF_STEP + arithmetic_slack(feed)
                    || feed <= 0.0
                    || feed / 60.0 > printer.max_feed_mm_s + F_HALF_STEP / 60.0
                {
                    return Err(failed(
                        "actual retraction distance, recovery or feed differs from policy",
                    ));
                }
                max_extrusion_error_mm = max_extrusion_error_mm
                    .max((next_e - wanted_e).abs())
                    .max((delta - desired).abs());
                if retract {
                    recovery_moves = 0;
                    retraction_count += 1;
                    retracted_filament_mm -= delta;
                    if retraction_count > 50_000 {
                        return Err(failed("process event budget exceeded"));
                    }
                }
                retracted_endpoint = if retract { Some(state.extrusion) } else { None };
                state.extrusion = next_e;
                if let Some(sink) = replay.as_deref_mut() {
                    sink.push(
                        state.position.unwrap(),
                        next_e,
                        feed,
                        (line_index + 1) as u32,
                        layer,
                        if retract { 11 } else { 12 },
                    )?;
                }
                continue;
            }
            expected::Action::Move {
                target,
                source,
                layer,
                role,
                association,
            } => (target, source, layer, role, association),
        };
        if layer_index != volume_layer {
            verify_region_volume(
                &plan.layers[volume_layer as usize],
                layer_nominal_volume,
                layer_support_volume,
            )?;
            for skipped in volume_layer + 1..layer_index {
                verify_region_volume(&plan.layers[skipped as usize], 0.0, 0.0)?;
            }
            volume_layer = layer_index;
            layer_nominal_volume = 0.0;
            layer_support_volume = 0.0;
        }
        let wanted_fan = if deposition_kind == DepositionKind::Bridge {
            recipe.process.bridge_fan_pwm
        } else if layer_index == 0 {
            recipe.process.fan_first_layers
        } else {
            recipe.process.fan_after_first_layers
        };
        if fan_state != Some(wanted_fan) {
            return Err(failed(
                "motion without independently decoded queued process fan state",
            ));
        }
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
            // Independent parsing admits at most six decimal places. Compare
            // that decoded number with this output contract's projection,
            // not raw-plan ±half-step (which admits the wrong tie neighbor).
            // Both conversions are correctly rounded f64 values; physical
            // decimal enclosure remains the shared proof operand, not extra
            // permission to choose another encoded coordinate.
            if position[axis] != project_xyz6_coordinate_mm(target[axis])? {
                return Err(failed("command position differs from projected XYZ6 path"));
            }
            if position[axis] < printer.build_envelope.min[axis]
                || position[axis] > printer.build_envelope.max[axis]
            {
                return Err(failed(
                    "command position lies outside printer build envelope",
                ));
            }
        }
        if retracted_endpoint.is_some() && source.is_none() {
            let previous = state
                .position
                .ok_or_else(|| failed("hop without known actual position"))?;
            let RetractionPolicy::Enabled { z_hop_mm, .. } = recipe.process.retraction else {
                unreachable!()
            };
            if z_hop_mm > 0.0 {
                let unsafe_hop = match recovery_moves {
                    0 => {
                        position[0..2] != previous[0..2]
                            || position[2] <= previous[2]
                            || position[2] <= plan.layers[layer_index as usize].z_mm
                    }
                    1 => position[2] != previous[2],
                    2 => position[0..2] != previous[0..2] || position[2] >= previous[2],
                    _ => true,
                };
                if unsafe_hop {
                    return Err(failed(
                        "actual travel hop lacks vertical clearance, constant elevated Z or stationary-XY landing",
                    ));
                }
            }
            recovery_moves += 1;
        }
        max_position_error_mm = max_position_error_mm.max(distance(position, target));
        let feed = decoded.fields[4].ok_or_else(|| failed("missing feed"))?;
        let mut process_speed = recipe.print_speed_mm_s;
        let mut process_flow = recipe.flow_multiplier;
        if layer_index == 0 {
            process_speed *= recipe.process.first_layer_speed_factor;
            process_flow *= recipe.process.first_layer_flow_factor;
        }
        if deposition_kind == DepositionKind::Bridge {
            process_speed *= recipe.process.bridge_speed_factor;
            process_flow *= recipe.process.bridge_flow_factor;
        }
        let volume_per_mm = association.map_or(0.0, |association| {
            let height = association.physical_z[1] - association.physical_z[0];
            ((association.width - height) * height
                + std::f64::consts::PI * (height * 0.5).powi(2)) * process_flow
        });
        let expected_feed = 60.0
            * if source.is_some() {
                process_speed
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
            let from = state
                .position
                .ok_or_else(|| failed("deposition before positioning"))?;
            if matches!(deposition_kind, DepositionKind::BottomSkin | DepositionKind::TopSkin
                | DepositionKind::SolidFill | DepositionKind::SparseFill) {
                let policy = if deposition_kind == DepositionKind::TopSkin {
                    recipe.top_skin_axis
                } else { recipe.rectilinear_axis };
                let fixed_axis = match policy {
                    RectilinearAxisPolicy::X => 1,
                    RectilinearAxisPolicy::Y => 0,
                    RectilinearAxisPolicy::AlternatingXY => if layer_index.is_multiple_of(2) { 1 } else { 0 },
                    RectilinearAxisPolicy::AlternatingYX => if layer_index.is_multiple_of(2) { 0 } else { 1 },
                };
                if from[fixed_axis] != position[fixed_axis] {
                    return Err(failed("actual ordinary deposition violates its authored axis policy"));
                }
            }
            let length = distance(from, position);
            let nominal_length = distance(source, target);
            let support = matches!(
                deposition_kind,
                DepositionKind::Support | DepositionKind::SupportInterface
            );
            if support {
                layer_support_volume += nominal_length * volume_per_mm;
            } else {
                layer_nominal_volume += nominal_length * volume_per_mm;
            }
            if retracted_endpoint.is_some() {
                return Err(failed("deposition before exact stationary recovery"));
            }
            let next_e = decoded.fields[3].ok_or_else(|| failed("missing extrusion"))?;
            let delta_e = next_e - state.extrusion;
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
            state.extrusion = next_e;
            if support {
                support_deposition_segments += 1;
                support_filament += delta_e;
            } else {
                deposition_segments += 1;
                object_filament += delta_e;
            }
            if deposition_segments + support_deposition_segments > MAX_DEPOSITION_SEGMENTS {
                return Err(failed("whole deposition budget exceeded"));
            }
        } else {
            travel_segments += 1;
            if travel_segments > MAX_DEPOSITION_SEGMENTS {
                return Err(failed("travel event budget exceeded"));
            }
        }
        if let Some(sink) = replay.as_deref_mut() {
            let kind = if source.is_none() {
                0
            } else {
                match deposition_kind {
                    spiling_contracts::manufacturing::DepositionKind::Perimeter => 1,
                    spiling_contracts::manufacturing::DepositionKind::SolidFill => 2,
                    DepositionKind::BottomSkin => 3,
                    DepositionKind::TopSkin => 4,
                    DepositionKind::SparseFill => 5,
                    DepositionKind::Bridge => 8,
                    DepositionKind::Support => 9,
                    DepositionKind::SupportInterface => 10,
                    DepositionKind::SeparatorWall => 13,
                }
            };
            sink.push_associated(
                position,
                state.extrusion,
                feed,
                (line_index + 1) as u32,
                layer_index,
                kind,
                association,
            )?;
        }
        state.position = Some(position);
    }
    cancelled(cancel)?;
    if setup_index != setup.len()
        || expected.next().is_some()
        || deposition_segments + support_deposition_segments == 0
        || retracted_endpoint.is_some()
        || (job.is_some() && (lifecycle_index != 11 || finish_index != 8))
    {
        return Err(failed("program is missing setup or normalized motions"));
    }
    verify_region_volume(
        &plan.layers[volume_layer as usize],
        layer_nominal_volume,
        layer_support_volume,
    )?;
    for skipped in volume_layer as usize + 1..plan.layers.len() {
        verify_region_volume(&plan.layers[skipped], 0.0, 0.0)?;
    }
    interfaces::verify_projected_byte_geometry(plan, recipe, cancel)?;
    let deposited_volume_mm3 = object_filament * filament_area;
    if !deposited_volume_mm3.is_finite() {
        return Err(failed("decoded deposited volume is nonfinite"));
    }
    Ok(VerificationReport {
        verified: true,
        coverage: vec![
            "Independent decoding of actual bytes; required ordered workflow, strict millimetre/absolute XYZ/E modes and lifecycle/object command field masks".into(),
            "Ordered travel/deposition completeness; XYZ6 <=0.0000005 mm/axis, E8 <=0.000000005 mm absolute / 0.00000001 mm delta, F6 <=0.0000005 mm/min; scale-aware floating roundoff only".into(),
            "Command-space build envelope, explicit feeds, printer speed and nominal volumetric flow limits".into(),
            "Authored ordinary hatch axis at actual scheduled layer index in the XYZ6 build/command frame (Bridge/support excluded); preferred perimeter seam direction and deterministic projected tie ordering".into(),
            "Rounded nominal bead area (width-height)*height + pi*(height/2)^2, explicit flow multiplier and actual decoded extrusion totals".into(),
            "True scheduled layer thickness, first-layer and bridge speed/flow overrides, actual queued cooling PWM and separate object/support deposited material".into(),
            "Conditional stationary E-only retraction, declared distance/feed, bounded travel Z-hop and exact recovery; recovered extrusion is not deposited material".into(),
            if job.is_some() { "Marlin 2 typed complete job: scale/volumetric reset, native homing declaration, heating/cooling waits, bounded priming, final barriers/lift/park/heater/fan/motor shutdown".into() } else { "Explicit diagnostic body-only workflow; no executable lifecycle".into() },
            if job.is_some() { "Explicit M149 C and integral firmware Celsius targets; actual priming excluded from conservatively expanded whole-object XY ownership bounds, including coordinate serialization padding".into() } else { "Diagnostic workflow has no thermal or priming obligations".into() },
        ],
        limitations: vec![
            "SOFTWARE VALIDATION ONLY, NOT MACHINE READY; no physical printer support or printability certification".into(),
            "No physics, structural/mechanical or material deposition validation".into(),
            "No collisions or component/fixture clearance validation; component shapes are metadata only".into(),
            "No firmware behavior, execution, calibration or hardware accuracy validation".into(),
            "Homing path, thermal sensor truth, firmware configuration and calibration records are declared, not physically measured or certified".into(),
            if job.is_some() { "Complete Marlin lifecycle/modal/wait/barrier semantics checked in software only".into() } else { "Diagnostic body-only workflow: no homing or thermal waits; first commanded endpoint has unknown start".into() },
            "Generated supports/bridges are bounded software geometry/process only; no physical support certification or swept-bead guarantee".into(),
        ],
        deposition_segments,
        travel_segments,
        deposited_volume_mm3,
        filament_length_mm: object_filament,
        max_position_error_mm,
        max_extrusion_error_mm,
        lifecycle_motion_count: if job.is_some() { 4 } else { 0 },
        priming_filament_length_mm,
        support_deposition_segments,
        support_deposited_volume_mm3: support_filament * filament_area,
        support_filament_length_mm: support_filament,
        retraction_count,
        retracted_filament_mm,
    })
}

mod interfaces;
#[cfg(test)]
mod tests;
