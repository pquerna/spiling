// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Ordered source-face finishes; native geometry admission belongs to the engine.
use crate::{
    geometry::{DefinitionId, FaceId, OccurrenceId, OccurrenceRecord},
    project::{ProjectError, ProjectErrorCode, StoredDefinition},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use ts_rs::TS;

pub const SURFACE_SCHEMA_VERSION: u32 = 1;
pub const MAX_SURFACE_LAYERS: usize = 64;
pub const MAX_SURFACE_DOCUMENT_BYTES: usize = 32 * 1024;
pub const MAX_SURFACE_RETAINED_BYTES: usize = 16 * 1024 * 1024;
fn invalid(message: &str) -> ProjectError {
    ProjectError::new(ProjectErrorCode::InvalidProject, message)
}
fn resource(message: &str) -> ProjectError {
    ProjectError::new(ProjectErrorCode::ResourceLimit, message)
}

#[derive(Debug, Clone, Copy, Serialize, TS, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
#[ts(type = "number")]
pub struct SurfaceLayerId(u32);
impl SurfaceLayerId {
    pub fn new(value: u32) -> Result<Self, ProjectError> {
        if value == 0 {
            return Err(invalid("surface layer ID must be nonzero"));
        }
        Ok(Self(value))
    }
    pub fn get(self) -> u32 {
        self.0
    }
}
impl<'de> Deserialize<'de> for SurfaceLayerId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Self::new(u32::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceScopeKind {
    Definition,
    Occurrence,
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SurfaceScope {
    Definition {
        definition_id: DefinitionId,
    },
    Occurrence {
        occurrence_id: OccurrenceId,
        expected_definition_id: DefinitionId,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SurfaceFinish {
    Smooth,
    VerticalRibs {
        depth_mm: f64,
        pitch_mm: f64,
        edge_margin_mm: f64,
        phase_mm: f64,
    },
}
impl SurfaceFinish {
    pub fn validate(&self) -> Result<(), ProjectError> {
        if let Self::VerticalRibs {
            depth_mm,
            pitch_mm,
            edge_margin_mm,
            phase_mm,
        } = self
        {
            if [*depth_mm, *pitch_mm, *edge_margin_mm]
                .iter()
                .any(|v| !v.is_finite() || *v <= 0.0)
                || !phase_mm.is_finite()
                || *phase_mm < 0.0
                || *phase_mm >= *pitch_mm
            {
                return Err(invalid(
                    "vertical ribs require finite positive depth, pitch and edge margin, and phase in [0,pitch)",
                ));
            }
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SurfaceLayer {
    pub layer_id: SurfaceLayerId,
    pub enabled: bool,
    pub scope: SurfaceScope,
    pub face_id: FaceId,
    pub finish: SurfaceFinish,
}
#[derive(Debug, Clone, Serialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SurfaceOverlays {
    pub schema_version: u32,
    pub layers: Vec<SurfaceLayer>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSurfaceOverlays {
    schema_version: u32,
    #[serde(deserialize_with = "bounded_layers")]
    layers: Vec<SurfaceLayer>,
}
fn bounded_layers<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<SurfaceLayer>, D::Error> {
    struct Layers;
    impl<'de> serde::de::Visitor<'de> for Layers {
        type Value = Vec<SurfaceLayer>;
        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("at most 64 surface layers")
        }
        fn visit_seq<A: serde::de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> Result<Self::Value, A::Error> {
            let mut layers = Vec::new();
            while let Some(layer) = seq.next_element()? {
                if layers.len() == MAX_SURFACE_LAYERS {
                    return Err(serde::de::Error::custom("surface layer budget exceeded"));
                }
                layers.push(layer);
            }
            Ok(layers)
        }
    }
    d.deserialize_seq(Layers)
}
impl<'de> Deserialize<'de> for SurfaceOverlays {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = RawSurfaceOverlays::deserialize(d)?;
        let doc = Self {
            schema_version: raw.schema_version,
            layers: raw.layers,
        };
        doc.validate().map_err(serde::de::Error::custom)?;
        Ok(doc)
    }
}
impl Default for SurfaceOverlays {
    fn default() -> Self {
        Self::empty()
    }
}
impl SurfaceOverlays {
    pub fn empty() -> Self {
        Self {
            schema_version: SURFACE_SCHEMA_VERSION,
            layers: Vec::new(),
        }
    }
    pub fn validate(&self) -> Result<(), ProjectError> {
        if self.schema_version != SURFACE_SCHEMA_VERSION {
            return Err(invalid("unsupported surface schema"));
        }
        if self.layers.len() > MAX_SURFACE_LAYERS || self.layers.capacity() > MAX_SURFACE_LAYERS {
            return Err(resource("surface layer allocation budget exceeded"));
        }
        let mut ids = BTreeSet::new();
        for layer in &self.layers {
            if !ids.insert(layer.layer_id) {
                return Err(invalid("duplicate surface layer ID"));
            }
            layer.finish.validate()?;
        }
        struct ByteCount(usize);
        impl std::io::Write for ByteCount {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0 = self.0.saturating_add(bytes.len());
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut count = ByteCount(0);
        serde_json::to_writer(&mut count, self)
            .map_err(|_| invalid("surface document serialization failed"))?;
        if count.0 > MAX_SURFACE_DOCUMENT_BYTES
            || self.allocated_bytes() > MAX_SURFACE_DOCUMENT_BYTES
        {
            return Err(resource("surface document byte budget exceeded"));
        }
        Ok(())
    }
    pub fn validate_targets(
        &self,
        definitions: &[StoredDefinition],
        occurrences: &[OccurrenceRecord],
        next_surface_layer: u32,
    ) -> Result<(), ProjectError> {
        self.validate()?;
        if next_surface_layer == 0 {
            return Err(invalid("invalid next surface layer counter"));
        }
        for layer in &self.layers {
            if layer.layer_id.get() >= next_surface_layer {
                return Err(invalid("surface layer ID exceeds allocator high-water"));
            }
            let valid = match &layer.scope {
                SurfaceScope::Definition { definition_id } => definitions
                    .iter()
                    .any(|d| &d.definition_id == definition_id),
                SurfaceScope::Occurrence {
                    occurrence_id,
                    expected_definition_id,
                } => occurrences.iter().any(|o| {
                    o.occurrence_id == *occurrence_id && &o.definition_id == expected_definition_id
                }),
            };
            if !valid {
                return Err(invalid(
                    "surface scope references unknown or inconsistent source identity",
                ));
            }
        }
        Ok(())
    }
    pub fn effective_finish(
        &self,
        definition_id: &DefinitionId,
        occurrence_id: OccurrenceId,
        face_id: &FaceId,
    ) -> Option<&SurfaceFinish> {
        let mut result = None;
        for occurrence_scope in [false, true] {
            for layer in &self.layers {
                let matches = match &layer.scope {
                    SurfaceScope::Definition { definition_id: id } => {
                        !occurrence_scope && id == definition_id
                    }
                    SurfaceScope::Occurrence {
                        occurrence_id: id,
                        expected_definition_id,
                    } => {
                        occurrence_scope
                            && *id == occurrence_id
                            && expected_definition_id == definition_id
                    }
                };
                if layer.enabled && &layer.face_id == face_id && matches {
                    result = Some(&layer.finish);
                }
            }
        }
        result
    }
    /// Retained allocation charge, including actual string and layer-vector capacity.
    pub fn allocated_bytes(&self) -> usize {
        let strings = self
            .layers
            .iter()
            .map(|layer| {
                layer.face_id.allocated_bytes()
                    + match &layer.scope {
                        SurfaceScope::Definition { definition_id } => {
                            definition_id.allocated_bytes()
                        }
                        SurfaceScope::Occurrence {
                            expected_definition_id,
                            ..
                        } => expected_definition_id.allocated_bytes(),
                    }
            })
            .sum::<usize>();
        std::mem::size_of::<Self>()
            + self.layers.capacity() * std::mem::size_of::<SurfaceLayer>()
            + strings
    }
}

#[cfg(test)]
mod tests;
