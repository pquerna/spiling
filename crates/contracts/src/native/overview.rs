// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::{required, *};
use crate::project::{PrintLayoutId, ProjectId, ProjectRevision};
impl TryFrom<rpc::LayoutOverviewSummary> for LayoutOverviewSummary {
    type Error = String;
    fn try_from(v: rpc::LayoutOverviewSummary) -> Result<Self, String> {
        if v.layouts.len() > crate::project::MAX_PRINT_LAYOUTS {
            return Err("overview exceeds project plate limit".into());
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
                    Ok(LayoutOverviewHeader {
                        layout_id: PrintLayoutId::parse(l.layout_id).map_err(|e| e.to_string())?,
                        build_area_mm: l.build_area_mm.map(TryInto::try_into).transpose()?,
                        captured_plate: l.captured_plate,
                        part_count: l.part_count,
                    })
                })
                .collect::<Result<_, String>>()?,
            definition_count: v.definition_count,
        };
        value.validate().map_err(|e| e.to_string())?;
        Ok(value)
    }
}
impl TryFrom<LayoutOverviewSummary> for rpc::LayoutOverviewSummary {
    type Error = String;
    fn try_from(v: LayoutOverviewSummary) -> Result<Self, String> {
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
                    Ok(rpc::LayoutOverviewHeader {
                        layout_id: l.layout_id.as_str().into(),
                        captured_plate: l.captured_plate,
                        build_area_mm: l.build_area_mm.map(TryInto::try_into).transpose()?,
                        part_count: l.part_count,
                    })
                })
                .collect::<Result<_, String>>()?,
            definition_count: v.definition_count,
        })
    }
}
impl TryFrom<rpc::LayoutOverviewPart> for LayoutOverviewPart {
    type Error = String;
    fn try_from(p: rpc::LayoutOverviewPart) -> Result<Self, String> {
        let value = Self {
            layout_index: p.layout_index,
            part: LayoutPartBounds {
                occurrence_id: OccurrenceId::new(p.occurrence_id).map_err(|e| e.to_string())?,
                definition_id: DefinitionId::parse(p.definition_id).map_err(|e| e.to_string())?,
                pose: required(p.pose, "overview pose")?.try_into()?,
                source_bounds_mm: required(p.source_bounds_mm, "source bounds")?.try_into()?,
            },
        };
        value.validate().map_err(|e| e.to_string())?;
        Ok(value)
    }
}
impl TryFrom<LayoutOverviewPart> for rpc::LayoutOverviewPart {
    type Error = String;
    fn try_from(v: LayoutOverviewPart) -> Result<Self, String> {
        v.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            layout_index: v.layout_index,
            occurrence_id: v.part.occurrence_id.get(),
            definition_id: v.part.definition_id.as_str().into(),
            pose: Some(v.part.pose.try_into()?),
            source_bounds_mm: Some(v.part.source_bounds_mm.try_into()?),
        })
    }
}
impl TryFrom<i32> for LayoutOverviewPageKind {
    type Error = String;
    fn try_from(value: i32) -> Result<Self, String> {
        match rpc::LayoutOverviewPageKind::try_from(value)
            .map_err(|_| "unknown LayoutOverviewPageKind".to_string())?
        {
            rpc::LayoutOverviewPageKind::LayoutOverviewDefinitions => Ok(Self::Definitions),
            rpc::LayoutOverviewPageKind::LayoutOverviewParts => Ok(Self::Parts),
            rpc::LayoutOverviewPageKind::Unspecified => {
                Err("unspecified LayoutOverviewPageKind".into())
            }
        }
    }
}
impl From<LayoutOverviewPageKind> for rpc::LayoutOverviewPageKind {
    fn from(value: LayoutOverviewPageKind) -> Self {
        match value {
            LayoutOverviewPageKind::Definitions => Self::LayoutOverviewDefinitions,
            LayoutOverviewPageKind::Parts => Self::LayoutOverviewParts,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::MAX_PRINT_LAYOUTS;
    use prost::Message;

    fn definition(n: u32) -> DefinitionRecord {
        DefinitionRecord {
            definition_id: DefinitionId::from_source_sha256(&{
                let mut hash = [0; 32];
                hash[..4].copy_from_slice(&n.to_le_bytes());
                hash
            }),
            provenance: SourceProvenance {
                normalized: None,
                source_hash: SourceHash::from_bytes(&n.to_le_bytes()),
                source_name: "x".repeat(MAX_DISPLAY_LABEL_BYTES as usize),
                source_unit: SourceUnit::Millimetre,
                uncertainty_mm: Some(0.001),
                source_solid: None,
            },
            face_count: MAX_NATIVE_FACES,
            bounds_mm: AabbMm::new([1.0, 2.0, 3.0], [100.0, 200.0, 300.0]).unwrap(),
            display: DefinitionDisplay::Mesh {
                artifact_id: ArtifactId::new(u32::MAX - n).unwrap(),
            },
        }
    }
    fn part(layout_index: u32, n: u32, definition: &DefinitionRecord) -> LayoutOverviewPart {
        LayoutOverviewPart {
            layout_index,
            part: LayoutPartBounds {
                occurrence_id: OccurrenceId::new(u32::MAX - n).unwrap(),
                definition_id: definition.definition_id.clone(),
                pose: RigidPoseMm {
                    translation_mm: [1.0, 2.0, 3.0],
                    rotation_xyzw: [0.5; 4],
                },
                source_bounds_mm: AabbMm::new([1.0, 2.0, 3.0], [100.0, 200.0, 300.0]).unwrap(),
            },
        }
    }
    fn summary(part_counts: &[u32], definition_count: u32) -> LayoutOverviewSummary {
        LayoutOverviewSummary {
            session_id: SessionId::new(),
            scene_revision: SceneRevision(u32::MAX),
            project_id: ProjectId::new(),
            project_revision: ProjectRevision(u32::MAX),
            layouts: part_counts
                .iter()
                .map(|&part_count| LayoutOverviewHeader {
                    layout_id: PrintLayoutId::new(),
                    captured_plate: true,
                    build_area_mm: Some(AabbMm::new([1.0, 2.0, 3.0], [220.0, 220.0, 3.0]).unwrap()),
                    part_count,
                })
                .collect(),
            definition_count,
        }
    }

    #[test]
    fn overview_pages_roundtrip_and_refuse_inconsistent_context() {
        let header = summary(&[2, 1], 2);
        let wire: rpc::LayoutOverviewSummary = header.clone().try_into().unwrap();
        assert_eq!(
            LayoutOverviewSummary::try_from(wire.clone()).unwrap(),
            header
        );
        let mut bad = wire.clone();
        bad.layouts.push(bad.layouts[0].clone());
        assert!(LayoutOverviewSummary::try_from(bad).is_err());
        let mut bad = wire.clone();
        bad.layouts[0].build_area_mm = None;
        assert!(LayoutOverviewSummary::try_from(bad).is_err());
        let mut bad = wire;
        bad.definition_count = 4;
        assert!(LayoutOverviewSummary::try_from(bad).is_err());

        let definitions = vec![definition(1), definition(2)];
        let parts = vec![
            part(0, 1, &definitions[0]),
            part(0, 2, &definitions[1]),
            part(1, 3, &definitions[0]),
        ];
        for row in &parts {
            let wire: rpc::LayoutOverviewPart = row.clone().try_into().unwrap();
            assert_eq!(&LayoutOverviewPart::try_from(wire.clone()).unwrap(), row);
            let mut bad = wire.clone();
            bad.pose = None;
            assert!(LayoutOverviewPart::try_from(bad).is_err());
            let mut bad = wire;
            bad.source_bounds_mm = None;
            assert!(LayoutOverviewPart::try_from(bad).is_err());
        }
        let overview =
            PrintLayoutOverview::assemble(header.clone(), definitions.clone(), parts.clone())
                .unwrap();
        assert_eq!(overview.layouts[0].parts.len(), 2);
        assert_eq!(overview.layouts[1].parts.len(), 1);
        // Out-of-order plates, wrong per-plate counts, missing pages and
        // absent definitions are refused, never reinterpreted.
        let mut reordered = parts.clone();
        reordered.swap(1, 2);
        assert!(
            PrintLayoutOverview::assemble(header.clone(), definitions.clone(), reordered).is_err()
        );
        let mut moved = parts.clone();
        moved[1].layout_index = 1;
        assert!(PrintLayoutOverview::assemble(header.clone(), definitions.clone(), moved).is_err());
        assert!(
            PrintLayoutOverview::assemble(header.clone(), definitions.clone(), parts[..2].to_vec())
                .is_err()
        );
        let mut absent = parts;
        absent[2].part.definition_id = definition(3).definition_id;
        assert!(PrintLayoutOverview::assemble(header, definitions, absent).is_err());

        let page = GeometryResponse::LayoutOverviewPage {
            definitions: vec![],
            parts: vec![part(1, 1, &definition(1)), part(0, 2, &definition(1))],
            next_offset: None,
        };
        assert!(rpc::GeometryReply::try_from(page).is_err());
        let command = GeometryCommand::GetLayoutOverviewPage {
            session_id: SessionId::new(),
            scene_revision: SceneRevision(2),
            project_revision: ProjectRevision(3),
            kind: LayoutOverviewPageKind::Parts,
            offset: 64,
        };
        let GeometryCall::Execute(wire) = geometry_call(command.clone(), "").unwrap() else {
            panic!("short overview page")
        };
        assert_eq!(GeometryCommand::try_from(wire).unwrap(), command);
    }

    /// Each reply is one control frame; the project as a whole is not.
    #[test]
    fn full_overview_pages_fit_the_control_envelope_and_projects_exceed_former_caps() {
        let fits = |reply: GeometryResponse| {
            let wire = rpc::GeometryReply::try_from(reply).unwrap();
            assert!(
                wire.encoded_len() + 1024 < crate::MAX_CONTROL_BYTES as usize,
                "overview page is {} bytes",
                wire.encoded_len()
            );
            GeometryResponse::try_from(wire).unwrap();
        };
        let definitions: Vec<_> = (0..100).map(definition).collect();
        let parts: Vec<_> = (0..MAX_PRINT_LAYOUTS as u32)
            .flat_map(|layout| (0..200).map(move |i| (layout, layout * 200 + i)))
            .map(|(layout, n)| part(layout, n, &definitions[n as usize % definitions.len()]))
            .collect();
        let header = summary(&[200; MAX_PRINT_LAYOUTS], definitions.len() as u32);
        fits(GeometryResponse::LayoutOverview {
            summary: header.clone(),
        });
        fits(GeometryResponse::LayoutOverviewPage {
            definitions: definitions[..SCENE_PAGE_SIZE as usize].to_vec(),
            parts: vec![],
            next_offset: Some(SCENE_PAGE_SIZE),
        });
        fits(GeometryResponse::LayoutOverviewPage {
            definitions: vec![],
            parts: parts[..SCENE_PAGE_SIZE as usize].to_vec(),
            next_offset: Some(SCENE_PAGE_SIZE),
        });
        let overview = PrintLayoutOverview::assemble(header, definitions, parts).unwrap();
        assert_eq!(overview.definitions.len(), 100);
        assert_eq!(
            overview
                .layouts
                .iter()
                .map(|l| l.parts.len())
                .sum::<usize>(),
            200 * MAX_PRINT_LAYOUTS
        );
    }
}
