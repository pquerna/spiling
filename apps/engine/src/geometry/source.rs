// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{check_cancel, error};
use spiling_contracts::geometry::*;
use std::{fs::OpenOptions, io::Read, path::PathBuf, sync::atomic::AtomicBool};

pub struct Captured {
    pub bytes: Vec<u8>,
    pub label: String,
}

pub fn capture(
    source: NativePath,
    remaining: u32,
    cancel: &AtomicBool,
) -> Result<Captured, GeometryError> {
    check_cancel(cancel)?;
    let path = PathBuf::from(source.to_os_string()?);
    let label = bounded_text(
        &path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy(),
        MAX_DISPLAY_LABEL_BYTES as usize,
    );
    let io_error = |e: std::io::Error| error(GeometryErrorCode::SourceIo, format!("{label}: {e}"));
    let mut options = OpenOptions::new();
    options.read(true);
    // A path replaced by a FIFO must not trap the worker in open before it can inspect the handle.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options.open(&path).map_err(io_error)?;
    let before = file.metadata().map_err(io_error)?;
    if !before.is_file() {
        return Err(error(
            GeometryErrorCode::SourceIo,
            format!("{label}: source is not a regular file"),
        ));
    }
    let limit = remaining.min(MAX_SOURCE_BYTES);
    if before.len() > u64::from(limit) {
        return Err(error(
            GeometryErrorCode::ResourceLimit,
            format!("{label}: source bytes exceed remaining budget"),
        ));
    }
    let modified = before.modified().map_err(io_error)?;
    let mut bytes = Vec::with_capacity(before.len() as usize);
    let mut reader = (&file).take(u64::from(limit) + 1);
    let mut block = [0; 64 * 1024];
    loop {
        check_cancel(cancel)?;
        let count = reader.read(&mut block).map_err(io_error)?;
        if count == 0 {
            break;
        }
        if bytes.len() + count > limit as usize {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                format!("{label}: source bytes exceed remaining budget"),
            ));
        }
        bytes.extend_from_slice(&block[..count]);
    }
    let after = file.metadata().map_err(io_error)?;
    if before.len() != after.len()
        || bytes.len() as u64 != after.len()
        || modified != after.modified().map_err(io_error)?
    {
        return Err(error(
            GeometryErrorCode::SourceChanged,
            format!("{label}: source changed during capture"),
        ));
    }
    check_cancel(cancel)?;
    Ok(Captured { bytes, label })
}
