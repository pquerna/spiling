// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;

pub(super) fn partition(
    classified: &mut ClassifiedRegions,
    source: &super::super::ProcessDomain,
    zone: Zone,
    admit: &mut impl FnMut(usize, usize) -> Result<(), ManufacturingError>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let interval = source.physical_z_mm;
    if !zone.is_z_invariant(interval[0], interval[1]) {
        return Err(invalid(
            "process span crosses an unresolved actual horizontal boundary",
        ));
    }
    let plate = zone.diaphragm.filter(|plate| {
        interval[0] >= plate.physical_z_mm[0] && interval[1] <= plate.physical_z_mm[1]
    });
    if plate.is_some() {
        for index in 0..2 {
            let skin = if index == 0 {
                &classified.bottom
            } else {
                &classified.top
            };
            let mut overlap =
                classification::intersection_accumulated(skin, &source.material, work, cancel)?;
            let units = classified.retained_points() + region::point_count(&overlap);
            let bytes = super::super::native::material_bytes(&mut overlap);
            admit(
                units,
                classification::byte_sum(classified.retained_bytes(), bytes)?,
            )?;
            if !overlap.0.is_empty() {
                return Err(invalid(
                    "UnsupportedInterface: complete diaphragm slab intersects mandatory exterior skin",
                ));
            }
        }
    }
    let side = usize::from(interval[0] >= zone.coordinate);
    let value = zone.values[side];
    for index in 0..classified.interiors.len() {
        let owner = &mut classified.interiors[index];
        crate::check_cancel(cancel)?;
        if !owner
            .coverage
            .contributions
            .iter()
            .any(|c| c.occurrence_id == source.coverage.contributions[0].occurrence_id)
        {
            continue;
        }
        if owner.coverage.contributions.len() != 1 {
            return Err(invalid(
                "UnsupportedInterface: diaphragm requires unambiguous original occurrence ownership",
            ));
        }
        owner.frame_origin_mm = Some(zone.frame_origin_mm);
        owner.coverage.fraction = if plate.is_some() { 1.0 } else { value.fraction };
        owner.coverage.contributions[0].provenance = if plate.is_some() {
            Some(zone.provenance)
        } else {
            value.provenance
        };
        if let Some(plate) = plate {
            let point_count = region::point_count(&owner.domain);
            let extent_bytes = owner.domain.0.len() * std::mem::size_of::<DiaphragmPolygonMm>()
                + owner
                    .domain
                    .0
                    .iter()
                    .map(|p| {
                        p.interiors().len() * std::mem::size_of::<Vec<[f64; 2]>>()
                            + (p.exterior().0.len()
                                + p.interiors().iter().map(|r| r.0.len()).sum::<usize>())
                                * std::mem::size_of::<[f64; 2]>()
                    })
                    .sum::<usize>();
            let extra = extent_bytes
                + owner.coverage.contributions[0]
                    .definition_id
                    .allocated_bytes()
                + std::mem::size_of::<ProcessDiaphragmCoverage>()
                + std::mem::size_of::<(OccurrenceId, ProcessProvenance)>();
            let units = classified.retained_points() + point_count;
            admit(
                units,
                classification::byte_sum(classified.retained_bytes(), extra)?,
            )?;
            classified.coverage.process_diaphragms.reserve_exact(1);
            classified.applied_operations.reserve_exact(1);
            let owner = &classified.interiors[index];
            let contribution = &owner.coverage.contributions[0];
            let extent = owner
                .domain
                .0
                .iter()
                .map(|polygon| DiaphragmPolygonMm {
                    outer: polygon.exterior().0.iter().map(|p| [p.x, p.y]).collect(),
                    holes: polygon
                        .interiors()
                        .iter()
                        .map(|ring| ring.0.iter().map(|p| [p.x, p.y]).collect())
                        .collect(),
                })
                .collect();
            classified
                .coverage
                .process_diaphragms
                .push(ProcessDiaphragmCoverage {
                    occurrence_id: contribution.occurrence_id,
                    definition_id: contribution.definition_id.clone(),
                    provenance: zone.provenance,
                    source_plane_mm: plate.source_plane_mm,
                    requested_thickness_mm: plate.thickness_mm,
                    physical_z_mm: plate.physical_z_mm,
                    neighbors: zone.values,
                    region_index: index as u16,
                    area_mm2: owner.coverage.area_mm2,
                    extent,
                });
        }
        if plate.is_some() || value.provenance == Some(zone.provenance) {
            let extra = std::mem::size_of::<(OccurrenceId, ProcessProvenance)>();
            admit(
                classified.retained_points(),
                classification::byte_sum(classified.retained_bytes(), extra)?,
            )?;
            classified.applied_operations.reserve_exact(1);
            classified.applied_operations.push((
                source.coverage.contributions[0].occurrence_id,
                zone.provenance,
            ));
        }
        admit(classified.retained_points(), classified.retained_bytes())?;
    }
    Ok(())
}

fn inward_xyz6(value: f64, lower: bool) -> Result<f64, ManufacturingError> {
    use spiling_contracts::manufacturing::program_coordinates::project_xyz6_coordinate_mm;
    project_xyz6_coordinate_mm(value)?;
    let scaled = value * 1_000_000.0;
    let integer = if lower { scaled.ceil() } else { scaled.floor() };
    let result = integer / 1_000_000.0;
    if !integer.is_finite()
        || integer.abs() > (1u64 << 52) as f64
        || project_xyz6_coordinate_mm(result)? != result
    {
        return Err(error(
            ManufacturingErrorCode::ProgramCoordinateConditioningFailure,
            "permanent plate endpoint has no checked inward XYZ6 construction",
        ));
    }
    Ok(result)
}

/// Preserve each selected stroke and its actual fixed row. Only along-axis
/// endpoints move inward on the output lattice; the resulting actual segment
/// is a subset of the already-clearance-checked actual command segment.
pub(in crate::planner) fn construct_coordinates(
    layers: &mut [PrintLayer],
    mut admit: impl FnMut(usize) -> Result<(), ManufacturingError>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    use geo::{Coord, Line, LineString, Polygon, Relate};
    use spiling_contracts::manufacturing::program_coordinates::project_xyz6_coordinate_mm;
    for first in 0..layers.len() {
        for plate_index in 0..layers[first].region_coverage.process_diaphragms.len() {
            let plate = &layers[first].region_coverage.process_diaphragms[plate_index];
            if layers[first].lower_z_mm != plate.physical_z_mm[0] {
                continue;
            }
            crate::check_cancel(cancel)?;
            let key = (
                plate.occurrence_id,
                plate.provenance,
                plate.physical_z_mm[1],
            );
            let points = plate
                .extent
                .iter()
                .map(|p| p.outer.len() + p.holes.iter().map(Vec::len).sum::<usize>())
                .sum::<usize>();
            admit(
                points
                    .checked_mul(8)
                    .ok_or_else(|| invalid("plate coordinate scratch overflow"))?,
            )?;
            let ring = |points: &Vec<[f64; 2]>| {
                LineString(points.iter().map(|p| Coord { x: p[0], y: p[1] }).collect())
            };
            let domain = MultiPolygon(
                plate
                    .extent
                    .iter()
                    .map(|p| Polygon::new(ring(&p.outer), p.holes.iter().map(ring).collect()))
                    .collect(),
            );
            let after = layers.partition_point(|layer| layer.lower_z_mm < key.2);
            let mut hits = Vec::new();
            hits.try_reserve_exact(points).map_err(|_| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "permanent plate row intersection scratch allocation refused",
                )
            })?;
            for layer in &mut layers[first..after] {
                let owner = layer
                    .region_coverage
                    .process_diaphragms
                    .iter()
                    .find(|plate| plate.occurrence_id == key.0 && plate.provenance == key.1)
                    .ok_or_else(|| {
                        invalid("permanent plate lost a complete physical construction interval")
                    })?
                    .region_index;
                for path in layer.paths.iter_mut().filter(|path| {
                    path.owner
                        == Some(DepositionOwner::Regional {
                            region_index: owner,
                        })
                }) {
                    crate::check_cancel(cancel)?;
                    if path.points_mm.len() != 2
                        || !matches!(
                            path.kind,
                            DepositionKind::SolidFill | DepositionKind::Bridge
                        )
                    {
                        return Err(invalid(
                            "permanent plate requires its original straight solid/Bridge strokes",
                        ));
                    }
                    let axis = if path.points_mm[0][1] == path.points_mm[1][1] {
                        region::HatchAxis::X
                    } else if path.points_mm[0][0] == path.points_mm[1][0] {
                        region::HatchAxis::Y
                    } else {
                        return Err(invalid(
                            "permanent plate stroke lost its authored rectilinear axis",
                        ));
                    };
                    let fixed = usize::from(axis == region::HatchAxis::X);
                    let along = 1 - fixed;
                    let coordinate = project_xyz6_coordinate_mm(path.points_mm[0][fixed])?;
                    let ends = [
                        project_xyz6_coordinate_mm(path.points_mm[0][along])?,
                        project_xyz6_coordinate_mm(path.points_mm[1][along])?,
                    ];
                    let original = [ends[0].min(ends[1]), ends[0].max(ends[1])];
                    let mut chosen = None;
                    for polygon in &domain.0 {
                        super::super::boolean::charge(
                            work,
                            points.saturating_mul(
                                (usize::BITS - points.max(1).leading_zeros()) as usize + 1,
                            ),
                        )?;
                        hits.clear();
                        region::row_intersections(polygon, coordinate, axis, &mut hits, cancel)?;
                        if hits.iter().any(|hit| !hit.is_finite()) {
                            return Err(error(
                                ManufacturingErrorCode::ProgramCoordinateConditioningFailure,
                                "permanent plate row intersection is not finite",
                            ));
                        }
                        for pair in hits.as_chunks::<2>().0 {
                            let lo = pair[0].max(original[0]);
                            let hi = pair[1].min(original[1]);
                            if hi <= lo {
                                continue;
                            }
                            if chosen.replace([lo, hi]).is_some() {
                                return Err(invalid(
                                    "UnrealizableRegion: actual plate row splits across positive owner intervals",
                                ));
                            }
                        }
                    }
                    let selected = chosen.ok_or_else(|| invalid(
                        "UnrealizableRegion: actual plate row loses its positive original owner"))?;
                    let interval = [
                        inward_xyz6(selected[0], true)?,
                        inward_xyz6(selected[1], false)?,
                    ];
                    if interval[1] - interval[0] <= 0.000002
                        || interval[0] < original[0]
                        || interval[1] > original[1]
                    {
                        return Err(invalid(
                            "UnrealizableRegion: inward XYZ6 plate row is lost or sub-printable",
                        ));
                    }
                    super::super::boolean::charge(
                        work,
                        points.saturating_mul(points.saturating_add(2)),
                    )?;
                    let endpoint = |value| {
                        let mut xy = [0.; 2];
                        xy[fixed] = coordinate;
                        xy[along] = value;
                        Coord { x: xy[0], y: xy[1] }
                    };
                    if !domain
                        .relate(&Line::new(endpoint(interval[0]), endpoint(interval[1])))
                        .is_covers()
                    {
                        return Err(invalid(
                            "UnsupportedInterface: constructed actual XYZ6 plate row is not wholly covered by its original raw owner",
                        ));
                    }
                    let ordered = if ends[0] <= ends[1] {
                        interval
                    } else {
                        [interval[1], interval[0]]
                    };
                    for (point, value) in path.points_mm.iter_mut().zip(ordered) {
                        point[fixed] = coordinate;
                        point[along] = value;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Bounded, schedule-only editing advice. These choices are not native material
/// or bridge admission, and never replace the requested thickness.
pub(in crate::planner) fn schedule_diagnostic(
    input: &CompilerInput<'_>,
    admission: &super::super::preflight::Admission,
    process: &[Capture],
    geometry_events: &[f64],
    mut failure: ManufacturingError,
    cancel: &AtomicBool,
) -> Result<ManufacturingError, ManufacturingError> {
    let capture = process
        .iter()
        .find(|capture| capture.zone.is_some_and(|zone| zone.diaphragm.is_some()))
        .expect("caller requires captured plate");
    let zone = capture.zone.expect("captured zone");
    let plate = zone.diaphragm.expect("captured plate");
    let occurrence = input
        .occurrences
        .iter()
        .find(|source| source.occurrence_id == capture.occurrence_id)
        .ok_or_else(|| invalid("plate schedule advice lost captured source"))?;
    let bed = input.intent.printer.build_envelope.min[2];
    let top = *admission
        .horizontal_events
        .last()
        .ok_or_else(|| invalid("plate schedule lost native top"))?;
    let recipe = &input.intent.recipe;
    let mut baseline = geometry_events.to_vec();
    baseline.extend(
        process
            .iter()
            .filter_map(|capture| capture.zone)
            .filter(|zone| zone.axis == 2)
            .map(|zone| zone.coordinate),
    );
    let baseline_schedule = match super::super::layer_schedule(
        bed,
        top,
        recipe.layer_height_mm,
        &baseline,
    ) {
        Ok(schedule) => schedule,
        Err(error) if error.code == ManufacturingErrorCode::UnsupportedGeometry => {
            failure.message = format!(
                "{}; no schedule-admissible plate choice established against existing native/field stops; explicit editing and Compile required",
                failure.message
            );
            return Ok(failure);
        }
        Err(error) => return Err(error),
    };
    let minimum = input
        .intent
        .printer
        .deposition_limits
        .min_bead_height_mm
        .max(2.0 * spiling_contracts::geometry::SECTION_PLANE_TOLERANCE_MM);
    let minimum_half = ((zone.coordinate + minimum).next_up() - zone.coordinate)
        .max(zone.coordinate - (zone.coordinate - minimum).next_down());
    let mut choices = Vec::new();
    choices
        .try_reserve_exact(2 * baseline_schedule.len() + 1)
        .map_err(|_| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "plate schedule advice allocation refused",
            )
        })?;
    choices.push(2.0 * minimum_half);
    for layer in &baseline_schedule {
        crate::check_cancel(cancel)?;
        for boundary in [layer.lower_z, layer.z] {
            let thickness = 2.0 * (boundary - zone.coordinate).abs();
            if thickness.is_finite() && thickness > 0.0 {
                choices.push(thickness);
            }
        }
    }
    choices.sort_unstable_by(|a, b| {
        (a - plate.thickness_mm)
            .abs()
            .total_cmp(&(b - plate.thickness_mm).abs())
    });
    let mut offered = [None; 2];
    let mut attempts = [0; 2];
    let mut events = geometry_events.to_vec();
    for thickness in choices {
        crate::check_cancel(cancel)?;
        let side = usize::from(thickness >= plate.thickness_mm);
        if offered[side].is_some() || attempts[side] == 16 {
            continue;
        }
        attempts[side] += 1;
        let ends = [
            plate.source_plane_mm - thickness * 0.5,
            plate.source_plane_mm + thickness * 0.5,
        ]
        .map(|height| super::forward(&occurrence.pose, [0., 0., height])[2]);
        if ends
            .iter()
            .any(|height| !height.is_finite() || *height <= bed || *height >= top)
        {
            continue;
        }
        events.truncate(geometry_events.len());
        for other in process {
            if let Some(zone) = other.zone.filter(|zone| zone.axis == 2) {
                if other.occurrence_id == capture.occurrence_id {
                    events.extend([ends[0], zone.coordinate, ends[1]]);
                } else {
                    events.extend(zone.z_events());
                }
            }
        }
        let scheduled = super::super::layer_schedule(bed, top, recipe.layer_height_mm, &events)
            .and_then(|schedule| {
                super::super::refine_surface_schedule(schedule, &admission.surface_fields, cancel)
            });
        match scheduled {
            Ok(schedule)
                if schedule.iter().all(|layer| {
                    input
                        .intent
                        .printer
                        .deposition_limits
                        .admits(recipe.bead_width_mm, layer.z - layer.lower_z)
                }) =>
            {
                offered[side] = Some(thickness)
            }
            Ok(_) => {}
            Err(error) if error.code == ManufacturingErrorCode::UnsupportedGeometry => {}
            Err(error) => return Err(error),
        }
    }
    failure.message = format!(
        "{}; source occurrence {} centered plane {} mm: nearest offered schedule-admissible thickness choices {:?} mm (current native/field stops and deposition limits; requires Compile, not material/bridge admission). Requested thickness remains {} mm",
        failure.message,
        capture.occurrence_id.get(),
        plate.source_plane_mm,
        offered.into_iter().flatten().collect::<Vec<_>>(),
        plate.thickness_mm
    );
    Ok(failure)
}
