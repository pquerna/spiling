// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use spiling_contracts::geometry::NativePath;
use std::ffi::OsString;
#[derive(Debug, Clone)]
pub enum Open {
    Project { path: NativePath, read_only: bool },
    Import { path: NativePath },
}
/// Native OS/shell arguments authorize one ordinary Open or Import action.
pub fn parse(arguments: impl IntoIterator<Item = OsString>) -> Result<Option<Open>, String> {
    let mut args = arguments.into_iter();
    let mut path = None;
    let mut read_only = false;
    let mut import = false;
    while let Some(arg) = args.next() {
        if arg == "--read-only" {
            read_only = true;
        } else if arg == "--open" || arg == "--import" {
            if path.is_some() {
                return Err("Specify one project or source file".into());
            }
            import = arg == "--import";
            path = Some(args.next().ok_or("--open/--import needs a file")?);
        } else if arg.to_string_lossy().starts_with('-') {
            return Err(
                "Usage: spiling-workbench [--read-only] [--open] PROJECT.spil | --import SOURCE"
                    .into(),
            );
        } else if path.replace(arg).is_some() {
            return Err("Specify one project or source file".into());
        }
    }
    if read_only && import {
        return Err("--read-only applies to project opening, not source import".into());
    }
    let Some(path) = path else {
        if read_only {
            return Err("--read-only needs a .spil project file".into());
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
    let path = NativePath::from_os_str(path.as_os_str()).map_err(|e| e.to_string())?;
    Ok(Some(if import {
        Open::Import { path }
    } else {
        Open::Project { path, read_only }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(values: &[&str]) -> Result<Option<Open>, String> {
        parse(values.iter().map(OsString::from))
    }
    #[test]
    fn launch_actions_preserve_intent_and_reject_ambiguous_inputs() {
        assert!(matches!(
            args(&["--import", "part.stl"]).unwrap(),
            Some(Open::Import { .. })
        ));
        assert!(matches!(
            args(&["--read-only", "--open", "part.spil"]).unwrap(),
            Some(Open::Project {
                read_only: true,
                ..
            })
        ));
        for input in [
            &["--import", "a.stl", "--open", "b.spil"][..],
            &["--read-only", "--import", "a.stl"],
            &["--import"],
            &["--open", "a.spil", "b.spil"],
        ] {
            assert!(args(input).is_err(), "{input:?}");
        }
    }
}
