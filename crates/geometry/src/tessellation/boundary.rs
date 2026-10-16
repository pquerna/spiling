// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Complete native UV coverage survives physical natural-boundary quotients.
use super::{coverage, invalid};
use crate::{
    GeometryWorkBudget,
    kernel::{Definition, PreparedFaceTrimDomain},
    parametric::Interval,
};
use monstertruck_meshing::prelude::PolygonMesh;
use monstertruck_modeling::TrimJointKind;
use spiling_contracts::geometry::GeometryError;
use std::{collections::HashMap, sync::atomic::AtomicBool};

pub(super) struct BoundaryFace<'a> {
    trim: PreparedFaceTrimDomain<'a>,
    reserved_bytes: usize,
    natural: Vec<usize>,
}
impl<'a> BoundaryFace<'a> {
    pub(super) fn prepare(
        definition: &'a Definition,
        ordinal: usize,
        polygon: &PolygonMesh,
        extra_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Self, GeometryError> {
        let reserved_bytes = coverage::proof_memory_bound(polygon)?.saturating_add(extra_bytes);
        coverage::check_memory(reserved_bytes)?;
        let trim = crate::kernel::prepare_face_trim_domain_with_byte_limit(
            definition,
            ordinal,
            coverage::PROOF_BYTES - reserved_bytes,
            work,
            cancel,
        )?;
        let mut natural = Vec::new();
        for (index, joint) in trim.native().joints().iter().enumerate() {
            work.charge(1, cancel)?;
            if matches!(&joint.kind, TrimJointKind::NaturalBoundary { .. }) {
                coverage::check_memory(
                    reserved_bytes
                        .saturating_add(trim.retained_bytes())
                        .saturating_add(coverage::vector_growth_bytes(&natural)),
                )?;
                coverage::reserve_vec(&mut natural, work, cancel)?;
                natural.push(index);
            }
        }
        Ok(Self {
            trim,
            reserved_bytes,
            natural,
        })
    }
    pub(super) fn native_retained_bytes(&self) -> usize {
        self.trim
            .retained_bytes()
            .saturating_add(coverage::vector_bytes(&self.natural))
    }
    pub(super) fn vertex(&self, index: usize) -> [f64; 3] {
        let point = self.trim.native().joints()[self.natural[index]].vertex_point;
        [point.x, point.y, point.z]
    }
    pub(super) fn joint_count(&self) -> usize {
        self.natural.len()
    }
    pub(super) fn joint_kind(&self, index: usize) -> &TrimJointKind {
        &self.trim.native().joints()[self.natural[index]].kind
    }
    pub(super) fn supports_normal(&self, index: usize, uv: [f64; 2]) -> bool {
        match &self.trim.native().joints()[self.natural[index]].kind {
            TrimJointKind::NaturalBoundary {
                supported_parameter_box,
                ..
            } => in_box(uv, supported_parameter_box),
            _ => false,
        }
    }
    pub(super) fn triangle_joint(
        &self,
        uv: &[[f64; 2]; 3],
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<usize>, GeometryError> {
        let mut found: Option<usize> = None;
        for (index, &native_index) in self.natural.iter().enumerate() {
            work.charge(1, cancel)?;
            let joint = &self.trim.native().joints()[native_index];
            let TrimJointKind::NaturalBoundary {
                supported_parameter_box,
                ..
            } = &joint.kind
            else {
                return Err(invalid("native natural boundary index changed kind"));
            };
            if uv
                .iter()
                .any(|point| in_box(*point, supported_parameter_box))
            {
                if let Some(old) = found {
                    if self.trim.native().joints()[self.natural[old]].vertex_id != joint.vertex_id {
                        return Err(invalid(
                            "one native triangle spans distinct natural boundary vertices",
                        ));
                    }
                }
                found = Some(index);
            }
        }
        Ok(found)
    }
    pub(super) fn canonicalize(
        &self,
        polygon: &mut PolygonMesh,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), GeometryError> {
        if self.natural.is_empty() {
            return Ok(());
        }
        let mut bindings = HashMap::<usize, usize>::new();
        for triangle in polygon.tri_faces() {
            for vertex in triangle {
                work.charge(1, cancel)?;
                let uv = vertex
                    .uv
                    .and_then(|index| polygon.uv_coords().get(index))
                    .ok_or_else(|| {
                        invalid("natural boundary quotient requires native parameters")
                    })?;
                for (index, &native_index) in self.natural.iter().enumerate() {
                    work.charge(1, cancel)?;
                    let joint = &self.trim.native().joints()[native_index];
                    let TrimJointKind::NaturalBoundary {
                        supported_parameter_box,
                        world_enclosure,
                        ..
                    } = &joint.kind
                    else {
                        return Err(invalid("native natural boundary index changed kind"));
                    };
                    if !in_box([uv.x, uv.y], supported_parameter_box) {
                        continue;
                    }
                    if let Some(&old) = bindings.get(&vertex.pos) {
                        if self.trim.native().joints()[self.natural[old]].vertex_id
                            != joint.vertex_id
                        {
                            return Err(invalid(
                                "mesh natural boundary binds distinct original native vertices",
                            ));
                        }
                        continue;
                    }
                    let original = self.vertex(index);
                    let point = polygon
                        .positions()
                        .get(vertex.pos)
                        .ok_or_else(|| invalid("natural boundary position index out of range"))?;
                    let point = [point.x, point.y, point.z];
                    let bound = distance_bound(
                        world_enclosure.map(|value| Interval {
                            lo: value.lower,
                            hi: value.upper,
                        }),
                        original,
                    )?;
                    let actual = distance_bound(point.map(Interval::point), original)?;
                    if actual > bound {
                        return Err(invalid(
                            "mesh natural boundary exceeds original vertex world witness",
                        ));
                    }
                    if bindings.len() == bindings.capacity() {
                        coverage::check_memory(
                            self.reserved_bytes
                                .saturating_add(self.native_retained_bytes())
                                .saturating_add(coverage::map_bytes::<usize, usize>(
                                    bindings.capacity().saturating_mul(2).max(8),
                                )),
                        )?;
                        work.charge(1, cancel)?;
                        bindings.try_reserve(1).map_err(|_| {
                            super::resource("natural boundary binding allocation failed")
                        })?;
                    }
                    bindings.insert(vertex.pos, index);
                }
            }
        }
        for (position, joint) in bindings {
            work.charge(1, cancel)?;
            polygon.positions_mut()[position] =
                self.trim.native().joints()[self.natural[joint]].vertex_point;
        }
        Ok(())
    }
    pub(super) fn certify(
        &self,
        definition: &Definition,
        ordinal: usize,
        polygon: &PolygonMesh,
        extra_bytes: usize,
        work: &mut GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<f64, GeometryError> {
        coverage::certify_prepared(
            definition,
            ordinal,
            polygon,
            self.reserved_bytes
                .saturating_add(extra_bytes)
                .saturating_add(coverage::vector_bytes(&self.natural)),
            &self.trim,
            &self.natural,
            work,
            cancel,
        )
    }
}
fn in_box(point: [f64; 2], bounds: &[[f64; 2]; 2]) -> bool {
    (0..2).all(|axis| bounds[axis][0] <= point[axis] && point[axis] <= bounds[axis][1])
}
fn distance_bound(position: [Interval; 3], point: [f64; 3]) -> Result<f64, GeometryError> {
    let squared = (0..3).fold(Interval::point(0.), |sum, axis| {
        sum.add(position[axis].sub(Interval::point(point[axis])).square())
    });
    Ok(Interval {
        lo: squared.lo.max(0.),
        hi: squared.hi,
    }
    .sqrt()?
    .hi)
}
