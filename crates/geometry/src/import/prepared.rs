// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{ast::*, check_cancel, error, invalid, structure, units, unsupported};
use crate::{Definition, GeometryError};
use sha2::{Digest, Sha256};
use solidcore_step::p21::{Exchange, Param};
use spiling_contracts::geometry::{GeometryErrorCode, MAX_SOURCE_BYTES, SourceSolidId};
use std::{
    sync::atomic::AtomicBool,
    time::{Duration, Instant},
};
use step_p21::ast::{DataSection, EntityInstance, Name, Parameter, Record};

#[derive(Clone, Copy, Default)]
pub struct StepImportMeasurements {
    pub structure_parse_time: Duration,
    pub entity_indexes: usize,
    pub entity_index_time: Duration,
    pub native_parses: usize,
    pub native_parse_time: Duration,
    pub native_parse_reuses: usize,
    pub exchange_admission_time: Duration,
    pub exchange_retained_bytes: usize,
    pub peak_exchange_retained_bytes: usize,
    pub source_hash_time: Duration,
}

pub struct PreparedStepSource<'a> {
    pub(super) text: &'a str,
    data: DataSection,
    pub(super) bodies: Vec<structure::SourceBody>,
    retained_bytes: usize,
}

/// Optional source-local read state. It never increases prepared-source admission.
pub struct StepImportSession<'source, 'bytes> {
    source: &'source PreparedStepSource<'bytes>,
    digest: [u8; 32],
    exchange: Option<(Exchange, usize)>,
    measurements: StepImportMeasurements,
}

impl<'a> PreparedStepSource<'a> {
    /// `placement_bytes` admits the expanded assembly placements; pass the
    /// caller's remaining source admission.
    pub fn new(
        bytes: &'a [u8],
        placement_bytes: usize,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        Self::new_metered(bytes, placement_bytes, cancel).map(|(source, _)| source)
    }

    pub fn new_metered(
        bytes: &'a [u8],
        placement_bytes: usize,
        cancel: &AtomicBool,
    ) -> Result<(Self, StepImportMeasurements), GeometryError> {
        check_cancel(cancel)?;
        if bytes.len() > MAX_SOURCE_BYTES as usize {
            return Err(resource("STEP source cap exceeded"));
        }
        lexical_bound(bytes, cancel)?;
        let text = std::str::from_utf8(bytes).map_err(|_| invalid("STEP source is not UTF-8"))?;
        let parse_started = Instant::now();
        let mut exchange =
            step_p21::parser::parse(text).map_err(|_| invalid("invalid ISO-10303-21 exchange"))?;
        let structure_parse_time = parse_started.elapsed();
        check_cancel(cancel)?;
        if exchange.data.len() != 1 || !exchange.reference.is_empty() || !exchange.anchor.is_empty()
        {
            return Err(unsupported("one local DATA section is required"));
        }
        units::validate_schema(&exchange.header)?;
        let data = exchange.data.remove(0);
        let index_started = Instant::now();
        let entities = Entities::new(&data, cancel)?;
        let entity_index_time = index_started.elapsed();
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
        Ok((
            Self {
                text,
                data,
                bodies,
                retained_bytes,
            },
            StepImportMeasurements {
                structure_parse_time,
                entity_indexes: 1,
                entity_index_time,
                ..Default::default()
            },
        ))
    }

    pub fn bodies(&self) -> &[structure::SourceBody] {
        &self.bodies
    }

    pub fn retained_bytes(&self) -> usize {
        self.retained_bytes
    }

    pub fn try_session(
        &self,
        allowance: usize,
        mut measurements: StepImportMeasurements,
    ) -> Option<StepImportSession<'_, 'a>> {
        if allowance.checked_sub(self.retained_bytes)? < size_of::<StepImportSession<'_, '_>>() {
            return None;
        }
        let started = Instant::now();
        let digest = Sha256::digest(self.text.as_bytes()).into();
        measurements.source_hash_time = started.elapsed();
        Some(StepImportSession {
            source: self,
            digest,
            exchange: None,
            measurements,
        })
    }

    pub fn matches_bytes(&self, bytes: &[u8]) -> bool {
        std::ptr::eq(self.text.as_bytes(), bytes) || self.text.as_bytes() == bytes
    }

    pub fn read_solid(
        &self,
        selected: Option<&SourceSolidId>,
        cancel: &AtomicBool,
        face_limit: u32,
        query_proof_byte_limit: usize,
    ) -> Result<Definition, GeometryError> {
        super::import_prepared(
            self,
            selected,
            cancel,
            face_limit,
            query_proof_byte_limit,
            None,
            false,
        )
    }

    pub(super) fn entities(&self, cancel: &AtomicBool) -> Result<Entities<'_>, GeometryError> {
        Entities::new(&self.data, cancel)
    }
}

impl StepImportSession<'_, '_> {
    pub fn digest(&self) -> [u8; 32] {
        self.digest
    }

    pub fn measurements(&self) -> StepImportMeasurements {
        self.measurements
    }

    pub(super) fn record_index(&mut self, duration: Duration) {
        self.measurements.entity_indexes += 1;
        self.measurements.entity_index_time += duration;
    }

    pub(super) fn native_exchange(&mut self) -> Result<(Exchange, Option<usize>), GeometryError> {
        if let Some((exchange, bytes)) = self.exchange.take() {
            self.measurements.exchange_retained_bytes = 0;
            self.measurements.native_parse_reuses += 1;
            return Ok((exchange, Some(bytes)));
        }
        let started = Instant::now();
        let exchange = parse_native(self.source.text);
        self.measurements.native_parses += 1;
        self.measurements.native_parse_time += started.elapsed();
        exchange.map(|exchange| (exchange, None))
    }

    pub(super) fn retain_exchange(
        &mut self,
        exchange: Exchange,
        known_bytes: Option<usize>,
        allowance: usize,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        let started = Instant::now();
        let admitted = allowance
            .checked_sub(Self::session_overhead())
            .and_then(|allowance| match known_bytes {
                Some(bytes) => (bytes <= allowance).then_some(bytes),
                None => exchange_bytes(&exchange, allowance, cancel),
            });
        check_cancel(cancel)?;
        if let Some(bytes) = admitted {
            self.exchange = Some((exchange, bytes));
        }
        let metrics = &mut self.measurements;
        metrics.exchange_admission_time += started.elapsed();
        metrics.exchange_retained_bytes =
            admitted.map_or(0, |bytes| bytes + Self::session_overhead());
        metrics.peak_exchange_retained_bytes = metrics
            .peak_exchange_retained_bytes
            .max(metrics.exchange_retained_bytes);
        Ok(())
    }

    fn session_overhead() -> usize {
        size_of::<Self>() - size_of::<Exchange>()
    }

    fn clear_exchange(&mut self) {
        self.exchange = None;
        self.measurements.exchange_retained_bytes = 0;
    }

    /// A cache hit takes the place of the ordinary transient parse allocation.
    /// Retention between calls must fit beside the prepared source and the
    /// newly admitted definition. The query limit excludes the prepared source.
    /// Set `retain_for_next` only when another selected solid may follow.
    pub fn read_solid_with_reuse(
        &mut self,
        selected: Option<&SourceSolidId>,
        cancel: &AtomicBool,
        face_limit: u32,
        query_proof_byte_limit: usize,
        retain_for_next: bool,
    ) -> Result<Definition, GeometryError> {
        if self.measurements.exchange_retained_bytes > query_proof_byte_limit {
            self.clear_exchange();
        }
        let result = super::import_prepared(
            self.source,
            selected,
            cancel,
            face_limit,
            query_proof_byte_limit,
            Some(self),
            retain_for_next,
        );
        if result.is_err() {
            self.clear_exchange();
        }
        result
    }
}

pub(super) fn parse_native(text: &str) -> Result<Exchange, GeometryError> {
    solidcore_step::p21::parse(text)
        .map_err(|cause| invalid(&format!("invalid ISO-10303-21 exchange: {cause}")))
}

/// Account the retained capacities and bound the pinned std BTreeMap by one
/// full internal node per entry: eleven key/value slots, twelve child pointers,
/// and node bookkeeping. Parameter depth is bounded by the native parser.
fn exchange_bytes(exchange: &Exchange, allowance: usize, cancel: &AtomicBool) -> Option<usize> {
    fn add(bytes: &mut usize, amount: usize, allowance: usize) -> Option<()> {
        *bytes = bytes.checked_add(amount)?;
        (*bytes <= allowance).then_some(())
    }

    fn params(
        parameters: &[Param],
        bytes: &mut usize,
        allowance: usize,
        cancel: &AtomicBool,
    ) -> Option<()> {
        for param in parameters {
            check_cancel(cancel).ok()?;
            match param {
                Param::Str(value) | Param::Enum(value) | Param::Binary(value) => {
                    add(bytes, value.capacity(), allowance)?;
                }
                Param::Typed(_, children) | Param::List(children) => {
                    if let Param::Typed(name, _) = param {
                        add(bytes, name.capacity(), allowance)?;
                    }
                    add(
                        bytes,
                        children.capacity().checked_mul(size_of::<Param>())?,
                        allowance,
                    )?;
                    params(children, bytes, allowance, cancel)?;
                }
                _ => {}
            }
        }
        Some(())
    }

    let header = &exchange.header;
    let mut bytes = size_of::<Exchange>();
    for collection in [
        &header.descriptions,
        &header.authors,
        &header.organizations,
        &exchange.schemas,
    ] {
        add(
            &mut bytes,
            collection.capacity().checked_mul(size_of::<String>())?,
            allowance,
        )?;
        for value in collection {
            check_cancel(cancel).ok()?;
            add(&mut bytes, value.capacity(), allowance)?;
        }
    }
    for value in [
        &header.implementation_level,
        &header.file_name,
        &header.time_stamp,
        &header.preprocessor_version,
        &header.originating_system,
        &header.authorization,
        &exchange.originating_system,
        &exchange.preprocessor,
        &exchange.description,
    ] {
        add(&mut bytes, value.capacity(), allowance)?;
    }
    add(
        &mut bytes,
        exchange
            .duplicates
            .capacity()
            .checked_mul(size_of::<u64>())?,
        allowance,
    )?;
    let node_bytes = 11 * (size_of::<u64>() + size_of::<solidcore_step::p21::Entity>())
        + 16 * size_of::<usize>();
    add(
        &mut bytes,
        exchange.entities.len().checked_mul(node_bytes)?,
        allowance,
    )?;
    for entity in exchange.entities.values() {
        check_cancel(cancel).ok()?;
        add(
            &mut bytes,
            entity
                .records
                .capacity()
                .checked_mul(size_of::<solidcore_step::p21::Record>())?,
            allowance,
        )?;
        for record in &entity.records {
            add(&mut bytes, record.name.capacity(), allowance)?;
            add(
                &mut bytes,
                record.params.capacity().checked_mul(size_of::<Param>())?,
                allowance,
            )?;
            params(&record.params, &mut bytes, allowance, cancel)?;
        }
    }
    Some(bytes)
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
