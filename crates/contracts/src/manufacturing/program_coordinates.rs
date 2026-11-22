// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Coordinate representation explicitly adopted by CartesianAbsoluteGcodeV1,
//! Marlin2SingleExtruderGcodeV1 and Marlin2FixedDualGcodeV1: Cartesian
//! millimetres written with three decimal places (micrometres, finer than any
//! supported printer's motor step), extrusion with six and feed with one.
//! This is not a generic plan-rounding policy for future output dialects.
//! Serialization/planning remain manufacturing-backend responsibilities.

use super::{ManufacturingError, ManufacturingErrorCode};

/// Decimal places of an emitted X, Y, Z, I or J word.
pub const PROGRAM_XYZ_DECIMALS: usize = 3;
/// Coordinate lattice points per millimetre.
pub const PROGRAM_XYZ_SCALE: f64 = 1_000.0;
/// Largest power of two dividing [`PROGRAM_XYZ_SCALE`]: a lattice value that
/// is a multiple of its reciprocal is exactly representable in binary.
const PROGRAM_XYZ_DYADIC: f64 = 8.0;
/// Decimal places of an emitted E word.
pub const PROGRAM_E_DECIMALS: usize = 6;
/// Extrusion lattice points per millimetre of filament.
pub const PROGRAM_E_TICKS: u64 = 1_000_000;
pub const PROGRAM_E_SCALE: f64 = PROGRAM_E_TICKS as f64;
/// Largest power of two dividing [`PROGRAM_E_SCALE`].
pub const PROGRAM_E_DYADIC: f64 = 64.0;
/// Decimal places of an emitted F word, and its lattice points per mm/min.
pub const PROGRAM_F_DECIMALS: usize = 1;
pub const PROGRAM_F_SCALE: f64 = 10.0;
/// Largest current coordinate whose rounded scaled integer is exact and
/// whose adjacent decimal lattice points remain distinct under f64 decoding.
/// Raw multiplication still has its normal rounding error.
pub const MAX_XYZ6_COORDINATE_MM: f64 = (1u64 << 52) as f64 / PROGRAM_XYZ_SCALE;
/// One step of the coordinate lattice.
pub const XYZ6_QUANTUM_MM: f64 = 1.0 / PROGRAM_XYZ_SCALE;
pub const XYZ6_PROJECTION_SCREEN_MM: f64 = 10.0 * XYZ6_QUANTUM_MM;

/// Software deposition-resolution policy, not calibrated motion performance.
pub const MIN_DEPOSITION_COMMAND_MM: f64 = 0.02;
/// Binary arithmetic slack for comparing lattice-derived lengths and heights
/// with a limit: values exactly at a limit can subtract a hair past it
/// (10.014 - 9.994 < 0.02). A thousandth of a lattice step, so it never
/// admits a different representable value.
pub const LATTICE_ARITHMETIC_SLACK_MM: f64 = XYZ6_QUANTUM_MM / 1_000.0;
/// Whether a command between lattice points is at least the minimum length.
pub fn meets_minimum_command(length_mm: f64) -> bool {
    length_mm >= MIN_DEPOSITION_COMMAND_MM - LATTICE_ARITHMETIC_SLACK_MM
}
/// Shortest raw (unrounded) edge that always rounds to a command at least
/// [`MIN_DEPOSITION_COMMAND_MM`] long: projection moves each endpoint at most
/// half a lattice step on each axis.
pub const MIN_RAW_DEPOSITION_COMMAND_MM: f64 =
    MIN_DEPOSITION_COMMAND_MM + 2.0 * std::f64::consts::SQRT_2 * XYZ6_QUANTUM_MM;
/// Reserved lateral budget for command representation, two lattice steps:
/// half a step on each axis moves a centreline at most 0.71 um, and an arc
/// replayed through its rounded I/J centre deviates about one step more. The
/// E word's own rounding is enclosed separately, where a width is inverted
/// from it.
pub const MAX_BEAD_COMMAND_LATERAL_ERROR_MM: f64 = 2.0 * XYZ6_QUANTUM_MM;
/// Continuing-wall width change per millimetre of effective transition length.
pub const MAX_WALL_WIDTH_GRADIENT: f64 = 0.20;

fn conditioning(message: &'static str) -> ManufacturingError {
    ManufacturingError::new(
        ManufacturingErrorCode::ProgramCoordinateConditioningFailure,
        message,
    )
}

/// Sole projection to the current Cartesian XYZ6 command representation.
#[inline]
pub fn project_xyz6_coordinate_mm(value: f64) -> Result<f64, ManufacturingError> {
    if !value.is_finite() || value.abs() > MAX_XYZ6_COORDINATE_MM {
        return Err(conditioning(
            "Cartesian XYZ6 coordinate exceeds the certified numeric range",
        ));
    }
    let value = (value * PROGRAM_XYZ_SCALE).round() / PROGRAM_XYZ_SCALE;
    Ok(if value == 0.0 { 0.0 } else { value })
}

/// Constant-size checked current-output point projection; no coordinate buffer.
#[inline]
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
    // A lattice number that is also an exact multiple of 1/8 has an exact
    // binary representation: 10^3 / 8 is the integer 125. This also covers
    // integer/zero coordinates, without weakening non-dyadic bounds.
    let binary_grid = rounded * PROGRAM_XYZ_DYADIC;
    if binary_grid.is_finite() && binary_grid.fract() == 0.0 {
        return Ok(0.0);
    }
    let half_quantum = (0.5 / PROGRAM_XYZ_SCALE).next_up();
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

/// The geometry of one commanded XY circular move (G2/G3): the start and end
/// as commanded, the center as start plus the commanded I/J offsets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CommandedArc {
    pub center_mm: [f64; 2],
    /// Distance of the start from the center; firmware moves on this radius.
    pub radius_mm: f64,
    /// Distance of the end from the center.
    pub end_radius_mm: f64,
    /// Angle swept in the commanded direction, in (0, 2*pi].
    pub sweep_radians: f64,
    /// Length along the start radius.
    pub length_mm: f64,
    start_angle: f64,
    clockwise: bool,
}

impl CommandedArc {
    /// A horizontal arc from `start` to `end` about `center`. Start and end
    /// share Z; equal start and end is a full turn.
    pub fn new(
        start: [f64; 3],
        end: [f64; 3],
        center_mm: [f64; 2],
        clockwise: bool,
    ) -> Result<Self, ManufacturingError> {
        let invalid = || {
            ManufacturingError::new(
                ManufacturingErrorCode::InvalidSpecification,
                "commanded arc is not a finite horizontal circle",
            )
        };
        if start[2] != end[2]
            || !start
                .iter()
                .chain(&end)
                .chain(&center_mm)
                .all(|v| v.is_finite())
        {
            return Err(invalid());
        }
        let radius_mm = (start[0] - center_mm[0]).hypot(start[1] - center_mm[1]);
        let end_radius_mm = (end[0] - center_mm[0]).hypot(end[1] - center_mm[1]);
        if !(radius_mm > 0.0 && end_radius_mm > 0.0) {
            return Err(invalid());
        }
        let start_angle = (start[1] - center_mm[1]).atan2(start[0] - center_mm[0]);
        let end_angle = (end[1] - center_mm[1]).atan2(end[0] - center_mm[0]);
        let mut sweep = if clockwise {
            start_angle - end_angle
        } else {
            end_angle - start_angle
        };
        while sweep <= 0.0 {
            sweep += std::f64::consts::TAU;
        }
        Ok(Self {
            center_mm,
            radius_mm,
            end_radius_mm,
            sweep_radians: sweep,
            length_mm: radius_mm * sweep,
            start_angle,
            clockwise,
        })
    }

    /// Angle swept from the start to `point`'s direction, in [0, 2*pi).
    pub fn angle_to(&self, point: [f64; 3]) -> f64 {
        let angle = (point[1] - self.center_mm[1]).atan2(point[0] - self.center_mm[0]);
        let delta = if self.clockwise {
            self.start_angle - angle
        } else {
            angle - self.start_angle
        };
        delta.rem_euclid(std::f64::consts::TAU)
    }

    /// XY bounds of the swept arc on the start radius.
    pub fn xy_bounds(&self) -> [[f64; 2]; 2] {
        let at = |angle: f64| {
            [
                self.center_mm[0] + self.radius_mm * angle.cos(),
                self.center_mm[1] + self.radius_mm * angle.sin(),
            ]
        };
        let direction = if self.clockwise { -1.0 } else { 1.0 };
        let end = self.start_angle + direction * self.sweep_radians;
        let mut lo = at(self.start_angle);
        let mut hi = lo;
        let mut include = |p: [f64; 2]| {
            for axis in 0..2 {
                lo[axis] = lo[axis].min(p[axis]);
                hi[axis] = hi[axis].max(p[axis]);
            }
        };
        include(at(end));
        // Every axis extreme the sweep passes.
        for quarter in 0..4 {
            let axis_angle = f64::from(quarter) * std::f64::consts::FRAC_PI_2;
            let delta = if self.clockwise {
                self.start_angle - axis_angle
            } else {
                axis_angle - self.start_angle
            }
            .rem_euclid(std::f64::consts::TAU);
            if delta <= self.sweep_radians {
                include(at(axis_angle));
            }
        }
        [lo, hi]
    }
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

    #[test]
    fn commanded_arcs_sweep_in_their_direction_and_bound_their_extremes() {
        let arc = CommandedArc::new([1.0, 0.0, 0.2], [0.0, 1.0, 0.2], [0.0, 0.0], false).unwrap();
        assert!((arc.sweep_radians - std::f64::consts::FRAC_PI_2).abs() < 1e-15);
        assert!((arc.length_mm - std::f64::consts::FRAC_PI_2).abs() < 1e-15);
        assert!((arc.angle_to([0.6, 0.8, 0.2]) - 0.8_f64.atan2(0.6)).abs() < 1e-15);
        let [lo, hi] = arc.xy_bounds();
        assert!(lo[0].abs() < 1e-15 && lo[1].abs() < 1e-15 && hi == [1.0, 1.0]);
        // The same ends clockwise go the long way round.
        let long = CommandedArc::new([1.0, 0.0, 0.2], [0.0, 1.0, 0.2], [0.0, 0.0], true).unwrap();
        assert!((long.sweep_radians - 1.5 * std::f64::consts::PI).abs() < 1e-15);
        let [lo, hi] = long.xy_bounds();
        assert_eq!([lo, hi], [[-1.0, -1.0], [1.0, 1.0]]);
        assert!(CommandedArc::new([1.0, 0.0, 0.2], [0.0, 1.0, 0.4], [0.0, 0.0], false).is_err());
    }
}
