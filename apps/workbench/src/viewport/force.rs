use glam::{DMat4, DQuat, DVec3};
use spiling_contracts::{
    analysis::{
        AnalysisFaceLoad,
        load::{AnalysisFacePreparation, ForceFootprint},
    },
    geometry::FaceRef,
};
use std::sync::Arc;

#[derive(Clone)]
pub struct Contact {
    pub preparation: Arc<AnalysisFacePreparation>,
    pub load: AnalysisFaceLoad,
}
pub struct Canvas {
    pub armed: bool,
    pub active: Option<Contact>,
    pub event: Option<Event>,
}
pub enum Event {
    Leave,
    Point {
        reference: FaceRef,
        seed_mm: [f64; 3],
        place: bool,
    },
    Change {
        load: AnalysisFaceLoad,
        finished: bool,
    },
    Revert(AnalysisFaceLoad),
    Cancel,
}
#[derive(Clone, Copy)]
enum Handle {
    Size,
    Magnitude,
    Direction,
    Position,
}
struct Drag {
    contact: Contact,
    handle: Handle,
    pointer: egui::Pos2,
    radial: egui::Vec2,
    span: f32,
}
#[derive(Default)]
pub struct Interaction {
    drag: Option<Drag>,
}

pub struct Projection {
    pub matrix: DMat4,
    pub center: DVec3,
    pub rect: egui::Rect,
}
impl Projection {
    fn screen(&self, world: DVec3) -> egui::Pos2 {
        let projected = self.matrix.project_point3(world - self.center);
        self.rect.center()
            + egui::vec2(
                projected.x as f32 * self.rect.width() * 0.5,
                -projected.y as f32 * self.rect.height() * 0.5,
            )
    }
    fn ray(&self, pointer: egui::Pos2) -> (DVec3, DVec3) {
        let horizontal = 2.0 * f64::from((pointer.x - self.rect.left()) / self.rect.width()) - 1.0;
        let vertical = 1.0 - 2.0 * f64::from((pointer.y - self.rect.top()) / self.rect.height());
        let inverse = self.matrix.inverse();
        let near = inverse.project_point3(DVec3::new(horizontal, vertical, 0.0));
        let far = inverse.project_point3(DVec3::new(horizontal, vertical, 1.0));
        (near + self.center, (far - near).normalize())
    }
    fn on_face(&self, pointer: egui::Pos2, face: &AnalysisFacePreparation) -> Option<[f64; 3]> {
        let (origin, direction) = self.ray(pointer);
        let rotation = DQuat::from_array(face.pose.rotation_xyzw);
        let origin = rotation.conjugate() * (origin - DVec3::from_array(face.pose.translation_mm));
        let direction = rotation.conjugate() * direction;
        let normal = DVec3::from_array(face.outward_normal);
        let denominator = direction.dot(normal);
        if denominator.abs() < 1e-8 {
            return None;
        }
        let distance = (DVec3::from_array(face.centroid_mm) - origin).dot(normal) / denominator;
        (distance >= 0.0).then(|| (origin + direction * distance).to_array())
    }
}

impl Interaction {
    pub fn cancel(&mut self) -> Option<Event> {
        self.drag
            .take()
            .map(|drag| Event::Revert(drag.contact.load))
    }
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        projection: &Projection,
        canvas: &mut Canvas,
        interactive: bool,
    ) -> bool {
        if !interactive && self.drag.is_some() {
            canvas.event = self.cancel();
        }
        let Some(contact) = canvas.active.clone() else {
            self.drag = None;
            return false;
        };
        let face = &contact.preparation;
        let Ok((area, centroid)) = face.contact(&contact.load.footprint) else {
            return false;
        };
        let rotation = DQuat::from_array(face.pose.rotation_xyzw);
        let world = |point: [f64; 3]| {
            rotation * DVec3::from_array(point) + DVec3::from_array(face.pose.translation_mm)
        };
        let anchor = projection.screen(world(centroid));
        if !anchor.is_finite() || !projection.rect.contains(anchor) {
            return false;
        }
        let color = if ui.visuals().dark_mode {
            egui::Color32::from_rgb(255, 204, 64)
        } else {
            egui::Color32::from_rgb(142, 79, 0)
        };
        let layer = egui::LayerId::new(
            ui.layer_id().order,
            egui::Id::unique("force-intent-overlay"),
        );
        ui.ctx().set_sublayer(ui.layer_id(), layer);
        let painter = ui
            .painter()
            .clone()
            .with_layer_id(layer)
            .with_clip_rect(projection.rect);
        let source_center = match contact.load.footprint {
            ForceFootprint::Disk { center_mm, .. } => center_mm,
            _ => face.centroid_mm,
        };
        let mut polygon = match contact.load.footprint {
            ForceFootprint::Disk { diameter_mm, .. } => {
                let horizontal: f64 = (0..3)
                    .map(|axis| {
                        (source_center[axis] - face.origin_mm[axis])
                            * face.horizontal_direction[axis]
                    })
                    .sum();
                (0..96)
                    .map(|index| {
                        let angle = std::f64::consts::TAU * index as f64 / 96.0;
                        [
                            horizontal + diameter_mm * 0.5 * angle.cos(),
                            source_center[2] - face.origin_mm[2] + diameter_mm * 0.5 * angle.sin(),
                        ]
                    })
                    .collect::<Vec<_>>()
            }
            ForceFootprint::WholeFace {} => vec![
                [0.0, 0.0],
                [face.width_mm, 0.0],
                [face.width_mm, face.height_mm],
                [0.0, face.height_mm],
            ],
        };
        for (axis, boundary, sign) in [
            (0, 0.0, 1.0),
            (0, face.width_mm, -1.0),
            (1, 0.0, 1.0),
            (1, face.height_mm, -1.0),
        ] {
            let previous = std::mem::take(&mut polygon);
            for index in 0..previous.len() {
                let first = previous[index];
                let second = previous[(index + 1) % previous.len()];
                let inside_first = sign * (first[axis] - boundary) >= 0.0;
                let inside_second = sign * (second[axis] - boundary) >= 0.0;
                if inside_first {
                    polygon.push(first);
                }
                if inside_first != inside_second {
                    let fraction = (boundary - first[axis]) / (second[axis] - first[axis]);
                    polygon.push(std::array::from_fn(|coordinate| {
                        first[coordinate] + fraction * (second[coordinate] - first[coordinate])
                    }));
                }
            }
        }
        let outline = polygon
            .iter()
            .map(|point| {
                projection.screen(world(std::array::from_fn(|axis| {
                    face.origin_mm[axis]
                        + face.horizontal_direction[axis] * point[0]
                        + if axis == 2 { point[1] } else { 0.0 }
                })))
            })
            .collect::<Vec<_>>();
        if outline.len() >= 3 && outline.iter().all(|point| point.is_finite()) {
            painter.add(egui::Shape::convex_polygon(
                outline.clone(),
                color.gamma_multiply(0.16),
                egui::Stroke::new(2.0, color),
            ));
        }
        let magnitude = DVec3::from_array(contact.load.force_n).length();
        if !magnitude.is_finite() || magnitude <= 0.0 {
            return false;
        }
        let direction = DVec3::from_array(contact.load.force_n)
            .try_normalize()
            .unwrap_or(-DVec3::from_array(face.outward_normal));
        let direction_screen = projection.screen(world(centroid) + rotation * direction) - anchor;
        let end_on = direction_screen.length_sq() < 0.01;
        let screen_unit = if direction_screen.length_sq() > 0.001 {
            direction_screen.normalized()
        } else {
            egui::vec2(0.0, -1.0)
        };
        let length = (35.0 + 16.0 * magnitude.ln_1p()) as f32;
        let tip = anchor + screen_unit * length.clamp(35.0, 170.0);
        let perpendicular = egui::vec2(-screen_unit.y, screen_unit.x);
        let rotate = tip + perpendicular * 23.0;
        if end_on {
            painter.circle_stroke(anchor, 9.0, egui::Stroke::new(2.0, color));
            if (rotation * direction).dot(projection.ray(anchor).1) > 0.0 {
                painter.line_segment(
                    [anchor - egui::vec2(4.0, 4.0), anchor + egui::vec2(4.0, 4.0)],
                    egui::Stroke::new(2.0, color),
                );
                painter.line_segment(
                    [
                        anchor - egui::vec2(-4.0, 4.0),
                        anchor + egui::vec2(-4.0, 4.0),
                    ],
                    egui::Stroke::new(2.0, color),
                );
            } else {
                painter.circle_filled(anchor, 3.0, color);
            }
            painter.line_segment(
                [anchor, tip],
                egui::Stroke::new(1.0, color.gamma_multiply(0.5)),
            );
        } else {
            painter.arrow(anchor, tip - anchor, egui::Stroke::new(3.0, color));
        }
        painter.line_segment([tip, rotate], egui::Stroke::new(1.0, color));
        painter.text(
            tip + perpendicular * -14.0,
            egui::Align2::CENTER_BOTTOM,
            format!("{magnitude:.2} N"),
            egui::FontId::proportional(13.0),
            color,
        );
        painter.text(
            anchor + egui::vec2(10.0, 12.0),
            egui::Align2::LEFT_TOP,
            format!("{area:.2} mm²"),
            egui::FontId::proportional(11.0),
            color,
        );
        if canvas.armed {
            return false;
        }
        let ring_edge = outline
            .iter()
            .max_by(|first, second| {
                first
                    .distance_sq(anchor)
                    .total_cmp(&second.distance_sq(anchor))
            })
            .copied()
            .unwrap_or(anchor);
        let radial = ring_edge - anchor;
        let radial_unit = if radial.length_sq() > 0.001 {
            radial.normalized()
        } else {
            egui::vec2(1.0, 0.0)
        };
        let ring = anchor + radial_unit * radial.length().max(30.0);
        if ring.is_finite() && !matches!(contact.load.footprint, ForceFootprint::WholeFace {}) {
            painter.line_segment([ring_edge, ring], egui::Stroke::new(1.0, color));
        }
        let mut hovered = false;
        for (handle, position, label) in [
            (Handle::Size, ring, "Contact diameter · mm"),
            (Handle::Magnitude, tip, "Force · N"),
            (
                Handle::Direction,
                rotate,
                "Direction · Shift snaps to part axis",
            ),
            (Handle::Position, anchor, "Move contact on face"),
        ] {
            if matches!(handle, Handle::Size | Handle::Position)
                && matches!(contact.load.footprint, ForceFootprint::WholeFace {})
            {
                continue;
            }
            let response = ui
                .interact(
                    egui::Rect::from_center_size(position, egui::vec2(18.0, 18.0)),
                    egui::Id::unique(("force-handle", face.reference.face_id.as_str(), label)),
                    if interactive {
                        egui::Sense::drag()
                    } else {
                        egui::Sense::hover()
                    },
                )
                .on_hover_text(label);
            painter.circle_filled(position, 4.5, color);
            hovered |= response.hovered() || response.dragged();
            if response.drag_started_by(egui::PointerButton::Primary)
                && let Some(pointer) = response.interact_pointer_pos()
            {
                self.drag = Some(Drag {
                    contact: contact.clone(),
                    handle,
                    pointer: ui
                        .input(|input| input.pointer.press_origin())
                        .unwrap_or(pointer),
                    radial: radial_unit,
                    span: radial.length().max(30.0),
                });
            }
        }
        if let Some(drag) = &self.drag {
            if drag.contact.preparation.reference != face.reference {
                self.drag = None;
                return hovered;
            }
            let (pointer, down, shift) = ui.input(|input| {
                (
                    input.pointer.interact_pos(),
                    input.pointer.primary_down(),
                    input.modifiers.shift,
                )
            });
            if let Some(pointer) = pointer {
                let delta = pointer - drag.pointer;
                let mut load = drag.contact.load.clone();
                let mut force = DVec3::from_array(load.force_n);
                let original_center = face
                    .contact(&load.footprint)
                    .map(|contact| contact.1)
                    .unwrap_or(face.centroid_mm);
                let couple = DVec3::from_array(load.moment_n_mm)
                    - DVec3::from_array(original_center).cross(force);
                match drag.handle {
                    Handle::Magnitude => {
                        force *= (f64::from(delta.dot(screen_unit)) * 0.012)
                            .clamp(-20.0, 20.0)
                            .exp();
                    }
                    Handle::Direction => {
                        let inverse = projection.matrix.inverse();
                        let right = (inverse.project_point3(DVec3::X * 0.01)
                            - inverse.project_point3(DVec3::ZERO))
                        .normalize();
                        let up = (inverse.project_point3(DVec3::Y * 0.01)
                            - inverse.project_point3(DVec3::ZERO))
                        .normalize();
                        let turn = DQuat::from_axis_angle(up, f64::from(delta.x) * 0.012)
                            * DQuat::from_axis_angle(right, f64::from(delta.y) * 0.012);
                        force = rotation.conjugate() * (turn * (rotation * force));
                        if shift {
                            let axis = (0..3)
                                .max_by(|first, second| {
                                    force[*first].abs().total_cmp(&force[*second].abs())
                                })
                                .unwrap();
                            force = DVec3::AXES[axis] * force[axis].signum() * force.length();
                        }
                    }
                    Handle::Size => {
                        if let ForceFootprint::Disk { diameter_mm, .. } = &mut load.footprint {
                            *diameter_mm = (*diameter_mm
                                * (1.0 + f64::from(delta.dot(drag.radial) / drag.span)))
                            .clamp(0.01, 1e6);
                        }
                    }
                    Handle::Position => {
                        if let (Some(point), ForceFootprint::Disk { center_mm, .. }) =
                            (projection.on_face(pointer, face), &mut load.footprint)
                            && let Ok(projected) = face.project(point)
                        {
                            *center_mm = projected;
                        }
                    }
                }
                if let Ok(load) = face.load_on(load.footprint, force.to_array(), couple.to_array())
                {
                    canvas.event = Some(Event::Change {
                        load,
                        finished: !down,
                    });
                }
            }
            if !down {
                self.drag = None;
            }
            return true;
        }
        hovered
    }
}

#[cfg(test)]
pub(crate) fn test_contact() -> Contact {
    use spiling_contracts::{geometry::*, project::*};
    let preparation = Arc::new(AnalysisFacePreparation {
        project_id: ProjectId::new(),
        project_revision: ProjectRevision(2),
        reference: FaceRef {
            session_id: SessionId::new(),
            scene_revision: SceneRevision(1),
            occurrence_id: OccurrenceId::new(1).unwrap(),
            definition_id: DefinitionId::from_source_sha256(&[1; 32]),
            face_id: FaceId::from_step_entity(1).unwrap(),
        },
        source_hash: SourceHash::from_digest(&[1; 32]),
        pose: RigidPoseMm::IDENTITY,
        area_mm2: 120.0,
        centroid_mm: [10.0, 0.0, 3.0],
        outward_normal: [0.0, -1.0, 0.0],
        origin_mm: [0.0; 3],
        horizontal_direction: [1.0, 0.0, 0.0],
        width_mm: 20.0,
        height_mm: 6.0,
    });
    let load = preparation
        .load_on(
            ForceFootprint::Disk {
                center_mm: preparation.centroid_mm,
                diameter_mm: 4.0,
            },
            [0.0, 10.0, 0.0],
            [0.0; 3],
        )
        .unwrap();
    Contact { preparation, load }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn projection() -> Projection {
        Projection {
            matrix: glam::dcamera::rh::proj::directx::orthographic(
                0.0, 20.0, 0.0, 6.0, 0.01, 100.0,
            ) * glam::dcamera::rh::view::look_at_mat4(
                DVec3::new(0.0, -20.0, 0.0),
                DVec3::ZERO,
                DVec3::Z,
            ),
            center: DVec3::ZERO,
            rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(800.0, 400.0)),
        }
    }
    fn frame(
        context: &egui::Context,
        interaction: &mut Interaction,
        canvas: &mut Canvas,
        projection: &Projection,
        events: Vec<egui::Event>,
    ) {
        canvas.event = None;
        let input = egui::RawInput {
            screen_rect: Some(projection.rect),
            events,
            ..Default::default()
        };
        let mut output = context.run_ui(input, |ui| {
            ui.interact(
                projection.rect,
                egui::Id::unique("force-test-canvas"),
                egui::Sense::click_and_drag(),
            );
            interaction.show(ui, projection, canvas, true);
        });
        output.textures_delta.clear();
    }
    #[test]
    fn pointer_resize_preserves_force_and_escape_restores_the_entire_load() {
        let context = egui::Context::default();
        let mut interaction = Interaction::default();
        let contact = test_contact();
        let mut canvas = Canvas {
            armed: false,
            active: Some(contact.clone()),
            event: None,
        };
        let projection = projection();
        let pointer = projection.screen(DVec3::from_array([10.0, 0.0, 1.0]));
        frame(
            &context,
            &mut interaction,
            &mut canvas,
            &projection,
            vec![egui::Event::PointerMoved(pointer)],
        );
        frame(
            &context,
            &mut interaction,
            &mut canvas,
            &projection,
            vec![egui::Event::PointerButton {
                pos: pointer,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            }],
        );
        let moved = pointer + egui::vec2(0.0, 20.0);
        frame(
            &context,
            &mut interaction,
            &mut canvas,
            &projection,
            vec![egui::Event::PointerMoved(moved)],
        );
        let Some(Event::Change {
            load,
            finished: false,
        }) = &canvas.event
        else {
            panic!("expected drag, got no change");
        };
        assert_eq!(load.force_n, contact.load.force_n);
        assert_ne!(load.footprint, contact.load.footprint);
        let event = interaction.cancel().unwrap();
        assert!(matches!(event, Event::Revert(load) if load == contact.load));
        assert!(interaction.cancel().is_none());
    }
    #[test]
    fn native_contact_projection_clips_centroid_and_changes_only_lever_arm_on_move() {
        let contact = test_contact();
        let face = &contact.preparation;
        let disk = ForceFootprint::Disk {
            center_mm: [0.0, 0.0, 3.0],
            diameter_mm: 2.0,
        };
        let (area, center) = face.contact(&disk).unwrap();
        assert!((area - std::f64::consts::PI * 0.5).abs() < 1e-10);
        assert!((center[0] - 4.0 / (3.0 * std::f64::consts::PI)).abs() < 1e-10);
        let load = face.load_on(disk, contact.load.force_n, [0.0; 3]).unwrap();
        assert_eq!(load.force_n, contact.load.force_n);
        assert!((load.moment_n_mm[2] - center[0] * 10.0).abs() < 1e-10);
        assert!(face.project([30.0, 0.0, 3.0]).is_err());
    }

    #[test]
    fn magnitude_and_direction_drags_finish_once_without_a_background_solve() {
        for rotate in [false, true] {
            let context = egui::Context::default();
            let mut interaction = Interaction::default();
            let contact = test_contact();
            let mut canvas = Canvas {
                armed: false,
                active: Some(contact.clone()),
                event: None,
            };
            let projection = projection();
            let anchor = projection.screen(DVec3::from_array(contact.preparation.centroid_mm));
            let pointer = anchor
                + egui::vec2(
                    if rotate { 23.0 } else { 0.0 },
                    -(35.0 + 16.0 * 10.0_f64.ln_1p()) as f32,
                );
            frame(
                &context,
                &mut interaction,
                &mut canvas,
                &projection,
                vec![egui::Event::PointerMoved(pointer)],
            );
            frame(
                &context,
                &mut interaction,
                &mut canvas,
                &projection,
                vec![egui::Event::PointerButton {
                    pos: pointer,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                }],
            );
            let moved = pointer
                + if rotate {
                    egui::vec2(30.0, 0.0)
                } else {
                    egui::vec2(0.0, -30.0)
                };
            frame(
                &context,
                &mut interaction,
                &mut canvas,
                &projection,
                vec![egui::Event::PointerMoved(moved)],
            );
            let Some(Event::Change {
                load,
                finished: false,
            }) = &canvas.event
            else {
                panic!("expected active drag");
            };
            assert_eq!(load.footprint, contact.load.footprint);
            if rotate {
                assert!((DVec3::from_array(load.force_n).length() - 10.0).abs() < 1e-10);
                assert_ne!(load.force_n, contact.load.force_n);
            } else {
                assert!(DVec3::from_array(load.force_n).length() > 10.0);
            }
            canvas.active.as_mut().unwrap().load = load.clone();
            frame(
                &context,
                &mut interaction,
                &mut canvas,
                &projection,
                vec![egui::Event::PointerButton {
                    pos: moved,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                }],
            );
            assert!(matches!(
                canvas.event,
                Some(Event::Change { finished: true, .. })
            ));
            frame(&context, &mut interaction, &mut canvas, &projection, vec![]);
            assert!(canvas.event.is_none());
        }
    }
}
