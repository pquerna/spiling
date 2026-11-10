// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;
use spiling_contracts::analysis::fastener::*;

pub enum Event {
    Leave,
    Point { reference: FaceRef, place: bool },
}
pub struct Canvas {
    pub armed: bool,
    pub selected: Option<u32>,
    pub saved: Vec<AnalysisFastener>,
    pub preview: Option<AnalysisFastener>,
    pub reference: Option<FaceRef>,
    pub pose: RigidPoseMm,
    pub event: Option<Event>,
}
// Reserved alongside source/replay pages: host preparations, retained ghost
// lines/keys and temporary checked display-outline construction.
pub(crate) const DISPLAY_RESERVE_BYTES: usize = 6 * 1024 * 1024;
const RING: usize = 32;
const MAX_GHOST_LINES: usize = 4096 * 3 + (MAX_ANALYSIS_FASTENERS + 1) * 256;
#[derive(PartialEq)]
struct GhostKey {
    records: Vec<AnalysisFastener>,
    pose: RigidPoseMm,
    reference: Option<FaceRef>,
    selected: Option<u32>,
    previewing: bool,
}
#[derive(Default)]
pub(super) struct Ghosts {
    key: Option<GhostKey>,
    scene: std::sync::Weak<Scene>,
    lines: Vec<([DVec3; 2], bool)>,
    labels: Vec<(DVec3, String, bool)>,
}
fn ring(center: DVec3, radius: f64, z: f64) -> [DVec3; RING] {
    std::array::from_fn(|index| {
        let angle = std::f64::consts::TAU * index as f64 / RING as f64;
        DVec3::new(
            center.x + radius * angle.cos(),
            center.y + radius * angle.sin(),
            z,
        )
    })
}
impl Ghosts {
    fn prepare(&mut self, canvas: &Canvas, scene: Option<&Arc<Scene>>) {
        let mut records = canvas.saved.clone();
        if let Some(preview) = &canvas.preview {
            records.push(preview.clone());
        }
        records.truncate(MAX_ANALYSIS_FASTENERS + 1);
        let key = GhostKey {
            records,
            pose: canvas.pose,
            reference: canvas.reference.clone(),
            selected: canvas.selected,
            previewing: canvas.preview.is_some(),
        };
        let scene_key = scene.map(Arc::downgrade).unwrap_or_default();
        if self.key.as_ref() == Some(&key) && self.scene.ptr_eq(&scene_key) {
            return;
        }
        self.lines.clear();
        self.labels.clear();
        let rotation = DQuat::from_array(canvas.pose.rotation_xyzw);
        let translation = DVec3::from_array(canvas.pose.translation_mm);
        let world = |point: DVec3| rotation * point + translation;
        let mut remaining = 4096;
        for (index, item) in key.records.iter().enumerate() {
            if item.validate().is_err() {
                continue;
            }
            let active = (index < canvas.saved.len() && canvas.selected == Some(item.id))
                || (canvas.preview.is_some() && index == canvas.saved.len());
            let hole = &item.hole;
            let center = DVec3::from_array(hole.center_mm);
            let bottom = center.z;
            let top = bottom + hole.depth_mm;
            // Complete native bore group, including periodic/split faces. The
            // mesh contributes only a checked display outline, never admission.
            if active
                && let Some(reference) = &canvas.reference
                && let Some(scene) = scene
            {
                for face in &hole.bore_faces {
                    let mut reference = reference.clone();
                    reference.face_id = face.clone();
                    if let Ok(edges) =
                        super::mechanics::patch_edges(scene, &reference, &mut remaining)
                    {
                        self.lines
                            .extend(edges.into_iter().map(|edge| (edge, true)));
                    }
                }
            }
            let shaft = [
                ring(center, hole.radius_mm * 0.8, bottom),
                ring(center, hole.radius_mm * 0.8, top),
            ];
            for level in &shaft {
                for index in 0..RING {
                    self.lines.push((
                        [world(level[index]), world(level[(index + 1) % RING])],
                        active,
                    ));
                }
            }
            for index in (0..RING).step_by(8) {
                self.lines
                    .push(([world(shaft[0][index]), world(shaft[1][index])], active));
            }
            let (label, end) = match item.attachment {
                FastenerAttachment::Clamp {
                    end,
                    washer_diameter_mm,
                } => {
                    let z = if end == HoleEnd::Start { bottom } else { top };
                    for radius in [hole.radius_mm, washer_diameter_mm * 0.5] {
                        let points = ring(center, radius, z);
                        for index in 0..RING {
                            self.lines.push((
                                [world(points[index]), world(points[(index + 1) % RING])],
                                active,
                            ));
                        }
                    }
                    ("Clamp · fixed".to_owned(), end)
                }
                FastenerAttachment::BondedEngagement { end, engagement_mm } => {
                    let start = if end == HoleEnd::Start {
                        bottom
                    } else {
                        top - engagement_mm
                    };
                    let stop = start + engagement_mm;
                    for z in [start, stop] {
                        let points = ring(center, hole.radius_mm, z);
                        for index in 0..RING {
                            self.lines.push((
                                [world(points[index]), world(points[(index + 1) % RING])],
                                active,
                            ));
                        }
                    }
                    for index in (0..RING).step_by(4) {
                        let angle = std::f64::consts::TAU * index as f64 / RING as f64;
                        let offset = DVec3::new(angle.cos(), angle.sin(), 0.0) * hole.radius_mm;
                        self.lines.push((
                            [
                                world(DVec3::new(center.x, center.y, start) + offset),
                                world(DVec3::new(center.x, center.y, stop) + offset),
                            ],
                            active,
                        ));
                    }
                    ("Bonded engagement · fixed".into(), end)
                }
                FastenerAttachment::Pin {
                    allow_axial_slide,
                    allow_spin,
                } => {
                    if allow_axial_slide {
                        let a = world(center - DVec3::Z * hole.radius_mm);
                        let b = world(center + DVec3::Z * (hole.depth_mm + hole.radius_mm));
                        self.lines.push(([a, b], active));
                        // Arrowheads at both ends indicate permitted axial travel.
                        for (point, sign) in [
                            (center - DVec3::Z * hole.radius_mm, 1.0),
                            (center + DVec3::Z * (hole.depth_mm + hole.radius_mm), -1.0),
                        ] {
                            for side in [-1.0, 1.0] {
                                self.lines.push((
                                    [
                                        world(point),
                                        world(
                                            point
                                                + (DVec3::Z * sign + DVec3::X * side)
                                                    * hole.radius_mm
                                                    * 0.45,
                                        ),
                                    ],
                                    active,
                                ));
                            }
                        }
                    }
                    if allow_spin {
                        let points = ring(center, hole.radius_mm * 1.5, top);
                        for index in 0..24 {
                            self.lines
                                .push(([world(points[index]), world(points[index + 1])], active));
                        }
                        let tip = points[24];
                        for side in [-1.0, 1.0] {
                            self.lines.push((
                                [
                                    world(tip),
                                    world(tip + DVec3::new(-0.5, side * 0.4, 0.0) * hole.radius_mm),
                                ],
                                active,
                            ));
                        }
                    }
                    (
                        match (allow_axial_slide, allow_spin) {
                            (true, true) => "Pin · slide + spin free",
                            (true, false) => "Pin · slide free",
                            (false, true) => "Pivot · spin free",
                            (false, false) => "Pin · fixed",
                        }
                        .into(),
                        HoleEnd::End,
                    )
                }
            };
            let seat = DVec3::new(
                center.x,
                center.y,
                if end == HoleEnd::Start { bottom } else { top },
            );
            // Ground/support hatch distinguishes a fixed reference from a shaft.
            let span = hole.radius_mm * 1.5;
            self.lines.push((
                [world(seat - DVec3::X * span), world(seat + DVec3::X * span)],
                active,
            ));
            for index in -2..=2 {
                let point = seat + DVec3::X * (index as f64 * span / 2.0);
                self.lines.push((
                    [
                        world(point),
                        world(point + DVec3::new(-0.4, 0.0, -0.4) * hole.radius_mm),
                    ],
                    active,
                ));
            }
            self.labels
                .push((world(seat), format!("{} · {label}", item.name), active));
        }
        self.key = Some(key);
        self.scene = scene_key;
        debug_assert!(self.lines.len() <= MAX_GHOST_LINES);
    }
    pub fn paint(
        &mut self,
        ui: &egui::Ui,
        canvas: &Canvas,
        scene: &Arc<Scene>,
        matrix: DMat4,
        center: DVec3,
        rect: egui::Rect,
    ) {
        self.prepare(canvas, Some(scene));
        let project = |point: DVec3| {
            let clip = matrix * (point - center).extend(1.0);
            (clip.is_finite() && clip.w > 0.0 && clip.z >= 0.0 && clip.z <= clip.w).then(|| {
                let point = clip.truncate() / clip.w;
                rect.center()
                    + egui::vec2(
                        point.x as f32 * rect.width() * 0.5,
                        -point.y as f32 * rect.height() * 0.5,
                    )
            })
        };
        let painter = ui.painter().with_clip_rect(rect);
        let normal = if ui.visuals().dark_mode {
            egui::Color32::from_rgb(100, 225, 187)
        } else {
            egui::Color32::from_rgb(0, 107, 78)
        };
        for (edge, active) in &self.lines {
            if let (Some(a), Some(b)) = (project(edge[0]), project(edge[1])) {
                painter.line_segment(
                    [a, b],
                    egui::Stroke::new(
                        if *active { 2.5 } else { 1.5 },
                        if *active {
                            ui.visuals().selection.stroke.color
                        } else {
                            normal
                        },
                    ),
                );
            }
        }
        for (point, label, active) in &self.labels {
            if let Some(point) = project(*point) {
                painter.text(
                    point + egui::vec2(10.0, 10.0),
                    egui::Align2::LEFT_TOP,
                    label,
                    egui::FontId::proportional(12.0),
                    if *active {
                        ui.visuals().selection.stroke.color
                    } else {
                        normal
                    },
                );
            }
        }
    }
}

#[cfg(test)]
pub(crate) fn test_preparation() -> AnalysisFastenerPreparation {
    use spiling_contracts::project::{ProjectId, ProjectRevision};
    AnalysisFastenerPreparation {
        project_id: ProjectId::new(),
        project_revision: ProjectRevision(3),
        reference: FaceRef {
            session_id: SessionId::new(),
            scene_revision: SceneRevision(1),
            occurrence_id: OccurrenceId::new(1).unwrap(),
            definition_id: DefinitionId::from_source_sha256(&[42; 32]),
            face_id: FaceId::from_step_entity(10).unwrap(),
        },
        source_hash: SourceHash::from_digest(&[42; 32]),
        pose: RigidPoseMm::IDENTITY,
        hole: AnalysisHole {
            bore_faces: vec![
                FaceId::from_step_entity(10).unwrap(),
                FaceId::from_step_entity(11).unwrap(),
            ],
            cap_faces: [
                FaceId::from_step_entity(12).unwrap(),
                FaceId::from_step_entity(13).unwrap(),
            ],
            center_mm: [2.0, 3.0, 4.0],
            radius_mm: 2.0,
            depth_mm: 6.0,
        },
        outer_xy_mm: [[-10.0, 12.0], [-5.0, 15.0]],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fastener_display_reserve_covers_cached_lines_preparations_and_outline_scratch() {
        let lines = MAX_GHOST_LINES * std::mem::size_of::<([DVec3; 2], bool)>();
        let outlines = 4096
            * 3
            * (2 * std::mem::size_of::<[DVec3; 2]>() + std::mem::size_of::<[[u64; 3]; 2]>() + 128);
        let preparations =
            256 * (std::mem::size_of::<AnalysisFastenerPreparation>() + (MAX_BORE_FACES + 2) * 128);
        assert!(lines + outlines + preparations < DISPLAY_RESERVE_BYTES);
    }
    #[test]
    fn fastener_ghosts_are_cached_bounded_and_show_native_end_and_allowed_motion() {
        let preparation = test_preparation();
        let fastener = AnalysisFastener {
            id: 1,
            name: "Mount".into(),
            hole: preparation.hole.clone(),
            attachment: FastenerAttachment::Clamp {
                end: HoleEnd::End,
                washer_diameter_mm: 8.0,
            },
        };
        let mut canvas = Canvas {
            armed: false,
            selected: Some(1),
            saved: vec![fastener],
            preview: None,
            reference: Some(preparation.reference),
            pose: RigidPoseMm::IDENTITY,
            event: None,
        };
        let mut ghosts = Ghosts::default();
        ghosts.prepare(&canvas, None);
        assert!(ghosts.lines.len() <= MAX_GHOST_LINES);
        // Washer touches the actual upper end, not a centered or reversed hole.
        assert!(
            ghosts
                .lines
                .iter()
                .any(|(line, _)| line[0].distance(DVec3::new(6.0, 3.0, 10.0)) < 1e-12)
        );
        let allocation = ghosts.lines.as_ptr();
        let lines = ghosts.lines.clone();
        ghosts.prepare(&canvas, None);
        assert_eq!(allocation, ghosts.lines.as_ptr());
        assert_eq!(lines, ghosts.lines);
        canvas.saved[0].attachment = FastenerAttachment::Pin {
            allow_axial_slide: true,
            allow_spin: true,
        };
        ghosts.prepare(&canvas, None);
        assert!(ghosts.labels[0].1.contains("slide + spin free"));
        assert!(
            ghosts
                .lines
                .iter()
                .any(|(line, _)| line[0].z < 4.0 && line[1].z > 10.0)
        );
        canvas.saved[0].attachment = FastenerAttachment::BondedEngagement {
            end: HoleEnd::Start,
            engagement_mm: 2.0,
        };
        ghosts.prepare(&canvas, None);
        assert!(ghosts.lines.iter().any(|(line,_)| (line[0].z - 6.0).abs() < 1e-12 && (line[1].z - 6.0).abs() < 1e-12));
        // Even an excessive caller cannot grow this display without bound.
        canvas.saved = vec![canvas.saved[0].clone(); 100];
        ghosts.prepare(&canvas, None);
        assert!(ghosts.labels.len() <= MAX_ANALYSIS_FASTENERS + 1);
        assert!(ghosts.lines.len() <= MAX_GHOST_LINES);
    }
}
