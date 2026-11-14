// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;

/// Opt-in construction study. No mechanical law or strength credit is implied.
/// Piers, source walls and the complete roof-root corridor remain protected.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WebArchitecture {
    #[serde(deserialize_with = "required_option")]
    pub opening: Option<WebOpening>,
    #[serde(deserialize_with = "required_option")]
    pub pier_rib_depth_mm: Option<f64>,
}

/// A flat-bottom window with a linear supported closing cap. Heights address
/// absolute build Z, including printers whose bed is not at Z=0. Windows occur
/// only between two retained orthogonal web roots with sufficient pier reserve.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WebOpening {
    pub bottom_z_mm: f64,
    pub height_mm: f64,
    pub width_mm: f64,
    pub cap_height_mm: f64,
    pub pier_width_mm: f64,
}
impl WebArchitecture {
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        if self.opening.is_none() && self.pier_rib_depth_mm.is_none() {
            return Err(invalid("web architecture requires an opening or pier rib"));
        }
        if let Some(depth) = self.pier_rib_depth_mm {
            positive(&[depth])?;
        }
        if let Some(o) = self.opening {
            positive(&[o.height_mm, o.width_mm, o.cap_height_mm, o.pier_width_mm])?;
            if !o.bottom_z_mm.is_finite()
                || !(o.bottom_z_mm + o.height_mm).is_finite()
                || o.cap_height_mm > o.height_mm
                || o.width_mm > o.cap_height_mm
            {
                return Err(invalid(
                    "web opening requires finite heights and a closing advance no greater than 0.5 mm per mm rise",
                ));
            }
        }
        Ok(())
    }
}

/// Classified centerline domain retained independently of proposed sparse roads.
/// Includes holes and disconnected components; it conveys no load qualification.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WebRoofDomain {
    pub region_index: u16,
    #[serde(deserialize_with = "diaphragm::diaphragm_extent")]
    pub extent: Vec<DiaphragmPolygonMm>,
    /// Part-level XY bounds `[min, max]` that anchor the web lattice on every
    /// layer, so webs stack vertically through changing sections.
    pub frame_mm: [[f64; 2]; 2],
    /// Areas whose ceiling is lower than the part's top roof horizon, each with
    /// the height its corbels must close by.
    #[serde(deserialize_with = "ceilings")]
    pub ceilings: Vec<WebRoofCeiling>,
}

/// Material that ends above this layer: a step, pocket floor or hole underside.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WebRoofCeiling {
    pub roof_z_mm: f64,
    #[serde(deserialize_with = "diaphragm::diaphragm_extent")]
    pub extent: Vec<DiaphragmPolygonMm>,
}
fn ceilings<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<WebRoofCeiling>, D::Error> {
    bounded_vec::<D, _, MAX_PRINT_LAYERS_USIZE>(d)
}
const MAX_PRINT_LAYERS_USIZE: usize = MAX_PLAN_COUNT;
fn extent_bytes(extent: &[DiaphragmPolygonMm]) -> usize {
    extent
        .iter()
        .map(|p| {
            p.outer.capacity() * std::mem::size_of::<[f64; 2]>()
                + p.holes.capacity() * std::mem::size_of::<Vec<[f64; 2]>>()
                + p.holes
                    .iter()
                    .map(|h| h.capacity() * std::mem::size_of::<[f64; 2]>())
                    .sum::<usize>()
        })
        .sum()
}
fn validate_extent(
    extent: &[DiaphragmPolygonMm],
    points: &mut usize,
) -> Result<(), ManufacturingError> {
    for p in extent {
        if p.holes.len() > MAX_PLAN_COUNT {
            return Err(resource("web + roof hole limit exceeded"));
        }
        for ring in std::iter::once(&p.outer).chain(&p.holes) {
            *points = points
                .checked_add(ring.len())
                .ok_or_else(|| resource("web + roof point overflow"))?;
            if *points > MAX_PLAN_COUNT {
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
pub(super) fn domains<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Vec<WebRoofDomain>, D::Error> {
    bounded_vec::<D, _, MAX_PROCESS_REGIONS>(d)
}
impl WebRoofDomain {
    pub fn allocated_bytes(&self) -> usize {
        self.extent.capacity() * std::mem::size_of::<DiaphragmPolygonMm>()
            + extent_bytes(&self.extent)
            + self.ceilings.capacity() * std::mem::size_of::<WebRoofCeiling>()
            + self
                .ceilings
                .iter()
                .map(|c| {
                    c.extent.capacity() * std::mem::size_of::<DiaphragmPolygonMm>()
                        + extent_bytes(&c.extent)
                })
                .sum::<usize>()
    }
    pub fn validate(&self) -> Result<(), ManufacturingError> {
        if self.extent.is_empty() || self.extent.len() > MAX_PLAN_COUNT {
            return Err(invalid("web + roof requires bounded classified domains"));
        }
        if self.frame_mm.iter().flatten().any(|v| !v.is_finite())
            || (0..2).any(|axis| self.frame_mm[0][axis] >= self.frame_mm[1][axis])
        {
            return Err(invalid(
                "web + roof requires a finite positive lattice frame",
            ));
        }
        let mut points = 0usize;
        validate_extent(&self.extent, &mut points)?;
        for ceiling in &self.ceilings {
            if !ceiling.roof_z_mm.is_finite() || ceiling.extent.is_empty() {
                return Err(invalid(
                    "web + roof ceiling requires a finite height and area",
                ));
            }
            validate_extent(&ceiling.extent, &mut points)?;
        }
        Ok(())
    }
}
