// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Independent full-bead material checks after the emitted/path bijection.
use super::{cancelled, failed, project_xyz6_point_mm};
use geo::{
    Area, BooleanOps, Contains, Coord, Distance, Euclidean, Line, LineString, MultiPolygon, Point,
    Polygon,
};
use spiling_contracts::manufacturing::*;
use spiling_geometry::{GeometryWorkBudget, PlanarCapsuleClearancePolicy, SectionLoopMm};
use std::sync::atomic::AtomicBool;

fn ring(points: &[[f64; 2]]) -> LineString<f64> {
    LineString(points.iter().map(|p| Coord { x: p[0], y: p[1] }).collect())
}
fn material(domain: &ResolvedMaterialDomain) -> MultiPolygon<f64> {
    MultiPolygon(
        domain
            .polygons
            .iter()
            .map(|p| {
                Polygon::new(
                    ring(&p.exterior_mm),
                    p.holes_mm.iter().map(|r| ring(r)).collect(),
                )
            })
            .collect(),
    )
}
fn loops(domain: &ResolvedMaterialDomain) -> Vec<SectionLoopMm> {
    domain
        .polygons
        .iter()
        .flat_map(|p| {
            std::iter::once((false, &p.exterior_mm)).chain(p.holes_mm.iter().map(|r| (true, r)))
        })
        .map(|(is_hole, r)| SectionLoopMm {
            is_hole,
            points_mm: r.iter().map(|p| [p[0], p[1], 0.0]).collect(),
        })
        .collect()
}

pub(super) fn verify(
    plan: &NormalizedPrintPlan,
    charged: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    if plan.material_domains.is_empty() {
        return Ok(());
    }
    let remaining = 128_000_000usize
        .checked_sub(*charged)
        .ok_or_else(|| failed("material verification geometry budget exhausted"))?;
    let mut work = GeometryWorkBudget::new(remaining as u64);
    let outcome = (|| {
        let domains: Vec<_> = plan
            .material_domains
            .iter()
            .map(|d| (d, material(d), loops(d)))
            .collect();
        for (i, (a, am, _)) in domains.iter().enumerate() {
            for (b, bm, _) in &domains[..i] {
                cancelled(cancel)?;
                if a.physical_z_mm[0] >= b.physical_z_mm[1]
                    || b.physical_z_mm[0] >= a.physical_z_mm[1]
                {
                    continue;
                }
                let count = |d: &ResolvedMaterialDomain| {
                    d.polygons
                        .iter()
                        .map(|p| {
                            p.exterior_mm.len() + p.holes_mm.iter().map(Vec::len).sum::<usize>()
                        })
                        .sum::<usize>()
                };
                let pairs = count(a)
                    .checked_mul(count(b))
                    .ok_or_else(|| failed("material verification work overflow"))?;
                *charged = charged
                    .checked_add(pairs)
                    .filter(|v| *v <= 128_000_000)
                    .ok_or_else(|| {
                        ManufacturingError::new(
                            ManufacturingErrorCode::ResourceLimit,
                            "material verification work budget exceeded",
                        )
                    })?;
                if a.physical_z_mm[0] < b.physical_z_mm[1]
                    && b.physical_z_mm[0] < a.physical_z_mm[1]
                    && am.intersection(bm).unsigned_area() > 0.0
                {
                    return Err(failed("material domains overlap in positive volume"));
                }
            }
        }
        for layer in &plan.layers {
            for path in &layer.paths {
                cancelled(cancel)?;
                let reference = path
                    .material_domain
                    .ok_or_else(|| failed("constructed actual bead has no material domain"))?;
                let (domain, own, _) = domains
                    .iter()
                    .find(|(d, _, _)| {
                        d.reference == reference
                            && d.physical_z_mm[0] <= path.physical_z_mm[0]
                            && d.physical_z_mm[1] >= path.physical_z_mm[1]
                    })
                    .ok_or_else(|| failed("actual bead has no complete physical material owner"))?;
                for edge in path.checked_edges()? {
                    let points = [
                        project_xyz6_point_mm(edge.points_mm[0])?,
                        project_xyz6_point_mm(edge.points_mm[1])?,
                    ];
                    let xy = points.map(|p| [p[0], p[1]]);
                    let line = Line::new(Coord::from(xy[0]), Coord::from(xy[1]));
                    if !own.contains(&Point::from(line.start))
                        || !own.contains(&Point::from(line.end))
                    {
                        return Err(failed("actual bead leaves its assigned material domain"));
                    }
                    for boundary in own
                        .0
                        .iter()
                        .flat_map(|p| std::iter::once(p.exterior()).chain(p.interiors()))
                        .flat_map(|r| r.lines())
                    {
                        cancelled(cancel)?;
                        *charged = charged
                            .checked_add(1)
                            .filter(|v| *v <= 128_000_000)
                            .ok_or_else(|| {
                                ManufacturingError::new(
                                    ManufacturingErrorCode::ResourceLimit,
                                    "material verification boundary budget exceeded",
                                )
                            })?;
                        if Euclidean.distance(&line, &boundary) + plan.offset_tolerance_mm
                            < edge.width_mm * 0.5
                                + program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM
                        {
                            return Err(failed(
                                "full actual bead enters a protected void or leaves its material domain",
                            ));
                        }
                    }
                    // Native boundary approximation cannot authorize extrusion into
                    // another owned volume. Its complete filled material is forbidden.
                    for (other, _, forbidden) in &domains {
                        if other.reference == domain.reference
                            || other.physical_z_mm[0] >= path.physical_z_mm[1]
                            || other.physical_z_mm[1] <= path.physical_z_mm[0]
                        {
                            continue;
                        }
                        let endpoint_error = points.iter().try_fold(0.0f64, |bound, p| {
                            Ok::<_, ManufacturingError>(
                                bound.max(
                                    program_coordinates::xyz6_coordinate_error_mm(p[0])?
                                        .hypot(program_coordinates::xyz6_coordinate_error_mm(p[1])?)
                                        .next_up(),
                                ),
                            )
                        })?;
                        spiling_geometry::certify_planar_capsule_clearance(xy, forbidden, PlanarCapsuleClearancePolicy { radius_mm:edge.width_mm*0.5, required_gap_mm:0.0, endpoint_error_mm:endpoint_error }, &mut work,cancel)
                        .map_err(|e|ManufacturingError::new(match e.code { spiling_contracts::geometry::GeometryErrorCode::Cancelled=>ManufacturingErrorCode::Cancelled, spiling_contracts::geometry::GeometryErrorCode::ResourceLimit=>ManufacturingErrorCode::ResourceLimit, _=>ManufacturingErrorCode::VerificationFailed },format!("actual {:?} bead {xy:?} width={} intrudes into {:?} material: {e}",domain.reference.role,edge.width_mm,other.reference.role)))?;
                        if (*charged as u64).saturating_add(work.consumed()) > 128_000_000 {
                            return Err(ManufacturingError::new(
                                ManufacturingErrorCode::ResourceLimit,
                                "shared emitted-geometry work budget exceeded",
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    })();
    *charged += work.consumed() as usize;
    outcome
}
