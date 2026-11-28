// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Single-nozzle Bambu Lab AMS declarations and independent verification
//! accounting.
//!
//! Format and firmware semantics are derived from read-only inspection of
//! Bambu Studio 02.09.01.70's A1 / A1 mini / P1S / X1C machine profiles and
//! `.gcode.3mf` exports (`M620`/`M621` AMS blocks, `T<slot>` switches,
//! `; FLUSH_START`..`; FLUSH_END` chute purge, bed-rooted prime tower,
//! multi-filament `slice_info.config` / `plate_1.json` / `filament_sequence.json`).
//! Nothing here establishes physical printability or firmware qualification.

use super::*;

/// Maximum AMS slots supported by Bambu single-nozzle systems (4 AMS × 4 slots).
pub const MAX_AMS_SLOTS: usize = 16;

/// Maximum declared chute flush volume per transition (mm³).
pub const MAX_AMS_FLUSH_VOLUME_MM3: f64 = 2000.0;

/// Maximum declared prime-tower priming volume per transition (mm³).
pub const MAX_AMS_PRIME_VOLUME_MM3: f64 = 500.0;

/// One filament loaded in a single-nozzle Bambu AMS slot (0-based index).
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AmsSlot {
    /// Polymer family identifier (e.g. `"pla"`, `"petg"`, `"abs"`, `"asa"`, `"tpu"`).
    pub polymer_family: String,
    /// Optional declared display sRGB colour (`#RRGGBB` in `.gcode.3mf`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub display_srgb: Option<[u8; 3]>,
    pub filament_diameter_mm: f64,
    /// Integral Celsius printing temperature for later layers.
    pub nozzle_temperature_c: f64,
    /// Integral Celsius printing temperature for the first layer.
    pub first_layer_nozzle_temperature_c: f64,
    /// Integral Celsius chute-flush temperature (>= printing temperatures).
    pub flush_temperature_c: f64,
    /// Declared volumetric flow rate during chute flushing (mm³/s).
    pub flush_volumetric_speed_mm3_s: f64,
}

impl AmsSlot {
    pub fn filament_area_mm2(&self) -> f64 {
        std::f64::consts::PI * (self.filament_diameter_mm * 0.5).powi(2)
    }

    /// Chute-flush linear filament feed rate in mm/min.
    pub fn flush_feed_mm_min(&self) -> f64 {
        (self.flush_volumetric_speed_mm3_s / self.filament_area_mm2()) * 60.0
    }

    pub fn validate_declaration(&self) -> Result<(), ManufacturingError> {
        let family = self.polymer_family.trim();
        if family.is_empty()
            || family != self.polymer_family
            || family.len() > 32
            || !family
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_ ".contains(&b))
        {
            return Err(invalid(
                "AMS slot polymer_family must be a short trimmed ASCII identifier",
            ));
        }
        if !self.filament_diameter_mm.is_finite()
            || !(0.5..=5.0).contains(&self.filament_diameter_mm)
        {
            return Err(invalid(
                "AMS slot filament diameter must lie in 0.5..=5.0 mm",
            ));
        }
        for temp in [
            self.nozzle_temperature_c,
            self.first_layer_nozzle_temperature_c,
            self.flush_temperature_c,
        ] {
            if !temp.is_finite() || temp <= 0.0 || temp > 500.0 || temp.fract() != 0.0 {
                return Err(invalid(
                    "AMS slot temperatures must be integral positive Celsius <= 500",
                ));
            }
        }
        if self.flush_temperature_c
            < self
                .nozzle_temperature_c
                .max(self.first_layer_nozzle_temperature_c)
        {
            return Err(invalid(
                "AMS slot flush temperature must be at least its printing temperatures",
            ));
        }
        if !self.flush_volumetric_speed_mm3_s.is_finite()
            || !(0.5..=100.0).contains(&self.flush_volumetric_speed_mm3_s)
        {
            return Err(invalid(
                "AMS slot flush volumetric speed must lie in 0.5..=100.0 mm^3/s",
            ));
        }
        Ok(())
    }

    pub fn validate(
        &self,
        printer: &PrinterSpecification,
        job: &SingleExtruderJob,
    ) -> Result<(), ManufacturingError> {
        self.validate_declaration()?;
        for temp in [
            self.nozzle_temperature_c,
            self.first_layer_nozzle_temperature_c,
            self.flush_temperature_c,
        ] {
            if temp < job.minimum_extrusion_temperature_c || temp > job.maximum_nozzle_temperature_c
            {
                return Err(invalid(
                    "AMS slot temperature lies outside the job's declared nozzle thermal bounds",
                ));
            }
        }
        if self.flush_volumetric_speed_mm3_s > printer.max_volumetric_flow_mm3_s {
            return Err(invalid(
                "AMS slot flush volumetric speed exceeds printer max volumetric flow",
            ));
        }
        Ok(())
    }
}

/// Bed-rooted rectangular prime tower for single-nozzle AMS pressure stabilization.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AmsPrimeTower {
    /// Minimum XY corner on the build plate (mm).
    pub origin_xy_mm: [f64; 2],
    /// Width (X) and depth (Y) of the tower footprint (mm).
    pub size_xy_mm: [f64; 2],
    /// Nominal deposited bead width on the tower (mm).
    pub bead_width_mm: f64,
    /// Minimum priming volume deposited on the tower after each mid-print switch (mm³).
    pub prime_volume_mm3: f64,
    /// Linear deposition speed on the prime tower (mm/s).
    pub print_speed_mm_s: f64,
}

impl AmsPrimeTower {
    pub fn min_xy_mm(&self) -> [f64; 2] {
        self.origin_xy_mm
    }

    pub fn max_xy_mm(&self) -> [f64; 2] {
        [
            self.origin_xy_mm[0] + self.size_xy_mm[0],
            self.origin_xy_mm[1] + self.size_xy_mm[1],
        ]
    }

    pub fn validate_declaration(&self) -> Result<(), ManufacturingError> {
        if self
            .origin_xy_mm
            .iter()
            .chain(&self.size_xy_mm)
            .any(|v| !v.is_finite())
        {
            return Err(invalid("AMS prime tower coordinates must be finite"));
        }
        if self
            .size_xy_mm
            .iter()
            .any(|&s| !(10.0..=200.0).contains(&s))
        {
            return Err(invalid(
                "AMS prime tower width and depth must lie in 10..=200 mm",
            ));
        }
        if !self.bead_width_mm.is_finite() || self.bead_width_mm <= 0.0 {
            return Err(invalid("AMS prime tower bead width must be positive"));
        }
        if self.size_xy_mm[0] < 4.0 * self.bead_width_mm
            || self.size_xy_mm[1] < 4.0 * self.bead_width_mm
        {
            return Err(invalid(
                "AMS prime tower footprint must fit at least four bead widths on each side",
            ));
        }
        if !self.prime_volume_mm3.is_finite()
            || self.prime_volume_mm3 <= 0.0
            || self.prime_volume_mm3 > MAX_AMS_PRIME_VOLUME_MM3
        {
            return Err(invalid(
                "AMS prime tower prime_volume_mm3 must lie in (0, 500] mm^3",
            ));
        }
        if !self.print_speed_mm_s.is_finite() || self.print_speed_mm_s <= 0.0 {
            return Err(invalid("AMS prime tower print speed must be positive"));
        }
        Ok(())
    }

    pub fn validate(&self, printer: &PrinterSpecification) -> Result<(), ManufacturingError> {
        self.validate_declaration()?;
        if self.print_speed_mm_s > printer.max_print_feed_mm_s {
            return Err(invalid(
                "AMS prime tower print speed exceeds printer max print feed",
            ));
        }
        if self.bead_width_mm < printer.deposition_limits.min_bead_width_mm
            || self.bead_width_mm > printer.deposition_limits.max_bead_width_mm
        {
            return Err(invalid(
                "AMS prime tower bead width lies outside printer deposition limits",
            ));
        }
        let min = self.min_xy_mm();
        let max = self.max_xy_mm();
        for axis in 0..2 {
            if min[axis] < printer.build_envelope.min[axis]
                || max[axis] > printer.build_envelope.max[axis]
            {
                return Err(invalid(
                    "AMS prime tower lies outside printer build envelope",
                ));
            }
        }
        for ex in &printer.bed_exclusions {
            if rects_overlap(min, max, ex.min_xy_mm, ex.max_xy_mm) {
                return Err(invalid(
                    "AMS prime tower overlaps a printer bed exclusion region",
                ));
            }
        }
        Ok(())
    }
}

fn rects_overlap(a_min: [f64; 2], a_max: [f64; 2], b_min: [f64; 2], b_max: [f64; 2]) -> bool {
    a_min[0] <= b_max[0] && b_min[0] <= a_max[0] && a_min[1] <= b_max[1] && b_min[1] <= a_max[1]
}

/// Single-nozzle Bambu Lab AMS configuration bound to a `BambuGcode3mf` job.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BambuAms {
    /// Loaded AMS filament slots (`2..=16`). Slot 0 is the primary job material.
    pub slots: Vec<AmsSlot>,
    /// Optional dedicated AMS slot for ordinary support paths (`DepositionKind::Support`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub support_slot: Option<u8>,
    /// Optional dedicated AMS slot for support interface paths (`DepositionKind::SupportInterface`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub support_interface_slot: Option<u8>,
    /// Row-major `N × N` flush volume matrix (mm³): entry `from * N + to` is the
    /// minimum chute flush volume when switching from slot `from` to slot `to`.
    pub flush_volumes_mm3: Vec<f64>,
    /// Optional bed-rooted rectangular prime tower.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub prime_tower: Option<AmsPrimeTower>,
}

impl BambuAms {
    /// Conservative default flush volume (mm³) between two polymer families:
    /// higher purge for dissimilar release pairs (e.g. PLA ↔ PETG) to avoid
    /// cross-polymer delamination, moderate purge for same-polymer changes.
    pub fn default_flush_volume_mm3(from_family: &str, to_family: &str) -> f64 {
        if from_family.eq_ignore_ascii_case(to_family) {
            240.0
        } else {
            500.0
        }
    }

    /// Build a default row-major `N × N` flush volume matrix from slot polymers.
    pub fn default_flush_matrix(slots: &[AmsSlot]) -> Vec<f64> {
        let n = slots.len();
        let mut matrix = vec![0.0; n * n];
        for from in 0..n {
            for to in 0..n {
                if from != to {
                    matrix[from * n + to] = Self::default_flush_volume_mm3(
                        &slots[from].polymer_family,
                        &slots[to].polymer_family,
                    );
                }
            }
        }
        matrix
    }

    /// Deterministic placement for a bed-rooted prime tower clear of bed
    /// exclusions, the prime line, and occupied XY boxes (`margin_mm` clearance).
    pub fn place_prime_tower(
        printer: &PrinterSpecification,
        occupied_xy: &[([f64; 2], [f64; 2])],
        size_xy_mm: [f64; 2],
        bead_width_mm: f64,
        prime_volume_mm3: f64,
        print_speed_mm_s: f64,
        margin_mm: f64,
    ) -> Option<AmsPrimeTower> {
        let margin = margin_mm.max(2.0);
        let env = printer.build_envelope;
        let x_lo = (env.min[0] + margin).ceil();
        let x_hi = (env.max[0] - size_xy_mm[0] - margin).floor();
        let y_lo = (env.min[1] + margin).ceil();
        let y_hi = (env.max[1] - size_xy_mm[1] - margin).floor();
        if x_lo > x_hi || y_lo > y_hi {
            return None;
        }
        // Try rear corners first (matching Bambu Studio's rear-left / rear-right
        // default prime tower placement), then front corners, then mid-edges.
        let x_mid = ((x_lo + x_hi) * 0.5).floor();
        let y_mid = ((y_lo + y_hi) * 0.5).floor();
        let candidates = [
            [x_lo, y_hi],
            [x_hi, y_hi],
            [x_hi, y_lo],
            [x_lo, y_lo],
            [x_mid, y_hi],
            [x_mid, y_lo],
            [x_lo, y_mid],
            [x_hi, y_mid],
        ];
        for origin in candidates {
            let tower = AmsPrimeTower {
                origin_xy_mm: origin,
                size_xy_mm,
                bead_width_mm,
                prime_volume_mm3,
                print_speed_mm_s,
            };
            if tower.validate(printer).is_err() {
                continue;
            }
            let min = [origin[0] - margin, origin[1] - margin];
            let max = [
                origin[0] + size_xy_mm[0] + margin,
                origin[1] + size_xy_mm[1] + margin,
            ];
            if occupied_xy
                .iter()
                .all(|&(occ_min, occ_max)| !rects_overlap(min, max, occ_min, occ_max))
            {
                return Some(tower);
            }
        }
        None
    }

    /// Authoritative AMS slot index for a path's deposition kind.
    pub fn slot_for_kind(&self, kind: DepositionKind) -> u8 {
        match kind {
            DepositionKind::SupportInterface => self
                .support_interface_slot
                .or(self.support_slot)
                .unwrap_or(0),
            DepositionKind::Support => self.support_slot.unwrap_or(0),
            _ => 0,
        }
    }

    /// Declared chute flush volume (mm³) when transitioning `from -> to`.
    pub fn flush_volume_mm3(&self, from: u8, to: u8) -> f64 {
        let n = self.slots.len();
        self.flush_volumes_mm3[usize::from(from) * n + usize::from(to)]
    }

    /// Chute flush temperature (°C): the hotter of the outgoing and incoming
    /// slots' declared flush temperatures.
    pub fn flush_temperature_c(&self, from: u8, to: u8) -> f64 {
        self.slots[usize::from(from)]
            .flush_temperature_c
            .max(self.slots[usize::from(to)].flush_temperature_c)
    }

    /// Required filament length (mm) of incoming slot `to` to flush `from -> to`.
    pub fn flush_length_mm(&self, from: u8, to: u8) -> f64 {
        let volume = self.flush_volume_mm3(from, to);
        if volume <= 0.0 {
            return 0.0;
        }
        volume / self.slots[usize::from(to)].filament_area_mm2()
    }

    pub fn validate_declaration(&self) -> Result<(), ManufacturingError> {
        let n = self.slots.len();
        if !(2..=MAX_AMS_SLOTS).contains(&n) {
            return Err(invalid("Bambu AMS requires between 2 and 16 slots"));
        }
        for slot in &self.slots {
            slot.validate_declaration()?;
        }
        for slot in [self.support_slot, self.support_interface_slot]
            .into_iter()
            .flatten()
        {
            if usize::from(slot) >= n {
                return Err(invalid(
                    "Bambu AMS support slot index exceeds declared slot count",
                ));
            }
        }
        if self.flush_volumes_mm3.len() != n * n {
            return Err(invalid(
                "Bambu AMS flush_volumes_mm3 must be an N x N matrix",
            ));
        }
        for from in 0..n {
            for to in 0..n {
                let v = self.flush_volumes_mm3[from * n + to];
                if !v.is_finite() || v < 0.0 || v > MAX_AMS_FLUSH_VOLUME_MM3 {
                    return Err(invalid(
                        "Bambu AMS flush volume must be finite and within 0..=2000 mm^3",
                    ));
                }
                if from == to && v != 0.0 {
                    return Err(invalid("Bambu AMS flush matrix diagonal must be zero"));
                }
            }
        }
        if let Some(tower) = &self.prime_tower {
            tower.validate_declaration()?;
        }
        Ok(())
    }

    pub fn validate(
        &self,
        printer: &PrinterSpecification,
        job: &SingleExtruderJob,
    ) -> Result<(), ManufacturingError> {
        self.validate_declaration()?;
        for slot in &self.slots {
            slot.validate(printer, job)?;
        }
        // Slot 0 owns the primary job thermal targets so there is one authority.
        let primary = &self.slots[0];
        if primary.nozzle_temperature_c != job.nozzle_temperature_c
            || primary.first_layer_nozzle_temperature_c != job.first_layer_nozzle_temperature_c
        {
            return Err(invalid(
                "Bambu AMS slot 0 nozzle temperatures must match the job's primary temperatures",
            ));
        }
        if let Some(tower) = &self.prime_tower {
            tower.validate(printer)?;
        }
        Ok(())
    }
}

/// Independently decoded per-slot filament accounting for a Bambu AMS job.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AmsSlotAccounting {
    pub slot: u8,
    pub model_filament_length_mm: f64,
    pub support_filament_length_mm: f64,
    pub adhesion_filament_length_mm: f64,
    pub priming_filament_length_mm: f64,
    pub flush_filament_length_mm: f64,
    pub tower_filament_length_mm: f64,
    pub used_for_object: bool,
    pub used_for_support: bool,
}

impl AmsSlotAccounting {
    pub fn total_filament_length_mm(&self) -> f64 {
        self.model_filament_length_mm
            + self.support_filament_length_mm
            + self.adhesion_filament_length_mm
            + self.priming_filament_length_mm
            + self.flush_filament_length_mm
            + self.tower_filament_length_mm
    }

    pub fn validate(&self, slot_count: usize) -> Result<(), ManufacturingError> {
        if usize::from(self.slot) >= slot_count {
            return Err(invalid("AMS verification slot index out of range"));
        }
        if [
            self.model_filament_length_mm,
            self.support_filament_length_mm,
            self.adhesion_filament_length_mm,
            self.priming_filament_length_mm,
            self.flush_filament_length_mm,
            self.tower_filament_length_mm,
        ]
        .iter()
        .any(|v| !v.is_finite() || *v < 0.0)
        {
            return Err(invalid(
                "AMS verification filament lengths must be finite and nonnegative",
            ));
        }
        Ok(())
    }
}

/// Independently verified single-nozzle Bambu AMS execution summary.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AmsVerification {
    pub slots: Vec<AmsSlotAccounting>,
    /// Number of mid-print `M620..M621` filament transitions executed.
    pub change_count: u32,
    /// Ordered 0-based slot sequence loaded by the program (initial load + each switch).
    pub sequence: Vec<u8>,
    pub tower_deposition_segments: u32,
    pub tower_travel_segments: u32,
    pub tower_layers: u32,
    pub total_flush_filament_length_mm: f64,
    pub total_tower_filament_length_mm: f64,
}

impl AmsVerification {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        let n = self.slots.len();
        if !(2..=MAX_AMS_SLOTS).contains(&n) {
            return Err(invalid("AMS verification requires 2..=16 slot records"));
        }
        for (i, slot) in self.slots.iter().enumerate() {
            if usize::from(slot.slot) != i {
                return Err(invalid(
                    "AMS verification slots must be ordered 0..N without gaps",
                ));
            }
            slot.validate(n)?;
        }
        if self.sequence.is_empty()
            || self.sequence.len() != self.change_count as usize + 1
            || self.sequence.iter().any(|&s| usize::from(s) >= n)
            || self.sequence.windows(2).any(|w| w[0] == w[1])
        {
            return Err(invalid(
                "AMS verification sequence must start with initial load and record each change",
            ));
        }
        if !self.total_flush_filament_length_mm.is_finite()
            || self.total_flush_filament_length_mm < 0.0
            || !self.total_tower_filament_length_mm.is_finite()
            || self.total_tower_filament_length_mm < 0.0
            || (self.tower_deposition_segments == 0) != (self.total_tower_filament_length_mm == 0.0)
            || (self.tower_layers == 0) != (self.tower_deposition_segments == 0)
        {
            return Err(invalid(
                "AMS verification flush/tower totals disagree with segment counts",
            ));
        }
        let flush_sum: f64 = self.slots.iter().map(|s| s.flush_filament_length_mm).sum();
        let tower_sum: f64 = self.slots.iter().map(|s| s.tower_filament_length_mm).sum();
        if (flush_sum - self.total_flush_filament_length_mm).abs() > 1e-6 * flush_sum.max(1.0)
            || (tower_sum - self.total_tower_filament_length_mm).abs() > 1e-6 * tower_sum.max(1.0)
        {
            return Err(invalid(
                "AMS verification slot sums disagree with flush/tower totals",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hardware::builtin_catalog;

    fn sample_slots(primary_c: f64, first_c: f64) -> Vec<AmsSlot> {
        vec![
            AmsSlot {
                polymer_family: "PLA".into(),
                display_srgb: Some([0x1F, 0x4E, 0x79]),
                filament_diameter_mm: 1.75,
                nozzle_temperature_c: primary_c,
                first_layer_nozzle_temperature_c: first_c,
                flush_temperature_c: primary_c.max(first_c),
                flush_volumetric_speed_mm3_s: 12.0,
            },
            AmsSlot {
                polymer_family: "PETG".into(),
                display_srgb: Some([0xFF, 0x66, 0x00]),
                filament_diameter_mm: 1.75,
                nozzle_temperature_c: 245.0,
                first_layer_nozzle_temperature_c: 245.0,
                flush_temperature_c: 245.0,
                flush_volumetric_speed_mm3_s: 10.0,
            },
        ]
    }

    #[test]
    fn bambu_ams_validation_lifecycle_and_p1s_exclusion_checks() {
        let catalog = builtin_catalog().unwrap();
        let p1s = catalog.find_setup("bambulab.p1s.04").unwrap();
        let capture = catalog.stock_capture(p1s).unwrap();
        let mut printer = p1s.printer.clone();
        let JobWorkflow::CompleteMarlin { job } = &mut printer.workflow else {
            panic!("complete job");
        };
        let slots = sample_slots(
            job.nozzle_temperature_c,
            job.first_layer_nozzle_temperature_c,
        );
        let flush = BambuAms::default_flush_matrix(&slots);
        assert_eq!(flush, vec![0.0, 500.0, 500.0, 0.0]);
        let tower = BambuAms::place_prime_tower(
            &p1s.printer,
            &[([80.0, 80.0], [110.0, 110.0])],
            [35.0, 35.0],
            0.44,
            20.0,
            100.0,
            5.0,
        )
        .expect("free corner");
        let ams = BambuAms {
            slots,
            support_slot: None,
            support_interface_slot: Some(1),
            flush_volumes_mm3: flush,
            prime_tower: Some(tower),
        };
        job.enable_bambu_ams(ams.clone()).unwrap();
        assert!(
            job.lifecycle
                .start
                .contains(&LifecycleStep::AmsLoad { slot: 0 })
        );
        assert!(job.lifecycle.end.contains(&LifecycleStep::AmsUnload));
        printer.validate().unwrap();

        // Removing AmsUnload fails lifecycle validation.
        let mut missing_unload = printer.clone();
        if let JobWorkflow::CompleteMarlin { job } = &mut missing_unload.workflow {
            job.lifecycle
                .end
                .retain(|s| !matches!(s, LifecycleStep::AmsUnload));
        }
        assert!(missing_unload.validate().is_err());

        // Non-zero flush diagonal is refused.
        let mut bad_diag = ams.clone();
        bad_diag.flush_volumes_mm3[0] = 10.0;
        assert!(bad_diag.validate_declaration().is_err());

        // Prime tower overlapping P1S cutter-corner bed exclusion [0..18, 0..28] is refused.
        let mut bad_tower = ams.clone();
        bad_tower.prime_tower.as_mut().unwrap().origin_xy_mm = [2.0, 2.0];
        let JobWorkflow::CompleteMarlin { job } = &printer.workflow else {
            unreachable!()
        };
        assert!(bad_tower.validate(&p1s.printer, job).is_err());

        // Capture validation reconciles per-job BambuAms while keeping physical limits strict.
        let mut intent = ManufacturingIntent {
            hardware: Some(std::sync::Arc::new(capture.clone())),
            material_profile: None,
            printer: printer.clone(),
            recipe: PlanarPrintRecipe {
                id: "ams-test".into(),
                revision: "1".into(),
                material_id: "pla".into(),
                material_revision: "1".into(),
                filament_diameter_mm: 1.75,
                layer_height_mm: 0.2,
                layer_height_policy: LayerHeightPolicy::Preferred,
                bead_width_mm: 0.44,
                sparse_infill: SparseInfillPolicy::Fine,
                perimeter_count: 2,
                seam_policy: SeamPolicy::Nearest,
                rectilinear_axis: RectilinearAxisPolicy::AlternatingXY,
                top_skin_axis: RectilinearAxisPolicy::AlternatingXY,
                infill_fraction: 0.15,
                top_skin_thickness_mm: 0.8,
                bottom_skin_thickness_mm: 0.6,
                print_speed_mm_s: 100.0,
                travel_speed_mm_s: printer.max_travel_feed_mm_s,
                flow_multiplier: 1.0,
                process: ProcessPolicy {
                    first_layer_speed_factor: 0.5,
                    first_layer_flow_factor: 1.0,
                    fan_first_layers: 1,
                    fan_after_first_layers: 255,
                    bridge_speed_factor: 0.5,
                    bridge_flow_factor: 1.0,
                    bridge_fan_pwm: 255,
                    max_bridge_span_mm: 10.0,
                    retraction: RetractionPolicy::Disabled,
                    minimum_layer_time_s: 0.0,
                    minimum_print_speed_mm_s: 10.0,
                    fan_short_layer_time_s: 0.0,
                    fan_short_layer_pwm: 255,
                },
                supports: SupportPolicy::Disabled,
                role_speeds: None,
                first_layer_height_mm: None,
                skirt: None,
                brim: None,
                road_tolerances: RoadTolerances::FDM,
            },
        };
        capture.validate_intent(&intent).unwrap();
        intent.printer.max_print_feed_mm_s -= 1.0;
        assert!(capture.validate_intent(&intent).is_err());
    }
}
