// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use crate::{Mesh, Result, check, invalid};
use std::sync::atomic::AtomicBool;

/// Binary is recognized by its exact record length, including a `solid` header.
/// No trusted normal or color convention is inferred from STL attributes.
pub fn decode(bytes: &[u8], cancel: &AtomicBool) -> Result<Mesh> {
    check(cancel)?;
    if bytes.len() >= 84 {
        let count = u32::from_le_bytes(bytes[80..84].try_into().unwrap()) as usize;
        if count.checked_mul(50).and_then(|n| n.checked_add(84)) == Some(bytes.len()) {
            return binary(bytes, count, cancel);
        }
    }
    ascii(bytes, cancel)
}
fn binary(bytes: &[u8], count: usize, cancel: &AtomicBool) -> Result<Mesh> {
    let vertex_count = count
        .checked_mul(3)
        .ok_or_else(|| invalid("STL count overflow"))?;
    u32::try_from(vertex_count).map_err(|_| invalid("STL indexed representation overflow"))?;
    let mut mesh = Mesh::default();
    mesh.vertices
        .try_reserve_exact(vertex_count)
        .map_err(|_| invalid("STL vertex allocation failed"))?;
    mesh.triangles
        .try_reserve_exact(count)
        .map_err(|_| invalid("STL triangle allocation failed"))?;
    for (i, record) in bytes[84..].as_chunks::<50>().0.iter().enumerate() {
        if i % 1024 == 0 {
            check(cancel)?;
        }
        let first = mesh.vertices.len() as u32;
        for vertex in record[12..48].as_chunks::<12>().0 {
            let mut p = [0.; 3];
            for (a, v) in vertex.as_chunks::<4>().0.iter().enumerate() {
                p[a] = f32::from_le_bytes(*v) as f64;
            }
            mesh.vertices.push(p);
        }
        mesh.triangles.push([first, first + 1, first + 2]);
    }
    mesh.validate()?;
    Ok(mesh)
}
fn ascii(bytes: &[u8], cancel: &AtomicBool) -> Result<Mesh> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| invalid("STL is neither valid binary nor UTF-8 ASCII"))?;
    // Only solid names are line-delimited. Facet grammar accepts arbitrary
    // whitespace; a parser must not silently scrape unrelated vertex tokens.
    let mut tokens = text.lines().flat_map(|line| {
        let limit = if matches!(line.split_whitespace().next(), Some("solid" | "endsolid")) {
            1
        } else {
            usize::MAX
        };
        line.split_whitespace().take(limit)
    });
    let mut mesh = Mesh::default();
    let mut in_solid = false;
    let mut saw_end = false;
    while let Some(word) = tokens.next() {
        check(cancel)?;
        match word {
            "solid" if !in_solid => {
                in_solid = true;
                saw_end = false;
            }
            "endsolid" if in_solid => {
                in_solid = false;
                saw_end = true;
            }
            "facet" if in_solid => {
                expect(&mut tokens, "normal")?;
                for _ in 0..3 {
                    number(tokens.next())?;
                }
                expect(&mut tokens, "outer")?;
                expect(&mut tokens, "loop")?;
                let first = u32::try_from(mesh.vertices.len())
                    .map_err(|_| invalid("STL index overflow"))?;
                first
                    .checked_add(3)
                    .ok_or_else(|| invalid("STL index overflow"))?;
                for _ in 0..3 {
                    expect(&mut tokens, "vertex")?;
                    mesh.vertices.push([
                        number(tokens.next())?,
                        number(tokens.next())?,
                        number(tokens.next())?,
                    ]);
                }
                expect(&mut tokens, "endloop")?;
                expect(&mut tokens, "endfacet")?;
                mesh.triangles.push([first, first + 1, first + 2]);
            }
            _ => return Err(invalid(format!("unexpected ASCII STL token {word}"))),
        }
    }
    if in_solid || !saw_end {
        return Err(invalid("incomplete ASCII STL"));
    }
    mesh.validate()?;
    Ok(mesh)
}
fn expect<'a>(tokens: &mut impl Iterator<Item = &'a str>, expected: &str) -> Result<()> {
    if tokens.next() == Some(expected) {
        Ok(())
    } else {
        Err(invalid(format!("ASCII STL expected {expected}")))
    }
}
fn number(token: Option<&str>) -> Result<f64> {
    let v = token
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|v| v.is_finite());
    v.ok_or_else(|| invalid("invalid finite STL number"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn binary_solid_header_and_truncation() {
        let mut bytes = vec![0; 134];
        bytes[..5].copy_from_slice(b"solid");
        bytes[80..84].copy_from_slice(&1u32.to_le_bytes());
        let c = AtomicBool::new(false);
        assert_eq!(decode(&bytes, &c).unwrap().triangles.len(), 1);
        assert!(decode(&bytes[..133], &c).is_err());
        bytes[96..100].copy_from_slice(&f32::NAN.to_le_bytes());
        assert!(decode(&bytes, &c).is_err());
    }
    #[test]
    fn ascii_grammar_not_vertex_scraping() {
        let c = AtomicBool::new(false);
        let valid=b"solid part name\nfacet normal 0 0 1\nouter loop\nvertex 0 0 0\nvertex 1 0 0\nvertex 0 1 0\nendloop\nendfacet\nendsolid part name\n";
        assert_eq!(decode(valid, &c).unwrap().vertices[1], [1., 0., 0.]);
        assert!(decode(b"solid x\nvertex 0 0 0\nendsolid", &c).is_err());
    }
}
