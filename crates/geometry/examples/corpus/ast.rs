// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use std::fmt::Write;
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
