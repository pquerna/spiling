// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{ProjectError, ProjectId, ProjectManifest, ProjectRevision, invalid};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, try_from = "RawGuard")]
pub struct ProjectRecoveryGuard {
    pub project_id: ProjectId,
    pub revision_high_water: ProjectRevision,
    pub next_occurrence_high_water: u32,
    pub next_surface_layer_high_water: u32,
    pub next_process_layer_high_water: u32,
    pub next_process_operation_high_water: u32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawGuard {
    project_id: ProjectId,
    revision_high_water: ProjectRevision,
    next_occurrence_high_water: u32,
    next_surface_layer_high_water: u32,
    next_process_layer_high_water: u32,
    next_process_operation_high_water: u32,
}

impl TryFrom<RawGuard> for ProjectRecoveryGuard {
    type Error = ProjectError;

    fn try_from(raw: RawGuard) -> Result<Self, Self::Error> {
        let guard = Self {
            project_id: raw.project_id,
            revision_high_water: raw.revision_high_water,
            next_occurrence_high_water: raw.next_occurrence_high_water,
            next_surface_layer_high_water: raw.next_surface_layer_high_water,
            next_process_layer_high_water: raw.next_process_layer_high_water,
            next_process_operation_high_water: raw.next_process_operation_high_water,
        };
        guard.validate()?;
        Ok(guard)
    }
}

impl ProjectRecoveryGuard {
    pub fn from_manifest(manifest: &ProjectManifest) -> Self {
        Self {
            project_id: manifest.project_id.clone(),
            revision_high_water: manifest.revision,
            next_occurrence_high_water: manifest.next_occurrence,
            next_surface_layer_high_water: manifest.next_surface_layer,
            next_process_layer_high_water: manifest.next_process_layer,
            next_process_operation_high_water: manifest.next_process_operation,
        }
    }

    pub fn validate(&self) -> Result<(), ProjectError> {
        if self.next_occurrence_high_water == 0
            || self.next_surface_layer_high_water == 0
            || self.next_process_layer_high_water == 0
            || self.next_process_operation_high_water == 0
        {
            return Err(invalid("recovery guard allocators must be nonzero"));
        }
        Ok(())
    }

    pub fn dominates(&self, manifest: &ProjectManifest) -> Result<(), ProjectError> {
        self.validate()?;
        if self.project_id != manifest.project_id
            || self.revision_high_water < manifest.revision
            || self.next_occurrence_high_water < manifest.next_occurrence
            || self.next_surface_layer_high_water < manifest.next_surface_layer
            || self.next_process_layer_high_water < manifest.next_process_layer
            || self.next_process_operation_high_water < manifest.next_process_operation
        {
            return Err(invalid(
                "recovery guard does not dominate the selected project root",
            ));
        }
        Ok(())
    }

    pub fn merge(&mut self, other: &Self) -> Result<(), ProjectError> {
        self.validate()?;
        other.validate()?;
        if self.project_id != other.project_id {
            return Err(invalid("recovery guard belongs to another project"));
        }
        self.revision_high_water = self.revision_high_water.max(other.revision_high_water);
        self.next_occurrence_high_water = self
            .next_occurrence_high_water
            .max(other.next_occurrence_high_water);
        self.next_surface_layer_high_water = self
            .next_surface_layer_high_water
            .max(other.next_surface_layer_high_water);
        self.next_process_layer_high_water = self
            .next_process_layer_high_water
            .max(other.next_process_layer_high_water);
        self.next_process_operation_high_water = self
            .next_process_operation_high_water
            .max(other.next_process_operation_high_water);
        Ok(())
    }

    pub fn apply_to(&self, manifest: &mut ProjectManifest) -> Result<(), ProjectError> {
        self.dominates(manifest)?;
        manifest.revision = self.revision_high_water;
        manifest.next_occurrence = self.next_occurrence_high_water;
        manifest.next_surface_layer = self.next_surface_layer_high_water;
        manifest.next_process_layer = self.next_process_layer_high_water;
        manifest.next_process_operation = self.next_process_operation_high_water;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard() -> ProjectRecoveryGuard {
        ProjectRecoveryGuard {
            project_id: ProjectId::new(),
            revision_high_water: ProjectRevision(10),
            next_occurrence_high_water: 20,
            next_surface_layer_high_water: 30,
            next_process_layer_high_water: 40,
            next_process_operation_high_water: 50,
        }
    }

    #[test]
    fn strict_guard_has_no_missing_unknown_duplicate_null_or_zero_fields() {
        let original = guard();
        let bytes = serde_json::to_vec(&original).unwrap();
        assert_eq!(
            serde_json::from_slice::<ProjectRecoveryGuard>(&bytes).unwrap(),
            original
        );
        let object = serde_json::to_value(&original).unwrap();
        for field in object.as_object().unwrap().keys() {
            let mut missing = object.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<ProjectRecoveryGuard>(missing).is_err());
            let mut null = object.clone();
            null[field] = serde_json::Value::Null;
            assert!(serde_json::from_value::<ProjectRecoveryGuard>(null).is_err());
            if field.starts_with("next_") {
                let mut zero = object.clone();
                zero[field] = serde_json::json!(0);
                assert!(serde_json::from_value::<ProjectRecoveryGuard>(zero).is_err());
            }
        }
        let mut unknown = object.clone();
        unknown["unexpected"] = serde_json::json!(false);
        assert!(serde_json::from_value::<ProjectRecoveryGuard>(unknown).is_err());
        let duplicate = String::from_utf8(bytes)
            .unwrap()
            .replace("{", "{\"revision_high_water\":1,");
        assert!(serde_json::from_str::<ProjectRecoveryGuard>(&duplicate).is_err());
        let mut trailing = serde_json::to_vec(&original).unwrap();
        trailing.extend_from_slice(b"{}");
        assert!(serde_json::from_slice::<ProjectRecoveryGuard>(&trailing).is_err());
    }

    #[test]
    fn guard_merge_is_project_bound_and_preserves_all_highwater_maxima() {
        let mut first = guard();
        let mut second = first.clone();
        second.revision_high_water = ProjectRevision(9);
        second.next_occurrence_high_water = 21;
        second.next_surface_layer_high_water = 31;
        second.next_process_layer_high_water = 41;
        second.next_process_operation_high_water = 51;
        first.merge(&second).unwrap();
        assert_eq!(first.revision_high_water, ProjectRevision(10));
        assert_eq!(first.next_occurrence_high_water, 21);
        assert_eq!(first.next_surface_layer_high_water, 31);
        assert_eq!(first.next_process_layer_high_water, 41);
        assert_eq!(first.next_process_operation_high_water, 51);
        let unchanged = first.clone();
        second.project_id = ProjectId::new();
        assert!(first.merge(&second).is_err());
        assert_eq!(first, unchanged);
    }
}
