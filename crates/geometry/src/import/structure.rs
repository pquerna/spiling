// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Product structure: which solids a STEP exchange contains and where its
//! assembly places each one. Each solid is still imported on its own, in its
//! own representation frame; this module only supplies the rigid placements.

use super::{ast::*, check_cancel, error, invalid, units, unsupported};
use spiling_contracts::geometry::{
    GeometryError, GeometryErrorCode, MAX_OCCURRENCES, RigidPoseMm, SourceSolidId,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::AtomicBool,
};
use step_p21::ast::Parameter;

/// One solid and every rigid placement of it in the source's root frame, in mm.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceBody {
    pub solid: SourceSolidId,
    /// The solid's own name in the source, if any.
    pub name: String,
    pub placements: Vec<RigidPoseMm>,
}

/// Every placed solid in an exchange, in source entity order.
pub fn source_bodies(bytes: &[u8], cancel: &AtomicBool) -> Result<Vec<SourceBody>, GeometryError> {
    check_cancel(cancel)?;
    if bytes.len() > super::MAX_SOURCE_BYTES as usize {
        return Err(error(
            GeometryErrorCode::ResourceLimit,
            "STEP source cap exceeded",
        ));
    }
    lexical_bound(bytes, cancel)?;
    let text = std::str::from_utf8(bytes).map_err(|_| invalid("STEP source is not UTF-8"))?;
    let exchange =
        step_p21::parser::parse(text).map_err(|_| invalid("invalid ISO-10303-21 exchange"))?;
    if exchange.data.len() != 1 || !exchange.reference.is_empty() || !exchange.anchor.is_empty() {
        return Err(unsupported("one local DATA section is required"));
    }
    units::validate_schema(&exchange.header)?;
    let entities = Entities::new(&exchange.data[0], cancel)?;
    bodies(&entities, cancel)
}

/// Rigid map from a representation's frame to its parent's: `p' = R p + t`.
#[derive(Debug, Clone, Copy)]
struct Rigid {
    /// Columns are the local x, y and z axes in the parent frame.
    r: [[f64; 3]; 3],
    t: [f64; 3],
}
impl Rigid {
    const IDENTITY: Self = Self {
        r: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        t: [0.0; 3],
    };
    fn apply(&self, v: [f64; 3]) -> [f64; 3] {
        std::array::from_fn(|i| (0..3).map(|j| self.r[j][i] * v[j]).sum::<f64>())
    }
    fn then(self, outer: Self) -> Self {
        // outer ∘ self
        let r = self.r.map(|column| outer.apply(column));
        let t = outer.apply(self.t);
        Self {
            r,
            t: std::array::from_fn(|i| t[i] + outer.t[i]),
        }
    }
    fn inverse(self) -> Self {
        let r: [[f64; 3]; 3] = std::array::from_fn(|i| std::array::from_fn(|j| self.r[j][i]));
        let inverse = Self { r, t: [0.0; 3] };
        let t = inverse.apply(self.t);
        Self {
            r,
            t: t.map(|v| -v),
        }
    }
    fn pose(self) -> Result<RigidPoseMm, GeometryError> {
        // Shepperd's method on the orthonormal rotation.
        let m = |row: usize, column: usize| self.r[column][row];
        let trace = m(0, 0) + m(1, 1) + m(2, 2);
        let q = if trace > 0.0 {
            let s = (trace + 1.0).sqrt() * 2.0;
            [
                (m(2, 1) - m(1, 2)) / s,
                (m(0, 2) - m(2, 0)) / s,
                (m(1, 0) - m(0, 1)) / s,
                0.25 * s,
            ]
        } else if m(0, 0) > m(1, 1) && m(0, 0) > m(2, 2) {
            let s = (1.0 + m(0, 0) - m(1, 1) - m(2, 2)).sqrt() * 2.0;
            [
                0.25 * s,
                (m(0, 1) + m(1, 0)) / s,
                (m(0, 2) + m(2, 0)) / s,
                (m(2, 1) - m(1, 2)) / s,
            ]
        } else if m(1, 1) > m(2, 2) {
            let s = (1.0 + m(1, 1) - m(0, 0) - m(2, 2)).sqrt() * 2.0;
            [
                (m(0, 1) + m(1, 0)) / s,
                0.25 * s,
                (m(1, 2) + m(2, 1)) / s,
                (m(0, 2) - m(2, 0)) / s,
            ]
        } else {
            let s = (1.0 + m(2, 2) - m(0, 0) - m(1, 1)).sqrt() * 2.0;
            [
                (m(0, 2) + m(2, 0)) / s,
                (m(1, 2) + m(2, 1)) / s,
                0.25 * s,
                (m(1, 0) - m(0, 1)) / s,
            ]
        };
        let norm = q.iter().fold(0.0_f64, |sum, v| sum.hypot(*v));
        let pose = RigidPoseMm {
            translation_mm: self.t,
            rotation_xyzw: q.map(|v| v / norm),
        };
        pose.validate()?;
        Ok(pose)
    }
}

const SHAPE_REPRESENTATIONS: [&str; 4] = [
    "SHAPE_REPRESENTATION",
    "ADVANCED_BREP_SHAPE_REPRESENTATION",
    "MANIFOLD_SURFACE_SHAPE_REPRESENTATION",
    "GEOMETRICALLY_BOUNDED_SURFACE_SHAPE_REPRESENTATION",
];
fn is_shape_representation(entities: &Entities<'_>, id: u64) -> bool {
    entities.records(id).is_ok_and(|records| {
        records
            .iter()
            .any(|r| SHAPE_REPRESENTATIONS.contains(&r.name.as_str()))
    })
}
fn shape_representation<'a>(
    entities: &Entities<'a>,
    id: u64,
) -> Result<&'a step_p21::ast::Record, GeometryError> {
    entities
        .records(id)?
        .iter()
        .copied()
        .find(|r| SHAPE_REPRESENTATIONS.contains(&r.name.as_str()))
        .ok_or_else(|| invalid("assembly link does not reference a shape representation"))
}

/// An AXIS2_PLACEMENT_3D as the map from its local frame to the frame it is
/// declared in, translation scaled to mm.
fn axis_placement(entities: &Entities<'_>, id: u64, scale: f64) -> Result<Rigid, GeometryError> {
    let frame = super::placement_frame(entities, id, scale)?;
    Ok(Rigid {
        r: [frame.x_axis, frame.y_axis, frame.z_axis],
        t: frame.origin_mm,
    })
}

fn length_scale(
    entities: &Entities<'_>,
    representation: u64,
    cancel: &AtomicBool,
) -> Result<f64, GeometryError> {
    let record = shape_representation(entities, representation)?;
    let context = reference(at(record, 2)?)?;
    Ok(units::context(entities, context, cancel)?
        .length
        .scale_to_mm())
}

struct Groups(BTreeMap<u64, u64>);
impl Groups {
    fn find(&mut self, id: u64) -> u64 {
        let mut root = id;
        while let Some(&parent) = self.0.get(&root) {
            if parent == root {
                break;
            }
            root = parent;
        }
        self.0.insert(id, root);
        root
    }
    fn union(&mut self, a: u64, b: u64) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.0.insert(a.max(b), a.min(b));
        }
    }
}

pub(super) fn bodies(
    entities: &Entities<'_>,
    cancel: &AtomicBool,
) -> Result<Vec<SourceBody>, GeometryError> {
    // Representations in one frame: plain shape-representation relationships.
    let mut groups = Groups(BTreeMap::new());
    // Placed child representations: (parent rep, child rep, child -> parent).
    let mut links = Vec::new();
    // Assembly usage direction: product definition -> its shape representations,
    // and the usage relating each transformed relationship.
    let mut definition_shapes: BTreeMap<u64, Vec<u64>> = BTreeMap::new();
    let mut usages: BTreeMap<u64, (u64, u64)> = BTreeMap::new();
    for records in entities.0.values() {
        check_cancel(cancel)?;
        for record in records {
            match record.name.as_str() {
                "SHAPE_DEFINITION_REPRESENTATION" => {
                    let shape = reference(at(record, 0)?)?;
                    let representation = reference(at(record, 1)?)?;
                    if let Ok(definition_shape) = entities.record(shape, "PRODUCT_DEFINITION_SHAPE")
                        && let Ok(definition) = reference(at(definition_shape, 2)?)
                    {
                        definition_shapes
                            .entry(definition)
                            .or_default()
                            .push(representation);
                    }
                }
                "NEXT_ASSEMBLY_USAGE_OCCURRENCE" | "ASSEMBLY_COMPONENT_USAGE" => {
                    let relating = reference(at(record, 3)?)?;
                    let related = reference(at(record, 4)?)?;
                    if relating == related {
                        return Err(unsupported("assembly usage places a product inside itself"));
                    }
                    for definition in [relating, related] {
                        entities.record(definition, "PRODUCT_DEFINITION")?;
                    }
                }
                "CONTEXT_DEPENDENT_SHAPE_REPRESENTATION" => {
                    let relationship = reference(at(record, 0)?)?;
                    let shape =
                        entities.record(reference(at(record, 1)?)?, "PRODUCT_DEFINITION_SHAPE")?;
                    let usage = reference(at(shape, 2)?)?;
                    let usage = entities
                        .records(usage)?
                        .iter()
                        .find(|r| {
                            matches!(
                                r.name.as_str(),
                                "NEXT_ASSEMBLY_USAGE_OCCURRENCE" | "ASSEMBLY_COMPONENT_USAGE"
                            )
                        })
                        .ok_or_else(|| {
                            unsupported("context-dependent shape without an assembly usage")
                        })?;
                    // relating (parent) and related (child) product definitions
                    usages.insert(
                        relationship,
                        (reference(at(usage, 3)?)?, reference(at(usage, 4)?)?),
                    );
                }
                _ => {}
            }
        }
    }
    for (&id, records) in &entities.0 {
        check_cancel(cancel)?;
        let Some(relationship) = records.iter().find(|r| {
            matches!(
                r.name.as_str(),
                "REPRESENTATION_RELATIONSHIP" | "SHAPE_REPRESENTATION_RELATIONSHIP"
            ) && args(r).is_ok_and(|ps| ps.len() == 4)
        }) else {
            continue;
        };
        let first = reference(at(relationship, 2)?)?;
        let second = reference(at(relationship, 3)?)?;
        if !is_shape_representation(entities, first) || !is_shape_representation(entities, second) {
            continue;
        }
        let Some(with) = records
            .iter()
            .find(|r| r.name == "REPRESENTATION_RELATIONSHIP_WITH_TRANSFORMATION")
        else {
            groups.union(first, second);
            continue;
        };
        let transformation = reference(at(with, 0)?)?;
        let item = entities
            .record(transformation, "ITEM_DEFINED_TRANSFORMATION")
            .map_err(|_| unsupported("assembly transformation is not an item-defined placement"))?;
        // ISO 10303-43: the transformation maps item 1 (in representation 1)
        // onto item 2 (in representation 2).
        let one = axis_placement(
            entities,
            reference(at(item, 2)?)?,
            length_scale(entities, first, cancel)?,
        )?;
        let two = axis_placement(
            entities,
            reference(at(item, 3)?)?,
            length_scale(entities, second, cancel)?,
        )?;
        let first_to_second = one.inverse().then(two);
        let first_is_child = match usages.get(&id) {
            Some(&(parent, child)) => {
                let shapes = |definition| {
                    definition_shapes
                        .get(&definition)
                        .cloned()
                        .unwrap_or_default()
                };
                let (parent, child) = (shapes(parent), shapes(child));
                if child.contains(&first) || parent.contains(&second) {
                    true
                } else if child.contains(&second) || parent.contains(&first) {
                    false
                } else {
                    return Err(unsupported(
                        "assembly usage does not identify its placed representation",
                    ));
                }
            }
            // Without a usage, representation 1 is the placed component.
            None => true,
        };
        if first_is_child {
            links.push((second, first, first_to_second));
        } else {
            links.push((first, second, first_to_second.inverse()));
        }
    }
    // Mapped items place a whole representation inside another.
    for records in entities.0.values() {
        check_cancel(cancel)?;
        for record in records {
            if !SHAPE_REPRESENTATIONS.contains(&record.name.as_str()) {
                continue;
            }
            let Ok(items) = list(at(record, 1)?) else {
                continue;
            };
            for item in items {
                let Ok(item) = reference(item) else { continue };
                let Ok(mapped) = entities.record(item, "MAPPED_ITEM") else {
                    continue;
                };
                let map = entities.record(reference(at(mapped, 1)?)?, "REPRESENTATION_MAP")?;
                let child = reference(at(map, 1)?)?;
                let origin = axis_placement(
                    entities,
                    reference(at(map, 0)?)?,
                    length_scale(entities, child, cancel)?,
                )?;
                let parent = *representations_containing(entities, item, cancel)?
                    .first()
                    .ok_or_else(|| {
                        unsupported("mapped item is not listed by a shape representation")
                    })?;
                let target = axis_placement(
                    entities,
                    reference(at(mapped, 2)?)?,
                    length_scale(entities, parent, cancel)?,
                )?;
                links.push((parent, child, origin.inverse().then(target)));
            }
        }
    }
    // Placements of every representation group, from the unplaced roots down.
    let mut solids = Vec::new();
    for item in super::SOLID_ITEMS {
        solids.extend(entities.named(item, cancel)?);
    }
    solids.sort_unstable();
    solids.dedup();
    let mut owners = BTreeMap::new();
    for &solid in &solids {
        // Exporters may list a solid in several representations of one frame.
        let mut containing = representations_containing(entities, solid, cancel)?.into_iter();
        let first = containing
            .next()
            .ok_or_else(|| unsupported("solid is not listed by a shape representation"))?;
        for other in containing {
            groups.union(first, other);
        }
        owners.insert(solid, first);
    }
    let owners: BTreeMap<u64, u64> = owners
        .into_iter()
        .map(|(solid, representation)| (solid, groups.find(representation)))
        .collect();
    let mut children: BTreeMap<u64, Vec<(u64, Rigid)>> = BTreeMap::new();
    let mut placed = BTreeSet::new();
    for (parent, child, map) in links {
        let (parent, child) = (groups.find(parent), groups.find(child));
        placed.insert(child);
        children.entry(parent).or_default().push((child, map));
    }
    let mut placements: BTreeMap<u64, Vec<Rigid>> = BTreeMap::new();
    let mut stack: Vec<(u64, Rigid, usize)> = owners
        .values()
        .chain(children.keys())
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|group| groups.find(group))
        .filter(|group| !placed.contains(group))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|root| (root, Rigid::IDENTITY, 0))
        .collect();
    let mut total = 0usize;
    while let Some((group, world, depth)) = stack.pop() {
        check_cancel(cancel)?;
        if depth > 64 {
            return Err(invalid("assembly placement graph is cyclic or too deep"));
        }
        total += 1;
        if total > 4 * MAX_OCCURRENCES as usize {
            return Err(error(
                GeometryErrorCode::ResourceLimit,
                "assembly places too many components",
            ));
        }
        placements.entry(group).or_default().push(world);
        for &(child, map) in children.get(&group).map(Vec::as_slice).unwrap_or_default() {
            stack.push((child, map.then(world), depth + 1));
        }
    }
    let mut bodies = Vec::with_capacity(solids.len());
    for solid in solids {
        let record = entities
            .records(solid)?
            .iter()
            .copied()
            .find(|r| super::SOLID_ITEMS.contains(&r.name.as_str()))
            .ok_or_else(|| invalid("STEP reference has wrong entity type"))?;
        let name = match at(record, 0)? {
            Parameter::String(name) => name.clone(),
            _ => String::new(),
        };
        let placements = placements
            .get(&owners[&solid])
            .map(|worlds| {
                worlds
                    .iter()
                    .map(|world| world.pose())
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()?
            .unwrap_or_default();
        if placements.is_empty() {
            // A solid in a representation no root reaches is not placed in the product.
            continue;
        }
        bodies.push(SourceBody {
            solid: SourceSolidId::from_step_entity(solid)?,
            name,
            placements,
        });
    }
    Ok(bodies)
}

/// Every shape representation listing `item` among its items.
fn representations_containing(
    entities: &Entities<'_>,
    item: u64,
    cancel: &AtomicBool,
) -> Result<Vec<u64>, GeometryError> {
    let mut found = Vec::new();
    for (&id, records) in &entities.0 {
        check_cancel(cancel)?;
        for record in records {
            if SHAPE_REPRESENTATIONS.contains(&record.name.as_str())
                && list(at(record, 1)?)?
                    .iter()
                    .any(|p| reference(p).ok() == Some(item))
            {
                found.push(id);
                break;
            }
        }
    }
    Ok(found)
}
