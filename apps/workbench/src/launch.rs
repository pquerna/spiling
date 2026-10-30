// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_contracts::geometry::NativePath;
use std::ffi::OsString;
#[derive(Debug, Clone)]
pub struct Open {
    pub path: NativePath,
    pub read_only: bool,
}
/// Native OS/shell arguments preserve platform path encoding and authorize one Open.
pub fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Option<Open>, String> {
    let mut args = arguments.into_iter();
    let mut path = None;
    let mut read_only = false;
    while let Some(arg) = args.next() {
        if arg == "--read-only" {
            read_only = true;
        } else if arg == "--open" {
            if path.is_some() {
                return Err("Specify one project directory".into());
            }
            path = Some(args.next().ok_or("--open needs a project directory")?);
        } else if arg.to_string_lossy().starts_with('-') {
            return Err("Usage: spiling-workbench [--read-only] [--open] PROJECT_DIRECTORY".into());
        } else if path.replace(arg).is_some() {
            return Err("Specify one project directory".into());
        }
    }
    let Some(path) = path else {
        if read_only {
            return Err("--read-only needs a project directory".into());
        }
        return Ok(None);
    };
    let path = std::path::PathBuf::from(path);
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    Ok(Some(Open {
        path: NativePath::from_os_str(path.as_os_str()).map_err(|e| e.to_string())?,
        read_only,
    }))
}
