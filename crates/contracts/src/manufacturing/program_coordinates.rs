// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Coordinate representation of the current CartesianAbsoluteGcodeV1 and
//! Marlin2SingleExtruderGcodeV1 output contracts: Cartesian millimetres written
//! with six decimal places. This is not a generic plan-rounding policy and
//! must not be inherited by a future output dialect without explicit review.
//! Serialization/planning remain manufacturing-backend responsibilities.

use super::{ManufacturingError, ManufacturingErrorCode};

/// Largest current XYZ6 coordinate whose rounded scaled integer is exact and
/// whose adjacent decimal lattice points remain distinct under f64 decoding
/// (about 4503.6 km). Raw multiplication still has its normal rounding error.
pub const MAX_XYZ6_COORDINATE_MM: f64 = (1u64 << 52) as f64 / 1_000_000.0;

fn conditioning(message: &'static str) -> ManufacturingError {
    ManufacturingError::new(
        ManufacturingErrorCode::ProgramCoordinateConditioningFailure,
        message,
    )
}

/// Sole projection to the current Cartesian XYZ6 command representation.
pub fn project_xyz6_coordinate_mm(value: f64) -> Result<f64, ManufacturingError> {
    if !value.is_finite() || value.abs() > MAX_XYZ6_COORDINATE_MM {
        return Err(conditioning(
            "Cartesian XYZ6 coordinate exceeds the certified numeric range",
        ));
    }
    let value = (value * 1_000_000.0).round() / 1_000_000.0;
    Ok(if value == 0.0 { 0.0 } else { value })
}

/// Constant-size checked current-output point projection; no coordinate buffer.
pub fn project_xyz6_point_mm(point: [f64; 3]) -> Result<[f64; 3], ManufacturingError> {
    Ok([
        project_xyz6_coordinate_mm(point[0])?,
        project_xyz6_coordinate_mm(point[1])?,
        project_xyz6_coordinate_mm(point[2])?,
    ])
}

/// Remaining physical-decimal error of an already projected XYZ6 coordinate.
/// This is an output-representation enclosure, never geometry-policy slack.
pub fn xyz6_coordinate_error_mm(rounded: f64) -> Result<f64, ManufacturingError> {
    if !rounded.is_finite() || rounded.abs() > MAX_XYZ6_COORDINATE_MM {
        return Err(conditioning(
            "Cartesian XYZ6 coordinate enclosure exceeds the certified numeric range",
        ));
    }
    // A six-decimal number that is also an exact multiple of 1/64 has an
    // exact binary representation: 10^6 / 64 is the integer 15625. This also
    // covers integer/zero coordinates, without weakening non-dyadic bounds.
    let binary_grid = rounded * 64.0;
    if binary_grid.is_finite() && binary_grid.fract() == 0.0 {
        return Ok(0.0);
    }
    let half_quantum = (0.5 / 1_000_000.0f64).next_up();
    let ulp = (rounded.next_up() - rounded).max(rounded - rounded.next_down());
    Ok(if ulp < half_quantum {
        ulp.next_up()
    } else {
        half_quantum
    })
}

/// Closed vertical enclosure of a nominal deposited bead at its actual XYZ6
/// top, including subtraction roundoff and the remaining decimal error.
pub fn project_xyz6_bead_height_mm(
    z_mm: f64,
    height_mm: f64,
) -> Result<[f64; 2], ManufacturingError> {
    let z = project_xyz6_coordinate_mm(z_mm)?;
    let error = xyz6_coordinate_error_mm(z)?;
    Ok([
        enclosed_vertical_gap_mm(enclosed_vertical_gap_mm(z, height_mm)?, error)?,
        -enclosed_vertical_gap_mm(-z, error)?,
    ])
}

/// Closed vertical hull of both actual XYZ6 segment endpoint beads.
pub fn project_xyz6_segment_bead_height_mm(
    start_z_mm: f64,
    end_z_mm: f64,
    height_mm: f64,
) -> Result<[f64; 2], ManufacturingError> {
    let start = project_xyz6_bead_height_mm(start_z_mm, height_mm)?;
    let end = project_xyz6_bead_height_mm(end_z_mm, height_mm)?;
    Ok([start[0].min(end[0]), start[1].max(end[1])])
}

/// Lower bound on the gap between an already enclosed lower/upper program
/// height pair. Error-free TwoDiff keeps exactly representable contact gaps
/// admissible; only a negative residual requires outward rounding.
pub fn enclosed_vertical_gap_mm(
    above_lower_mm: f64,
    below_upper_mm: f64,
) -> Result<f64, ManufacturingError> {
    let difference = above_lower_mm - below_upper_mm;
    let virtual_b = above_lower_mm - difference;
    let recovered_a = difference + virtual_b;
    let round_a = above_lower_mm - recovered_a;
    let round_b = virtual_b - below_upper_mm;
    let residual = round_a + round_b;
    // Finite rounded subtraction alone does not establish finite compensation.
    // A recovered virtual operand can overflow even when the head is finite.
    if !above_lower_mm.is_finite()
        || !below_upper_mm.is_finite()
        || !difference.is_finite()
        || !virtual_b.is_finite()
        || !recovered_a.is_finite()
        || !round_a.is_finite()
        || !round_b.is_finite()
        || !residual.is_finite()
    {
        return Err(conditioning(
            "Cartesian XYZ6 gap compensation is not finite",
        ));
    }
    let lower = if residual < 0.0 {
        difference.next_down()
    } else {
        difference
    };
    if !lower.is_finite() {
        return Err(conditioning("Cartesian XYZ6 gap enclosure is not finite"));
    }
    Ok(lower)
}
