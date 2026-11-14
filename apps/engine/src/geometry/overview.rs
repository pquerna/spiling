// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::*;
impl Runtime {
    /// Plate context and counts; definitions and parts are read in pages.
    pub(super) fn layout_overview(&self) -> Result<LayoutOverviewSummary, GeometryError> {
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
                Ok(LayoutOverviewHeader {
                    layout_id: layout.layout_id.clone(),
                    build_area_mm: area,
                    captured_plate: layout.hardware.is_some(),
                    part_count: count(layout.occurrences.len())?,
                })
            })
            .collect::<Result<_, GeometryError>>()?;
        let summary = LayoutOverviewSummary {
            session_id: self.session_id.clone(),
            scene_revision: self.revision,
            project_id: self.project.info().project_id,
            project_revision: self.project.info().revision,
            layouts,
            definition_count: count(snapshot.sources.len())?,
        };
        summary.validate()?;
        Ok(summary)
    }

    /// One bounded page of overview definitions or layout-ordered parts.
    pub(super) fn layout_overview_page(
        &self,
        kind: LayoutOverviewPageKind,
        offset: u32,
    ) -> Result<GeometryResponse, GeometryError> {
        let snapshot = self.project.snapshot();
        let page = SCENE_PAGE_SIZE as usize;
        let start = offset as usize;
        let (definitions, parts, total) = match kind {
            LayoutOverviewPageKind::Definitions => (
                snapshot
                    .sources
                    .keys()
                    .skip(start)
                    .take(page)
                    .map(|id| self.definitions[id].record.clone())
                    .collect(),
                Vec::new(),
                snapshot.sources.len(),
            ),
            LayoutOverviewPageKind::Parts => {
                let mut parts = Vec::with_capacity(page);
                let mut skip = start;
                let mut total = 0;
                for (index, layout) in snapshot.layouts.iter().enumerate() {
                    total += layout.occurrences.len();
                    if skip >= layout.occurrences.len() {
                        skip -= layout.occurrences.len();
                        continue;
                    }
                    for part in layout.occurrences.values().skip(skip) {
                        if parts.len() == page {
                            break;
                        }
                        parts.push(LayoutOverviewPart {
                            layout_index: count(index)?,
                            part: LayoutPartBounds {
                                occurrence_id: part.occurrence_id,
                                definition_id: part.definition_id.clone(),
                                pose: part.pose,
                                source_bounds_mm: placement::scene_bounds(
                                    part.pose,
                                    self.definitions[&part.definition_id].record.bounds_mm,
                                )?,
                            },
                        });
                    }
                    skip = 0;
                }
                (Vec::new(), parts, total)
            }
        };
        if start > total {
            return Err(unknown());
        }
        let end = start + definitions.len() + parts.len();
        Ok(GeometryResponse::LayoutOverviewPage {
            definitions,
            parts,
            next_offset: (end < total).then(|| count(end)).transpose()?,
        })
    }
}
fn count(value: usize) -> Result<u32, GeometryError> {
    u32::try_from(value).map_err(|_| resource("overview count exceeds its u32 width"))
}
