// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_manufacturing::{WorkingSet, replay_support_stage};
use std::{path::Path, sync::atomic::AtomicBool};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let input = args.next().ok_or("usage: spiling-stage BLOB [BYTES]")?;
    let bytes = args.next().map_or(Ok(8 * 1024 * 1024 * 1024usize), |s| {
        s.to_str()
            .ok_or("invalid byte allowance")?
            .parse::<usize>()
            .map_err(|_| "invalid byte allowance")
    })?;
    if args.next().is_some() {
        return Err("usage: spiling-stage BLOB [BYTES]".into());
    }
    let report = replay_support_stage(
        Path::new(&input),
        WorkingSet::new(bytes)?,
        &AtomicBool::new(false),
    )?;
    println!("{report}");
    Ok(())
}
