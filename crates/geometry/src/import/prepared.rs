// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{ast::*, check_cancel, error, invalid, structure, units, unsupported};
use crate::{Definition, GeometryError};
use spiling_contracts::geometry::{GeometryErrorCode, MAX_SOURCE_BYTES, SourceSolidId};
use std::sync::atomic::AtomicBool;
use step_p21::ast::{DataSection, EntityInstance, Name, Parameter, Record};

pub struct PreparedStepSource<'a> {
    pub(super) bytes: &'a [u8],
    pub(super) data: DataSection,
    pub(super) bodies: Vec<structure::SourceBody>,
    retained_bytes: usize,
}

impl<'a> PreparedStepSource<'a> {
    /// `placement_bytes` admits the expanded assembly placements; pass the
    /// caller's remaining source admission.
    pub fn new(
        bytes: &'a [u8],
        placement_bytes: usize,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        check_cancel(cancel)?;
        if bytes.len() > MAX_SOURCE_BYTES as usize {
            return Err(resource("STEP source cap exceeded"));
        }
        lexical_bound(bytes, cancel)?;
        let text = std::str::from_utf8(bytes).map_err(|_| invalid("STEP source is not UTF-8"))?;
        let mut exchange =
            step_p21::parser::parse(text).map_err(|_| invalid("invalid ISO-10303-21 exchange"))?;
        check_cancel(cancel)?;
        if exchange.data.len() != 1 || !exchange.reference.is_empty() || !exchange.anchor.is_empty()
        {
            return Err(unsupported("one local DATA section is required"));
        }
        units::validate_schema(&exchange.header)?;
        let data = exchange.data.remove(0);
        let entities = Entities::new(&data, cancel)?;
        let bodies = structure::bodies(&entities, placement_bytes, cancel)?;
        drop(entities);
        let mut retained_bytes = size_of::<Self>()
            .saturating_add(data.meta.capacity().saturating_mul(size_of::<Parameter>()))
            .saturating_add(
                data.entities
                    .capacity()
                    .saturating_mul(size_of::<EntityInstance>()),
            )
            .saturating_add(
                bodies
                    .capacity()
                    .saturating_mul(size_of::<structure::SourceBody>()),
            );
        for parameter in &data.meta {
            retained_bytes = retained_bytes.saturating_add(parameter_bytes(parameter, cancel)?);
        }
        for entity in &data.entities {
            check_cancel(cancel)?;
            match entity {
                EntityInstance::Simple { record, .. } => {
                    retained_bytes = retained_bytes.saturating_add(record_bytes(record, cancel)?);
                }
                EntityInstance::Complex { subsuper, .. } => {
                    retained_bytes = retained_bytes
                        .saturating_add(subsuper.0.capacity().saturating_mul(size_of::<Record>()));
                    for record in &subsuper.0 {
                        retained_bytes =
                            retained_bytes.saturating_add(record_bytes(record, cancel)?);
                    }
                }
            }
        }
        for body in &bodies {
            check_cancel(cancel)?;
            retained_bytes = retained_bytes
                .saturating_add(body.name.capacity())
                .saturating_add(body.solid.allocated_bytes())
                .saturating_add(
                    body.placements
                        .capacity()
                        .saturating_mul(size_of::<spiling_contracts::geometry::RigidPoseMm>()),
                );
        }
        Ok(Self {
            bytes,
            data,
            bodies,
            retained_bytes,
        })
    }

    pub fn bodies(&self) -> &[structure::SourceBody] {
        &self.bodies
    }

    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    pub fn matches_bytes(&self, bytes: &[u8]) -> bool {
        std::ptr::eq(self.bytes, bytes) || self.bytes == bytes
    }

    pub fn read_solid(
        &self,
        selected: Option<&SourceSolidId>,
        cancel: &AtomicBool,
        face_limit: u32,
        query_proof_byte_limit: usize,
    ) -> Result<Definition, GeometryError> {
        super::import_prepared(self, selected, cancel, face_limit, query_proof_byte_limit)
    }
}

fn resource(message: &str) -> GeometryError {
    error(GeometryErrorCode::ResourceLimit, message)
}

fn record_bytes(record: &Record, cancel: &AtomicBool) -> Result<usize, GeometryError> {
    Ok(record
        .name
        .capacity()
        .saturating_add(parameter_bytes(&record.parameter, cancel)?))
}

fn parameter_bytes(parameter: &Parameter, cancel: &AtomicBool) -> Result<usize, GeometryError> {
    let mut stack = Vec::new();
    stack
        .try_reserve(1)
        .map_err(|_| resource("STEP accounting stack allocation failed"))?;
    stack.push(std::slice::from_ref(parameter).iter());
    let mut bytes = 0usize;
    while let Some(iterator) = stack.last_mut() {
        check_cancel(cancel)?;
        let Some(parameter) = iterator.next() else {
            stack.pop();
            continue;
        };
        let children = match parameter {
            Parameter::Typed { keyword, parameter } => {
                bytes = bytes
                    .saturating_add(keyword.capacity())
                    .saturating_add(size_of::<Parameter>());
                Some(std::slice::from_ref(parameter.as_ref()))
            }
            Parameter::String(value)
            | Parameter::Enumeration(value)
            | Parameter::Ref(Name::ConstantEntity(value) | Name::ConstantValue(value)) => {
                bytes = bytes.saturating_add(value.capacity());
                None
            }
            Parameter::List(values) => {
                bytes =
                    bytes.saturating_add(values.capacity().saturating_mul(size_of::<Parameter>()));
                Some(values.as_slice())
            }
            _ => None,
        };
        if let Some(children) = children {
            stack
                .try_reserve(1)
                .map_err(|_| resource("STEP accounting stack allocation failed"))?;
            stack.push(children.iter());
        }
    }
    Ok(bytes)
}
