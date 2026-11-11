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
    remaining: usize,
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
    let limit = remaining.min(MAX_SOURCE_BYTES as usize);
    if before.len() > limit as u64 {
        return Err(error(
            GeometryErrorCode::ResourceLimit,
            format!("{label}: source bytes exceed remaining budget"),
        ));
    }
    let modified = before.modified().map_err(io_error)?;
    let mut bytes = Vec::new();
    bytes
        .try_reserve_exact(before.len() as usize)
        .map_err(|_| {
            error(
                GeometryErrorCode::ResourceLimit,
                format!("{label}: cannot allocate captured source"),
            )
        })?;
    if bytes.capacity() > limit {
        return Err(error(
            GeometryErrorCode::ResourceLimit,
            "captured source capacity exceeds admission",
        ));
    }
    let mut reader = (&file).take(limit as u64 + 1);
    let mut block = [0; 64 * 1024];
    loop {
        check_cancel(cancel)?;
        let count = reader.read(&mut block).map_err(io_error)?;
        if count == 0 {
            break;
        }
        if bytes.len() + count > limit {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                format!("{label}: source bytes exceed remaining budget"),
            ));
        }
        if bytes.len() + count > before.len() as usize {
            return Err(error(
                GeometryErrorCode::SourceChanged,
                format!("{label}: source grew during capture"),
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
