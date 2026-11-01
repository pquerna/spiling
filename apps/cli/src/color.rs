// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::{Command, Error, Options, set_once};
use spiling_contracts::color_naming::*;
use std::{ffi::OsString, path::PathBuf};
pub(super) fn parse(
    args: impl Iterator<Item = OsString>,
    mut engine: Option<PathBuf>,
    mut store: Option<PathBuf>,
) -> Result<Option<Options>, Error> {
    let mut args = args;
    let mut colors = Vec::new();
    let mut mode = None;
    while let Some(value) = args.next() {
        match value.to_str() {
            Some("--help" | "-h") => return Ok(None),
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
            Some("--mode") => {
                let text = args
                    .next()
                    .ok_or("--mode requires exact, nearest or descriptive")?;
                let value = match text.to_str() {
                    Some("exact") => ColorNamingMode::Exact,
                    Some("nearest") => ColorNamingMode::Nearest,
                    Some("descriptive") => ColorNamingMode::Descriptive,
                    _ => return Err("--mode requires exact, nearest or descriptive".into()),
                };
                set_once(&mut mode, value, "--mode")?;
            }
            Some(text) => {
                if colors.len() == MAX_COLOR_NAMING_SAMPLES {
                    return Err("at most 64 colors per request".into());
                }
                colors.push(parse_srgb8_hex(text)?);
            }
            None => return Err("colors must be UTF-8 hexadecimal sRGB".into()),
        }
    }
    let request = ColorNamingRequest {
        mode: mode.unwrap_or_default(),
        colors,
    };
    request.validate()?;
    Ok(Some(Options {
        command: Command::Color(request),
        engine: engine.unwrap_or(
            std::env::current_exe()?
                .with_file_name(format!("spiling-engine{}", std::env::consts::EXE_SUFFIX)),
        ),
        store,
    }))
}
