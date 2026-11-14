// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Projection of validated SPLR7 events, never a machine-program parser.
pub mod beads;
mod records;
pub mod seams;
pub mod table;
pub mod upload;
const METADATA_BYTES: usize = 16_384;
use glam::DVec3;
use spiling_contracts::manufacturing::*;
use std::{collections::BTreeMap, ops::Range, sync::Arc};

/// Scheduled layer of one checked SPLR record.
fn record_layer(record: &[u8]) -> u32 {
    u32::from_le_bytes(
        record[REPLAY_LAYER_OFFSET..REPLAY_LAYER_OFFSET + 4]
            .try_into()
            .unwrap(),
    )
}
/// Active tool of one checked SPLR record.
fn record_tool(record: &[u8]) -> u32 {
    u32::from(u16::from_le_bytes(
        record[REPLAY_TOOL_OFFSET..REPLAY_TOOL_OFFSET + 2]
            .try_into()
            .unwrap(),
    ))
}

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
    pub seam: bool,
    pub instances: Arc<table::Table>,
    pub events: Arc<Vec<(u32, u32)>>,
}
#[derive(Debug)]
pub struct Replay {
    pub origin: DVec3,
    pub layer_windows: Vec<Range<u32>>,
    pub event_count: u32,
    pub batches: Vec<Batch>,
    pub seams: Vec<seams::SeamStart>,
    pub owned_bytes: usize,
    pub beads: Result<beads::Beads, String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct View {
    pub layer: Option<u32>,
    /// Inclusive upper layer; None preserves single-layer selection.
    pub layer_end: Option<u32>,
    /// Dim unselected deposited paths, independent of selected-layer playback.
    pub context: bool,
    pub events: u32,
    pub travels: bool,
    pub pressure: bool,
    pub source: bool,
    pub visible: bool,
    pub material: bool,
    pub beads: bool,
    pub seams: bool,
    pub selected_seam: Option<u32>,
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
            seams: false,
            selected_seam: None,
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
    "Skirt, brim, waste",
];
fn event_range(
    events: &[(u32, u32)],
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
            .position(|(_, l)| *l != REPLAY_LIFECYCLE_LAYER)
            .unwrap_or(events.len());
        let last = events
            .iter()
            .rposition(|(_, l)| *l != REPLAY_LIFECYCLE_LAYER)
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
        if self.seam || !view.context || !matches!(self.kind, 1..=5 | 8..=10 | 13 | 15) {
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
            if upper < REPLAY_LIFECYCLE_LAYER - 1 {
                self.range(View {
                    layer: Some(upper + 1),
                    layer_end: Some(REPLAY_LIFECYCLE_LAYER - 1),
                    ..other
                })
            } else {
                0..0
            },
        ]
    }
    pub fn range(&self, view: View) -> Range<u32> {
        if self.seam && !view.seams {
            return 0..0;
        }
        event_range(&self.events, self.kind, self.tool, view, 1)
    }
}

impl Replay {
    /// First actual deposition in the selection, using its existing start vertex.
    /// Context, travel, startup priming and stationary events are not path starts.
    pub fn toolpath_start(&self, view: View) -> Option<(usize, u32)> {
        self.batches
            .iter()
            .enumerate()
            .filter(|(_, batch)| {
                !batch.points && !batch.seam && matches!(batch.kind, 1..=5 | 8..=10 | 13 | 15)
            })
            .filter_map(|(index, batch)| {
                let range = batch.range(view);
                (!range.is_empty()).then(|| {
                    let event = batch.events[range.start as usize].0;
                    (event, index, range.start)
                })
            })
            .min_by_key(|(event, _, _)| *event)
            .map(|(_, batch, vertex)| (batch, vertex))
    }

    pub fn project(
        bytes: &[u8],
        descriptor: &ReplayDisplayDescriptor,
        origin: DVec3,
    ) -> Result<Self, String> {
        let source =
            spiling_chunks::ChunkSource::from_slice(bytes, u64::from(MAX_REPLAY_DISPLAY_BYTES))
                .map_err(|e| e.to_string())?;
        Self::project_document(&source, descriptor, origin)
    }
    pub fn project_document(
        source: &spiling_chunks::ChunkSource,
        descriptor: &ReplayDisplayDescriptor,
        origin: DVec3,
    ) -> Result<Self, String> {
        Self::project_document_with_budget(source, descriptor, origin, table::available_growth())
    }
    fn project_document_with_budget(
        source: &spiling_chunks::ChunkSource,
        descriptor: &ReplayDisplayDescriptor,
        origin: DVec3,
        resident: usize,
    ) -> Result<Self, String> {
        let _preparation = crate::performance::span("replay_projection");
        validate_replay_reader(source.reader(), descriptor).map_err(|e| e.to_string())?;
        let count = descriptor.info.event_count as usize;
        let records = records::Records::new(source.clone(), count);
        let mut seam_counts = BTreeMap::<(usize, u8, u32), usize>::new();
        seams::scan_records(&records, |start| {
            *seam_counts
                .entry((
                    start.first_event as usize / REPLAY_PAGE_EVENTS as usize,
                    start.role,
                    start.tool,
                ))
                .or_default() += 1
        })?;
        let seam_count: usize = seam_counts.values().sum();
        let record = |i: usize| records.get(i);
        let point = |i: usize| -> Result<[f32; 3], String> {
            let r = record(i)?;
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
        let envelope = beads::Beads::project_records(&records, origin);
        let facets = envelope.as_ref().map_or(0, |b| b.facets);
        let retain = count.saturating_add(seam_count).saturating_mul(2 * 36 + 8) <= resident;
        let backing = if retain {
            None
        } else {
            Some(spiling_chunks::Spool::new().map_err(|e| e.to_string())?)
        };
        let mut counts = BTreeMap::<(usize, u8, u32, bool), usize>::new();
        let mut layer_windows = vec![0..0; descriptor.info.layer_count as usize];
        for i in 0..count {
            let r = record(i)?;
            let kind = r[46];
            let layer = record_layer(&r);
            if layer != REPLAY_LIFECYCLE_LAYER {
                let range = &mut layer_windows[layer as usize];
                if range.start == range.end {
                    range.start = i as u32;
                }
                range.end = i as u32 + 1;
            }
            if kind == 14 {
                continue;
            }
            let points = i == 0 || matches!(kind, 11 | 12);
            let tool = if descriptor.info.tool_epoch_count > 0 {
                record_tool(&r)
            } else {
                0
            };
            *counts
                .entry((i / REPLAY_PAGE_EVENTS as usize, kind, tool, points))
                .or_default() += 1;
        }
        let mut groups: BTreeMap<_, _> = counts
            .into_iter()
            .map(|((page, kind, tool, points), count)| {
                (
                    (page, kind, tool, points),
                    Batch {
                        kind,
                        tool,
                        points,
                        seam: false,
                        instances: Arc::new(table::Table::with_capacity(if retain {
                            count
                        } else {
                            0
                        })),
                        events: Arc::new(Vec::with_capacity(count)),
                    },
                )
            })
            .collect();
        for i in 0..count {
            let r = record(i)?;
            let kind = r[46];
            // The first endpoint is an unknown-start marker only in the single-tool stream.
            if kind == 14 || (i == 0 && descriptor.info.tool_epoch_count > 0) {
                continue;
            }
            let marker = i == 0;
            let points = marker || matches!(kind, 11 | 12);
            let tool = if descriptor.info.tool_epoch_count > 0 {
                record_tool(&r)
            } else {
                0
            };
            let batch = groups
                .entry((i / REPLAY_PAGE_EVENTS as usize, kind, tool, points))
                .or_insert_with(|| Batch {
                    kind,
                    tool,
                    points,
                    seam: false,
                    instances: Arc::new(table::Table::with_capacity(0)),
                    events: Arc::new(vec![]),
                });
            Arc::get_mut(&mut batch.instances)
                .unwrap()
                .push(beads::Instance {
                    start: if points { point(i)? } else { point(i - 1)? },
                    end: point(i)?,
                    width: f64::from_le_bytes(r[48..56].try_into().unwrap()) as f32,
                    height: (f64::from_le_bytes(r[64..72].try_into().unwrap())
                        - f64::from_le_bytes(r[56..64].try_into().unwrap()))
                        as f32,
                    class: u32::from(kind) | (facets << 4),
                });
            Arc::get_mut(&mut batch.events)
                .unwrap()
                .push((i as u32, record_layer(&r)));
            if !retain && ((i + 1).is_multiple_of(REPLAY_PAGE_EVENTS as usize) || i + 1 == count) {
                let page = i / REPLAY_PAGE_EVENTS as usize;
                for (_, batch) in
                    groups.range_mut((page, 0, 0, false)..=(page, u8::MAX, u32::MAX, true))
                {
                    Arc::get_mut(&mut batch.instances)
                        .unwrap()
                        .spool(backing.as_ref().unwrap())?;
                }
            }
        }
        if !retain {
            for batch in groups.values_mut() {
                Arc::get_mut(&mut batch.instances)
                    .unwrap()
                    .spool(backing.as_ref().unwrap())?;
            }
        }
        let mut batches: Vec<_> = groups.into_values().collect();
        let mut seams = Vec::with_capacity(seam_count);
        let mut markers: BTreeMap<_, _> = seam_counts
            .into_iter()
            .map(|((page, kind, tool), count)| {
                (
                    (page, kind, tool),
                    Batch {
                        kind,
                        tool,
                        points: true,
                        seam: true,
                        instances: Arc::new(table::Table::with_capacity(if retain {
                            count
                        } else {
                            0
                        })),
                        events: Arc::new(Vec::with_capacity(count)),
                    },
                )
            })
            .collect();
        seams::scan_records(&records, |start| {
            let batch = markers
                .get_mut(&(
                    start.first_event as usize / REPLAY_PAGE_EVENTS as usize,
                    start.role,
                    start.tool,
                ))
                .unwrap();
            let position = (DVec3::from_array(start.position) - origin)
                .as_vec3()
                .to_array();
            Arc::get_mut(&mut batch.instances)
                .unwrap()
                .push(beads::Instance {
                    start: position,
                    end: position,
                    width: 0.0,
                    height: 0.0,
                    class: u32::from(start.role),
                });
            Arc::get_mut(&mut batch.events)
                .unwrap()
                .push((start.first_event, start.layer));
            seams.push(start);
        })?;
        batches.extend(markers.into_values());
        if !retain {
            for batch in &mut batches {
                Arc::get_mut(&mut batch.instances)
                    .unwrap()
                    .spool(backing.as_ref().unwrap())?;
            }
        }
        let beads = envelope.map(|mut beads| {
            for batch in &batches {
                if !batch.points && !batch.seam && matches!(batch.kind,1..=5|8..=10|13|15) {
                    beads.batches.push(beads::BeadBatch {
                        kind: batch.kind,
                        tool: batch.tool,
                        instances: batch.instances.clone(),
                        events: batch.events.clone(),
                    });
                }
            }
            beads
        });
        let owned_bytes = METADATA_BYTES
            + source.retained_bytes()
            + seams.capacity() * std::mem::size_of::<seams::SeamStart>()
            + batches
                .iter()
                .map(|b| {
                    b.instances.retained_bytes() + b.instances.len() * 36 + b.events.capacity() * 8
                })
                .sum::<usize>();
        if std::env::var_os("SPILING_OPEN_TRACE").as_deref() == Some(std::ffi::OsStr::new("1")) {
            let gpu_bytes = batches
                .iter()
                .map(|b| b.instances.len() * 36)
                .sum::<usize>();
            eprintln!(
                "SPILING_REPLAY {}",
                serde_json::json!({
                    "events": count, "raw_bytes": source.len(), "cpu_retained_bytes": owned_bytes - gpu_bytes,
                    "gpu_bytes": gpu_bytes, "batches": batches.len(), "resident_tables": retain,
                })
            );
        }
        Ok(Self {
            beads,
            layer_windows,
            event_count: descriptor.info.event_count,
            origin,
            batches,
            seams,
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
            let replay = Replay::project(&bytes, &descriptor, origin).unwrap();
            let all = View {
                pressure: true,
                seams: true,
                ..Default::default()
            };
            for batch in &replay.batches {
                assert_ne!(batch.kind, 14);
                if matches!(batch.kind, 11 | 12) {
                    assert!(batch.range(View::default()).is_empty());
                }
                assert_eq!(batch.range(all).len(), batch.instances.len());
                for (j, (event, layer)) in batch.events.iter().enumerate() {
                    let n = if batch.points { 1 } else { 2 };
                    for k in 0..n {
                        let actual = if batch.seam {
                            event - 1
                        } else if batch.points {
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
                            (DVec3::from_array(
                                (if k == 0 {
                                    batch.instances.load().unwrap()[j].start
                                } else {
                                    batch.instances.load().unwrap()[j].end
                                })
                                .map(f64::from)
                            ) - p)
                                .length()
                                <= 0.001
                        );
                    }
                    if *layer == REPLAY_LIFECYCLE_LAYER {
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
    pub(super) fn fixture(name: &str) -> (Vec<u8>, ReplayDisplayDescriptor) {
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
    fn start_marker_uses_first_selected_actual_deposition_not_travel_or_context() {
        for name in [
            "replay",
            "replay-sparse",
            "replay-fixed-dual",
            "replay-waste-only",
        ] {
            let (bytes, descriptor) = fixture(name);
            let origin = DVec3::splat(0.125);
            let replay = Replay::project(&bytes, &descriptor, origin).unwrap();
            let records = bytes[REPLAY_HEADER_BYTES..]
                .as_chunks::<REPLAY_RECORD_BYTES>()
                .0;
            for view in [
                View::default(),
                View {
                    layer: Some(0),
                    ..View::default()
                },
                View {
                    layer: Some(1),
                    ..View::default()
                },
                View {
                    role: Some(5),
                    ..View::default()
                },
                View {
                    tool: Some(0),
                    ..View::default()
                },
                View {
                    tool: Some(1),
                    ..View::default()
                },
                View {
                    events: descriptor.info.event_count / 2,
                    ..View::default()
                },
                View {
                    events: 0,
                    ..View::default()
                },
                View {
                    visible: false,
                    ..View::default()
                },
            ] {
                // Oracle is chronological decoded commands, independent of role-sorted GPU batches.
                let expected = records.iter().enumerate().find(|(event, record)| {
                    let layer = record_layer(&record[..]);
                    let tool = record_tool(&record[..]);
                    view.visible
                        && (*event as u32) < view.events
                        && matches!(record[46], 1..=5 | 8..=10 | 13 | 15)
                        && view.layer.is_none_or(|lo| {
                            layer >= lo && layer <= view.layer_end.unwrap_or(lo).max(lo)
                        })
                        && view.role.is_none_or(|role| role == record[46])
                        && view.tool.is_none_or(|selected| selected == tool)
                });
                let actual = replay.toolpath_start(view);
                assert_eq!(actual.is_some(), expected.is_some(), "{name}");
                if let (Some((batch, vertex)), Some((event, _))) = (actual, expected) {
                    let batch = &replay.batches[batch];
                    assert_eq!(batch.events[vertex as usize].0 as usize, event);
                    let previous = &records[event - 1];
                    let expected = DVec3::from_array(std::array::from_fn(|axis| {
                        f64::from_le_bytes(previous[axis * 8..axis * 8 + 8].try_into().unwrap())
                    })) - origin;
                    let position = DVec3::from_array(
                        batch.instances.load().unwrap()[vertex as usize]
                            .start
                            .map(f64::from),
                    );
                    assert!((position - expected).length() <= 0.001);
                }
            }
        }
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
            let replay = Replay::project(&bytes, &descriptor, origin).unwrap();
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
                let line = replay
                    .batches
                    .iter()
                    .find(|line| {
                        !line.points
                            && !line.seam
                            && line.kind == batch.kind
                            && line.tool == batch.tool
                    })
                    .unwrap();
                assert!(Arc::ptr_eq(&line.instances, &batch.instances));
                assert!(Arc::ptr_eq(&line.events, &batch.events));

                assert!(!matches!(batch.kind, 0 | 6 | 7 | 11 | 12 | 14));
                for (instance, (event, layer)) in batch
                    .instances
                    .load()
                    .unwrap()
                    .iter()
                    .zip(batch.events.iter())
                {
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
    fn shared_beads_fit_the_line_budget_and_numerical_fallback_keeps_checked_lines() {
        let (bytes, descriptor) = fixture("replay");
        assert!(Replay::project(&bytes, &descriptor, DVec3::NAN).is_err());
        assert!(Replay::project(&bytes, &descriptor, DVec3::splat(f64::INFINITY)).is_err());
        let source = spiling_chunks::ChunkSource::from_slice(&bytes, u64::MAX).unwrap();
        let replay =
            Replay::project_document_with_budget(&source, &descriptor, DVec3::ZERO, 0).unwrap();
        assert!(replay.beads.is_ok());
        assert!(!replay.batches.is_empty());
        for batch in &replay.batches {
            assert_eq!(batch.instances.capacity(), 0);
        }
        let replay = Replay::project(&bytes, &descriptor, DVec3::new(10_000.0, 0.0, 0.0)).unwrap();
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
            seam: false,
            instances: Arc::new(table::Table::with_capacity(0)),
            events: Arc::new(vec![
                (0, REPLAY_LIFECYCLE_LAYER),
                (2, 0),
                (5, 1),
                (7, 1),
                (9, 2),
                (12, 3),
                (20, REPLAY_LIFECYCLE_LAYER),
            ]),
        };
        let view = View {
            layer: Some(1),
            events: 0,
            ..View::default()
        };
        // Below and above remain whole even at zero selected-command progress.
        // No selected paths or unattributed lifecycle sentinels enter context.
        assert_eq!(batch.context_ranges(view), [1..2, 4..6]);
        assert!(batch.range(view).is_empty());
        assert_eq!(
            batch.context_ranges(View {
                layer_end: Some(2),
                ..view
            }),
            [1..2, 5..6]
        );
        assert_eq!(
            batch.context_ranges(View {
                layer: Some(0),
                ..view
            }),
            [0..0, 2..6]
        );
        assert_eq!(
            batch.context_ranges(View {
                layer: Some(REPLAY_LIFECYCLE_LAYER - 1),
                ..view
            }),
            [1..6, 0..0]
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
                instances: Arc::new(table::Table::with_capacity(0)),
                points: false,
                seam: false,
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
            (0, REPLAY_LIFECYCLE_LAYER),
            (2, 0),
            (5, 1),
            (7, 1),
            (9, 2),
            (20, REPLAY_LIFECYCLE_LAYER),
        ];
        assert_eq!(event_range(&lifecycle, 1, 0, view, 2), 4..8);
        assert_eq!(
            event_range(&lifecycle, 1, 0, View { events: 10, ..view }, 2),
            4..10
        );
        assert_eq!(
            event_range(&[(0, REPLAY_LIFECYCLE_LAYER)], 1, 0, view, 2),
            2..2
        );
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
