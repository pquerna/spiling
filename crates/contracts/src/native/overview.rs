// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::{required, *};
use crate::project::{MAX_PRINT_LAYOUTS, PrintLayoutId, ProjectId, ProjectRevision};
impl TryFrom<rpc::PrintLayoutOverview> for PrintLayoutOverview {
    type Error = String;
    fn try_from(v: rpc::PrintLayoutOverview) -> Result<Self, String> {
        if v.layouts.len() > MAX_PRINT_LAYOUTS
            || v.layouts.iter().map(|l| l.parts.len()).sum::<usize>() > MAX_OCCURRENCES as usize
        {
            return Err("overview exceeds project limits".into());
        }
        let value = Self {
            session_id: SessionId::parse(v.session_id).map_err(|e| e.to_string())?,
            scene_revision: SceneRevision(v.scene_revision),
            project_id: ProjectId::parse(v.project_id).map_err(|e| e.to_string())?,
            project_revision: ProjectRevision(v.project_revision),
            layouts: v
                .layouts
                .into_iter()
                .map(|l| {
                    Ok(LayoutOverview {
                        layout_id: PrintLayoutId::parse(l.layout_id).map_err(|e| e.to_string())?,
                        build_area_mm: l.build_area_mm.map(TryInto::try_into).transpose()?,
                        captured_plate: l.captured_plate,
                        parts: l
                            .parts
                            .into_iter()
                            .map(|p| {
                                Ok(LayoutPartBounds {
                                    occurrence_id: OccurrenceId::new(p.occurrence_id)
                                        .map_err(|e| e.to_string())?,
                                    source_bounds_mm: required(
                                        p.source_bounds_mm,
                                        "source bounds",
                                    )?
                                    .try_into()?,
                                })
                            })
                            .collect::<Result<_, String>>()?,
                    })
                })
                .collect::<Result<_, String>>()?,
        };
        value.validate().map_err(|e| e.to_string())?;
        Ok(value)
    }
}
impl TryFrom<PrintLayoutOverview> for rpc::PrintLayoutOverview {
    type Error = String;
    fn try_from(v: PrintLayoutOverview) -> Result<Self, String> {
        v.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            session_id: v.session_id.as_str().into(),
            scene_revision: v.scene_revision.get(),
            project_id: v.project_id.as_str().into(),
            project_revision: v.project_revision.get(),
            layouts: v
                .layouts
                .into_iter()
                .map(|l| {
                    Ok(rpc::LayoutOverview {
                        layout_id: l.layout_id.as_str().into(),
                        captured_plate: l.captured_plate,
                        build_area_mm: l.build_area_mm.map(TryInto::try_into).transpose()?,
                        parts: l
                            .parts
                            .into_iter()
                            .map(|p| {
                                Ok(rpc::LayoutPartBounds {
                                    occurrence_id: p.occurrence_id.get(),
                                    source_bounds_mm: Some(p.source_bounds_mm.try_into()?),
                                })
                            })
                            .collect::<Result<_, String>>()?,
                    })
                })
                .collect::<Result<_, String>>()?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overview_roundtrip_preserves_scope_and_refuses_duplicate_or_missing_context() {
        let layout_id = PrintLayoutId::new();
        let v = PrintLayoutOverview {
            session_id: SessionId::parse("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            scene_revision: SceneRevision(2),
            project_id: ProjectId::new(),
            project_revision: ProjectRevision(3),
            layouts: vec![LayoutOverview {
                layout_id,
                build_area_mm: Some(AabbMm::new([0.0; 3], [200.0, 100.0, 0.0]).unwrap()),
                captured_plate: true,
                parts: vec![LayoutPartBounds {
                    occurrence_id: OccurrenceId::new(1).unwrap(),
                    source_bounds_mm: AabbMm::new([-1.0; 3], [1.0; 3]).unwrap(),
                }],
            }],
        };
        let wire: rpc::PrintLayoutOverview = v.clone().try_into().unwrap();
        assert_eq!(PrintLayoutOverview::try_from(wire.clone()).unwrap(), v);
        let mut bad = wire.clone();
        bad.layouts.push(bad.layouts[0].clone());
        assert!(PrintLayoutOverview::try_from(bad).is_err());
        let mut bad = wire.clone();
        bad.layouts[0].build_area_mm = None;
        assert!(PrintLayoutOverview::try_from(bad).is_err());
        let mut bad = wire;
        bad.layouts[0].parts[0].source_bounds_mm = None;
        assert!(PrintLayoutOverview::try_from(bad).is_err());
        let command = GeometryCommand::GetLayoutOverview {
            session_id: v.session_id,
            scene_revision: v.scene_revision,
            project_revision: v.project_revision,
        };
        let GeometryCall::Execute(wire) = geometry_call(command.clone(), "").unwrap() else {
            panic!("short overview")
        };
        assert_eq!(GeometryCommand::try_from(wire).unwrap(), command);
    }
}
