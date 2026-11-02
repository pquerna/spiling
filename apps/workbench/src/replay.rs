// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Projection of validated SPLR6 events, never a machine-program parser.
pub mod beads;
const METADATA_BYTES: usize = 16_384;
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
    pub beads: Result<beads::Beads, String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct View {
    pub layer: Option<u16>,
    /// Inclusive upper layer; None preserves single-layer selection.
    pub layer_end: Option<u16>,
    /// Dim unselected deposited paths, independent of selected-layer playback.
    pub context: bool,
    pub events: u32,
    pub travels: bool,
    pub pressure: bool,
    pub source: bool,
    pub visible: bool,
    pub material: bool,
    pub beads: bool,
    pub role: Option<u8>,
    pub tool: Option<u32>,
}
impl Default for View {
    fn default() -> Self {
        Self {
            layer: None,
            layer_end: None,
            context: true,
            events: u32::MAX,
            travels: true,
            pressure: false,
            source: false,
            visible: true,
            material: false,
            beads: true,
            role: None,
            tool: None,
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
fn event_range(
    events: &[(u32, u16)],
    kind: u8,
    tool: u32,
    view: View,
    vertices: usize,
) -> Range<u32> {
    if !view.visible
        || (!view.travels && matches!(kind, 0 | 6))
        || (!view.pressure && matches!(kind, 11 | 12))
        || view.role.is_some_and(|role| role != kind)
        || view.tool.is_some_and(|selected| selected != tool)
    {
        return 0..0;
    }
    // Lifecycle events can share a role/tool batch with body commands. Exclude
    // leading/trailing unattributed sentinels before searching ordered layers,
    // while retaining batch offsets and global command ordinals.
    let (offset, events) = if view.layer.is_some() {
        let first = events
            .iter()
            .position(|(_, l)| *l != u16::MAX)
            .unwrap_or(events.len());
        let last = events
            .iter()
            .rposition(|(_, l)| *l != u16::MAX)
            .map_or(first, |i| i + 1);
        (first, &events[first..last])
    } else {
        (0, events)
    };
    let start = offset
        + view
            .layer
            .map_or(0, |layer| events.partition_point(|(_, l)| *l < layer));
    let end = offset
        + events.partition_point(|(event, layer)| {
            *event < view.events
                && view
                    .layer
                    .is_none_or(|l| *layer <= view.layer_end.unwrap_or(l).max(l))
        });
    (start * vertices) as u32..(end.max(start) * vertices) as u32
}
impl Batch {
    /// Borrow at most two ranges from existing checked centerlines. Context shows
    /// complete unselected deposition, never travels, pressure or lifecycle events.
    /// Role/tool filters still apply; command progress belongs to the selection.
    pub fn context_ranges(&self, view: View) -> [Range<u32>; 2] {
        let Some(lower) = view.layer else {
            return [0..0, 0..0];
        };
        if !view.context || !matches!(self.kind, 1..=5 | 8..=10 | 13 | 15) {
            return [0..0, 0..0];
        }
        let upper = view.layer_end.unwrap_or(lower).max(lower);
        let other = View {
            events: u32::MAX,
            ..view
        };
        [
            if lower > 0 {
                self.range(View {
                    layer: Some(0),
                    layer_end: Some(lower - 1),
                    ..other
                })
            } else {
                0..0
            },
            if upper < u16::MAX - 1 {
                self.range(View {
                    layer: Some(upper + 1),
                    layer_end: Some(u16::MAX - 1),
                    ..other
                })
            } else {
                0..0
            },
        ]
    }
    pub fn range(&self, view: View) -> Range<u32> {
        event_range(
            &self.events,
            self.kind,
            self.tool,
            view,
            if self.points { 1 } else { 2 },
        )
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
        let upper =
            METADATA_BYTES + bytes.len() + count * (2 * std::mem::size_of::<Vertex>() * 3 + 8);
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
            if !p.is_finite() || !f.is_finite() || (f.as_dvec3() - p).length() > 0.001 {
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
        let owned_bytes = METADATA_BYTES
            + bytes.len()
            + batches
                .iter()
                .map(|b| {
                    b.vertices.capacity() * std::mem::size_of::<Vertex>() * 2
                        + b.events.capacity() * 8
                })
                .sum::<usize>();
        let beads = beads::Beads::project(bytes, origin, reserved + upper);
        let owned_bytes = owned_bytes + beads.as_ref().map_or(0, |b| b.owned_bytes);
        Ok(Self {
            beads,
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
            let all = View {
                pressure: true,
                ..Default::default()
            };
            for batch in &replay.batches {
                assert_ne!(batch.kind, 14);
                if matches!(batch.kind, 11 | 12) {
                    assert!(batch.range(View::default()).is_empty());
                }
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

#[cfg(test)]
mod bead_tests {
    use super::*;
    fn fixture(name: &str) -> (Vec<u8>, ReplayDisplayDescriptor) {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../fixtures/protocol/{name}.json"));
        let fixture: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let bytes = fixture["data"]
            .as_str()
            .unwrap()
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 16).unwrap())
            .collect();
        (
            bytes,
            serde_json::from_value(fixture["descriptor"].clone()).unwrap(),
        )
    }
    #[test]
    fn nominal_envelopes_keep_actual_endpoints_intervals_roles_and_epochs() {
        for name in [
            "replay",
            "replay-sparse",
            "replay-fixed-dual",
            "replay-waste-only",
        ] {
            let (bytes, descriptor) = fixture(name);
            let origin = DVec3::splat(0.125);
            let replay = Replay::project(&bytes, &descriptor, 0, origin).unwrap();
            let beads = replay.beads.as_ref().unwrap();
            assert!(beads.chord_error_mm <= 0.001);
            assert_eq!(
                beads.statistics(View::default()).unwrap().count,
                descriptor.info.deposition_count as usize
            );
            assert!(
                beads
                    .statistics(View {
                        events: 0,
                        ..View::default()
                    })
                    .is_none()
            );
            assert!(
                beads
                    .statistics(View {
                        role: Some(14),
                        ..View::default()
                    })
                    .is_none()
            );
            assert!(
                beads
                    .statistics(View {
                        tool: Some(2),
                        ..View::default()
                    })
                    .is_none()
            );
            let number = |record: &[u8], offset| {
                f64::from_le_bytes(record[offset..offset + 8].try_into().unwrap())
            };
            let records = bytes[REPLAY_HEADER_BYTES..]
                .as_chunks::<REPLAY_RECORD_BYTES>()
                .0;
            assert_eq!(
                beads.instance_count,
                records.iter().filter(|r| r[47] & 2 != 0).count()
            );
            for batch in &beads.batches {
                assert!(!matches!(batch.kind, 0 | 6 | 7 | 11 | 12 | 14));
                for (instance, (event, layer)) in batch.instances.iter().zip(&batch.events) {
                    let record = &records[*event as usize];
                    let prior = &records[*event as usize - 1];
                    for (p, r) in [(instance.start, prior), (instance.end, record)] {
                        let expected =
                            DVec3::new(number(r, 0), number(r, 8), number(r, 16)) - origin;
                        assert!((DVec3::from_array(p.map(f64::from)) - expected).length() <= 0.001);
                    }
                    assert!((f64::from(instance.width) - number(record, 48)).abs() < 0.000001);
                    assert!(
                        (f64::from(instance.height) - (number(record, 64) - number(record, 56)))
                            .abs()
                            < 0.000001
                    );
                    assert!(
                        (f64::from(instance.end[2]) - f64::from(instance.height) + origin.z
                            - number(record, 56))
                        .abs()
                            < 0.000001
                    );
                    let selected = View {
                        role: Some(batch.kind),
                        tool: Some(batch.tool),
                        layer: Some(*layer),
                        events: event + 1,
                        ..View::default()
                    };
                    assert!(!batch.range(selected).is_empty());
                    assert!(
                        batch
                            .range(View {
                                role: Some(14),
                                ..selected
                            })
                            .is_empty()
                    );
                    assert!(
                        batch
                            .range(View {
                                tool: Some(2),
                                ..selected
                            })
                            .is_empty()
                    );
                    assert!(
                        batch
                            .range(View {
                                events: 0,
                                ..selected
                            })
                            .is_empty()
                    );
                }
            }
        }
    }
    #[test]
    fn exhausted_bead_budget_and_large_local_extent_keep_checked_centerlines() {
        let (bytes, descriptor) = fixture("replay");
        assert!(Replay::project(&bytes, &descriptor, 0, DVec3::NAN).is_err());
        assert!(Replay::project(&bytes, &descriptor, 0, DVec3::splat(f64::INFINITY)).is_err());
        let base_peak = METADATA_BYTES
            + bytes.len()
            + descriptor.info.event_count as usize * (2 * std::mem::size_of::<Vertex>() * 3 + 8);
        let reserved = spiling_contracts::geometry::MAX_DISPLAY_BUFFER_BYTES as usize - base_peak;
        let replay = Replay::project(&bytes, &descriptor, reserved, DVec3::ZERO).unwrap();
        assert!(replay.beads.as_ref().unwrap_err().contains("budget"));
        assert!(!replay.batches.is_empty());
        assert!(
            reserved + replay.owned_bytes
                <= spiling_contracts::geometry::MAX_DISPLAY_BUFFER_BYTES as usize
        );
        let replay =
            Replay::project(&bytes, &descriptor, 0, DVec3::new(10_000.0, 0.0, 0.0)).unwrap();
        assert!(replay.beads.as_ref().unwrap_err().contains("numerical"));
        assert!(!replay.batches.is_empty());
    }
}

#[cfg(test)]
mod layer_range_tests {
    use super::*;
    #[test]
    fn other_layers_are_deposition_context_not_selected_playback_or_lifecycle() {
        let batch = Batch {
            kind: 1,
            tool: 0,
            points: false,
            vertices: Vec::new(),
            events: vec![
                (0, u16::MAX),
                (2, 0),
                (5, 1),
                (7, 1),
                (9, 2),
                (12, 3),
                (20, u16::MAX),
            ],
        };
        let view = View {
            layer: Some(1),
            events: 0,
            ..View::default()
        };
        // Below and above remain whole even at zero selected-command progress.
        // No selected paths or unattributed lifecycle sentinels enter context.
        assert_eq!(batch.context_ranges(view), [2..4, 8..12]);
        assert!(batch.range(view).is_empty());
        assert_eq!(
            batch.context_ranges(View {
                layer_end: Some(2),
                ..view
            }),
            [2..4, 10..12]
        );
        assert_eq!(
            batch.context_ranges(View {
                layer: Some(0),
                ..view
            }),
            [0..0, 4..12]
        );
        assert_eq!(
            batch.context_ranges(View {
                layer: Some(u16::MAX - 1),
                ..view
            }),
            [2..12, 0..0]
        );
        for excluded in [
            View {
                context: false,
                ..view
            },
            View {
                layer: None,
                ..view
            },
            View {
                visible: false,
                ..view
            },
            View {
                role: Some(2),
                ..view
            },
            View {
                tool: Some(1),
                ..view
            },
        ] {
            assert!(batch.context_ranges(excluded).iter().all(Range::is_empty));
        }
        for kind in [0, 6, 7, 11, 12, 14] {
            let non_deposition = Batch {
                kind,
                events: batch.events.clone(),
                vertices: Vec::new(),
                points: false,
                tool: 0,
            };
            assert!(
                non_deposition
                    .context_ranges(view)
                    .iter()
                    .all(Range::is_empty)
            );
        }
    }
    #[test]
    fn inclusive_layer_ranges_compose_with_exclusive_progress_and_role_tool_filters() {
        let events = [(2, 0), (3, 0), (5, 1), (7, 1), (9, 2), (12, 3)];
        let view = View {
            layer: Some(1),
            layer_end: Some(2),
            events: 9,
            ..Default::default()
        };
        assert_eq!(event_range(&events, 1, 0, view, 2), 4..8);
        let lifecycle = [
            (0, u16::MAX),
            (2, 0),
            (5, 1),
            (7, 1),
            (9, 2),
            (20, u16::MAX),
        ];
        assert_eq!(event_range(&lifecycle, 1, 0, view, 2), 4..8);
        assert_eq!(
            event_range(&lifecycle, 1, 0, View { events: 10, ..view }, 2),
            4..10
        );
        assert_eq!(event_range(&[(0, u16::MAX)], 1, 0, view, 2), 2..2);
        assert_eq!(
            event_range(&events, 1, 0, View { events: 10, ..view }, 1),
            2..5
        );
        assert_eq!(
            event_range(&events, 1, 0, View { events: 3, ..view }, 2),
            4..4
        );
        assert_eq!(
            event_range(
                &events,
                1,
                0,
                View {
                    role: Some(2),
                    ..view
                },
                2
            ),
            0..0
        );
        assert_eq!(
            event_range(
                &events,
                1,
                0,
                View {
                    tool: Some(1),
                    ..view
                },
                2
            ),
            0..0
        );
        assert_eq!(
            event_range(
                &events,
                1,
                0,
                View {
                    visible: false,
                    ..view
                },
                2
            ),
            0..0
        );
    }
}
