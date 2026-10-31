// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use std::{
    collections::{HashMap, HashSet},
    fmt::Write,
};
use step_p21::ast::{EntityInstance, Exchange, Name, Parameter, Record};

pub fn parameters(r: &mut Record) -> &mut Vec<Parameter> {
    let Parameter::List(ps) = &mut r.parameter else {
        panic!("recipe record must contain parameter list")
    };
    ps
}
pub fn records_mut(exchange: &mut Exchange) -> impl Iterator<Item = &mut Record> {
    exchange.data[0].entities.iter_mut().flat_map(|e| match e {
        EntityInstance::Simple { record, .. } => std::slice::from_mut(record).iter_mut(),
        EntityInstance::Complex { subsuper, .. } => subsuper.0.iter_mut(),
    })
}
pub fn first<'a>(exchange: &'a mut Exchange, name: &str) -> &'a mut Record {
    records_mut(exchange)
        .find(|r| r.name == name)
        .expect("controlled baseline record")
}
pub fn next_id(exchange: &Exchange) -> u64 {
    exchange.data[0]
        .entities
        .iter()
        .map(|e| match e {
            EntityInstance::Simple { id, .. } | EntityInstance::Complex { id, .. } => *id,
        })
        .max()
        .unwrap()
        + 1
}
pub fn add(exchange: &mut Exchange, id: u64, source: &str) {
    let record: Record = source.parse().expect("original controlled STEP record");
    exchange.data[0]
        .entities
        .push(EntityInstance::Simple { id, record });
}

fn fields(record: &Record) -> Option<&[Parameter]> {
    match &record.parameter {
        Parameter::List(fields) => Some(fields),
        _ => None,
    }
}

fn referenced<'a>(records: &HashMap<u64, &'a Record>, reference: &Parameter) -> Option<&'a Record> {
    let Parameter::Ref(Name::Entity(id)) = reference else {
        return None;
    };
    records.get(id).copied()
}

fn associated_surface<'a>(
    records: &HashMap<u64, &'a Record>,
    association: &Parameter,
) -> Option<&'a Record> {
    let record = referenced(records, association)?;
    if record.name == "PCURVE" {
        referenced(records, fields(record)?.get(1)?)
    } else {
        Some(record)
    }
}

/// Exact declared co-carrier identity, independent of a placement's X gauge.
/// This authors explicit seams; it does not certify any remaining intersection.
fn same_declared_carrier(records: &HashMap<u64, &Record>, first: &Record, second: &Record) -> bool {
    if std::ptr::eq(first, second) {
        return true;
    }
    if first.name != second.name
        || !matches!(
            first.name.as_str(),
            "PLANE"
                | "CYLINDRICAL_SURFACE"
                | "CONICAL_SURFACE"
                | "SPHERICAL_SURFACE"
                | "TOROIDAL_SURFACE"
        )
    {
        return false;
    }
    let same = || -> Option<bool> {
        let a = fields(first)?;
        let b = fields(second)?;
        if a.get(2..)? != b.get(2..)? {
            return Some(false);
        }
        let a = referenced(records, a.get(1)?)?;
        let b = referenced(records, b.get(1)?)?;
        if a.name != "AXIS2_PLACEMENT_3D" || b.name != "AXIS2_PLACEMENT_3D" {
            return Some(false);
        }
        let a = fields(a)?;
        let b = fields(b)?;
        let pa = referenced(records, a.get(1)?)?;
        let pb = referenced(records, b.get(1)?)?;
        if pa.name != "CARTESIAN_POINT"
            || pb.name != "CARTESIAN_POINT"
            || fields(pa)?.get(1)? != fields(pb)?.get(1)?
        {
            return Some(false);
        }
        if first.name == "SPHERICAL_SURFACE" {
            return Some(true);
        }
        match (a.get(2)?, b.get(2)?) {
            (Parameter::NotProvided, Parameter::NotProvided) => Some(true),
            (a, b) => {
                let a = referenced(records, a)?;
                let b = referenced(records, b)?;
                Some(
                    a.name == "DIRECTION"
                        && b.name == "DIRECTION"
                        && fields(a)?.get(1)? == fields(b)?.get(1)?,
                )
            }
        }
    };
    same() == Some(true)
}

/// Original implicit-source variant of a controlled explicit-carrier recipe.
/// Keep proved co-carrier chart seams explicit: those carriers do not isolate
/// a transverse curve. Geometry, source IDs, associations and master choice
/// remain unchanged; the independent oracle is the recipe's analytic solid.
pub fn implicit_intersections(source: &str, filename: &str) -> String {
    let mut exchange = step_p21::parser::parse(source).expect("controlled original STEP source");
    parameters(
        exchange
            .header
            .iter_mut()
            .find(|record| record.name == "FILE_NAME")
            .expect("controlled original STEP filename"),
    )[0] = Parameter::String(filename.into());
    let mut changed = 0;
    let seams: HashSet<u64> = {
        let records: HashMap<_, _> = exchange.data[0]
            .entities
            .iter()
            .filter_map(|entity| match entity {
                EntityInstance::Simple { id, record } => Some((*id, record)),
                _ => None,
            })
            .collect();
        records
            .iter()
            .filter_map(|(id, record)| {
                if record.name != "SURFACE_CURVE" {
                    return None;
                }
                let Parameter::List(associations) = fields(record)?.get(2)? else {
                    return None;
                };
                let [a, b] = associations.as_slice() else {
                    return None;
                };
                let a = associated_surface(&records, a)?;
                let b = associated_surface(&records, b)?;
                same_declared_carrier(&records, a, b).then_some(*id)
            })
            .collect()
    };
    for entity in &mut exchange.data[0].entities {
        let (id, records) = match entity {
            EntityInstance::Simple { id, record } => (*id, std::slice::from_mut(record)),
            EntityInstance::Complex { id, subsuper } => (*id, subsuper.0.as_mut_slice()),
        };
        if seams.contains(&id) {
            continue;
        }
        for record in records {
            if record.name == "SURFACE_CURVE" {
                record.name = "INTERSECTION_CURVE".into();
                changed += 1;
            }
        }
    }
    assert!(
        changed > 0,
        "implicit source recipe needs explicit carrier intersections"
    );
    serialize(&exchange)
}

/// Invalid source LINE carrier while its original BREP vertices stay unchanged.
/// A converter must not silently replace this declaration by the vertex chord.
pub fn wrong_spatial_line(source: &str, filename: &str) -> String {
    let mut exchange = step_p21::parser::parse(source).expect("controlled original box source");
    parameters(
        exchange
            .header
            .iter_mut()
            .find(|record| record.name == "FILE_NAME")
            .unwrap(),
    )[0] = Parameter::String(filename.into());
    let point = next_id(&exchange);
    add(&mut exchange, point, "CARTESIAN_POINT('',(0.,0.,123.))");
    parameters(first(&mut exchange, "LINE"))[1] = Parameter::Ref(Name::Entity(point));
    serialize(&exchange)
}
fn emit_parameter(out: &mut String, p: &Parameter) {
    match p {
        Parameter::Typed { keyword, parameter } => {
            out.push_str(keyword);
            out.push('(');
            emit_parameter(out, parameter);
            out.push(')');
        }
        Parameter::Integer(v) => write!(out, "{v}").unwrap(),
        Parameter::Real(v) => {
            if v.is_infinite() {
                // Controlled nonfinite adversary uses a grammar-valid overflowing real.
                out.push_str("1.0E999");
            } else {
                // A real always carries a decimal point/exponent, even if integral.
                let s = format!("{v:.17e}");
                out.push_str(&s.replace('e', "E"));
            }
        }
        Parameter::String(s) => {
            out.push('\'');
            out.push_str(&s.replace('\'', "''"));
            out.push('\'');
        }
        Parameter::Enumeration(s) => {
            out.push('.');
            out.push_str(s);
            out.push('.');
        }
        Parameter::List(ps) => {
            out.push('(');
            for (index, p) in ps.iter().enumerate() {
                if index != 0 {
                    out.push(',');
                }
                emit_parameter(out, p);
            }
            out.push(')');
        }
        Parameter::Ref(Name::Entity(id)) => write!(out, "#{id}").unwrap(),
        Parameter::Ref(_) => panic!("original recipe does not use external references"),
        Parameter::NotProvided => out.push('$'),
        Parameter::Omitted => out.push('*'),
    }
}
fn emit_record(out: &mut String, r: &Record) {
    out.push_str(&r.name);
    emit_parameter(out, &r.parameter);
}
pub fn serialize(exchange: &Exchange) -> String {
    let mut out = String::from("ISO-10303-21;\nHEADER;\n");
    for r in &exchange.header {
        emit_record(&mut out, r);
        out.push_str(";\n");
    }
    out.push_str("ENDSEC;\nDATA;\n");
    for e in &exchange.data[0].entities {
        match e {
            EntityInstance::Simple { id, record } => {
                write!(&mut out, "#{id}=").unwrap();
                emit_record(&mut out, record);
            }
            EntityInstance::Complex { id, subsuper } => {
                write!(&mut out, "#{id}=(").unwrap();
                for r in &subsuper.0 {
                    emit_record(&mut out, r);
                    out.push(' ');
                }
                out.push(')');
            }
        }
        out.push_str(";\n");
    }
    out.push_str("ENDSEC;\nEND-ISO-10303-21;\n");
    out
}
