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

/// Original implicit-source variant of a controlled explicit-carrier recipe.
/// Geometry, source entity IDs and carrier incidence stay unchanged; only the
/// declared curve subtype changes. Its independent oracle remains the recipe's
/// analytic solid, not an import/export or sampled-curve comparison.
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
    for record in records_mut(&mut exchange) {
        if record.name == "SURFACE_CURVE" {
            record.name = "INTERSECTION_CURVE".into();
            changed += 1;
        }
    }
    assert!(
        changed > 0,
        "implicit source recipe needs explicit carrier intersections"
    );
    serialize(&exchange)
}

/// Controlled OCCT fitting adversary: its alternate top-face association stays
/// valid, but the preferred PCURVE is moved to the distinct bottom plane.
pub fn wrong_selected_master(source: &str, filename: &str) -> String {
    let mut exchange = step_p21::parser::parse(source).expect("controlled OCCT fitting source");
    parameters(
        exchange
            .header
            .iter_mut()
            .find(|record| record.name == "FILE_NAME")
            .unwrap(),
    )[0] = Parameter::String(filename.into());
    let selected = {
        let declaration = first(&mut exchange, "SURFACE_CURVE");
        let fields = parameters(declaration);
        assert!(matches!(&fields[3],Parameter::Enumeration(choice) if choice=="PCURVE_S1"));
        let Parameter::List(associations) = &fields[2] else {
            panic!("controlled associated geometry");
        };
        let Parameter::Ref(Name::Entity(selected)) = associations[0] else {
            panic!("controlled preferred PCURVE");
        };
        selected
    };
    let bottom_plane = exchange.data[0]
        .entities
        .iter()
        .filter_map(|entity| match entity {
            EntityInstance::Simple { id, record } if record.name == "PLANE" => Some(*id),
            _ => None,
        })
        .nth(1)
        .expect("controlled fitting has distinct top and bottom planes");
    let selected = exchange.data[0]
        .entities
        .iter_mut()
        .find_map(|entity| match entity {
            EntityInstance::Simple { id, record } if *id == selected => Some(record),
            _ => None,
        })
        .expect("controlled preferred PCURVE entity");
    assert_eq!(selected.name, "PCURVE");
    parameters(selected)[1] = Parameter::Ref(Name::Entity(bottom_plane));
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
