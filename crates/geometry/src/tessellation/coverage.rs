// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! An oriented triangle 2-chain has winding multiplicity determined by its
//! boundary. Positive triangles and exactly cancelled interior edges therefore
//! prove nonoverlap and complete coverage of the simple polygonal domain. Native
//! trim correspondence is a separate obligation; this graph alone is no proof
//! of coverage of a source face.
use super::{PolygonMesh, canonical_bits, invalid};
use crate::{GeometryWorkBudget, parametric::Interval};
use monstertruck_modeling::{
    Point2, PreparedFaceParameterDomain, SearchNearestParameter, TrimDomainError, TrimJointKind,
    ValidatedCurveJet2D, ValidatedGeometryBudget,
};
use spiling_contracts::geometry::{GeometryError, GeometryErrorCode};
use std::{
    collections::{HashMap, HashSet},
    sync::atomic::AtomicBool,
};

pub(super) const PROOF_BYTES: usize = 32 * 1024 * 1024;

pub(super) fn proof_memory_bound(polygon: &PolygonMesh) -> Result<usize, GeometryError> {
    if polygon.tri_faces().is_empty() {
        return Err(invalid("native coverage proof has no triangles"));
    }
    // Native vector capacities are already live, independently of their lengths.
    let mut native = polygon
        .positions()
        .capacity()
        .saturating_mul(24)
        .saturating_add(polygon.normals().capacity().saturating_mul(24))
        .saturating_add(polygon.uv_coords().capacity().saturating_mul(16))
        .saturating_add(
            polygon
                .tri_faces()
                .capacity()
                .saturating_mul(std::mem::size_of_val(&polygon.tri_faces()[0])),
        )
        .saturating_add(
            polygon
                .quad_faces()
                .capacity()
                .saturating_mul(std::mem::size_of::<
                    [monstertruck_meshing::prelude::StandardVertex; 4],
                >()),
        );
    native = native.saturating_add(polygon.other_faces().capacity().saturating_mul(
        std::mem::size_of::<Vec<monstertruck_meshing::prelude::StandardVertex>>(),
    ));
    for face in polygon.other_faces() {
        native = native.saturating_add(face.capacity().saturating_mul(std::mem::size_of::<
            monstertruck_meshing::prelude::StandardVertex,
        >()));
    }
    check_memory(native)?;
    Ok(native)
}

pub(super) fn check_memory(bytes: usize) -> Result<(), GeometryError> {
    if bytes > PROOF_BYTES {
        return Err(super::resource(&format!(
            "display proof requires {bytes} retained bytes; frozen cap is {PROOF_BYTES}"
        )));
    }
    Ok(())
}
pub(super) fn vector_bytes<T>(values: &Vec<T>) -> usize {
    values
        .capacity()
        .saturating_mul(std::mem::size_of::<T>())
        .saturating_add(std::mem::size_of::<Vec<T>>())
}
fn growth_capacity(length: usize, capacity: usize) -> usize {
    if length == capacity {
        capacity.saturating_mul(2).max(8)
    } else {
        capacity
    }
}
pub(super) fn vector_growth_bytes<T>(values: &Vec<T>) -> usize {
    growth_capacity(values.len(), values.capacity())
        .saturating_mul(std::mem::size_of::<T>())
        .saturating_add(std::mem::size_of::<Vec<T>>())
}
pub(super) fn map_bytes<K, V>(capacity: usize) -> usize {
    // Capacity-derived conservative table storage (7/8 load, controls and
    // alignment), not an invented per-triangle allocation. Tables with removals
    // retain their peak: tombstones can reduce insertion capacity.
    if capacity == 0 {
        return 0;
    }
    capacity
        .saturating_add(capacity / 7)
        .saturating_add(16)
        .saturating_mul(std::mem::size_of::<(K, V)>().saturating_add(1))
        .saturating_add(64)
}
fn reserve_map<K: Eq + std::hash::Hash, V>(
    values: &mut HashMap<K, V>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    if values.len() == values.capacity() {
        work.charge(1, cancel)?;
        values
            .try_reserve(1)
            .map_err(|_| super::resource("native graph map allocation failed"))?;
    }
    Ok(())
}
pub(super) fn reserve_vec<T>(
    values: &mut Vec<T>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(), GeometryError> {
    if values.len() == values.capacity() {
        work.charge(1, cancel)?;
        values
            .try_reserve(1)
            .map_err(|_| super::resource("native graph vector allocation failed"))?;
    }
    Ok(())
}
pub(super) fn circuit_bytes<B>(circuits: &Vec<Vec<Corner<B>>>) -> usize {
    circuits
        .iter()
        .fold(vector_bytes(circuits), |sum, circuit| {
            sum.saturating_add(vector_bytes(circuit))
        })
}

pub(super) fn correspondence_bytes<B>(
    circuits: &Vec<Vec<Corner<B>>>,
    coedge_count: usize,
    wire_count: usize,
) -> usize {
    let endpoints = circuits.iter().fold(0_usize, |sum, circuit| {
        sum.saturating_add(circuit.len().saturating_sub(1))
    });
    // Rust Vec<bool> stores bytes, not a bit-packed specialization.
    let used = endpoints.saturating_mul(std::mem::size_of::<bool>());
    circuit_bytes(circuits)
        .saturating_add(endpoints.saturating_mul(16))
        .saturating_add(used)
        .saturating_add(
            circuits
                .len()
                .saturating_mul(std::mem::size_of::<Vec<bool>>()),
        )
        .saturating_add(coedge_count.saturating_mul(32))
        .saturating_add(map_bytes::<(usize, usize), usize>(
            coedge_count.saturating_mul(2),
        ))
        .saturating_add(map_bytes::<usize, usize>(wire_count.saturating_mul(2)))
}

#[cfg(test)]
pub(super) fn certify(
    definition: &crate::kernel::Definition,
    ordinal: usize,
    polygon: &PolygonMesh,
    extra_retained_bytes: usize,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<f64, GeometryError> {
    let reserved = proof_memory_bound(polygon)?.saturating_add(extra_retained_bytes);
    check_memory(reserved)?;
    let prepared = crate::kernel::prepare_face_trim_domain_with_byte_limit(
        definition,
        ordinal,
        PROOF_BYTES - reserved,
        work,
        cancel,
    )?;
    certify_prepared(
        &PreparedCoverageQuery {
            definition,
            ordinal,
            polygon,
            reserved,
            prepared: prepared.native(),
            certified_uv_domain: prepared.certified_uv_domain(),
            source_chart_error_mm: prepared.source_chart_error_mm(),
            natural_indices: &[],
        },
        work,
        cancel,
    )
}

pub(super) struct PreparedCoverageQuery<'a, 'source> {
    pub(super) definition: &'a crate::kernel::Definition,
    pub(super) ordinal: usize,
    pub(super) polygon: &'a PolygonMesh,
    pub(super) reserved: usize,
    pub(super) prepared: &'a PreparedFaceParameterDomain<'source>,
    pub(super) certified_uv_domain: Option<[[f64; 2]; 2]>,
    pub(super) source_chart_error_mm: f64,
    pub(super) natural_indices: &'a [usize],
}

pub(super) fn certify_prepared(
    query: &PreparedCoverageQuery<'_, '_>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<f64, GeometryError> {
    let &PreparedCoverageQuery {
        definition,
        ordinal,
        polygon,
        reserved,
        prepared,
        certified_uv_domain,
        source_chart_error_mm,
        natural_indices,
    } = query;
    check_memory(reserved.saturating_add(prepared.retained_bytes()))?;
    let circuits = if natural_indices.is_empty() {
        triangulated_boundary(
            polygon,
            work,
            cancel,
            reserved.saturating_add(prepared.retained_bytes()),
        )?
    } else {
        triangulated_boundary_in_chart(
            polygon,
            work,
            cancel,
            reserved.saturating_add(prepared.retained_bytes()),
            Some((prepared, natural_indices)),
            |uv, _point, position| {
                Ok(Corner {
                    uv,
                    position,
                    bounds: (),
                })
            },
        )?
    };
    check_memory(
        reserved
            .saturating_add(prepared.retained_bytes())
            .saturating_add(correspondence_bytes(
                &circuits,
                prepared.coedges().len(),
                definition.face_boundaries[ordinal].wires.len(),
            )),
    )?;
    let endpoint_count = circuits.iter().map(|circuit| circuit.len() - 1).sum();
    work.charge(1, cancel)?;
    let mut endpoints = Vec::with_capacity(endpoint_count);
    for (loop_index, circuit) in circuits.iter().enumerate() {
        endpoints.extend((0..circuit.len() - 1).map(|vertex| (loop_index, vertex)));
    }
    endpoints.sort_unstable_by(|&(a, b), &(c, d)| {
        circuits[a][b].uv[0]
            .total_cmp(&circuits[c][d].uv[0])
            .then_with(|| (a, b).cmp(&(c, d)))
    });
    let domain = prepared;
    work.charge(circuits.len() as u64 + 4, cancel)?;
    let mut used: Vec<Vec<bool>> = circuits
        .iter()
        .map(|circuit| vec![false; circuit.len() - 1])
        .collect();
    let mut source_loops =
        HashMap::<usize, usize>::with_capacity(definition.face_boundaries[ordinal].wires.len());
    let mut maximum = 0.0_f64;
    let mut winding_alignment = 0_i8;
    let mut bindings = Vec::with_capacity(domain.coedges().len());
    let mut occurrences = HashMap::with_capacity(domain.coedges().len());
    for (index, coedge) in domain.coedges().iter().enumerate() {
        work.charge(1, cancel)?;
        let a = coedge.oriented_range[0].min(coedge.oriented_range[1]);
        let b = coedge.oriented_range[0].max(coedge.oriented_range[1]);
        let start = endpoint(domain, index, a, work, cancel)?;
        let end = endpoint(domain, index, b, work, cancel)?;
        let context = |error: GeometryError, endpoint: [Interval; 2], parameter: f64| {
            GeometryError::new(
                error.code,
                format!(
                    "source face {ordinal} original coedge {:?} at {parameter}, UV {endpoint:?}, mesh chart extremes {:?}..{:?}: {}",
                    (coedge.identity.wire_index, coedge.identity.coedge_index),
                    endpoints
                        .first()
                        .map(|&(wire, vertex)| circuits[wire][vertex].uv),
                    endpoints
                        .last()
                        .map(|&(wire, vertex)| circuits[wire][vertex].uv),
                    error.message,
                ),
            )
        };
        let start_vertex = find_endpoint(&circuits, &endpoints, start, work, cancel)
            .map_err(|error| context(error, start, a))?;
        let end_vertex = find_endpoint(&circuits, &endpoints, end, work, cancel)
            .map_err(|error| context(error, end, b))?;
        if start_vertex.0 != end_vertex.0 {
            return Err(invalid(
                "native coedge endpoints belong to different mesh loops",
            ));
        }
        bindings.push([start_vertex, end_vertex]);
        occurrences.insert(
            (coedge.identity.wire_index, coedge.identity.coedge_index),
            index,
        );
        let circuit_index = start_vertex.0;
        if let Some(&old) = source_loops.get(&coedge.identity.wire_index) {
            if old != circuit_index {
                return Err(invalid("one native trim loop maps to multiple mesh loops"));
            }
        } else {
            if source_loops.values().any(|&old| old == circuit_index) {
                return Err(invalid(
                    "multiple native trim loops collapse to one mesh loop",
                ));
            }
            source_loops.insert(coedge.identity.wire_index, circuit_index);
        }
        let circuit = &circuits[circuit_index];
        let count = circuit.len() - 1;
        let mut current = start_vertex.1;
        let candidates = [
            (1_i8, (current + 1) % count),
            (-1_i8, (current + count - 1) % count),
        ];
        let mut continuation: Option<(i8, f64, f64)> = None;
        for (direction, next) in candidates {
            let t = if next == end_vertex.1 {
                Some(b)
            } else {
                work.charge(100, cancel)?;
                // Rounded mesh UV need not lie exactly on the source trim.
                // This proposes a parameter; the complete span proof below
                // still bounds every original point and endpoint displacement.
                coedge.curve.search_nearest_parameter(
                    Point2::new(circuit[next].uv[0], circuit[next].uv[1]),
                    (a, b),
                    100,
                )
            };
            if let Some(t) = t
                && t.is_finite()
                && t > a
                && t <= b
            {
                let error = span_error(
                    domain,
                    index,
                    [a, t],
                    [circuit[current], circuit[next]],
                    certified_uv_domain,
                    work,
                    cancel,
                )?;
                // A clamped nearest proposal can be barely above the
                // endpoint on the wrong side. Select by its certified
                // displacement, not by the smallest proposed parameter.
                if continuation.is_none_or(|old| error < old.2) {
                    continuation = Some((direction, t, error));
                }
            }
        }
        let (direction, initial_parameter, initial_error) = continuation
            .ok_or_else(|| {
                GeometryError::new(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    format!(
                        "source face {ordinal} original coedge {:?}, raw range [{a}, {b}], initial UV {start:?}, terminal UV {end:?}, mesh endpoints {start_vertex:?}/{end_vertex:?}, neighboring UV {:?}: native coedge has no ordered mesh boundary continuation",
                        (coedge.identity.wire_index, coedge.identity.coedge_index),
                        candidates.map(|(_, next)| circuit[next].uv),
                    ),
                )
            })?;
        let alignment = direction * if coedge.identity.orientation { 1 } else { -1 };
        if winding_alignment == 0 {
            winding_alignment = alignment;
        }
        if alignment != winding_alignment {
            return Err(invalid(
                "native and polygon trim loops have inconsistent material winding",
            ));
        }
        let mut parameter = a;
        let mut traversed = 0;
        loop {
            work.charge(1, cancel)?;
            let next = if direction > 0 {
                (current + 1) % count
            } else {
                (current + count - 1) % count
            };
            let next_parameter = if traversed == 0 {
                initial_parameter
            } else if next == end_vertex.1 {
                b
            } else {
                work.charge(100, cancel)?;
                coedge
                    .curve
                    .search_nearest_parameter(
                        Point2::new(circuit[next].uv[0], circuit[next].uv[1]),
                        (parameter, b),
                        100,
                    )
                    .ok_or_else(|| correspondence("mesh boundary has no native coedge parameter"))?
            };
            if !next_parameter.is_finite() || next_parameter <= parameter || next_parameter > b {
                return Err(GeometryError::new(
                    GeometryErrorCode::ParameterCorrespondenceUnavailable,
                    format!(
                        "source face {ordinal} original coedge {:?}, raw range [{a}, {b}], parameters {parameter} -> {next_parameter}, neighboring UV {:?} -> {:?}: native coedge correspondence is nonmonotone/out-of-range",
                        (coedge.identity.wire_index, coedge.identity.coedge_index),
                        circuit[current].uv,
                        circuit[next].uv,
                    ),
                ));
            }
            let edge = if direction > 0 { current } else { next };
            if used[circuit_index][edge] {
                return Err(invalid(
                    "native mesh boundary edge corresponds to multiple source coedges",
                ));
            }
            let error = if traversed == 0 {
                initial_error
            } else {
                span_error(
                    domain,
                    index,
                    [parameter, next_parameter],
                    [circuit[current], circuit[next]],
                    certified_uv_domain,
                    work,
                    cancel,
                )?
            };
            maximum = maximum.max(error);
            used[circuit_index][edge] = true;
            parameter = next_parameter;
            current = next;
            traversed += 1;
            if current == end_vertex.1 {
                break;
            }
            if traversed >= count {
                return Err(invalid("native coedge fails complete mesh loop closure"));
            }
        }
    }
    for (index, coedge) in domain.coedges().iter().enumerate() {
        work.charge(1, cancel)?;
        let first = *occurrences
            .get(&(coedge.identity.wire_index, 0))
            .ok_or_else(|| invalid("native trim occurrence sequence has no first coedge"))?;
        let next = if index + 1 < domain.coedges().len()
            && domain.coedges()[index + 1].identity.wire_index == coedge.identity.wire_index
        {
            index + 1
        } else {
            first
        };
        // Identities and native joints follow material orientation, including
        // reversal of the absolute coedge sequence on a reversed source face.
        let (incoming_index, outgoing_index) = if domain.face().orientation() {
            (index, next)
        } else {
            (next, index)
        };
        let incoming = domain.coedges()[incoming_index].identity;
        let outgoing = domain.coedges()[outgoing_index].identity;
        let from = bindings[incoming_index][usize::from(incoming.orientation)];
        let to = bindings[outgoing_index][usize::from(!outgoing.orientation)];
        if from != to {
            let joint = domain
                .joints()
                .iter()
                .find(|joint| {
                    joint.incoming.wire_index == incoming.wire_index
                        && joint.incoming.coedge_index == incoming.coedge_index
                        && joint.outgoing.wire_index == outgoing.wire_index
                        && joint.outgoing.coedge_index == outgoing.coedge_index
                })
                .ok_or_else(|| {
                    correspondence("native trim joint has no original source incidence")
                })?;
            let TrimJointKind::NaturalBoundary {
                fixed_axis,
                connector,
                supported_parameter_box,
                world_enclosure,
                ..
            } = &joint.kind
            else {
                return Err(correspondence(
                    "native trim joint does not preserve mesh chart vertex identity",
                ));
            };
            if from.0 != to.0 || *fixed_axis > 1 {
                return Err(correspondence(
                    "native natural boundary changes source loop or parameter axis",
                ));
            }
            let circuit = &circuits[from.0];
            let count = circuit.len() - 1;
            let direction = winding_alignment;
            let varying = 1 - *fixed_axis;
            let delta = connector.1[varying] - connector.0[varying];
            if !delta.is_finite() || delta == 0. {
                return Err(correspondence(
                    "native natural boundary has no finite monotone connector",
                ));
            }
            let original = joint.vertex_point;
            let original = [original.x, original.y, original.z];
            let squared = (0..3).fold(Interval::point(0.), |sum, axis| {
                sum.add(
                    Interval {
                        lo: world_enclosure[axis].lower,
                        hi: world_enclosure[axis].upper,
                    }
                    .sub(Interval::point(original[axis]))
                    .square(),
                )
            });
            let world_error = Interval {
                lo: squared.lo.max(0.),
                hi: squared.hi,
            }
            .sqrt()?
            .hi;
            let mut current = from.1;
            let mut traversed = 0;
            loop {
                work.charge(1, cancel)?;
                let next = if direction > 0 {
                    (current + 1) % count
                } else {
                    (current + count - 1) % count
                };
                let edge = if direction > 0 { current } else { next };
                if used[from.0][edge] {
                    return Err(invalid(
                        "native natural boundary reuses a source coedge segment",
                    ));
                }
                for corner in [circuit[current], circuit[next]] {
                    if !(0..2).all(|axis| {
                        supported_parameter_box[axis][0] <= corner.uv[axis]
                            && corner.uv[axis] <= supported_parameter_box[axis][1]
                    }) || corner.point(polygon) != original
                    {
                        return Err(correspondence(
                            "natural boundary lacks its original vertex/tube witness",
                        ));
                    }
                }
                let advance = if delta > 0. {
                    circuit[next].uv[varying] > circuit[current].uv[varying]
                } else {
                    circuit[next].uv[varying] < circuit[current].uv[varying]
                };
                if !advance {
                    return Err(correspondence(
                        "native natural boundary correspondence is nonmonotone",
                    ));
                }
                used[from.0][edge] = true;
                current = next;
                traversed += 1;
                if current == to.1 {
                    break;
                }
                if traversed >= count {
                    return Err(correspondence(
                        "native natural boundary fails complete connector closure",
                    ));
                }
            }
            maximum = maximum.max(world_error);
        }
    }
    for joint in domain.joints() {
        work.charge(1, cancel)?;
        let index = *occurrences
            .get(&(joint.incoming.wire_index, joint.incoming.coedge_index))
            .ok_or_else(|| invalid("native trim uncertainty joint has no incoming occurrence"))?;
        let (circuit, vertex) = bindings[index][usize::from(joint.incoming.orientation)];
        let corner = circuits[circuit][vertex];
        if let TrimJointKind::NaturalBoundary {
            world_enclosure, ..
        } = &joint.kind
        {
            let point = joint.vertex_point;
            let point = [point.x, point.y, point.z];
            if corner.point(polygon) != point {
                return Err(correspondence(
                    "native natural boundary lost its original shared vertex",
                ));
            }
            let squared = (0..3).fold(Interval::point(0.), |sum, axis| {
                sum.add(
                    Interval {
                        lo: world_enclosure[axis].lower,
                        hi: world_enclosure[axis].upper,
                    }
                    .sub(Interval::point(point[axis]))
                    .square(),
                )
            });
            maximum = maximum.max(
                Interval {
                    lo: squared.lo.max(0.),
                    hi: squared.hi,
                }
                .sqrt()?
                .hi,
            );
            continue;
        }
        let joint_box = std::array::from_fn(|axis| {
            let margin = domain.uncertainty_uv()[axis];
            let tube = Interval {
                lo: joint.enclosure[axis].lower,
                hi: joint.enclosure[axis].upper,
            }
            .add(Interval {
                lo: -margin,
                hi: margin,
            });
            [tube.lo, tube.hi]
        });
        let joint_box = source_tube_in_domain(joint_box, certified_uv_domain)?;
        // A joint consumes only the complete carrier image. It neither needs
        // nor grants a C1 chart across an actual continuous native knot.
        let image = domain.face().with_surface(|surface| {
            crate::parametric::surface_piecewise_enclosure(surface, joint_box, work, cancel)
        })?;
        let squared = (0..3).fold(Interval::point(0.0), |sum, axis| {
            sum.add(
                image.position[axis]
                    .sub(Interval::point(corner.point(polygon)[axis]))
                    .square(),
            )
        });
        maximum = maximum.max(
            Interval {
                lo: squared.lo.max(0.0),
                hi: squared.hi,
            }
            .sqrt()?
            .hi,
        );
    }
    if source_loops.len() != circuits.len() || used.iter().flatten().any(|present| !present) {
        return Err(invalid(
            "native trim coverage has missing loops or unmatched boundary segments",
        ));
    }
    Ok(crate::parametric::up(maximum + source_chart_error_mm))
}

pub(super) fn trim_error(error: TrimDomainError) -> GeometryError {
    match error {
        TrimDomainError::Geometry(error) => crate::parametric::native_error(error),
        TrimDomainError::ResourceLimit => {
            GeometryError::new(GeometryErrorCode::ResourceLimit, error.to_string())
        }
        TrimDomainError::MissingExactTrim { .. }
        | TrimDomainError::UnclosedChart
        | TrimDomainError::UnsupportedSeam => correspondence(&error.to_string()),
        TrimDomainError::InvalidChart => invalid(&error.to_string()),
    }
}
fn correspondence(message: &str) -> GeometryError {
    GeometryError::new(
        GeometryErrorCode::ParameterCorrespondenceUnavailable,
        message,
    )
}
fn jet(
    domain: &PreparedFaceParameterDomain<'_>,
    coedge: usize,
    range: [f64; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<ValidatedCurveJet2D, GeometryError> {
    let mut native_work =
        ValidatedGeometryBudget::new(usize::try_from(work.remaining()).unwrap_or(usize::MAX));
    let result = domain.boundary_span_enclosure(coedge, range, &mut native_work, cancel);
    crate::parametric::debit_native_work(work, native_work.consumed(), cancel)?;
    result.map_err(trim_error)
}
fn endpoint(
    domain: &PreparedFaceParameterDomain<'_>,
    coedge: usize,
    t: f64,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<[Interval; 2], GeometryError> {
    Ok(jet(domain, coedge, [t, t], work, cancel)?
        .position
        .map(|value| Interval {
            lo: value.lower,
            hi: value.upper,
        }))
}
pub(super) fn find_endpoint<B: CoordinateEnclosure>(
    circuits: &[Vec<Corner<B>>],
    endpoints: &[(usize, usize)],
    point: [Interval; 2],
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<(usize, usize), GeometryError> {
    let mut found = None;
    let allowance = 1e-7 * (1.0 + point[0].abs_upper());
    let first = endpoints.partition_point(|&(loop_index, vertex)| {
        circuits[loop_index][vertex].uv[0] < point[0].lo - allowance
    });
    for &(loop_index, vertex) in &endpoints[first..] {
        let corner = &circuits[loop_index][vertex];
        if corner.uv[0] > point[0].hi + allowance {
            break;
        }
        work.charge(1, cancel)?;
        // Candidate location only: complete interval residuals, not this
        // tolerance or a nearest-parameter search, supply the certificate.
        if (0..2).all(|axis| {
            point[axis]
                .sub(Interval::point(corner.uv[axis]))
                .abs_upper()
                <= 1e-7 * (1.0 + point[axis].abs_upper())
        }) {
            if found.is_some() {
                return Err(correspondence(
                    "native trim endpoint has ambiguous mesh chart identity",
                ));
            }
            found = Some((loop_index, vertex));
        }
    }
    found.ok_or_else(|| correspondence("native trim endpoint is absent from mesh chart boundary"))
}

fn span_error(
    domain: &PreparedFaceParameterDomain<'_>,
    coedge: usize,
    range: [f64; 2],
    chord: [Corner; 2],
    certified_uv_domain: Option<[[f64; 2]; 2]>,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<f64, GeometryError> {
    let chord = chord.map(|corner| corner.uv.map(Interval::point));
    split_span_error(
        &TrimSpanQuery {
            domain,
            coedge,
            certified_uv_domain,
        },
        range,
        chord,
        0,
        work,
        cancel,
    )
}

struct TrimSpanQuery<'a, 'source> {
    domain: &'a PreparedFaceParameterDomain<'source>,
    coedge: usize,
    certified_uv_domain: Option<[[f64; 2]; 2]>,
}

fn split_span_error(
    query: &TrimSpanQuery<'_, '_>,
    range: [f64; 2],
    chord: [[Interval; 2]; 2],
    depth: u32,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<f64, GeometryError> {
    let &TrimSpanQuery {
        domain,
        coedge,
        certified_uv_domain,
    } = query;
    work.charge(1, cancel)?;
    if depth > 64 {
        return Err(GeometryError::new(
            GeometryErrorCode::ResourceLimit,
            "native trim knot subdivision depth exceeded",
        ));
    }
    let midpoint = 0.5 * range[0] + 0.5 * range[1];
    let split = domain
        .boundary_parameter_intervals(coedge)
        .map_err(trim_error)?
        .flat_map(|interval| interval.into_iter())
        .filter(|t| range[0] < *t && *t < range[1])
        .min_by(|a, b| (a - midpoint).abs().total_cmp(&(b - midpoint).abs()));
    if let Some(split) = split {
        let fraction = Interval::point(split)
            .sub(Interval::point(range[0]))
            .div(Interval::point(range[1]).sub(Interval::point(range[0])))?;
        let middle = std::array::from_fn(|axis| {
            chord[0][axis].add(chord[1][axis].sub(chord[0][axis]).mul(fraction))
        });
        let left = split_span_error(
            &TrimSpanQuery {
                domain,
                coedge,
                certified_uv_domain,
            },
            [range[0], split],
            [chord[0], middle],
            depth + 1,
            work,
            cancel,
        )?;
        let right = split_span_error(
            &TrimSpanQuery {
                domain,
                coedge,
                certified_uv_domain,
            },
            [split, range[1]],
            [middle, chord[1]],
            depth + 1,
            work,
            cancel,
        )?;
        return Ok(left.max(right));
    }
    let uv = jet(domain, coedge, range, work, cancel)?;
    let start = endpoint(domain, coedge, range[0], work, cancel)?;
    let end = endpoint(domain, coedge, range[1], work, cancel)?;
    let dt = Interval::point(range[1]).sub(Interval::point(range[0]));
    let second = uv.second.map(|value| Interval {
        lo: value.lower,
        hi: value.upper,
    });
    let uv_error: [Interval; 2] = std::array::from_fn(|axis| {
        let endpoints = start[axis]
            .sub(chord[0][axis])
            .abs_upper()
            .max(end[axis].sub(chord[1][axis]).abs_upper());
        second[axis]
            .mul(dt.square())
            .mul(Interval::point(0.125))
            .hull(Interval::point(0.0))
            .add(Interval {
                lo: -endpoints,
                hi: endpoints,
            })
            .add(Interval {
                lo: -domain.uncertainty_uv()[axis],
                hi: domain.uncertainty_uv()[axis],
            })
    });
    let source_box = std::array::from_fn(|axis| {
        let margin = domain.uncertainty_uv()[axis];
        let tube = Interval {
            lo: uv.position[axis].lower,
            hi: uv.position[axis].upper,
        }
        .add(Interval {
            lo: -margin,
            hi: margin,
        });
        [tube.lo, tube.hi]
    });
    let source_box = source_tube_in_domain(source_box, certified_uv_domain)?;
    let uv_box = std::array::from_fn(|axis| {
        [
            source_box[axis][0].min(chord[0][axis].lo.min(chord[1][axis].lo)),
            source_box[axis][1].max(chord[0][axis].hi.max(chord[1][axis].hi)),
        ]
    });
    // This is a first-order Lipschitz transport over the complete source tube,
    // not a surface Hessian/Taylor or normal proof. Native continuous piecewise
    // first jets enclose both sides of actual C0 knots. The UV curve's separate
    // second-order chord proof above still uses strict, knot-partitioned jets.
    let image = domain.face().with_surface(|surface| {
        crate::parametric::surface_piecewise_enclosure(surface, uv_box, work, cancel)
    })?;
    let displacement: [Interval; 3] = std::array::from_fn(|axis| {
        image.du[axis]
            .mul(Interval {
                lo: -uv_error[0].abs_upper(),
                hi: uv_error[0].abs_upper(),
            })
            .add(image.dv[axis].mul(Interval {
                lo: -uv_error[1].abs_upper(),
                hi: uv_error[1].abs_upper(),
            }))
    });
    let squared = displacement
        .into_iter()
        .fold(Interval::point(0.0), |sum, value| sum.add(value.square()));
    let bound = Interval {
        lo: squared.lo.max(0.0),
        hi: squared.hi,
    }
    .sqrt()?
    .hi;
    if !bound.is_finite() {
        return Err(GeometryError::new(
            GeometryErrorCode::ConditioningFailure,
            "native trim approximation has a nonfinite whole-span enclosure",
        ));
    }
    Ok(bound)
}

fn source_tube_in_domain(
    mut tube: [[f64; 2]; 2],
    certified_domain: Option<[[f64; 2]; 2]>,
) -> Result<[[f64; 2]; 2], GeometryError> {
    // Only the retained source-to-native closed domain proof permits this
    // intersection: never an incidental sweep net or periodic principal range.
    if let Some(domain) = certified_domain {
        for axis in 0..2 {
            tube[axis][0] = tube[axis][0].max(domain[axis][0]);
            tube[axis][1] = tube[axis][1].min(domain[axis][1]);
            if tube[axis][0] > tube[axis][1] {
                return Err(correspondence(
                    "source chart tube contradicts its certified native domain",
                ));
            }
        }
    }
    Ok(tube)
}

#[derive(Clone, Copy)]
pub(super) struct Corner<B = ()> {
    pub uv: [f64; 2],
    pub position: usize,
    pub bounds: B,
}
pub(super) trait CoordinateEnclosure: Copy {
    fn intervals(self, uv: [f64; 2]) -> [Interval; 2];
}
impl CoordinateEnclosure for () {
    fn intervals(self, uv: [f64; 2]) -> [Interval; 2] {
        uv.map(Interval::point)
    }
}
impl CoordinateEnclosure for [Interval; 2] {
    fn intervals(self, _uv: [f64; 2]) -> [Interval; 2] {
        self
    }
}
impl<B: CoordinateEnclosure> Corner<B> {
    pub fn chart_bounds(self) -> [Interval; 2] {
        self.bounds.intervals(self.uv)
    }
    pub fn point(self, polygon: &PolygonMesh) -> [f64; 3] {
        let point = polygon.positions()[self.position];
        [point.x, point.y, point.z]
    }
}
#[derive(Clone, Copy)]
struct EdgeUse {
    from: usize,
    to: usize,
}

pub(super) fn triangulated_boundary(
    polygon: &PolygonMesh,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    reserved: usize,
) -> Result<Vec<Vec<Corner>>, GeometryError> {
    triangulated_boundary_in_chart(
        polygon,
        work,
        cancel,
        reserved,
        None,
        |uv, _point, position| {
            Ok(Corner {
                uv,
                position,
                bounds: (),
            })
        },
    )
}

pub(super) fn triangulated_boundary_in_chart<B: CoordinateEnclosure>(
    polygon: &PolygonMesh,
    work: &mut GeometryWorkBudget,
    cancel: &AtomicBool,
    reserved: usize,
    natural: Option<(&PreparedFaceParameterDomain<'_>, &[usize])>,
    mut chart: impl FnMut([f64; 2], [f64; 3], usize) -> Result<Corner<B>, GeometryError>,
) -> Result<Vec<Vec<Corner<B>>>, GeometryError> {
    let mut corners = Vec::<Corner<B>>::new();
    let mut vertices = HashMap::<[u64; 2], usize>::new();
    let mut edges = HashMap::<(usize, usize), EdgeUse>::new();
    let mut triangle_sign = 0;
    let mut edge_capacity_peak = 0;
    'triangles: for triangle in polygon.tri_faces() {
        work.charge(1, cancel)?;
        let mut native_uvs = [[0.; 2]; 3];
        let mut native_points = [[0.; 3]; 3];
        for (i, vertex) in triangle.iter().enumerate() {
            let parameter = vertex
                .uv
                .and_then(|index| polygon.uv_coords().get(index))
                .ok_or_else(|| invalid("coverage audit has absent/out-of-range native UV"))?;
            let point = polygon
                .positions()
                .get(vertex.pos)
                .ok_or_else(|| invalid("coverage audit has out-of-range native position"))?;
            native_uvs[i] = [parameter.x, parameter.y];
            native_points[i] = [point.x, point.y, point.z];
            if native_uvs[i].iter().any(|value| !value.is_finite()) {
                return Err(invalid("coverage audit has nonfinite native UV"));
            }
        }
        if let Some((domain, indices)) = natural {
            for &index in indices {
                work.charge(1, cancel)?;
                let joint = domain
                    .joints()
                    .get(index)
                    .ok_or_else(|| invalid("native natural boundary index is absent"))?;
                let TrimJointKind::NaturalBoundary {
                    fixed_axis,
                    supported_parameter_box,
                    ..
                } = &joint.kind
                else {
                    return Err(invalid("native natural boundary index changed kind"));
                };
                if *fixed_axis > 1 {
                    return Err(invalid("native natural boundary axis is invalid"));
                }
                let point = joint.vertex_point;
                let point = [point.x, point.y, point.z];
                if native_uvs.iter().all(|uv| {
                    uv[*fixed_axis] == native_uvs[0][*fixed_axis]
                        && (0..2).all(|axis| {
                            supported_parameter_box[axis][0] <= uv[axis]
                                && uv[axis] <= supported_parameter_box[axis][1]
                        })
                }) && native_points.iter().all(|corner| *corner == point)
                {
                    // Exactly zero UV 2-simplex, NOT a nonzero-area pole wedge.
                    // The entire supported native image is charged by its joint.
                    continue 'triangles;
                }
            }
        }
        let mut ids = [0; 3];
        let mut bounds = [[Interval::point(0.0); 2]; 3];
        for (i, vertex) in triangle.iter().enumerate() {
            let corner = chart(native_uvs[i], native_points[i], vertex.pos)?;
            bounds[i] = corner.chart_bounds();
            let key = corner.uv.map(canonical_bits);
            ids[i] = if let Some(&index) = vertices.get(&key) {
                if corners[index].point(polygon) != corner.point(polygon) {
                    return Err(invalid(
                        "one native parameter has inconsistent mesh positions",
                    ));
                }
                index
            } else {
                work.charge(1, cancel)?;
                let extra = map_bytes::<[u64; 2], usize>(growth_capacity(
                    vertices.len(),
                    vertices.capacity(),
                ))
                .saturating_add(map_bytes::<(usize, usize), EdgeUse>(edge_capacity_peak))
                .saturating_add(vector_growth_bytes(&corners));
                check_memory(reserved.saturating_add(extra))?;
                reserve_map(&mut vertices, work, cancel)?;
                reserve_vec(&mut corners, work, cancel)?;
                let index = corners.len();
                vertices.insert(key, index);
                corners.push(corner);
                index
            };
        }
        let sign = orientation(bounds[0], bounds[1], bounds[2])?;
        if triangle_sign == 0 {
            triangle_sign = sign;
        }
        if sign != triangle_sign {
            return Err(invalid(
                "native UV triangle chain contains reversed winding",
            ));
        }
        // Normalize the parameter 2-chain, not display/native material normals.
        if sign < 0 {
            ids.swap(1, 2);
        }
        for (from, to) in [(ids[0], ids[1]), (ids[1], ids[2]), (ids[2], ids[0])] {
            let key = (from.min(to), from.max(to));
            if let Some(edge) = edges.remove(&key) {
                if edge.from != to || edge.to != from {
                    return Err(invalid(
                        "native UV triangle edge has duplicate oriented incidence",
                    ));
                }
                // Exact opposite integer-chain uses cancel. Positive triangle
                // area plus a simple boundary proves multiplicity without
                // retaining every already-cancelled interior edge.
            } else {
                work.charge(1, cancel)?;
                let extra = vector_bytes(&corners)
                    .saturating_add(map_bytes::<[u64; 2], usize>(vertices.capacity()))
                    .saturating_add(map_bytes::<(usize, usize), EdgeUse>(
                        if edges.len() == edges.capacity() {
                            edge_capacity_peak.saturating_mul(2).max(8)
                        } else {
                            edge_capacity_peak
                        },
                    ));
                check_memory(reserved.saturating_add(extra))?;
                reserve_map(&mut edges, work, cancel)?;
                edge_capacity_peak = edge_capacity_peak.max(edges.capacity());
                edges.insert(key, EdgeUse { from, to });
            }
        }
    }
    drop(vertices);
    let mut next = HashMap::<usize, usize>::new();
    let mut incoming = HashSet::<usize>::new();
    for edge in edges.values() {
        work.charge(1, cancel)?;
        let extra = vector_bytes(&corners)
            .saturating_add(map_bytes::<(usize, usize), EdgeUse>(edge_capacity_peak))
            .saturating_add(map_bytes::<usize, usize>(growth_capacity(
                next.len(),
                next.capacity(),
            )))
            .saturating_add(map_bytes::<usize, ()>(growth_capacity(
                incoming.len(),
                incoming.capacity(),
            )));
        check_memory(reserved.saturating_add(extra))?;
        reserve_map(&mut next, work, cancel)?;
        if incoming.len() == incoming.capacity() {
            work.charge(1, cancel)?;
            incoming
                .try_reserve(1)
                .map_err(|_| super::resource("native boundary vertex allocation failed"))?;
        }
        if next.insert(edge.from, edge.to).is_some() || !incoming.insert(edge.to) {
            return Err(invalid("native mesh boundary is branching or pinched"));
        }
    }
    drop(edges);
    if next.is_empty()
        || next.len() != incoming.len()
        || next.keys().any(|key| !incoming.contains(key))
    {
        return Err(invalid("native UV mesh boundary is absent/open"));
    }
    drop(incoming);
    check_memory(
        reserved
            .saturating_add(vector_bytes(&corners))
            .saturating_add(map_bytes::<usize, usize>(next.capacity()))
            .saturating_add(
                next.len()
                    .saturating_mul(std::mem::size_of::<(usize, usize)>()),
            ),
    )?;
    let mut boundary_edges: Vec<_> = next.iter().map(|(&a, &b)| (a, b)).collect();
    boundary_edges.sort_unstable_by(|&(a, b), &(c, d)| {
        corners[a].chart_bounds()[0]
            .lo
            .min(corners[b].chart_bounds()[0].lo)
            .total_cmp(
                &corners[c].chart_bounds()[0]
                    .lo
                    .min(corners[d].chart_bounds()[0].lo),
            )
            .then_with(|| (a, b).cmp(&(c, d)))
    });
    for (i, &(a, b)) in boundary_edges.iter().enumerate() {
        for &(c, d) in &boundary_edges[i + 1..] {
            if corners[c].chart_bounds()[0]
                .lo
                .min(corners[d].chart_bounds()[0].lo)
                > corners[a].chart_bounds()[0]
                    .hi
                    .max(corners[b].chart_bounds()[0].hi)
            {
                break;
            }
            work.charge(1, cancel)?;
            if a == c || a == d || b == c || b == d {
                continue;
            }
            if segments_may_intersect(
                corners[a].chart_bounds(),
                corners[b].chart_bounds(),
                corners[c].chart_bounds(),
                corners[d].chart_bounds(),
            )? {
                return Err(invalid("native mesh boundary crosses or overlaps itself"));
            }
        }
    }
    drop(boundary_edges);
    let mut loops = Vec::new();
    let next_capacity = next.capacity();
    while let Some(&start) = next.keys().min() {
        work.charge(1, cancel)?;
        check_memory(
            reserved
                .saturating_add(vector_bytes(&corners))
                .saturating_add(map_bytes::<usize, usize>(next_capacity))
                .saturating_add(circuit_bytes(&loops))
                .saturating_add(std::mem::size_of::<Corner<B>>() * 8),
        )?;
        work.charge(1, cancel)?;
        let mut circuit = vec![corners[start]];
        let mut current = start;
        loop {
            work.charge(1, cancel)?;
            let successor = next
                .remove(&current)
                .ok_or_else(|| invalid("native mesh boundary repeats before closure"))?;
            check_memory(
                reserved
                    .saturating_add(vector_bytes(&corners))
                    .saturating_add(map_bytes::<usize, usize>(next_capacity))
                    .saturating_add(circuit_bytes(&loops))
                    .saturating_add(vector_growth_bytes(&circuit)),
            )?;
            reserve_vec(&mut circuit, work, cancel)?;
            circuit.push(corners[successor]);
            if successor == start {
                break;
            }
            current = successor;
        }
        if circuit.len() < 4 {
            return Err(invalid("native mesh boundary has fewer than three edges"));
        }
        check_memory(
            reserved
                .saturating_add(vector_bytes(&corners))
                .saturating_add(map_bytes::<usize, usize>(next_capacity))
                .saturating_add(circuit_bytes(&loops))
                .saturating_add(vector_bytes(&circuit))
                .saturating_add(vector_growth_bytes(&loops)),
        )?;
        reserve_vec(&mut loops, work, cancel)?;
        loops.push(circuit);
    }
    Ok(loops)
}

fn orientation(a: [Interval; 2], b: [Interval; 2], c: [Interval; 2]) -> Result<i8, GeometryError> {
    let value = determinant(a, b, c);
    if value.lo > 0.0 {
        Ok(1)
    } else if value.hi < 0.0 {
        Ok(-1)
    } else {
        Err(GeometryError::new(
            GeometryErrorCode::AnalysisIndeterminate,
            "native UV orientation cannot be separated from zero",
        ))
    }
}
fn determinant(a: [Interval; 2], b: [Interval; 2], c: [Interval; 2]) -> Interval {
    let delta = |p: [Interval; 2], axis: usize| p[axis].sub(a[axis]);
    delta(b, 0)
        .mul(delta(c, 1))
        .sub(delta(b, 1).mul(delta(c, 0)))
}
fn segments_may_intersect(
    a: [Interval; 2],
    b: [Interval; 2],
    c: [Interval; 2],
    d: [Interval; 2],
) -> Result<bool, GeometryError> {
    if (0..2).any(|axis| {
        a[axis].hi.max(b[axis].hi) < c[axis].lo.min(d[axis].lo)
            || c[axis].hi.max(d[axis].hi) < a[axis].lo.min(b[axis].lo)
    }) {
        return Ok(false);
    }
    let side = [
        determinant(a, b, c),
        determinant(a, b, d),
        determinant(c, d, a),
        determinant(c, d, b),
    ];
    if (side[0].lo > 0.0 && side[1].lo > 0.0)
        || (side[0].hi < 0.0 && side[1].hi < 0.0)
        || (side[2].lo > 0.0 && side[3].lo > 0.0)
        || (side[2].hi < 0.0 && side[3].hi < 0.0)
    {
        return Ok(false);
    }
    if side.iter().any(|value| value.contains_zero()) {
        return Err(GeometryError::new(
            GeometryErrorCode::AnalysisIndeterminate,
            "native mesh boundary intersection ordering is unresolved",
        ));
    }
    Ok(true)
}
