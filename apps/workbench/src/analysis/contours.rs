// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Sampled-contour display and point-to-point inspection. Never native metrology.
use crate::scene::Section;
use glam::{DVec2, DVec3};
use spiling_contracts::geometry::OccurrenceId;
use std::sync::{Arc, Weak};
const MAX_PREVIEW_POINTS: usize = 65_536;
pub struct ContourPreview {
    section: Weak<Section>,
    scope: Option<OccurrenceId>,
    loops: Vec<Vec<DVec2>>,
    bounds: [DVec2; 2],
    area: f64,
    perimeter: f64,
    samples: Vec<DVec2>,
    measured: Vec<DVec2>,
    allowance: f64,
}
impl ContourPreview {
    pub fn matches(&self, section: &Arc<Section>, scope: Option<OccurrenceId>) -> bool {
        self.section.ptr_eq(&Arc::downgrade(section)) && self.scope == scope
    }
    pub fn load(section: &Arc<Section>, scope: Option<OccurrenceId>) -> Result<Self, String> {
        let count = section
            .loops
            .iter()
            .filter(|l| scope.is_none_or(|id| l.occurrence_id == id))
            .map(|l| l.point_count as usize)
            .sum::<usize>();
        if count > MAX_PREVIEW_POINTS {
            return Err("2D contour preview exceeds its 65,536-sample budget; native 3D contour remains available.".into());
        }
        let frame = section.summary.frame;
        let u = DVec3::from_array(frame.x_axis);
        let v = DVec3::from_array(frame.y_axis);
        let mut result = Self {
            section: Arc::downgrade(section),
            scope,
            loops: Vec::new(),
            bounds: [DVec2::splat(f64::INFINITY), DVec2::splat(f64::NEG_INFINITY)],
            area: 0.0,
            perimeter: 0.0,
            samples: Vec::with_capacity(count),
            measured: Vec::new(),
            allowance: section.summary.sampling_tolerance_mm,
        };
        for chunk in &section.chunks {
            let checked = spiling_contracts::display::validate_section_chunk(
                &chunk.bytes,
                &chunk.metadata,
                &section.summary,
                &section.summary.session_id,
                section.summary.artifact_id,
                chunk.metadata.chunk_index,
            )
            .map_err(|e| e.to_string())?;
            for i in 0..chunk.metadata.loop_count as usize {
                let ordinal = chunk.metadata.first_loop_ordinal as usize + i;
                let metadata = section
                    .loops
                    .get(ordinal)
                    .ok_or("Section loop metadata missing")?;
                if scope.is_some_and(|id| id != metadata.occurrence_id) {
                    continue;
                }
                let start = checked.loop_offsets.get(i).ok_or("Section start missing")? as usize;
                let end = checked
                    .loop_offsets
                    .get(i + 1)
                    .ok_or("Section end missing")? as usize;
                let mut points = Vec::with_capacity(end - start);
                for j in start..end {
                    let relative = DVec3::from_array(
                        checked
                            .points_relative_mm
                            .get(j)
                            .ok_or("Section point missing")?,
                    ) + (DVec3::from_array(checked.plane_origin_mm)
                        - DVec3::from_array(frame.origin_mm));
                    let p = DVec2::new(relative.dot(u), relative.dot(v));
                    if !p.is_finite() {
                        return Err("Contour projection is not finite".into());
                    }
                    result.bounds[0] = result.bounds[0].min(p);
                    result.bounds[1] = result.bounds[1].max(p);
                    points.push(p);
                    result.samples.push(p);
                }
                if let Some(anchor) = points.first().copied() {
                    let mut signed = 0.0;
                    for (a, b) in points.iter().zip(points.iter().cycle().skip(1)) {
                        signed += (*a - anchor).perp_dot(*b - anchor) * 0.5;
                        result.perimeter += a.distance(*b);
                    }
                    result.area += signed.abs() * if metadata.is_hole { -1.0 } else { 1.0 };
                }
                result.loops.push(points);
            }
        }
        Ok(result)
    }
    pub fn show(&mut self, ui: &mut egui::Ui) {
        if self.samples.is_empty() {
            ui.label("Empty section in this scope.");
            return;
        }
        ui.label(format!(
            "Sampled loop area ≈ {:.3} mm² · boundary ≈ {:.3} mm",
            self.area, self.perimeter
        ));
        ui.weak("Areas sum occurrences; overlaps are counted separately. These are sampled display estimates.");
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), 175.0),
            egui::Sense::click(),
        );
        let center = (self.bounds[0] + self.bounds[1]) * 0.5;
        let span = self.bounds[1] - self.bounds[0];
        let scale = (f64::from(rect.width() - 16.0) / span.x.max(1e-6))
            .min(f64::from(rect.height() - 16.0) / span.y.max(1e-6));
        let screen = |p: DVec2| {
            rect.center()
                + egui::vec2(
                    ((p.x - center.x) * scale) as f32,
                    (-(p.y - center.y) * scale) as f32,
                )
        };
        ui.painter()
            .rect_filled(rect, 4.0, ui.visuals().extreme_bg_color);
        for points in &self.loops {
            ui.painter().add(egui::Shape::closed_line(
                points.iter().map(|p| screen(*p)).collect(),
                egui::Stroke::new(1.5, ui.visuals().selection.stroke.color),
            ));
        }
        if response.clicked()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            let nearest = self.samples.iter().min_by(|a, b| {
                screen(**a)
                    .distance_sq(pointer)
                    .total_cmp(&screen(**b).distance_sq(pointer))
            });
            if let Some(point) = nearest.filter(|p| screen(**p).distance(pointer) <= 12.0) {
                if self.measured.len() == 2 {
                    self.measured.clear();
                }
                self.measured.push(*point);
            }
        }
        for p in &self.measured {
            ui.painter()
                .circle_filled(screen(*p), 4.0, ui.visuals().warn_fg_color);
        }
        if self.measured.len() == 2 {
            ui.painter().line_segment(
                [screen(self.measured[0]), screen(self.measured[1])],
                egui::Stroke::new(1.0, ui.visuals().warn_fg_color),
            );
            ui.label(format!(
                "Sample-to-sample distance {:.4} mm · endpoint sampling allowance {:.4} mm each",
                self.measured[0].distance(self.measured[1]),
                self.allowance
            ));
        } else {
            ui.weak("Click two contour samples to measure a distance.");
        }
    }
}
