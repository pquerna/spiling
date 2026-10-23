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

/// Software deposition-resolution policy, not calibrated motion performance.
pub const MIN_DEPOSITION_COMMAND_MM: f64 = 0.02;
/// Reserved lateral budget for XYZ6/E8 nominal-width and coordinate enclosure.
pub const MAX_BEAD_COMMAND_LATERAL_ERROR_MM: f64 = 0.001;
/// Continuing-wall width change per millimetre of effective transition length.
pub const MAX_WALL_WIDTH_GRADIENT: f64 = 0.20;

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

/// Guaranteed vertical interior of the associated nominal bead at its actual
/// XYZ6 top. Unlike an outer clearance hull, this interval can establish strict
/// positive nominal section overlap. It is not a physical bead measurement.
pub fn project_xyz6_bead_inner_height_mm(
    physical_z_mm: [f64; 2],
) -> Result<[f64; 2], ManufacturingError> {
    let z = project_xyz6_coordinate_mm(physical_z_mm[1])?;
    let error = xyz6_coordinate_error_mm(z)?;
    let height_lower = enclosed_vertical_gap_mm(physical_z_mm[1], physical_z_mm[0])?;
    let top_upper = -enclosed_vertical_gap_mm(-z, error)?;
    let lower_inner = -enclosed_vertical_gap_mm(height_lower, top_upper)?;
    let upper_inner = enclosed_vertical_gap_mm(z, error)?;
    Ok([lower_inner, upper_inner])
}

/// Invert one constant-section G1's declared rounded-bead area algebra using
/// both extrusion endpoints. Returns the full positive width-equivalent interval,
/// not measured bead shape or proof that every enclosed width is a valid stadium.
/// Rounded circular commands can yield equivalents below `height_mm`. Callers
/// admit nominal stadium geometry and independently check forward E8 volume;
/// neither clamp this interval nor add intended-accumulator quantization error
/// to its actual decoded uncertainty. Preserve both bounds for process-width,
/// gradient and lateral-fidelity checks.
/// Length and E uncertainties must enclose projection/decimal/reset state.
pub fn commanded_stadium_width_enclosure_mm(
    length_mm: f64,
    length_error_mm: f64,
    extrusion_delta_mm: f64,
    extrusion_error_mm: f64,
    filament_area_mm2: f64,
    height_mm: f64,
    flow_factor: f64,
) -> Result<[f64; 2], ManufacturingError> {
    if [
        length_mm,
        extrusion_delta_mm,
        filament_area_mm2,
        height_mm,
        flow_factor,
    ]
    .iter()
    .any(|v| !v.is_normal() || *v <= 0.0)
        || [length_error_mm, extrusion_error_mm]
            .iter()
            .any(|v| !v.is_finite() || *v < 0.0)
    {
        return Err(conditioning(
            "commanded bead inversion requires positive finite dimensions and nonnegative uncertainty",
        ));
    }
    let length_lower = (length_mm - length_error_mm).next_down();
    let length_upper = (length_mm + length_error_mm).next_up();
    let extrusion_lower = (extrusion_delta_mm - extrusion_error_mm).next_down();
    let extrusion_upper = (extrusion_delta_mm + extrusion_error_mm).next_up();
    if [length_lower, length_upper, extrusion_lower, extrusion_upper]
        .iter()
        .any(|v| !v.is_normal() || *v <= 0.0)
    {
        return Err(conditioning(
            "commanded bead is unresolved at XYZ6/E8 precision",
        ));
    }
    let correction = height_mm * (1.0 - std::f64::consts::FRAC_PI_4);
    let denominator = flow_factor * height_mm;
    let numerator_lower = filament_area_mm2 * extrusion_lower;
    let numerator_upper = filament_area_mm2 * extrusion_upper;
    let denominator_lower = length_lower * denominator;
    let denominator_upper = length_upper * denominator;
    // The relative-error bound below applies only to normal arithmetic. A
    // nonzero subnormal product can silently lose far more than 64 ulps of the
    // final width even when the ratio is finite and comfortably above height.
    if [
        correction,
        denominator,
        numerator_lower,
        numerator_upper,
        denominator_lower,
        denominator_upper,
    ]
    .iter()
    .any(|value| !value.is_normal())
    {
        return Err(conditioning(
            "commanded bead inversion has an underflowing or overflowing intermediate product",
        ));
    }
    let quotient_lower = numerator_lower / denominator_upper;
    let quotient_upper = numerator_upper / denominator_lower;
    let lower = quotient_lower + correction;
    let upper = quotient_upper + correction;
    let arithmetic = 64.0 * f64::EPSILON * lower.abs().max(upper.abs()).max(height_mm);
    if [quotient_lower, quotient_upper, lower, upper, arithmetic]
        .iter()
        .any(|value| !value.is_normal())
    {
        return Err(conditioning(
            "commanded bead inversion arithmetic is outside its normal-error envelope",
        ));
    }
    let bounds = [
        (lower - arithmetic).next_down(),
        (upper + arithmetic).next_up(),
    ];
    if bounds.iter().any(|v| !v.is_finite()) || bounds[0] <= 0.0 || bounds[0] > bounds[1] {
        return Err(conditioning(
            "commanded positive width-equivalent interval cannot be enclosed",
        ));
    }
    Ok(bounds)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circular_command_keeps_its_untrimmed_width_equivalent_interval() {
        // Diameter-one bead, diameter-two filament: E=L/4 exactly.
        let interval = commanded_stadium_width_enclosure_mm(
            4.0,
            0.000002,
            1.0,
            0.00000001,
            std::f64::consts::PI,
            1.0,
            1.0,
        )
        .unwrap();
        assert!(interval[0] < 1.0 && interval[1] > 1.0);
        let below = commanded_stadium_width_enclosure_mm(
            4.0,
            0.000002,
            0.5,
            0.00000001,
            std::f64::consts::PI,
            1.0,
            1.0,
        )
        .unwrap();
        assert!(
            below[0] > 0.0 && below[1] < 1.0,
            "the algebraic helper reports the interval; model compatibility belongs to its consumer"
        );
    }

    #[test]
    fn commanded_width_inverts_normal_volume_and_refuses_subnormal_products() {
        let width = 0.45;
        let height = 0.2;
        let length = 0.02;
        let flow = 0.97;
        let filament_area = std::f64::consts::PI * (1.75f64 * 0.5).powi(2);
        let area = (width - height) * height + std::f64::consts::FRAC_PI_4 * height * height;
        let extrusion = area * length * flow / filament_area;
        let enclosure = commanded_stadium_width_enclosure_mm(
            length,
            0.000002,
            extrusion,
            0.00000001,
            filament_area,
            height,
            flow,
        )
        .unwrap();
        assert!(enclosure[0] <= width && width <= enclosure[1]);
        assert!(0.30 < enclosure[0] && enclosure[1] < 0.75);

        // Tiny inputs are not rejected merely for being nonphysical: all
        // intermediate products in this inverse remain normal and conditioned.
        let normal =
            commanded_stadium_width_enclosure_mm(1e-80, 0.0, 1e-80, 0.0, 1.234e-80, 1e-80, 1.0)
                .unwrap();
        assert!(normal[0] <= 1.234 && 1.234 <= normal[1]);

        // Each operand is normal, each product is positive, and the old finite
        // ratio was about 1.234. Its subnormal products have relative roundoff
        // vastly exceeding a normal f64 relative-error allowance.
        let failure =
            commanded_stadium_width_enclosure_mm(1e-160, 0.0, 1e-160, 0.0, 1.234e-160, 1e-160, 1.0)
                .unwrap_err();
        assert_eq!(
            failure.code,
            ManufacturingErrorCode::ProgramCoordinateConditioningFailure
        );
    }
}
