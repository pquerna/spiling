// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{check_cancel, error, invalid, unsupported};
use spiling_contracts::geometry::{GeometryError, GeometryErrorCode, MAX_STEP_ENTITIES};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::AtomicBool,
};
use step_p21::ast::{DataSection, EntityInstance, Name, Parameter, Record};

pub(super) struct Entities<'a>(pub BTreeMap<u64, Vec<&'a Record>>);
impl<'a> Entities<'a> {
    pub fn new(data: &'a DataSection, cancel: &AtomicBool) -> Result<Self, GeometryError> {
        if data.entities.len() > MAX_STEP_ENTITIES as usize {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "STEP entity cap exceeded",
            ));
        }
        let mut map = BTreeMap::new();
        for entity in &data.entities {
            check_cancel(cancel)?;
            let (id, records) = match entity {
                EntityInstance::Simple { id, record } => (*id, vec![record]),
                EntityInstance::Complex { id, subsuper } => (
                    *id,
                    subsuper
                        .0
                        .iter()
                        .map(|record| {
                            check_cancel(cancel)?;
                            Ok(record)
                        })
                        .collect::<Result<Vec<_>, GeometryError>>()?,
                ),
            };
            if id == 0 || map.insert(id, records).is_some() {
                return Err(invalid("duplicate or zero STEP entity ID"));
            }
        }
        Ok(Self(map))
    }
    pub fn records(&self, id: u64) -> Result<&[&'a Record], GeometryError> {
        self.0
            .get(&id)
            .map(Vec::as_slice)
            .ok_or_else(|| invalid("missing STEP entity reference"))
    }
    pub fn record(&self, id: u64, name: &str) -> Result<&'a Record, GeometryError> {
        self.records(id)?
            .iter()
            .copied()
            .find(|r| r.name == name)
            .ok_or_else(|| invalid("STEP reference has wrong entity type"))
    }
    pub fn named(&self, name: &str, cancel: &AtomicBool) -> Result<Vec<u64>, GeometryError> {
        let mut matches = Vec::new();
        for (id, records) in &self.0 {
            for record in records {
                check_cancel(cancel)?;
                if record.name == name {
                    matches.push(*id);
                    break;
                }
            }
        }
        Ok(matches)
    }
    pub fn validate_geometry(&self, root: u64, cancel: &AtomicBool) -> Result<(), GeometryError> {
        let mut pending = vec![root];
        let mut visited = BTreeSet::new();
        let mut parameters = Vec::new();
        while let Some(id) = pending.pop() {
            check_cancel(cancel)?;
            if !visited.insert(id) {
                continue;
            }
            for record in self.records(id)? {
                check_cancel(cancel)?;
                if !matches!(
                    record.name.as_str(),
                    "MANIFOLD_SOLID_BREP"
                        | "CLOSED_SHELL"
                        | "ORIENTED_FACE"
                        | "ADVANCED_FACE"
                        | "FACE_SURFACE"
                        | "FACE_BOUND"
                        | "FACE_OUTER_BOUND"
                        | "EDGE_LOOP"
                        | "ORIENTED_EDGE"
                        | "EDGE_CURVE"
                        | "VERTEX_POINT"
                        | "CARTESIAN_POINT"
                        | "DIRECTION"
                        | "VECTOR"
                        | "AXIS2_PLACEMENT_3D"
                        | "AXIS2_PLACEMENT_2D"
                        | "PLANE"
                        | "CYLINDRICAL_SURFACE"
                        | "LINE"
                        | "CIRCLE"
                        | "TRIMMED_CURVE"
                        | "SURFACE_CURVE"
                        | "SEAM_CURVE"
                        | "PCURVE"
                        | "DEFINITIONAL_REPRESENTATION"
                        | "REPRESENTATION_CONTEXT"
                        | "GEOMETRIC_REPRESENTATION_CONTEXT"
                        | "PARAMETRIC_REPRESENTATION_CONTEXT"
                ) {
                    return Err(unsupported(&format!(
                        "reachable STEP #{id} {} is outside planar/cylindrical profile",
                        record.name
                    )));
                }
                parameters.push(&record.parameter);
                while let Some(parameter) = parameters.pop() {
                    check_cancel(cancel)?;
                    match parameter {
                        Parameter::Ref(Name::Entity(id)) => pending.push(*id),
                        Parameter::Ref(_) => {
                            return Err(invalid("unsupported STEP reference form"));
                        }
                        Parameter::Real(v) if !v.is_finite() => {
                            return Err(invalid("nonfinite source coordinate"));
                        }
                        Parameter::List(children) => parameters.extend(children.iter().rev()),
                        Parameter::Typed { parameter, .. } => parameters.push(parameter),
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    }
}
pub(super) fn list(p: &Parameter) -> Result<&[Parameter], GeometryError> {
    if let Parameter::List(v) = p {
        Ok(v)
    } else {
        Err(invalid("expected STEP parameter list"))
    }
}
pub(super) fn args(record: &Record) -> Result<&[Parameter], GeometryError> {
    list(&record.parameter)
}
pub(super) fn at(record: &Record, index: usize) -> Result<&Parameter, GeometryError> {
    args(record)?
        .get(index)
        .ok_or_else(|| invalid("missing STEP parameter"))
}
pub(super) fn reference(p: &Parameter) -> Result<u64, GeometryError> {
    if let Parameter::Ref(Name::Entity(id)) = p {
        Ok(*id)
    } else {
        Err(invalid("expected direct STEP entity reference"))
    }
}
pub(super) fn number(p: &Parameter) -> Result<f64, GeometryError> {
    let v = match p {
        Parameter::Real(v) => *v,
        Parameter::Integer(v) => *v as f64,
        _ => return Err(invalid("expected finite STEP number")),
    };
    if v.is_finite() {
        Ok(v)
    } else {
        Err(invalid("nonfinite STEP number"))
    }
}
pub(super) fn enumeration(p: &Parameter, value: &str) -> bool {
    matches!(p, Parameter::Enumeration(s) if s == value)
}
pub(super) fn vector(p: &Parameter) -> Result<[f64; 3], GeometryError> {
    let ps = list(p)?;
    if ps.len() != 3 {
        return Err(invalid("expected 3D coordinates"));
    }
    Ok([number(&ps[0])?, number(&ps[1])?, number(&ps[2])?])
}

/// Count entity assignments without interpreting strings/comments as syntax.
pub(super) fn lexical_bound(bytes: &[u8], cancel: &AtomicBool) -> Result<(), GeometryError> {
    let mut i = 0;
    let mut count = 0;
    let mut next_check = 0;
    while i < bytes.len() {
        poll_lexical(i, &mut next_check, cancel)?;
        match bytes[i] {
            b'\'' | b'"' => {
                let quote = bytes[i];
                i += 1;
                loop {
                    poll_lexical(i, &mut next_check, cancel)?;
                    if i == bytes.len() {
                        return Err(invalid("unterminated STEP string"));
                    }
                    if bytes[i] == quote {
                        i += 1;
                        if i < bytes.len() && bytes[i] == quote {
                            i += 1;
                        } else {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
            }
            b'/' if bytes.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i + 1 < bytes.len() && &bytes[i..i + 2] != b"*/" {
                    poll_lexical(i, &mut next_check, cancel)?;
                    i += 1;
                }
                if i + 1 == bytes.len() || i == bytes.len() {
                    return Err(invalid("unterminated STEP comment"));
                }
                i += 2;
            }
            b'#' => {
                i += 1;
                let first = i;
                while i < bytes.len() && bytes[i].is_ascii_digit() {
                    poll_lexical(i, &mut next_check, cancel)?;
                    i += 1;
                }
                if i == first {
                    continue;
                }
                // Comments are whitespace in Part 21, including before '='.
                loop {
                    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
                        poll_lexical(i, &mut next_check, cancel)?;
                        i += 1;
                    }
                    if bytes.get(i..i + 2) == Some(b"/*") {
                        i += 2;
                        while i + 1 < bytes.len() && &bytes[i..i + 2] != b"*/" {
                            poll_lexical(i, &mut next_check, cancel)?;
                            i += 1;
                        }
                        if i + 1 >= bytes.len() {
                            return Err(invalid("unterminated STEP comment"));
                        }
                        i += 2;
                    } else {
                        break;
                    }
                }
                if bytes.get(i) == Some(&b'=') {
                    count += 1;
                    if count > MAX_STEP_ENTITIES {
                        return Err(error(
                            GeometryErrorCode::ResourceLimit,
                            "STEP entity cap exceeded",
                        ));
                    }
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    Ok(())
}

fn poll_lexical(
    position: usize,
    next_check: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    if position >= *next_check {
        check_cancel(cancel)?;
        *next_check = position + 4096;
    }
    Ok(())
}
