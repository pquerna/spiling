// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::*;

pub const MAX_ANALYSIS_FASTENERS: usize = 8;
pub const MAX_BORE_FACES: usize = 32;

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AnalysisBoreProfile {
    pub hole: AnalysisHole,
    pub outer_xy_mm: [[f64; 2]; 2],
    pub outer_faces: [FaceId; 4],
}
impl AnalysisBoreProfile {
    pub fn validate(&self) -> Result<(), ProjectError> {
        self.hole.validate()?;
        if self
            .outer_xy_mm
            .iter()
            .any(|range| range.iter().any(|value| !value.is_finite()) || range[0] >= range[1])
            || (0..2).any(|axis| {
                self.hole.center_mm[axis] - self.hole.radius_mm <= self.outer_xy_mm[axis][0]
                    || self.hole.center_mm[axis] + self.hole.radius_mm >= self.outer_xy_mm[axis][1]
            })
        {
            return Err(invalid(
                "bored plate needs positive surrounding native ligaments",
            ));
        }
        let mut faces = BTreeSet::new();
        if self
            .outer_faces
            .iter()
            .chain(&self.hole.cap_faces)
            .chain(&self.hole.bore_faces)
            .any(|face| !faces.insert(face))
        {
            return Err(invalid("bored plate boundary ownership must be disjoint"));
        }
        Ok(())
    }
    pub fn native_volume_mm3(&self) -> f64 {
        ((self.outer_xy_mm[0][1] - self.outer_xy_mm[0][0])
            * (self.outer_xy_mm[1][1] - self.outer_xy_mm[1][0])
            - std::f64::consts::PI * self.hole.radius_mm * self.hole.radius_mm)
            * self.hole.depth_mm
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[expect(
    clippy::large_enum_variant,
    reason = "One bounded inline geometry capture avoids a separate allocation in each result"
)]
pub enum AnalysisMeshGeometry {
    AffineHex27 {},
    RationalBoredPlate { profile: AnalysisBoreProfile },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AnalysisFastenerResponse {
    pub fastener_id: u32,
    pub area_mm2: f64,
    pub reaction_force_n: [f64; 3],
    pub reaction_moment_n_mm: [f64; 3],
    pub maximum_constraint_error_mm: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AnalysisHole {
    #[serde(deserialize_with = "bounded::<_, _, MAX_BORE_FACES>")]
    pub bore_faces: Vec<FaceId>,
    pub cap_faces: [FaceId; 2],
    pub center_mm: [f64; 3],
    pub radius_mm: f64,
    pub depth_mm: f64,
}
impl AnalysisHole {
    pub fn validate(&self) -> Result<(), ProjectError> {
        let mut faces = BTreeSet::new();
        if self.bore_faces.is_empty()
            || self.bore_faces.len() > MAX_BORE_FACES
            || self.center_mm.iter().any(|value| !value.is_finite())
            || !self.radius_mm.is_finite()
            || !(0.005..=500_000.0).contains(&self.radius_mm)
            || !self.depth_mm.is_finite()
            || !(0.01..=1_000_000.0).contains(&self.depth_mm)
            || self
                .bore_faces
                .iter()
                .chain(&self.cap_faces)
                .any(|face| !faces.insert(face))
        {
            return Err(invalid(
                "fastener needs a finite, complete, uniquely owned native through-hole",
            ));
        }
        Ok(())
    }
    pub fn midpoint_mm(&self) -> [f64; 3] {
        [
            self.center_mm[0],
            self.center_mm[1],
            self.center_mm[2] + 0.5 * self.depth_mm,
        ]
    }
    pub fn allocated_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.bore_faces.capacity() * std::mem::size_of::<FaceId>()
            + self
                .bore_faces
                .iter()
                .chain(&self.cap_faces)
                .map(FaceId::allocated_bytes)
                .sum::<usize>()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HoleEnd {
    Start,
    End,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FastenerAttachment {
    Clamp {
        end: HoleEnd,
        washer_diameter_mm: f64,
    },
    Pin {
        allow_axial_slide: bool,
        allow_spin: bool,
    },
    BondedEngagement {
        end: HoleEnd,
        engagement_mm: f64,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AnalysisFastener {
    pub id: u32,
    pub name: String,
    pub hole: AnalysisHole,
    pub attachment: FastenerAttachment,
}
impl AnalysisFastener {
    pub fn validate(&self) -> Result<(), ProjectError> {
        self.hole.validate()?;
        if self.id == 0
            || self.name.trim() != self.name
            || self.name.is_empty()
            || self.name.len() > 80
            || self.name.chars().any(char::is_control)
        {
            return Err(invalid(
                "fastener needs a nonzero case-local identity and a short name",
            ));
        }
        match self.attachment {
            FastenerAttachment::Clamp {
                washer_diameter_mm, ..
            } if !washer_diameter_mm.is_finite()
                || washer_diameter_mm <= 2.0 * self.hole.radius_mm + 1e-6
                || washer_diameter_mm > 1_000_000.0 =>
            {
                Err(invalid(
                    "washer diameter must exceed the hole diameter and have finite positive contact",
                ))
            }
            FastenerAttachment::BondedEngagement { engagement_mm, .. }
                if !engagement_mm.is_finite()
                    || engagement_mm < 0.01
                    || engagement_mm > self.hole.depth_mm =>
            {
                Err(invalid(
                    "engagement must be 0.01 mm or greater and fit the complete native bore depth",
                ))
            }
            _ => Ok(()),
        }
    }
    pub fn constrained_motions(&self) -> [bool; 6] {
        match self.attachment {
            FastenerAttachment::Pin {
                allow_axial_slide,
                allow_spin,
            } => [true, true, !allow_axial_slide, true, true, !allow_spin],
            _ => [true; 6],
        }
    }
    pub fn allocated_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.name.capacity() + self.hole.allocated_bytes()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AnalysisFastenerPreparation {
    pub project_id: ProjectId,
    pub project_revision: ProjectRevision,
    pub reference: FaceRef,
    pub source_hash: SourceHash,
    pub pose: RigidPoseMm,
    pub hole: AnalysisHole,
    pub outer_xy_mm: [[f64; 2]; 2],
}
impl AnalysisFastenerPreparation {
    pub fn validate(&self) -> Result<(), ProjectError> {
        self.hole.validate()?;
        self.pose
            .validate()
            .map_err(|error| invalid(&error.to_string()))?;
        if !self.hole.bore_faces.contains(&self.reference.face_id)
            || self
                .outer_xy_mm
                .iter()
                .any(|range| range.iter().any(|value| !value.is_finite()) || range[0] >= range[1])
            || (0..2).any(|axis| {
                self.hole.center_mm[axis] - self.hole.radius_mm <= self.outer_xy_mm[axis][0]
                    || self.hole.center_mm[axis] + self.hole.radius_mm >= self.outer_xy_mm[axis][1]
            })
        {
            return Err(invalid(
                "prepared mounting hole must have positive surrounding material and match the selected native bore",
            ));
        }
        Ok(())
    }
    pub fn washer_limit_mm(&self) -> f64 {
        2.0 * (0..2)
            .flat_map(|axis| {
                [
                    self.hole.center_mm[axis] - self.outer_xy_mm[axis][0],
                    self.outer_xy_mm[axis][1] - self.hole.center_mm[axis],
                ]
            })
            .fold(f64::INFINITY, f64::min)
    }
}
