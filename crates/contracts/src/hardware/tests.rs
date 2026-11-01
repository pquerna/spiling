// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
use super::*;
fn capture() -> ProjectHardwareCapture {
    let c = builtin_catalog().unwrap();
    ProjectHardwareCapture {
        setup: c.setups[0].clone(),
        plate: c.plates[0].clone(),
        face: "textured".into(),
        accessories: vec![],
    }
}
#[test]
fn portable_capture_has_checked_wire_roundtrip_and_independent_plate_admission() {
    let c = capture();
    let wire: crate::rpc::ProjectHardwareCapture = c.clone().try_into().unwrap();
    let back: ProjectHardwareCapture = wire.try_into().unwrap();
    assert_eq!(c, back);
    let part = AabbMm {
        min: [10., 10., 0.],
        max: [20., 20., 10.],
    };
    assert!(c.admits_bounds(&part).is_ok());
    let mut narrow = c.clone();
    narrow.plate.faces[0].usable_mm.max[0] = 15.;
    assert!(narrow.admits_bounds(&part).is_err());
    assert_ne!(c.hash().unwrap(), narrow.hash().unwrap());
    let mut blocked = c.clone();
    blocked.plate.faces[0].keepouts_mm.push(part);
    assert!(blocked.admits_bounds(&part).is_err());
    let mut invalid = c.clone();
    invalid.plate.mounting_interface = "different-mount".into();
    assert!(invalid.validate().is_err());
}
#[test]
fn catalog_identity_is_not_machine_support() {
    let c = builtin_catalog().unwrap();
    assert!(c.models.iter().any(|m| m.id == "bambulab.h2c"));
    assert!(!c.setups.iter().any(|s| s.model.id == "bambulab.h2c"));
}
#[test]
fn same_name_devices_and_normalized_credentials_keep_distinct_identities() {
    let c = capture();
    let machine = |id| OwnedMachine {
        id,
        nickname: "Workshop".into(),
        serial: None,
        location: None,
        setup: c.setup.clone(),
        generation: 1,
    };
    let mut h = HardwareLibrary::default();
    h.machines.push(machine(uuid::Uuid::new_v4().to_string()));
    h.machines.push(machine(uuid::Uuid::new_v4().to_string()));
    h.validate().unwrap();
    let r = CredentialRef {
        slot_id: uuid::Uuid::new_v4().to_string(),
        role: CredentialRole::LanAccessCode,
        backend: CredentialBackend::Keyring,
    };
    assert_eq!(
        r.account().unwrap(),
        format!("v1.{}.lan-access-code", r.slot_id)
    );
    let mut bad = r.clone();
    bad.slot_id = "../../elsewhere".into();
    assert!(bad.account().is_err());
    h.machines[1].id = h.machines[0].id.clone();
    assert!(h.validate().is_err());
}
#[test]
fn private_record_cannot_deserialize_as_project_capture() {
    let c = capture();
    let mut v = serde_json::to_value(&c).unwrap();
    v["nickname"] = "private-machine".into();
    assert!(serde_json::from_value::<ProjectHardwareCapture>(v).is_err());
}

#[test]
fn plate_datum_temperature_nozzle_and_nonfinite_bounds_are_explicit_refusals() {
    let c = capture();
    let mut intent: ManufacturingIntent = serde_json::from_str(include_str!(
        "../../../../fixtures/manufacturing/complete-sparse.intent.json"
    ))
    .unwrap();
    let catalog = builtin_catalog().unwrap();
    let mut complete = c.clone();
    complete.setup = catalog
        .setups
        .iter()
        .find(|s| {
            matches!(
                s.printer.workflow,
                crate::manufacturing::JobWorkflow::CompleteMarlin { .. }
            )
        })
        .unwrap()
        .clone();
    intent.printer = complete.setup.printer.clone();
    intent.hardware = Some(std::sync::Arc::new(complete.clone()));
    complete.validate_intent(&intent).unwrap();
    complete.plate.faces[0].max_bed_celsius = 10.;
    assert!(
        complete
            .validate_intent(&intent)
            .unwrap_err()
            .contains("temperature")
    );
    let mut datum = c.clone();
    datum.plate.surface_z_mm = 1.;
    assert!(datum.validate().unwrap_err().contains("datum"));
    let mut nozzle = c.clone();
    nozzle.accessories.push(AccessoryDefinition {
        id: "test.nozzle".into(),
        label: "0.6 mm nozzle".into(),
        mounting_interface: nozzle.setup.model.mounting_interface.clone(),
        effect: AccessoryEffect::Nozzle {
            diameter_mm: 0.6,
            material: "hardened steel".into(),
        },
    });
    assert!(nozzle.validate().unwrap_err().contains("nozzle"));
    assert!(
        c.admits_bounds(&AabbMm {
            min: [f64::NAN, 0., 0.],
            max: [10.; 3]
        })
        .is_err()
    );
    for field in ["setup", "plate", "face", "accessories"] {
        let mut value = serde_json::to_value(&c).unwrap();
        value.as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<ProjectHardwareCapture>(value).is_err());
    }
}

#[test]
fn standalone_native_owned_records_validate_identity_and_file_route_authority() {
    let invalid = crate::rpc::OwnedMachine {
        id: "not-a-uuid".into(),
        nickname: "Workshop".into(),
        setup: Some(capture().setup.try_into().unwrap()),
        generation: 1,
        serial: None,
        location: None,
    };
    assert!(OwnedMachine::try_from(invalid).is_err());
    let machine = uuid::Uuid::new_v4().to_string();
    let mut route = EndpointRoute {
        id: uuid::Uuid::new_v4().to_string(),
        machine_id: machine.clone(),
        family: ConnectionFamily::File,
        endpoint: "local-file".into(),
        expected_identity: machine,
        generation: 1,
        credential: None,
    };
    route.validate().unwrap();
    route.credential = Some(CredentialRef {
        slot_id: uuid::Uuid::new_v4().to_string(),
        role: CredentialRole::ApiKey,
        backend: CredentialBackend::PlaintextFile,
    });
    assert!(route.validate().is_err());
}
