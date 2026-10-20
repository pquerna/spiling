// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Definition-local finite selectors admitted against the finished occurrence enclosure.
use super::{
    classification::{self, ClassifiedRegions, InteriorRegion},
    region,
};
use super::{forward, inverse, rotate, surface};
use crate::{CompilerInput, error};
use geo::{Area, BoundingRect, MultiPolygon, Rect};
use spiling_contracts::{
    geometry::{AabbMm, OccurrenceId},
    manufacturing::*,
    process::{
        ProcessInterfacePermission, ProcessProvenance, ProcessSelector, ResolvedProcessValue,
    },
    surface::SurfaceScope,
};
use std::sync::atomic::AtomicBool;

mod partition;
pub(super) mod diaphragm;

fn owner_points(owner: &InteriorRegion) -> usize {
    region::point_count(&owner.domain)
        + (owner.coverage.contributions.capacity()
            * std::mem::size_of::<ProcessRegionContribution>()
            + owner
                .coverage
                .contributions
                .iter()
                .map(|c| c.definition_id.allocated_bytes())
                .sum::<usize>())
        .div_ceil(std::mem::size_of::<geo::Coord<f64>>())
}

#[derive(Clone, Copy)]
pub(super) struct Diaphragm {
    pub source_plane_mm: f64,
    pub thickness_mm: f64,
    pub physical_z_mm: [f64; 2],
}

#[derive(Clone, Copy)]
pub(super) struct Zone {
    pub axis: usize,
    pub coordinate: f64,
    pub frame_origin_mm: [f64; 2],
    pub values: [ResolvedProcessValue; 2],
    pub provenance: ProcessProvenance,
    pub authorized: bool,
    pub diaphragm: Option<Diaphragm>,
}
impl Zone {
    pub(super) fn z_events(self) -> [f64; 3] {
        self.diaphragm.map_or([self.coordinate; 3], |plate| [
            plate.physical_z_mm[0], self.coordinate, plate.physical_z_mm[1],
        ])
    }
    pub(super) fn is_z_invariant(self, lower: f64, upper: f64) -> bool {
        self.axis != 2 || self.z_events().into_iter().all(|event| event <= lower || event >= upper)
    }
}

pub(super) struct Capture {
    pub occurrence_id: OccurrenceId,
    pub zone: Option<Zone>,
    pub operations: Vec<ProcessProvenance>,
    pub applied: bool,
}

#[derive(Clone, Copy)]
struct ActiveBox {
    provenance: ProcessProvenance,
    min: [f64; 3],
    max: [f64; 3],
    permission: Option<ProcessInterfacePermission>,
    active: bool,
}

fn effective_boxes(
    input: &CompilerInput<'_>,
    occurrence: &spiling_contracts::geometry::OccurrenceRecord,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Vec<ActiveBox>, ManufacturingError> {
    let matches = |scope: &SurfaceScope| match scope {
        SurfaceScope::Definition { definition_id } => definition_id == &occurrence.definition_id,
        SurfaceScope::Occurrence {
            occurrence_id,
            expected_definition_id,
        } => {
            *occurrence_id == occurrence.occurrence_id
                && expected_definition_id == &occurrence.definition_id
        }
    };
    let bound = input
        .process_document
        .layers
        .iter()
        .filter(|l| l.enabled && matches(&l.scope))
        .flat_map(|l| &l.operations)
        .filter(|o| matches!(o.selector, ProcessSelector::SourceBoxMm { .. }))
        .count();
    if bound == 0 {
        return Ok(Vec::new());
    }
    super::boolean::charge(
        work,
        bound * 6 * (usize::BITS - (2 * bound).leading_zeros()) as usize,
    )?;
    let mut candidates = Vec::new();
    let mut cuts: [Vec<f64>; 3] = std::array::from_fn(|_| Vec::new());
    candidates.try_reserve_exact(bound).map_err(|_| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "box arrangement metadata allocation refused",
        )
    })?;
    for cut in &mut cuts {
        cut.try_reserve_exact(2 * bound).map_err(|_| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "box arrangement cut allocation refused",
            )
        })?;
    }
    for layer in input
        .process_document
        .layers
        .iter()
        .filter(|l| l.enabled && matches(&l.scope))
    {
        for operation in &layer.operations {
            if let ProcessSelector::SourceBoxMm { min, max } = operation.selector {
                for axis in 0..3 {
                    cuts[axis].extend([min[axis], max[axis]]);
                }
                candidates.push(ActiveBox {
                    provenance: ProcessProvenance {
                        layer_id: layer.layer_id,
                        operation_id: operation.operation_id,
                    },
                    min,
                    max,
                    permission: operation.interface,
                    active: false,
                });
            }
        }
    }
    if candidates.is_empty() {
        return Ok(candidates);
    }
    for cut in &mut cuts {
        cut.sort_unstable_by(f64::total_cmp);
        cut.dedup();
    }
    let cells = cuts
        .iter()
        .try_fold(
            input
                .process_document
                .layers
                .iter()
                .map(|l| l.operations.len())
                .sum::<usize>()
                .max(1),
            |n, axis| n.checked_mul(axis.len().saturating_sub(1)),
        )
        .ok_or_else(|| invalid("process field arrangement work overflow"))?;
    super::boolean::charge(work, cells)?;
    // Half-open Cartesian cells cover every finite selector exactly. Sampling
    // their lower corners is exact for an axis-aligned piecewise-constant field;
    // no geometry/bounds heuristic or native query discovers precedence.
    for x in cuts[0].windows(2) {
        for y in cuts[1].windows(2) {
            for z in cuts[2].windows(2) {
                crate::check_cancel(cancel)?;
                let value = input.process_document.resolve_at(
                    &occurrence.definition_id,
                    occurrence.occurrence_id,
                    input.intent.recipe.infill_fraction,
                    [x[0], y[0], z[0]],
                );
                if let Some(boxed) = value
                    .provenance
                    .and_then(|p| candidates.iter_mut().find(|b| b.provenance == p))
                {
                    boxed.active = true;
                }
            }
        }
    }
    candidates.retain(|boxed| boxed.active);
    Ok(candidates)
}

pub(super) fn capture(
    input: &CompilerInput<'_>,
    admission: &super::preflight::Admission,
    occurrence_id: OccurrenceId,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Capture, ManufacturingError> {
    let occurrence = input
        .occurrences
        .iter()
        .find(|o| o.occurrence_id == occurrence_id)
        .ok_or_else(|| invalid("zone lost its captured occurrence"))?;
    let mut boxes = effective_boxes(input, occurrence, work, cancel)?;
    let operations = boxes.iter().map(|b| b.provenance).collect();
    let empty = |operations| Capture {
        occurrence_id,
        zone: None,
        operations,
        applied: false,
    };
    if boxes.is_empty() {
        return Ok(empty(operations));
    }
    let native = admission.native_bounds[&occurrence_id];
    let finished = admission
        .surface_fields
        .get(&occurrence_id)
        .map_or(native, |fields| surface::expanded(native, fields));
    // Every corner of the conservative final-D world enclosure is transported;
    // the source enclosure alone would miss authorized outward relief.
    let mut local = AabbMm {
        min: [f64::INFINITY; 3],
        max: [f64::NEG_INFINITY; 3],
    };
    for bits in 0..8 {
        let world = std::array::from_fn(|a| {
            if bits & (1 << a) == 0 {
                finished.min[a]
            } else {
                finished.max[a]
            }
        });
        let point = inverse(&occurrence.pose, world);
        let scale = world
            .into_iter()
            .chain(occurrence.pose.translation_mm)
            .fold(1_f64, |s, v| s.max(v.abs()));
        let arithmetic = scale * 64. * f64::EPSILON;
        for a in 0..3 {
            local.min[a] = local.min[a].min((point[a] - arithmetic).next_down());
            local.max[a] = local.max[a].max((point[a] + arithmetic).next_up());
        }
    }
    boxes.retain(|b| !(0..3).any(|a| b.max[a] <= local.min[a] || b.min[a] >= local.max[a]));
    if boxes.is_empty() {
        return Ok(empty(operations));
    }
    // An interface parallel to a current hatch axis and a local-Z upright pose
    // requires an exact signed axis permutation. Approximate alignment is not authority.
    let basis = [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]].map(|p| rotate(&occurrence.pose, p));
    if basis[2][0] != 0.
        || basis[2][1] != 0.
        || basis[2][2].abs() != 1.
        || basis[..2].iter().any(|p| {
            p[2] != 0. || !((p[0].abs() == 1. && p[1] == 0.) || (p[1].abs() == 1. && p[0] == 0.))
        })
    {
        return Err(invalid(
            "SourceBoxMm requires local Z parallel to build Z and its separator aligned to a build hatch axis",
        ));
    }
    let mut selected = None;
    for boxed in boxes {
        let ActiveBox {
            provenance,
            min,
            max,
            permission,
            ..
        } = boxed;
        if (0..3).any(|a| max[a] <= local.min[a] || min[a] >= local.max[a]) {
            continue;
        }
        let mut cut = None;
        for a in 0..3 {
            for face in [min[a], max[a]] {
                if face >= local.min[a] && face <= local.max[a] {
                    if cut.replace((a, face)).is_some() {
                        return Err(invalid(
                            "SourceBoxMm has an extra face intersecting the conservative finished-D enclosure; exactly one X/Y/Z interface is admitted",
                        ));
                    }
                }
            }
        }
        let Some((local_axis, plane)) = cut else {
            return Err(invalid(
                "SourceBoxMm must expose exactly one internal X/Y/Z boundary",
            ));
        };
        let axis = if local_axis == 2 { 2 } else { usize::from(basis[local_axis][1] != 0.) };
        let mut point = std::array::from_fn(|a| local.min[a] + (local.max[a] - local.min[a]) * 0.5);
        point[local_axis] = plane;
        let coordinate = forward(&occurrence.pose, point)[axis];
        if selected.is_some_and(|prior: (usize, f64, ProcessProvenance, Option<ProcessInterfacePermission>, f64)| {
            prior.0 != axis || prior.1 != coordinate
        }) {
            return Err(invalid(
                "multiple distinct box interfaces are not supported in one occurrence",
            ));
        }
        selected = Some((axis, coordinate, provenance, permission, plane));
    }
    let Some((axis, coordinate, _, _, source_plane)) = selected else {
        return Ok(empty(operations));
    };
    let values = [0, 1].map(|side| {
        let mut world =
            std::array::from_fn(|a| finished.min[a] + (finished.max[a] - finished.min[a]) * 0.5);
        world[axis] = if side == 0 {
            finished.min[axis] + (coordinate - finished.min[axis]) * 0.5
        } else {
            coordinate + (finished.max[axis] - coordinate) * 0.5
        };
        input.process_document.resolve_at(
            &occurrence.definition_id,
            occurrence_id,
            input.intent.recipe.infill_fraction,
            inverse(&occurrence.pose, world),
        )
    });
    let winners = values.map(|value| {
        value.provenance.and_then(|p| {
            input
                .process_document
                .layers
                .iter()
                .find(|l| l.layer_id == p.layer_id)
                .and_then(|l| {
                    l.operations
                        .iter()
                        .find(|o| o.operation_id == p.operation_id)
                })
                .and_then(|o| {
                    matches!(o.selector, ProcessSelector::SourceBoxMm { .. }).then_some((
                        p,
                        o.interface,
                    ))
                })
        })
    });
    let Some((provenance, _)) = winners.iter().flatten().next().copied() else {
        return Ok(empty(operations));
    };
    if winners.iter().flatten().any(|(p, _)| *p != provenance) {
        return Err(invalid(
            "multiple effective box contributors require unsupported interface composition",
        ));
    }
    let permission = winners.iter().flatten().next().and_then(|(_, permission)| *permission);
    let diaphragm = match (axis, permission) {
        (2, Some(ProcessInterfacePermission::HorizontalDiaphragm { thickness_mm })) => {
            let endpoints = [source_plane - thickness_mm * 0.5, source_plane + thickness_mm * 0.5]
                .map(|z| forward(&occurrence.pose, [0., 0., z])[2]);
            let physical_z_mm = [endpoints[0].min(endpoints[1]), endpoints[0].max(endpoints[1])];
            if !physical_z_mm.iter().all(|z| z.is_finite()) || physical_z_mm[0] >= physical_z_mm[1] {
                return Err(invalid("UnsupportedInterface: diaphragm endpoints are not distinct finite world heights"));
            }
            Some(Diaphragm { source_plane_mm: source_plane, thickness_mm, physical_z_mm })
        }
        (2, None) if values[0].fraction == values[1].fraction => None,
        (2, _) => return Err(invalid("UnsupportedInterface: Z density boundary requires explicit HorizontalDiaphragm thickness permission")),
        (_, Some(ProcessInterfacePermission::HorizontalDiaphragm { .. })) => {
            return Err(invalid("UnsupportedInterface: horizontal diaphragm permission requires a source-Z interface"));
        }
        _ => None,
    };
    let authorized = permission == Some(ProcessInterfacePermission::SeparatorWall);
    Ok(Capture {
        occurrence_id,
        zone: Some(Zone {
            axis,
            coordinate,
            frame_origin_mm: [
                occurrence.pose.translation_mm[0],
                occurrence.pose.translation_mm[1],
            ],
            values,
            provenance,
            authorized,
            diaphragm,
        }),
        operations,
        applied: false,
    })
}

fn invalid(message: &str) -> ManufacturingError {
    error(ManufacturingErrorCode::InvalidSpecification, message)
}

/// Reserve one open rib band, then label only the residual eligible interior.
/// Every complementary cut is checked against its ORIGINAL owner, not an area epsilon.
pub(super) fn partition(
    mut classified: ClassifiedRegions,
    domains: &[super::ProcessDomain],
    recipe: &PlanarPrintRecipe,
    mut admit: impl FnMut(usize) -> Result<(), ManufacturingError>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<ClassifiedRegions, ManufacturingError> {
    for source in domains {
        let Some(zone) = source.zone else {
            continue;
        };
        if zone.axis == 2 {
            diaphragm::partition(&mut classified, source, zone, &mut admit, work, cancel)?;
            continue;
        }
        if zone.values[0].fraction == zone.values[1].fraction {
            for owner in &classified.interiors {
                admit(region::point_count(&owner.domain).saturating_mul(4))?;
                let eligible = classification::intersection_accumulated(
                    &owner.domain,
                    &source.material,
                    work,
                    cancel,
                )?;
                let Some(bounds) = eligible.bounding_rect() else {
                    continue;
                };
                let mut min = [bounds.min().x, bounds.min().y];
                let mut max = [bounds.max().x, bounds.max().y];
                let inside = zone
                    .values
                    .iter()
                    .position(|v| v.provenance == Some(zone.provenance))
                    .ok_or_else(|| invalid("effective zone lost winning source operation"))?;
                if inside == 0 {
                    max[zone.axis] = max[zone.axis].min(zone.coordinate);
                } else {
                    min[zone.axis] = min[zone.axis].max(zone.coordinate);
                }
                if min[zone.axis] >= max[zone.axis] {
                    continue;
                }
                let mask = MultiPolygon(vec![
                    Rect::new(
                        geo::Coord {
                            x: min[0],
                            y: min[1],
                        },
                        geo::Coord {
                            x: max[0],
                            y: max[1],
                        },
                    )
                    .to_polygon(),
                ]);
                if !classification::intersection_accumulated(&eligible, &mask, work, cancel)?
                    .0
                    .is_empty()
                {
                    classified.applied_operations.push((
                        source.coverage.contributions[0].occurrence_id,
                        zone.provenance,
                    ));
                    break;
                }
            }
            continue;
        }
        let old = std::mem::take(&mut classified.interiors);
        let old_records = old.capacity() * std::mem::size_of::<InteriorRegion>();
        let mut owners = old.into_iter();
        let mut next: Vec<InteriorRegion> = Vec::new();
        while let Some(owner) = owners.next() {
            crate::check_cancel(cancel)?;
            let original_points = region::point_count(&owner.domain);
            let other_points = classified.retained_points()
                + owners.as_slice().iter().map(owner_points).sum::<usize>()
                + next.iter().map(owner_points).sum::<usize>()
                + (old_records + next.capacity() * std::mem::size_of::<InteriorRegion>())
                    .div_ceil(std::mem::size_of::<geo::Coord<f64>>());
            admit(
                other_points
                    .checked_add(
                        original_points
                            .checked_mul(8)
                            .ok_or_else(|| invalid("zone point bound overflow"))?,
                    )
                    .ok_or_else(|| invalid("zone live point bound overflow"))?,
            )?;
            let (eligible, outside, _) =
                classification::split_process_region(owner.domain, &source.material, work, cancel)?;
            if !outside.0.is_empty() {
                next.push(InteriorRegion {
                    coverage: ProcessRegionCoverage {
                        area_mm2: outside.unsigned_area(),
                        ..owner.coverage.clone()
                    },
                    domain: outside,
                    frame_origin_mm: owner.frame_origin_mm,
                });
            }
            if eligible.0.is_empty() {
                continue;
            }
            let bounds = eligible
                .bounding_rect()
                .ok_or_else(|| invalid("eligible zone has no bounds"))?;
            let c = zone.coordinate;
            let low = if zone.axis == 0 {
                bounds.min().x
            } else {
                bounds.min().y
            };
            let high = if zone.axis == 0 {
                bounds.max().x
            } else {
                bounds.max().y
            };
            // A boundary wholly in a global skin or outside this island owns no rib.
            if c <= low || c >= high {
                let side = usize::from(c <= low);
                let value = zone.values[side];
                if value.provenance == Some(zone.provenance) {
                    classified.applied_operations.push((
                        source.coverage.contributions[0].occurrence_id,
                        zone.provenance,
                    ));
                }
                let mut contributions = source.coverage.contributions.clone();
                for contribution in &mut contributions {
                    contribution.provenance = value.provenance;
                }
                next.push(InteriorRegion {
                    coverage: ProcessRegionCoverage {
                        area_mm2: eligible.unsigned_area(),
                        fraction: value.fraction,
                        contributions,
                    },
                    domain: eligible,
                    frame_origin_mm: Some(zone.frame_origin_mm),
                });
                continue;
            }
            if !zone.authorized {
                return Err(invalid(
                    "UnsupportedInterface: differing box density requires explicit permanent SeparatorWall permission",
                ));
            }
            let compatible = matches!(
                (recipe.rectilinear_axis, zone.axis),
                (RectilinearAxisPolicy::X, 0) | (RectilinearAxisPolicy::Y, 1)
            );
            if !compatible {
                return Err(invalid(
                    "UnsupportedInterface: box separator requires an explicitly fixed compatible rectilinear axis",
                ));
            }
            let half = recipe.bead_width_mm * 0.5;
            if c - low <= half || high - c <= half {
                return Err(invalid(
                    "UnrealizableRegion: selector leaves a positive residual strip narrower than the separator half-bead ownership",
                ));
            }
            // One graph transports original source-edge parameter intervals
            // through BOTH cuts. Independently quantized Boolean clips cannot
            // certify their reunion against an untouched original boundary.
            let borrowed_points = classified.retained_points()
                + owners.as_slice().iter().map(owner_points).sum::<usize>()
                + next.iter().map(owner_points).sum::<usize>()
                + region::point_count(&eligible)
                + (old_records
                    + next.capacity() * std::mem::size_of::<InteriorRegion>()
                    + owner.coverage.contributions.capacity()
                        * std::mem::size_of::<ProcessRegionContribution>()
                    + owner
                        .coverage
                        .contributions
                        .iter()
                        .map(|c| c.definition_id.allocated_bytes())
                        .sum::<usize>())
                .div_ceil(std::mem::size_of::<geo::Coord<f64>>());
            let pieces = partition::split(
                &eligible,
                zone.axis,
                [c - half, c + half],
                |scratch| {
                    admit(
                        borrowed_points
                            .checked_add(scratch)
                            .ok_or_else(|| invalid("zone partition live storage overflow"))?,
                    )
                },
                work,
                cancel,
            )?;
            let mut hits = Vec::new();
            let mut centerlines = Vec::new();
            let hatch_axis = if zone.axis == 0 {
                region::HatchAxis::Y
            } else {
                region::HatchAxis::X
            };
            for polygon in &eligible.0 {
                hits.clear();
                region::row_intersections(polygon, c, hatch_axis, &mut hits, cancel)?;
                for pair in hits.as_chunks::<2>().0 {
                    let mut a = [0.; 2];
                    let mut b = [0.; 2];
                    a[zone.axis] = c;
                    b[zone.axis] = c;
                    a[1 - zone.axis] = pair[0];
                    b[1 - zone.axis] = pair[1];
                    if pair[0] >= pair[1] {
                        return Err(invalid("UnrealizableRegion: zero separator centerline"));
                    }
                    centerlines.push([a, b]);
                }
            }
            if centerlines.is_empty() {
                return Err(invalid(
                    "UnrealizableRegion: positive separator ownership has no actual centerline",
                ));
            }
            let [left, band, right] = pieces;
            let contribution = &source.coverage.contributions[0];
            if next.len() + 2 > MAX_PROCESS_REGIONS {
                return Err(error(
                    ManufacturingErrorCode::ResourceLimit,
                    "regional owner limit exceeded",
                ));
            }
            let region_indices = [next.len() as u16, next.len() as u16 + 1];
            classified
                .coverage
                .process_interfaces
                .push(ProcessInterfaceCoverage {
                    occurrence_id: contribution.occurrence_id,
                    definition_id: contribution.definition_id.clone(),
                    provenance: zone.provenance,
                    region_indices,
                    axis: zone.axis as u8,
                    coordinate_mm: c,
                    thickness_mm: recipe.bead_width_mm,
                    bonding_overlap_mm: half,
                    area_mm2: band.unsigned_area(),
                    centerlines_mm: centerlines,
                    endpoint_bonds: Vec::new(),
                });
            classified
                .separator_bands
                .try_reserve_exact(1)
                .map_err(|_| invalid("separator band retention allocation refused"))?;
            classified.separator_bands.push(band);
            classified.applied_operations.push((
                source.coverage.contributions[0].occurrence_id,
                zone.provenance,
            ));
            for (side, piece) in [left, right].into_iter().enumerate() {
                if piece.0.is_empty() {
                    return Err(invalid(
                        "UnrealizableRegion: separator consumes a density zone",
                    ));
                }
                // A nonempty labelled island must admit a fixed-width bead, not
                // survive only because a scalar area threshold discarded its neck.
                for polygon in &piece.0 {
                    let r = polygon
                        .bounding_rect()
                        .ok_or_else(|| invalid("zone residual bounds"))?;
                    if r.width() <= recipe.bead_width_mm || r.height() <= recipe.bead_width_mm {
                        return Err(invalid(
                            "UnrealizableRegion: positive density sliver is narrower than one fixed bead",
                        ));
                    }
                }
                let mut contributions = source.coverage.contributions.clone();
                for contribution in &mut contributions {
                    contribution.provenance = zone.values[side].provenance;
                }
                next.push(InteriorRegion {
                    coverage: ProcessRegionCoverage {
                        area_mm2: piece.unsigned_area(),
                        fraction: zone.values[side].fraction,
                        contributions,
                    },
                    domain: piece,
                    frame_origin_mm: Some(zone.frame_origin_mm),
                });
            }
        }
        classified.interiors = next;
    }
    if !classified.separator_bands.is_empty() && domains.len() > 1 {
        for polygon in &classified.layout.fill.0 {
            let mut owned = false;
            for source in domains {
                let conditioned = std::iter::once(polygon)
                    .chain(&source.material.0)
                    .flat_map(|p| std::iter::once(p.exterior()).chain(p.interiors()))
                    .flat_map(|r| &r.0)
                    .all(|p| {
                        super::native::merge_predicate_coordinate(p.x)
                            && super::native::merge_predicate_coordinate(p.y)
                    });
                if conditioned
                    && classification::strictly_contained_process_polygon(
                        polygon,
                        &source.material,
                        work,
                        cancel,
                    )?
                {
                    owned = true;
                    break;
                }
            }
            if !owned {
                return Err(invalid(
                    "UnsupportedInterface: mandatory-skin bridge corridor has ambiguous cross-occurrence material ownership",
                ));
            }
        }
    }
    classified.coverage.process_regions = classified
        .interiors
        .iter()
        .map(|i| i.coverage.clone())
        .collect();
    classified.coverage.separator_area_mm2 = classified
        .coverage
        .process_interfaces
        .iter()
        .map(|i| i.area_mm2)
        .sum();
    classified.coverage.diaphragm_area_mm2 = classified.coverage.process_diaphragms.iter().map(|plate| plate.area_mm2).sum();
    classified.coverage.solid_interior_area_mm2 = classified
        .interiors
        .iter()
        .enumerate()
        .filter(|(index, i)| i.coverage.fraction == 1.
            && !classified.coverage.process_diaphragms.iter().any(|plate| usize::from(plate.region_index) == *index))
        .map(|(_, i)| i.coverage.area_mm2)
        .sum();
    classified.coverage.sparse_interior_area_mm2 = classified
        .interiors
        .iter()
        .filter(|i| i.coverage.fraction < 1.)
        .map(|i| i.coverage.area_mm2)
        .sum();
    classified.coverage.intentional_void_area_mm2 = classified
        .interiors
        .iter()
        .map(|i| i.coverage.area_mm2 * (1. - i.coverage.fraction))
        .sum();
    Ok(classified)
}
