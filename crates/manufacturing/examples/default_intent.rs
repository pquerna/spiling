// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Print the default manufacturing intent for a shipped setup, a polymer
//! family and a quality preset: the same creation the engine's
//! CreateDefaultIntent performs, as inert JSON (supports automatic).
//!
//! `cargo run -p spiling-manufacturing --example default_intent -- SETUP PLA standard`
use spiling_contracts::hardware::builtin_catalog;
use spiling_manufacturing::{
    QualityPreset, SupportChoice, create_default_intent, generic_selection,
};
use std::sync::Arc;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [setup, family, quality] = args.as_slice() else {
        return Err("usage: default_intent SETUP POLYMER_FAMILY draft|standard|fine".into());
    };
    let quality =
        QualityPreset::parse(quality).ok_or_else(|| format!("unknown quality {quality}"))?;
    let catalog = builtin_catalog()?;
    let setup = catalog.find_setup(setup)?;
    let selection = generic_selection(setup, family)?;
    let hardware = Arc::new(catalog.stock_capture(setup)?);
    let intent = create_default_intent(hardware, selection, quality, SupportChoice::Automatic)?;
    println!("{}", serde_json::to_string_pretty(&intent)?);
    Ok(())
}
