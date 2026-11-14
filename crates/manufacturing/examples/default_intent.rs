// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Print the default manufacturing intent for a shipped setup, a polymer
//! family and a quality preset: the same creation the engine performs for a
//! first material, as inert JSON for the CLI's `--intent`.
//!
//! `cargo run -p spiling-manufacturing --example default_intent -- SETUP_ID PLA standard`
use spiling_contracts::hardware::{ProjectHardwareCapture, builtin_catalog};
use spiling_manufacturing::{QualityPreset, create_default_intent, generic_selection};
use std::sync::Arc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [setup_id, family, quality] = args.as_slice() else {
        return Err("usage: default_intent SETUP_ID POLYMER_FAMILY draft|standard|fine".into());
    };
    let quality = match quality.as_str() {
        "draft" => QualityPreset::Draft,
        "standard" => QualityPreset::Standard,
        "fine" => QualityPreset::Fine,
        other => return Err(format!("unknown quality {other}").into()),
    };
    let catalog = builtin_catalog()?;
    let setup = catalog
        .setups
        .iter()
        .find(|s| &s.id == setup_id)
        .ok_or_else(|| format!("no shipped setup {setup_id}"))?
        .clone();
    let plate = catalog
        .plates
        .iter()
        .find(|p| p.mounting_interface == setup.model.mounting_interface)
        .ok_or("the setup has no plate")?
        .clone();
    let face = plate
        .faces
        .first()
        .ok_or("the plate has no face")?
        .key
        .clone();
    let selection = generic_selection(&setup, family)?;
    let hardware = Arc::new(ProjectHardwareCapture {
        setup,
        plate,
        face,
        accessories: Vec::new(),
    });
    let intent = create_default_intent(hardware, selection, quality)?;
    println!("{}", serde_json::to_string_pretty(&intent)?);
    Ok(())
}
