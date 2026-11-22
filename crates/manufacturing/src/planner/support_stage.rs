// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Support owns raster recovery and protected-void projection as part of its
//! stage. Both the compiler and a standalone replay must go through this seam.
use super::*;

mod snapshot;
pub(super) use snapshot::Capture;
pub(crate) use snapshot::replay;

pub(super) struct StageInput<'a> {
    pub(super) support: support::SupportInput<'a>,
    pub(super) classifications: &'a [(usize, classification::ClassifiedRegions, usize, usize)],
    pub(super) phase_by_layer: &'a [region::OrdinaryHatchPhase],
    pub(super) models: &'a BTreeMap<
        spiling_contracts::geometry::OccurrenceId,
        Arc<spiling_geometry::tolerant::TolerantModel>,
    >,
    pub(super) retained_points: usize,
    pub(super) support_classified_bytes: usize,
    pub(super) web_roof: Option<&'a cathedral::Field>,
    pub(super) cathedral_roads: &'a [Vec<DepositionPath>],
    pub(super) tentative: bool,
}

pub(super) struct StageWork<'a, 'section> {
    pub(super) segments: &'a mut usize,
    pub(super) geometry: &'a mut spiling_geometry::GeometryWorkBudget,
    pub(super) support: &'a mut support::SupportWork,
    pub(super) neighborhoods: RasterWork<'a, 'section>,
    pub(super) void_memo: &'a mut tolerant::VoidMemo,
}

/// Raster callbacks need the prior cache only to account for live scratch.
/// Standalone replay retains the recorded reservation and will refuse if
/// admission would require eviction from a cache it does not own.
pub(super) enum RasterWork<'a, 'section> {
    Live(&'a mut SkinNeighborhoods<'section>),
    Replay {
        work: usize,
        points: usize,
        bytes: usize,
    },
}

impl RasterWork<'_, '_> {
    fn work_mut(&mut self) -> &mut usize {
        match self {
            Self::Live(pool) => pool.work_mut(),
            Self::Replay { work, .. } => work,
        }
    }

    fn points(&self, retained: usize) -> Result<usize, ManufacturingError> {
        match self {
            Self::Live(pool) => skin_retained_points(retained, pool, &[]),
            Self::Replay { points, .. } => {
                skin_query_retained_points(retained.saturating_add(*points), &[])
            }
        }
    }

    fn bytes(&self, retained: usize) -> Result<usize, ManufacturingError> {
        match self {
            Self::Live(pool) => skin_retained_bytes(retained, pool, &[]),
            Self::Replay { bytes, .. } => geometry_bytes(&[retained, *bytes]),
        }
    }

    fn admit(
        &mut self,
        retained: usize,
        bytes: usize,
        scratch: usize,
        scratch_bytes: usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        match self {
            Self::Live(pool) => {
                admit_skin_scratch(retained, bytes, pool, &[], scratch, scratch_bytes, cancel)
            }
            Self::Replay { .. } => {
                check_cancel(cancel)?;
                remaining_geometry_units(
                    self.points(retained.saturating_add(scratch))?,
                    self.bytes(geometry_bytes(&[bytes, scratch_bytes])?)?,
                )?;
                Ok(())
            }
        }
    }
}

pub(super) fn run(
    layers: &mut [PrintLayer],
    input: &StageInput<'_>,
    work: StageWork<'_, '_>,
    cancel: &AtomicBool,
) -> Result<support::SupportAdmission, ManufacturingError> {
    let StageInput {
        support,
        classifications,
        phase_by_layer,
        models,
        retained_points,
        support_classified_bytes,
        web_roof,
        cathedral_roads,
        tentative,
    } = *input;
    let StageWork {
        segments,
        geometry,
        support: support_work,
        mut neighborhoods,
        void_memo,
    } = work;
    let support::SupportInput {
        sections,
        schedule,
        recipe,
        native_points: support_native_points,
        ..
    } = support;
    support_work.planar = *neighborhoods.work_mut();
    support::realize(
        layers,
        &support,
        segments,
        cancel,
        geometry,
        tentative,
        support_work,
        |index,
         axis,
         segments,
         retained_support_points,
         retained_support_bytes,
         anchor_owned_bytes,
         planar_work,
         recovery| {
            *neighborhoods.work_mut() = *planar_work;
            let outcome = (|| {
                let entry = classifications.partition_point(|entry| entry.0 <= index);
                let (_, classified, classified_points, classified_bytes) = classifications
                    .get(entry.wrapping_sub(1))
                    .expect("support reconstructs only classified object layers");
                let classified_bytes = *classified_bytes;
                let classified_points = *classified_points;
                let live_retained_points = (retained_points - classified_points)
                    .checked_add(retained_support_points)
                    .filter(|total| *total <= crate::working_set::units())
                    .ok_or_else(|| {
                        error(
                            ManufacturingErrorCode::ResourceLimit,
                            "support raster retained point limit exceeded",
                        )
                    })?;
                let outside_owned_bytes = geometry_bytes(&[
                    support_classified_bytes - classified_bytes,
                    retained_support_bytes,
                ])?;
                let mut live_owned_bytes =
                    geometry_bytes(&[outside_owned_bytes, anchor_owned_bytes])?;
                let anchor_capacity = std::cell::Cell::new(anchor_owned_bytes);
                let elevation = &schedule[index];
                let current = span_at(sections, index as u32);
                let hatch_layer = region::HatchLayer {
                    roof: web_roof.map(|field| {
                        if cathedral_roads.get(index).is_some() {
                            field.reused_roof()
                        } else {
                            field.roof()
                        }
                    }),
                    width_mm: recipe.bead_width_mm,
                    z: elevation.z,
                    lower_z: elevation.lower_z,
                    height: elevation.z - elevation.lower_z,
                    axis,
                    top_axis: region::HatchAxis::for_policy(
                        recipe.top_skin_axis,
                        hatch_key(schedule, index),
                    ),
                    recipe,
                };
                let recovering_components = recovery.is_some();
                let mut skin_bridge_domains = Vec::new();
                let mut skin_command_fits = Vec::new();
                let paths = if let Some(anchors) = recovery {
                    let mut check_component =
                        |candidate: &mut support::RasterCandidate,
                         axis,
                         scratch: usize,
                         scratch_owned_bytes: usize| {
                            let live = scratch.checked_sub(classified_points).ok_or_else(|| {
                                error(
                                    ManufacturingErrorCode::ResourceLimit,
                                    "component scratch omits retained classification",
                                )
                            })?;
                            let receipt = anchors(
                                candidate,
                                axis,
                                live,
                                scratch_owned_bytes
                                    .checked_sub(classified_bytes)
                                    .ok_or_else(|| {
                                        error(
                                            ManufacturingErrorCode::ResourceLimit,
                                            "component scratch omits retained classification bytes",
                                        )
                                    })?,
                            )?;
                            anchor_capacity.set(receipt.workspace_bytes);
                            Ok(receipt)
                        };
                    classified.paths_with_bridge_recovery(
                        &hatch_layer,
                        phase_by_layer[index],
                        &sections[current].process_domains,
                        classified_bytes,
                        anchor_owned_bytes,
                        segments,
                        |scratch, scratch_bytes, work| {
                            boolean::charge(neighborhoods.work_mut(), work)?;
                            neighborhoods.admit(
                                live_retained_points,
                                outside_owned_bytes,
                                scratch,
                                scratch_bytes,
                                cancel,
                            )
                        },
                        &mut check_component,
                        &mut skin_bridge_domains,
                        &mut skin_command_fits,
                        cancel,
                    )?
                } else {
                    let (paths, _, fits) = classified.paths(
                        elevation.z,
                        elevation.lower_z,
                        axis,
                        region::HatchAxis::for_policy(
                            recipe.top_skin_axis,
                            hatch_key(schedule, index),
                        ),
                        recipe,
                        web_roof.map(|field| {
                            if cathedral_roads.get(index).is_some() {
                                field.reused_roof()
                            } else {
                                field.roof()
                            }
                        }),
                        phase_by_layer[index],
                        classified_bytes,
                        &sections[current].process_domains,
                        segments,
                        |scratch, scratch_bytes, work| {
                            boolean::charge(neighborhoods.work_mut(), work)?;
                            neighborhoods.admit(
                                live_retained_points,
                                live_owned_bytes,
                                scratch,
                                scratch_bytes,
                                cancel,
                            )
                        },
                        cancel,
                    )?;
                    skin_command_fits = fits;
                    paths
                };
                let paths = match cathedral_roads.get(index) {
                    Some(built) => {
                        let mut paths = paths;
                        let count = |paths: &[DepositionPath]| -> usize {
                            paths
                                .iter()
                                .filter(|path| cathedral::is_road(path))
                                .map(|path| path.points_mm.len().saturating_sub(1))
                                .sum()
                        };
                        *segments = segments
                            .saturating_sub(count(&paths))
                            .saturating_add(count(built));
                        paths.retain(|path| !cathedral::is_road(path));
                        paths.extend(built.iter().cloned());
                        paths
                    }
                    None => paths,
                };
                // Recovery scratch carries the whole workspace receipt once.
                // Refresh its capacity before cloning interface metadata below.
                live_owned_bytes = geometry_bytes(&[outside_owned_bytes, anchor_capacity.get()])?;
                let base = neighborhoods.points(live_retained_points)?
                    + classified.retained_points()
                    + paths.iter().map(|p| p.points_mm.capacity()).sum::<usize>()
                    + bonds::skin_bridge_points(&skin_bridge_domains)
                    + bonds::skin_command_fit_points(&skin_command_fits);
                let live_bytes = geometry_bytes(&[
                    neighborhoods.bytes(live_owned_bytes)?,
                    classified_bytes,
                    bonds::path_bytes(&paths),
                    bonds::skin_bridge_bytes(&skin_bridge_domains),
                    bonds::skin_command_fit_bytes(&skin_command_fits),
                ])?;
                let mut admit = classification::fixed_owned_budget(base, live_bytes);
                admit(
                    bonds::interface_owned_bytes(&classified.coverage.process_interfaces)
                        .div_ceil(std::mem::size_of::<geo::Coord<f64>>()),
                    bonds::interface_owned_bytes(&classified.coverage.process_interfaces),
                )?;
                let process_interfaces = classified.coverage.process_interfaces.clone();
                let whole_rows = if recovering_components {
                    // A bridge band and authored complement are disjoint owners.
                    // Global row extension must not re-enter the selected band.
                    Vec::new()
                } else {
                    let outside_rows_bytes = geometry_bytes(&[
                        live_bytes,
                        bonds::interface_owned_bytes(&process_interfaces),
                    ])?;
                    let outside_rows_units = base
                        + bonds::interface_owned_bytes(&process_interfaces)
                            .div_ceil(std::mem::size_of::<geo::Coord<f64>>());
                    classified.whole_rows(
                        &paths,
                        &hatch_layer,
                        segments,
                        cancel,
                        classification::fixed_owned_budget(outside_rows_units, outside_rows_bytes),
                    )?
                };
                Ok(support::RasterCandidate {
                    skin_bridge_domains,
                    skin_command_fits,
                    paths,
                    whole_rows,
                    process_interfaces,
                })
            })();
            *planar_work = *neighborhoods.work_mut();
            outcome
        },
        |demand_bounds,
         retained_units,
         live_bytes,
         _native_points,
         _native_owned_bytes,
         _support_work,
         _native_work| {
            tolerant::project_protected_voids(
                models,
                void_memo,
                demand_bounds,
                retained_units,
                live_bytes,
                cancel,
            )
        },
        |index, candidate, segments, work, outside_owned_bytes| {
            let current = span_at(sections, index as u32);
            let base = support_native_points
                + bonds::skin_bridge_points(&candidate.skin_bridge_domains)
                + bonds::skin_command_fit_points(&candidate.skin_command_fits)
                + candidate
                    .paths
                    .iter()
                    .map(|p| p.points_mm.capacity())
                    .sum::<usize>();
            let candidate_owned_bytes = geometry_bytes(&[
                bonds::path_bytes(&candidate.paths),
                bonds::path_bytes(&candidate.whole_rows),
                bonds::skin_bridge_bytes(&candidate.skin_bridge_domains),
                bonds::skin_command_fit_bytes(&candidate.skin_command_fits),
            ])?;
            bonds::construct(
                &mut candidate.process_interfaces,
                &sections[current].process_domains,
                &mut candidate.paths,
                schedule[index].z,
                schedule[index].lower_z,
                recipe,
                segments,
                classification::fixed_point_budget(
                    base,
                    geometry_bytes(&[outside_owned_bytes, candidate_owned_bytes])?,
                ),
                work,
                cancel,
            )
        },
    )
}
