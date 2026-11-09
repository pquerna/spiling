// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;

/// Classified centerline domain retained independently of proposed sparse roads.
/// Includes holes and disconnected components; it conveys no load qualification.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WebRoofDomain {
    pub region_index: u16,
    #[serde(deserialize_with = "diaphragm::diaphragm_extent")]
    pub extent: Vec<DiaphragmPolygonMm>,
}
pub(super) fn domains<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<WebRoofDomain>, D::Error> {
    bounded_vec::<D, _, MAX_PROCESS_REGIONS>(d)
}
impl WebRoofDomain {
    pub fn allocated_bytes(&self) -> usize {
        self.extent.capacity() * std::mem::size_of::<DiaphragmPolygonMm>()
            + self
                .extent
                .iter()
                .map(|p| {
                    p.outer.capacity() * std::mem::size_of::<[f64; 2]>()
                        + p.holes.capacity() * std::mem::size_of::<Vec<[f64; 2]>>()
                        + p.holes
                            .iter()
                            .map(|h| h.capacity() * std::mem::size_of::<[f64; 2]>())
                            .sum::<usize>()
                })
                .sum::<usize>()
    }
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        if self.extent.is_empty() || self.extent.len() > MAX_PROCESS_REGIONS {
            return Err(invalid("web + roof requires bounded classified domains"));
        }
        let mut points = 0usize;
        for p in &self.extent {
            if p.holes.len() > MAX_PROCESS_REGIONS {
                return Err(resource("web + roof hole limit exceeded"));
            }
            for ring in std::iter::once(&p.outer).chain(&p.holes) {
                points = points
                    .checked_add(ring.len())
                    .ok_or_else(|| resource("web + roof point overflow"))?;
                if points > MAX_DEPOSITION_SEGMENTS as usize {
                    return Err(resource("web + roof domain point limit exceeded"));
                }
                if ring.len() < 4
                    || ring.first() != ring.last()
                    || ring.iter().flatten().any(|v| !v.is_finite())
                {
                    return Err(invalid(
                        "web + roof requires finite closed classified rings",
                    ));
                }
            }
        }
        Ok(())
    }
}
