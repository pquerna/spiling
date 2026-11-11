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

fn replaced_sparse(path: &DepositionPath, owner: Option<DepositionOwner>) -> bool {
    path.owner == owner
        && (path.kind == DepositionKind::SparseFill
            || path.kind == DepositionKind::Bridge
                && matches!(owner, Some(DepositionOwner::Regional { .. })))
}

#[derive(Clone, Copy)]
struct CoarseBudget {
    segment_base: usize,
    live_bytes: usize,
    outside_bytes: usize,
}
fn hatch_coarse(
    interior: &classification::InteriorRegion,
    hatch: &region::HatchLayer<'_>,
    owner: Option<DepositionOwner>,
    budget: CoarseBudget,
    cancel: &AtomicBool,
) -> Result<Vec<DepositionPath>, ManufacturingError> {
    let mut coarse = Vec::new();
    let mut segments = budget.segment_base;
    let mut proposal_points = 0usize;
    for polygon in &interior.domain.0 {
        check_cancel(cancel)?;
        let rows = if let Some(origin) = interior.frame_origin_mm {
            region::HatchRows::common(polygon, interior.coverage.fraction, hatch, origin)?
        } else {
            region::HatchRows::new(polygon, interior.coverage.fraction, hatch)?
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
    let proposal_bytes = proposal_points
        .checked_mul(std::mem::size_of::<DepositionPath>() + 2 * std::mem::size_of::<[f64; 3]>())
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "combined proposal byte overflow",
            )
        })?;
    proposal_bytes
        .checked_add(budget.live_bytes)
        .filter(|bytes| *bytes <= MAX_STAGE_BYTES)
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "combined hatch proposal exceeds 4 MiB",
            )
        })?;
    super::geometry_bytes(&[budget.outside_bytes, proposal_bytes])?;
    region::hatch_with_phase(
        &interior.domain,
        DepositionKind::SparseFill,
        interior.coverage.fraction,
        hatch,
        interior.frame_origin_mm,
        region::OrdinaryHatchPhase::Centered,
        &mut coarse,
        &mut segments,
        cancel,
    )?;
    for p in &mut coarse {
        p.owner = owner;
    }
    if budget
        .live_bytes
        .saturating_add(super::bonds::path_bytes(&coarse))
        > MAX_STAGE_BYTES
    {
        return Err(error(
            ManufacturingErrorCode::ResourceLimit,
            "combined coarse staging exceeds4 MiB",
        ));
    }
    Ok(coarse)
}

#[expect(
    clippy::too_many_arguments,
    reason = "Band combination retains separate native sources, fine layers, walls and skin horizons with their work owners"
)]
pub(super) fn combine(
    plan: &mut NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    sections: &[SectionSpan],
    schedule: &[LayerElevation],
    layouts: &[Option<Arc<classification::WallPlan>>],
    neighborhoods: &mut super::SkinNeighborhoods<'_>,
    retained_points: usize,
    outside_layer_payload_bytes: usize,
    geometry_work: &mut spiling_geometry::GeometryWorkBudget,
    native_live_bytes: usize,
    native_sources: &[spiling_geometry::NativeUnionSourceCapture],
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let SparseInfillPolicy::CombineEligible { max_height_mm } = intent.recipe.sparse_infill else {
        return Ok(());
    };
    let recipe = &intent.recipe;
    let ceiling = max_height_mm.min(intent.effective_deposition_limits().max_bead_height_mm);
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
        let Some(layout) = &layouts[first] else {
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
        if (first..=last).any(|index| {
            layouts[index]
                .as_ref()
                .is_none_or(|other| other.fill != layout.fill)
        }) {
            record(
                plan,
                first,
                last,
                None,
                SparseBandDisposition::RetainedFine {
                    reason: SparseBandVeto::ChangingDomain,
                },
            )?;
            first += 1;
            continue;
        }
        // Band geometry and original source ownership are fixed here. Exact
        // source invariance OR the separately queried constant inner-material
        // prism is required; occupied outer envelopes cannot establish it.
        let live_geometry = super::skin_retained_points(retained_points, neighborhoods, &[])?;
        let live_owned_bytes = super::skin_retained_bytes(
            super::geometry_bytes(&[
                outside_layer_payload_bytes,
                super::layer_payload_bytes(&plan.layers)?,
            ])?,
            neighborhoods,
            &[],
        )?;
        let mut classified = classification::ClassifiedRegions::new(
            Arc::clone(layout),
            &layout.fill,
            &layout.fill,
            classification::fixed_owned_budget(live_geometry, live_owned_bytes),
            recipe,
            cancel,
        )?
        .with_process(
            &section.process_domains,
            intent,
            classification::fixed_owned_budget(live_geometry, live_owned_bytes),
            neighborhoods.work_mut(),
            cancel,
        )?;
        classified.with_tool_widths(
            intent,
            classification::fixed_owned_budget(live_geometry, live_owned_bytes),
            neighborhoods.work_mut(),
            cancel,
        )?;
        let classified_owned_bytes = classified.retained_bytes();
        let mut retained_landing = false;
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
                        .any(|p| p.owner == *o && !replaced_sparse(p, *o))
                        || !l.paths.iter().any(|p| replaced_sparse(p, *o))
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
            let width = interior.bead_width_mm.unwrap_or(recipe.bead_width_mm);
            let area = (width - height) * height + std::f64::consts::PI * (height * 0.5).powi(2);
            let tool = match &intent.printer.workflow {
                JobWorkflow::FixedDualMarlin { job } => {
                    let assignment = job.assignment_for_region(&interior.coverage)?;
                    let material = assignment.material.material_id_for_interval([lower_z, z])?;
                    Some(
                        job.resolve_tool(
                            material,
                            assignment
                                .active_interior()
                                .map(|binding| binding.tool_id.as_str())
                                .or(assignment.tool_id.as_deref()),
                        )?,
                    )
                }
                _ => None,
            };
            let process = intent.process_for_tool(tool)?;
            let tool_admits = match (&intent.printer.workflow, tool) {
                (JobWorkflow::FixedDualMarlin { job }, Some(index)) => {
                    job.tools[usize::from(index)].admits_deposition(width, height)
                }
                _ => intent.admits_deposition(width, height),
            };
            if !tool_admits
                || height < intent.effective_deposition_limits().min_bead_height_mm
                || height > width
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
            if area * process.flow_multiplier * process.speed_mm_s
                > process.max_volumetric_flow_mm3_s
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
                    width,
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
            let segment_base = plan
                .layers
                .iter()
                .flat_map(|l| &l.paths)
                .map(|p| p.points_mm.len() - 1)
                .sum();
            let hatch = region::HatchLayer {
                roof: None,
                width_mm: width,
                z,
                lower_z,
                height,
                axis: region::HatchAxis::for_policy(recipe.rectilinear_axis, last as u32),
                recipe,
                top_axis: region::HatchAxis::for_policy(recipe.top_skin_axis, last as u32),
            };
            let budget = CoarseBudget {
                segment_base,
                live_bytes,
                outside_bytes: super::geometry_bytes(&[live_owned_bytes, classified_owned_bytes])?,
            };
            let mut coarse = hatch_coarse(interior, &hatch, terminal_owner, budget, cancel)?;
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
            super::geometry_bytes(&[
                live_owned_bytes,
                classified_owned_bytes,
                super::bonds::path_bytes(&coarse),
            ])?;
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
            let mut coarse_admission = support::strokes::analyze(
                &coarse,
                base.iter(),
                std::iter::empty(),
                recipe,
                &mut anchors,
                &mut anchor_work,
                cancel,
            )?;
            let mut landing_aligned = false;
            if coarse_admission.failure.is_some() {
                // A failed base authorizes one bounded alternative: align the
                // bound-tool rows to a retained landing, or independently prove
                // genuine orthogonal Bridges for the existing single-tool lane.
                drop(coarse_admission);
                drop(coarse);
                if matches!(&intent.printer.workflow,
                    JobWorkflow::FixedDualMarlin { job } if job.has_bound_interior())
                {
                    // Reuse the retained landing's whole hole-split XY rows,
                    // regenerating their physical interval rather than moving
                    // the landing or inventing source-material anchors.
                    coarse = align_to_landing(
                        &plan.layers[last + 1],
                        owners[last + 1 - first],
                        terminal_owner,
                        &hatch,
                        budget,
                        cancel,
                    )?;
                    if !section.invariant {
                        // Include whole anchor extensions in the full-height
                        // query. The earlier interior-only receipt cannot grant
                        // authority to copied rows outside that center domain.
                        let domain = aligned_row_domain(&coarse, width, budget, cancel)?;
                        if !inner_material_admits(
                            section,
                            &domain,
                            &interior.coverage,
                            [lower_z, z],
                            width,
                            intent,
                            native_sources,
                            native_live_bytes
                                .saturating_add(super::bonds::path_bytes(&coarse))
                                .saturating_add(region::cloned_polygons_bytes(&domain.0)?),
                            geometry_work,
                            cancel,
                        )? {
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
                    }
                    landing_aligned = true;
                } else {
                    let alternate = region::HatchLayer {
                        axis: hatch.axis.orthogonal(),
                        ..hatch
                    };
                    coarse = hatch_coarse(interior, &alternate, terminal_owner, budget, cancel)?;
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
                coarse_admission = support::strokes::analyze(
                    &coarse,
                    base.iter(),
                    std::iter::empty(),
                    recipe,
                    &mut anchors,
                    &mut anchor_work,
                    cancel,
                )?;
                if coarse.is_empty()
                    || (!landing_aligned && coarse_admission.bridges.len() != coarse.len())
                {
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
            }
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
            if landing_aligned {
                let volume = coarse.iter().try_fold(0.0, |sum, path| {
                    region::nominal_volume_with_process(
                        path,
                        last as u32,
                        process.flow_multiplier,
                        process.policy,
                    )
                    .map(|v| sum + v)
                })?;
                let target = plan.layers[first..=last]
                    .iter()
                    .zip(&owners)
                    .map(|(layer, owner)| {
                        let Some(DepositionOwner::Regional { region_index }) = owner else {
                            unreachable!()
                        };
                        let coverage =
                            &layer.region_coverage.process_regions[usize::from(*region_index)];
                        coverage.area_mm2 * coverage.fraction * (layer.z_mm - layer.lower_z_mm)
                    })
                    .sum::<f64>();
                if !volume.is_finite() || volume < target {
                    record(
                        plan,
                        first,
                        last,
                        terminal_owner,
                        SparseBandDisposition::RetainedFine {
                            reason: SparseBandVeto::NominalDose,
                        },
                    )?;
                    continue;
                }
            }
            if coarse.iter().any(|path| {
                let bridge = path.kind == DepositionKind::Bridge;
                let speed = process.speed_mm_s
                    * if bridge {
                        process.policy.bridge_speed_factor
                    } else {
                        1.0
                    };
                let flow = process.flow_multiplier
                    * if bridge {
                        process.policy.bridge_flow_factor
                    } else {
                        1.0
                    };
                let feed = (speed * 60_000_000.0).round() / 1_000_000.0;
                area * flow * feed / 60.0 > process.max_volumetric_flow_mm3_s
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
            let landing_capacity = plan.layers[last + 1].paths.len();
            let landing_bytes = landing_capacity
                .checked_mul(std::mem::size_of::<(usize, DepositionKind)>())
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "combined landing role capacity overflow",
                    )
                })?;
            if live_bytes
                .saturating_add(super::bonds::path_bytes(&coarse))
                .saturating_add(landing_bytes)
                > MAX_STAGE_BYTES
            {
                return Err(error(
                    ManufacturingErrorCode::ResourceLimit,
                    "combined landing role staging exceeds4 MiB",
                ));
            }
            let mut landing_roles = Vec::new();
            landing_roles
                .try_reserve_exact(landing_capacity)
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
                    .filter(|path| offset == 0 || !replaced_sparse(path, owners[offset - 1]))
                    .chain(coarse.iter().filter(|_| offset == terminal_offset + 1));
                // Coarse strokes were proved against the actual band base.
                // Every retained fine wall/interface still needs actual earlier
                // strokes, including the proposed coarse terminal at landing.
                for (path_index, path) in plan.layers[first + offset]
                    .paths
                    .iter()
                    .enumerate()
                    .filter(|(_, path)| {
                        offset > terminal_offset || !replaced_sparse(path, owners[offset])
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
                        landing_roles.push((path_index, DepositionKind::Bridge));
                    }
                    if admission.bridges.is_empty() && path.kind == DepositionKind::Bridge {
                        if offset == terminal_offset + 1 && replaced_sparse(path, owners[offset]) {
                            landing_roles.push((path_index, DepositionKind::SparseFill));
                        } else {
                            landing = false;
                            break;
                        }
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
            for &(index, kind) in &landing_roles {
                plan.layers[last + 1].paths[index].kind = kind;
            }
            for (offset, layer) in plan.layers[first..=last].iter_mut().enumerate() {
                layer
                    .paths
                    .retain(|path| !replaced_sparse(path, owners[offset]));
            }
            plan.layers[last].paths.extend(coarse);
            record(
                plan,
                first,
                last,
                terminal_owner,
                SparseBandDisposition::Combined,
            )?;
            plan.sparse_bands
                .last_mut()
                .expect("recorded band")
                .landing_aligned = landing_aligned;
            retained_landing |= landing_aligned;
        }
        first = last + if retained_landing { 2 } else { 1 };
    }
    support::object_volumes(&mut plan.layers, intent)?;
    Ok(())
}

fn aligned_row_domain(
    paths: &[DepositionPath],
    width: f64,
    budget: CoarseBudget,
    cancel: &AtomicBool,
) -> Result<geo::MultiPolygon<f64>, ManufacturingError> {
    let bytes = paths
        .len()
        .checked_mul(
            std::mem::size_of::<geo::Polygon<f64>>() + 5 * std::mem::size_of::<geo::Coord<f64>>(),
        )
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "aligned native row domain size overflow",
            )
        })?;
    if bytes.saturating_add(budget.live_bytes).saturating_add(
        paths.len() * (std::mem::size_of::<DepositionPath>() + 2 * std::mem::size_of::<[f64; 3]>()),
    ) > MAX_STAGE_BYTES
    {
        return Err(error(
            ManufacturingErrorCode::ResourceLimit,
            "aligned native row domain staging limit exceeded",
        ));
    }
    super::geometry_bytes(&[budget.outside_bytes, bytes])?;
    let mut polygons = Vec::new();
    polygons.try_reserve_exact(paths.len()).map_err(|_| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "aligned native row domain allocation refused",
        )
    })?;
    for path in paths {
        check_cancel(cancel)?;
        let [a, b] = path.points_mm.as_slice() else {
            return Err(error(
                ManufacturingErrorCode::InvalidSpecification,
                "aligned native row domain requires two endpoints",
            ));
        };
        // A positive-width conservative centerline superset. Native admission
        // independently expands it by the whole bead/projection radius.
        let [min, max] = if a[0] == b[0] {
            [
                [a[0] - width * 0.5, a[1].min(b[1])],
                [a[0] + width * 0.5, a[1].max(b[1])],
            ]
        } else if a[1] == b[1] {
            [
                [a[0].min(b[0]), a[1] - width * 0.5],
                [a[0].max(b[0]), a[1] + width * 0.5],
            ]
        } else {
            return Err(error(
                ManufacturingErrorCode::InvalidSpecification,
                "aligned native row domain is not rectilinear",
            ));
        };
        polygons.push(geo::Polygon::new(
            geo::LineString::from(vec![
                (min[0], min[1]),
                (max[0], min[1]),
                (max[0], max[1]),
                (min[0], max[1]),
                (min[0], min[1]),
            ]),
            Vec::new(),
        ));
    }
    let domain = geo::MultiPolygon(polygons);
    region::check_size(&domain)?;
    Ok(domain)
}

fn align_to_landing(
    landing: &PrintLayer,
    owner: Option<DepositionOwner>,
    terminal_owner: Option<DepositionOwner>,
    hatch: &region::HatchLayer<'_>,
    budget: CoarseBudget,
    cancel: &AtomicBool,
) -> Result<Vec<DepositionPath>, ManufacturingError> {
    let count = landing
        .paths
        .iter()
        .filter(|p| replaced_sparse(p, owner))
        .count();
    let bytes = count
        .checked_mul(std::mem::size_of::<DepositionPath>() + 2 * std::mem::size_of::<[f64; 3]>())
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "landing-aligned row bytes overflow",
            )
        })?;
    if bytes.saturating_add(budget.live_bytes) > MAX_STAGE_BYTES
        || budget.segment_base.saturating_add(count) > MAX_DEPOSITION_SEGMENTS as usize
    {
        return Err(error(
            ManufacturingErrorCode::ResourceLimit,
            "landing-aligned row staging limit exceeded",
        ));
    }
    super::geometry_bytes(&[budget.outside_bytes, bytes])?;
    let mut result = Vec::new();
    result.try_reserve_exact(count).map_err(|_| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "landing-aligned row allocation refused",
        )
    })?;
    for path in landing.paths.iter().filter(|p| replaced_sparse(p, owner)) {
        check_cancel(cancel)?;
        if path.points_mm.len() != 2 || region::constant_width(path)? != hatch.width_mm {
            return Ok(Vec::new());
        }
        result.push(DepositionPath {
            kind: DepositionKind::SparseFill,
            material_domain: path.material_domain,
            owner: terminal_owner,
            width: WidthProfile::Constant {
                width_mm: hatch.width_mm,
            },
            physical_z_mm: [hatch.lower_z, hatch.z],
            points_mm: path
                .points_mm
                .iter()
                .map(|p| [p[0], p[1], hatch.z])
                .collect(),
            arcs: Vec::new(),
        });
    }
    Ok(result)
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
                    nominal_stadium_core_width_mm(region::constant_width(separator)?, fine_height)
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
                    let edges = path.checked_edges()?;
                    let first = edges.edge(0).ok_or_else(|| {
                        error(
                            ManufacturingErrorCode::InvalidSpecification,
                            "coarse contact lacks first edge",
                        )
                    })?;
                    let last = edges.edge(edges.len() - 1).ok_or_else(|| {
                        error(
                            ManufacturingErrorCode::InvalidSpecification,
                            "coarse contact lacks last edge",
                        )
                    })?;
                    for (point, width) in [
                        (first.points_mm[0], first.width_mm),
                        (last.points_mm[1], last.width_mm),
                    ] {
                        let Some(core) = nominal_stadium_core_width_mm(width, height) else {
                            continue;
                        };
                        let inner_sum = ((core + fine_core) * 0.5).next_down();
                        let outer_sum = (width + region::constant_width(separator)?) * 0.5;
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
        landing_aligned: false,
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
    width_mm: f64,
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
    let radius = (width_mm * 0.5 + projection_radius).next_up();
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
            .map_err(|failure| super::native_error(failure, "full-height native inner material"))?
            .is_none()
        {
            return Ok(false);
        }
    }
    Ok(true)
}
