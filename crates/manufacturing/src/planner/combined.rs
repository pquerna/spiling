// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::{LayerElevation, SectionSpan, classification, region, support};
use crate::{check_cancel, error};
use spiling_contracts::manufacturing::*;
use std::sync::{Arc, atomic::AtomicBool};

// At most three actual intervals plus their first retained landing layer are
// staged. The original admitted fine alternative remains untouched until commit.
const MAX_BAND_INTERVALS: usize = 3;
const MAX_STAGE_BYTES: usize = 4 * 1024 * 1024;

pub(super) fn combine(
    plan: &mut NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    sections: &[SectionSpan],
    schedule: &[LayerElevation],
    layouts: &[Option<Arc<classification::WallLayout>>],
    neighborhoods: &mut super::SkinNeighborhoods<'_>,
    retained_points: usize,
    geometry_work: &mut spiling_geometry::GeometryWorkBudget,
    native_live_bytes: usize,
    native_sources: &[spiling_geometry::NativeUnionSourceCapture],
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let SparseInfillPolicy::CombineEligible { max_height_mm } = intent.recipe.sparse_infill else {
        return Ok(());
    };
    let recipe = &intent.recipe;
    let ceiling = max_height_mm.min(intent.printer.deposition_limits.max_bead_height_mm);
    // This first envelope makes no new support-clearance claim after changing
    // sparse pitch. Jobs with actual breakaway strokes retain their already
    // admitted fine alternative and its complete native clearance proof.
    let has_support = plan
        .layers
        .iter()
        .flat_map(|layer| &layer.paths)
        .any(|path| {
            matches!(
                path.kind,
                DepositionKind::Support | DepositionKind::SupportInterface
            )
        });
    let mut anchor_work = 0;
    let mut anchors = support::AnchorWorkspace::default();
    let mut first = 1usize;
    while first + 1 < plan.layers.len() {
        check_cancel(cancel)?;
        let mut last = first;
        while last + 1 < plan.layers.len() - 1
            && last + 1 - first < MAX_BAND_INTERVALS
            && schedule[last + 1].z - schedule[first].lower_z <= ceiling
        {
            last += 1;
        }
        if last == first {
            record(
                plan,
                first,
                first,
                None,
                SparseBandDisposition::RetainedFine {
                    reason: SparseBandVeto::ProcessHeight,
                },
            )?;
            first += 1;
            continue;
        }
        let span = super::span_at(sections, first as u32);
        let section = &sections[span];
        let neighborhood = first - 1..=last + 1;
        let reason = if (!section.invariant && section.inner_material.is_none())
            || (first..=last).any(|i| super::span_at(sections, i as u32) != span)
        {
            Some(SparseBandVeto::ChangingDomain)
        } else if has_support
            || neighborhood.clone().any(|i| {
                let layer = &plan.layers[i];
                layer.region_coverage.bottom_skin_area_mm2 > 0.0
                    || layer.region_coverage.top_skin_area_mm2 > 0.0
                    || !layer.region_coverage.process_diaphragms.is_empty()
                    || layer.region_coverage.surface_added_area_mm2 > 0.0
                    || layer.paths.iter().any(|p| {
                        matches!(
                            p.kind,
                            DepositionKind::Support | DepositionKind::SupportInterface
                        )
                    })
            })
        {
            Some(SparseBandVeto::MandatoryNeighborhood)
        } else {
            None
        };
        if let Some(reason) = reason {
            record(
                plan,
                first,
                last,
                None,
                SparseBandDisposition::RetainedFine { reason },
            )?;
            first = last + 1;
            continue;
        }
        let Some(layout) = &layouts[span] else {
            record(
                plan,
                first,
                last,
                None,
                SparseBandDisposition::RetainedFine {
                    reason: SparseBandVeto::ChangingDomain,
                },
            )?;
            first = last + 1;
            continue;
        };
        // Band geometry and original source ownership are fixed here. Exact
        // source invariance OR the separately queried constant inner-material
        // prism is required; occupied outer envelopes cannot establish it.
        let live_geometry = super::skin_retained_points(retained_points, neighborhoods, &[])?;
        let classified = classification::ClassifiedRegions::new(
            Arc::clone(layout),
            &layout.fill,
            &layout.fill,
            classification::fixed_point_budget(live_geometry),
            recipe,
            cancel,
        )?
        .with_process(
            &section.process_domains,
            recipe,
            classification::fixed_point_budget(live_geometry + layout.point_count()),
            neighborhoods.work_mut(),
            cancel,
        )?;
        for (region_index, interior) in classified.interiors.iter().enumerate() {
            check_cancel(cancel)?;
            if interior.coverage.fraction >= 1.0 {
                continue;
            }
            let owner = Some(DepositionOwner::Regional {
                region_index: region_index as u16,
            });
            let mut owners = [None; MAX_BAND_INTERVALS + 1];
            let mut same = true;
            for (offset, layer) in plan.layers[first..=last + 1].iter().enumerate() {
                let mut matches = layer
                    .region_coverage
                    .process_regions
                    .iter()
                    .enumerate()
                    .filter(|(_, r)| {
                        r.fraction == interior.coverage.fraction
                            && r.contributions == interior.coverage.contributions
                    });
                let Some((index, _)) = matches.next() else {
                    same = false;
                    break;
                };
                if matches.next().is_some() {
                    same = false;
                    break;
                }
                owners[offset] = Some(DepositionOwner::Regional {
                    region_index: index as u16,
                });
            }
            if !same
                || plan.layers[first..=last].iter().zip(&owners).any(|(l, o)| {
                    l.paths
                        .iter()
                        .any(|p| p.owner == *o && p.kind != DepositionKind::SparseFill)
                        || !l
                            .paths
                            .iter()
                            .any(|p| p.owner == *o && p.kind == DepositionKind::SparseFill)
                })
            {
                record(
                    plan,
                    first,
                    last,
                    owner,
                    SparseBandDisposition::RetainedFine {
                        reason: SparseBandVeto::ChangingDomain,
                    },
                )?;
                continue;
            }
            let terminal_owner = owners[last - first];
            let z = schedule[last].z;
            let lower_z = schedule[first].lower_z;
            let height = z - lower_z;
            let area = (recipe.bead_width_mm - height) * height
                + std::f64::consts::PI * (height * 0.5).powi(2);
            if height < intent.printer.deposition_limits.min_bead_height_mm
                || height > recipe.bead_width_mm
            {
                record(
                    plan,
                    first,
                    last,
                    terminal_owner,
                    SparseBandDisposition::RetainedFine {
                        reason: SparseBandVeto::ProcessHeight,
                    },
                )?;
                continue;
            }
            if area * recipe.flow_multiplier * recipe.print_speed_mm_s
                > intent.printer.max_volumetric_flow_mm3_s
            {
                record(
                    plan,
                    first,
                    last,
                    terminal_owner,
                    SparseBandDisposition::RetainedFine {
                        reason: SparseBandVeto::FlowLimit,
                    },
                )?;
                continue;
            }
            if !section.invariant
                && !inner_material_admits(
                    section,
                    &interior.domain,
                    &interior.coverage,
                    [lower_z, z],
                    intent,
                    native_sources,
                    native_live_bytes,
                    geometry_work,
                    cancel,
                )?
            {
                record(
                    plan,
                    first,
                    last,
                    terminal_owner,
                    SparseBandDisposition::RetainedFine {
                        reason: SparseBandVeto::ChangingDomain,
                    },
                )?;
                continue;
            }
            let live_bytes = plan.layers[first..=last + 1]
                .iter()
                .map(|l| super::bonds::path_bytes(&l.paths))
                .sum::<usize>();
            if live_bytes > MAX_STAGE_BYTES {
                return Err(error(
                    ManufacturingErrorCode::ResourceLimit,
                    "combined sparse band staging exceeds 4 MiB",
                ));
            }
            let mut coarse = Vec::new();
            let mut segments = plan
                .layers
                .iter()
                .flat_map(|l| &l.paths)
                .map(|p| p.points_mm.len() - 1)
                .sum();
            let hatch = region::HatchLayer {
                z,
                lower_z,
                height,
                axis: region::HatchAxis::for_policy(recipe.rectilinear_axis, last as u32),
                recipe,
                top_axis: region::HatchAxis::for_policy(recipe.top_skin_axis, last as u32),
            };
            let mut proposal_points = 0usize;
            for polygon in &interior.domain.0 {
                check_cancel(cancel)?;
                let rows = if let Some(origin) = interior.frame_origin_mm {
                    region::HatchRows::common(polygon, interior.coverage.fraction, &hatch, origin)?
                } else {
                    region::HatchRows::new(polygon, interior.coverage.fraction, &hatch)?
                };
                proposal_points = proposal_points
                    .checked_add(rows.point_bound()?)
                    .ok_or_else(|| {
                        error(
                            ManufacturingErrorCode::ResourceLimit,
                            "combined hatch staging overflow",
                        )
                    })?;
            }
            proposal_points
                .checked_mul(
                    std::mem::size_of::<DepositionPath>() + 2 * std::mem::size_of::<[f64; 3]>(),
                )
                .and_then(|bytes| bytes.checked_add(live_bytes))
                .filter(|bytes| *bytes <= MAX_STAGE_BYTES)
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "combined hatch proposal exceeds 4 MiB",
                    )
                })?;
            region::hatch_with_phase(
                &interior.domain,
                DepositionKind::SparseFill,
                interior.coverage.fraction,
                &hatch,
                interior.frame_origin_mm,
                &mut coarse,
                &mut segments,
                cancel,
            )?;
            for p in &mut coarse {
                p.owner = terminal_owner;
            }
            if coarse.is_empty() {
                record(
                    plan,
                    first,
                    last,
                    terminal_owner,
                    SparseBandDisposition::RetainedFine {
                        reason: SparseBandVeto::ChangingDomain,
                    },
                )?;
                continue;
            }
            if live_bytes + super::bonds::path_bytes(&coarse) > MAX_STAGE_BYTES {
                return Err(error(
                    ManufacturingErrorCode::ResourceLimit,
                    "combined sparse band alternative exceeds 4 MiB",
                ));
            }
            if !full_height_interface_contacts(
                &plan.layers[first..=last],
                &coarse,
                &owners,
                neighborhoods.work_mut(),
                cancel,
            )? {
                record(
                    plan,
                    first,
                    last,
                    terminal_owner,
                    SparseBandDisposition::RetainedFine {
                        reason: SparseBandVeto::InterfaceBond,
                    },
                )?;
                continue;
            }
            let base = &plan.layers[first - 1].paths;
            let coarse_admission = support::strokes::analyze(
                &coarse,
                base.iter(),
                std::iter::empty(),
                recipe,
                &mut anchors,
                &mut anchor_work,
                cancel,
            )?;
            if coarse_admission.failure.is_some() {
                record(
                    plan,
                    first,
                    last,
                    terminal_owner,
                    SparseBandDisposition::RetainedFine {
                        reason: SparseBandVeto::MissingLowerAnchor,
                    },
                )?;
                continue;
            }
            // Only independently found direct, bounded base gaps become
            // RegionalBridge. Supported rows retain ordinary SparseFill.
            for &index in &coarse_admission.bridges {
                coarse[index].kind = DepositionKind::Bridge;
            }
            if coarse.iter().any(|path| {
                let bridge = path.kind == DepositionKind::Bridge;
                let speed = recipe.print_speed_mm_s
                    * if bridge {
                        recipe.process.bridge_speed_factor
                    } else {
                        1.0
                    };
                let flow = recipe.flow_multiplier
                    * if bridge {
                        recipe.process.bridge_flow_factor
                    } else {
                        1.0
                    };
                let feed = (speed * 60_000_000.0).round() / 1_000_000.0;
                area * flow * feed / 60.0 > intent.printer.max_volumetric_flow_mm3_s
            }) {
                record(
                    plan,
                    first,
                    last,
                    terminal_owner,
                    SparseBandDisposition::RetainedFine {
                        reason: SparseBandVeto::FlowLimit,
                    },
                )?;
                continue;
            }
            // Borrow the valid fine alternative throughout proof. The only
            // staged edits besides owned coarse paths are landing-kind indices;
            // their allocation is bounded by the already charged fine paths.
            let mut landing_bridges = Vec::new();
            landing_bridges
                .try_reserve_exact(plan.layers[last + 1].paths.len())
                .map_err(|_| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "combined landing index allocation refused",
                    )
                })?;
            let mut landing = true;
            let terminal_offset = last - first;
            for offset in 0..=terminal_offset + 1 {
                let previous_layer = &plan.layers[first + offset - 1];
                let previous = previous_layer
                    .paths
                    .iter()
                    .filter(|path| {
                        offset == 0
                            || !(path.kind == DepositionKind::SparseFill
                                && path.owner == owners[offset - 1])
                    })
                    .chain(coarse.iter().filter(|_| offset == terminal_offset + 1));
                // Coarse strokes were proved against the actual band base.
                // Every retained fine wall/interface still needs actual earlier
                // strokes, including the proposed coarse terminal at landing.
                for (path_index, path) in plan.layers[first + offset]
                    .paths
                    .iter()
                    .enumerate()
                    .filter(|(_, path)| {
                        offset > terminal_offset
                            || !(path.kind == DepositionKind::SparseFill
                                && path.owner == owners[offset])
                    })
                {
                    check_cancel(cancel)?;
                    if path.physical_z_mm[0] != schedule[first + offset].lower_z {
                        continue;
                    }
                    let admission = support::strokes::analyze(
                        std::slice::from_ref(path),
                        previous.clone(),
                        std::iter::empty(),
                        recipe,
                        &mut anchors,
                        &mut anchor_work,
                        cancel,
                    )?;
                    if admission.failure.is_some() {
                        landing = false;
                        break;
                    }
                    if !admission.bridges.is_empty() && path.kind != DepositionKind::Bridge {
                        if offset != terminal_offset + 1
                            || path.kind != DepositionKind::SparseFill
                            || path.owner != owners[offset]
                        {
                            landing = false;
                            break;
                        }
                        landing_bridges.push(path_index);
                    }
                }
                if !landing {
                    break;
                }
            }
            if !landing {
                record(
                    plan,
                    first,
                    last,
                    terminal_owner,
                    SparseBandDisposition::RetainedFine {
                        reason: SparseBandVeto::UnsupportedLanding,
                    },
                )?;
                continue;
            }
            // No geometry or earlier layer changes after this point: replace each
            // source/profile owner once, retain every other fine owner verbatim.
            for &index in &landing_bridges {
                plan.layers[last + 1].paths[index].kind = DepositionKind::Bridge;
            }
            for (offset, layer) in plan.layers[first..=last].iter_mut().enumerate() {
                layer.paths.retain(|path| {
                    !(path.kind == DepositionKind::SparseFill && path.owner == owners[offset])
                });
            }
            plan.layers[last].paths.extend(coarse);
            record(
                plan,
                first,
                last,
                terminal_owner,
                SparseBandDisposition::Combined,
            )?;
        }
        first = last + 1;
    }
    support::object_volumes(&mut plan.layers, recipe);
    Ok(())
}

/// The stadium's central rectangle supplies only a nominal full-height inner
/// core. Its strict overlap is separate from the permitted outer overlap and
/// from earlier-layer support; no future extrusion is an anchor here.
fn full_height_interface_contacts(
    layers: &[PrintLayer],
    coarse: &[DepositionPath],
    owners: &[Option<DepositionOwner>],
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    for (layer, owner) in layers.iter().zip(owners) {
        let Some(DepositionOwner::Regional { region_index }) = owner else {
            return Ok(false);
        };
        for interface in &layer.region_coverage.process_interfaces {
            if !interface.region_indices.contains(region_index) {
                continue;
            }
            for line in &interface.centerlines_mm {
                check_cancel(cancel)?;
                let ends = line.map(|p| [p[0], p[1], layer.z_mm]);
                let Some(separator) = layer.paths.iter().find(|p| {
                    p.kind == DepositionKind::SeparatorWall
                        && p.points_mm.len() == 2
                        && (p.points_mm.as_slice() == ends
                            || p.points_mm.as_slice() == [ends[1], ends[0]])
                }) else {
                    return Ok(false);
                };
                let fine_height =
                    (separator.physical_z_mm[1] - separator.physical_z_mm[0]).next_up();
                let Some(fine_core) =
                    nominal_stadium_core_width_mm(separator.bead_width_mm, fine_height)
                else {
                    return Ok(false);
                };
                let fine_inner = program_coordinates::project_xyz6_bead_inner_height_mm(
                    separator.physical_z_mm,
                )?;
                let mut bonded = false;
                for path in coarse {
                    if path.physical_z_mm[0] > separator.physical_z_mm[0]
                        || path.physical_z_mm[1] < separator.physical_z_mm[1]
                    {
                        continue;
                    }
                    let inner =
                        program_coordinates::project_xyz6_bead_inner_height_mm(path.physical_z_mm)?;
                    if inner[0].max(fine_inner[0]) >= inner[1].min(fine_inner[1]) {
                        continue;
                    }
                    let height = (path.physical_z_mm[1] - path.physical_z_mm[0]).next_up();
                    let Some(core) = nominal_stadium_core_width_mm(path.bead_width_mm, height)
                    else {
                        continue;
                    };
                    let inner_sum = ((core + fine_core) * 0.5).next_down();
                    let outer_sum = (path.bead_width_mm + separator.bead_width_mm) * 0.5;
                    for point in [path.points_mm[0], *path.points_mm.last().unwrap()] {
                        super::boolean::charge(work, 1)?;
                        check_cancel(cancel)?;
                        if support::endpoint_bond(point, ends, outer_sum)?
                            && support::endpoint_bond(point, ends, inner_sum)?
                        {
                            bonded = true;
                            break;
                        }
                    }
                    if bonded {
                        break;
                    }
                }
                if !bonded {
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
}

fn record(
    plan: &mut NormalizedPrintPlan,
    first: usize,
    last: usize,
    owner: Option<DepositionOwner>,
    disposition: SparseBandDisposition,
) -> Result<(), ManufacturingError> {
    if plan.sparse_bands.len() >= MAX_SPARSE_BANDS {
        return Err(error(
            ManufacturingErrorCode::ResourceLimit,
            "combined sparse decision limit exceeded",
        ));
    }
    plan.sparse_bands.try_reserve(1).map_err(|_| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "combined sparse decision allocation refused",
        )
    })?;
    plan.sparse_bands.push(SparseBandDecision {
        first_layer: first as u32,
        last_layer: last as u32,
        owner,
        disposition,
    });
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(super) fn inner_material_admits(
    section: &SectionSpan,
    eligible: &geo::MultiPolygon<f64>,
    coverage: &ProcessRegionCoverage,
    physical_z: [f64; 2],
    intent: &ManufacturingIntent,
    sources: &[spiling_geometry::NativeUnionSourceCapture],
    live_bytes: usize,
    work: &mut spiling_geometry::GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    use spiling_contracts::manufacturing::program_coordinates::project_xyz6_bead_height_mm;
    let Some(captures) = &section.inner_material else {
        return Ok(false);
    };
    if coverage.contributions.is_empty() {
        return Ok(false);
    }
    let outer_z = project_xyz6_bead_height_mm(physical_z[1], physical_z[1] - physical_z[0])?;
    let max_coordinate = intent
        .printer
        .build_envelope
        .min
        .into_iter()
        .chain(intent.printer.build_envelope.max)
        .map(f64::abs)
        .fold(0.0_f64, f64::max);
    // Bound XYZ6's multiply/round/divide on every endpoint in the admitted
    // envelope, then enclose its two-axis displacement. Not policy slack.
    let ulp = max_coordinate.next_up() - max_coordinate;
    let projection_radius =
        (((0.5e-6_f64).next_up() + 4.0 * ulp).next_up() * 2.0_f64.sqrt().next_up()).next_up();
    let radius = (intent.recipe.bead_width_mm * 0.5 + projection_radius).next_up();
    for contribution in &coverage.contributions {
        let Some(capture) = captures.iter().find(|capture| {
            capture.source().occurrence_id() == contribution.occurrence_id
                && capture.source().definition_id() == &contribution.definition_id
        }) else {
            return Ok(false);
        };
        let Some(domain) = section.process_domains.iter().find(|domain| {
            domain.coverage.contributions.iter().any(|source| {
                source.occurrence_id == contribution.occurrence_id
                    && source.definition_id == contribution.definition_id
            })
        }) else {
            return Ok(false);
        };
        let expected = sources
            .iter()
            .find(|source| {
                source.occurrence_id() == contribution.occurrence_id
                    && source.definition_id() == &contribution.definition_id
            })
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::NativeAnalysisIndeterminate,
                    "inner-material source lost its completed native binding",
                )
            })?;
        let actual = super::native::encode_polygons(
            domain.material.0.iter(),
            capture.sample_world_z_mm(),
            cancel,
        )?;
        let candidate =
            super::native::encode_polygons(eligible.0.iter(), capture.sample_world_z_mm(), cancel)?;
        let byte_limit = spiling_geometry::MAX_PREPARED_SECTION_SOURCE_BYTES.saturating_sub(
            live_bytes
                .saturating_add(super::native::loop_bytes(&actual))
                .saturating_add(super::native::loop_bytes(&candidate)),
        );
        if capture
            .certify_region(
                expected, &actual, &candidate, outer_z, radius, byte_limit, work, cancel,
            )
            .map_err(|failure| {
                super::native_error(failure, "full-height native inner material")
            })?
            .is_none()
        {
            return Ok(false);
        }
    }
    Ok(true)
}
