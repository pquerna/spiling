// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;

impl Viewport {
    pub(super) fn reconcile_inspection(
        &mut self,
        summary: Option<&SceneSummary>,
        interactive: bool,
        selection: (Option<FaceRef>, Option<OccurrenceId>),
        now: f64,
        animations_enabled: bool,
    ) {
        let same = |first: &SceneSummary, second: &SceneSummary| {
            first.session_id == second.session_id
                && first.layout_id == second.layout_id
                && first.revision == second.revision
        };
        let changed = match (&self.focus_scene, summary) {
            (Some(old), Some(current)) => !same(old, current),
            (None, None) => false,
            _ => true,
        };
        if changed || !interactive {
            if let Some(reveal) = self.reveal.take() {
                self.dim_context = reveal.dim_context;
            }
            self.attention = None;
            self.focus_problem = None;
            if changed {
                self.motion = None;
                self.previous_camera = None;
            }
            self.pending_focus = self.pending_focus.take().filter(|request| {
                interactive && summary.is_some_and(|current| same(current, &request.summary))
            });
            self.focus_scene = summary.cloned();
        }
        if self
            .last_selection
            .as_ref()
            .is_some_and(|old| old != &selection)
        {
            let pending = self.pending_focus.take();
            self.close_reveal(now, animations_enabled);
            self.attention = None;
            self.pending_focus = pending;
        }
        self.last_selection = Some(selection);
    }
}

pub(super) struct Reveal {
    pub target: crate::scene::FocusTarget,
    pub normal: DVec3,
    pub offset_mm: f64,
    pub camera: CameraState,
    pub dim_context: bool,
    pub attention: Option<[[u32; 4]; 2]>,
    pub previous_camera: Option<CameraState>,
}
impl Reveal {
    pub fn new(target: crate::scene::FocusTarget, camera: CameraState, dim_context: bool) -> Self {
        Self {
            target,
            normal: target
                .normal
                .unwrap_or_else(|| DQuat::from_array(camera.rotation) * DVec3::Z),
            offset_mm: if target.normal.is_some() { 0.002 } else { 0.0 },
            camera,
            dim_context,
            attention: None,
            previous_camera: None,
        }
    }
    pub fn plane(&self) -> PlaneMm {
        PlaneMm {
            origin_mm: (self.target.center() + self.normal * self.offset_mm).to_array(),
            normal: self.normal.to_array(),
        }
    }
    pub fn radius(&self) -> f64 {
        (DVec3::from_array(self.target.bounds_mm.max)
            - DVec3::from_array(self.target.bounds_mm.min))
        .length()
        .max(0.02)
            * 0.5
    }
}

pub(super) fn participant_mask(
    scene: &Scene,
    participants: &[OccurrenceId],
) -> Option<[[u32; 4]; 2]> {
    if participants.is_empty() || scene.occurrences.len() > MAX_OCCURRENCES as usize {
        return None;
    }
    let mut words = [[0u32; 4]; 2];
    for id in participants {
        let index = scene
            .occurrences
            .iter()
            .position(|item| item.occurrence_id == *id)?;
        if !mark_participant(&mut words, index) {
            return None;
        }
    }
    Some(words)
}
fn mark_participant(words: &mut [[u32; 4]; 2], index: usize) -> bool {
    if index >= 256 {
        return false;
    }
    words[index / 128][(index / 32) % 4] |= 1 << (index % 32);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cutaway_has_independent_plane_and_retains_the_original_camera() {
        let camera = Viewport::default().camera();
        let target = crate::scene::FocusTarget {
            bounds_mm: AabbMm {
                min: [10.0, 20.0, 30.0],
                max: [14.0, 26.0, 32.0],
            },
            normal: Some(DVec3::X),
        };
        let mut reveal = Reveal::new(target, camera.clone(), false);
        assert!(reveal.offset_mm > 0.0);
        reveal.offset_mm = 0.0;
        assert_eq!(reveal.plane().normal, [1.0, 0.0, 0.0]);
        assert_eq!(reveal.plane().origin_mm, [12.0, 23.0, 31.0]);
        reveal.offset_mm = 2.0;
        assert_eq!(reveal.plane().origin_mm, [14.0, 23.0, 31.0]);
        assert_eq!(reveal.camera.pan, camera.pan);
        assert_eq!(reveal.camera.distance, camera.distance);
        assert!(!reveal.dim_context);
    }
    #[test]
    fn closing_reveal_restores_camera_and_context_without_touching_section_state() {
        let mut viewport = Viewport::default();
        let camera = viewport.camera();
        let section = crate::analysis::SectionView {
            enabled: true,
            offset_mm: 37.0,
            ..Default::default()
        };
        let target = crate::scene::FocusTarget {
            bounds_mm: AabbMm {
                min: [0.0; 3],
                max: [10.0; 3],
            },
            normal: None,
        };
        viewport.reveal = Some(Reveal::new(target, camera.clone(), false));
        viewport.dim_context = true;
        viewport.pan = DVec3::splat(42.0);
        assert!(viewport.close_reveal(1.0, false));
        assert!(viewport.reveal.is_none());
        assert!(!viewport.dim_context);
        assert_eq!(viewport.camera().pan, camera.pan);
        assert_eq!(viewport.camera().distance, camera.distance);
        assert!(section.enabled);
        assert_eq!(section.offset_mm, 37.0);
        assert!(!viewport.close_reveal(2.0, false));
    }
    #[test]
    fn multiple_participants_cover_word_boundaries_without_aliasing() {
        const {
            assert!(MAX_OCCURRENCES <= 256);
        }
        let mut words = [[0u32; 4]; 2];
        for index in [0, 31, 32, 127, 128, 255] {
            assert!(mark_participant(&mut words, index));
        }
        assert_eq!(
            words,
            [[0x80000001, 1, 0, 0x80000000], [1, 0, 0, 0x80000000]]
        );
        assert!(!mark_participant(&mut words, 256));
        assert_eq!(
            words
                .iter()
                .flatten()
                .map(|word| word.count_ones())
                .sum::<u32>(),
            6
        );
    }
    #[test]
    fn canceling_an_area_removes_the_previous_cut_and_owner_attention() {
        let mut viewport = Viewport::default();
        let target = crate::scene::FocusTarget {
            bounds_mm: AabbMm {
                min: [0.0; 3],
                max: [10.0; 3],
            },
            normal: Some(DVec3::Z),
        };
        viewport.reveal = Some(Reveal::new(target, viewport.camera(), false));
        viewport.attention = Some([[1, 0, 0, 0], [0; 4]]);
        viewport.dim_context = true;
        viewport.cancel_area();
        assert!(viewport.reveal.is_none());
        assert!(viewport.attention.is_none());
        assert!(viewport.motion.is_none());
        assert!(viewport.pending_focus.is_none());
        assert!(!viewport.dim_context);
    }
    #[test]
    fn fresh_area_request_survives_first_show_and_selection_change_as_one_unit() {
        let mut viewport = Viewport::default();
        let summary = SceneSummary {
            layout_id: spiling_contracts::project::PrintLayoutId::new(),
            session_id: SessionId::new(),
            revision: SceneRevision(2),
            definition_count: 1,
            occurrence_count: 1,
            bounds_mm: Some(AabbMm {
                min: [0.0; 3],
                max: [10.0; 3],
            }),
            unique_mesh_bytes: 0,
        };
        let target = crate::scene::FocusTarget {
            bounds_mm: summary.bounds_mm.unwrap(),
            normal: Some(DVec3::X),
        };
        let mask = [[1, 0, 0, 0], [0; 4]];
        viewport.last_selection = Some((None, None));
        viewport.pending_focus = Some(focus::Request {
            summary: summary.clone(),
            target,
            participants: Some(mask),
            reveal: true,
            started_reveal: false,
            area_applied: false,
        });
        viewport.reconcile_inspection(
            Some(&summary),
            true,
            (None, Some(OccurrenceId::new(1).unwrap())),
            1.0,
            false,
        );
        viewport.apply_focus_area();
        assert_eq!(viewport.attention, Some(mask));
        assert!(viewport.reveal.is_some());
        assert!(viewport.pending_focus.as_ref().unwrap().started_reveal);
        assert!(viewport.dim_context);
        let stale = SceneSummary {
            revision: SceneRevision(3),
            ..summary
        };
        viewport.reconcile_inspection(Some(&stale), true, (None, None), 2.0, false);
        assert!(viewport.pending_focus.is_none());
        assert!(viewport.reveal.is_none());
        assert!(viewport.attention.is_none());
    }
}
