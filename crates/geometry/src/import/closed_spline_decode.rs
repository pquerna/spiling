// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! A temporary representation for the pinned decoder's clamped-closure gate.
//! Neither the exchange observed by source capture nor the published native
//! carrier keeps this representation. Original PCURVEs are never changed.

use super::{check_cancel, error, invalid};
use solidcraft_geom::brep::Surface;
use solidcraft_step::p21::{Entity, Exchange, Param};
use spiling_contracts::geometry::{GeometryError, GeometryErrorCode, MAX_NATIVE_FACES};
use std::{mem::size_of, sync::atomic::AtomicBool};

type Result<T, E = GeometryError> = std::result::Result<T, E>;

struct Axis {
    carrier: u64,
    record: usize,
    slot: usize,
    direction: usize,
    controls: [usize; 2],
    rational: bool,
    // Swapped, not cloned: these hold the literal source Params during decoding.
    multiplicities: Param,
    knots: Param,
    original: Vec<f64>,
    decoded: Vec<f64>,
}

fn charge(bytes: &mut usize, additional: usize, limit: usize) -> Result<()> {
    *bytes = bytes
        .checked_add(additional)
        .filter(|n| *n <= limit)
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "closed spline decode scratch capacity exhausted",
            )
        })?;
    Ok(())
}

fn numeric(p: &Param) -> Option<f64> {
    // Only scalar numeric Params: cloning a knot cannot hide nested allocations.
    let x = match p {
        Param::Int(x) => *x as f64,
        Param::Real(x) => *x,
        _ => return None,
    };
    (x.is_finite() && x.abs() <= solidcraft_step::read::MAX_COORD).then_some(x)
}

fn face_carrier(ex: &Exchange, mut id: u64, cancel: &AtomicBool) -> Result<u64> {
    // The admission AST has already checked genealogy. Bound this independent
    // decoder-side lookup too; a cycle must never become an unbounded walk.
    for _ in 0..ex.entities.len() {
        check_cancel(cancel)?;
        let e = ex
            .get(id)
            .ok_or_else(|| invalid("missing decoder face genealogy"))?;
        if matches!(e.name(), "ADVANCED_FACE" | "FACE_SURFACE") {
            return e
                .params()
                .get(2)
                .and_then(Param::as_ref_id)
                .ok_or_else(|| invalid("missing decoder face carrier"));
        }
        if e.name() != "ORIENTED_FACE" {
            break;
        }
        id = e
            .params()
            .get(2)
            .and_then(Param::as_ref_id)
            .ok_or_else(|| invalid("missing decoder oriented face"))?;
    }
    Err(invalid("invalid decoder face genealogy"))
}

fn point(ex: &Exchange, p: &Param) -> Option<[f64; 3]> {
    let e = ex.get(p.as_ref_id()?)?;
    if e.name() != "CARTESIAN_POINT" {
        return None;
    }
    let c = e.params().get(1)?.as_list()?;
    if c.len() != 3 {
        return None;
    }
    Some([numeric(&c[0])?, numeric(&c[1])?, numeric(&c[2])?])
}

// Return complete net dimensions and exact endpoint-row identity in each axis.
// A declaration alone never grants the decoder a periodic representation.
fn closed_rows(
    ex: &Exchange,
    e: &Entity,
    base: &[Param],
    cancel: &AtomicBool,
) -> Result<Option<ClosedRowInventory>> {
    let Some(rows) = base.get(2).and_then(Param::as_list) else {
        return Ok(None);
    };
    let nu = rows.len();
    let nv = rows
        .first()
        .and_then(Param::as_list)
        .map_or(0, <[Param]>::len);
    if nu == 0
        || nv == 0
        || nu
            .checked_mul(nv)
            .is_none_or(|n| n > solidcraft_step::read::MAX_CTRL)
    {
        return Ok(None);
    }
    let weights = match e.record("RATIONAL_B_SPLINE_SURFACE") {
        Some(r) => match r.first().and_then(Param::as_list) {
            Some(w) if w.len() == nu => Some(w),
            _ => return Ok(None),
        },
        None => None,
    };
    let Some(first) = rows[0].as_list().filter(|r| r.len() == nv) else {
        return Ok(None);
    };
    let mut equal = [true; 2];
    for (i, row) in rows.iter().enumerate() {
        check_cancel(cancel)?;
        let Some(row) = row.as_list().filter(|r| r.len() == nv) else {
            return Ok(None);
        };
        let wr = if let Some(w) = weights {
            match w[i].as_list().filter(|r| r.len() == nv) {
                Some(w) => Some(w),
                None => return Ok(None),
            }
        } else {
            None
        };
        for (j, p) in row.iter().enumerate() {
            check_cancel(cancel)?;
            let Some(p) = point(ex, p) else {
                return Ok(None);
            };
            if i == nu - 1 {
                equal[0] &= point(ex, &first[j]) == Some(p);
            }
            if j == nv - 1 {
                equal[1] &= point(ex, &row[0]) == Some(p);
            }
            if let Some(wr) = wr {
                let Some(w) = numeric(&wr[j]).filter(|w| *w > 0.0) else {
                    return Ok(None);
                };
                if i == nu - 1 {
                    equal[0] &= weights
                        .and_then(|w| w[0].as_list())
                        .and_then(|w| numeric(&w[j]))
                        == Some(w);
                }
                if j == nv - 1 {
                    equal[1] &= numeric(&wr[0]) == Some(w);
                }
            }
        }
    }
    Ok(Some(([nu, nv], equal, weights.is_some())))
}

/// Exact basis identity on the CLOSED active domain [U_p,U_n]. With
/// multiplicity p at each active end, the only Cox-de Boor recurrence involving
/// U_0 multiplies N_(0,p-1), whose support ends at U_p. It vanishes in the active
/// interior. The term involving U_(n+p) similarly has support starting at U_n.
/// Replacing ONLY those exterior slots therefore changes no degree-p basis
/// function in (U_p,U_n). Multiplicity p (p>=1) gives the same one-sided endpoint
/// limits P_0 and P_(n-1) as clamped evaluation, extending identity to both ends.
/// This applies coefficient-wise to polynomial and homogeneous tensor nets;
/// positive weights retain the denominator. Equal complete endpoint rows prove
/// closure separately. Lesser multiplicities and exterior laws are not covered.
pub(super) fn same_active_basis_with_inactive_end_slots(
    original: &[f64],
    clamped: &[f64],
    degree: usize,
    count: usize,
) -> bool {
    if degree == 0
        || degree > solidcraft_step::read::MAX_DEGREE
        || count <= degree
        || count.checked_add(degree).and_then(|n| n.checked_add(1)) != Some(original.len())
        || original.len() != clamped.len()
        || original
            .iter()
            .any(|x| !x.is_finite() || x.abs() > solidcraft_step::read::MAX_COORD)
        || original.windows(2).any(|w| w[0] > w[1])
    {
        return false;
    }
    let last = original.len() - 1;
    let (a, b) = (original[degree], original[count]);
    original[0] < a
        && a < b
        && b < original[last]
        && original[1..=degree].iter().all(|x| *x == a)
        && original[degree + 1] > a
        && original[count - 1] < b
        && original[count..last].iter().all(|x| *x == b)
        && clamped[0] == a
        && clamped[last] == b
        && original[1..last] == clamped[1..last]
}

fn axis(
    carrier: u64,
    e: &Entity,
    direction: usize,
    controls: [usize; 2],
    rational: bool,
    bytes: &mut usize,
    limit: usize,
    cancel: &AtomicBool,
) -> Result<Option<Axis>> {
    let (record, offset) = if e.is_complex() {
        let Some(i) = e
            .records
            .iter()
            .position(|r| r.name == "B_SPLINE_SURFACE_WITH_KNOTS")
        else {
            return Ok(None);
        };
        (i, 0)
    } else if e.name() == "B_SPLINE_SURFACE_WITH_KNOTS" {
        (0, 8)
    } else {
        return Ok(None);
    };
    let base = if e.is_complex() {
        e.record("B_SPLINE_SURFACE")
    } else {
        e.params().get(1..8)
    };
    let Some(base) = base else {
        return Ok(None);
    };
    let Some(p) = base
        .get(direction)
        .and_then(Param::as_i64)
        .filter(|p| (1..=solidcraft_step::read::MAX_DEGREE as i64).contains(p))
        .map(|p| p as usize)
    else {
        return Ok(None);
    };
    let params = &e.records[record].params;
    let slot = offset + direction;
    let Some(m) = params.get(slot).and_then(Param::as_list) else {
        return Ok(None);
    };
    let Some(k) = params.get(slot + 2).and_then(Param::as_list) else {
        return Ok(None);
    };
    if m.len() != k.len() || k.len() < 4 || controls[direction] <= p {
        return Ok(None);
    }
    let last = k.len() - 1;
    let (Some(start), Some(end), Some(left), Some(right)) = (
        numeric(&k[1]),
        numeric(&k[last - 1]),
        numeric(&k[0]),
        numeric(&k[last]),
    ) else {
        return Ok(None);
    };
    if !(left < start && start < end && end < right) {
        return Ok(None);
    }
    let mut total = 0usize;
    let mut previous = None;
    let mut run = 0usize;
    let mut endpoint_multiplicities = [0usize; 2];
    for (mult, knot) in m.iter().zip(k) {
        check_cancel(cancel)?;
        let Some(mult) = mult.as_i64().filter(|n| *n > 0 && *n <= p as i64 + 1) else {
            return Ok(None);
        };
        let Some(knot) = numeric(knot) else {
            return Ok(None);
        };
        if previous.is_some_and(|a| a > knot) {
            return Ok(None);
        }
        // STEP permits multiple entries for one knot. Qualify multiplicities
        // of complete equal runs, just as the decoder combines them.
        run = if previous == Some(knot) {
            run + mult as usize
        } else {
            mult as usize
        };
        if run > p {
            return Ok(None);
        }
        if knot == start {
            endpoint_multiplicities[0] += mult as usize;
        }
        if knot == end {
            endpoint_multiplicities[1] += mult as usize;
        }
        previous = Some(knot);
        let Some(sum) = total.checked_add(mult as usize) else {
            return Ok(None);
        };
        total = sum;
    }
    if total != controls[direction] + p + 1
        || m[0].as_i64() != Some(1)
        || m[last].as_i64() != Some(1)
        || endpoint_multiplicities != [p, p]
    {
        return Ok(None);
    }

    // The expanded-vector theorem above is shared with native construction:
    // neither temporary decoder clamping nor canonical IGES clamping may
    // change the complete closed active-domain parameter law.
    let extra = total
        .checked_mul(2 * size_of::<f64>())
        .and_then(|n| {
            k.len()
                .checked_mul(4 * size_of::<Param>())
                .and_then(|m| n.checked_add(m))
        })
        .ok_or_else(|| {
            error(
                GeometryErrorCode::ResourceLimit,
                "closed spline decode size overflow",
            )
        })?;
    charge(bytes, extra, limit)?;
    let mut original = Vec::with_capacity(total);
    for (mult, knot) in m.iter().zip(k) {
        check_cancel(cancel)?;
        original.extend(std::iter::repeat_n(
            numeric(knot).expect("qualified numeric knot"),
            mult.as_i64().expect("qualified multiplicity") as usize,
        ));
    }
    let mut decoded = original.clone();
    decoded[0] = original[p];
    decoded[total - 1] = original[controls[direction]];
    if !same_active_basis_with_inactive_end_slots(&original, &decoded, p, controls[direction]) {
        return Err(invalid("closed spline decode changed its active basis"));
    }
    let mut multiplicities = m[1..last].to_vec();
    multiplicities[0] = Param::Int(m[1].as_i64().expect("qualified multiplicity") + 1);
    multiplicities[last - 2] =
        Param::Int(m[last - 1].as_i64().expect("qualified multiplicity") + 1);
    Ok(Some(Axis {
        carrier,
        record,
        slot,
        direction,
        controls,
        rational,
        multiplicities: Param::List(multiplicities),
        knots: Param::List(k[1..last].to_vec()),
        original,
        decoded,
    }))
}

fn swap_exchange(ex: &mut Exchange, axes: &mut [Axis]) {
    // All slots were qualified before mutation. read_solid only borrows ex;
    // there are no fallible steps between this swap, decoding and its inverse.
    for a in axes {
        let params = &mut ex
            .entities
            .get_mut(&a.carrier)
            .expect("qualified carrier")
            .records[a.record]
            .params;
        std::mem::swap(&mut params[a.slot], &mut a.multiplicities);
        std::mem::swap(&mut params[a.slot + 2], &mut a.knots);
    }
}

pub(super) fn read_solid(
    ex: &mut Exchange,
    solid_id: u64,
    context: solidcraft_step::units::Context,
    byte_limit: usize,
    cancel: &AtomicBool,
) -> Result<solidcraft_step::topology::Solid> {
    let root = ex
        .get(solid_id)
        .filter(|e| e.name() == "MANIFOLD_SOLID_BREP")
        .ok_or_else(|| invalid("missing decoder solid root"))?;
    let shell = root
        .params()
        .get(1)
        .and_then(Param::as_ref_id)
        .and_then(|id| ex.get(id))
        .filter(|e| e.name() == "CLOSED_SHELL")
        .ok_or_else(|| invalid("missing decoder closed shell"))?;
    let faces = shell
        .params()
        .get(1)
        .and_then(Param::as_list)
        .ok_or_else(|| invalid("missing decoder shell faces"))?;
    if faces.len() > MAX_NATIVE_FACES as usize {
        return Err(error(
            GeometryErrorCode::ResourceLimit,
            "native face cap exceeded",
        ));
    }
    let byte_limit = byte_limit.min(super::MAX_QUERY_PROOF_BYTES);
    let mut bytes = 0usize;
    let mut axes = Vec::new();
    for face in faces {
        check_cancel(cancel)?;
        let carrier = face_carrier(
            ex,
            face.as_ref_id()
                .ok_or_else(|| invalid("decoder face reference"))?,
            cancel,
        )?;
        if axes.iter().any(|a: &Axis| a.carrier == carrier) {
            continue;
        }
        let e = ex
            .get(carrier)
            .ok_or_else(|| invalid("missing decoder carrier"))?;
        let base = if e.is_complex() {
            e.record("B_SPLINE_SURFACE")
        } else if e.name() == "B_SPLINE_SURFACE_WITH_KNOTS" {
            e.params().get(1..8)
        } else {
            None
        };
        let Some(base) = base else {
            continue;
        };
        if !(0..2).any(|d| base.get(4 + d).and_then(Param::as_bool) == Some(true)) {
            continue;
        }
        let Some((controls, equal, rational)) = closed_rows(ex, e, base, cancel)? else {
            continue;
        };
        for d in 0..2 {
            if equal[d]
                && base.get(4 + d).and_then(Param::as_bool) == Some(true)
                && let Some(a) = axis(
                    carrier, e, d, controls, rational, &mut bytes, byte_limit, cancel,
                )?
            {
                if axes.len() == axes.capacity() {
                    let additional = axes.capacity().max(1);
                    let growth = additional.checked_mul(size_of::<Axis>()).ok_or_else(|| {
                        error(
                            GeometryErrorCode::ResourceLimit,
                            "closed spline decode metadata size overflow",
                        )
                    })?;
                    charge(&mut bytes, growth, byte_limit)?;
                    axes.reserve_exact(additional);
                }
                axes.push(a);
            }
        }
    }
    check_cancel(cancel)?;
    swap_exchange(ex, &mut axes);
    let decoded = solidcraft_step::topology::read_solid(ex, solid_id, context);
    swap_exchange(ex, &mut axes); // Restore even on decoder failure, before mapping it.
    let mut solid = decoded.map_err(|cause| {
        invalid(&format!(
            "normalized STEP topology extraction failed: {cause:?}"
        ))
    })?;
    if solid.face_origin.len() != solid.brep.faces.len() {
        return Err(invalid("decoder spline face inventory mismatch"));
    }
    for a in &axes {
        let mut found = false;
        for (&origin, face) in solid.face_origin.iter().zip(&mut solid.brep.faces) {
            check_cancel(cancel)?;
            if face_carrier(ex, origin, cancel)? != a.carrier {
                continue;
            }
            found = true;
            // Restore only the exact carrier/type/net/vector association that
            // was supplied to the decoder. Never guess a replacement surface.
            let (u, v, controls, rational) = match &mut face.surface {
                Surface::Spline {
                    u_knots,
                    v_knots,
                    points,
                } => (
                    u_knots,
                    v_knots,
                    [points.len(), points.first().map_or(0, Vec::len)],
                    false,
                ),
                Surface::RationalSpline {
                    u_knots,
                    v_knots,
                    homogeneous,
                } => (
                    u_knots,
                    v_knots,
                    [homogeneous.len(), homogeneous.first().map_or(0, Vec::len)],
                    true,
                ),
                _ => return Err(invalid("decoder changed original spline carrier type")),
            };
            let knots = if a.direction == 0 { u } else { v };
            if face.nurbs.is_some()
                || controls != a.controls
                || rational != a.rational
                || *knots != a.decoded
            {
                return Err(invalid(
                    "decoder changed original spline carrier association",
                ));
            }
            knots.clone_from(&a.original);
        }
        if !found {
            return Err(invalid("decoder lost original spline face carrier"));
        }
    }
    Ok(solid)
}

// Control dimensions, exact closed-row flags and rationality.
type ClosedRowInventory = ([usize; 2], [bool; 2], bool);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::test_source;
    use sha2::{Digest, Sha256};
    use spiling_contracts::geometry::{DefinitionId, SourceHash};

    #[test]
    fn authored_rational_seams_import_without_changing_source_identity_or_units() {
        for (source, count) in [
            (test_source::nurbs_cylinder(), 3),
            (test_source::protected_bore(true), 7),
        ] {
            let bytes = test_source::original_step_bytes(source);
            let cancel = AtomicBool::new(false);
            let mut exchange = super::super::brep_data_bridge::parse_step_exchange(
                std::str::from_utf8(&bytes).unwrap(),
            )
            .unwrap();
            let root = *exchange
                .entities
                .iter()
                .find(|(_, entity)| entity.name() == "MANIFOLD_SOLID_BREP")
                .unwrap()
                .0;
            let original = exchange.clone();
            let solid = read_solid(
                &mut exchange,
                root,
                solidcraft_step::units::Context::default(),
                super::super::MAX_QUERY_PROOF_BYTES,
                &cancel,
            )
            .unwrap();
            assert_eq!(exchange.entities, original.entities);
            assert_eq!(solid.face_origin.len(), count);
            assert_eq!(solid.brep.faces.len(), count);
            let declared_seams = exchange
                .entities
                .values()
                .filter(|entity| entity.name() == "SEAM_CURVE")
                .count();
            let mut native_seams = 0;
            for edge in &solid.brep.edges {
                let [first, second] = edge.uses.as_slice() else {
                    continue;
                };
                if first.face != second.face || first.r#loop != second.r#loop {
                    continue;
                }
                let solidcraft_geom::brep::Loop::Edges { edges } =
                    &solid.brep.faces[first.face as usize].loops[first.r#loop as usize]
                else {
                    panic!("seam uses must belong to an edge loop")
                };
                let first = &edges[first.position as usize];
                let second = &edges[second.position as usize];
                assert_ne!(first.forward, second.forward);
                assert_ne!(first.pcurve, second.pcurve);
                native_seams += 1;
            }
            assert_eq!(declared_seams, 1);
            assert_eq!(native_seams, declared_seams, "no inserted seam edge");
            solid.brep.validate().unwrap();
            solid.brep.check_geometry().unwrap();

            let definition = crate::from_step_bytes_with_cancel(&bytes, &cancel).unwrap();
            let digest: [u8; 32] = Sha256::digest(&bytes).into();
            assert_eq!(definition.id, DefinitionId::from_source_sha256(&digest));
            assert_eq!(
                definition.provenance.source_hash,
                SourceHash::from_bytes(&bytes)
            );
            assert_eq!(definition.source_units_scale_to_mm, 1.0);
            assert_eq!(definition.faces.len(), count);
            for (info, &face) in definition.faces.iter().zip(&definition.face_ids) {
                let captured = definition.original_source.face(face).unwrap();
                assert_eq!(captured.source_face.0, info.face_id.step_entity());
                assert_eq!(captured.face_entity.0.to_string(), info.surface_face_entity);
            }
        }
    }
}
