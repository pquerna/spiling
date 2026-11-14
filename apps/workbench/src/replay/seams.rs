// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use crate::analysis::SectionView;
use spiling_contracts::geometry::PlaneMm;

#[derive(Debug, Clone, PartialEq)]
pub struct SeamStart {
    pub position: [f64; 3],
    pub physical_z: [f64; 2],
    pub layer: u32,
    pub tool: u32,
    pub role: u8,
    pub entry_event: u32,
    pub first_event: u32,
    pub closing_event: u32,
    pub command_line: u32,
    pub closing_line: u32,
}
impl SeamStart {
    pub fn in_layers(&self, view: View) -> bool {
        view.layer.is_none_or(|lower| {
            self.layer >= lower && self.layer <= view.layer_end.unwrap_or(lower).max(lower)
        })
    }
    pub fn shown(&self, view: View, clip: SectionClip, origin: DVec3) -> bool {
        view.visible
            && view.seams
            && self.in_layers(view)
            && view.role.is_none_or(|role| role == self.role)
            && view.tool.is_none_or(|tool| tool == self.tool)
            && self.first_event < view.events
            && clip.keeps((DVec3::from_array(self.position) - origin).as_vec3())
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SectionClip {
    pub plane: [f32; 4],
    pub enabled: bool,
    pub section_only: bool,
}
impl SectionClip {
    pub fn project(view: &SectionView, plane: PlaneMm, origin: DVec3, radius: f64) -> Self {
        let normal = DVec3::from_array(plane.normal) * if view.flipped { -1.0 } else { 1.0 };
        let offset = -normal.dot(DVec3::from_array(plane.origin_mm) - origin);
        let projected = [
            normal.x as f32,
            normal.y as f32,
            normal.z as f32,
            offset as f32,
        ];
        let error = (glam::Vec3::from_array([projected[0], projected[1], projected[2]]).as_dvec3()
            - normal)
            .length()
            * radius
            + (f64::from(projected[3]) - offset).abs();
        let enabled =
            view.enabled && projected.iter().all(|value| value.is_finite()) && error <= 0.001;
        Self {
            plane: projected,
            enabled,
            section_only: enabled && view.section_only,
        }
    }
    pub fn keeps(self, point: glam::Vec3) -> bool {
        !self.enabled
            || (!self.section_only
                && glam::Vec4::from_array(self.plane).dot(point.extend(1.0)) <= 0.000001)
    }
}

fn number(record: &[u8; REPLAY_RECORD_BYTES], offset: usize) -> f64 {
    f64::from_le_bytes(record[offset..offset + 8].try_into().unwrap())
}
fn integer(record: &[u8; REPLAY_RECORD_BYTES], offset: usize) -> u32 {
    u32::from_le_bytes(record[offset..offset + 4].try_into().unwrap())
}
fn position(record: &[u8; REPLAY_RECORD_BYTES]) -> [f64; 3] {
    [number(record, 0), number(record, 8), number(record, 16)]
}
fn wall(record: &[u8; REPLAY_RECORD_BYTES]) -> bool {
    record[47] & 2 != 0 && integer(record, 72) == u32::MAX - 2
}
fn same_path(first: &[u8; REPLAY_RECORD_BYTES], next: &[u8; REPLAY_RECORD_BYTES]) -> bool {
    wall(next)
        && first[REPLAY_TOOL_OFFSET..REPLAY_TOOL_OFFSET + 2]
            == next[REPLAY_TOOL_OFFSET..REPLAY_TOOL_OFFSET + 2]
        && first[56..80] == next[56..80]
}

pub(super) fn scan_records(
    records: &super::records::Records,
    mut accept: impl FnMut(SeamStart),
) -> Result<(), String> {
    let mut index = 1;
    while index < records.count {
        let first = &records.get(index)?;
        let entry_index = if records.get(index - 1)?[46] == 12 && index >= 2 {
            let recovery = &records.get(index - 1)?;
            let travel = &records.get(index - 2)?;
            if position(recovery) != position(travel)
                || super::record_layer(recovery) != super::record_layer(travel)
                || super::record_tool(recovery) != super::record_tool(travel)
            {
                index += 1;
                continue;
            }
            index - 2
        } else {
            index - 1
        };
        let entry = &records.get(entry_index)?;
        if !wall(first)
            || entry[46] != 0
            || super::record_layer(entry) != super::record_layer(first)
            || super::record_tool(entry) != super::record_tool(first)
        {
            index += 1;
            continue;
        }
        let mut end = index + 1;
        while end < records.count && same_path(first, &records.get(end)?) {
            end += 1;
        }
        let closing = &records.get(end - 1)?;
        if position(entry) == position(closing) {
            accept(SeamStart {
                position: position(entry),
                physical_z: [number(first, 56), number(first, 64)],
                layer: super::record_layer(first),
                tool: super::record_tool(first),
                role: first[46],
                entry_event: entry_index as u32,
                first_event: index as u32,
                closing_event: end as u32 - 1,
                command_line: integer(first, 40),
                closing_line: integer(closing, 40),
            });
        }
        index = end;
    }
    Ok(())
}

#[cfg(test)]
fn scan(bytes: &[u8], accept: impl FnMut(SeamStart)) {
    let source = spiling_chunks::ChunkSource::from_slice(bytes, u64::MAX).unwrap();
    let records = super::records::Records::new(
        source,
        (bytes.len() - REPLAY_HEADER_BYTES) / REPLAY_RECORD_BYTES,
    );
    scan_records(&records, accept).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiling_contracts::geometry::{AabbMm, SourceHash};

    fn record(point: [f64; 3], kind: u8, owner: u32) -> [u8; REPLAY_RECORD_BYTES] {
        let mut result = [0; REPLAY_RECORD_BYTES];
        for (axis, value) in point.into_iter().enumerate() {
            result[axis * 8..axis * 8 + 8].copy_from_slice(&value.to_le_bytes());
        }
        result[32..40].copy_from_slice(&1200f64.to_le_bytes());
        result[46] = kind;
        result[47] = if matches!(kind, 1..=5 | 8..=10 | 13 | 15) {
            3
        } else {
            1
        };
        if result[47] & 2 != 0 {
            result[48..56].copy_from_slice(&0.4f64.to_le_bytes());
            result[64..72].copy_from_slice(&point[2].to_le_bytes());
        }
        result[72..76].copy_from_slice(&owner.to_le_bytes());
        result
    }
    fn closed() -> Vec<[u8; REPLAY_RECORD_BYTES]> {
        vec![
            record([0.0, 0.0, 0.2], 0, u32::MAX),
            record([2.0, 0.0, 0.2], 1, u32::MAX - 2),
            record([2.0, 2.0, 0.2], 1, u32::MAX - 2),
            record([0.0, 0.0, 0.2], 1, u32::MAX - 2),
        ]
    }
    fn encode(mut records: Vec<[u8; REPLAY_RECORD_BYTES]>) -> (Vec<u8>, ReplayDisplayDescriptor) {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../../../fixtures/protocol/replay.json"))
                .unwrap();
        let mut descriptor: ReplayDisplayDescriptor =
            serde_json::from_value(fixture["descriptor"].clone()).unwrap();
        let mut extrusion = 0.0f64;
        let mut min = [f64::INFINITY; 3];
        let mut max = [f64::NEG_INFINITY; 3];
        for (index, record) in records.iter_mut().enumerate() {
            record[40..44].copy_from_slice(&(index as u32 + 10).to_le_bytes());
            if index == 0 {
                record[47] &= !1;
            }
            if record[47] & 2 != 0 {
                extrusion += 1.0;
            }
            if record[46] == 11 {
                extrusion -= 0.5;
            }
            if record[46] == 12 {
                extrusion += 0.5;
            }
            record[24..32].copy_from_slice(&extrusion.to_le_bytes());
            for (axis, value) in position(record).into_iter().enumerate() {
                min[axis] = min[axis].min(value);
                max[axis] = max[axis].max(value);
            }
        }
        let info = &mut descriptor.info;
        info.event_count = records.len() as u32;
        info.deposition_count = records.iter().filter(|record| record[47] & 2 != 0).count() as u32;
        info.travel_count = records.iter().filter(|record| record[46] == 0).count() as u32;
        info.process_count = records
            .iter()
            .filter(|record| matches!(record[46], 11 | 12))
            .count() as u32;
        info.bounds_mm = AabbMm { min, max };
        let mut bytes = vec![0; REPLAY_HEADER_BYTES];
        bytes[0..4].copy_from_slice(b"SPLR");
        bytes[4..6].copy_from_slice(&(REPLAY_SCHEMA_VERSION as u16).to_le_bytes());
        bytes[6..8].copy_from_slice(&(REPLAY_HEADER_BYTES as u16).to_le_bytes());
        for (offset, value) in [
            (8, info.event_count),
            (12, REPLAY_RECORD_BYTES as u32),
            (16, info.deposition_count),
            (20, info.travel_count),
            (24, 1),
            (88, info.process_count),
        ] {
            bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
        }
        for (offset, values) in [(32, min), (56, max)] {
            for (axis, value) in values.into_iter().enumerate() {
                bytes[offset + axis * 8..offset + axis * 8 + 8]
                    .copy_from_slice(&value.to_le_bytes());
            }
        }
        for record in records {
            bytes.extend(record);
        }
        let hash = SourceHash::from_bytes(&bytes);
        descriptor.resource.name = format!("artifacts/{}", hash.as_str());
        descriptor.resource.sha256 = hash.as_str().into();
        descriptor.resource.size_bytes = bytes.len().to_string();
        if let Ok(pages) = index_replay(std::io::Cursor::new(&bytes), &descriptor.info) {
            descriptor.pages = pages;
        }
        (bytes, descriptor)
    }
    fn project(records: Vec<[u8; REPLAY_RECORD_BYTES]>) -> Replay {
        let (bytes, descriptor) = encode(records);
        Replay::project(&bytes, &descriptor, DVec3::ZERO).unwrap()
    }

    #[test]
    fn a_closed_run_keeps_its_start_and_predecessors_across_index_pages() {
        let mut input = vec![record([0., 0., 0.2], 0, u32::MAX)];
        for i in 0..REPLAY_PAGE_EVENTS + 2 {
            input.push(record(
                [if i % 2 == 0 { 2. } else { 0. }, 0., 0.2],
                1,
                u32::MAX - 2,
            ));
        }
        let (bytes, descriptor) = encode(input);
        assert!(descriptor.pages.len() > 1);
        let source = spiling_chunks::ChunkSource::from_slice(&bytes, u64::MAX).unwrap();
        let replay =
            Replay::project_document_with_budget(&source, &descriptor, DVec3::ZERO, 0).unwrap();
        assert_eq!(replay.seams.len(), 1);
        assert_eq!(replay.seams[0].first_event, 1);
        assert_eq!(
            replay.seams[0].closing_event,
            descriptor.info.event_count - 1
        );
        assert_eq!(
            replay
                .beads
                .as_ref()
                .unwrap()
                .statistics(View::default())
                .unwrap()
                .count,
            descriptor.info.deposition_count as usize
        );
        for batch in replay.batches.iter().filter(|b| !b.seam && b.kind == 1) {
            let data = batch.instances.load().unwrap();
            for (instance, (ordinal, _)) in data.iter().zip(batch.events.iter()) {
                let previous =
                    &bytes[REPLAY_HEADER_BYTES + (*ordinal as usize - 1) * REPLAY_RECORD_BYTES..];
                assert_eq!(
                    instance.start[0],
                    f64::from_le_bytes(previous[..8].try_into().unwrap()) as f32
                );
            }
        }
    }

    #[test]
    fn only_complete_wall_runs_close_at_exact_decoded_xyz() {
        let replay = project(closed());
        assert_eq!(replay.seams.len(), 1);
        let start = &replay.seams[0];
        assert_eq!(start.position, [0.0, 0.0, 0.2]);
        assert_eq!(start.physical_z, [0.0, 0.2]);
        assert_eq!(
            (start.entry_event, start.first_event, start.closing_event),
            (0, 1, 3)
        );
        assert_eq!((start.command_line, start.closing_line), (11, 13));
        let mut open = closed();
        open[3][0..8].copy_from_slice(&0.000001f64.to_le_bytes());
        assert!(project(open).seams.is_empty());
        let mut continues = closed();
        continues.push(record([3.0, 0.0, 0.2], 1, u32::MAX - 2));
        assert!(project(continues).seams.is_empty());
        for owner in [u32::MAX, u32::MAX - 1, 0] {
            let mut unknown = closed();
            for record in &mut unknown[1..] {
                record[72..76].copy_from_slice(&owner.to_le_bytes());
            }
            assert!(project(unknown).seams.is_empty());
        }
        let mut different_z = closed();
        different_z[3][16..24].copy_from_slice(&0.2000001f64.to_le_bytes());
        different_z[3][64..72].copy_from_slice(&0.2000001f64.to_le_bytes());
        assert!(project(different_z).seams.is_empty());
    }

    #[test]
    fn wall_bridge_variable_width_and_coincident_entry_keep_one_start() {
        let mut records = closed();
        records[1][46] = 8;
        records[2][48..56].copy_from_slice(&0.6f64.to_le_bytes());
        records.insert(0, record([0.0, 0.0, 0.2], 0, u32::MAX));
        let replay = project(records);
        assert_eq!(replay.seams.len(), 1);
        assert_eq!(replay.seams[0].role, 8);
        assert_eq!(
            (replay.seams[0].first_event, replay.seams[0].closing_event),
            (2, 4)
        );
        let marker = replay.batches.iter().find(|batch| batch.seam).unwrap();
        assert!(marker.points);
        assert_eq!(marker.instances.load().unwrap()[0].end, [0.0, 0.0, 0.2]);
        assert!(replay.beads.is_ok());
    }

    #[test]
    fn stationary_entry_recovery_preserves_closed_start_but_not_mid_run_pressure() {
        let mut records = closed();
        records.insert(1, record([0.0, 0.0, 0.2], 11, u32::MAX));
        records.insert(2, record([0.0, 0.0, 0.2], 0, u32::MAX));
        records.insert(3, record([0.0, 0.0, 0.2], 12, u32::MAX));
        let replay = project(records.clone());
        assert_eq!(replay.seams.len(), 1);
        assert_eq!(replay.seams[0].position, [0.0, 0.0, 0.2]);
        assert_eq!(
            (
                replay.seams[0].entry_event,
                replay.seams[0].first_event,
                replay.seams[0].closing_event
            ),
            (2, 4, 6)
        );
        for offset in [0, 44, 76] {
            let mut incompatible = records.clone();
            incompatible[3][offset] ^= 1;
            let (bytes, _) = encode(incompatible);
            let mut starts = Vec::new();
            scan(&bytes, |start| starts.push(start));
            assert!(starts.is_empty(), "incompatible recovery at {offset}");
        }
        records.insert(5, record([2.0, 0.0, 0.2], 11, u32::MAX));
        records.insert(6, record([2.0, 0.0, 0.2], 12, u32::MAX));
        assert!(project(records).seams.is_empty());
    }

    #[test]
    fn travels_pressure_and_association_boundaries_never_join_runs() {
        for kind in [0, 11] {
            let mut records = closed();
            records.insert(2, record([2.0, 0.0, 0.2], kind, u32::MAX));
            if kind == 11 {
                records.insert(3, record([2.0, 0.0, 0.2], 12, u32::MAX));
            }
            assert!(project(records).seams.is_empty());
        }
        for owner in [u32::MAX, u32::MAX - 1, 0] {
            let mut records = closed();
            records[2][72..76].copy_from_slice(&owner.to_le_bytes());
            assert!(project(records).seams.is_empty());
        }
        let mut interval = closed();
        interval[2][56..64].copy_from_slice(&0.1f64.to_le_bytes());
        assert!(project(interval).seams.is_empty());
        let mut epoch = closed();
        epoch.insert(2, record([2.0, 0.0, 0.2], 14, u32::MAX));
        let bytes = encode(epoch).0;
        let mut starts = vec![];
        scan(&bytes, |start| starts.push(start));
        assert!(starts.is_empty());
    }

    #[test]
    fn actual_goldens_preserve_epochs_without_inventing_role_based_seams() {
        for (name, expected) in [
            ("replay", 0),
            ("replay-sparse", 0),
            ("replay-fixed-dual", 2),
            ("replay-waste-only", 2),
        ] {
            let (bytes, descriptor) = super::super::bead_tests::fixture(name);
            let replay = Replay::project(&bytes, &descriptor, DVec3::ZERO).unwrap();
            assert_eq!(replay.seams.len(), expected, "{name}");
            for start in &replay.seams {
                let records = bytes[REPLAY_HEADER_BYTES..]
                    .as_chunks::<REPLAY_RECORD_BYTES>()
                    .0;
                assert_eq!(
                    position(&records[start.entry_event as usize]),
                    start.position
                );
                assert_eq!(
                    position(&records[start.closing_event as usize]),
                    start.position
                );
                assert!(
                    records[start.first_event as usize..=start.closing_event as usize]
                        .iter()
                        .all(|record| wall(record) && super::record_tool(record) == start.tool)
                );
            }
        }
    }

    #[test]
    fn selected_layer_filters_playback_and_section_do_not_include_context() {
        let replay = project(closed());
        let start = &replay.seams[0];
        let marker = replay.batches.iter().find(|batch| batch.seam).unwrap();
        let view = View {
            seams: true,
            ..Default::default()
        };
        assert!(start.shown(view, SectionClip::default(), replay.origin));
        assert_eq!(marker.range(view), 0..1);
        for hidden in [
            View {
                seams: false,
                ..view
            },
            View {
                visible: false,
                ..view
            },
            View {
                layer: Some(1),
                ..view
            },
            View {
                role: Some(8),
                ..view
            },
            View {
                tool: Some(1),
                ..view
            },
            View { events: 1, ..view },
        ] {
            assert!(!start.shown(hidden, SectionClip::default(), replay.origin));
            assert!(marker.range(hidden).is_empty());
            assert!(marker.context_ranges(hidden).iter().all(Range::is_empty));
        }
        assert!(start.shown(
            View {
                layer: Some(0),
                layer_end: Some(2),
                events: 2,
                ..view
            },
            SectionClip::default(),
            replay.origin
        ));
        let mut section = SectionView {
            enabled: true,
            offset_mm: 0.1,
            ..Default::default()
        };
        let clipped = SectionClip::project(&section, section.plane(None), replay.origin, 5.0);
        assert!(!start.shown(view, clipped, replay.origin));
        section.flipped = true;
        let flipped = SectionClip::project(&section, section.plane(None), replay.origin, 5.0);
        assert!(start.shown(view, flipped, replay.origin));
        section.section_only = true;
        let only = SectionClip::project(&section, section.plane(None), replay.origin, 5.0);
        assert!(!start.shown(view, only, replay.origin));
    }

    #[test]
    fn marker_identity_and_hash_admission_precede_publication() {
        let (mut bytes, descriptor) = encode(closed());
        let replay = Replay::project(&bytes, &descriptor, DVec3::ZERO).unwrap();
        assert_eq!(replay.seams.len(), 1);
        let marker = replay.batches.iter().find(|b| b.seam).unwrap();
        assert_eq!(marker.events[0].0, replay.seams[0].first_event);
        bytes[REPLAY_HEADER_BYTES + REPLAY_RECORD_BYTES] ^= 1;
        assert!(Replay::project(&bytes, &descriptor, DVec3::ZERO).is_err());
    }
}
