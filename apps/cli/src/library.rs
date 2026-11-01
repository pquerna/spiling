// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::{Command, Error, Options, set_once};
use spiling_contracts::{geometry::NativePath, library::*};
use spiling_engine_client::EngineRpc;
use std::{ffi::OsString, path::PathBuf};
pub(super) const USAGE: &str = "library (hardware|hardware-catalog|add-machine NAME [--setup ID]|add-plate NAME [--plate ID]|hardware-capture --machine UUID --plate UUID --face KEY|replace-hardware HARDWARE_JSON|status|filaments|recipes|recipe RECIPE_UUID|spools|add-filament NAME|add-spool FILAMENT_UUID|write REQUEST_JSON) [--library ABSOLUTE_PATH] [--query TEXT] [--on-hand] [--hex RRGGBB] [--grams N] [--engine PATH]. Unknown color, quantity and manufacturing identity remain unspecified. Rich definitions and explicit revision-bound edits use bounded LibraryWrite JSON; repeat its request_id to resolve an uncertain publication.";
pub(super) struct Workflow {
    location: Option<PathBuf>,
    action: Action,
}
enum Action {
    Hardware,
    HardwareCatalog,
    AddMachine(String, Option<String>),
    AddPlate(String, Option<String>),
    HardwareCapture(String, String, String),
    ReplaceHardware(Box<spiling_contracts::hardware::HardwareLibrary>),
    Status,
    Filaments(LibraryFilamentQuery),
    Spools,
    Recipes(LibraryRecipeQuery),
    Recipe(String),
    AddFilament(Box<FilamentDefinition>),
    AddSpool(String, Option<f64>),
    Write(Box<LibraryWrite>),
}
pub(super) fn parse(
    mut args: impl Iterator<Item = OsString>,
    mut engine: Option<PathBuf>,
    mut store: Option<PathBuf>,
) -> Result<Option<Options>, Error> {
    let action = args.next().ok_or(USAGE)?;
    let argument = if matches!(
        action.to_str(),
        Some(
            "add-filament"
                | "add-spool"
                | "write"
                | "recipe"
                | "add-machine"
                | "add-plate"
                | "replace-hardware"
        )
    ) {
        Some(args.next().ok_or(USAGE)?)
    } else {
        None
    };
    let mut location = None;
    let mut query = LibraryFilamentQuery::default();
    let mut hex = None;
    let mut grams = None;
    let mut profile = None;
    let mut setup_id = None;
    let mut machine_id = None;
    let mut plate_id = None;
    let mut face = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--help" | "-h") => return Ok(None),
            Some("--setup") => {
                setup_id = Some(
                    args.next()
                        .ok_or("--setup requires ID")?
                        .into_string()
                        .map_err(|_| "setup requires UTF-8")?,
                )
            }
            Some("--machine") => {
                machine_id = Some(
                    args.next()
                        .ok_or("--machine requires ID")?
                        .into_string()
                        .map_err(|_| "machine requires UTF-8")?,
                )
            }
            Some("--plate") => {
                plate_id = Some(
                    args.next()
                        .ok_or("--plate requires ID")?
                        .into_string()
                        .map_err(|_| "plate requires UTF-8")?,
                )
            }
            Some("--face") => {
                face = Some(
                    args.next()
                        .ok_or("--face requires key")?
                        .into_string()
                        .map_err(|_| "face requires UTF-8")?,
                )
            }
            Some("--library") => set_once(
                &mut location,
                PathBuf::from(args.next().ok_or("--library requires a path")?),
                "--library",
            )?,
            Some("--engine") => set_once(
                &mut engine,
                PathBuf::from(args.next().ok_or("--engine requires a path")?),
                "--engine",
            )?,
            Some("--store") => set_once(
                &mut store,
                PathBuf::from(args.next().ok_or("--store requires a path")?),
                "--store",
            )?,
            Some("--query") => {
                query.query = args
                    .next()
                    .ok_or("--query requires text")?
                    .into_string()
                    .map_err(|_| "query requires UTF-8")?
            }
            Some("--on-hand") => query.on_hand_only = true,
            Some("--include-archived") => query.include_archived = true,
            Some("--hex") => {
                let value = args.next().ok_or("--hex requires RGB")?;
                set_once(
                    &mut hex,
                    spiling_contracts::color_naming::parse_srgb8_hex(
                        value.to_str().ok_or("hex requires UTF-8")?,
                    )?,
                    "--hex",
                )?;
            }
            Some("--profile") => {
                use std::io::Read;
                let path = PathBuf::from(args.next().ok_or("--profile requires a file")?);
                let mut bytes = Vec::new();
                std::fs::File::open(path)?
                    .take(32769)
                    .read_to_end(&mut bytes)?;
                if bytes.len() > 32768 {
                    return Err("profile exceeds 32 KiB".into());
                }
                let selection: spiling_contracts::manufacturing::MaterialProfileSelection =
                    serde_json::from_slice(&bytes)?;
                selection.validate()?;
                set_once(&mut profile, selection.profile, "--profile")?;
            }
            Some("--grams") => {
                let value = args.next().ok_or("--grams requires a number")?;
                set_once(
                    &mut grams,
                    value
                        .to_str()
                        .ok_or("grams requires UTF-8")?
                        .parse::<f64>()?,
                    "--grams",
                )?;
            }
            _ => return Err(USAGE.into()),
        }
    }
    if location.as_ref().is_some_and(|p| !p.is_absolute()) {
        return Err("--library must be absolute".into());
    }
    let action = match action.to_str() {
        Some("hardware") => Action::Hardware,
        Some("hardware-catalog") => Action::HardwareCatalog,
        Some("add-machine") => Action::AddMachine(
            argument
                .unwrap()
                .into_string()
                .map_err(|_| "nickname requires UTF-8")?,
            setup_id,
        ),
        Some("add-plate") => Action::AddPlate(
            argument
                .unwrap()
                .into_string()
                .map_err(|_| "nickname requires UTF-8")?,
            plate_id,
        ),
        Some("hardware-capture") => Action::HardwareCapture(
            machine_id.ok_or("--machine is required")?,
            plate_id.ok_or("--plate is required")?,
            face.ok_or("--face is required")?,
        ),
        Some("replace-hardware") => {
            use std::io::Read;
            let mut bytes = Vec::new();
            std::fs::File::open(PathBuf::from(argument.unwrap()))?
                .take(49153)
                .read_to_end(&mut bytes)?;
            if bytes.len() > 49152 {
                return Err("hardware exceeds 48 KiB".into());
            }
            let value: spiling_contracts::hardware::HardwareLibrary =
                serde_json::from_slice(&bytes)?;
            value.validate()?;
            Action::ReplaceHardware(Box::new(value))
        }
        Some("status") => Action::Status,
        Some("filaments") => Action::Filaments(query),
        Some("spools") => Action::Spools,
        Some("recipes") => Action::Recipes(LibraryRecipeQuery {
            query: query.query,
            ..Default::default()
        }),
        Some("recipe") => {
            let id = argument
                .unwrap()
                .into_string()
                .map_err(|_| "recipe UUID requires UTF-8")?;
            validate_library_uuid(&id)?;
            Action::Recipe(id)
        }
        Some("add-filament") => Action::AddFilament(Box::new(FilamentDefinition {
            name: argument
                .unwrap()
                .into_string()
                .map_err(|_| "name requires UTF-8")?,
            material: profile.as_ref().map(|p| p.material.clone()),
            color_name: None,
            appearance: hex.map(|rgb| DeclaredAppearance {
                color: spiling_contracts::appearance::AppearanceColor::Srgb {
                    channels: rgb.map(|v| f64::from(v) / 255.0),
                },
                source: DefinitionSource::Operator,
            }),
            profiles: profile.into_iter().collect(),
            source: DefinitionSource::Operator,
        })),
        Some("add-spool") => Action::AddSpool(
            argument
                .unwrap()
                .into_string()
                .map_err(|_| "filament id requires UTF-8")?,
            grams,
        ),
        Some("write") => {
            use std::io::Read;
            let path = PathBuf::from(argument.unwrap());
            let mut bytes = Vec::new();
            std::fs::File::open(path)?
                .take(65537)
                .read_to_end(&mut bytes)?;
            if bytes.len() > 65536 {
                return Err("library request exceeds 64 KiB".into());
            }
            let request: LibraryWrite = serde_json::from_slice(&bytes)?;
            request.validate()?;
            Action::Write(Box::new(request))
        }
        _ => return Err(USAGE.into()),
    };
    Ok(Some(Options {
        command: Command::Library(Workflow { location, action }),
        engine: engine.unwrap_or(
            std::env::current_exe()?
                .with_file_name(format!("spiling-engine{}", std::env::consts::EXE_SUFFIX)),
        ),
        store,
    }))
}
pub(super) async fn execute(rpc: &EngineRpc, flow: Workflow) -> Result<serde_json::Value, Error> {
    if let Some(path) = flow.location {
        rpc.library_set_location(NativePath::from_os_str(path.as_os_str())?)
            .await?;
    }
    let value = match flow.action {
        Action::Hardware => serde_json::to_value(rpc.library_hardware().await?)?,
        Action::HardwareCatalog => serde_json::to_value(rpc.hardware_catalog().await?)?,
        Action::HardwareCapture(machine, plate, face) => serde_json::to_value(
            rpc.library_hardware()
                .await?
                .hardware
                .capture(&machine, &plate, &face)?,
        )?,
        action @ (Action::AddMachine(..) | Action::AddPlate(..) | Action::ReplaceHardware(..)) => {
            use spiling_contracts::hardware::*;
            let mut snapshot = rpc.library_hardware().await?;
            match action {
                Action::AddMachine(nickname, setup) => {
                    let catalog = rpc.hardware_catalog().await?;
                    let setup = catalog
                        .setups
                        .into_iter()
                        .find(|s| setup.as_ref().is_none_or(|id| id == &s.id))
                        .ok_or("catalog setup absent; catalog-only models cannot be planned")?;
                    snapshot.hardware.machines.push(OwnedMachine {
                        id: uuid::Uuid::new_v4().to_string(),
                        nickname,
                        serial: None,
                        location: None,
                        setup,
                        generation: 1,
                    });
                }
                Action::AddPlate(nickname, plate) => {
                    let catalog = rpc.hardware_catalog().await?;
                    let definition = catalog
                        .plates
                        .into_iter()
                        .find(|p| plate.as_ref().is_none_or(|id| id == &p.id))
                        .ok_or("catalog plate absent")?;
                    snapshot.hardware.plates.push(OwnedPlate {
                        id: uuid::Uuid::new_v4().to_string(),
                        nickname,
                        definition,
                        installed_machine: None,
                        notes: String::new(),
                    });
                }
                Action::ReplaceHardware(h) => snapshot.hardware = *h,
                _ => unreachable!(),
            }
            snapshot.hardware.validate()?;
            let request = LibraryWrite {
                library_id: snapshot.identity.as_ref().map(|i| i.id.clone()),
                base_revision: snapshot.identity.map_or(0, |i| i.revision),
                request_id: uuid::Uuid::new_v4().to_string(),
                mutation: LibraryMutation::ReplaceHardware {
                    hardware: snapshot.hardware,
                },
            };
            serde_json::json!({"request":request,"receipt":rpc.library_write(&request).await?})
        }
        Action::Status => serde_json::to_value(rpc.library_status().await?)?,
        Action::Filaments(mut query) => {
            let mut rows = Vec::new();
            loop {
                let page = rpc.library_filaments(&query).await?;
                rows.extend(page.entries);
                if let Some(offset) = page.next_offset {
                    query.expected = page.identity;
                    query.offset = offset;
                } else {
                    break;
                }
            }
            serde_json::to_value(rows)?
        }
        Action::Recipes(mut query) => {
            let mut rows = Vec::new();
            loop {
                let page = rpc.library_recipes(&query).await?;
                rows.extend(page.entries);
                if let Some(offset) = page.next_offset {
                    query.expected = page.identity;
                    query.offset = offset;
                } else {
                    break;
                }
            }
            serde_json::to_value(rows)?
        }
        Action::Recipe(id) => {
            let status = rpc.library_status().await?;
            let identity = status.identity.ok_or("My library is empty")?;
            let mut query = LibraryRecipeQuery {
                expected: Some(identity.clone()),
                ..Default::default()
            };
            let reference = loop {
                let page = rpc.library_recipes(&query).await?;
                if let Some(r) = page.entries.into_iter().find(|r| r.reference.id.0 == id) {
                    break r.reference;
                }
                if let Some(offset) = page.next_offset {
                    query.offset = offset;
                } else {
                    return Err("recipe is absent".into());
                }
            };
            serde_json::to_value(rpc.library_recipe(&identity.id, &reference).await?)?
        }
        Action::Spools => {
            let mut rows = Vec::new();
            let mut query = LibrarySpoolQuery::default();
            loop {
                let page = rpc.library_spools(&query).await?;
                rows.extend(page.spools);
                if let Some(offset) = page.next_offset {
                    query.expected = page.identity;
                    query.offset = offset;
                } else {
                    break;
                }
            }
            serde_json::to_value(rows)?
        }
        Action::Write(request) => serde_json::to_value(rpc.library_write(&request).await?)?,
        action => {
            let status = rpc.library_status().await?;
            let mutation = match action {
                Action::AddFilament(definition) => LibraryMutation::PutFilament {
                    id: FilamentId(uuid::Uuid::new_v4().to_string()),
                    expected: None,
                    definition: *definition,
                },
                Action::AddSpool(id, grams) => {
                    validate_library_uuid(&id)?;
                    let mut query = LibraryFilamentQuery {
                        expected: status.identity.clone(),
                        include_archived: true,
                        ..Default::default()
                    };
                    let reference = loop {
                        let page = rpc.library_filaments(&query).await?;
                        if let Some(row) = page
                            .entries
                            .into_iter()
                            .find(|e| e.entry.current.id.0 == id)
                        {
                            break row.entry.current;
                        }
                        if let Some(offset) = page.next_offset {
                            query.offset = offset;
                        } else {
                            return Err("filament is not in this library".into());
                        }
                    };
                    LibraryMutation::PutSpool {
                        spool: SpoolRecord {
                            id: SpoolId(uuid::Uuid::new_v4().to_string()),
                            filament: reference,
                            name: "Spool".into(),
                            state: StockState::OnHand,
                            quantity: grams.map(|remaining_g| StockQuantity {
                                remaining_g,
                                source: QuantitySource::Entered,
                            }),
                            lot: None,
                            location: None,
                        },
                    }
                }
                _ => unreachable!(),
            };
            let request = LibraryWrite {
                library_id: status.identity.as_ref().map(|i| i.id.clone()),
                base_revision: status.identity.map_or(0, |i| i.revision),
                request_id: uuid::Uuid::new_v4().to_string(),
                mutation,
            };
            // Retain the exact operation in output so uncertain publication can be resolved.
            serde_json::json!({"request":request,"receipt":rpc.library_write(&request).await?})
        }
    };
    Ok(serde_json::json!({"command":"library","result":value}))
}
