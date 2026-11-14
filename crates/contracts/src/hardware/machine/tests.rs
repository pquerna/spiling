// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
use super::*;
use crate::hardware::builtin_catalog;

fn described() -> Vec<HardwareSetupDefinition> {
    builtin_catalog()
        .unwrap()
        .setups
        .into_iter()
        .filter(|s| s.machine.is_some())
        .collect()
}

#[test]
fn every_catalog_printer_reference_has_a_derived_setup() {
    let setups = described();
    for (vendor, model) in [
        ("Prusa Research", "Original Prusa MK4S"),
        ("Bambu Lab", "A1"),
        ("Bambu Lab", "A1 mini"),
        ("Creality", "Ender-3 V3 SE"),
        ("Generic", "Klipper Cartesian"),
        ("Generic", "Marlin bed-slinger"),
    ] {
        let s = setups
            .iter()
            .find(|s| s.model.vendor == vendor && s.model.model == model)
            .unwrap_or_else(|| panic!("{vendor} {model} has no setup"));
        let m = s.machine.as_ref().unwrap();
        assert_eq!(s.model.declared_volume_mm, Some(m.build_envelope()));
        assert_eq!(m.lineage.first().map(String::as_str), Some("generic.fff"));
    }
}

#[test]
fn derived_lifecycle_lies_inside_the_bed_and_parks_above_any_part() {
    for s in described() {
        let JobWorkflow::CompleteMarlin { job } = &s.printer.workflow else {
            panic!("derived setups are complete jobs");
        };
        let e = s.printer.build_envelope;
        let park = [job.park_xy_mm[0], job.park_xy_mm[1], e.max[2]];
        for p in [job.prime_start_mm, job.prime_end_mm, park] {
            assert!(
                (0..3).all(|a| p[a] >= e.min[a] && p[a] <= e.max[a]),
                "{}",
                s.id
            );
        }
        assert_eq!(job.park_clearance_mm, 10.0);
        assert!(job.prime_start_mm[1] < e.max[1]);
    }
}

#[test]
fn deposition_limits_scale_with_the_nozzle() {
    let s = described()
        .into_iter()
        .find(|s| s.id == "generic.marlin-bedslinger.04")
        .unwrap();
    let l = s.printer.deposition_limits;
    assert_eq!((l.min_bead_width_mm, l.max_bead_width_mm), (0.3, 0.75));
    assert_eq!(l.max_bead_height_mm, 0.3);
    let mut m = s.machine.clone().unwrap();
    m.declare(MachineLimit::NozzleDiameterMm, 0.8);
    assert_eq!(
        m.source(MachineLimit::NozzleDiameterMm),
        Some(&FactSource::OperatorDeclared)
    );
    let p = m.derive_printer(&s.id, &s.label).unwrap();
    assert_eq!(p.deposition_limits.min_bead_width_mm, 0.6);
    assert_eq!(p.deposition_limits.max_bead_width_mm, 1.5);
    assert_eq!(p.deposition_limits.max_bead_height_mm, 0.6);
}

#[test]
fn small_bed_edits_rederive_lifecycle_inside_the_new_bed() {
    let mut s = described()
        .into_iter()
        .find(|s| s.id == "generic.klipper-cartesian.04")
        .unwrap();
    let m = s.machine.as_mut().unwrap();
    m.declare(MachineLimit::BedXMm, 180.0);
    m.declare(MachineLimit::BedYMm, 180.0);
    m.declare(MachineLimit::MaxZMm, 180.0);
    // The stale printer no longer matches its facts.
    assert!(s.validate().is_err());
    s.rederive().unwrap();
    let JobWorkflow::CompleteMarlin { job } = &s.printer.workflow else {
        unreachable!()
    };
    assert_eq!(job.prime_start_mm, [9.0, 171.0, 0.2]);
    assert_eq!(job.park_xy_mm, [9.0, 171.0]);
}

#[test]
fn published_print_cap_is_separate_from_travel() {
    let s = described()
        .into_iter()
        .find(|s| s.id == "creality.ender-3-v3-se.04")
        .unwrap();
    // The Ender publishes a 250 mm/s print speed; travel inherits the family.
    assert_eq!(s.printer.max_print_feed_mm_s, 250.0);
    assert_eq!(s.printer.max_travel_feed_mm_s, 800.0);
}

#[test]
fn generic_defaults_are_sized_for_modern_printers_and_sourced() {
    let s = described()
        .into_iter()
        .find(|s| s.id == "generic.klipper-cartesian.04")
        .unwrap();
    let m = s.machine.as_ref().unwrap();
    assert!(s.printer.max_print_feed_mm_s >= 500.0);
    assert!(m.limit(MachineLimit::MaxAccelerationMmS2) >= 20_000.0);
    assert!(s.printer.max_volumetric_flow_mm3_s >= 25.0);
    assert!(m.limits.iter().all(|l| matches!(
        &l.source,
        FactSource::FamilyDefault { family } if family == "generic.fff"
    )));
    let mk4s = described()
        .into_iter()
        .find(|s| s.id == "prusa.mk4s.04")
        .unwrap();
    let m = mk4s.machine.unwrap();
    assert!(matches!(
        m.source(MachineLimit::BedXMm),
        Some(FactSource::Published { .. })
    ));
    assert!(matches!(
        m.source(MachineLimit::MaxVolumetricFlowMm3S),
        Some(FactSource::FamilyDefault { .. })
    ));
}

#[test]
fn inheritance_refuses_cycles_missing_facts_and_foreign_defaults() {
    let base = |id: &str, parent: Option<&str>| MachineLayer {
        id: id.into(),
        parent: parent.map(Into::into),
        firmware: None,
        kinematics: None,
        levelling: None,
        heated_bed: None,
        arc_moves: None,
        printer_model: None,
        limits: vec![],
    };
    let cycle = [base("a", Some("b")), base("b", Some("a"))];
    assert!(
        MachineDescription::resolve(&cycle, "a")
            .unwrap_err()
            .contains("cycle")
    );
    assert!(MachineDescription::resolve(&[base("a", None)], "a").is_err());
    let mut foreign = base("a", None);
    foreign.limits.push(SourcedLimit {
        key: MachineLimit::BedXMm,
        value: 100.0,
        source: FactSource::FamilyDefault {
            family: "other".into(),
        },
    });
    assert!(
        MachineDescription::resolve(&[foreign], "a")
            .unwrap_err()
            .contains("family default")
    );
}

#[test]
fn machine_description_has_checked_wire_roundtrip() {
    for s in described() {
        let wire: crate::rpc::HardwareSetupDefinition = s.clone().try_into().unwrap();
        let back: HardwareSetupDefinition = wire.try_into().unwrap();
        assert_eq!(s, back);
    }
}

#[test]
fn firmware_family_selects_dialect_and_lifecycle_table() {
    use crate::manufacturing::{LifecycleBlock, LifecycleStep as S, OutputDialect as D};
    let setups = described();
    let get = |id: &str| setups.iter().find(|s| s.id == id).unwrap();
    let lifecycle = |s: &HardwareSetupDefinition| match &s.printer.workflow {
        JobWorkflow::CompleteMarlin { job } => job.lifecycle.clone(),
        _ => unreachable!(),
    };
    let expect = [
        ("creality.ender-3-v3-se.04", D::Marlin2SingleExtruderGcodeV1),
        (
            "generic.marlin-bedslinger.04",
            D::Marlin2SingleExtruderGcodeV1,
        ),
        ("prusa.mk4s.04", D::PrusaMarlinSingleExtruderGcodeV1),
        (
            "generic.klipper-cartesian.04",
            D::KlipperSingleExtruderGcodeV1,
        ),
        ("bambulab.a1.04", D::BambuLabGcode3mfV1),
        ("bambulab.a1-mini.04", D::BambuLabGcode3mfV1),
    ];
    for (id, dialect) in expect {
        let s = get(id);
        assert_eq!(s.printer.capabilities.output_dialect, dialect, "{id}");
        let l = lifecycle(s);
        // Probe-mesh printers level every print, after homing and bed wait.
        let home = l.start.iter().position(|x| *x == S::Home).unwrap();
        let probe = l.start.iter().position(|x| *x == S::ProbeMesh).unwrap();
        assert!(home < probe, "{id}");
        assert!(!l.start.contains(&S::LevellingOff) && !l.start.contains(&S::CelsiusUnits));
        assert_eq!(
            l.start.contains(&S::VolumetricOff),
            matches!(
                dialect,
                D::Marlin2SingleExtruderGcodeV1 | D::BambuLabGcode3mfV1
            ),
            "{id}"
        );
    }
    let mk4s = lifecycle(get("prusa.mk4s.04"));
    assert_eq!(
        mk4s.start[..2],
        [
            S::NozzleCheck,
            S::ModelCheck {
                model: "MK4S".into()
            }
        ]
    );
    // Stored-mesh Marlin restores the mesh; Klipper macros replace the table.
    let mut marlin = get("generic.marlin-bedslinger.04").clone();
    let m = marlin.machine.as_mut().unwrap();
    m.levelling = BedLevelling::StoredMesh;
    marlin.rederive().unwrap();
    assert!(lifecycle(&marlin).start.contains(&S::RestoreMesh));
    let mut klipper = get("generic.klipper-cartesian.04").clone();
    let m = klipper.machine.as_mut().unwrap();
    m.start_block = Some(LifecycleBlock::klipper_start_print());
    m.end_block = Some(LifecycleBlock::klipper_end_print());
    klipper.rederive().unwrap();
    let l = lifecycle(&klipper);
    assert!(!l.start.contains(&S::Home) && !l.start.contains(&S::ProbeMesh));
    assert!(l.start.contains(&S::RestoreModes) && l.start.contains(&S::ResetExtrusion));
    assert_eq!(l.end.len(), 4, "{:?}", l.end);
    // A block line too long for Marlin's command buffer refuses derivation.
    let mut long = get("generic.marlin-bedslinger.04").clone();
    let m = long.machine.as_mut().unwrap();
    let mut block = LifecycleBlock::klipper_end_print();
    block.lines = vec![format!("M117 {}", "x".repeat(96))];
    m.end_block = Some(block);
    assert!(long.rederive().is_err());
}
