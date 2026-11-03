// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0
use super::{request_id, required, session_parent};
use crate::{
    geometry::*,
    project::{ProjectId, ProjectRevision},
    rpc,
};
fn checked<T>(r: Result<T, GeometryError>) -> Result<T, String> {
    r.map_err(|e| e.to_string())
}
impl TryFrom<rpc::LayoutRequest> for LayoutRequest {
    type Error = String;
    fn try_from(v: rpc::LayoutRequest) -> Result<Self, String> {
        if v.pinned.len() > MAX_OCCURRENCES as usize {
            return Err("layout pin count exceeds limit".into());
        }
        let r = Self {
            objective: match rpc::LayoutObjective::try_from(v.objective) {
                Ok(rpc::LayoutObjective::LayoutArrange) => LayoutObjective::Arrange,
                Ok(rpc::LayoutObjective::LayoutCopies) => LayoutObjective::Copies,
                Ok(rpc::LayoutObjective::LayoutFill) => LayoutObjective::Fill,
                _ => return Err("invalid layout objective".into()),
            },
            style: match rpc::LayoutStyle::try_from(v.style) {
                Ok(rpc::LayoutStyle::LayoutBalanced) => LayoutStyle::Balanced,
                Ok(rpc::LayoutStyle::LayoutDense) => LayoutStyle::Dense,
                _ => return Err("invalid layout style".into()),
            },
            template: v
                .template
                .map(OccurrenceId::new)
                .transpose()
                .map_err(|e| e.to_string())?,
            quantity: v.quantity,
            use_additional_plates: v.use_additional_plates,
            pinned: v
                .pinned
                .into_iter()
                .map(OccurrenceId::new)
                .collect::<Result<_, _>>()
                .map_err(|e| e.to_string())?,
            spacing_mm: v.spacing_mm,
        };
        checked(r.validate())?;
        Ok(r)
    }
}
impl TryFrom<LayoutRequest> for rpc::LayoutRequest {
    type Error = String;
    fn try_from(v: LayoutRequest) -> Result<Self, String> {
        checked(v.validate())?;
        Ok(Self {
            objective: match v.objective {
                LayoutObjective::Arrange => rpc::LayoutObjective::LayoutArrange,
                LayoutObjective::Copies => rpc::LayoutObjective::LayoutCopies,
                LayoutObjective::Fill => rpc::LayoutObjective::LayoutFill,
            } as i32,
            style: match v.style {
                LayoutStyle::Balanced => rpc::LayoutStyle::LayoutBalanced,
                LayoutStyle::Dense => rpc::LayoutStyle::LayoutDense,
            } as i32,
            template: v.template.map(OccurrenceId::get),
            quantity: v.quantity,
            use_additional_plates: v.use_additional_plates,
            pinned: v.pinned.into_iter().map(OccurrenceId::get).collect(),
            spacing_mm: v.spacing_mm,
        })
    }
}
impl TryFrom<rpc::LayoutPlacement> for LayoutPlacement {
    type Error = String;
    fn try_from(v: rpc::LayoutPlacement) -> Result<Self, String> {
        Ok(Self {
            source: checked(OccurrenceId::new(v.source))?,
            copy: v.copy,
            pose: required(v.pose, "layout pose")?.try_into()?,
        })
    }
}
impl TryFrom<LayoutPlacement> for rpc::LayoutPlacement {
    type Error = String;
    fn try_from(v: LayoutPlacement) -> Result<Self, String> {
        checked(v.pose.validate())?;
        Ok(Self {
            source: v.source.get(),
            copy: v.copy,
            pose: Some(v.pose.try_into()?),
        })
    }
}
impl TryFrom<rpc::PrepareLayoutRequest> for GeometryCommand {
    type Error = String;
    fn try_from(v: rpc::PrepareLayoutRequest) -> Result<Self, String> {
        request_id(&v.request_id)?;
        Ok(Self::PrepareLayout {
            session_id: session_parent(&v.parent)?,
            base_revision: SceneRevision(v.base_revision),
            project_revision: ProjectRevision(v.project_revision),
            request: required(v.request, "layout request")?.try_into()?,
        })
    }
}
impl TryFrom<rpc::ApplyLayoutRequest> for GeometryCommand {
    type Error = String;
    fn try_from(v: rpc::ApplyLayoutRequest) -> Result<Self, String> {
        Ok(Self::ApplyLayout {
            session_id: checked(SessionId::parse(v.session_id))?,
            base_revision: SceneRevision(v.base_revision),
            project_revision: ProjectRevision(v.project_revision),
            proposal_id: checked(SourceHash::parse(v.proposal_id))?,
        })
    }
}
impl TryFrom<rpc::LayoutProposal> for LayoutProposal {
    type Error = String;
    fn try_from(v: rpc::LayoutProposal) -> Result<Self, String> {
        if v.placements.len() > MAX_OCCURRENCES as usize
            || v.footprints_mm.len() > MAX_OCCURRENCES as usize
            || v.additional_plates.len() >= crate::project::MAX_PRINT_LAYOUTS
        {
            return Err("layout proposal exceeds occurrence limit".into());
        }
        let r = Self {
            id: checked(SourceHash::parse(v.id))?,
            session_id: checked(SessionId::parse(v.session_id))?,
            project_id: ProjectId::parse(v.project_id).map_err(|e| e.to_string())?,
            project_revision: ProjectRevision(v.project_revision),
            scene_revision: SceneRevision(v.scene_revision),
            request: required(v.request, "layout request")?.try_into()?,
            placements: v
                .placements
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            footprints_mm: v
                .footprints_mm
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            additional_plates: v
                .additional_plates
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            requested_count: v.requested_count,
            placed_count: v.placed_count,
            candidates_examined: v.candidates_examined,
            manufacturing_attempts: v.manufacturing_attempts,
            limited_by: match rpc::LayoutLimit::try_from(v.limited_by) {
                Ok(rpc::LayoutLimit::LayoutQuantityLimit) => LayoutLimit::RequestedQuantity,
                Ok(rpc::LayoutLimit::LayoutBuildAreaLimit) => LayoutLimit::BuildArea,
                Ok(rpc::LayoutLimit::LayoutSearchLimit) => LayoutLimit::SearchBudget,
                Ok(rpc::LayoutLimit::LayoutManufacturingLimit) => LayoutLimit::ManufacturingBudget,
                Ok(rpc::LayoutLimit::LayoutPlateLimit) => LayoutLimit::PlateBudget,
                Ok(rpc::LayoutLimit::LayoutOccurrenceLimit) => LayoutLimit::OccurrenceBudget,
                _ => return Err("invalid layout limit".into()),
            },
            verification: required(v.verification, "layout verification")?.try_into()?,
        };
        checked(r.validate())?;
        Ok(r)
    }
}
impl TryFrom<LayoutProposal> for rpc::LayoutProposal {
    type Error = String;
    fn try_from(v: LayoutProposal) -> Result<Self, String> {
        checked(v.validate())?;
        Ok(Self {
            id: v.id.into(),
            session_id: v.session_id.into(),
            project_id: v.project_id.as_str().into(),
            project_revision: v.project_revision.0,
            scene_revision: v.scene_revision.get(),
            request: Some(v.request.try_into()?),
            placements: v
                .placements
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            footprints_mm: v
                .footprints_mm
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            additional_plates: v
                .additional_plates
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            requested_count: v.requested_count,
            placed_count: v.placed_count,
            candidates_examined: v.candidates_examined,
            manufacturing_attempts: v.manufacturing_attempts,
            limited_by: match v.limited_by {
                LayoutLimit::RequestedQuantity => rpc::LayoutLimit::LayoutQuantityLimit,
                LayoutLimit::BuildArea => rpc::LayoutLimit::LayoutBuildAreaLimit,
                LayoutLimit::SearchBudget => rpc::LayoutLimit::LayoutSearchLimit,
                LayoutLimit::ManufacturingBudget => rpc::LayoutLimit::LayoutManufacturingLimit,
                LayoutLimit::PlateBudget => rpc::LayoutLimit::LayoutPlateLimit,
                LayoutLimit::OccurrenceBudget => rpc::LayoutLimit::LayoutOccurrenceLimit,
            } as i32,
            verification: Some(v.verification.try_into()?),
        })
    }
}

impl TryFrom<rpc::PrintLayoutInfo> for crate::project::PrintLayoutInfo {
    type Error = String;
    fn try_from(v: rpc::PrintLayoutInfo) -> Result<Self, String> {
        crate::project::validate_layout_name(&v.name).map_err(|e| e.to_string())?;
        if v.occurrence_count > MAX_OCCURRENCES {
            return Err("layout occurrence count exceeds limit".into());
        }
        Ok(Self {
            layout_id: crate::project::PrintLayoutId::parse(v.layout_id)
                .map_err(|e| e.to_string())?,
            name: v.name,
            occurrence_count: v.occurrence_count,
            configured: v.configured,
            has_result: v.has_result,
        })
    }
}
impl TryFrom<crate::project::PrintLayoutInfo> for rpc::PrintLayoutInfo {
    type Error = String;
    fn try_from(v: crate::project::PrintLayoutInfo) -> Result<Self, String> {
        crate::project::validate_layout_name(&v.name).map_err(|e| e.to_string())?;
        if v.occurrence_count > MAX_OCCURRENCES {
            return Err("layout occurrence count exceeds limit".into());
        }
        Ok(Self {
            layout_id: v.layout_id.as_str().into(),
            name: v.name,
            occurrence_count: v.occurrence_count,
            configured: v.configured,
            has_result: v.has_result,
        })
    }
}
impl TryFrom<rpc::PrintLayoutEdit> for crate::project::PrintLayoutEdit {
    type Error = String;
    fn try_from(v: rpc::PrintLayoutEdit) -> Result<Self, String> {
        use rpc::print_layout_edit::Edit;
        let id = |s| crate::project::PrintLayoutId::parse(s).map_err(|e| e.to_string());
        let edit = match required(v.edit, "print layout edit")? {
            Edit::Create(v) => Self::Create {
                name: v.name,
                copy_setup_from: v.copy_setup_from.map(id).transpose()?,
            },
            Edit::Duplicate(v) => Self::Duplicate {
                source: id(v.source)?,
                name: v.name,
            },
            Edit::Rename(v) => Self::Rename {
                layout_id: id(v.layout_id)?,
                name: v.name,
            },
            Edit::Remove(v) => Self::Remove {
                layout_id: id(v.layout_id)?,
            },
            Edit::Reorder(v) => {
                if v.layout_ids.len() > crate::project::MAX_PRINT_LAYOUTS {
                    return Err("plate order exceeds limit".into());
                }
                Self::Reorder {
                    layout_ids: v.layout_ids.into_iter().map(id).collect::<Result<_, _>>()?,
                }
            }
            Edit::Transfer(v) => {
                if v.occurrences.len() > MAX_OCCURRENCES as usize {
                    return Err("part transfer exceeds limit".into());
                }
                Self::Transfer {
                    source: id(v.source)?,
                    target: id(v.target)?,
                    occurrences: v
                        .occurrences
                        .into_iter()
                        .map(OccurrenceId::new)
                        .collect::<Result<_, _>>()
                        .map_err(|e| e.to_string())?,
                    copy: v.copy,
                }
            }
        };
        edit.validate().map_err(|e| e.to_string())?;
        Ok(edit)
    }
}
impl TryFrom<crate::project::PrintLayoutEdit> for rpc::PrintLayoutEdit {
    type Error = String;
    fn try_from(v: crate::project::PrintLayoutEdit) -> Result<Self, String> {
        use crate::project::PrintLayoutEdit as D;
        use rpc::print_layout_edit::Edit;
        v.validate().map_err(|e| e.to_string())?;
        Ok(Self {
            edit: Some(match v {
                D::Create {
                    name,
                    copy_setup_from,
                } => Edit::Create(rpc::CreatePrintLayout {
                    name,
                    copy_setup_from: copy_setup_from.map(|id| id.as_str().into()),
                }),
                D::Duplicate { source, name } => Edit::Duplicate(rpc::DuplicatePrintLayout {
                    source: source.as_str().into(),
                    name,
                }),
                D::Rename { layout_id, name } => Edit::Rename(rpc::RenamePrintLayout {
                    layout_id: layout_id.as_str().into(),
                    name,
                }),
                D::Remove { layout_id } => Edit::Remove(rpc::RemovePrintLayout {
                    layout_id: layout_id.as_str().into(),
                }),
                D::Reorder { layout_ids } => Edit::Reorder(rpc::ReorderPrintLayouts {
                    layout_ids: layout_ids
                        .into_iter()
                        .map(|id| id.as_str().into())
                        .collect(),
                }),
                D::Transfer {
                    source,
                    target,
                    occurrences,
                    copy,
                } => Edit::Transfer(rpc::TransferPrintParts {
                    source: source.as_str().into(),
                    target: target.as_str().into(),
                    occurrences: occurrences.into_iter().map(OccurrenceId::get).collect(),
                    copy,
                }),
            }),
        })
    }
}

impl TryFrom<rpc::AdditionalPlateProposal> for AdditionalPlateProposal {
    type Error = String;
    fn try_from(v: rpc::AdditionalPlateProposal) -> Result<Self, String> {
        if v.placements.len() > MAX_OCCURRENCES as usize
            || v.footprints_mm.len() > MAX_OCCURRENCES as usize
        {
            return Err("additional plate exceeds occurrence limit".into());
        }
        let r = Self {
            placements: v
                .placements
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            footprints_mm: v
                .footprints_mm
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            candidates_examined: v.candidates_examined,
            manufacturing_attempts: v.manufacturing_attempts,
            verification: required(v.verification, "additional plate verification")?.try_into()?,
        };
        checked(r.validate())?;
        Ok(r)
    }
}
impl TryFrom<AdditionalPlateProposal> for rpc::AdditionalPlateProposal {
    type Error = String;
    fn try_from(v: AdditionalPlateProposal) -> Result<Self, String> {
        checked(v.validate())?;
        Ok(Self {
            placements: v
                .placements
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            footprints_mm: v
                .footprints_mm
                .into_iter()
                .map(TryInto::try_into)
                .collect::<Result<_, _>>()?,
            candidates_examined: v.candidates_examined,
            manufacturing_attempts: v.manufacturing_attempts,
            verification: Some(v.verification.try_into()?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_layout_request_roundtrip_keeps_scope_pins_and_quantity_without_defaults() {
        for objective in [
            LayoutObjective::Arrange,
            LayoutObjective::Copies,
            LayoutObjective::Fill,
        ] {
            let request = LayoutRequest {
                objective,
                template: (objective != LayoutObjective::Arrange)
                    .then(|| OccurrenceId::new(3).unwrap()),
                quantity: 2,
                use_additional_plates: false,
                pinned: vec![OccurrenceId::new(7).unwrap()],
                spacing_mm: 3.5,
                style: LayoutStyle::Dense,
            };
            let wire: rpc::LayoutRequest = request.clone().try_into().unwrap();
            assert_eq!(request, LayoutRequest::try_from(wire.clone()).unwrap());
            assert!(
                LayoutRequest::try_from(rpc::LayoutRequest {
                    style: 0,
                    ..wire.clone()
                })
                .is_err()
            );
            assert!(
                LayoutRequest::try_from(rpc::LayoutRequest {
                    objective: 99,
                    ..wire.clone()
                })
                .is_err()
            );
            assert!(
                LayoutRequest::try_from(rpc::LayoutRequest {
                    quantity: 0,
                    use_additional_plates: false,
                    ..wire.clone()
                })
                .is_err()
            );
            assert!(
                LayoutRequest::try_from(rpc::LayoutRequest {
                    pinned: vec![7, 7],
                    ..wire
                })
                .is_err()
            );
        }
    }
}
