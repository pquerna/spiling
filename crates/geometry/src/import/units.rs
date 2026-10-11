// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{ast::*, check_cancel, error};
use spiling_contracts::geometry::{GeometryError, GeometryErrorCode, SourceUnit};
use std::{collections::BTreeSet, sync::atomic::AtomicBool};
use step_p21::ast::{Parameter, Record};

fn units_error() -> GeometryError {
    error(
        GeometryErrorCode::UnsupportedUnits,
        "missing, conflicting or unsupported representation units",
    )
}
fn dimension(entities: &Entities<'_>, p: &Parameter, length: bool) -> Result<(), GeometryError> {
    if matches!(p, Parameter::Omitted) {
        return Ok(());
    }
    let id = reference(p).map_err(|_| units_error())?;
    let r = entities
        .record(id, "DIMENSIONAL_EXPONENTS")
        .map_err(|_| units_error())?;
    let ps = args(r).map_err(|_| units_error())?;
    if ps.len() != 7 {
        return Err(units_error());
    }
    for (index, p) in ps.iter().enumerate() {
        if number(p).map_err(|_| units_error())? != if length && index == 0 { 1.0 } else { 0.0 } {
            return Err(units_error());
        }
    }
    Ok(())
}
fn measure(p: &Parameter, keyword_expected: &str) -> Result<f64, GeometryError> {
    if let Parameter::Typed { keyword, parameter } = p
        && keyword == keyword_expected
    {
        return match parameter.as_ref() {
            Parameter::List(ps) if ps.len() == 1 => number(&ps[0]),
            p => number(p),
        }
        .map_err(|_| units_error());
    }
    Err(units_error())
}
fn unit(
    entities: &Entities<'_>,
    id: u64,
    cancel: &AtomicBool,
) -> Result<(bool, f64, Option<SourceUnit>), GeometryError> {
    let mut current = id;
    let mut seen = BTreeSet::new();
    let mut factors = Vec::new();
    let (length, mut scale, mut source) = loop {
        check_cancel(cancel)?;
        if !seen.insert(current) {
            return Err(units_error());
        }
        let records = entities.records(current).map_err(|_| units_error())?;
        if records.len() != 3
            || records
                .iter()
                .enumerate()
                .any(|(i, r)| records[..i].iter().any(|other| other.name == r.name))
            || records.iter().any(|r| {
                !matches!(
                    r.name.as_str(),
                    "LENGTH_UNIT"
                        | "PLANE_ANGLE_UNIT"
                        | "NAMED_UNIT"
                        | "SI_UNIT"
                        | "CONVERSION_BASED_UNIT"
                )
            })
        {
            return Err(units_error());
        }
        let length = records.iter().any(|r| r.name == "LENGTH_UNIT");
        let angle = records.iter().any(|r| r.name == "PLANE_ANGLE_UNIT");
        if length == angle {
            return Err(units_error());
        }
        let named = records
            .iter()
            .find(|r| r.name == "NAMED_UNIT")
            .ok_or_else(units_error)?;
        dimension(entities, at(named, 0).map_err(|_| units_error())?, length)?;
        if let Some(si) = records.iter().find(|r| r.name == "SI_UNIT") {
            let prefix = at(si, 0).map_err(|_| units_error())?;
            let name = at(si, 1).map_err(|_| units_error())?;
            break if length && enumeration(name, "METRE") {
                if matches!(prefix, Parameter::NotProvided) {
                    (true, 1000.0, Some(SourceUnit::Metre))
                } else if enumeration(prefix, "MILLI") {
                    (true, 1.0, Some(SourceUnit::Millimetre))
                } else {
                    return Err(units_error());
                }
            } else if angle
                && enumeration(name, "RADIAN")
                && matches!(prefix, Parameter::NotProvided)
            {
                (false, 1.0, None)
            } else {
                return Err(units_error());
            };
        }
        let conversion = records
            .iter()
            .find(|r| r.name == "CONVERSION_BASED_UNIT")
            .ok_or_else(units_error)?;
        if matches!(at(named, 0).map_err(|_| units_error())?, Parameter::Omitted) {
            return Err(units_error());
        }
        if !length
            || !matches!(at(conversion, 0).map_err(|_| units_error())?, Parameter::String(name) if name.eq_ignore_ascii_case("inch"))
        {
            return Err(units_error());
        }
        let factor_id =
            reference(at(conversion, 1).map_err(|_| units_error())?).map_err(|_| units_error())?;
        let factor = entities
            .record(factor_id, "LENGTH_MEASURE_WITH_UNIT")
            .map_err(|_| units_error())?;
        factors.push(measure(
            at(factor, 0).map_err(|_| units_error())?,
            "LENGTH_MEASURE",
        )?);
        current =
            reference(at(factor, 1).map_err(|_| units_error())?).map_err(|_| units_error())?;
    };
    // Unwind values, not recursive calls: the source cap bounds this heap storage.
    for value in factors.into_iter().rev() {
        check_cancel(cancel)?;
        scale *= value;
        if !length || !scale.is_finite() || (scale - 25.4).abs() > 1e-12 {
            return Err(units_error());
        }
        source = Some(SourceUnit::Inch);
    }
    Ok((length, scale, source))
}
fn context_units(
    entities: &Entities<'_>,
    id: u64,
    cancel: &AtomicBool,
) -> Result<(SourceUnit, Option<f64>), GeometryError> {
    let records = entities.records(id).map_err(|_| units_error())?;
    if records.len() > 4
        || records
            .iter()
            .enumerate()
            .any(|(i, r)| records[..i].iter().any(|other| other.name == r.name))
    {
        return Err(units_error());
    }
    let geometric = entities
        .record(id, "GEOMETRIC_REPRESENTATION_CONTEXT")
        .map_err(|_| units_error())?;
    if number(at(geometric, 0).map_err(|_| units_error())?).map_err(|_| units_error())? != 3.0 {
        return Err(units_error());
    }
    let assigned = entities
        .record(id, "GLOBAL_UNIT_ASSIGNED_CONTEXT")
        .map_err(|_| units_error())?;
    let mut length = None;
    let mut radians = false;
    for p in list(at(assigned, 0).map_err(|_| units_error())?).map_err(|_| units_error())? {
        check_cancel(cancel)?;
        let uid = reference(p).map_err(|_| units_error())?;
        // Solid-angle units are permitted context metadata, but cannot substitute for radians.
        if entities
            .records(uid)
            .map_err(|_| units_error())?
            .iter()
            .any(|r| r.name == "SOLID_ANGLE_UNIT")
        {
            continue;
        }
        let (is_length, _, source) = unit(entities, uid, cancel)?;
        if is_length {
            if length.replace(source.ok_or_else(units_error)?).is_some() {
                return Err(units_error());
            }
        } else if std::mem::replace(&mut radians, true) {
            return Err(units_error());
        }
    }
    let length = length.ok_or_else(units_error)?;
    if !radians {
        return Err(units_error());
    }
    let mut uncertainty = None;
    if let Ok(record) = entities.record(id, "GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT") {
        for p in list(at(record, 0).map_err(|_| units_error())?).map_err(|_| units_error())? {
            check_cancel(cancel)?;
            let r = entities
                .record(
                    reference(p).map_err(|_| units_error())?,
                    "UNCERTAINTY_MEASURE_WITH_UNIT",
                )
                .map_err(|_| units_error())?;
            let value = measure(at(r, 0).map_err(|_| units_error())?, "LENGTH_MEASURE")?;
            let (is_length, scale, _) = unit(
                entities,
                reference(at(r, 1).map_err(|_| units_error())?).map_err(|_| units_error())?,
                cancel,
            )?;
            let value = value * scale;
            if !is_length
                || !value.is_finite()
                || value < 0.0
                || uncertainty.replace(value).is_some_and(|old| old != value)
            {
                return Err(units_error());
            }
        }
    }
    Ok((length, uncertainty))
}
pub(super) fn resolve(
    entities: &Entities<'_>,
    solid: u64,
    cancel: &AtomicBool,
) -> Result<(SourceUnit, Option<f64>), GeometryError> {
    let mut resolved = None;
    for records in entities.0.values() {
        check_cancel(cancel)?;
        for r in records {
            if !matches!(
                r.name.as_str(),
                "ADVANCED_BREP_SHAPE_REPRESENTATION"
                    | "SHAPE_REPRESENTATION"
                    | "MANIFOLD_SURFACE_SHAPE_REPRESENTATION"
            ) {
                continue;
            }
            let items = list(at(r, 1).map_err(|_| units_error())?).map_err(|_| units_error())?;
            let mut references_solid = false;
            for item in items {
                check_cancel(cancel)?;
                if reference(item).ok() == Some(solid) {
                    references_solid = true;
                    break;
                }
            }
            if !references_solid {
                continue;
            }
            let current = context_units(
                entities,
                reference(at(r, 2).map_err(|_| units_error())?).map_err(|_| units_error())?,
                cancel,
            )?;
            if resolved
                .replace(current)
                .is_some_and(|previous| previous != current)
            {
                return Err(units_error());
            }
        }
    }
    resolved.ok_or_else(units_error)
}

pub(super) fn validate_schema(header: &[Record]) -> Result<(), GeometryError> {
    let schemas = header
        .iter()
        .filter(|r| r.name == "FILE_SCHEMA")
        .collect::<Vec<_>>();
    if schemas.len() != 1 {
        return Err(super::unsupported(
            "one AP203/AP214 FILE_SCHEMA is required",
        ));
    }
    let ps = args(schemas[0])?;
    let names = ps
        .first()
        .ok_or_else(|| super::invalid("missing FILE_SCHEMA names"))?;
    let names = list(names)?;
    if names.len() != 1 || !matches!(&names[0], Parameter::String(s) if supported_schema(s)) {
        return Err(super::unsupported("only AP203/AP214 schemas are admitted"));
    }
    Ok(())
}

fn supported_schema(identifier: &str) -> bool {
    let (name, qualifier) = match identifier.split_once('{') {
        Some((name, qualifier)) => (name.trim(), Some(qualifier)),
        None => (identifier.trim(), None),
    };
    let family = if name.eq_ignore_ascii_case("AUTOMOTIVE_DESIGN") {
        214
    } else if name.eq_ignore_ascii_case("CONFIG_CONTROL_DESIGN") {
        203
    } else {
        return false;
    };
    let Some(qualifier) = qualifier else {
        return true;
    };
    let Some(qualifier) = qualifier.trim_end().strip_suffix('}') else {
        return false;
    };
    let mut numbers = qualifier.split_whitespace();
    for expected in [1, 0, 10303, family] {
        if numbers.next().and_then(|value| value.parse::<u64>().ok()) != Some(expected) {
            return false;
        }
    }
    let mut edition_components = 0;
    for value in numbers {
        if !matches!(value.parse::<u64>(), Ok(1..)) {
            return false;
        }
        edition_components += 1;
    }
    // Registered schema identifiers may append edition/conformance arcs.
    // The named family and ISO/AP prefix above remain mandatory.
    edition_components >= 3
}
