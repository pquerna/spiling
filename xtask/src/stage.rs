// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::{Context, Result};
use std::{env, ffi::OsString, path::PathBuf, process::Command};

const USAGE: &str = "cargo xtask stage capture -- CLI [CLI args...]\ncargo xtask stage run BLOB [--runner SPILING-STAGE]";

pub(crate) fn execute(context: &Context, args: &[OsString]) -> Result<()> {
    match args {
        [command, separator, cli, rest @ ..] if command == "capture" && separator == "--" => {
            let home = env::var_os("HOME").ok_or("HOME is not configured")?;
            let dir = PathBuf::from(home).join("work/cache/stages");
            context.runner.run(
                Command::new(cli)
                    .args(rest)
                    .env("SPILING_STAGE_CAPTURE_DIR", dir),
            )
        }
        [command, blob] if command == "run" => {
            let runner = context
                .target
                .join("profiling")
                .join(format!("spiling-stage{}", env::consts::EXE_SUFFIX));
            if !runner.is_file() {
                return Err(
                    "build spiling-stage for the profiling profile or pass --runner".into(),
                );
            }
            context.runner.run(Command::new(runner).arg(blob))
        }
        [command, blob, flag, runner] if command == "run" && flag == "--runner" => {
            context.runner.run(Command::new(runner).arg(blob))
        }
        _ => Err(USAGE.into()),
    }
}
