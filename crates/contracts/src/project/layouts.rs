// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! One independently configured print job. Source definitions remain project-owned.
use super::*;
pub const MAX_PRINT_LAYOUTS: usize = 16;
#[derive(Debug, Clone, Serialize, TS, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(transparent)]
#[ts(type = "string")]
pub struct PrintLayoutId(String);
impl PrintLayoutId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().hyphenated().to_string())
    }
    pub fn parse(value: impl Into<String>) -> Result<Self, ProjectError> {
        let value = value.into();
        let uuid =
            uuid::Uuid::parse_str(&value).map_err(|_| invalid("invalid print layout UUID"))?;
        let mut canonical = [0; 36];
        if uuid.is_nil()
            || uuid.hyphenated().encode_lower(&mut canonical).as_bytes() != value.as_bytes()
        {
            return Err(invalid("print layout UUID must be canonical and nonnil"));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub(crate) fn allocated_bytes(&self) -> usize {
        self.0.capacity()
    }
}
impl Default for PrintLayoutId {
    fn default() -> Self {
        Self::new()
    }
}
impl<'de> Deserialize<'de> for PrintLayoutId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PrintLayoutRecord {
    pub layout_id: PrintLayoutId,
    pub name: String,
    pub occurrences: Vec<OccurrenceRecord>,
    #[serde(deserialize_with = "required_option")]
    pub hardware: Option<Arc<crate::hardware::ProjectHardwareCapture>>,
    #[serde(deserialize_with = "required_option")]
    pub manufacturing_intent: Option<Arc<ManufacturingIntent>>,
    pub manufacturing_options: Vec<ManufacturingOption>,
    #[serde(deserialize_with = "required_option")]
    pub manufacturing_artifact: Option<ManufacturingArtifactRecord>,
}
impl PrintLayoutRecord {
    pub fn validate(
        &self,
        project: &ProjectId,
        definitions: &[StoredDefinition],
        surfaces: &SurfaceOverlays,
        process: &ProcessDocument,
    ) -> Result<(), ProjectError> {
        validate_layout_name(&self.name)?;
        if let Some(h) = &self.hardware {
            h.validate().map_err(|e| invalid(&e))?;
        }
        if self
            .manufacturing_intent
            .as_ref()
            .is_some_and(|m| m.hardware != self.hardware)
        {
            return Err(invalid(
                "layout hardware differs from its manufacturing intent",
            ));
        }
        validate_manufacturing_options(&self.manufacturing_options)?;
        if let Some(intent) = &self.manufacturing_intent {
            intent.validate().map_err(|e| invalid(&e.message))?;
        }
        if let Some(record) = &self.manufacturing_artifact {
            if self.occurrences.is_empty() {
                return Err(invalid("layout artifact requires nonempty geometry"));
            }
            record.validate().map_err(|e| invalid(&e.message))?;
            let intent = self
                .manufacturing_intent
                .as_ref()
                .ok_or_else(|| invalid("layout artifact requires intent"))?;
            let (surfaces, process) = layout_documents(&self.occurrences, surfaces, process);
            let used: BTreeSet<_> = self.occurrences.iter().map(|o| &o.definition_id).collect();
            let definitions: Vec<_> = definitions
                .iter()
                .filter(|d| used.contains(&d.definition_id))
                .cloned()
                .collect();
            let hash = input_fingerprint(
                project,
                &definitions,
                &self.occurrences,
                intent,
                &surfaces,
                &process,
            )
            .map_err(|e| invalid(&e.message))?;
            if hash != record.input_hash {
                return Err(invalid("layout artifact input hash is not current"));
            }
        }
        Ok(())
    }
}
pub fn validate_layout_name(name: &str) -> Result<(), ProjectError> {
    if name.trim().is_empty() || name.len() > 80 || name.chars().any(char::is_control) {
        return Err(invalid(
            "plate name must contain 1 to 80 bytes without control characters",
        ));
    }
    Ok(())
}
/// Keep shared definition defaults and only this layout's occurrence-owned contributions.
pub fn layout_documents(
    occurrences: &[OccurrenceRecord],
    surfaces: &SurfaceOverlays,
    process: &ProcessDocument,
) -> (SurfaceOverlays, ProcessDocument) {
    let ids: BTreeSet<_> = occurrences.iter().map(|o| o.occurrence_id).collect();
    let definitions: BTreeSet<_> = occurrences.iter().map(|o| &o.definition_id).collect();
    let includes = |scope: &SurfaceScope| match scope {
        SurfaceScope::Definition { definition_id } => definitions.contains(definition_id),
        SurfaceScope::Occurrence { occurrence_id, .. } => ids.contains(occurrence_id),
    };
    let mut surfaces = surfaces.clone();
    surfaces.layers.retain(|l| includes(&l.scope));
    let images: BTreeSet<_> = surfaces
        .layers
        .iter()
        .filter_map(|l| match &l.finish {
            SurfaceFinish::ImageRelief { image_hash, .. } => Some(image_hash),
            _ => None,
        })
        .collect();
    surfaces.images.retain(|i| images.contains(&i.image_hash));
    let mut process = process.clone();
    process.layers.retain(|l| includes(&l.scope));
    (surfaces, process)
}
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PrintLayoutInfo {
    pub layout_id: PrintLayoutId,
    pub name: String,
    pub occurrence_count: u32,
    pub configured: bool,
    pub has_result: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
pub enum PrintLayoutEdit {
    Create {
        name: String,
        copy_setup_from: Option<PrintLayoutId>,
    },
    Rename {
        layout_id: PrintLayoutId,
        name: String,
    },
    Remove {
        layout_id: PrintLayoutId,
    },
    Duplicate {
        source: PrintLayoutId,
        name: String,
    },
    Reorder {
        layout_ids: Vec<PrintLayoutId>,
    },
    Transfer {
        source: PrintLayoutId,
        target: PrintLayoutId,
        occurrences: Vec<OccurrenceId>,
        copy: bool,
    },
}
impl PrintLayoutEdit {
    pub fn validate(&self) -> Result<(), ProjectError> {
        match self {
            Self::Create { name, .. }
            | Self::Rename { name, .. }
            | Self::Duplicate { name, .. } => validate_layout_name(name),
            Self::Remove { .. } => Ok(()),
            Self::Reorder { layout_ids }
                if !layout_ids.is_empty()
                    && layout_ids.len() <= MAX_PRINT_LAYOUTS
                    && layout_ids.iter().collect::<BTreeSet<_>>().len() == layout_ids.len() =>
            {
                Ok(())
            }
            Self::Transfer {
                source,
                target,
                occurrences,
                ..
            } if source != target
                && !occurrences.is_empty()
                && occurrences.len() <= MAX_OCCURRENCES as usize
                && occurrences.iter().collect::<BTreeSet<_>>().len() == occurrences.len() =>
            {
                Ok(())
            }
            _ => Err(invalid("invalid plate order, transfer or quantity")),
        }
    }
}
