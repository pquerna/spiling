// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! The stored manufacturing bundle: its coordinates as packed binary columns,
//! then the JSON envelope with each coordinate run replaced by its length.
//!
//! ```text
//! b"SPMB" | revision u32 | xyz count u64 | xy count u64
//! | x[] y[] z[] (f64 LE, one column each) | u[] v[] (f64 LE) | JSON
//! ```
//!
//! Path points (`DepositionPath::points_mm`) form the xyz columns and polygon
//! rings (`DiaphragmPolygonMm`) the xy columns, both in document order. Only
//! [`encode_bundle`] and [`decode_bundle_from`](super::decode_bundle_from)
//! switch the envelope to lengths; every other serialization (exports, the
//! protocol, tests) writes the coordinates inline as before. Columns are exact
//! f64 bits, so a decoded bundle equals the encoded one.

use super::{ManufacturingBundle, ManufacturingError, invalid, resource};
use serde::{Serialize, Serializer, de::Error};
use std::cell::RefCell;
use std::io::{self, Read, Write};

const MAGIC: &[u8; 4] = b"SPMB";
const REVISION: u32 = 1;
const HEADER_BYTES: u64 = 4 + 4 + 8 + 8;

enum Mode {
    /// First encoding pass: gather every run into the columns.
    Collect(Columns),
    /// Second encoding pass: write lengths, counting them back.
    Lengths { xyz: u64, xy: u64 },
    /// Decoding: hand out the columns in document order.
    Take(Source),
}

#[derive(Default)]
struct Columns {
    xyz: Vec<[f64; 3]>,
    xy: Vec<[f64; 2]>,
}

struct Source {
    xyz: [Vec<f64>; 3],
    xy: [Vec<f64>; 2],
    next_xyz: usize,
    next_xy: usize,
}

thread_local! {
    static MODE: RefCell<Option<Mode>> = const { RefCell::new(None) };
}

/// Restores inline coordinates when the scope ends, including on unwind.
struct Scope;
impl Scope {
    fn enter(mode: Mode) -> Self {
        MODE.with(|cell| {
            let mut slot = cell.borrow_mut();
            assert!(slot.is_none(), "bundle column scopes do not nest");
            *slot = Some(mode);
        });
        Scope
    }
    fn exit(self) -> Mode {
        let mode = MODE.with(|cell| cell.borrow_mut().take());
        std::mem::forget(self);
        mode.expect("bundle column scope is active")
    }
}
impl Drop for Scope {
    fn drop(&mut self) {
        MODE.with(|cell| cell.borrow_mut().take());
    }
}

/// Whether the deserializer reads lengths in place of coordinate runs.
pub(super) fn packed() -> bool {
    MODE.with(|cell| matches!(*cell.borrow(), Some(Mode::Take(_))))
}

fn run<S: Serializer, T: Serialize>(
    values: &[T],
    s: S,
    collect: impl FnOnce(&mut Columns),
    count: impl FnOnce(&mut u64, &mut u64),
) -> Result<S::Ok, S::Error> {
    let packed = MODE.with(|cell| match &mut *cell.borrow_mut() {
        Some(Mode::Collect(columns)) => {
            collect(columns);
            true
        }
        Some(Mode::Lengths { xyz, xy }) => {
            count(xyz, xy);
            true
        }
        Some(Mode::Take(_)) | None => false,
    });
    if packed {
        s.serialize_u64(values.len() as u64)
    } else {
        values.serialize(s)
    }
}

/// `serialize_with` for a path's points.
pub(super) fn xyz<S: Serializer>(points: &[[f64; 3]], s: S) -> Result<S::Ok, S::Error> {
    run(
        points,
        s,
        |columns| columns.xyz.extend_from_slice(points),
        |xyz, _| *xyz += points.len() as u64,
    )
}

/// `serialize_with` for one polygon ring.
pub(super) fn xy<S: Serializer>(ring: &[[f64; 2]], s: S) -> Result<S::Ok, S::Error> {
    run(
        ring,
        s,
        |columns| columns.xy.extend_from_slice(ring),
        |_, xy| *xy += ring.len() as u64,
    )
}

/// `serialize_with` for a polygon's holes: each ring packs on its own.
pub(super) fn xy_rings<S: Serializer>(rings: &[Vec<[f64; 2]>], s: S) -> Result<S::Ok, S::Error> {
    struct Ring<'a>(&'a [[f64; 2]]);
    impl Serialize for Ring<'_> {
        fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            xy(self.0, s)
        }
    }
    s.collect_seq(rings.iter().map(|ring| Ring(ring)))
}

fn take<E: Error, T>(pick: impl FnOnce(&mut Source) -> Option<T>) -> Result<T, E> {
    MODE.with(|cell| match &mut *cell.borrow_mut() {
        Some(Mode::Take(source)) => pick(source),
        _ => None,
    })
    .ok_or_else(|| E::custom("bundle coordinate columns are exhausted"))
}

/// The next path point from the xyz columns.
pub(super) fn take_xyz<E: Error>() -> Result<[f64; 3], E> {
    take(|source| {
        let i = source.next_xyz;
        let [x, y, z] = &source.xyz;
        let point = [*x.get(i)?, *y.get(i)?, *z.get(i)?];
        source.next_xyz += 1;
        Some(point)
    })
}

/// The next ring vertex from the xy columns.
pub(super) fn take_xy<E: Error>() -> Result<[f64; 2], E> {
    take(|source| {
        let i = source.next_xy;
        let [u, v] = &source.xy;
        let point = [*u.get(i)?, *v.get(i)?];
        source.next_xy += 1;
        Some(point)
    })
}

fn remaining(pick: impl FnOnce(&Source) -> usize) -> u64 {
    MODE.with(|cell| match &*cell.borrow() {
        Some(Mode::Take(source)) => pick(source) as u64,
        _ => 0,
    })
}

/// Path points not yet taken from the xyz columns.
pub(super) fn xyz_remaining() -> u64 {
    remaining(|source| source.xyz[0].len() - source.next_xyz)
}

/// A ring written as its length, read back from the xy columns.
pub(super) fn packed_ring<'de, D: serde::Deserializer<'de>>(
    d: D,
    limit: usize,
) -> Result<Vec<[f64; 2]>, D::Error> {
    let count = <u64 as serde::Deserialize>::deserialize(d)?;
    let count = usize::try_from(count)
        .ok()
        .filter(|count| *count <= limit)
        .ok_or_else(|| D::Error::custom("polygon ring limit exceeded"))?;
    if count as u64 > remaining(|source| source.xy[0].len() - source.next_xy) {
        return Err(D::Error::custom("bundle coordinate columns are exhausted"));
    }
    (0..count).map(|_| take_xy()).collect()
}

fn corrupt(message: impl Into<String>) -> ManufacturingError {
    ManufacturingError::new(
        super::ManufacturingErrorCode::CorruptArtifact,
        message.into(),
    )
}

/// Encode the stored bundle. The envelope is serialized twice: once to
/// gather the columns, which precede it, and once to write it.
pub fn encode_bundle(bundle: &ManufacturingBundle, out: &mut impl Write) -> io::Result<()> {
    let counts = write_columns(bundle, out)?;
    let scope = Scope::enter(Mode::Lengths { xyz: 0, xy: 0 });
    serde_json::to_writer(&mut *out, bundle)?;
    check_lengths(scope, counts)
}

fn check_lengths(scope: Scope, counts: (u64, u64)) -> io::Result<()> {
    let Mode::Lengths { xyz, xy } = scope.exit() else {
        unreachable!("length scope returns its counts");
    };
    if (xyz, xy) != counts {
        return Err(io::Error::other(
            "bundle serialization is not deterministic",
        ));
    }
    Ok(())
}

/// Write the header and columns; returns the xyz and xy counts.
fn write_columns(bundle: &ManufacturingBundle, out: &mut impl Write) -> io::Result<(u64, u64)> {
    let scope = Scope::enter(Mode::Collect(Columns::default()));
    serde_json::to_writer(io::sink(), bundle)?;
    let Mode::Collect(columns) = scope.exit() else {
        unreachable!("collect scope returns its columns");
    };
    let counts = (columns.xyz.len() as u64, columns.xy.len() as u64);
    out.write_all(MAGIC)?;
    out.write_all(&REVISION.to_le_bytes())?;
    out.write_all(&counts.0.to_le_bytes())?;
    out.write_all(&counts.1.to_le_bytes())?;
    let mut buffer = Vec::with_capacity(64 * 1024);
    let mut column = |values: &mut dyn Iterator<Item = f64>| -> io::Result<()> {
        for value in values {
            buffer.extend_from_slice(&value.to_le_bytes());
            if buffer.len() == buffer.capacity() {
                out.write_all(&buffer)?;
                buffer.clear();
            }
        }
        out.write_all(&buffer)?;
        buffer.clear();
        Ok(())
    };
    for axis in 0..3 {
        column(&mut columns.xyz.iter().map(|point| point[axis]))?;
    }
    for axis in 0..2 {
        column(&mut columns.xy.iter().map(|point| point[axis]))?;
    }
    Ok(counts)
}

/// A stored bundle split into its header and columns and its envelope as a
/// JSON value, so tests can edit the envelope; concatenate the prefix and
/// the edited value's compact JSON to store it again.
#[cfg(test)]
pub(super) fn packed_value(bundle: &ManufacturingBundle) -> (Vec<u8>, serde_json::Value) {
    let mut prefix = Vec::new();
    let counts = write_columns(bundle, &mut prefix).unwrap();
    let scope = Scope::enter(Mode::Lengths { xyz: 0, xy: 0 });
    let value = serde_json::to_value(bundle).unwrap();
    check_lengths(scope, counts).unwrap();
    (prefix, value)
}

/// The column counts from a stored bundle's header.
fn read_header(reader: &mut impl Read, len: u64) -> Result<(usize, usize), ManufacturingError> {
    let mut header = [0u8; HEADER_BYTES as usize];
    reader
        .read_exact(&mut header)
        .map_err(|_| corrupt("manufacturing bundle header is truncated"))?;
    if &header[..4] != MAGIC {
        return Err(corrupt(
            "manufacturing bundle is not in the stored encoding; recompile the plan",
        ));
    }
    let word = |at: usize| u64::from_le_bytes(header[at..at + 8].try_into().unwrap());
    if u32::from_le_bytes(header[4..8].try_into().unwrap()) != REVISION {
        return Err(invalid("unsupported manufacturing bundle revision"));
    }
    let (xyz, xy) = (word(8), word(16));
    let column_bytes = xyz
        .checked_mul(24)
        .zip(xy.checked_mul(16))
        .and_then(|(a, b)| a.checked_add(b))
        .and_then(|bytes| bytes.checked_add(HEADER_BYTES));
    match column_bytes {
        Some(bytes) if bytes < len => Ok((xyz as usize, xy as usize)),
        _ => Err(resource("bundle coordinate columns exceed the bundle")),
    }
}

fn read_column(reader: &mut impl Read, count: usize) -> Result<Vec<f64>, ManufacturingError> {
    let mut values = Vec::new();
    values
        .try_reserve_exact(count)
        .map_err(|_| resource("bundle coordinate allocation refused"))?;
    let mut buffer = [0u8; 64 * 1024];
    let mut remaining = count;
    while remaining > 0 {
        let take = remaining.min(buffer.len() / 8);
        let bytes = &mut buffer[..take * 8];
        reader
            .read_exact(bytes)
            .map_err(|_| corrupt("bundle coordinate columns are truncated"))?;
        for chunk in bytes.as_chunks::<8>().0 {
            let value = f64::from_le_bytes(*chunk);
            if !value.is_finite() {
                return Err(corrupt("bundle coordinate is not finite"));
            }
            values.push(value);
        }
        remaining -= take;
    }
    Ok(values)
}

/// Skip a stored bundle's header and columns, leaving `reader` at its JSON.
pub(super) fn skip_columns(reader: &mut impl Read, len: u64) -> Result<(), ManufacturingError> {
    let (xyz, xy) = read_header(reader, len)?;
    let bytes = xyz as u64 * 24 + xy as u64 * 16;
    let skipped = io::copy(&mut reader.take(bytes), &mut io::sink())
        .map_err(|_| corrupt("bundle coordinate columns are truncated"))?;
    if skipped != bytes {
        return Err(corrupt("bundle coordinate columns are truncated"));
    }
    Ok(())
}

/// Read a stored bundle's columns, then run `parse` over its JSON with the
/// columns in scope. A successful parse must consume every column value.
pub(super) fn with_columns<R: Read, T, E>(
    mut reader: R,
    len: u64,
    parse: impl FnOnce(R) -> Result<T, E>,
) -> Result<Result<T, E>, ManufacturingError> {
    let (xyz, xy) = read_header(&mut reader, len)?;
    let mut column = || read_column(&mut reader, xyz);
    let xyz_columns = [column()?, column()?, column()?];
    let mut column = || read_column(&mut reader, xy);
    let xy_columns = [column()?, column()?];
    let scope = Scope::enter(Mode::Take(Source {
        xyz: xyz_columns,
        xy: xy_columns,
        next_xyz: 0,
        next_xy: 0,
    }));
    let parsed = parse(reader);
    let Mode::Take(source) = scope.exit() else {
        unreachable!("take scope returns its source");
    };
    if parsed.is_ok() && (source.next_xyz != xyz || source.next_xy != xy) {
        return Err(corrupt("bundle coordinate columns are not fully used"));
    }
    Ok(parsed)
}
