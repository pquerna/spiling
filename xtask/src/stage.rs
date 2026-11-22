// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::{Context, Result};
use std::{env, ffi::OsString, path::PathBuf, process::Command};

const USAGE: &str = "cargo xtask stage capture [support|realization] -- CLI [CLI args...]\ncargo xtask stage run [support|realization] BLOB [--runner SPILING-STAGE]";

pub(crate) fn execute(context: &Context, args: &[OsString]) -> Result<()> {
    let Some((command, args)) = args.split_first() else {
        return Err(USAGE.into());
    };
    let (stage, args) = match args {
        [stage, rest @ ..] if stage == "support" => ("support", rest),
        [stage, rest @ ..] if stage == "realization" => ("realization", rest),
        _ => ("support", args),
    };
    match args {
        [separator, cli, rest @ ..] if command == "capture" && separator == "--" => {
            let home = env::var_os("HOME").ok_or("HOME is not configured")?;
            let dir = PathBuf::from(home).join("work/cache/stages");
            context.runner.run(
                Command::new(cli)
                    .args(rest)
                    .env("SPILING_STAGE_CAPTURE_DIR", dir)
                    .env("SPILING_STAGE_CAPTURE_STAGE", stage),
            )
        }
        [blob, rest @ ..] if command == "run" => {
            let runner = match rest {
                [] => context
                    .target
                    .join("profiling")
                    .join(format!("spiling-stage{}", env::consts::EXE_SUFFIX)),
                [flag, runner] if flag == "--runner" => PathBuf::from(runner),
                _ => return Err(USAGE.into()),
            };
            if !runner.is_file() {
                return Err(
                    "build spiling-stage for the profiling profile or pass its path with --runner"
                        .into(),
                );
            }
            let mut cmd = Command::new(runner);
            if stage == "realization" {
                cmd.arg("--realization");
            }
            context.runner.run(cmd.arg(blob))
        }
        _ => Err(USAGE.into()),
    }
}
