// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::{required, *};
use crate::project::{MAX_PRINT_LAYOUTS, PrintLayoutId, ProjectId, ProjectRevision};
impl TryFrom<rpc::PrintLayoutOverview> for PrintLayoutOverview {
    type Error = String;
    fn try_from(v: rpc::PrintLayoutOverview) -> Result<Self, String> {
        if v.definitions.len() > MAX_DEFINITIONS as usize
            || v.layouts.len() > MAX_PRINT_LAYOUTS
            || v.layouts.iter().map(|l| l.parts.len()).sum::<usize>() > MAX_OCCURRENCES as usize
        {
            return Err("overview exceeds project limits".into());
        }
        let definitions: Vec<DefinitionRecord> = v
            .definitions
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<_, _>>()?;
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
                                    definition_id: definitions
                                        .get(p.definition_index as usize)
                                        .ok_or("overview definition index is out of bounds")?
                                        .definition_id
                                        .clone(),
                                    pose: required(p.pose, "overview pose")?.try_into()?,
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
            definitions,
        };
        value.validate().map_err(|e| e.to_string())?;
        Ok(value)
    }
}
impl TryFrom<PrintLayoutOverview> for rpc::PrintLayoutOverview {
    type Error = String;
    fn try_from(v: PrintLayoutOverview) -> Result<Self, String> {
        v.validate().map_err(|e| e.to_string())?;
        let indices: std::collections::BTreeMap<_, _> = v
            .definitions
            .iter()
            .enumerate()
            .map(|(i, d)| (d.definition_id.clone(), i as u32))
            .collect();
        Ok(Self {
            session_id: v.session_id.as_str().into(),
            scene_revision: v.scene_revision.get(),
            project_id: v.project_id.as_str().into(),
            project_revision: v.project_revision.get(),
            definitions: v
                .definitions
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
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
                                    definition_index: indices[&p.definition_id],
                                    pose: Some(p.pose.try_into()?),
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
        let definition = DefinitionRecord {
            definition_id: DefinitionId::from_source_sha256(&[1; 32]),
            provenance: SourceProvenance {
                source_hash: SourceHash::from_bytes(b"overview"),
                source_name: "Part.step".into(),
                source_unit: SourceUnit::Millimetre,
                uncertainty_mm: None,
                source_solid: None,
            },
            face_count: 1,
            bounds_mm: AabbMm::new([-1.0; 3], [1.0; 3]).unwrap(),
            display: DefinitionDisplay::Mesh {
                artifact_id: ArtifactId::new(1).unwrap(),
            },
        };
        let v = PrintLayoutOverview {
            session_id: SessionId::parse("550e8400-e29b-41d4-a716-446655440000").unwrap(),
            scene_revision: SceneRevision(2),
            project_id: ProjectId::new(),
            project_revision: ProjectRevision(3),
            definitions: vec![definition.clone()],
            layouts: vec![LayoutOverview {
                layout_id,
                build_area_mm: Some(AabbMm::new([0.0; 3], [200.0, 100.0, 0.0]).unwrap()),
                captured_plate: true,
                parts: vec![LayoutPartBounds {
                    occurrence_id: OccurrenceId::new(1).unwrap(),
                    definition_id: definition.definition_id,
                    pose: RigidPoseMm::IDENTITY,
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
        let mut bad = wire.clone();
        bad.layouts[0].parts[0].definition_index = 1;
        assert!(PrintLayoutOverview::try_from(bad).is_err());
        let mut bad = wire.clone();
        bad.layouts[0].parts[0].pose = None;
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
    #[test]
    fn full_plate_overview_fits_the_existing_control_envelope() {
        use prost::Message;
        let definitions: Vec<_> = (1..=MAX_DEFINITIONS as u8)
            .map(|n| DefinitionRecord {
                definition_id: DefinitionId::from_source_sha256(&[n; 32]),
                provenance: SourceProvenance {
                    source_hash: SourceHash::parse(format!("{n:02x}").repeat(32)).unwrap(),
                    source_name: "x".repeat(MAX_DISPLAY_LABEL_BYTES as usize),
                    source_unit: SourceUnit::Millimetre,
                    uncertainty_mm: Some(0.001),
                    source_solid: None,
                },
                face_count: MAX_NATIVE_FACES,
                bounds_mm: AabbMm::new([1.0, 2.0, 3.0], [100.0, 200.0, 300.0]).unwrap(),
                display: DefinitionDisplay::Mesh {
                    artifact_id: ArtifactId::new(u32::MAX - u32::from(n)).unwrap(),
                },
            })
            .collect();
        let layouts = (0..MAX_PRINT_LAYOUTS)
            .map(|n| LayoutOverview {
                layout_id: PrintLayoutId::new(),
                captured_plate: true,
                build_area_mm: Some(AabbMm::new([1.0, 2.0, 3.0], [220.0, 220.0, 3.0]).unwrap()),
                parts: (0..16)
                    .map(|i| LayoutPartBounds {
                        occurrence_id: OccurrenceId::new(u32::MAX - (n * 16 + i) as u32).unwrap(),
                        definition_id: definitions[(n * 16 + i) % definitions.len()]
                            .definition_id
                            .clone(),
                        pose: RigidPoseMm {
                            translation_mm: [1.0, 2.0, 3.0],
                            rotation_xyzw: [0.5; 4],
                        },
                        source_bounds_mm: AabbMm::new([1.0, 2.0, 3.0], [100.0, 200.0, 300.0])
                            .unwrap(),
                    })
                    .collect(),
            })
            .collect();
        let overview = PrintLayoutOverview {
            session_id: SessionId::new(),
            scene_revision: SceneRevision(u32::MAX),
            project_id: ProjectId::new(),
            project_revision: ProjectRevision(u32::MAX),
            definitions,
            layouts,
        };
        let wire: rpc::PrintLayoutOverview = overview.try_into().unwrap();
        assert!(
            wire.encoded_len() + 1024 < crate::MAX_CONTROL_BYTES as usize,
            "overview is {} bytes",
            wire.encoded_len()
        );
        PrintLayoutOverview::try_from(wire).unwrap();
    }
}
