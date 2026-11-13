// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;
impl SurfaceComplex {
    /// Explicit packed f64/u32 arrays; bounded patch metadata is independent of
    /// the triangle inventory. No arbitrary Rust layout or JSON mesh arrays.
    pub fn encode(&self) -> Result<Vec<u8>, GeometryError> {
        self.validate()?;
        let metadata = serde_json::to_vec(&self.faces).map_err(|e| invalid(e.to_string()))?;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"SPSC\x01\0\0\0");
        for count in [self.positions.len(), self.triangles.len(), metadata.len()] {
            bytes.extend_from_slice(
                &u64::try_from(count)
                    .map_err(|_| invalid("surface complex size overflow"))?
                    .to_le_bytes(),
            );
        }
        bytes.extend_from_slice(&self.error_mm.to_le_bytes());
        for p in &self.positions {
            for v in p {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
        }
        for (t, r) in self.triangles.iter().zip(&self.regions) {
            for v in t {
                bytes.extend_from_slice(&v.to_le_bytes());
            }
            bytes.extend_from_slice(&r.to_le_bytes());
        }
        bytes.extend_from_slice(&metadata);
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, GeometryError> {
        if bytes.len() < 40 || &bytes[..8] != b"SPSC\x01\0\0\0" {
            return Err(invalid("unknown normalized source representation"));
        }
        let count = |offset| {
            usize::try_from(u64::from_le_bytes(
                bytes[offset..offset + 8].try_into().unwrap(),
            ))
            .map_err(|_| invalid("surface complex count overflow"))
        };
        let (vertices, triangles, meta) = (count(8)?, count(16)?, count(24)?);
        let vertex_end = vertices
            .checked_mul(24)
            .and_then(|v| v.checked_add(40))
            .ok_or_else(|| invalid("surface complex vertex overflow"))?;
        let triangle_end = triangles
            .checked_mul(16)
            .and_then(|v| v.checked_add(vertex_end))
            .ok_or_else(|| invalid("surface complex triangle overflow"))?;
        if triangle_end.checked_add(meta) != Some(bytes.len()) {
            return Err(invalid("surface complex lengths disagree"));
        }
        let mut positions = Vec::new();
        positions
            .try_reserve_exact(vertices)
            .map_err(|_| invalid("surface complex allocation failed"))?;
        for p in bytes[40..vertex_end].as_chunks::<24>().0 {
            positions.push(std::array::from_fn(|a| {
                f64::from_le_bytes(p[a * 8..a * 8 + 8].try_into().unwrap())
            }));
        }
        let mut ts = Vec::new();
        let mut regions = Vec::new();
        ts.try_reserve_exact(triangles)
            .map_err(|_| invalid("surface complex allocation failed"))?;
        regions
            .try_reserve_exact(triangles)
            .map_err(|_| invalid("surface complex allocation failed"))?;
        for row in bytes[vertex_end..triangle_end].as_chunks::<16>().0 {
            ts.push(std::array::from_fn(|a| {
                u32::from_le_bytes(row[a * 4..a * 4 + 4].try_into().unwrap())
            }));
            regions.push(u32::from_le_bytes(row[12..16].try_into().unwrap()));
        }
        let result = Self {
            positions,
            triangles: ts,
            regions,
            faces: serde_json::from_slice(&bytes[triangle_end..])
                .map_err(|e| invalid(e.to_string()))?,
            error_mm: f64::from_le_bytes(bytes[32..40].try_into().unwrap()),
        };
        result.validate()?;
        Ok(result)
    }
}
