// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use spiling_contracts::{geometry::SourceHash, project::StoredDefinition};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

pub(crate) type Failure = Box<dyn std::error::Error>;

pub(crate) fn capture_dir(stage: &str) -> Option<PathBuf> {
    let requested = std::env::var("SPILING_STAGE_CAPTURE_STAGE");
    (requested.as_deref().unwrap_or("support") == stage)
        .then(|| std::env::var_os("SPILING_STAGE_CAPTURE_DIR"))
        .flatten()
        .map(PathBuf::from)
}

pub(crate) fn source_key(definitions: &[StoredDefinition]) -> Result<String, Failure> {
    let mut hashes: Vec<&SourceHash> = definitions
        .iter()
        .map(|d| &d.provenance.source_hash)
        .collect();
    hashes.sort_unstable();
    hashes.dedup();
    Ok(match hashes.as_slice() {
        [hash] => hash.as_str().to_owned(),
        [] => return Err("no source hashes for the stage input".into()),
        _ => digest(&hashes)?,
    })
}

pub(crate) struct HashWriter<W> {
    pub(crate) writer: W,
    pub(crate) hash: Sha256,
}

impl<W: Write> Write for HashWriter<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let len = self.writer.write(bytes)?;
        self.hash.update(&bytes[..len]);
        Ok(len)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

pub(crate) fn digest(value: &impl Serialize) -> Result<String, Failure> {
    let mut w = HashWriter {
        writer: io::sink(),
        hash: Sha256::new(),
    };
    serde_json::to_writer(&mut w, value)?;
    Ok(format!("{:x}", w.hash.finalize()))
}

pub(crate) fn write_blob(
    dir: &Path,
    producer: &str,
    source: &str,
    value: &impl Serialize,
) -> Result<PathBuf, Failure> {
    let path = dir.join(producer).join(source);
    fs::create_dir_all(&path)?;
    // A capture cannot publish a partial blob. Concurrent writers of the same
    // exact content converge on the same final name.
    static NEXT_CAPTURE: AtomicU64 = AtomicU64::new(0);
    let temp = path.join(format!(
        ".{}-{}.pending",
        std::process::id(),
        NEXT_CAPTURE.fetch_add(1, Ordering::Relaxed),
    ));
    let writer = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)?;
    let blob = (|| -> Result<PathBuf, Failure> {
        let mut writer = HashWriter {
            writer: BufWriter::new(writer),
            hash: Sha256::new(),
        };
        serde_json::to_writer(&mut writer, value)?;
        writer.flush()?;
        let blob = path.join(format!("{:x}.json", writer.hash.finalize()));
        fs::rename(&temp, &blob)?;
        Ok(blob)
    })();
    if blob.is_err() {
        let _ = fs::remove_file(&temp);
    }
    blob
}

pub(crate) fn read_blob<T: DeserializeOwned>(
    path: &Path,
    working_set: crate::WorkingSet,
) -> Result<T, Failure> {
    use io::{Read, Seek};
    let mut file = File::open(path)?;
    // Local dev artifacts are input, not an import or manufacturing authority.
    // JSON decoding needs extra capacity for nested structs and grown vectors.
    let limit = u64::try_from(working_set.bytes() / 16)?;
    let size = file.metadata()?.len();
    if size > limit {
        return Err("stage blob exceeds replay decode admission".into());
    }
    // Check the content address in a streaming pass, then decode the same file.
    file.rewind()?;
    let mut writer = HashWriter {
        writer: io::sink(),
        hash: Sha256::new(),
    };
    io::copy(&mut (&mut file).take(size), &mut writer)?;
    let name = format!("{:x}.json", writer.hash.finalize());
    if path.file_name().and_then(|name| name.to_str()) != Some(name.as_str()) {
        return Err("stage blob content address mismatch".into());
    }
    file.rewind()?;
    Ok(serde_json::from_reader(BufReader::new(file.take(size)))?)
}
