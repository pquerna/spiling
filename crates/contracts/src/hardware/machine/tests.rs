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
        for p in [job.prime_start_mm, job.prime_end_mm, job.park_position_mm] {
            assert!(
                (0..3).all(|a| p[a] >= e.min[a] && p[a] <= e.max[a]),
                "{}",
                s.id
            );
        }
        assert_eq!(job.park_position_mm[2], e.max[2]);
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
    assert_eq!(job.park_position_mm, [9.0, 171.0, 180.0]);
}

#[test]
fn generic_defaults_are_sized_for_modern_printers_and_sourced() {
    let s = described()
        .into_iter()
        .find(|s| s.id == "generic.klipper-cartesian.04")
        .unwrap();
    let m = s.machine.as_ref().unwrap();
    assert!(s.printer.max_feed_mm_s >= 500.0);
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
