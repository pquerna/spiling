// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Projection of validated SPLR6 events, never a machine-program parser.
use glam::DVec3;
use spiling_contracts::manufacturing::*;
use std::{collections::BTreeMap, ops::Range};

#[repr(C)]
#[derive(Debug, Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub color: [f32; 3],
}
#[derive(Debug)]
pub struct Batch {
    pub kind: u8,
    pub tool: u32,
    pub points: bool,
    pub vertices: Vec<Vertex>,
    pub events: Vec<(u32, u16)>,
}
#[derive(Debug)]
pub struct Replay {
    pub origin: DVec3,
    pub batches: Vec<Batch>,
    pub owned_bytes: usize,
}
#[derive(Debug, Clone, Copy)]
pub struct View {
    pub layer: Option<u16>,
    pub events: u32,
    pub travels: bool,
    pub source: bool,
    pub visible: bool,
    pub material: bool,
}
impl Default for View {
    fn default() -> Self {
        Self {
            layer: None,
            events: u32::MAX,
            travels: true,
            source: false,
            visible: true,
            material: false,
        }
    }
}
pub const ROLES: [&str; 16] = [
    "Travel",
    "Perimeter",
    "Solid",
    "Bottom skin",
    "Top skin",
    "Sparse",
    "Lifecycle travel",
    "Prime",
    "Bridge",
    "Support",
    "Support interface",
    "Retract",
    "Unretract",
    "Separator wall",
    "Tool epoch",
    "Waste",
];
const COLORS: [[u8; 3]; 16] = [
    [142, 172, 189],
    [255, 187, 102],
    [108, 222, 199],
    [182, 147, 255],
    [255, 121, 155],
    [142, 208, 90],
    [123, 145, 235],
    [255, 240, 122],
    [255, 147, 74],
    [98, 162, 255],
    [179, 229, 255],
    [226, 96, 134],
    [246, 184, 206],
    [245, 239, 68],
    [255, 119, 183],
    [215, 167, 255],
];
impl Batch {
    pub fn range(&self, view: View) -> Range<u32> {
        if !view.visible || (!view.travels && matches!(self.kind, 0 | 6)) {
            return 0..0;
        }
        let start = if let Some(layer) = view.layer {
            self.events.partition_point(|(_, l)| *l < layer)
        } else {
            0
        };
        let end = self.events.partition_point(|(event, layer)| {
            *event < view.events && view.layer.is_none_or(|l| *layer <= l)
        });
        let n = if self.points { 1 } else { 2 };
        (start * n) as u32..(end.max(start) * n) as u32
    }
}
impl Replay {
    pub fn project(
        bytes: &[u8],
        descriptor: &ReplayDisplayDescriptor,
        reserved: usize,
        origin: DVec3,
    ) -> Result<Self, String> {
        validate_replay_display(bytes, descriptor).map_err(|e| e.to_string())?;
        let count = descriptor.info.event_count as usize;
        // Raw input plus CPU vertices, GPU copy, event indices and a bounded staging copy.
        let upper = bytes.len() + count * (2 * std::mem::size_of::<Vertex>() * 3 + 8);
        if reserved
            .checked_add(upper)
            .is_none_or(|n| n > spiling_contracts::geometry::MAX_DISPLAY_BUFFER_BYTES as usize)
        {
            return Err("Replay exceeds the combined active/staged display budget".into());
        }
        let record = |i: usize| {
            &bytes[REPLAY_HEADER_BYTES + i * REPLAY_RECORD_BYTES
                ..REPLAY_HEADER_BYTES + (i + 1) * REPLAY_RECORD_BYTES]
        };
        let point = |i: usize| -> Result<[f32; 3], String> {
            let r = record(i);
            let p = DVec3::new(
                f64::from_le_bytes(r[0..8].try_into().unwrap()),
                f64::from_le_bytes(r[8..16].try_into().unwrap()),
                f64::from_le_bytes(r[16..24].try_into().unwrap()),
            ) - origin;
            let f = p.as_vec3();
            if (f.as_dvec3() - p).length() > 0.001 {
                return Err("Replay projection exceeds 0.001 mm".into());
            }
            Ok(f.to_array())
        };
        let mut counts = BTreeMap::<(u8, u32, bool), usize>::new();
        for i in 0..count {
            let r = record(i);
            let kind = r[46];
            if kind == 14 {
                continue;
            }
            let points = i == 0 || matches!(kind, 11 | 12);
            let tool = if descriptor.info.tool_epoch_count > 0 {
                u32::from_le_bytes(r[76..80].try_into().unwrap())
            } else {
                0
            };
            *counts.entry((kind, tool, points)).or_default() += 1;
        }
        let mut groups: BTreeMap<_, _> = counts
            .into_iter()
            .map(|((kind, tool, points), count)| {
                (
                    (kind, tool, points),
                    Batch {
                        kind,
                        tool,
                        points,
                        vertices: Vec::with_capacity(count * if points { 1 } else { 2 }),
                        events: Vec::with_capacity(count),
                    },
                )
            })
            .collect();
        for i in 0..count {
            let r = record(i);
            let kind = r[46];
            // The first endpoint is an unknown-start marker only in the single-tool stream.
            if kind == 14 || (i == 0 && descriptor.info.tool_epoch_count > 0) {
                continue;
            }
            let marker = i == 0;
            let points = marker || matches!(kind, 11 | 12);
            let tool = if descriptor.info.tool_epoch_count > 0 {
                u32::from_le_bytes(r[76..80].try_into().unwrap())
            } else {
                0
            };
            let batch = groups.entry((kind, tool, points)).or_insert_with(|| Batch {
                kind,
                tool,
                points,
                vertices: vec![],
                events: vec![],
            });
            let color = COLORS[kind as usize].map(|n| f32::from(n) / 255.0);
            if !points {
                batch.vertices.push(Vertex {
                    position: point(i - 1)?,
                    color,
                });
            }
            batch.vertices.push(Vertex {
                position: point(i)?,
                color,
            });
            batch
                .events
                .push((i as u32, u16::from_le_bytes(r[44..46].try_into().unwrap())));
        }
        let batches: Vec<_> = groups.into_values().collect();
        let owned_bytes = bytes.len()
            + batches
                .iter()
                .map(|b| {
                    b.vertices.capacity() * std::mem::size_of::<Vertex>() * 2
                        + b.events.capacity() * 8
                })
                .sum::<usize>();
        Ok(Self {
            origin,
            batches,
            owned_bytes,
        })
    }
}

/// Catalog tints from captured evidence, never a local mixture/color predictor.
pub fn declared_tool_colors(workflow: &JobWorkflow) -> Option<[[f32; 4]; 2]> {
    let JobWorkflow::FixedDualMarlin { job } = workflow else {
        return None;
    };
    let mut colors = [None, None];
    for construction in &job.surface_constructions {
        for capture in construction
            .optical_origin
            .iter()
            .flat_map(|origin| &origin.materials)
        {
            let tool = job
                .tools
                .iter()
                .position(|tool| tool.material.id == capture.loaded_material.id)?;
            let spiling_contracts::appearance::AppearanceEvidence::CatalogEstimate { srgb } =
                capture.evidence.evidence
            else {
                continue;
            };
            let color = [srgb[0] as f32, srgb[1] as f32, srgb[2] as f32, 1.0];
            if colors[tool].is_some_and(|old| old != color) {
                return None;
            }
            colors[tool] = Some(color);
        }
    }
    Some([colors[0]?, colors[1]?])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_golden_epochs_endpoints_ranges_and_budget() {
        for name in [
            "replay",
            "replay-sparse",
            "replay-fixed-dual",
            "replay-waste-only",
        ] {
            let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join(format!("../../fixtures/protocol/{name}.json"));
            let fixture: serde_json::Value =
                serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            let hex = fixture["data"].as_str().unwrap();
            let bytes: Vec<u8> = hex
                .as_bytes()
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap())
                .collect();
            let descriptor: ReplayDisplayDescriptor =
                serde_json::from_value(fixture["descriptor"].clone()).unwrap();
            let origin = DVec3::splat(0.125);
            let replay = Replay::project(&bytes, &descriptor, 0, origin).unwrap();
            assert!(Replay::project(&bytes, &descriptor, 128 * 1024 * 1024, origin).is_err());
            let all = View::default();
            for batch in &replay.batches {
                assert_ne!(batch.kind, 14);
                assert_eq!(batch.range(all).len(), batch.vertices.len());
                for (j, (event, layer)) in batch.events.iter().enumerate() {
                    let n = if batch.points { 1 } else { 2 };
                    for k in 0..n {
                        let actual = if batch.points {
                            *event
                        } else {
                            event - 1 + k as u32
                        };
                        let expected = &fixture["actual_positions"][actual as usize];
                        let p = DVec3::new(
                            expected[0].as_f64().unwrap(),
                            expected[1].as_f64().unwrap(),
                            expected[2].as_f64().unwrap(),
                        ) - origin;
                        assert!(
                            (DVec3::from_array(batch.vertices[j * n + k].position.map(f64::from))
                                - p)
                                .length()
                                <= 0.001
                        );
                    }
                    if *layer == u16::MAX {
                        assert!(
                            batch
                                .range(View {
                                    layer: Some(0),
                                    ..all
                                })
                                .is_empty()
                        );
                    }
                }
                assert!(batch.range(View { events: 0, ..all }).is_empty());
                if matches!(batch.kind, 0 | 6) {
                    assert!(
                        batch
                            .range(View {
                                travels: false,
                                ..all
                            })
                            .is_empty()
                    );
                }
                if batch.kind == 15 {
                    assert!(
                        !batch
                            .range(View {
                                travels: false,
                                ..all
                            })
                            .is_empty()
                    );
                }
            }
            assert!(replay.owned_bytes >= bytes.len());
        }
    }
}
