// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::*;
impl Runtime {
    pub(super) fn layout_overview(&self) -> Result<PrintLayoutOverview, GeometryError> {
        let snapshot = self.project.snapshot();
        let layouts = snapshot
            .layouts
            .iter()
            .map(|layout| {
                let area = if let Some(h) = &layout.hardware {
                    let mut b = h
                        .selected_face()
                        .expect("validated captured face")
                        .usable_mm;
                    for a in 0..2 {
                        b.min[a] = b.min[a].max(h.setup.printer.build_envelope.min[a]);
                        b.max[a] = b.max[a].min(h.setup.printer.build_envelope.max[a]);
                    }
                    b.min[2] = h.plate.surface_z_mm;
                    b.max[2] = b.min[2];
                    Some(b)
                } else {
                    layout.manufacturing_intent.as_ref().map(|i| {
                        let mut b = i.printer.build_envelope;
                        b.max[2] = b.min[2];
                        b
                    })
                };
                Ok(LayoutOverview {
                    layout_id: layout.layout_id.clone(),
                    build_area_mm: area,
                    captured_plate: layout.hardware.is_some(),
                    parts: layout
                        .occurrences
                        .values()
                        .map(|part| {
                            Ok(LayoutPartBounds {
                                occurrence_id: part.occurrence_id,
                                source_bounds_mm: placement::scene_bounds(
                                    part.pose,
                                    self.definitions[&part.definition_id].record.bounds_mm,
                                )?,
                            })
                        })
                        .collect::<Result<_, GeometryError>>()?,
                })
            })
            .collect::<Result<_, GeometryError>>()?;
        let overview = PrintLayoutOverview {
            session_id: self.session_id.clone(),
            scene_revision: self.revision,
            project_id: self.project.info().project_id,
            project_revision: self.project.info().revision,
            layouts,
        };
        overview.validate()?;
        Ok(overview)
    }
}
