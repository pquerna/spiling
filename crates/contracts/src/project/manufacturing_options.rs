// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{ProjectError, ProjectErrorCode, invalid};
use crate::{geometry::SourceHash, manufacturing::ManufacturingIntent};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, sync::Arc};
use ts_rs::TS;

pub const MAX_MANUFACTURING_OPTIONS: usize = 8;
pub const MAX_MANUFACTURING_OPTIONS_BYTES: usize = 256 * 1024;
pub const MAX_MANUFACTURING_OPTION_NAME_BYTES: usize = 128;

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManufacturingOption {
    pub id: SourceHash,
    pub name: String,
    pub context_hash: SourceHash,
    pub intent: Arc<ManufacturingIntent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ManufacturingOptionInfo {
    pub id: SourceHash,
    pub name: String,
    pub context_hash: SourceHash,
}

impl ManufacturingOption {
    pub fn info(&self) -> ManufacturingOptionInfo {
        ManufacturingOptionInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            context_hash: self.context_hash.clone(),
        }
    }
    pub fn new(
        name: String,
        context_hash: SourceHash,
        intent: Arc<ManufacturingIntent>,
    ) -> Result<Self, ProjectError> {
        Self::validate_content(&name, &intent)?;
        let id = Self::identity(&name, &context_hash, &intent)?;
        Ok(Self {
            id,
            name,
            context_hash,
            intent,
        })
    }

    fn identity(
        name: &str,
        context_hash: &SourceHash,
        intent: &ManufacturingIntent,
    ) -> Result<SourceHash, ProjectError> {
        let bytes = serde_json::to_vec(&(
            "spiling.manufacturing-option.v1",
            name,
            context_hash,
            intent,
        ))
        .map_err(|_| invalid("manufacturing option identity cannot be encoded"))?;
        Ok(SourceHash::from_bytes(&bytes))
    }

    pub fn validate(&self) -> Result<(), ProjectError> {
        Self::validate_content(&self.name, &self.intent)?;
        if self.id != Self::identity(&self.name, &self.context_hash, &self.intent)? {
            return Err(invalid("manufacturing option content identity mismatch"));
        }
        Ok(())
    }

    fn validate_content(name: &str, intent: &ManufacturingIntent) -> Result<(), ProjectError> {
        if name.is_empty()
            || name.len() > MAX_MANUFACTURING_OPTION_NAME_BYTES
            || name.trim() != name
            || name.chars().any(char::is_control)
        {
            return Err(invalid(
                "manufacturing option name must be 1..128 bytes without surrounding whitespace or controls",
            ));
        }
        intent.validate().map_err(|error| {
            ProjectError::new(
                if error.code == crate::manufacturing::ManufacturingErrorCode::ResourceLimit {
                    ProjectErrorCode::ResourceLimit
                } else {
                    ProjectErrorCode::InvalidProject
                },
                error.message,
            )
        })
    }
}

pub fn validate_manufacturing_options(options: &[ManufacturingOption]) -> Result<(), ProjectError> {
    if options.len() > MAX_MANUFACTURING_OPTIONS {
        return Err(ProjectError::new(
            ProjectErrorCode::ResourceLimit,
            "manufacturing option count exceeds 8",
        ));
    }
    let mut identities = BTreeSet::new();
    let mut names = BTreeSet::new();
    for option in options {
        option.validate()?;
        if !identities.insert(&option.id) || !names.insert(&option.name) {
            return Err(invalid("duplicate manufacturing option identity or name"));
        }
    }
    let bytes = serde_json::to_vec(options)
        .map_err(|_| invalid("manufacturing options cannot be encoded"))?;
    if bytes.len() > MAX_MANUFACTURING_OPTIONS_BYTES {
        return Err(ProjectError::new(
            ProjectErrorCode::ResourceLimit,
            "manufacturing options exceed 256 KiB",
        ));
    }
    Ok(())
}
