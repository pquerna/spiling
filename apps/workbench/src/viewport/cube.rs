// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;

pub(super) fn bounds(canvas: egui::Rect) -> Option<egui::Rect> {
    (canvas.width() >= 240.0 && canvas.height() >= 180.0).then(|| {
        egui::Rect::from_min_size(
            canvas.right_top() + egui::vec2(-106.0, 10.0),
            egui::vec2(96.0, 112.0),
        )
    })
}

struct Face {
    normal: DVec3,
    horizontal: DVec3,
    vertical: DVec3,
    label: &'static str,
    points: [egui::Pos2; 4],
}

fn faces(rotation: DQuat, rect: egui::Rect) -> Vec<Face> {
    let inverse = rotation.conjugate();
    let project = |point: DVec3| {
        let point = inverse * point;
        rect.center() + egui::vec2(point.x as f32, -point.y as f32) * 24.0
    };
    let mut faces = Vec::with_capacity(3);
    for (normal, horizontal, label) in [
        (DVec3::Z, DVec3::X, "Top"),
        (-DVec3::Z, -DVec3::X, "Bottom"),
        (-DVec3::Y, DVec3::X, "Front"),
        (DVec3::Y, -DVec3::X, "Back"),
        (-DVec3::X, -DVec3::Y, "Left"),
        (DVec3::X, DVec3::Y, "Right"),
    ] {
        if (inverse * normal).z <= 1e-6 {
            continue;
        }
        let vertical = normal.cross(horizontal);
        faces.push(Face {
            normal,
            horizontal,
            vertical,
            label,
            points: [
                project(normal - horizontal - vertical),
                project(normal + horizontal - vertical),
                project(normal + horizontal + vertical),
                project(normal - horizontal + vertical),
            ],
        });
    }
    faces
}

fn hit(face: &Face, pointer: egui::Pos2) -> Option<DVec3> {
    let horizontal = face.points[1] - face.points[0];
    let vertical = face.points[3] - face.points[0];
    let delta = pointer - face.points[0];
    let determinant = horizontal.x * vertical.y - horizontal.y * vertical.x;
    if determinant.abs() < 1e-6 {
        return None;
    }
    let first = (delta.x * vertical.y - delta.y * vertical.x) / determinant;
    let second = (horizontal.x * delta.y - horizontal.y * delta.x) / determinant;
    if !(0.0..=1.0).contains(&first) || !(0.0..=1.0).contains(&second) {
        return None;
    }
    let band = |value: f32| {
        if value < 0.2 {
            -1.0
        } else if value > 0.8 {
            1.0
        } else {
            0.0
        }
    };
    Some((face.normal + face.horizontal * band(first) + face.vertical * band(second)).normalize())
}

pub(super) fn show(
    ui: &mut egui::Ui,
    canvas: egui::Rect,
    rotation: DQuat,
    plate: bool,
) -> Option<DVec3> {
    let rect = bounds(canvas)?;
    let layer = layer(ui);
    ui.ctx().set_sublayer(ui.layer_id(), layer);
    let overlay = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("navigation-cube")
            .layer_id(layer)
            .max_rect(rect),
    );
    let response = overlay.interact(
        rect,
        overlay.scope_id().with("cube"),
        egui::Sense::click_and_drag(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::Role::Button,
            true,
            "View cube. Drag to orbit. Use View menu for all named views.",
        )
    });
    let home = overlay
        .interact(
            egui::Rect::from_min_size(
                rect.left_bottom() + egui::vec2(0.0, -24.0),
                egui::vec2(32.0, 24.0),
            ),
            overlay.scope_id().with("home"),
            egui::Sense::click(),
        )
        .on_hover_text("Isometric · 0");
    home.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Isometric view"));
    let painter = overlay.painter();
    painter.rect_filled(rect, 6.0, ui.visuals().window_fill().gamma_multiply(0.94));
    let cube = egui::Rect::from_min_size(rect.min, egui::vec2(96.0, 88.0));
    let faces = faces(rotation, cube);
    let pointer = response.hover_pos();
    let mut selected = None;
    for face in &faces {
        let hovered = pointer.and_then(|pointer| hit(face, pointer));
        let color = if hovered.is_some() {
            ui.visuals().selection.bg_fill
        } else {
            ui.visuals().widgets.inactive.bg_fill
        };
        painter.add(egui::Shape::convex_polygon(
            face.points.to_vec(),
            color,
            ui.visuals().widgets.noninteractive.fg_stroke,
        ));
        let center = face
            .points
            .iter()
            .fold(egui::Vec2::ZERO, |sum, point| sum + point.to_vec2())
            * 0.25;
        painter.text(
            center.to_pos2(),
            egui::Align2::CENTER_CENTER,
            face.label,
            egui::FontId::proportional(10.0),
            ui.visuals().text_color(),
        );
        if hovered.is_some() {
            selected = hovered;
        }
    }
    painter.text(
        rect.center_bottom() + egui::vec2(16.0, -6.0),
        egui::Align2::CENTER_BOTTOM,
        if plate { "Plate" } else { "Project" },
        egui::FontId::proportional(10.0),
        ui.visuals().weak_text_color(),
    );
    painter.text(
        rect.left_bottom() + egui::vec2(16.0, -12.0),
        egui::Align2::CENTER_CENTER,
        "3D",
        egui::FontId::proportional(11.0),
        ui.visuals().text_color(),
    );
    if home.clicked() {
        return Some(StandardView::Isometric.direction());
    }
    response
        .on_hover_text("Click a face, edge or corner to snap; drag to orbit")
        .clicked()
        .then_some(selected)
        .flatten()
}

pub(super) fn layer(ui: &egui::Ui) -> egui::LayerId {
    egui::LayerId::new(
        ui.layer_id().order,
        ui.scope_id().with("navigation-cube-layer"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_face_edge_and_corner_targets_are_available_without_scene_picking() {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(96.0, 96.0));
        for view in StandardView::ALL {
            let faces = faces(navigation::orientation(view.direction()), rect);
            assert!(!faces.is_empty());
            for face in faces {
                let center = face.points[0]
                    + ((face.points[1] - face.points[0]) + (face.points[3] - face.points[0])) * 0.5;
                assert!((hit(&face, center).unwrap() - face.normal).length() < 1e-12);
                assert!(hit(&face, egui::pos2(-100.0, -100.0)).is_none());
                let corner = face.points[0].lerp(center, 0.1);
                assert!(
                    (hit(&face, corner).unwrap()
                        - (face.normal - face.horizontal - face.vertical).normalize())
                    .length()
                        < 1e-12
                );
            }
        }
    }
    #[test]
    fn small_canvas_keeps_named_menu_instead_of_obscuring_content() {
        assert!(
            bounds(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(200.0, 200.0)
            ))
            .is_none()
        );
    }
}
