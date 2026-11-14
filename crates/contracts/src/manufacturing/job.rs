// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;

#[expect(
    clippy::large_enum_variant,
    reason = "A bounded inert job selection stays inline; no per-intent heap indirection"
)]
/// These are explicit software workflows, not compatibility modes.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum JobWorkflow {
    DiagnosticBodyOnly,
    CompleteMarlin { job: SingleExtruderJob },
    FixedDualMarlin { job: FixedDualJob },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HomingCoordinateContract {
    /// Native Cartesian XYZ after homing, without workspace/home offsets. A
    /// firmware bed mesh, when the lifecycle activates one, corrects Z below
    /// command space and is reported, not modelled.
    MarlinNativeCartesian,
}

/// Declared provenance is not measured qualification. Hashes identify externally
/// supplied firmware configuration and calibration records, not engine approval.
/// The printer's output dialect renders the declared `lifecycle`; temperatures
/// below are integral Celsius targets despite their f64 wire type.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SingleExtruderJob {
    pub machine_id: String,
    pub firmware_version: String,
    pub configuration_hash: SourceHash,
    pub calibration_record_hash: SourceHash,
    pub homing_contract: HomingCoordinateContract,
    pub home_position_mm: [f64; 3],
    pub nozzle_temperature_c: f64,
    pub bed_temperature_c: f64,
    pub minimum_extrusion_temperature_c: f64,
    pub maximum_nozzle_temperature_c: f64,
    pub maximum_bed_temperature_c: f64,
    /// Priming is excluded from conservative all-layer object XY ownership,
    /// independent of Z, protected holes or gaps; plan admission checks rounding.
    pub prime_start_mm: [f64; 3],
    pub prime_end_mm: [f64; 3],
    pub prime_filament_length_mm: f64,
    pub prime_speed_mm_s: f64,
    /// XY park position. The park lifts vertically from the last position to
    /// the highest emitted Z plus `park_clearance_mm` (capped at the envelope
    /// top, and strictly above the last position), then moves here at that Z.
    pub park_xy_mm: [f64; 2],
    pub park_clearance_mm: f64,
    pub lifecycle: FirmwareLifecycle,
}
impl SingleExtruderJob {
    pub fn validate_declaration(&self) -> Result<(), ManufacturingError> {
        label(&self.machine_id)?;
        // Names the declared firmware; the program dialect is the printer's
        // explicit Marlin 2 single-extruder output dialect.
        label(&self.firmware_version)?;
        positive(&[
            self.nozzle_temperature_c,
            self.bed_temperature_c,
            self.minimum_extrusion_temperature_c,
            self.maximum_nozzle_temperature_c,
            self.maximum_bed_temperature_c,
            self.prime_filament_length_mm,
            self.prime_speed_mm_s,
            self.park_clearance_mm,
        ])?;
        if [
            self.nozzle_temperature_c,
            self.bed_temperature_c,
            self.minimum_extrusion_temperature_c,
            self.maximum_nozzle_temperature_c,
            self.maximum_bed_temperature_c,
        ]
        .iter()
        .any(|temperature| temperature.fract() != 0.0)
        {
            return Err(invalid(
                "Marlin temperatures must be integral Celsius degrees",
            ));
        }
        let prime_feed = self.prime_speed_mm_s * 60.0;
        if !prime_feed.is_finite() || prime_feed < 0.000001 {
            return Err(invalid(
                "priming feed cannot be represented at declared precision",
            ));
        }
        if self.nozzle_temperature_c < self.minimum_extrusion_temperature_c
            || self.nozzle_temperature_c > self.maximum_nozzle_temperature_c
            || self.bed_temperature_c > self.maximum_bed_temperature_c
            || self.maximum_nozzle_temperature_c > 500.0
            || self.maximum_bed_temperature_c > 200.0
            || self.prime_filament_length_mm > 100.0
        {
            return Err(invalid(
                "job thermal, priming or feed declaration exceeds bounded limits",
            ));
        }
        for point in [
            self.home_position_mm,
            self.prime_start_mm,
            self.prime_end_mm,
            [self.park_xy_mm[0], self.park_xy_mm[1], 0.0],
        ] {
            if point.iter().any(|value| !value.is_finite()) {
                return Err(invalid(
                    "lifecycle command endpoint exceeds command-space envelope",
                ));
            }
        }
        let length = (self.prime_end_mm[0] - self.prime_start_mm[0])
            .hypot(self.prime_end_mm[1] - self.prime_start_mm[1])
            .hypot(self.prime_end_mm[2] - self.prime_start_mm[2]);
        if length <= 0.0 || !length.is_finite() || self.park_clearance_mm > 1000.0 {
            return Err(invalid(
                "priming requires a nonzero bounded line and bounded park clearance",
            ));
        }
        Ok(())
    }
    pub fn validate(&self, printer: &PrinterSpecification) -> Result<(), ManufacturingError> {
        self.validate_declaration()?;
        if self.prime_speed_mm_s > printer.max_print_feed_mm_s {
            return Err(invalid("priming feed exceeds printer limit"));
        }
        let park = [
            self.park_xy_mm[0],
            self.park_xy_mm[1],
            printer.build_envelope.max[2],
        ];
        for point in [
            self.home_position_mm,
            self.prime_start_mm,
            self.prime_end_mm,
            park,
        ] {
            if (0..3).any(|a| {
                point[a] < printer.build_envelope.min[a] || point[a] > printer.build_envelope.max[a]
            }) {
                return Err(invalid(
                    "lifecycle command endpoint exceeds command-space envelope",
                ));
            }
        }
        if printer.build_envelope.max[2] <= self.prime_start_mm[2].max(self.prime_end_mm[2]) {
            return Err(invalid("park needs clearance above the prime line"));
        }
        self.lifecycle
            .validate(printer.capabilities.output_dialect, self)?;
        for (_, block) in self.lifecycle.blocks() {
            block.validate(printer.capabilities.output_dialect)?;
        }
        Ok(())
    }
}
