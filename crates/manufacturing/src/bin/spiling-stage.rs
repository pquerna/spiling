// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_manufacturing::{WorkingSet, replay_realization_stage, replay_support_stage};
use std::{path::Path, sync::atomic::AtomicBool};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    const USAGE: &str = "usage: spiling-stage [--realization] BLOB [BYTES]";
    let first = args.next().ok_or(USAGE)?;
    let realization = first == "--realization";
    let input = if realization {
        args.next().ok_or(USAGE)?
    } else {
        first
    };
    let bytes = args.next().map_or(Ok(8 * 1024 * 1024 * 1024usize), |s| {
        s.to_str()
            .ok_or("invalid byte allowance")?
            .parse::<usize>()
            .map_err(|_| "invalid byte allowance")
    })?;
    if args.next().is_some() {
        return Err(USAGE.into());
    }
    let allowance = WorkingSet::new(bytes)?;
    let cancel = AtomicBool::new(false);
    let report = if realization {
        replay_realization_stage(Path::new(&input), allowance, &cancel)?
    } else {
        replay_support_stage(Path::new(&input), allowance, &cancel)?
    };
    println!("{report}");
    Ok(())
}
