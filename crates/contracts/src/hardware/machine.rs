// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Machine descriptions: a small set of sourced machine facts from which a
//! complete single-extruder setup is derived. Facts are declarations with a
//! recorded origin, never physical qualification.
//!
//! Derivation rules (documented in `docs/protocol/hardware.md`):
//! - Build envelope `[0,0,0]..[bed_x, bed_y, max_z]`; bed is envelope minimum Z.
//! - Deposition widths `[min_ratio, max_ratio] × nozzle`, heights from the Z
//!   motion step up to `height_ratio × nozzle`.
//! - Prime line along the back edge, `margin = min(10, 5% of the smaller bed
//!   side)` from the edges, `min(20, bed_x − 2·margin)` long, at half the
//!   nozzle diameter, purging 1.8× the nominal bead volume.
//! - Park at the back-left margin after a vertical lift of 10 mm above the
//!   highest emitted Z (capped at the envelope top).
//! - Output dialect and lifecycle from the firmware family's table
//!   (`derive_lifecycle`), levelling from the declared capability, with any
//!   operator start/end block inserted and the typed steps it covers omitted.
//! - Print feed bounds every extruding command, travel feed every other motion;
//!   volumetric flow is the declared hotend ceiling. Acceleration is recorded
//!   for time estimation.
use super::{HardwareModelDefinition, HardwareSetupDefinition, PlateDefinition, text};
use crate::{
    geometry::{AabbMm, RigidPoseMm, SourceHash},
    manufacturing::{
        DepositionLimits, ExtruderDrive, ExtrusionEffect, FirmwareLifecycle,
        HomingCoordinateContract, JobWorkflow, LevellingEffect, LifecycleBlock, LifecycleStep,
        MANUFACTURING_SCHEMA_VERSION, MachineComponent, MachineComponentRole,
        MachineComponentShape, ModeEffect, MotionResolution, MotionSpace, OutputDialect,
        PrinterCapabilities, PrinterCoordinateFrame, PrinterSpecification, SingleExtruderJob,
        ThermalEffect,
    },
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use ts_rs::TS;

/// Firmware family recorded for the machine; it selects the output dialect
/// and the lifecycle table.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FirmwareFamily {
    Marlin2,
    PrusaBuddy,
    Klipper,
    BambuLab,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MachineKinematics {
    BedSlinger,
    CoreXy,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BedLevelling {
    /// No firmware mesh; the lifecycle commands no levelling.
    Manual,
    /// Probe a mesh every print after homing.
    ProbeMesh,
    /// Activate a mesh stored in the firmware.
    StoredMesh,
}

/// Numeric machine facts. Every complete description carries each key once.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum MachineLimit {
    BedXMm,
    BedYMm,
    MaxZMm,
    NozzleDiameterMm,
    FilamentDiameterMm,
    MaxPrintFeedMmS,
    MaxTravelFeedMmS,
    MaxAccelerationMmS2,
    MaxVolumetricFlowMm3S,
    MaxNozzleTemperatureC,
    MinExtrusionTemperatureC,
    MaxBedTemperatureC,
    MinBeadWidthPerNozzle,
    MaxBeadWidthPerNozzle,
    MaxBeadHeightPerNozzle,
    XyResolutionMm,
    ZResolutionMm,
    ToolheadRadiusMm,
    ToolheadHeightMm,
    FirstLayerSpeedMmS,
    OuterWallSpeedMmS,
    InnerWallSpeedMmS,
    SparseInfillSpeedMmS,
    SolidInfillSpeedMmS,
    TopSkinSpeedMmS,
    SupportSpeedMmS,
    BridgeSpeedMmS,
    FirstLayerInfillSpeedMmS,
}
impl MachineLimit {
    pub const ALL: [Self; 28] = [
        Self::BedXMm,
        Self::BedYMm,
        Self::MaxZMm,
        Self::NozzleDiameterMm,
        Self::FilamentDiameterMm,
        Self::MaxPrintFeedMmS,
        Self::MaxTravelFeedMmS,
        Self::MaxAccelerationMmS2,
        Self::MaxVolumetricFlowMm3S,
        Self::MaxNozzleTemperatureC,
        Self::MinExtrusionTemperatureC,
        Self::MaxBedTemperatureC,
        Self::MinBeadWidthPerNozzle,
        Self::MaxBeadWidthPerNozzle,
        Self::MaxBeadHeightPerNozzle,
        Self::XyResolutionMm,
        Self::ZResolutionMm,
        Self::ToolheadRadiusMm,
        Self::ToolheadHeightMm,
        Self::FirstLayerSpeedMmS,
        Self::OuterWallSpeedMmS,
        Self::InnerWallSpeedMmS,
        Self::SparseInfillSpeedMmS,
        Self::SolidInfillSpeedMmS,
        Self::TopSkinSpeedMmS,
        Self::SupportSpeedMmS,
        Self::BridgeSpeedMmS,
        Self::FirstLayerInfillSpeedMmS,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::BedXMm => "Bed X (mm)",
            Self::BedYMm => "Bed Y (mm)",
            Self::MaxZMm => "Maximum Z (mm)",
            Self::NozzleDiameterMm => "Nozzle diameter (mm)",
            Self::FilamentDiameterMm => "Filament diameter (mm)",
            Self::MaxPrintFeedMmS => "Maximum print feed (mm/s)",
            Self::MaxTravelFeedMmS => "Maximum travel feed (mm/s)",
            Self::MaxAccelerationMmS2 => "Maximum acceleration (mm/s²)",
            Self::MaxVolumetricFlowMm3S => "Maximum volumetric flow (mm³/s)",
            Self::MaxNozzleTemperatureC => "Maximum nozzle temperature (°C)",
            Self::MinExtrusionTemperatureC => "Minimum extrusion temperature (°C)",
            Self::MaxBedTemperatureC => "Maximum bed temperature (°C)",
            Self::MinBeadWidthPerNozzle => "Minimum bead width ÷ nozzle",
            Self::MaxBeadWidthPerNozzle => "Maximum bead width ÷ nozzle",
            Self::MaxBeadHeightPerNozzle => "Maximum bead height ÷ nozzle",
            Self::XyResolutionMm => "XY motion step (mm)",
            Self::ZResolutionMm => "Z motion step (mm)",
            Self::ToolheadRadiusMm => "Toolhead clearance radius (mm)",
            Self::ToolheadHeightMm => "Toolhead clearance height (mm)",
            Self::FirstLayerSpeedMmS => "First-layer speed (mm/s)",
            Self::OuterWallSpeedMmS => "Outer wall speed (mm/s)",
            Self::InnerWallSpeedMmS => "Inner wall speed (mm/s)",
            Self::SparseInfillSpeedMmS => "Sparse infill speed (mm/s)",
            Self::SolidInfillSpeedMmS => "Solid infill speed (mm/s)",
            Self::TopSkinSpeedMmS => "Top skin speed (mm/s)",
            Self::SupportSpeedMmS => "Support speed (mm/s)",
            Self::BridgeSpeedMmS => "Bridge speed (mm/s)",
            Self::FirstLayerInfillSpeedMmS => "First-layer infill speed (mm/s)",
        }
    }
    fn admits(self, v: f64) -> bool {
        let range = match self {
            Self::BedXMm | Self::BedYMm | Self::MaxZMm => 10.0..=5000.0,
            Self::NozzleDiameterMm => 0.1..=3.0,
            Self::FilamentDiameterMm => 1.0..=3.5,
            Self::MaxPrintFeedMmS
            | Self::MaxTravelFeedMmS
            | Self::FirstLayerSpeedMmS
            | Self::OuterWallSpeedMmS
            | Self::InnerWallSpeedMmS
            | Self::SparseInfillSpeedMmS
            | Self::SolidInfillSpeedMmS
            | Self::TopSkinSpeedMmS
            | Self::SupportSpeedMmS
            | Self::BridgeSpeedMmS
            | Self::FirstLayerInfillSpeedMmS => 1.0..=5000.0,
            Self::MaxAccelerationMmS2 => 1.0..=500_000.0,
            Self::MaxVolumetricFlowMm3S => 0.1..=1000.0,
            Self::MaxNozzleTemperatureC | Self::MinExtrusionTemperatureC => 1.0..=500.0,
            // Zero declares a machine without a heated bed.
            Self::MaxBedTemperatureC => 0.0..=200.0,
            Self::MinBeadWidthPerNozzle | Self::MaxBeadWidthPerNozzle => 0.25..=4.0,
            Self::MaxBeadHeightPerNozzle => 0.05..=1.0,
            Self::XyResolutionMm | Self::ZResolutionMm => {
                0.000_001..=crate::manufacturing::MAX_MOTION_RESOLUTION_MM
            }
            Self::ToolheadRadiusMm | Self::ToolheadHeightMm => 0.1..=1000.0,
        };
        let integral = matches!(
            self,
            Self::MaxNozzleTemperatureC | Self::MinExtrusionTemperatureC | Self::MaxBedTemperatureC
        );
        v.is_finite() && range.contains(&v) && (!integral || v.fract() == 0.0)
    }
}

/// Where a declared fact comes from.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FactSource {
    /// A manufacturer's published specification (URL or document reference).
    Published { reference: String },
    /// A family/generic default, sized for modern machines, declared by `family`.
    FamilyDefault { family: String },
    /// An operator edit in a custom setup.
    OperatorDeclared,
}
impl FactSource {
    fn validate(&self) -> Result<(), String> {
        match self {
            Self::Published { reference } => text(reference),
            Self::FamilyDefault { family } => text(family),
            Self::OperatorDeclared => Ok(()),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SourcedLimit {
    pub key: MachineLimit,
    pub value: f64,
    pub source: FactSource,
}

fn required_option<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(d)
}

/// One inheritance layer of authoring data: a family/generic parent or a
/// concrete printer that overrides what it knows.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MachineLayer {
    pub id: String,
    #[serde(deserialize_with = "required_option")]
    pub parent: Option<String>,
    #[serde(deserialize_with = "required_option")]
    pub firmware: Option<FirmwareFamily>,
    #[serde(deserialize_with = "required_option")]
    pub kinematics: Option<MachineKinematics>,
    #[serde(deserialize_with = "required_option")]
    pub levelling: Option<BedLevelling>,
    #[serde(deserialize_with = "required_option")]
    pub heated_bed: Option<bool>,
    #[serde(deserialize_with = "required_option")]
    pub arc_moves: Option<bool>,
    #[serde(deserialize_with = "required_option")]
    pub extruder_drive: Option<ExtruderDrive>,
    /// Model identifier the firmware checks (Prusa `M862.3`), if any.
    #[serde(deserialize_with = "required_option")]
    pub printer_model: Option<String>,
    pub limits: Vec<SourcedLimit>,
}

/// A complete, resolved machine description: the single authority from which
/// a setup's printer specification is derived.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MachineDescription {
    pub id: String,
    /// Inheritance chain, root first, ending with `id`.
    pub lineage: Vec<String>,
    pub firmware: FirmwareFamily,
    pub kinematics: MachineKinematics,
    pub levelling: BedLevelling,
    pub heated_bed: bool,
    pub arc_moves: bool,
    pub extruder_drive: ExtruderDrive,
    #[serde(deserialize_with = "required_option")]
    pub printer_model: Option<String>,
    /// Exactly one entry per `MachineLimit`, in `MachineLimit::ALL` order.
    pub limits: Vec<SourcedLimit>,
    /// Operator start/end blocks (for example Klipper `START_PRINT` and
    /// `END_PRINT` macros) with declared effects; null uses the typed table.
    #[serde(deserialize_with = "required_option")]
    pub start_block: Option<LifecycleBlock>,
    #[serde(deserialize_with = "required_option")]
    pub end_block: Option<LifecycleBlock>,
}

const MAX_LAYERS: usize = 64;

impl MachineDescription {
    pub fn validate(&self) -> Result<(), String> {
        text(&self.id)?;
        if let Some(model) = &self.printer_model {
            text(model)?;
        }
        for block in [&self.start_block, &self.end_block].into_iter().flatten() {
            block.validate(self.dialect()).map_err(|e| e.to_string())?;
        }
        if self.lineage.is_empty()
            || self.lineage.len() > MAX_LAYERS
            || self.lineage.last() != Some(&self.id)
        {
            return Err("machine lineage must end with the description identity".into());
        }
        for l in &self.lineage {
            text(l)?;
        }
        if self.limits.len() != MachineLimit::ALL.len()
            || self
                .limits
                .iter()
                .zip(MachineLimit::ALL)
                .any(|(l, k)| l.key != k)
        {
            return Err("machine description must declare every limit once, in order".into());
        }
        for l in &self.limits {
            l.source.validate()?;
            if !l.key.admits(l.value) {
                return Err(format!(
                    "machine {} is outside its admitted range",
                    l.key.label()
                ));
            }
        }
        let v = |k| self.limit(k);
        if (self.heated_bed != (v(MachineLimit::MaxBedTemperatureC) > 0.0))
            || v(MachineLimit::MinBeadWidthPerNozzle) > v(MachineLimit::MaxBeadWidthPerNozzle)
            || v(MachineLimit::MaxBeadHeightPerNozzle) > v(MachineLimit::MaxBeadWidthPerNozzle)
            || v(MachineLimit::MinExtrusionTemperatureC) > v(MachineLimit::MaxNozzleTemperatureC)
            || v(MachineLimit::ZResolutionMm)
                > v(MachineLimit::MaxBeadHeightPerNozzle) * v(MachineLimit::NozzleDiameterMm)
        {
            return Err("machine deposition or thermal facts are inconsistent".into());
        }
        Ok(())
    }
    pub fn limit(&self, key: MachineLimit) -> f64 {
        self.limits
            .iter()
            .find(|l| l.key == key)
            .map_or(f64::NAN, |l| l.value)
    }
    pub fn source(&self, key: MachineLimit) -> Option<&FactSource> {
        self.limits.iter().find(|l| l.key == key).map(|l| &l.source)
    }
    /// An operator edit: the value changes and its origin becomes operator-declared.
    pub fn declare(&mut self, key: MachineLimit, value: f64) {
        if let Some(l) = self.limits.iter_mut().find(|l| l.key == key)
            && l.value != value
        {
            l.value = value;
            l.source = FactSource::OperatorDeclared;
        }
    }
    pub fn hash(&self) -> Result<SourceHash, String> {
        Ok(SourceHash::from_bytes(
            &serde_json::to_vec(self).map_err(|e| e.to_string())?,
        ))
    }

    /// Resolve `leaf` through its parents; child facts override parent facts.
    pub fn resolve(layers: &[MachineLayer], leaf: &str) -> Result<Self, String> {
        let by_id: BTreeMap<&str, &MachineLayer> =
            layers.iter().map(|l| (l.id.as_str(), l)).collect();
        if by_id.len() != layers.len() || layers.len() > MAX_LAYERS {
            return Err("duplicate or too many machine layers".into());
        }
        let mut chain = Vec::new();
        let mut next = Some(leaf);
        while let Some(id) = next {
            let layer = by_id
                .get(id)
                .ok_or_else(|| format!("machine layer {id} is missing"))?;
            if chain.iter().any(|l: &&MachineLayer| l.id == id) || chain.len() >= MAX_LAYERS {
                return Err("machine layer inheritance forms a cycle".into());
            }
            chain.push(*layer);
            next = layer.parent.as_deref();
        }
        chain.reverse();
        let mut limits = BTreeMap::new();
        let (mut firmware, mut kinematics, mut levelling, mut heated, mut arcs) =
            (None, None, None, None, None);
        let mut drive = None;
        let mut printer_model = None;
        for layer in &chain {
            text(&layer.id)?;
            let mut declared = std::collections::BTreeSet::new();
            for l in &layer.limits {
                if let FactSource::FamilyDefault { family } = &l.source
                    && family != &layer.id
                {
                    return Err("a family default must be declared by its own family layer".into());
                }
                if !declared.insert(l.key) {
                    return Err("machine layer declares a limit twice".into());
                }
                limits.insert(l.key, l.clone());
            }
            firmware = layer.firmware.or(firmware);
            kinematics = layer.kinematics.or(kinematics);
            levelling = layer.levelling.or(levelling);
            heated = layer.heated_bed.or(heated);
            arcs = layer.arc_moves.or(arcs);
            drive = layer.extruder_drive.or(drive);
            printer_model = layer.printer_model.clone().or(printer_model);
        }
        let missing = |what: &str| format!("machine {leaf} does not declare {what}");
        let limits = MachineLimit::ALL
            .iter()
            .map(|k| limits.remove(k).ok_or_else(|| missing(k.label())))
            .collect::<Result<Vec<_>, _>>()?;
        let d = Self {
            id: leaf.into(),
            lineage: chain.iter().map(|l| l.id.clone()).collect(),
            firmware: firmware.ok_or_else(|| missing("a firmware family"))?,
            kinematics: kinematics.ok_or_else(|| missing("kinematics"))?,
            levelling: levelling.ok_or_else(|| missing("bed levelling"))?,
            heated_bed: heated.ok_or_else(|| missing("whether the bed is heated"))?,
            arc_moves: arcs.ok_or_else(|| missing("arc move support"))?,
            extruder_drive: drive.ok_or_else(|| missing("its extruder drive"))?,
            printer_model,
            limits,
            start_block: None,
            end_block: None,
        };
        d.validate()?;
        Ok(d)
    }

    /// The machine's per-role deposition speeds, each within its print feed.
    pub fn role_speeds(&self) -> crate::manufacturing::RoleSpeeds {
        let cap = self.limit(MachineLimit::MaxPrintFeedMmS);
        let v = |k| self.limit(k).min(cap);
        crate::manufacturing::RoleSpeeds {
            first_layer_mm_s: v(MachineLimit::FirstLayerSpeedMmS),
            outer_wall_mm_s: v(MachineLimit::OuterWallSpeedMmS),
            inner_wall_mm_s: v(MachineLimit::InnerWallSpeedMmS),
            sparse_infill_mm_s: v(MachineLimit::SparseInfillSpeedMmS),
            solid_infill_mm_s: v(MachineLimit::SolidInfillSpeedMmS),
            top_skin_mm_s: v(MachineLimit::TopSkinSpeedMmS),
            support_mm_s: v(MachineLimit::SupportSpeedMmS),
            bridge_mm_s: v(MachineLimit::BridgeSpeedMmS),
            first_layer_infill_mm_s: v(MachineLimit::FirstLayerInfillSpeedMmS),
        }
    }
    pub fn build_envelope(&self) -> AabbMm {
        AabbMm {
            min: [0.0; 3],
            max: [
                self.limit(MachineLimit::BedXMm),
                self.limit(MachineLimit::BedYMm),
                self.limit(MachineLimit::MaxZMm),
            ],
        }
    }

    /// Derive the complete printer specification for setup `id`/`name`.
    pub fn derive_printer(&self, id: &str, name: &str) -> Result<PrinterSpecification, String> {
        self.validate()?;
        let v = |k| self.limit(k);
        let nozzle = v(MachineLimit::NozzleDiameterMm);
        let envelope = self.build_envelope();
        let [bx, by, _] = envelope.max;
        let margin = r6((0.05 * bx.min(by)).min(10.0));
        let prime_length = r6((bx - 2.0 * margin).min(20.0));
        let prime_z = r6(0.5 * nozzle);
        let filament = v(MachineLimit::FilamentDiameterMm);
        let purge_volume = 1.8 * (1.25 * nozzle) * prime_z * prime_length;
        let prime_filament =
            (purge_volume / (std::f64::consts::PI * filament * filament / 4.0) * 1000.0).round()
                / 1000.0;
        let max_nozzle = v(MachineLimit::MaxNozzleTemperatureC);
        let min_extrusion = v(MachineLimit::MinExtrusionTemperatureC);
        let max_bed = v(MachineLimit::MaxBedTemperatureC);
        let description = self.hash()?;
        let identity = |p: [f64; 3]| RigidPoseMm {
            translation_mm: p,
            rotation_xyzw: [0.0, 0.0, 0.0, 1.0],
        };
        let printer = PrinterSpecification {
            schema_version: MANUFACTURING_SCHEMA_VERSION,
            id: id.into(),
            revision: description.as_str().into(),
            name: name.into(),
            coordinate_frame: PrinterCoordinateFrame::RightHandedMillimetres,
            build_envelope: envelope,
            components: vec![
                MachineComponent {
                    id: "bed".into(),
                    role: MachineComponentRole::Bed,
                    pose: identity([0.0; 3]),
                    shape: MachineComponentShape::Box {
                        bounds_mm: AabbMm {
                            min: [0.0, 0.0, -4.0],
                            max: [bx, by, 0.0],
                        },
                    },
                },
                MachineComponent {
                    id: "toolhead-envelope".into(),
                    role: MachineComponentRole::Toolhead,
                    pose: identity([0.0; 3]),
                    shape: MachineComponentShape::Cylinder {
                        radius_mm: v(MachineLimit::ToolheadRadiusMm),
                        height_mm: v(MachineLimit::ToolheadHeightMm),
                    },
                },
            ],
            capabilities: PrinterCapabilities {
                motion_space: MotionSpace::CartesianXyz,
                extruders: 1,
                output_dialect: self.dialect(),
                generated_supports: true,
                arc_moves: self.arc_moves,
            },
            workflow: JobWorkflow::CompleteMarlin {
                job: SingleExtruderJob {
                    machine_id: id.into(),
                    firmware_version: self.firmware_label().into(),
                    configuration_hash: description.clone(),
                    calibration_record_hash: SourceHash::from_bytes(
                        format!(
                            "spiling.uncalibrated-machine-description\n{}",
                            description.as_str()
                        )
                        .as_bytes(),
                    ),
                    homing_contract: HomingCoordinateContract::MarlinNativeCartesian,
                    home_position_mm: [0.0; 3],
                    // Generic starting targets; a captured material profile owns
                    // the operating temperatures. An unheated machine's bed is off.
                    nozzle_temperature_c: 200f64.clamp(min_extrusion, max_nozzle),
                    bed_temperature_c: 60f64.min(max_bed),
                    first_layer_nozzle_temperature_c: 200f64.clamp(min_extrusion, max_nozzle),
                    first_layer_bed_temperature_c: 60f64.min(max_bed),
                    timing_model: Some(crate::manufacturing::PrintTimeModel {
                        acceleration_mm_s2: v(MachineLimit::MaxAccelerationMmS2),
                        junction_deviation_mm: MARLIN_JUNCTION_DEVIATION_MM,
                    }),
                    minimum_extrusion_temperature_c: min_extrusion,
                    maximum_nozzle_temperature_c: max_nozzle,
                    maximum_bed_temperature_c: max_bed,
                    prime_start_mm: [margin, r6(by - margin), prime_z],
                    prime_end_mm: [r6(margin + prime_length), r6(by - margin), prime_z],
                    prime_filament_length_mm: prime_filament,
                    prime_speed_mm_s: 10f64.min(v(MachineLimit::MaxPrintFeedMmS)),
                    park_xy_mm: [margin, r6(by - margin)],
                    park_clearance_mm: 10.0,
                    lifecycle: self.derive_lifecycle(),
                },
            },
            nozzle_diameter_mm: nozzle,
            motion_resolution: MotionResolution {
                xy_mm: v(MachineLimit::XyResolutionMm),
                z_mm: v(MachineLimit::ZResolutionMm),
            },
            deposition_limits: DepositionLimits {
                min_bead_width_mm: r6(v(MachineLimit::MinBeadWidthPerNozzle) * nozzle),
                max_bead_width_mm: r6(v(MachineLimit::MaxBeadWidthPerNozzle) * nozzle),
                min_bead_height_mm: v(MachineLimit::ZResolutionMm),
                max_bead_height_mm: r6(v(MachineLimit::MaxBeadHeightPerNozzle) * nozzle),
            },
            max_print_feed_mm_s: v(MachineLimit::MaxPrintFeedMmS),
            max_travel_feed_mm_s: v(MachineLimit::MaxTravelFeedMmS),
            max_volumetric_flow_mm3_s: v(MachineLimit::MaxVolumetricFlowMm3S),
        };
        printer.validate().map_err(|e| e.to_string())?;
        Ok(printer)
    }
    /// The stock build sheet for this machine: usable over the whole bed.
    pub fn derive_plate(&self, model: &HardwareModelDefinition) -> PlateDefinition {
        let usable = self.build_envelope();
        PlateDefinition {
            id: format!("{}.plate", self.id),
            label: format!("{} stock sheet", model.model),
            mounting_interface: model.mounting_interface.clone(),
            thickness_mm: 1.0,
            surface_z_mm: 0.0,
            source: "Derived from the machine description bed".into(),
            faces: vec![super::PlateFaceDefinition {
                key: "stock".into(),
                label: "Stock face".into(),
                surface: "declared stock surface".into(),
                usable_mm: usable,
                keepouts_mm: vec![],
                max_bed_celsius: self.limit(MachineLimit::MaxBedTemperatureC),
            }],
        }
    }
    fn firmware_label(&self) -> &'static str {
        match self.firmware {
            FirmwareFamily::Marlin2 => "marlin2",
            FirmwareFamily::PrusaBuddy => "prusa-buddy",
            FirmwareFamily::Klipper => "klipper",
            FirmwareFamily::BambuLab => "bambu-lab (.gcode.3mf delivery not yet supported)",
        }
    }
    pub fn dialect(&self) -> OutputDialect {
        match self.firmware {
            FirmwareFamily::Marlin2 => OutputDialect::Marlin2SingleExtruderGcodeV1,
            FirmwareFamily::PrusaBuddy => OutputDialect::PrusaMarlinSingleExtruderGcodeV1,
            FirmwareFamily::Klipper => OutputDialect::KlipperSingleExtruderGcodeV1,
            FirmwareFamily::BambuLab => OutputDialect::BambuLabGcode3mfV1,
        }
    }

    /// The firmware family's lifecycle table. Start: feed/flow/fan resets
    /// (plus `M200 D0` on Marlin, nozzle/model checks on Prusa), the operator
    /// start block, heat the bed and a non-oozing standby nozzle, home, wait
    /// for the bed, level per capability, wait for the print nozzle, prime.
    /// End: barrier, lift and park, barrier, the operator end block, heaters,
    /// fan and motors off. A typed step the operator block declares it covers
    /// is omitted. References: Creality `*fastabl*` and Prusa MK4S start
    /// G-code in PrusaSlicer's bundled profiles; Klipper
    /// <https://www.klipper3d.org/Slicers.html>.
    pub fn derive_lifecycle(&self) -> FirmwareLifecycle {
        use LifecycleStep as S;
        let prusa = self.firmware == FirmwareFamily::PrusaBuddy;
        let mut start = Vec::new();
        if prusa {
            start.push(S::NozzleCheck);
            if let Some(model) = &self.printer_model {
                start.push(S::ModelCheck {
                    model: model.clone(),
                });
            }
        }
        start.extend([S::FeedScaleReset, S::FlowScaleReset]);
        if matches!(
            self.firmware,
            FirmwareFamily::Marlin2 | FirmwareFamily::BambuLab
        ) {
            start.push(S::VolumetricOff);
        }
        start.push(S::FanOff);
        let none = crate::manufacturing::BlockEffects::NONE;
        let begin = self.start_block.as_ref().map_or(none, |b| b.effects);
        if let Some(block) = &self.start_block {
            start.push(S::Block {
                block: block.clone(),
            });
        }
        // Loadcell probing (Prusa) needs a settled, non-oozing nozzle.
        let standby = Some(if prusa { 170.0 } else { 150.0 });
        let heated = self.heated_bed;
        if heated && begin.bed == ThermalEffect::Unchanged {
            start.push(S::BedTarget);
        }
        if begin.nozzle == ThermalEffect::Unchanged {
            start.push(S::NozzleTarget { standby_c: standby });
            if prusa {
                start.push(S::NozzleWait { standby_c: standby });
            }
        }
        if !begin.homes {
            start.push(S::Home);
        }
        if begin.modes == ModeEffect::Unknown {
            start.push(S::RestoreModes);
        }
        if heated && begin.bed != ThermalEffect::Wait {
            start.push(S::BedWait);
        }
        if begin.levelling != LevellingEffect::Active {
            match self.levelling {
                BedLevelling::Manual => {}
                BedLevelling::ProbeMesh => start.push(S::ProbeMesh),
                // Prusa Buddy keeps no stored mesh; it probes each print.
                BedLevelling::StoredMesh if prusa => start.push(S::ProbeMesh),
                BedLevelling::StoredMesh => start.push(S::RestoreMesh),
            }
        }
        if begin.nozzle != ThermalEffect::Wait {
            start.push(S::NozzleWait { standby_c: None });
        }
        if begin.extrusion == ExtrusionEffect::Unknown {
            start.push(S::ResetExtrusion);
        }
        start.push(S::Prime);
        let mut end = vec![S::Barrier, S::Park, S::Barrier];
        let finish = self.end_block.as_ref().map_or(none, |b| b.effects);
        if let Some(block) = &self.end_block {
            end.push(S::Block {
                block: block.clone(),
            });
        }
        if finish.nozzle != ThermalEffect::Off {
            end.push(S::NozzleOff);
        }
        if heated && finish.bed != ThermalEffect::Off {
            end.push(S::BedOff);
        }
        if !finish.fan_off {
            end.push(S::FanOff);
        }
        if !finish.motors_off {
            end.push(S::MotorsOff);
        }
        FirmwareLifecycle { start, end }
    }
}
/// Marlin's default junction deviation (`JUNCTION_DEVIATION_MM`), used for
/// every family's print-time estimate.
pub const MARLIN_JUNCTION_DEVIATION_MM: f64 = 0.013;
fn r6(v: f64) -> f64 {
    (v * 1_000_000.0).round() / 1_000_000.0
}

impl HardwareSetupDefinition {
    /// A setup whose printer is derived from `machine`.
    pub fn derived(
        id: String,
        label: String,
        model: HardwareModelDefinition,
        machine: MachineDescription,
    ) -> Result<Self, String> {
        let printer = machine.derive_printer(&id, &label)?;
        let setup = Self {
            id,
            label,
            model,
            machine: Some(machine),
            printer,
        };
        setup.validate()?;
        Ok(setup)
    }
    /// Re-derive after an edit of identity or facts.
    pub fn rederive(&mut self) -> Result<(), String> {
        if let Some(m) = &self.machine {
            self.printer = m.derive_printer(&self.id, &self.label)?;
        }
        self.validate()
    }
}

/// Authoring data: inheritance layers and the concrete setups built from them.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MachineCatalog {
    pub layers: Vec<MachineLayer>,
    pub setups: Vec<MachineSetupDeclaration>,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MachineSetupDeclaration {
    pub id: String,
    pub label: String,
    /// Catalog model identity.
    pub model: String,
    /// Concrete machine layer.
    pub machine: String,
}

#[cfg(test)]
mod tests;
