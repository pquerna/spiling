// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::required;
use crate::{ArtifactView, appearance::*, geometry::SourceHash, optical::*, rpc};
use prost::Message;
use std::sync::Arc;

fn observation(value: rpc::OpticalObservation) -> Result<OpticalObservation, String> {
    let illuminant = match rpc::AppearanceIlluminant::try_from(value.illuminant)
        .map_err(|_| "unknown optical illuminant")?
    {
        rpc::AppearanceIlluminant::D65 => AppearanceIlluminant::D65,
        rpc::AppearanceIlluminant::D50 => AppearanceIlluminant::D50,
        rpc::AppearanceIlluminant::Unknown => AppearanceIlluminant::Unknown,
        rpc::AppearanceIlluminant::Unspecified => return Err("missing optical illuminant".into()),
    };
    let observer = match rpc::AppearanceObserver::try_from(value.observer)
        .map_err(|_| "unknown optical observer")?
    {
        rpc::AppearanceObserver::Cie1931TwoDegree => AppearanceObserver::Cie1931TwoDegree,
        rpc::AppearanceObserver::Cie1964TenDegree => AppearanceObserver::Cie1964TenDegree,
        rpc::AppearanceObserver::Unknown => AppearanceObserver::Unknown,
        rpc::AppearanceObserver::Unspecified => return Err("missing optical observer".into()),
    };
    let view = match rpc::OpticalView::try_from(value.view).map_err(|_| "unknown optical view")? {
        rpc::OpticalView::NormalToPatch => OpticalView::NormalToPatch,
        rpc::OpticalView::Unspecified => return Err("missing optical view".into()),
    };
    Ok(OpticalObservation {
        illuminant,
        observer,
        view,
    })
}

fn observation_wire(value: &OpticalObservation) -> rpc::OpticalObservation {
    rpc::OpticalObservation {
        illuminant: match value.illuminant {
            AppearanceIlluminant::D65 => rpc::AppearanceIlluminant::D65,
            AppearanceIlluminant::D50 => rpc::AppearanceIlluminant::D50,
            AppearanceIlluminant::Unknown => rpc::AppearanceIlluminant::Unknown,
        } as i32,
        observer: match value.observer {
            AppearanceObserver::Cie1931TwoDegree => rpc::AppearanceObserver::Cie1931TwoDegree,
            AppearanceObserver::Cie1964TenDegree => rpc::AppearanceObserver::Cie1964TenDegree,
            AppearanceObserver::Unknown => rpc::AppearanceObserver::Unknown,
        } as i32,
        view: match value.view {
            OpticalView::NormalToPatch => rpc::OpticalView::NormalToPatch,
        } as i32,
    }
}

impl TryFrom<rpc::OpticalRequest> for OpticalRequest {
    type Error = String;
    fn try_from(value: rpc::OpticalRequest) -> Result<Self, String> {
        if value.encoded_len() > MAX_OPTICAL_REQUEST_BYTES {
            return Err("optical protobuf request exceeds its byte bound".into());
        }
        let model =
            match rpc::OpticalModel::try_from(value.model).map_err(|_| "unknown optical model")? {
                rpc::OpticalModel::PrusaV7Reference => OpticalModel::PrusaV7_007bb922,
                rpc::OpticalModel::Unspecified => return Err("missing optical model".into()),
            };
        let target = required(value.target, "optical target")?;
        let patch = required(value.patch, "optical patch")?;
        let origin = required(patch.origin_mm, "optical patch origin")?;
        let recipe = required(value.recipe, "optical recipe")?;
        let search = required(value.search, "optical search")?;
        let request = Self {
            model,
            materials: value.materials,
            target: OpticalTarget {
                color: required(target.color, "optical target color")?.try_into()?,
                delta_e00_tolerance: target.delta_e00_tolerance,
                lightness_tolerance: target.lightness_tolerance,
            },
            observation: observation(required(value.observation, "optical observation")?)?,
            patch: OpticalPatch {
                origin_mm: [origin.x, origin.y, origin.z],
                normal_xy: [patch.normal_x, patch.normal_y],
                width_mm: patch.width_mm,
                height_mm: patch.height_mm,
            },
            recipe: LayerCycle {
                slots: recipe
                    .slots
                    .into_iter()
                    .map(|slot| {
                        Ok(OpticalSlot {
                            material: u8::try_from(slot.material)
                                .map_err(|_| "optical material index overflow")?,
                            height_mm: slot.height_mm,
                        })
                    })
                    .collect::<Result<_, String>>()?,
                phase_slot: u16::try_from(recipe.phase_slot)
                    .map_err(|_| "optical phase overflow")?,
                origin_z_mm: recipe.origin_z_mm,
            },
            search: OpticalSearchSpace {
                patterns: search
                    .patterns
                    .into_iter()
                    .map(|pattern| {
                        Ok(OpticalPattern {
                            materials: pattern
                                .materials
                                .into_iter()
                                .map(|index| {
                                    u8::try_from(index)
                                        .map_err(|_| "optical pattern index overflow".to_owned())
                                })
                                .collect::<Result<_, _>>()?,
                        })
                    })
                    .collect::<Result<_, String>>()?,
                slot_heights_mm: search.slot_heights_mm,
                phase_slots: search
                    .phase_slots
                    .into_iter()
                    .map(|phase| {
                        u16::try_from(phase).map_err(|_| "optical search phase overflow".to_owned())
                    })
                    .collect::<Result<_, _>>()?,
                origin_z_mm: search.origin_z_mm,
            },
            limits: required(value.limits, "optical search limits")?.try_into()?,
        };
        request.validate().map_err(|error| error.to_string())?;
        Ok(request)
    }
}

impl TryFrom<&OpticalRequest> for rpc::OpticalRequest {
    type Error = String;
    fn try_from(value: &OpticalRequest) -> Result<Self, String> {
        value.validate().map_err(|error| error.to_string())?;
        let request = Self {
            model: match value.model {
                OpticalModel::PrusaV7_007bb922 => rpc::OpticalModel::PrusaV7Reference,
            } as i32,
            materials: value.materials.clone(),
            target: Some(rpc::OpticalTarget {
                color: Some(value.target.color.try_into()?),
                delta_e00_tolerance: value.target.delta_e00_tolerance,
                lightness_tolerance: value.target.lightness_tolerance,
            }),
            observation: Some(observation_wire(&value.observation)),
            patch: Some(rpc::OpticalPatch {
                origin_mm: Some(rpc::Vec3 {
                    x: value.patch.origin_mm[0],
                    y: value.patch.origin_mm[1],
                    z: value.patch.origin_mm[2],
                }),
                normal_x: value.patch.normal_xy[0],
                normal_y: value.patch.normal_xy[1],
                width_mm: value.patch.width_mm,
                height_mm: value.patch.height_mm,
            }),
            recipe: Some(rpc::OpticalLayerCycle {
                slots: value
                    .recipe
                    .slots
                    .iter()
                    .map(|slot| rpc::OpticalSlot {
                        material: u32::from(slot.material),
                        height_mm: slot.height_mm,
                    })
                    .collect(),
                phase_slot: u32::from(value.recipe.phase_slot),
                origin_z_mm: value.recipe.origin_z_mm,
            }),
            search: Some(rpc::OpticalSearchSpace {
                patterns: value
                    .search
                    .patterns
                    .iter()
                    .map(|pattern| rpc::OpticalPattern {
                        materials: pattern
                            .materials
                            .iter()
                            .map(|&index| u32::from(index))
                            .collect(),
                    })
                    .collect(),
                slot_heights_mm: value.search.slot_heights_mm.clone(),
                phase_slots: value
                    .search
                    .phase_slots
                    .iter()
                    .map(|&phase| u32::from(phase))
                    .collect(),
                origin_z_mm: value.search.origin_z_mm,
            }),
            limits: Some(value.limits.try_into()?),
        };
        if request.encoded_len() > MAX_OPTICAL_REQUEST_BYTES {
            return Err("optical protobuf request exceeds its byte bound".into());
        }
        Ok(request)
    }
}

impl TryFrom<OpticalRequest> for rpc::OpticalRequest {
    type Error = String;
    fn try_from(value: OpticalRequest) -> Result<Self, String> {
        value.validate().map_err(|error| error.to_string())?;
        let request = Self {
            model: match value.model {
                OpticalModel::PrusaV7_007bb922 => rpc::OpticalModel::PrusaV7Reference,
            } as i32,
            materials: value.materials,
            target: Some(rpc::OpticalTarget {
                color: Some(value.target.color.try_into()?),
                delta_e00_tolerance: value.target.delta_e00_tolerance,
                lightness_tolerance: value.target.lightness_tolerance,
            }),
            observation: Some(observation_wire(&value.observation)),
            patch: Some(rpc::OpticalPatch {
                origin_mm: Some(rpc::Vec3 {
                    x: value.patch.origin_mm[0],
                    y: value.patch.origin_mm[1],
                    z: value.patch.origin_mm[2],
                }),
                normal_x: value.patch.normal_xy[0],
                normal_y: value.patch.normal_xy[1],
                width_mm: value.patch.width_mm,
                height_mm: value.patch.height_mm,
            }),
            recipe: Some(rpc::OpticalLayerCycle {
                slots: value
                    .recipe
                    .slots
                    .into_iter()
                    .map(|slot| rpc::OpticalSlot {
                        material: u32::from(slot.material),
                        height_mm: slot.height_mm,
                    })
                    .collect(),
                phase_slot: u32::from(value.recipe.phase_slot),
                origin_z_mm: value.recipe.origin_z_mm,
            }),
            search: Some(rpc::OpticalSearchSpace {
                patterns: value
                    .search
                    .patterns
                    .into_iter()
                    .map(|pattern| rpc::OpticalPattern {
                        materials: pattern.materials.into_iter().map(u32::from).collect(),
                    })
                    .collect(),
                slot_heights_mm: value.search.slot_heights_mm,
                phase_slots: value
                    .search
                    .phase_slots
                    .into_iter()
                    .map(u32::from)
                    .collect(),
                origin_z_mm: value.search.origin_z_mm,
            }),
            limits: Some(value.limits.try_into()?),
        };
        if request.encoded_len() > MAX_OPTICAL_REQUEST_BYTES {
            return Err("optical protobuf request exceeds its byte bound".into());
        }
        Ok(request)
    }
}

impl TryFrom<rpc::OpticalSelection> for OpticalSelection {
    type Error = String;
    fn try_from(value: rpc::OpticalSelection) -> Result<Self, String> {
        use rpc::optical_selection::Kind;
        match required(value.kind, "optical selection kind")? {
            Kind::Requested(_) => Ok(Self::Requested),
            Kind::CandidateIndex(index) if (index as usize) < MAX_OPTICAL_CANDIDATES => {
                Ok(Self::Candidate { index })
            }
            Kind::CandidateIndex(_) => Err("optical selection index exceeds grid bound".into()),
        }
    }
}
impl TryFrom<OpticalSelection> for rpc::OpticalSelection {
    type Error = String;
    fn try_from(value: OpticalSelection) -> Result<Self, String> {
        use rpc::optical_selection::Kind;
        Ok(Self {
            kind: Some(match value {
                OpticalSelection::Requested => Kind::Requested(rpc::OpticalRequestedSelection {}),
                OpticalSelection::Candidate { index }
                    if (index as usize) < MAX_OPTICAL_CANDIDATES =>
                {
                    Kind::CandidateIndex(index)
                }
                OpticalSelection::Candidate { .. } => {
                    return Err("optical selection index exceeds grid bound".into());
                }
            }),
        })
    }
}
impl TryFrom<rpc::OpticalReportRecord> for OpticalReportRecord {
    type Error = String;
    fn try_from(value: rpc::OpticalReportRecord) -> Result<Self, String> {
        let record = Self {
            hash: SourceHash::parse(value.hash).map_err(|error| error.to_string())?,
            byte_count: value.byte_count,
            input_hash: SourceHash::parse(value.input_hash).map_err(|error| error.to_string())?,
            producer: value.producer,
            complete: value.complete,
        };
        record.validate().map_err(|error| error.to_string())?;
        Ok(record)
    }
}
impl TryFrom<OpticalReportRecord> for rpc::OpticalReportRecord {
    type Error = String;
    fn try_from(value: OpticalReportRecord) -> Result<Self, String> {
        value.validate().map_err(|error| error.to_string())?;
        Ok(Self {
            hash: value.hash.into(),
            byte_count: value.byte_count,
            input_hash: value.input_hash.into(),
            producer: value.producer,
            complete: value.complete,
        })
    }
}
impl TryFrom<rpc::OpticalStudyInfo> for OpticalStudyInfo {
    type Error = String;
    fn try_from(value: rpc::OpticalStudyInfo) -> Result<Self, String> {
        Ok(Self {
            request: Arc::new(required(value.request, "optical study request")?.try_into()?),
            report: value.report.map(TryInto::try_into).transpose()?,
            selection: value.selection.map(TryInto::try_into).transpose()?,
        })
    }
}
impl TryFrom<OpticalStudyInfo> for rpc::OpticalStudyInfo {
    type Error = String;
    fn try_from(value: OpticalStudyInfo) -> Result<Self, String> {
        Ok(Self {
            request: Some(value.request.as_ref().try_into()?),
            report: value.report.map(TryInto::try_into).transpose()?,
            selection: value.selection.map(TryInto::try_into).transpose()?,
        })
    }
}

pub(super) fn validate_report_resource(
    record: &OpticalReportRecord,
    resource: &ArtifactView,
) -> Result<(), String> {
    record.validate().map_err(|error| error.to_string())?;
    let size = super::validate_artifact_view(resource, MAX_APPEARANCE_REPORT_BYTES as u64)?;
    if size != record.byte_count
        || resource.sha256 != record.hash.as_str()
        || resource.name.strip_prefix("artifacts/") != Some(record.hash.as_str())
        || resource.media_type != OPTICAL_REPORT_MIME
    {
        return Err("optical report descriptor disagrees with checked record".into());
    }
    Ok(())
}
