// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::{SectionSpan, boolean_budget, region, span_at};
use crate::{check_cancel, error};
use geo::algorithm::buffer::{BufferStyle, LineJoin};
use geo::{Area, BooleanOps, BoundingRect, Buffer, MultiPolygon, Polygon};
use spiling_contracts::manufacturing::*;
use std::{
    rc::{Rc, Weak},
    sync::atomic::AtomicBool,
};
mod anchors;
pub(super) use anchors::inward_bond_endpoint;
pub(crate) use anchors::strict_capsule_point;
pub(crate) use anchors::{
    AnchorCandidate, AnchorWorkspace, PreparedAnchorPaths, endpoint_bond, projected_edges,
};
pub(in crate::planner) mod strokes;
pub(crate) use strokes::StrokeRuns;
pub(crate) use strokes::{GapKind, gap_kind};
#[cfg(test)]
mod contact_extensions;
mod proximity;
mod raster;

struct Demand {
    target: usize,
    failure: strokes::Failure,
}

pub(super) struct RasterCandidate {
    pub(super) paths: Vec<DepositionPath>,
    pub(super) whole_rows: Vec<DepositionPath>,
    pub(super) process_interfaces: Vec<ProcessInterfaceCoverage>,
    pub(super) skin_bridge_domains: Vec<SkinBridgeDomain>,
    pub(super) skin_command_fits: Vec<SkinCommandFitDomain>,
}

fn authored_component_axis(
    candidate: &RasterCandidate,
    index: u32,
    recipe: &PlanarPrintRecipe,
) -> region::HatchAxis {
    let top = !candidate.paths.is_empty()
        && candidate.paths.iter().all(|path| {
            path.owner
                == Some(DepositionOwner::MandatorySkin {
                    skin: ProcessSkinKind::TopSkin,
                })
        });
    region::HatchAxis::for_policy(
        if top {
            recipe.top_skin_axis
        } else {
            recipe.rectilinear_axis
        },
        index,
    )
}

pub(super) fn component_admission<'a>(
    (candidate, axis, budget, contact_bridge): (
        &mut RasterCandidate,
        region::HatchAxis,
        usize,
        bool,
    ),
    previous: impl Iterator<Item = &'a DepositionPath> + Clone,
    contacts: impl Iterator<Item = &'a DepositionPath> + Clone,
    recipe: &PlanarPrintRecipe,
    anchors: &mut AnchorWorkspace,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<super::classification::ComponentAnchorAdmission, ManufacturingError> {
    // One borrowed lower set for every endpoint, row and stroke query below.
    let mut lower = strokes::LowerStrokes::new(previous.clone(), contacts.clone());
    for path in &mut candidate.paths {
        let Some(DepositionOwner::RegionalSkinBridge { skin_first, .. }) = path.owner else {
            continue;
        };
        let [a, transition, b] = path.points_mm.as_slice() else {
            return Err(error(
                ManufacturingErrorCode::InvalidSpecification,
                "mixed Bridge lost its sole transition",
            ));
        };
        let (a, transition, b) = (*a, *transition, *b);
        let along = if a[0] == b[0] { 1 } else { 0 };
        let t = (transition[along] - a[along]) / (b[along] - a[along]);
        anchors.set_budget(budget);
        let mut endpoint = None;
        for gap in lower.gaps(
            AnchorCandidate {
                endpoints_mm: [a, b],
                bead_width_mm: region::constant_width(path)?,
            },
            path.physical_z_mm[0],
            false,
            anchors,
            work,
            cancel,
        )? {
            if gap.start < t && t < gap.end {
                let parameter = if skin_first && gap.left {
                    Some(gap.start)
                } else if !skin_first && gap.right {
                    Some(gap.end)
                } else {
                    None
                };
                endpoint = parameter.map(|parameter| {
                    std::array::from_fn(|axis| a[axis] + (b[axis] - a[axis]) * parameter)
                });
                break;
            }
        }
        if let Some(endpoint) = endpoint {
            path.points_mm[if skin_first { 0 } else { 2 }] = endpoint;
        }
    }
    if !candidate.whole_rows.is_empty() {
        let owned = candidate
            .paths
            .iter()
            .chain(&candidate.whole_rows)
            .map(|path| path.points_mm.capacity())
            .sum::<usize>();
        let retained = (super::MAX_RETAINED_GEOMETRY_UNITS)
            .checked_sub(budget)
            .and_then(|total| total.checked_sub(owned))
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "corridor raster retention accounting mismatch",
                )
            })?;
        coalesce_bridge_rows_with(
            candidate,
            BridgeRowContext {
                axis,
                demanded_reorientation: contact_bridge,
                retained_points: retained,
                owned_bytes: 0,
                recipe,
            },
            previous.clone(),
            contacts.clone(),
            &mut lower,
            anchors,
            work,
            cancel,
        )?;
    }
    let paths = &candidate.paths;
    let query_budget = (budget / 2).max(anchors.retained_records());
    let witness_budget = budget.checked_sub(query_budget).ok_or_else(|| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "live component anchor workspace exceeds retention",
        )
    })?;
    let mut proof = super::classification::ComponentAnchorAdmission {
        supported: true,
        bridges: Vec::new(),
        demands: Vec::new(),
        workspace_points: anchors.retained_records(),
        workspace_bytes: anchors.retained_bytes(),
    };
    for (index, path) in paths.iter().enumerate() {
        check_cancel(cancel)?;
        if path.kind == DepositionKind::Perimeter {
            continue;
        }
        if path.points_mm.len() != 2
            && !matches!(path.owner, Some(DepositionOwner::RegionalSkinBridge { .. }))
        {
            return Err(error(
                ManufacturingErrorCode::InvalidSpecification,
                "component hatch is not a straight stroke",
            ));
        }
        let a = path.points_mm[0];
        let b = *path.points_mm.last().expect("validated component");
        let length = (b[0] - a[0]).hypot(b[1] - a[1]);
        anchors.set_budget(query_budget);
        let mut bridge = false;
        let mut demand = false;
        for gap in lower.gaps(
            AnchorCandidate {
                endpoints_mm: [a, b],
                bead_width_mm: region::constant_width(path)?,
            },
            path.physical_z_mm[0],
            true,
            anchors,
            work,
            cancel,
        )? {
            match strokes::gap_kind(
                gap,
                (gap.end - gap.start) * length,
                region::constant_width(path)?,
                recipe.process.max_bridge_span_mm,
            ) {
                strokes::GapKind::Supported => {}
                // Only an explicit demand-rooted orthogonal construction may
                // use a real bounded gap ending on eligible support contacts.
                strokes::GapKind::Contact => {
                    bridge |= contact_bridge || path.kind == DepositionKind::Bridge;
                }
                strokes::GapKind::Bridge => bridge = true,
                strokes::GapKind::Demand => {
                    demand = true;
                    reserve_component_record(
                        &mut proof.demands,
                        2,
                        proof.bridges.capacity(),
                        witness_budget,
                    )?;
                    proof.demands.push([
                        std::array::from_fn(|axis| a[axis] + (b[axis] - a[axis]) * gap.start),
                        std::array::from_fn(|axis| a[axis] + (b[axis] - a[axis]) * gap.end),
                    ]);
                }
            }
        }
        if demand {
            proof.supported = false;
        } else if bridge {
            reserve_component_record(
                &mut proof.bridges,
                1,
                proof.demands.capacity() * 2,
                witness_budget,
            )?;
            proof.bridges.push(index);
        }
    }
    proof.workspace_points = anchors.retained_records();
    proof.workspace_bytes = anchors.retained_bytes();
    Ok(proof)
}

pub(super) fn component_budget(
    retained: usize,
    owned_bytes: usize,
    scratch: usize,
    scratch_owned_bytes: usize,
) -> Result<usize, ManufacturingError> {
    workspace_budget(
        retained.saturating_add(scratch),
        owned_bytes.saturating_add(scratch_owned_bytes),
    )
}

fn stroke_proof_reservation(paths: usize) -> usize {
    // The existing stroke analyzer grows only a Vec<usize> of bridge indices.
    // Reserve its old/new growth arrays; use actual capacity once returned.
    if paths == 0 {
        0
    } else {
        paths
            .saturating_mul(3)
            .max(4)
            .saturating_mul(std::mem::size_of::<usize>())
    }
}

fn workspace_budget(units: usize, bytes: usize) -> Result<usize, ManufacturingError> {
    let remaining = super::remaining_geometry_units(units, bytes)?;
    // Reserve both old/new workspace arrays during growth. These record
    // reservations are separate from measured persistent payload receipts.
    let stride = 2 * std::mem::size_of::<anchors::AnchorGap>().max(std::mem::size_of::<(
        f64,
        f64,
        f64,
        usize,
    )>());
    Ok(remaining.min((MAX_MANUFACTURING_RETAINED_BYTES as usize - bytes) / stride))
}

fn reserve_component_record<T>(
    values: &mut Vec<T>,
    units: usize,
    other: usize,
    budget: usize,
) -> Result<(), ManufacturingError> {
    if values.len() < values.capacity() {
        return Ok(());
    }
    let limit = budget.saturating_sub(other) / units;
    let capacity = values.capacity().saturating_mul(2).max(4).min(limit);
    if capacity <= values.len() {
        return Err(error(
            ManufacturingErrorCode::ResourceLimit,
            "component anchor witness retention exceeded",
        ));
    }
    values
        .try_reserve_exact(capacity - values.len())
        .map_err(|_| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "component anchor witness allocation refused",
            )
        })
}

pub(super) struct ProtectedVoidSet {
    pub regions: MultiPolygon<f64>,
    pub scopes: Vec<spiling_geometry::ProtectedVoidHeightScope>,
    /// Truncated retries only: projected voids outside every demand this
    /// pass saw. A complete pass may include them through higher demands.
    pub unclaimed: Vec<(Polygon<f64>, spiling_geometry::ProtectedVoidHeightScope)>,
}

/// Admission inputs shared by whole-row ownership and deposited-anchor proof.
struct BridgeRowContext<'a> {
    axis: region::HatchAxis,
    demanded_reorientation: bool,
    retained_points: usize,
    owned_bytes: usize,
    recipe: &'a PlanarPrintRecipe,
}

/// Exact payload receipt captured before worker-local sharing.
/// The Cell is temporary inventory state; these owners never cross threads.
struct OwnedMaterial {
    material: MultiPolygon<f64>,
    bytes: usize,
    units: usize,
    header_bytes: usize,
    counted: std::cell::Cell<bool>,
}

impl OwnedMaterial {
    fn new(mut material: MultiPolygon<f64>) -> Self {
        let bytes = super::native::material_bytes(&mut material);
        let units = material_units(&material);
        Self {
            material,
            bytes,
            units,
            header_bytes: 0,
            counted: std::cell::Cell::new(false),
        }
    }

    fn into_shared(mut self) -> Rc<Self> {
        self.header_bytes = std::mem::size_of::<Self>() + 2 * std::mem::size_of::<usize>();
        Rc::new(self)
    }
}

impl std::ops::Deref for OwnedMaterial {
    type Target = MultiPolygon<f64>;
    fn deref(&self) -> &Self::Target {
        &self.material
    }
}

/// One union for an incoming owner and the fixed column at this cache's scope.
/// The weak source key must not keep an old contour array alive.
struct SharedUnion {
    source: Weak<OwnedMaterial>,
    result: Rc<OwnedMaterial>,
}

// Identical fixed-XY columns share immutable contours between layers. Reuse a
// union for identical incoming owners; a weak key retains no old point arrays.
// Each cache is scoped to one fixed immutable `other` column below.
#[expect(
    clippy::too_many_arguments,
    reason = "Column union must account both shared owners, its weak cache and foreign work before replacement"
)]
fn merge_shared(
    into: &mut Rc<OwnedMaterial>,
    other: &Rc<OwnedMaterial>,
    cached: &mut Option<SharedUnion>,
    live_units: usize,
    live_bytes: usize,
    work: &mut usize,
    foreign_work: usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    if other.0.is_empty() {
        return Ok(());
    }
    if into.0.is_empty() {
        *into = Rc::clone(other);
        return Ok(());
    }
    if let Some(cached) = cached
        && cached.source.as_ptr() == Rc::as_ptr(into)
    {
        *into = Rc::clone(&cached.result);
        return Ok(());
    }
    charge_boolean(into, other, work, foreign_work, cancel)?;
    let mut result = into.material.union(&other.material);
    region::check_size(&result)?;
    region::canonicalize(&mut result);
    check_cancel(cancel)?;
    let result = OwnedMaterial::new(result);
    // Stock overlay output is opaque until returned. Admit all known old/new
    // owners before allocating its Rc header or publishing either reference.
    super::remaining_geometry_units(
        live_units.saturating_add(result.units),
        live_bytes.saturating_add(result.bytes).saturating_add(
            std::mem::size_of::<OwnedMaterial>() + 2 * std::mem::size_of::<usize>(),
        ),
    )?;
    let result = result.into_shared();
    *cached = Some(SharedUnion {
        source: Rc::downgrade(into),
        result: Rc::clone(&result),
    });
    *into = result;
    Ok(())
}

/// Realized object geometry and the retained native ownership used by support.
pub(super) struct SupportInput<'a> {
    pub(super) sections: &'a [SectionSpan],
    pub(super) schedule: &'a [super::LayerElevation],
    pub(super) native_points: usize,
    /// Outside owners, including the frozen cache and layer record array,
    /// but excluding every layer's mutable path and coverage payloads. The
    /// projection callback refreshes the native proof component after compaction.
    pub(super) native_owned_bytes: usize,
    pub(super) recipe: &'a PlanarPrintRecipe,
    pub(super) source_z_allowance_mm: f64,
}

#[derive(Default)]
pub(super) struct SupportWork {
    pub(super) planar: usize,
    /// Support-owned subset of planar work, excluding object raster/skins.
    /// This receipt survives every compile-local schedule retry.
    pub(super) support_owned: usize,
    pub(super) anchors: usize,
    /// Deposition Z (bits) of layers the schedule can refine no further.
    /// Their unanchored strokes print as authored and are reported, the
    /// build-plate-only outcome, instead of requesting refinement again.
    pub(super) exhausted: std::collections::BTreeSet<u64>,
}

impl SupportWork {
    fn foreign_work(&self) -> Result<usize, ManufacturingError> {
        self.planar.checked_sub(self.support_owned).ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "support work-domain accounting mismatch",
            )
        })
    }

    fn finish(
        &mut self,
        planar: usize,
        foreign: usize,
        anchors: usize,
    ) -> Result<(), ManufacturingError> {
        self.planar = planar;
        self.anchors = anchors;
        self.support_owned = planar.checked_sub(foreign).ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "support work-domain publication mismatch",
            )
        })?;
        Ok(())
    }
}

// Exclude foreign work from the local policy only. The actual global counter
// is never reset, offset, refunded, or charged a second time.
fn exclude_object_raster_work(
    foreign: &mut usize,
    before: usize,
    after: usize,
) -> Result<(), ManufacturingError> {
    *foreign = after
        .checked_sub(before)
        .and_then(|charged| foreign.checked_add(charged))
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "object raster work-domain accounting mismatch",
            )
        })?;
    Ok(())
}

pub(super) enum SupportAdmission {
    Complete,
    /// A truncated retry cannot decide what the complete pass would.
    Undecided,
    RefineUnreachableLayer {
        index: usize,
        failure: ManufacturingError,
    },
}

// Anchors come only from actual deposited capsules. Corridor exclusion consumes
// every lower slab's continuous-target outer occupancy, separately from the
// sampled/scheduled D used by walls and physical-distance skin classification.
#[expect(
    clippy::too_many_arguments,
    reason = "Support orchestration keeps native void projection, raster generation and interface finalization as distinct callbacks"
)]
pub(super) fn realize(
    layers: &mut [PrintLayer],
    input: &SupportInput<'_>,
    segments: &mut usize,
    cancel: &AtomicBool,
    native_work: &mut spiling_geometry::GeometryWorkBudget,
    tentative: bool,
    truncated: bool,
    work_bank: &mut SupportWork,
    mut raster: impl FnMut(
        usize,
        region::HatchAxis,
        &mut usize,
        usize,
        usize,
        usize,
        &mut usize,
        Option<&mut super::classification::ComponentAnchorCheck<'_>>,
    ) -> Result<RasterCandidate, ManufacturingError>,
    mut project_voids: impl FnMut(
        &[[[f64; 2]; 2]],
        usize,
        // Complete live bytes, followed by mutable native unit/byte receipts.
        usize,
        &mut usize,
        &mut usize,
        &mut usize,
        &mut spiling_geometry::GeometryWorkBudget,
    ) -> Result<ProtectedVoidSet, ManufacturingError>,
    mut finalize_interface: impl FnMut(
        usize,
        &mut RasterCandidate,
        &mut usize,
        &mut usize,
        usize,
    ) -> Result<bool, ManufacturingError>,
) -> Result<SupportAdmission, ManufacturingError> {
    let mut work = work_bank.planar;
    let mut anchor_work = work_bank.anchors;
    let mut foreign_work = work_bank.foreign_work()?;
    let sup_clock = std::time::Instant::now();
    let outcome = (|| {
        let SupportInput {
            sections,
            schedule,
            mut native_points,
            mut native_owned_bytes,
            recipe,
            source_z_allowance_mm,
        } = *input;
        let width = recipe.bead_width_mm;
        let mut demands: Vec<Demand> = Vec::new();
        let mut constructed_points = layers.iter().try_fold(0usize, |total, layer| {
            total
                .checked_add(super::bonds::skin_command_fit_points(
                    &layer.skin_command_fits,
                ))
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "initial skin command fit accounting overflow",
                    )
                })
        })?;
        let mut retained = native_points
            .checked_add(constructed_points)
            .filter(|n| *n <= super::MAX_RETAINED_GEOMETRY_UNITS)
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "initial skin command fit retained budget exceeded",
                )
            })?;
        let mut anchors = AnchorWorkspace::default();
        super::remaining_geometry_units(
            retained,
            native_owned_bytes
                .saturating_add(layer_payload_bytes(layers.iter()))
                .saturating_add(layers.len().saturating_mul(std::mem::size_of::<bool>())),
        )?;
        let mut joined_layers = vec![false; layers.len()];
        let mut original_bridge_roles = Vec::new();
        let mut owned_bytes = native_owned_bytes
            .saturating_add(layer_payload_bytes(layers.iter()))
            .saturating_add(joined_layers.capacity() * std::mem::size_of::<bool>());
        anchors.set_budget(workspace_budget(retained, owned_bytes)?);
        if let Some(first) = layers.first_mut()
            && !finalize_layer_interface(
                0,
                first,
                segments,
                &mut work,
                owned_bytes.saturating_add(anchors.retained_bytes()),
                &mut finalize_interface,
            )?
        {
            return Err(super::bonds::unresolved(first));
        }
        for index in 1..layers.len() {
            owned_bytes = native_owned_bytes
                + layer_payload_bytes(layers.iter())
                + vec_bytes(&joined_layers)
                + vec_bytes(&demands)
                + vec_bytes(&original_bridge_roles);
            anchors.set_budget(workspace_budget(retained, owned_bytes)?);
            let (prior, remaining) = layers.split_at_mut(index);
            let (layer, following) = remaining.split_first_mut().expect("current support layer");
            check_cancel(cancel)?;
            let mut interface_bound = finalize_layer_interface(
                index,
                layer,
                segments,
                &mut work,
                owned_bytes.saturating_add(anchors.retained_bytes()),
                &mut finalize_interface,
            )?;
            owned_bytes = native_owned_bytes
                + layer_payload_bytes(
                    prior
                        .iter()
                        .chain(std::iter::once(&*layer))
                        .chain(following.iter()),
                )
                + vec_bytes(&joined_layers)
                + vec_bytes(&demands)
                + vec_bytes(&original_bridge_roles);
            anchors.set_budget(workspace_budget(retained, owned_bytes)?);
            let authored_axis = region::HatchAxis::for_policy(recipe.rectilinear_axis, layer.index);
            let previous = prior[index - 1].paths.iter();
            anchors.set_budget(workspace_budget(
                retained,
                owned_bytes.saturating_add(stroke_proof_reservation(layer.paths.len())),
            )?);
            let mut admission = strokes::analyze(
                &layer.paths,
                previous.clone(),
                std::iter::empty(),
                recipe,
                &mut anchors,
                &mut anchor_work,
                cancel,
            )?;
            if admission.failure.is_some() || !interface_bound {
                // Preserve ordinary authored hatch; only a finite demanded role
                // component may acquire an independently proved Bridge construction.
                let without_old = *segments - path_segments(&layer.paths);
                for recover_components in [false, true] {
                    let axis = authored_axis;
                    let mut staged_segments = without_old;
                    anchors.set_budget(0);
                    let extra_owned_bytes = layer_payload_bytes(
                        prior
                            .iter()
                            .chain(std::iter::once(&*layer))
                            .chain(following.iter()),
                    ) + vec_bytes(&joined_layers)
                        + vec_bytes(&demands)
                        + vec_bytes(&original_bridge_roles)
                        + vec_bytes(&admission.bridges);
                    owned_bytes = native_owned_bytes.saturating_add(extra_owned_bytes);
                    let staged_anchor_bytes = anchors.retained_bytes();
                    let mut check_component =
                        |candidate: &mut RasterCandidate,
                         axis,
                         scratch: usize,
                         scratch_owned_bytes: usize| {
                            let contact_bridge =
                                axis != authored_component_axis(candidate, index as u32, recipe);
                            let budget = component_budget(
                                retained,
                                owned_bytes,
                                scratch.saturating_add(candidate_units(candidate)),
                                scratch_owned_bytes
                                    .saturating_add(candidate_owned_bytes(candidate)),
                            )?;
                            component_admission(
                                (candidate, axis, budget, contact_bridge),
                                previous.clone(),
                                std::iter::empty(),
                                recipe,
                                &mut anchors,
                                &mut anchor_work,
                                cancel,
                            )
                        };
                    let recovery: Option<&mut super::classification::ComponentAnchorCheck<'_>> =
                        if recover_components {
                            Some(&mut check_component)
                        } else {
                            None
                        };
                    let raster_before = work;
                    let candidate = raster(
                        index,
                        axis,
                        &mut staged_segments,
                        constructed_points,
                        extra_owned_bytes,
                        staged_anchor_bytes,
                        &mut work,
                        recovery,
                    );
                    exclude_object_raster_work(&mut foreign_work, raster_before, work)?;
                    let mut candidate = candidate?;
                    let joined = coalesce_bridge_rows(
                        &mut candidate,
                        BridgeRowContext {
                            axis,
                            demanded_reorientation: false,
                            retained_points: retained,
                            owned_bytes,
                            recipe,
                        },
                        previous.clone(),
                        std::iter::empty(),
                        &mut anchors,
                        &mut anchor_work,
                        cancel,
                    )?;
                    let candidate_bound = finalize_interface(
                        index,
                        &mut candidate,
                        &mut staged_segments,
                        &mut work,
                        owned_bytes.saturating_add(anchors.retained_bytes()),
                    )?;
                    if !candidate_bound {
                        if recover_components && !interface_bound {
                            // Diagnostic only: an unresolved layer always refuses
                            // before it can become the next layer's anchor authority.
                            layer.paths = candidate.paths;
                            layer.region_coverage.process_interfaces = candidate.process_interfaces;
                            {
                                replace_skin_command_fits(
                                    &mut layer.skin_command_fits,
                                    candidate.skin_command_fits,
                                    &mut retained,
                                    &mut constructed_points,
                                )?;
                                replace_skin_bridge_domains(
                                    &mut layer.region_coverage,
                                    candidate.skin_bridge_domains,
                                    &mut retained,
                                    &mut constructed_points,
                                )?;
                            };
                        }
                        continue;
                    }
                    anchors.set_budget(candidate_budget(
                        &candidate.paths,
                        retained,
                        owned_bytes
                            .saturating_add(candidate_aux_bytes(&candidate))
                            .saturating_add(stroke_proof_reservation(candidate.paths.len())),
                    )?);
                    let other = strokes::analyze(
                        &candidate.paths,
                        previous.clone(),
                        std::iter::empty(),
                        recipe,
                        &mut anchors,
                        &mut anchor_work,
                        cancel,
                    )?;
                    let complete = other.failure.is_none();
                    // Same-axis joins preserve every required piece and have their
                    // own direct-anchor proof. Keep that monotone improvement even
                    // when another stroke still needs support; do not demand a
                    // corridor for fragments that we can already anchor.
                    let component_bridges = candidate
                        .paths
                        .iter()
                        .any(|path| path.kind == DepositionKind::Bridge);
                    if complete || joined || component_bridges {
                        *segments = without_old + path_segments(&candidate.paths);
                        layer.paths = candidate.paths;
                        layer.region_coverage.process_interfaces = candidate.process_interfaces;
                        {
                            replace_skin_command_fits(
                                &mut layer.skin_command_fits,
                                candidate.skin_command_fits,
                                &mut retained,
                                &mut constructed_points,
                            )?;
                            replace_skin_bridge_domains(
                                &mut layer.region_coverage,
                                candidate.skin_bridge_domains,
                                &mut retained,
                                &mut constructed_points,
                            )?;
                        };
                        interface_bound = true;
                        joined_layers[index] = joined || component_bridges;
                        admission = other;
                        if complete {
                            break;
                        }
                    }
                }
                owned_bytes = native_owned_bytes
                    + layer_payload_bytes(
                        prior
                            .iter()
                            .chain(std::iter::once(&*layer))
                            .chain(following.iter()),
                    )
                    + vec_bytes(&joined_layers)
                    + vec_bytes(&demands)
                    + vec_bytes(&original_bridge_roles);
                anchors.set_budget(workspace_budget(retained, owned_bytes)?);
            }
            if !interface_bound {
                return Err(super::bonds::unresolved(layer));
            }
            // Permanent construction and its first upper landing may never be
            // repaired by temporary support, even when unrelated support is enabled.
            for path in &layer.paths {
                let Some(DepositionOwner::Regional { region_index }) = path.owner else {
                    continue;
                };
                let plate_owned = layer
                    .region_coverage
                    .process_diaphragms
                    .iter()
                    .any(|plate| plate.region_index == region_index);
                let landing_owned = prior[index - 1]
                    .region_coverage
                    .process_diaphragms
                    .iter()
                    .filter(|plate| plate.physical_z_mm[1] == layer.lower_z_mm)
                    .any(|plate| {
                        layer
                            .region_coverage
                            .process_regions
                            .get(usize::from(region_index))
                            .is_some_and(|region| {
                                region.contributions.iter().any(|source| {
                                    source.occurrence_id == plate.occurrence_id
                                        && source.definition_id == plate.definition_id
                                })
                            })
                    });
                if plate_owned || landing_owned {
                    anchors.set_budget(workspace_budget(
                        retained,
                        owned_bytes
                            .saturating_add(vec_bytes(&admission.bridges))
                            .saturating_add(stroke_proof_reservation(1)),
                    )?);
                    let proof = strokes::analyze(
                        std::slice::from_ref(path),
                        previous.clone(),
                        std::iter::empty(),
                        recipe,
                        &mut anchors,
                        &mut anchor_work,
                        cancel,
                    )?;
                    if proof.failure.is_some() {
                        return Err(error(
                            ManufacturingErrorCode::UnsupportedGeometry,
                            format!(
                                "UnsupportedInterface: permanent diaphragm or upper landing at layer {} Z={} mm lacks actual lower anchors; temporary support is not authorized",
                                layer.index, layer.z_mm
                            ),
                        ));
                    }
                }
            }
            if let Some(failure) = admission.failure.take() {
                let exhausted = work_bank.exhausted.contains(&layer.z_mm.to_bits());
                if matches!(recipe.supports, SupportPolicy::Disabled) || exhausted {
                    if tentative && index > 0 && !exhausted {
                        let failure =
                            admission_error(index, layer.z_mm, &layer.paths, &failure, recipe);
                        return Ok(SupportAdmission::RefineUnreachableLayer { index, failure });
                    }
                    // Supports are off, or no finer schedule can anchor or
                    // support this layer: print the overhang as PrusaSlicer
                    // does and report it, rather than refuse the part.
                    layer.region_coverage.unsupported_overhang_mm = layer
                        .region_coverage
                        .unsupported_overhang_mm
                        .max(failure.span);
                } else {
                    let live = native_owned_bytes
                        + layer_payload_bytes(
                            prior
                                .iter()
                                .chain(std::iter::once(&*layer))
                                .chain(following.iter()),
                        )
                        + vec_bytes(&joined_layers)
                        + vec_bytes(&demands)
                        + vec_bytes(&original_bridge_roles)
                        + anchors.retained_bytes()
                        + vec_bytes(&admission.bridges);
                    reserve_owned(&mut demands, 1, live)?;
                    demands.push(Demand {
                        target: index,
                        failure,
                    });
                }
            }
            let live = native_owned_bytes
                + layer_payload_bytes(
                    prior
                        .iter()
                        .chain(std::iter::once(&*layer))
                        .chain(following.iter()),
                )
                + vec_bytes(&joined_layers)
                + vec_bytes(&demands)
                + vec_bytes(&original_bridge_roles)
                + anchors.retained_bytes()
                + vec_bytes(&admission.bridges);
            reserve_owned(&mut original_bridge_roles, admission.bridges.len(), live)?;
            for path in admission.bridges {
                original_bridge_roles.push((index, path, layer.paths[path].kind));
                layer.paths[path].kind = DepositionKind::Bridge;
            }
        }
        super::trace_stage("sup_strokes", sup_clock);
        if demands.is_empty() {
            return Ok(SupportAdmission::Complete);
        }
        anchors.set_budget(0);
        let SupportPolicy::Breakaway {
            density,
            xy_clearance_mm,
            z_gap_mm,
            interface_thickness_mm,
        } = recipe.supports
        else {
            unreachable!("disabled support demand refused above")
        };
        // The gap is measured against the sliced object layers, which ADR012
        // places within the recorded tolerance of the BREP; widening it by the
        // whole enclosure margin would drop a full layer below every contact.
        let selection_gap_mm = z_gap_mm;
        if tentative {
            for demand in &demands {
                let path = &layers[demand.target].paths[demand.failure.path];
                if contact_layer(
                    layers,
                    demand.target,
                    path,
                    selection_gap_mm,
                    &mut work,
                    cancel,
                )?
                .is_none()
                {
                    return Ok(SupportAdmission::RefineUnreachableLayer {
                        index: demand.target,
                        failure: unreachable_layer_error(layers, demand, z_gap_mm)?,
                    });
                }
            }
        }
        // Native all-Z void branches are projected only after actual strokes prove
        // demand. Scheduled hole samples cannot certify a protected projection.
        owned_bytes = native_owned_bytes
            + layer_payload_bytes(layers.iter())
            + vec_bytes(&joined_layers)
            + vec_bytes(&demands)
            + vec_bytes(&original_bridge_roles)
            + anchors.retained_bytes();
        super::remaining_geometry_units(
            retained,
            owned_bytes.saturating_add(
                demands
                    .len()
                    .saturating_mul(std::mem::size_of::<[[f64; 2]; 2]>()),
            ),
        )?;
        let mut demand_bounds = Vec::with_capacity(demands.len());
        owned_bytes = owned_bytes.saturating_add(vec_bytes(&demand_bounds));
        // Enclose the half-bead placement search with the existing offset
        // construction allowance before querying any nearby protected void.
        // Query every forbidden mask that can meet the proposed full bead and
        // its clearance, including tolerant source enclosure outside raw bounds.
        let search_bound = (width / 2.0
            + xy_clearance_mm
            + source_z_allowance_mm
            + super::OFFSET_TOLERANCE_MM * 0.5)
            .next_up();
        for demand in &demands {
            check_cancel(cancel)?;
            let candidate = &sections[span_at(sections, demand.target as u32)].material;
            let bounds = candidate.bounding_rect().ok_or_else(|| {
                error(
                    ManufacturingErrorCode::NativeAnalysisIndeterminate,
                    "actual support demand has no material footprint bounds",
                )
            })?;
            demand_bounds.push([
                [
                    (bounds.min().x - search_bound).next_down(),
                    (bounds.max().x + search_bound).next_up(),
                ],
                [
                    (bounds.min().y - search_bound).next_down(),
                    (bounds.max().y + search_bound).next_up(),
                ],
            ]);
        }
        let native_points_before = native_points;
        let native_bytes_before = native_owned_bytes;
        super::trace_stage("sup_pre_voids", sup_clock);
        let projected = project_voids(
            &demand_bounds,
            retained,
            owned_bytes,
            &mut native_points,
            &mut native_owned_bytes,
            &mut work,
            native_work,
        )?;
        retained = retained
            .checked_sub(native_points_before)
            .and_then(|n| n.checked_add(native_points))
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "native void proof-unit handoff accounting mismatch",
                )
            })?;
        owned_bytes = owned_bytes
            .checked_sub(native_bytes_before)
            .and_then(|n| n.checked_add(native_owned_bytes))
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "native void owner handoff accounting mismatch",
                )
            })?;
        super::remaining_geometry_units(retained, owned_bytes)?;
        let ProtectedVoidSet {
            regions: protected,
            scopes,
            unclaimed,
        } = projected;
        // Truncated-retry gate operands stay live through the demand loop.
        native_owned_bytes = unclaimed.iter().fold(native_owned_bytes, |bytes, (mask, _)| {
            bytes.saturating_add(
                (mask.exterior().0.capacity() * std::mem::size_of::<geo::Coord<f64>>())
                    .saturating_add(std::mem::size_of::<(Polygon<f64>, spiling_geometry::ProtectedVoidHeightScope)>()),
            )
        });
        let protected = OwnedMaterial::new(protected);
        if protected.0.len() != scopes.len() {
            return Err(error(
                ManufacturingErrorCode::NativeAnalysisIndeterminate,
                "native protected void scopes do not match their outer masks",
            ));
        }
        region::check_size(&protected)?;
        let layer_metadata_bytes = layers.len().saturating_mul(
            2 * std::mem::size_of::<Rc<OwnedMaterial>>()
                + std::mem::size_of::<usize>()
                + std::mem::size_of::<Option<usize>>(),
        );
        super::remaining_geometry_units(
            retained,
            owned_bytes
                .saturating_add(vec_bytes(&scopes))
                .saturating_add(protected.bytes)
                .saturating_add(layer_metadata_bytes)
                .saturating_add(
                    std::mem::size_of::<OwnedMaterial>() + 2 * std::mem::size_of::<usize>(),
                ),
        )?;
        let empty = OwnedMaterial::new(MultiPolygon(Vec::new())).into_shared();
        let mut support_regions = vec![Rc::clone(&empty); layers.len()];
        let mut interfaces = vec![empty; layers.len()];
        let mut object_start = vec![0; layers.len()];
        let mut contact_tops = vec![None; layers.len()];
        let mut support_metadata_bytes = vec_bytes(&joined_layers)
            + vec_bytes(&demands)
            + vec_bytes(&original_bridge_roles)
            + vec_bytes(&demand_bounds)
            + vec_bytes(&scopes)
            + vec_bytes(&support_regions)
            + vec_bytes(&interfaces)
            + vec_bytes(&object_start)
            + vec_bytes(&contact_tops);
        owned_bytes = native_owned_bytes
            + layer_payload_bytes(layers.iter())
            + support_metadata_bytes
            + anchors.retained_bytes();
        remaining_geometry_budget(
            retained,
            owned_bytes,
            &support_regions,
            &interfaces,
            &[&protected],
        )?;
        // Borrow each final-D owner once. Its nominal bead prism remains an outer
        // exclusion after any later re-hatching; it never supplies an anchor.
        let available = remaining_geometry_budget(
            retained,
            owned_bytes,
            &support_regions,
            &interfaces,
            &[&protected],
        )?;
        if sections.len() > available {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "nominal object scope retention exceeded",
            ));
        }
        super::remaining_geometry_units(
            retained,
            owned_bytes
                .saturating_add(geometry_receipt(&support_regions, &interfaces, &[&protected]).1)
                .saturating_add(
                    sections
                        .len()
                        .saturating_mul(std::mem::size_of::<(usize, f64)>()),
                ),
        )?;
        let mut nominal: Vec<(usize, f64)> = Vec::new();
        nominal.try_reserve_exact(sections.len()).map_err(|_| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "nominal object scope allocation refused",
            )
        })?;
        owned_bytes = owned_bytes.saturating_add(vec_bytes(&nominal));
        for (index, section) in sections.iter().enumerate() {
            check_cancel(cancel)?;
            if section.material.0.is_empty() {
                continue;
            }
            let mut floor: Option<f64> = None;
            // A truncated retry's spans may extend past its output layers.
            let objects = layers
                .get(section.start as usize..(section.end as usize).min(layers.len()))
                .unwrap_or(&[]);
            for object in objects {
                super::native::charge_support_clearance(&mut work, cancel)?;
                if object.paths.is_empty() {
                    continue;
                }
                for path in &object.paths {
                    let lower = program_coordinates::project_xyz6_bead_height_mm(
                        path.physical_z_mm[1],
                        path.physical_z_mm[1] - path.physical_z_mm[0],
                    )?[0];
                    floor = Some(floor.map_or(lower, |value| value.min(lower)));
                }
            }
            if let Some(floor) = floor {
                nominal.push((index, floor));
            }
        }
        super::boolean::charge(
            &mut work,
            nominal.len().checked_mul(nominal.len()).ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "nominal object scope sorting work overflow",
                )
            })?,
        )?;
        nominal.sort_unstable_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
        check_cancel(cancel)?;
        let base_retained = retained;
        let retained = retained
            .checked_add(nominal.capacity())
            .filter(|n| *n <= super::MAX_RETAINED_GEOMETRY_UNITS)
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "nominal object scope retention exceeded",
                )
            })?;
        remaining_geometry_budget(
            retained,
            owned_bytes,
            &support_regions,
            &interfaces,
            &[&protected],
        )?;
        for demand in &demands {
            check_cancel(cancel)?;
            let demanded_path = &layers[demand.target].paths[demand.failure.path];
            let Some(top) = contact_layer(
                layers,
                demand.target,
                demanded_path,
                selection_gap_mm,
                &mut work,
                cancel,
            )?
            else {
                return Err(unreachable_layer_error(layers, demand, z_gap_mm)?);
            };
            let previous_top = contact_tops[demand.target];
            contact_tops[demand.target] = Some(top);
            let interface_bottom = layers[top].z_mm - interface_thickness_mm;
            // Realized object D supplies a conservative support candidate only
            // AFTER actual deposited strokes prove demand; it is never an anchor map.
            // The half-bead search expansion permits a real support bead under an
            // outer edge; subsequent C/nominal-D clipping and actual proofs own it.
            let demanded_section = &sections[span_at(sections, demand.target as u32)];
            let candidate = &demanded_section.material;
            remaining_geometry_budget(
                retained,
                owned_bytes,
                &support_regions,
                &interfaces,
                &[&protected],
            )?;
            charge_boolean(candidate, candidate, &mut work, foreign_work, cancel)?;
            // Beveled corners bound this placement search without constructing a
            // fine round-disk Minkowski sum. Search geometry is never occupied C.
            let mut column = OwnedMaterial::new(
                candidate
                    .buffer_with_style(BufferStyle::new(width / 2.0).line_join(LineJoin::Bevel)),
            );
            check_cancel(cancel)?;
            region::check_size(&column)?;
            let mut proximity = index_column(
                &column,
                retained,
                owned_bytes,
                &support_regions,
                &interfaces,
                &[&protected, &column],
            )?;
            // Certify a fixed-XY corridor all the way to the bed before publishing.
            // Top-down publication before lower clipping leaves floating components.
            for section in &sections[..=span_at(sections, top as u32)] {
                if column.0.is_empty() {
                    break;
                }
                // Independent certified outer operands may overlap. Even-odd
                // normalization would erase their overlap; the staged pieces are
                // instead offset together, whose positive-winding convolution is
                // their union, and that union is subtracted once per section.
                let mut batch = PieceBatch::default();
                // Bounds-only rejection before a piece is constructed. It makes
                // exactly the per-piece decisions below with a distance bound.
                let screen = std::cell::Cell::new(ColumnScreen::new(&column, xy_clearance_mm)?);
                let scanned = std::cell::Cell::new(0usize);
                let bed = layers[0].lower_z_mm;
                let top_z = layers[top].z_mm;
                let screen_piece = |bounds: [[f64; 2]; 2], height: Option<[f64; 2]>| {
                    check_cancel(cancel)?;
                    scanned.set(scanned.get().saturating_add(1));
                    if let Some([lower, upper]) = height
                        && (lower > top_z || upper < bed)
                    {
                        return Ok(false);
                    }
                    Ok(!screen.get().is_some_and(|screen| screen.rejects(bounds)))
                };
                let visited = section.visit_occupied_screened(cancel, screen_piece, |occupied, height| {
                    check_cancel(cancel)?;
                    if column.0.is_empty() {
                        return Ok(false);
                    }
                    if let Some([lower, upper]) = height
                        && (lower > top_z || upper < bed)
                    {
                        return Ok(true);
                    }
                    super::boolean::budget_polygon_accumulated(occupied, &mut work, cancel)?;
                    let Some(distance) =
                        candidate_clearance_distance(occupied, &column, xy_clearance_mm)?
                    else {
                        return Ok(true);
                    };
                    if proximity.separated(occupied, separation_reach(distance)) {
                        return Ok(true);
                    }
                    batch.stage(
                        occupied,
                        retained,
                        owned_bytes.saturating_add(proximity.bytes()),
                        &support_regions,
                        &interfaces,
                        &[&protected, &column],
                    )?;
                    if batch.points >= PieceBatch::FLUSH_POINTS {
                        subtract_batch(
                            &mut batch,
                            &mut column,
                            &mut proximity,
                            xy_clearance_mm,
                            retained,
                            owned_bytes,
                            &support_regions,
                            &interfaces,
                            &protected,
                            &mut work,
                            foreign_work,
                            cancel,
                        )?;
                        screen.set(ColumnScreen::new(&column, xy_clearance_mm)?);
                    }
                    Ok(!column.0.is_empty())
                });
                work = work.checked_add(scanned.get()).ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "cumulative continuous occupied scope work counter overflow",
                    )
                })?;
                visited?;
                subtract_batch(
                    &mut batch,
                    &mut column,
                    &mut proximity,
                    xy_clearance_mm,
                    retained,
                    owned_bytes,
                    &support_regions,
                    &interfaces,
                    &protected,
                    &mut work,
                    foreign_work,
                    cancel,
                )?;
            }
            // A complete pass may also hold voids claimed only by higher
            // demands. Each would be skipped there iff it cannot reach this
            // column, whose bounds only shrink below; otherwise only the
            // complete pass can decide.
            if !column.0.is_empty() {
                for (mask, scope) in &unclaimed {
                    check_cancel(cancel)?;
                    if void_reaches_corridor(scope, layers[0].lower_z_mm, layers[top].z_mm)
                        && candidate_clearance_distance(mask, &column, xy_clearance_mm)?
                            .is_some()
                    {
                        return Ok(SupportAdmission::Undecided);
                    }
                }
            }
            for (mask, scope) in protected.0.iter().zip(&scopes) {
                if column.0.is_empty() {
                    break;
                }
                check_cancel(cancel)?;
                work = work.checked_add(1).ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "cumulative protected-void scope work counter overflow",
                    )
                })?;
                if !void_reaches_corridor(scope, layers[0].lower_z_mm, layers[top].z_mm) {
                    continue;
                }
                super::boolean::budget_polygon_accumulated(mask, &mut work, cancel)?;
                let Some(distance) = candidate_clearance_distance(mask, &column, xy_clearance_mm)?
                else {
                    continue;
                };
                let clearance = OwnedMaterial::new(offset_polygon_owned(
                    mask,
                    distance,
                    retained,
                    owned_bytes,
                    &support_regions,
                    &interfaces,
                    &[&protected, &column],
                    cancel,
                )?);
                remaining_geometry_budget(
                    retained,
                    owned_bytes,
                    &support_regions,
                    &interfaces,
                    &[&protected, &column, &clearance],
                )?;
                charge_boolean(&column, &clearance, &mut work, foreign_work, cancel)?;
                let next = OwnedMaterial::new(column.material.difference(&clearance.material));
                remaining_geometry_budget(
                    retained,
                    owned_bytes,
                    &support_regions,
                    &interfaces,
                    &[&protected, &column, &clearance, &next],
                )?;
                column = next;
                region::check_size(&column)?;
                remaining_geometry_budget(
                    retained,
                    owned_bytes,
                    &support_regions,
                    &interfaces,
                    &[&protected, &column, &clearance],
                )?;
            }
            region::check_size(&column)?;
            if column.0.is_empty() {
                let layer = &layers[demand.target];
                let failure = &demand.failure;
                let path = &layer.paths[failure.path];
                let a = path.points_mm[failure.first];
                let b = path.points_mm[failure.last];
                let previous = &layers[demand.target - 1];
                let nearest_a = nearest_stroke(
                    [a[0], a[1]],
                    previous.paths.iter(),
                    &mut work,
                    foreign_work,
                    cancel,
                )?;
                let nearest_b = nearest_stroke(
                    [b[0], b[1]],
                    previous.paths.iter(),
                    &mut work,
                    foreign_work,
                    cancel,
                )?;
                let unresolved = error(
                    ManufacturingErrorCode::NativeAnalysisIndeterminate,
                    format!(
                        "layer {} Z {:.9}: unresolved continuous outer occupancy/protected geometry cannot certify any bed-anchored support corridor; conservative exclusion is not proof of actual collision; role {:?}, stroke {:?} to {:?}, gap ({:.12},{:.12}) span {:.12}; previous layer {} Z {:.9}, nearest start {:?}, nearest end {:?}; nominal bead width {:.9} mm, XY clearance {:.9} mm, Z gap {:.9} mm, bridge limit {:.9} mm",
                        demand.target,
                        layer.z_mm,
                        path.kind,
                        a,
                        b,
                        failure.start,
                        failure.end,
                        failure.span,
                        previous.index,
                        previous.z_mm,
                        nearest_a,
                        nearest_b,
                        width,
                        xy_clearance_mm,
                        z_gap_mm,
                        recipe.process.max_bridge_span_mm
                    ),
                );
                // A nearby actual lower footprint can justify trying finer
                // physical intervals. It grants no anchor/clearance authority.
                let near = [nearest_a.as_ref(), nearest_b.as_ref()]
                    .into_iter()
                    .all(|stroke| stroke.is_some_and(|stroke| stroke.distance_mm <= 2.0 * width));
                let attached_end = [nearest_a.as_ref(), nearest_b.as_ref()]
                    .into_iter()
                    .flatten()
                    .any(|stroke| stroke.distance_mm < (width + stroke.width_mm) * 0.5);
                if near && attached_end {
                    return Ok(SupportAdmission::RefineUnreachableLayer {
                        index: demand.target,
                        failure: unresolved,
                    });
                }
                // The part blocks every column from the bed (an overhang over
                // its own floor). Support resting on the part is not built
                // yet; print the overhang as authored and report it.
                drop(unresolved);
                contact_tops[demand.target] = previous_top;
                let coverage = &mut layers[demand.target].region_coverage;
                coverage.unsupported_overhang_mm =
                    coverage.unsupported_overhang_mm.max(demand.failure.span);
                continue;
            }
            region::canonicalize(&mut column.material);
            column.bytes = super::native::material_bytes(&mut column.material);
            column.units = material_units(&column.material);
            remaining_geometry_budget(
                retained,
                owned_bytes.saturating_add(
                    std::mem::size_of::<OwnedMaterial>() + 2 * std::mem::size_of::<usize>(),
                ),
                &support_regions,
                &interfaces,
                &[&protected, &column],
            )?;
            let mut column = column.into_shared();
            let mut proximity = index_column(
                &column,
                retained,
                owned_bytes,
                &support_regions,
                &interfaces,
                &[&protected, &column],
            )?;
            let mut shared_union = None;
            let mut next_nominal = 0;
            // A weak source can outlive its last strong geometry owner. Reserve
            // old/new Rc header records across cache replacement, including offsets
            // performed while the previous cache entry is still live.
            let weak_header_reservation =
                2 * (std::mem::size_of::<OwnedMaterial>() + 2 * std::mem::size_of::<usize>());
            owned_bytes = owned_bytes.saturating_add(weak_header_reservation);
            for i in 0..=top {
                check_cancel(cancel)?;
                let upper = program_coordinates::project_xyz6_segment_bead_height_mm(
                    layers[i].z_mm,
                    layers[i].z_mm,
                    layers[i].z_mm - layers[i].lower_z_mm,
                )?[1];
                // Higher portions are subsets of the already bed-anchored lower
                // column. Do not lower/remap its original eligible contact top.
                while let Some(&(index, floor)) = nominal.get(next_nominal) {
                    super::native::charge_support_clearance(&mut work, cancel)?;
                    if program_coordinates::enclosed_vertical_gap_mm(floor, upper)? >= z_gap_mm {
                        break;
                    }
                    for polygon in &sections[index].material.0 {
                        check_cancel(cancel)?;
                        if column.0.is_empty() {
                            break;
                        }
                        super::boolean::budget_polygon_accumulated(polygon, &mut work, cancel)?;
                        let Some(distance) =
                            candidate_clearance_distance(polygon, &column, xy_clearance_mm)?
                        else {
                            continue;
                        };
                        if proximity.separated(polygon, separation_reach(distance)) {
                            continue;
                        }
                        let owned_bytes = owned_bytes.saturating_add(proximity.bytes());
                        let clearance = OwnedMaterial::new(offset_polygon_owned(
                            polygon,
                            distance,
                            retained,
                            owned_bytes,
                            &support_regions,
                            &interfaces,
                            &[&protected, &column],
                            cancel,
                        )?);
                        remaining_geometry_budget(
                            retained,
                            owned_bytes,
                            &support_regions,
                            &interfaces,
                            &[&protected, &column, &clearance],
                        )?;
                        charge_boolean(&column, &clearance, &mut work, foreign_work, cancel)
                            .map_err(|mut e| {
                                e.message = format!(
                                    "support layer {i}, nominal object owner {index}: {}",
                                    e.message,
                                );
                                e
                            })?;
                        let next =
                            OwnedMaterial::new(column.material.difference(&clearance.material));
                        region::check_size(&next)?;
                        remaining_geometry_budget(
                            retained,
                            owned_bytes.saturating_add(
                                std::mem::size_of::<OwnedMaterial>()
                                    + 2 * std::mem::size_of::<usize>(),
                            ),
                            &support_regions,
                            &interfaces,
                            &[&protected, &column, &clearance, &next],
                        )?;
                        column = next.into_shared();
                        shared_union = None;
                        drop(clearance);
                        proximity = index_column(
                            &column,
                            retained,
                            owned_bytes,
                            &support_regions,
                            &interfaces,
                            &[&protected, &column],
                        )?;
                    }
                    next_nominal += 1;
                }
                if column.0.is_empty() {
                    break;
                }
                let (live_units, live_bytes) =
                    geometry_receipt(&support_regions, &interfaces, &[&protected, &column]);
                merge_shared(
                    &mut support_regions[i],
                    &column,
                    &mut shared_union,
                    retained.saturating_add(live_units),
                    owned_bytes.saturating_add(live_bytes),
                    &mut work,
                    foreign_work,
                    cancel,
                )?;
                if layers[i].z_mm > interface_bottom {
                    let (live_units, live_bytes) =
                        geometry_receipt(&support_regions, &interfaces, &[&protected, &column]);
                    merge_shared(
                        &mut interfaces[i],
                        &column,
                        &mut shared_union,
                        retained.saturating_add(live_units),
                        owned_bytes.saturating_add(live_bytes),
                        &mut work,
                        foreign_work,
                        cancel,
                    )?;
                }
                remaining_geometry_budget(
                    retained,
                    owned_bytes,
                    &support_regions,
                    &interfaces,
                    &[&protected, &column],
                )?;
            }
            owned_bytes = owned_bytes.saturating_sub(weak_header_reservation);
        }
        super::trace_stage("sup_columns", sup_clock);
        if truncated {
            // No demand in range refines; only the complete pass decides
            // the rest of the schedule and publishes support.
            return Ok(SupportAdmission::Undecided);
        }
        drop(nominal);
        let mut retained = base_retained;
        // Support centers stay half a bead inside the clipped support region. This
        // gives the nominal bead, not only its centerline, the declared XY clearance.
        for i in 0..layers.len() {
            owned_bytes =
                native_owned_bytes + layer_payload_bytes(layers.iter()) + support_metadata_bytes;
            anchors.set_budget(0);
            let (prior, remaining) = layers.split_at_mut(i);
            let (layer, _following) = remaining.split_first_mut().expect("current support layer");
            let interface = interfaces[i].as_ref();
            let retained_domains =
                retained + geometry_receipt(&support_regions, &interfaces, &[&protected]).0;
            let mut domains = raster::Domains::new(
                &support_regions[i],
                interface,
                Rc::ptr_eq(&support_regions[i], &interfaces[i]),
                width,
                retained_domains,
                owned_bytes.saturating_add(
                    geometry_receipt(&support_regions, &interfaces, &[&protected]).1,
                ),
                anchors.retained_bytes(),
                &mut work,
                foreign_work,
                cancel,
            )?;
            let preferred =
                region::HatchAxis::for_policy(RectilinearAxisPolicy::AlternatingXY, layer.index);
            let mut paths = Vec::new();
            for axis in [preferred, preferred.orthogonal()] {
                anchors.set_budget(0);
                domains.set_anchor_owned_bytes(anchors.retained_bytes());
                paths = domains.paths(
                    interface,
                    density,
                    &region::HatchLayer {
                        roof: None,
                        width_mm: recipe.bead_width_mm,
                        z: layer.z_mm,
                        lower_z: layer.lower_z_mm,
                        height: layer.z_mm - layer.lower_z_mm,
                        axis,
                        top_axis: axis,
                        recipe,
                    },
                    *segments,
                    &mut work,
                    foreign_work,
                    cancel,
                )?;
                // A clearance-clipped proposal can leave an unprintable sliver
                // without carrying any final demand. Omit that proposal, then
                // require actual lower anchors and complete final object/contact
                // admission below; a nonempty polygon is not required material.
                if paths.is_empty() {
                    break;
                }
                if i == 0 {
                    break;
                }
                anchors.set_budget(domains.anchor_budget(&paths)?);
                let failure = raster::failure(
                    &paths,
                    prior[i - 1]
                        .paths
                        .iter()
                        .filter(|path| support_role(path.kind)),
                    recipe,
                    &mut anchors,
                    &mut anchor_work,
                    cancel,
                )?;
                let Some(failure) = failure else {
                    break;
                };
                if axis != preferred {
                    let path = &paths[failure.path];
                    return Err(error(
                        ManufacturingErrorCode::UnsupportedGeometry,
                        format!(
                            "layer {i}: neither orthogonal support/interface raster has bounded deposited anchors; role {:?}, stroke {:?} to {:?}, gap ({:.12},{:.12}) span {:.12}; increase support density or revise support corridor",
                            path.kind,
                            path.points_mm[failure.first],
                            path.points_mm[failure.last],
                            failure.start,
                            failure.end,
                            failure.span,
                        ),
                    ));
                }
                // Release failed paths and anchor records before staging the other
                // axis. Neither attempt changes object or support ownership.
                paths = Vec::new();
            }
            *segments += path_segments(&paths);
            anchors.set_budget(0);
            domains.set_anchor_owned_bytes(anchors.retained_bytes());
            layer.region_coverage.support_nominal_deposition_volume_mm3 =
                paths.iter().try_fold(0.0, |sum, path| {
                    Ok::<_, ManufacturingError>(
                        sum + region::nominal_volume(path, layer.index, recipe)?,
                    )
                })?;
            // Support area describes support actually deposited: a region the
            // raster could not stroke deposits nothing and claims no area.
            layer.region_coverage.support_area_mm2 =
                if layer.region_coverage.support_nominal_deposition_volume_mm3 == 0.0 {
                    0.0
                } else {
                    support_regions[i].unsigned_area()
                };
            object_start[i] = paths.len();
            let live_bytes = domains
                .owned_bytes()
                .saturating_add(super::bonds::path_bytes(&paths));
            reserve_owned(&mut paths, layer.paths.len(), live_bytes)?;
            paths.append(&mut layer.paths);
            layer.paths = paths;
        }
        super::trace_stage("sup_raster", sup_clock);
        // Restore original object roles before reevaluating the actual emitted
        // support/contact strokes. A supported contour is not a fake Bridge.
        support_metadata_bytes =
            support_metadata_bytes.saturating_sub(vec_bytes(&original_bridge_roles));
        for (index, path, kind) in original_bridge_roles {
            layers[index].paths[object_start[index] + path].kind = kind;
        }
        let mut retained_regions =
            retained + geometry_receipt(&support_regions, &interfaces, &[&protected]).0;
        // Raster staging must include these live support/protected domains without
        // counting the caller's object/cache reservation twice.
        let retained_support_points = retained_regions.checked_sub(retained).ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "support retained point accounting underflow",
            )
        })?;
        for index in 1..layers.len() {
            let (prior, remaining) = layers.split_at_mut(index);
            let (layer, following) = remaining.split_first_mut().expect("current support layer");
            let extra_owned_bytes = layer_payload_bytes(
                prior
                    .iter()
                    .chain(std::iter::once(&*layer))
                    .chain(following.iter()),
            ) + support_metadata_bytes
                + geometry_receipt(&support_regions, &interfaces, &[&protected]).1;
            owned_bytes = native_owned_bytes.saturating_add(extra_owned_bytes);
            anchors.set_budget(workspace_budget(retained_regions, owned_bytes)?);
            let authored_axis = region::HatchAxis::for_policy(recipe.rectilinear_axis, layer.index);
            let begin = object_start[index];
            let previous = prior[index - 1].paths.iter();
            let contact_paths = contact_tops[index]
                .map(|top| prior[top].paths.as_slice())
                .unwrap_or(&[]);
            let contact = contact_paths.iter().filter(|path| support_role(path.kind));
            if joined_layers[index] {
                // Other support demands can change the eventual previous footprints.
                // Recreate ownership pieces; never retain a stale composite Bridge.
                let without_old = *segments - path_segments(&layer.paths[begin..]);
                let mut staged_segments = without_old;
                anchors.set_budget(0);
                let extra_owned_bytes = layer_payload_bytes(
                    prior
                        .iter()
                        .chain(std::iter::once(&*layer))
                        .chain(following.iter()),
                ) + support_metadata_bytes
                    + geometry_receipt(&support_regions, &interfaces, &[&protected]).1;
                let staged_anchor_bytes = anchors.retained_bytes();
                let mut check_component =
                    |candidate: &mut RasterCandidate,
                     axis,
                     scratch: usize,
                     scratch_owned_bytes: usize| {
                        let contact_bridge =
                            axis != authored_component_axis(candidate, index as u32, recipe);
                        let budget = component_budget(
                            retained_regions,
                            native_owned_bytes + extra_owned_bytes,
                            scratch.saturating_add(candidate_units(candidate)),
                            scratch_owned_bytes.saturating_add(candidate_owned_bytes(candidate)),
                        )?;
                        component_admission(
                            (candidate, axis, budget, contact_bridge),
                            previous.clone(),
                            contact.clone(),
                            recipe,
                            &mut anchors,
                            &mut anchor_work,
                            cancel,
                        )
                    };
                let raster_before = work;
                let candidate = raster(
                    index,
                    authored_axis,
                    &mut staged_segments,
                    retained_support_points + constructed_points,
                    extra_owned_bytes,
                    staged_anchor_bytes,
                    &mut work,
                    Some(&mut check_component),
                );
                exclude_object_raster_work(&mut foreign_work, raster_before, work)?;
                let mut candidate = candidate?;
                coalesce_bridge_rows(
                    &mut candidate,
                    BridgeRowContext {
                        axis: authored_axis,
                        demanded_reorientation: false,
                        retained_points: retained_regions,
                        owned_bytes: native_owned_bytes + extra_owned_bytes,
                        recipe,
                    },
                    previous.clone(),
                    contact.clone(),
                    &mut anchors,
                    &mut anchor_work,
                    cancel,
                )?;
                if !finalize_interface(
                    index,
                    &mut candidate,
                    &mut staged_segments,
                    &mut work,
                    owned_bytes.saturating_add(anchors.retained_bytes()),
                )? {
                    layer.paths = candidate.paths;
                    layer.region_coverage.process_interfaces = candidate.process_interfaces;
                    {
                        layer.region_coverage.skin_bridge_domains = candidate.skin_bridge_domains;
                        layer.skin_command_fits = candidate.skin_command_fits;
                    };
                    return Err(super::bonds::unresolved(layer));
                }
                *segments = without_old + path_segments(&candidate.paths);
                layer.paths.truncate(begin);
                let live_bytes = native_owned_bytes
                    + support_metadata_bytes
                    + layer_payload_bytes(
                        prior
                            .iter()
                            .chain(std::iter::once(&*layer))
                            .chain(following.iter()),
                    )
                    + geometry_receipt(&support_regions, &interfaces, &[&protected]).1
                    + super::bonds::path_bytes(&candidate.paths)
                    + candidate_aux_bytes(&candidate)
                    + anchors.retained_bytes();
                reserve_owned(&mut layer.paths, candidate.paths.len(), live_bytes)?;
                layer.paths.extend(candidate.paths);
                layer.region_coverage.process_interfaces = candidate.process_interfaces;
                {
                    replace_skin_command_fits(
                        &mut layer.skin_command_fits,
                        candidate.skin_command_fits,
                        &mut retained,
                        &mut constructed_points,
                    )?;
                    replace_skin_bridge_domains(
                        &mut layer.region_coverage,
                        candidate.skin_bridge_domains,
                        &mut retained,
                        &mut constructed_points,
                    )?;
                };
                retained_regions =
                    retained + geometry_receipt(&support_regions, &interfaces, &[&protected]).0;
                owned_bytes = native_owned_bytes
                    + support_metadata_bytes
                    + layer_payload_bytes(
                        prior
                            .iter()
                            .chain(std::iter::once(&*layer))
                            .chain(following.iter()),
                    )
                    + geometry_receipt(&support_regions, &interfaces, &[&protected]).1;
                anchors.set_budget(workspace_budget(retained_regions, owned_bytes)?);
            }
            anchors.set_budget(workspace_budget(
                retained_regions,
                owned_bytes.saturating_add(stroke_proof_reservation(layer.paths.len() - begin)),
            )?);
            let mut admission = strokes::analyze(
                &layer.paths[begin..],
                previous.clone(),
                contact.clone(),
                recipe,
                &mut anchors,
                &mut anchor_work,
                cancel,
            )?;
            if admission.failure.is_some() {
                let without_old = *segments - path_segments(&layer.paths[begin..]);
                for recover_components in [false, true] {
                    let axis = authored_axis;
                    let mut staged_segments = without_old;
                    anchors.set_budget(0);
                    let extra_owned_bytes = layer_payload_bytes(
                        prior
                            .iter()
                            .chain(std::iter::once(&*layer))
                            .chain(following.iter()),
                    ) + support_metadata_bytes
                        + geometry_receipt(&support_regions, &interfaces, &[&protected]).1
                        + vec_bytes(&admission.bridges);
                    owned_bytes = native_owned_bytes.saturating_add(extra_owned_bytes);
                    let staged_anchor_bytes = anchors.retained_bytes();
                    let mut check_component =
                        |candidate: &mut RasterCandidate,
                         axis,
                         scratch: usize,
                         scratch_owned_bytes: usize| {
                            let contact_bridge =
                                axis != authored_component_axis(candidate, index as u32, recipe);
                            let budget = component_budget(
                                retained_regions,
                                owned_bytes,
                                scratch.saturating_add(candidate_units(candidate)),
                                scratch_owned_bytes
                                    .saturating_add(candidate_owned_bytes(candidate)),
                            )?;
                            component_admission(
                                (candidate, axis, budget, contact_bridge),
                                previous.clone(),
                                contact.clone(),
                                recipe,
                                &mut anchors,
                                &mut anchor_work,
                                cancel,
                            )
                        };
                    let recovery: Option<&mut super::classification::ComponentAnchorCheck<'_>> =
                        if recover_components {
                            Some(&mut check_component)
                        } else {
                            None
                        };
                    let raster_before = work;
                    let candidate = raster(
                        index,
                        axis,
                        &mut staged_segments,
                        retained_support_points + constructed_points,
                        extra_owned_bytes,
                        staged_anchor_bytes,
                        &mut work,
                        recovery,
                    );
                    exclude_object_raster_work(&mut foreign_work, raster_before, work)?;
                    let mut candidate = candidate?;
                    coalesce_bridge_rows(
                        &mut candidate,
                        BridgeRowContext {
                            axis,
                            demanded_reorientation: false,
                            retained_points: retained_regions,
                            owned_bytes,
                            recipe,
                        },
                        previous.clone(),
                        contact.clone(),
                        &mut anchors,
                        &mut anchor_work,
                        cancel,
                    )?;
                    if !finalize_interface(
                        index,
                        &mut candidate,
                        &mut staged_segments,
                        &mut work,
                        owned_bytes.saturating_add(anchors.retained_bytes()),
                    )? {
                        continue;
                    }
                    anchors.set_budget(candidate_budget(
                        &candidate.paths,
                        retained_regions,
                        owned_bytes
                            .saturating_add(candidate_aux_bytes(&candidate))
                            .saturating_add(stroke_proof_reservation(candidate.paths.len())),
                    )?);
                    let other = strokes::analyze(
                        &candidate.paths,
                        previous.clone(),
                        contact.clone(),
                        recipe,
                        &mut anchors,
                        &mut anchor_work,
                        cancel,
                    )?;
                    if other.failure.is_none() {
                        *segments = without_old + path_segments(&candidate.paths);
                        layer.paths.truncate(begin);
                        let live_bytes = native_owned_bytes
                            + support_metadata_bytes
                            + layer_payload_bytes(
                                prior
                                    .iter()
                                    .chain(std::iter::once(&*layer))
                                    .chain(following.iter()),
                            )
                            + geometry_receipt(&support_regions, &interfaces, &[&protected]).1
                            + super::bonds::path_bytes(&candidate.paths)
                            + candidate_aux_bytes(&candidate)
                            + anchors.retained_bytes();
                        reserve_owned(&mut layer.paths, candidate.paths.len(), live_bytes)?;
                        layer.paths.extend(candidate.paths);
                        layer.region_coverage.process_interfaces = candidate.process_interfaces;
                        {
                            replace_skin_command_fits(
                                &mut layer.skin_command_fits,
                                candidate.skin_command_fits,
                                &mut retained,
                                &mut constructed_points,
                            )?;
                            replace_skin_bridge_domains(
                                &mut layer.region_coverage,
                                candidate.skin_bridge_domains,
                                &mut retained,
                                &mut constructed_points,
                            )?;
                        };
                        retained_regions = retained
                            + geometry_receipt(&support_regions, &interfaces, &[&protected]).0;
                        admission = other;
                        break;
                    }
                }
                owned_bytes = native_owned_bytes
                    + support_metadata_bytes
                    + layer_payload_bytes(
                        prior
                            .iter()
                            .chain(std::iter::once(&*layer))
                            .chain(following.iter()),
                    )
                    + geometry_receipt(&support_regions, &interfaces, &[&protected]).1;
                anchors.set_budget(workspace_budget(retained_regions, owned_bytes)?);
            }
            // Build plate only: a stroke no support could reach still prints
            // as authored and is reported, never refused.
            if let Some(failure) = admission.failure {
                layer.region_coverage.unsupported_overhang_mm = layer
                    .region_coverage
                    .unsupported_overhang_mm
                    .max(failure.span);
            }
            for path in admission.bridges {
                layer.paths[begin + path].kind = DepositionKind::Bridge;
            }
        }
        // Contact selection is only a proposal until actual final paths exist.
        // Check the same closed, physical-decimal bead enclosure used by the XY
        // exclusion gates; no schedule epsilon can authorize a smaller Z gap.
        for (target, top) in contact_tops.iter().enumerate() {
            let Some(top) = *top else {
                continue;
            };
            let mut object_bottom = f64::INFINITY;
            let mut support_top = f64::NEG_INFINITY;
            for path in &layers[target].paths {
                if support_role(path.kind) {
                    continue;
                }
                for edge in path.points_mm.windows(2) {
                    super::native::charge_support_clearance(&mut work, cancel)?;
                    object_bottom = object_bottom.min(
                        program_coordinates::project_xyz6_segment_bead_height_mm(
                            edge[0][2],
                            edge[1][2],
                            path.physical_z_mm[1] - path.physical_z_mm[0],
                        )?[0],
                    );
                }
            }
            for path in &layers[top].paths {
                if !support_role(path.kind) {
                    continue;
                }
                for edge in path.points_mm.windows(2) {
                    super::native::charge_support_clearance(&mut work, cancel)?;
                    support_top = support_top.max(
                        program_coordinates::project_xyz6_segment_bead_height_mm(
                            edge[0][2],
                            edge[1][2],
                            path.physical_z_mm[1] - path.physical_z_mm[0],
                        )?[1],
                    );
                }
            }
            // No support was emitted under this layer after all: there is no
            // contact gap to prove, and its strokes are reported unsupported.
            if !support_top.is_finite() {
                continue;
            }
            if !object_bottom.is_finite()
                || program_coordinates::enclosed_vertical_gap_mm(object_bottom, support_top)?
                    < z_gap_mm
            {
                return Err(error(
                    ManufacturingErrorCode::ProgramCoordinateConditioningFailure,
                    format!(
                        "layer {target}: actual emitted contact does not prove the minimum Z gap {z_gap_mm} mm"
                    ),
                ));
            }
        }
        // Offset/overlay corridor masks only propose candidates. Certify every
        // final actual emitted support capsule against the unchanged raw all-Z C
        // and scoped positive void masks, after both raster/anchor alternatives.
        super::trace_stage("sup_contacts", sup_clock);
        super::native::certify_support_clearance(
            layers,
            &super::native::SupportClearanceGeometry {
                sections,
                schedule,
                protected: &protected,
                scopes: &scopes,
            },
            recipe,
            native_work,
            &mut work,
            cancel,
        )?;
        Ok(SupportAdmission::Complete)
    })();
    work_bank.finish(work, foreign_work, anchor_work)?;
    outcome
}

fn contact_layer(
    layers: &[PrintLayer],
    target: usize,
    path: &DepositionPath,
    z_gap_mm: f64,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<usize>, ManufacturingError> {
    let floor = program_coordinates::project_xyz6_bead_height_mm(
        path.physical_z_mm[1],
        path.physical_z_mm[1] - path.physical_z_mm[0],
    )?[0];
    for i in (0..target).rev() {
        super::native::charge_support_clearance(work, cancel)?;
        let upper = program_coordinates::project_xyz6_bead_height_mm(
            layers[i].z_mm,
            layers[i].z_mm - layers[i].lower_z_mm,
        )?[1];
        if program_coordinates::enclosed_vertical_gap_mm(floor, upper)? >= z_gap_mm {
            return Ok(Some(i));
        }
    }
    Ok(None)
}

fn unreachable_layer_error(
    layers: &[PrintLayer],
    demand: &Demand,
    z_gap_mm: f64,
) -> Result<ManufacturingError, ManufacturingError> {
    let path = &layers[demand.target].paths[demand.failure.path];
    let floor = program_coordinates::project_xyz6_bead_height_mm(
        path.physical_z_mm[1],
        path.physical_z_mm[1] - path.physical_z_mm[0],
    )?[0];
    Ok(error(
        ManufacturingErrorCode::UnsupportedGeometry,
        format!(
            "layer {}: no printable support layer fits below the declared Z gap {z_gap_mm} mm; demanded {:?} stroke {:?} to {:?}, physical interval {:?} mm, width {} mm, uncovered parameters [{:.9}, {:.9}], span {:.9} mm, emitted object floor {:.9} mm, bed {:.9} mm",
            demand.target,
            path.kind,
            path.points_mm[demand.failure.first],
            path.points_mm[demand.failure.last],
            path.physical_z_mm,
            demand.failure.width_mm,
            demand.failure.start,
            demand.failure.end,
            demand.failure.span,
            floor,
            layers[0].lower_z_mm,
        ),
    ))
}

/// Move existing buffers, never copy geometry. Final construction is complete
/// before this layer can supply lower anchors to its successor.
fn finalize_layer_interface(
    index: usize,
    layer: &mut PrintLayer,
    segments: &mut usize,
    work: &mut usize,
    outside_owned_bytes: usize,
    finalize: &mut impl FnMut(
        usize,
        &mut RasterCandidate,
        &mut usize,
        &mut usize,
        usize,
    ) -> Result<bool, ManufacturingError>,
) -> Result<bool, ManufacturingError> {
    if layer.region_coverage.process_interfaces.is_empty() {
        return Ok(true);
    }
    let mut candidate = RasterCandidate {
        skin_command_fits: std::mem::take(&mut layer.skin_command_fits),
        skin_bridge_domains: std::mem::take(&mut layer.region_coverage.skin_bridge_domains),
        paths: std::mem::take(&mut layer.paths),
        whole_rows: Vec::new(),
        process_interfaces: std::mem::take(&mut layer.region_coverage.process_interfaces),
    };
    let result = match outside_owned_bytes.checked_sub(candidate_owned_bytes(&candidate)) {
        Some(outside) => finalize(index, &mut candidate, segments, work, outside),
        None => Err(error(
            ManufacturingErrorCode::ResourceLimit,
            "moved interface candidate exceeds its live ownership receipt",
        )),
    };
    layer.paths = candidate.paths;
    layer.region_coverage.process_interfaces = candidate.process_interfaces;
    {
        layer.region_coverage.skin_bridge_domains = candidate.skin_bridge_domains;
        layer.skin_command_fits = candidate.skin_command_fits;
    };
    result
}

fn replace_skin_bridge_domains(
    coverage: &mut PlanarRegionCoverage,
    domains: Vec<SkinBridgeDomain>,
    retained: &mut usize,
    constructed: &mut usize,
) -> Result<(), ManufacturingError> {
    let old = super::bonds::skin_bridge_points(&coverage.skin_bridge_domains);
    let new = super::bonds::skin_bridge_points(&domains);
    let revised = retained
        .checked_sub(old)
        .and_then(|n| n.checked_add(new))
        .filter(|n| *n <= super::MAX_RETAINED_GEOMETRY_UNITS)
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "retained mixed bridge domain budget exceeded",
            )
        })?;
    *constructed = constructed
        .checked_sub(old)
        .and_then(|n| n.checked_add(new))
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "mixed bridge domain accounting mismatch",
            )
        })?;
    *retained = revised;
    coverage.skin_bridge_domains = domains;
    Ok(())
}

fn replace_skin_command_fits(
    current: &mut Vec<SkinCommandFitDomain>,
    domains: Vec<SkinCommandFitDomain>,
    retained: &mut usize,
    constructed: &mut usize,
) -> Result<(), ManufacturingError> {
    let old = super::bonds::skin_command_fit_points(current);
    let new = super::bonds::skin_command_fit_points(&domains);
    let revised = retained
        .checked_sub(old)
        .and_then(|n| n.checked_add(new))
        .filter(|n| *n <= super::MAX_RETAINED_GEOMETRY_UNITS)
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "retained skin command fit budget exceeded",
            )
        })?;
    let revised_constructed = constructed
        .checked_sub(old)
        .and_then(|n| n.checked_add(new))
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "skin command fit accounting mismatch",
            )
        })?;
    *retained = revised;
    *constructed = revised_constructed;
    *current = domains;
    Ok(())
}

fn support_role(kind: DepositionKind) -> bool {
    matches!(
        kind,
        DepositionKind::Support | DepositionKind::SupportInterface
    )
}

fn path_segments(paths: &[DepositionPath]) -> usize {
    paths.iter().map(|path| path.points_mm.len() - 1).sum()
}

fn candidate_budget(
    paths: &Vec<DepositionPath>,
    retained: usize,
    owned_bytes: usize,
) -> Result<usize, ManufacturingError> {
    let units = paths.iter().fold(retained, |total, path| {
        total.saturating_add(path.points_mm.capacity())
    });
    workspace_budget(
        units,
        owned_bytes.saturating_add(super::bonds::path_bytes(paths)),
    )
}

fn candidate_aux_bytes(candidate: &RasterCandidate) -> usize {
    super::bonds::interface_owned_bytes(&candidate.process_interfaces)
        + vec_bytes(&candidate.skin_bridge_domains)
        + candidate
            .skin_bridge_domains
            .iter()
            .map(SkinBridgeDomain::allocated_bytes)
            .sum::<usize>()
        + super::bonds::skin_command_fit_bytes(&candidate.skin_command_fits)
        + super::bonds::path_bytes(&candidate.whole_rows)
}

fn candidate_owned_bytes(candidate: &RasterCandidate) -> usize {
    super::bonds::path_bytes(&candidate.paths).saturating_add(candidate_aux_bytes(candidate))
}

fn candidate_units(candidate: &RasterCandidate) -> usize {
    candidate_owned_bytes(candidate).div_ceil(std::mem::size_of::<geo::Coord<f64>>())
}

fn matches_skin_witness(path: &DepositionPath, bond: &ProcessEndpointBond) -> bool {
    let ProcessEndpointBond::Skin {
        skin_kind,
        process_kind,
        skin_segment_mm,
        ..
    } = bond
    else {
        return false;
    };
    if path.kind != *process_kind
        || path.owner != Some(DepositionOwner::MandatorySkin { skin: *skin_kind })
        || path.points_mm.len() != 2
    {
        return false;
    }
    let a = [path.points_mm[0][0], path.points_mm[0][1]];
    let b = [path.points_mm[1][0], path.points_mm[1][1]];
    [a, b] == *skin_segment_mm || [b, a] == *skin_segment_mm
}

fn coalesce_bridge_rows<'a>(
    candidate: &mut RasterCandidate,
    context: BridgeRowContext<'_>,
    previous: impl Iterator<Item = &'a DepositionPath> + Clone,
    contacts: impl Iterator<Item = &'a DepositionPath> + Clone,
    anchors: &mut AnchorWorkspace,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    let mut lower = strokes::LowerStrokes::new(previous.clone(), contacts.clone());
    coalesce_bridge_rows_with(
        candidate, context, previous, contacts, &mut lower, anchors, work, cancel,
    )
}

/// Bridge-row coalescing over one borrowed lower set shared by every row proof.
#[allow(clippy::too_many_arguments)]
fn coalesce_bridge_rows_with<'a, P, C>(
    candidate: &mut RasterCandidate,
    context: BridgeRowContext<'_>,
    previous: P,
    contacts: C,
    lower: &mut strokes::LowerStrokes<'a, P, C>,
    anchors: &mut AnchorWorkspace,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError>
where
    P: Iterator<Item = &'a DepositionPath> + Clone,
    C: Iterator<Item = &'a DepositionPath> + Clone,
{
    let mut joined = false;
    for axis in [context.axis, context.axis.orthogonal()] {
        let fixed = usize::from(axis == region::HatchAxis::X);
        if !candidate.whole_rows.iter().any(|row| {
            row.points_mm.len() == 2 && row.points_mm[0][fixed] == row.points_mm[1][fixed]
        }) {
            continue;
        }
        joined |= coalesce_bridge_rows_on_axis(
            candidate,
            BridgeRowContext {
                axis,
                demanded_reorientation: context.demanded_reorientation,
                retained_points: context.retained_points,
                owned_bytes: context.owned_bytes,
                recipe: context.recipe,
            },
            previous.clone(),
            contacts.clone(),
            lower,
            anchors,
            work,
            cancel,
        )?;
    }
    Ok(joined)
}

#[expect(
    clippy::too_many_arguments,
    reason = "Row candidate, borrowed anchor inventory and caller-owned work stay explicit"
)]
fn coalesce_bridge_rows_on_axis<'a, P, C>(
    candidate: &mut RasterCandidate,
    context: BridgeRowContext<'_>,
    previous: P,
    contacts: C,
    lower: &mut strokes::LowerStrokes<'a, P, C>,
    anchors: &mut AnchorWorkspace,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError>
where
    P: Iterator<Item = &'a DepositionPath> + Clone,
    C: Iterator<Item = &'a DepositionPath> + Clone,
{
    let BridgeRowContext {
        axis,
        demanded_reorientation,
        retained_points,
        owned_bytes,
        recipe,
    } = context;
    if candidate.whole_rows.is_empty() {
        return Ok(false);
    }
    let fixed = if axis == region::HatchAxis::X { 1 } else { 0 };
    let along = 1 - fixed;
    // Compare actual emitted-grid coordinates, not a widened anchor radius or
    // native tolerance. Whole rows themselves come from authoritative fill.
    let grid = program_coordinates::project_xyz6_coordinate_mm;
    let row_units = candidate
        .whole_rows
        .iter()
        .map(|path| path.points_mm.capacity())
        .sum::<usize>();
    let budget = candidate_budget(
        &candidate.paths,
        retained_points.saturating_add(row_units),
        owned_bytes
            .saturating_add(candidate_aux_bytes(candidate))
            .saturating_add(2 * std::mem::size_of::<[f64; 3]>() + 2 * stroke_proof_reservation(1)),
    )?;
    let original_len = candidate.paths.len();
    let workspace_budget = budget.checked_sub(original_len).ok_or_else(|| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "bridge row ownership workspace exceeded",
        )
    })?;
    let query_budget = (workspace_budget / 2).max(anchors.retained_records());
    let pieces_budget = workspace_budget.checked_sub(query_budget).ok_or_else(|| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "live bridge anchor workspace exceeded",
        )
    })?;
    anchors.set_budget(query_budget);
    let mut pieces = Vec::new();
    for (index, path) in candidate.paths.iter().enumerate() {
        check_cancel(cancel)?;
        if path.points_mm.len() != 2
            || matches!(
                path.kind,
                DepositionKind::Perimeter | DepositionKind::SeparatorWall
            )
        {
            continue;
        }
        let a = path.points_mm[0];
        let b = path.points_mm[1];
        let row = grid(a[fixed])?;
        if row != grid(b[fixed])? {
            continue;
        }
        let lo = grid(a[along])?.min(grid(b[along])?);
        let hi = grid(a[along])?.max(grid(b[along])?);
        reserve_component_record(&mut pieces, 2, 0, pieces_budget)?;
        pieces.push((row, lo, hi, index));
    }
    pieces.sort_unstable_by(|a, b| {
        a.0.total_cmp(&b.0)
            .then(a.1.total_cmp(&b.1))
            .then(a.2.total_cmp(&b.2))
    });
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Ownership {
        Required,
        Frozen,
        Replaced,
        Removed,
    }
    let mut ownership = Vec::new();
    ownership.try_reserve_exact(original_len).map_err(|_| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "bridge row ownership allocation refused",
        )
    })?;
    ownership.resize(original_len, Ownership::Required);
    for &(_, _, _, index) in &pieces {
        for bond in candidate
            .process_interfaces
            .iter()
            .flat_map(|interface| interface.endpoint_bonds.iter().flatten())
        {
            check_cancel(cancel)?;
            *work = work.checked_add(1).ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "cumulative bridge witness ownership work counter overflow",
                )
            })?;
            if matches_skin_witness(&candidate.paths[index], bond) {
                // The actual continuous wall connection is already chosen.
                // Freeze construction, not lower-anchor/support admission.
                ownership[index] = Ownership::Frozen;
                break;
            }
        }
    }
    let construction = matches!(
        recipe.sparse_infill,
        SparseInfillPolicy::Grid | SparseInfillPolicy::WebRoof { .. }
    );
    let retained_infill = |path: &DepositionPath| {
        construction
            && (path.kind == DepositionKind::SparseFill
                || path.kind == DepositionKind::Bridge
                    && matches!(path.owner, Some(DepositionOwner::Regional { .. })))
    };
    if construction {
        for &(_, _, _, index) in &pieces {
            if retained_infill(&candidate.paths[index]) {
                ownership[index] = Ownership::Frozen;
            }
        }
    }
    let mut joined = false;
    for mut row in candidate.whole_rows.extract_if(.., |row| {
        row.points_mm.len() == 2 && row.points_mm[0][fixed] == row.points_mm[1][fixed]
    }) {
        check_cancel(cancel)?;
        if retained_infill(&row) {
            continue;
        }
        let key = grid(row.points_mm[0][fixed])?;
        let lo = grid(row.points_mm[0][along])?.min(grid(row.points_mm[1][along])?);
        let hi = grid(row.points_mm[0][along])?.max(grid(row.points_mm[1][along])?);
        let whole_start = row.points_mm[0][along].min(row.points_mm[1][along]);
        let whole_end = row.points_mm[0][along].max(row.points_mm[1][along]);
        let begin = pieces.partition_point(|piece| piece.0 < key);
        let end = pieces.partition_point(|piece| piece.0 <= key);
        // Each piece is visited at most twice while finding disjoint groups.
        *work = work.checked_add((end - begin) * 2).ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "cumulative bridge row ownership work counter overflow",
            )
        })?;
        let mut cursor = begin;
        let mut previous_required_end = None;
        let mut previous_emitted_end = None;
        while cursor < end {
            check_cancel(cancel)?;
            let (_, start, stop, index) = pieces[cursor];
            if ownership[index] == Ownership::Frozen
                || candidate.paths[index].width != row.width
                || candidate.paths[index].physical_z_mm != row.physical_z_mm
            {
                if stop > lo && start < hi {
                    let points = &candidate.paths[index].points_mm;
                    let required_end = points[0][along].max(points[1][along]);
                    previous_required_end = Some(
                        previous_required_end
                            .map_or(required_end, |previous: f64| previous.max(required_end)),
                    );
                    previous_emitted_end =
                        Some(previous_emitted_end.map_or(stop, |previous: f64| previous.max(stop)));
                }
                cursor += 1;
                continue;
            }
            let group_begin = cursor;
            let mut first = None;
            let mut last = None;
            let mut covered: f64 = f64::NEG_INFINITY;
            let mut required_start = f64::INFINITY;
            let mut required_end = f64::NEG_INFINITY;
            let mut count = 0;
            let mut core_owner = None;
            while cursor < end {
                check_cancel(cancel)?;
                let (_, start, stop, index) = pieces[cursor];
                if (ownership[index] == Ownership::Frozen
                    || candidate.paths[index].width != row.width
                    || candidate.paths[index].physical_z_mm != row.physical_z_mm)
                    && stop > lo
                    && start < hi
                {
                    // Retain frozen rows as required barriers so a neighbor
                    // cannot consume their geometry as an unrequired extension.
                    break;
                }
                if ownership[index] != Ownership::Required
                    || start < lo
                    || stop > hi
                    || stop <= start
                {
                    cursor += 1;
                    continue;
                }
                if let Some(last) = last
                    && start > covered
                {
                    let previous_piece: &DepositionPath = &candidate.paths[last];
                    let current_piece = &candidate.paths[index];
                    let endpoint =
                        if current_piece.points_mm[0][along] <= current_piece.points_mm[1][along] {
                            current_piece.points_mm[0]
                        } else {
                            current_piece.points_mm[1]
                        };
                    // A same-row centerline gap is not missing material when
                    // the actual emitted current bead capsules already overlap
                    // positively. Tangency is not connectivity. This candidate
                    // connector still needs independent previous-anchor proof.
                    if !(AnchorCandidate {
                        endpoints_mm: [previous_piece.points_mm[0], previous_piece.points_mm[1]],
                        bead_width_mm: region::constant_width(previous_piece)?,
                    })
                    .emitted_endpoint_footprint_overlaps(
                        endpoint,
                        region::constant_width(current_piece)?,
                    )? {
                        break;
                    }
                }
                first.get_or_insert(index);
                let piece_owner = candidate.paths[index].owner;
                if count == 0 {
                    core_owner = piece_owner;
                } else if core_owner != piece_owner {
                    // Every required core owner must agree. Anchor-only end
                    // extensions do not contribute a new mandatory owner.
                    core_owner = None;
                }
                // Nested or overlapping owners cannot move the union's end
                // backwards or discard material owned by an earlier piece.
                if stop > covered {
                    covered = stop;
                    last = Some(index);
                }
                for point in &candidate.paths[index].points_mm {
                    required_start = required_start.min(point[along]);
                    required_end = required_end.max(point[along]);
                }
                count += 1;
                cursor += 1;
            }
            let Some(first) = first else {
                continue;
            };
            // Neighboring required groups bound each search, but are not
            // anchors. A group may reach the nearest real previous interval
            // inside an unrequired gap without filling that gap completely.
            let left_emitted_end = previous_emitted_end;
            let next_required_start = if cursor < end {
                let next = &candidate.paths[pieces[cursor].3].points_mm;
                Some(next[0][along].min(next[1][along]))
            } else {
                None
            };
            let extension_start = previous_required_end.unwrap_or(whole_start);
            let extension_end = next_required_start.unwrap_or(whole_end);
            previous_required_end = Some(required_end);
            previous_emitted_end = Some(grid(required_end)?);
            row.points_mm[0][along] = required_start;
            row.points_mm[1][along] = required_end;
            row.owner = core_owner;
            let contact_extension = demanded_reorientation
                && matches!(core_owner, Some(DepositionOwner::MandatorySkin { .. }));
            let required_proof =
                lower.analyze(std::slice::from_ref(&row), recipe, anchors, work, cancel)?;
            // A short one-ended overhang may already satisfy ordinary support.
            // In a staged orthogonal proposal it still needs a real second
            // anchor before demand-bound band selection can consume it as Bridge.
            let construct_ends = required_proof.failure.is_some()
                || (demanded_reorientation && required_proof.bridges.is_empty());
            let proof = if construct_ends {
                // End extension cannot repair an over-limit interior span.
                if required_proof
                    .failure
                    .as_ref()
                    .is_some_and(|failure| failure.start > 0.0 && failure.end < 1.0)
                {
                    continue;
                }
                let whole = AnchorCandidate {
                    endpoints_mm: [
                        {
                            let mut p = row.points_mm[0];
                            p[along] = extension_start;
                            p
                        },
                        {
                            let mut p = row.points_mm[1];
                            p[along] = extension_end;
                            p
                        },
                    ],
                    bead_width_mm: region::constant_width(&row)?,
                };
                let length = extension_end - extension_start;
                let mut start = ((required_start - extension_start) / length).clamp(0.0, 1.0);
                let mut end = ((required_end - extension_start) / length).clamp(0.0, 1.0);
                let mut anchored = true;
                let gaps = if contact_extension {
                    anchors.gaps(
                        whole,
                        previous
                            .clone()
                            .filter(|lower| lower.physical_z_mm[1] == row.physical_z_mm[0]),
                        contacts.clone().filter(|lower| {
                            support_role(lower.kind)
                                && lower.physical_z_mm[1] <= row.physical_z_mm[0]
                        }),
                        work,
                        cancel,
                    )?
                } else {
                    lower.gaps(whole, row.physical_z_mm[0], false, anchors, work, cancel)?
                };
                for gap in gaps {
                    if gap.start <= start && start < gap.end {
                        anchored &= gap.left;
                        start = gap.start;
                    }
                    if gap.start < end && end <= gap.end {
                        anchored &= gap.right;
                        end = gap.end;
                    }
                }
                if !anchored {
                    continue;
                }
                row.points_mm[0][along] = extension_start + start * length;
                row.points_mm[1][along] = extension_start + end * length;
                lower.analyze(std::slice::from_ref(&row), recipe, anchors, work, cancel)?
            } else {
                if count < 2 || required_proof.bridges.is_empty() {
                    continue;
                }
                required_proof
            };
            if proof.failure.is_some() || proof.bridges.is_empty() {
                continue;
            }
            let emitted_start = grid(row.points_mm[0][along])?;
            let emitted_end = grid(row.points_mm[1][along])?;
            if let Some(left) = left_emitted_end
                && emitted_start < grid(required_start)?
                && anchors::emitted_coordinate_separation_lower_mm(emitted_start, left)?
                    <= region::constant_width(&row)?
            {
                continue;
            }
            if let Some(right) = next_required_start
                && emitted_end > grid(required_end)?
                && anchors::emitted_coordinate_separation_lower_mm(grid(right)?, emitted_end)?
                    <= region::constant_width(&row)?
            {
                continue;
            }
            // Preserve an actual footprint gap between independent groups,
            // including after an earlier group acquired its own extension.
            previous_emitted_end = Some(emitted_end);
            for &(_, start, stop, index) in &pieces[group_begin..cursor] {
                if ownership[index] == Ownership::Required && start >= lo && stop <= hi {
                    ownership[index] = Ownership::Removed;
                }
            }
            // Reuse an existing two-point owner instead of retaining both all
            // original pieces and newly allocated group paths until compaction.
            candidate.paths[first].kind = row.kind;
            candidate.paths[first].owner = row.owner;
            candidate.paths[first]
                .points_mm
                .copy_from_slice(&row.points_mm);
            ownership[first] = Ownership::Replaced;
            joined = true;
        }
    }
    let mut index = 0;
    candidate.paths.retain(|_| {
        let keep = ownership[index] != Ownership::Removed;
        index += 1;
        keep
    });
    Ok(joined)
}

fn admission_error(
    index: usize,
    z: f64,
    paths: &[DepositionPath],
    failure: &strokes::Failure,
    recipe: &PlanarPrintRecipe,
) -> ManufacturingError {
    let path = &paths[failure.path];
    error(
        ManufacturingErrorCode::UnsupportedGeometry,
        format!(
            "layer {index} Z {z:.6}: neither orthogonal native raster has bounded deposited anchors (bridge limit {:.3} mm); role {:?}, stroke {:?} to {:?}, gap ({:.12},{:.12}) span {:.12}; enable breakaway supports or revise placement/infill",
            recipe.process.max_bridge_span_mm,
            path.kind,
            path.points_mm[failure.first],
            path.points_mm[failure.last],
            failure.start,
            failure.end,
            failure.span
        ),
    )
}

pub(in crate::planner) fn object_volumes(
    layers: &mut [PrintLayer],
    intent: &ManufacturingIntent,
) -> Result<(), ManufacturingError> {
    for layer in layers {
        layer.region_coverage.nominal_deposition_volume_mm3 = layer
            .paths
            .iter()
            .filter(|path| !support_role(path.kind))
            .try_fold(0.0, |sum, path| {
                let process = intent.process_for_path(path)?;
                Ok::<_, ManufacturingError>(
                    sum + region::nominal_volume_with_process(
                        path,
                        layer.index,
                        process.flow_multiplier,
                        process.policy,
                    )?,
                )
            })?;
    }
    Ok(())
}

struct NearestStroke {
    distance_mm: f64,
    width_mm: f64,
    kind: DepositionKind,
    physical_z_mm: [f64; 2],
    endpoints_mm: [[f64; 3]; 2],
}

impl std::fmt::Debug for NearestStroke {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NearestStroke")
            .field("distance_mm", &self.distance_mm)
            .field("width_mm", &self.width_mm)
            .field("kind", &self.kind)
            .field("physical_z_mm", &self.physical_z_mm)
            .field("endpoints_mm", &self.endpoints_mm)
            .finish()
    }
}

fn nearest_stroke<'a>(
    point: [f64; 2],
    paths: impl Iterator<Item = &'a DepositionPath>,
    work: &mut usize,
    foreign_work: usize,
    cancel: &AtomicBool,
) -> Result<Option<NearestStroke>, ManufacturingError> {
    let mut nearest: Option<NearestStroke> = None;
    for path in paths {
        for local in path.checked_edges()? {
            let edge = local.points_mm;
            charge_owned_work(work, foreign_work, 1, cancel)?;
            let delta = [edge[1][0] - edge[0][0], edge[1][1] - edge[0][1]];
            let length = delta[0].hypot(delta[1]);
            let unit = [delta[0] / length, delta[1] / length];
            let along = ((point[0] - edge[0][0]) * unit[0] + (point[1] - edge[0][1]) * unit[1])
                .clamp(0.0, length);
            let distance = (point[0] - edge[0][0] - along * unit[0])
                .hypot(point[1] - edge[0][1] - along * unit[1]);
            if !distance.is_finite() {
                return Err(error(
                    ManufacturingErrorCode::UnsupportedGeometry,
                    "nonfinite deposited-stroke diagnostic distance",
                ));
            }
            if nearest
                .as_ref()
                .is_none_or(|best| distance < best.distance_mm)
            {
                nearest = Some(NearestStroke {
                    distance_mm: distance,
                    width_mm: local.width_mm,
                    kind: path.kind,
                    physical_z_mm: path.physical_z_mm,
                    endpoints_mm: [edge[0], edge[1]],
                });
            }
        }
    }
    Ok(nearest)
}

fn charge_boolean(
    a: &MultiPolygon<f64>,
    b: &MultiPolygon<f64>,
    work: &mut usize,
    foreign_work: usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    boolean_budget(a, b, cancel)?;
    let pairs = region::point_count(a)
        .checked_mul(region::point_count(b))
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "support Boolean work overflow",
            )
        })?;
    charge_owned_work(work, foreign_work, pairs, cancel)
}

fn charge_owned_work(
    work: &mut usize,
    foreign: usize,
    amount: usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    let owned = work.checked_sub(foreign).ok_or_else(|| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "support work-domain charge mismatch",
        )
    })?;
    let next = work.checked_add(amount)
        .ok_or_else(|| error(
            ManufacturingErrorCode::ResourceLimit,
            format!(
                "cumulative planar work counter overflow in support; global {}, support-owned {owned}, requested {amount}",
                *work,
            ),
        ))?;
    *work = next;
    Ok(())
}

fn reserve_owned<T>(
    values: &mut Vec<T>,
    additional: usize,
    live_bytes: usize,
) -> Result<(), ManufacturingError> {
    let needed = values.len().checked_add(additional).ok_or_else(|| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "support capacity overflow",
        )
    })?;
    if needed <= values.capacity() {
        return Ok(());
    }
    let capacity = needed.max(values.capacity().saturating_mul(2)).max(4);
    let old_bytes = vec_bytes(values);
    let requested_bytes = capacity
        .checked_mul(std::mem::size_of::<T>())
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "support allocation byte overflow",
            )
        })?;
    // Both old and replacement arrays may coexist during reallocation.
    super::remaining_geometry_units(0, live_bytes.saturating_add(requested_bytes))?;
    values
        .try_reserve_exact(capacity - values.len())
        .map_err(|_| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "support allocation refused",
            )
        })?;
    super::remaining_geometry_units(
        0,
        live_bytes
            .saturating_sub(old_bytes)
            .saturating_add(vec_bytes(values)),
    )?;
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "Offset admission inventories shared support, interfaces and temporary material without retaining them"
)]
fn offset_polygon_owned(
    polygon: &Polygon<f64>,
    distance: f64,
    retained: usize,
    owned_bytes: usize,
    support: &[Rc<OwnedMaterial>],
    interfaces: &[Rc<OwnedMaterial>],
    temporary: &[&OwnedMaterial],
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    let (units, bytes) = geometry_receipt(support, interfaces, temporary);
    region::offset_polygon_admitted(
        polygon,
        distance,
        |scratch, scratch_bytes, overlay| {
            super::remaining_geometry_units(
                retained.saturating_add(units).saturating_add(scratch),
                owned_bytes
                    .saturating_add(bytes)
                    .saturating_add(scratch_bytes),
            )?;
            if let Some((a, b)) = overlay {
                boolean_budget(a, b, cancel)?;
            }
            Ok(())
        },
        cancel,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "Offset admission inventories shared support, interfaces and temporary material without retaining them"
)]
fn offset_owned(
    region: &MultiPolygon<f64>,
    distance: f64,
    retained: usize,
    owned_bytes: usize,
    support: &[Rc<OwnedMaterial>],
    interfaces: &[Rc<OwnedMaterial>],
    temporary: &[&OwnedMaterial],
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    let (units, bytes) = geometry_receipt(support, interfaces, temporary);
    region::offset_admitted(
        region,
        distance,
        |scratch, scratch_bytes, overlay| {
            super::remaining_geometry_units(
                retained.saturating_add(units).saturating_add(scratch),
                owned_bytes
                    .saturating_add(bytes)
                    .saturating_add(scratch_bytes),
            )?;
            if let Some((a, b)) = overlay {
                boolean_budget(a, b, cancel)?;
            }
            Ok(())
        },
        cancel,
    )
}

fn vec_bytes<T>(values: &Vec<T>) -> usize {
    values.capacity().saturating_mul(std::mem::size_of::<T>())
}

fn material_units(material: &MultiPolygon<f64>) -> usize {
    material
        .0
        .iter()
        .map(|polygon| {
            polygon.exterior().0.capacity()
                + polygon
                    .interiors()
                    .iter()
                    .map(|ring| ring.0.capacity())
                    .sum::<usize>()
        })
        .sum()
}

fn layer_payload_bytes<'a>(layers: impl Iterator<Item = &'a PrintLayer>) -> usize {
    layers
        .map(|layer| {
            super::bonds::path_bytes(&layer.paths)
                .saturating_add(super::bonds::coverage_owned_bytes(&layer.region_coverage))
                .saturating_add(super::bonds::skin_command_fit_bytes(
                    &layer.skin_command_fits,
                ))
        })
        .fold(0usize, usize::saturating_add)
}

fn geometry_receipt(
    support: &[Rc<OwnedMaterial>],
    interfaces: &[Rc<OwnedMaterial>],
    temporary: &[&OwnedMaterial],
) -> (usize, usize) {
    // Owner-local marks avoid an allocating pointer set and all-pairs scans.
    for owner in support
        .iter()
        .chain(interfaces)
        .map(Rc::as_ref)
        .chain(temporary.iter().copied())
    {
        owner.counted.set(false);
    }
    let mut units = 0usize;
    let mut bytes = 0usize;
    for owner in support
        .iter()
        .chain(interfaces)
        .map(Rc::as_ref)
        .chain(temporary.iter().copied())
    {
        if owner.counted.replace(true) {
            continue;
        }
        units = units.saturating_add(owner.units);
        bytes = bytes
            .saturating_add(owner.bytes)
            .saturating_add(owner.header_bytes);
    }
    (units, bytes)
}

fn remaining_geometry_budget(
    native: usize,
    owned_bytes: usize,
    support: &[Rc<OwnedMaterial>],
    interfaces: &[Rc<OwnedMaterial>],
    temporary: &[&OwnedMaterial],
) -> Result<usize, ManufacturingError> {
    let (units, bytes) = geometry_receipt(support, interfaces, temporary);
    super::remaining_geometry_units(
        native.saturating_add(units),
        owned_bytes.saturating_add(bytes),
    )
}

/// Column bounds and one clearance distance bounding every per-piece distance
/// for pieces inside `frame` (overlay padding is monotone in the work frame).
#[derive(Clone, Copy)]
struct ColumnScreen {
    column: [[f64; 2]; 2],
    frame: [[f64; 2]; 2],
    reach: f64,
}

impl ColumnScreen {
    /// Margin of the bounding frame around the column; pieces beyond it fall
    /// back to the exact per-piece computation.
    const FRAME_MM: f64 = 1024.0;

    fn new(column: &MultiPolygon<f64>, requested: f64) -> Result<Option<Self>, ManufacturingError> {
        let Some(b) = column.bounding_rect() else {
            return Ok(None);
        };
        let column = [[b.min().x, b.min().y], [b.max().x, b.max().y]];
        let frame = [
            column[0].map(|v| v - Self::FRAME_MM),
            column[1].map(|v| v + Self::FRAME_MM),
        ];
        let Ok(reach) = clearance_distance_in_frame(frame, frame, requested) else {
            return Ok(None);
        };
        Ok(Some(Self {
            column,
            frame,
            reach,
        }))
    }

    /// True only when `candidate_clearance_distance` would return `None`.
    fn rejects(&self, piece: [[f64; 2]; 2]) -> bool {
        let (a, b, d) = (piece, self.column, self.reach);
        a.iter().flatten().all(|v| v.is_finite())
            && a[0][0] >= self.frame[0][0]
            && a[0][1] >= self.frame[0][1]
            && a[1][0] <= self.frame[1][0]
            && a[1][1] <= self.frame[1][1]
            && ((a[1][0] + d).next_up() < b[0][0]
                || (a[0][0] - d).next_down() > b[1][0]
                || (a[1][1] + d).next_up() < b[0][1]
                || (a[0][1] - d).next_down() > b[1][1])
    }
}

/// Exclusion pieces staged against one column for a single union subtraction.
struct PieceBatch {
    pieces: MultiPolygon<f64>,
    frame: Option<[[f64; 2]; 2]>,
    points: usize,
    bytes: usize,
}

impl Default for PieceBatch {
    fn default() -> Self {
        Self {
            pieces: MultiPolygon(Vec::new()),
            frame: None,
            points: 0,
            bytes: 0,
        }
    }
}

impl PieceBatch {
    /// Flush granularity bounds the staged copy, not total work.
    const FLUSH_POINTS: usize = 1 << 14;

    fn stage(
        &mut self,
        piece: &Polygon<f64>,
        retained: usize,
        owned_bytes: usize,
        support: &[Rc<OwnedMaterial>],
        interfaces: &[Rc<OwnedMaterial>],
        temporary: &[&OwnedMaterial],
    ) -> Result<(), ManufacturingError> {
        let points = piece.exterior().0.len()
            + piece.interiors().iter().map(|r| r.0.len()).sum::<usize>();
        let bytes = std::mem::size_of::<Polygon<f64>>()
            + std::mem::size_of_val(piece.interiors())
            + points * std::mem::size_of::<geo::Coord<f64>>();
        let records = (self.pieces.0.len() + 1)
            .max(self.pieces.0.capacity())
            .saturating_mul(2 * std::mem::size_of::<Polygon<f64>>());
        remaining_geometry_budget(
            retained.saturating_add(self.points).saturating_add(points),
            owned_bytes
                .saturating_add(self.bytes)
                .saturating_add(bytes)
                .saturating_add(records),
            support,
            interfaces,
            temporary,
        )?;
        let b = piece.bounding_rect().ok_or_else(|| {
            error(
                ManufacturingErrorCode::NativeAnalysisIndeterminate,
                "native outer operand has no finite bounds",
            )
        })?;
        self.frame = Some(union_frame(
            self.frame,
            [[b.min().x, b.min().y], [b.max().x, b.max().y]],
        ));
        self.pieces.0.push(piece.clone());
        self.points += points;
        self.bytes += bytes;
        Ok(())
    }

    fn owned_bytes(&self) -> usize {
        self.bytes
            .saturating_add(self.pieces.0.capacity() * std::mem::size_of::<Polygon<f64>>())
    }
}

/// Positive-winding union of possibly overlapping pieces, each oriented with
/// its material on the left; overlaps are never XOR-normalized.
fn union_pieces(pieces: &MultiPolygon<f64>) -> MultiPolygon<f64> {
    use i_overlay::core::{fill_rule::FillRule, overlay_rule::OverlayRule};
    use i_overlay::float::overlay::FloatOverlay;
    let mut paths: Vec<Vec<[f64; 2]>> = Vec::new();
    for polygon in &pieces.0 {
        for (index, ring) in std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .enumerate()
        {
            let mut points: Vec<[f64; 2]> = ring.0.iter().map(|c| [c.x, c.y]).collect();
            if points.len() > 1 && points.first() == points.last() {
                points.pop();
            }
            let twice_area: f64 = (0..points.len())
                .map(|i| {
                    let a = points[i];
                    let b = points[(i + 1) % points.len()];
                    a[0] * b[1] - b[0] * a[1]
                })
                .sum();
            if (twice_area < 0.0) == (index == 0) {
                points.reverse();
            }
            paths.push(points);
        }
    }
    let shapes = FloatOverlay::with_subj(&paths).overlay(OverlayRule::Subject, FillRule::Positive);
    MultiPolygon(
        shapes
            .into_iter()
            .map(|shape| {
                let mut rings = shape.into_iter().map(|ring| {
                    let mut line =
                        geo::LineString(ring.into_iter().map(geo::Coord::from).collect());
                    line.close();
                    line
                });
                Polygon::new(
                    rings.next().unwrap_or(geo::LineString::empty()),
                    rings.collect(),
                )
            })
            .collect(),
    )
}

fn union_frame(frame: Option<[[f64; 2]; 2]>, other: [[f64; 2]; 2]) -> [[f64; 2]; 2] {
    match frame {
        None => other,
        Some(f) => [
            [f[0][0].min(other[0][0]), f[0][1].min(other[0][1])],
            [f[1][0].max(other[1][0]), f[1][1].max(other[1][1])],
        ],
    }
}

// Subtracting the union of staged clearances equals subtracting each in turn.
// One distance from the batch frame united with the column frame is at least
// every per-piece distance, so the batch removes a superset, never less.
#[expect(
    clippy::too_many_arguments,
    reason = "Batch subtraction inventories shared support, interfaces and temporary material"
)]
fn subtract_batch(
    batch: &mut PieceBatch,
    column: &mut OwnedMaterial,
    proximity: &mut proximity::ColumnProximity,
    xy_clearance_mm: f64,
    retained: usize,
    owned_bytes: usize,
    support: &[Rc<OwnedMaterial>],
    interfaces: &[Rc<OwnedMaterial>],
    protected: &OwnedMaterial,
    work: &mut usize,
    foreign_work: usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let staged = std::mem::take(batch);
    let (Some(frame), Some(b)) = (staged.frame, column.bounding_rect()) else {
        return Ok(());
    };
    let distance = clearance_distance_in_frame(
        frame,
        [[b.min().x, b.min().y], [b.max().x, b.max().y]],
        xy_clearance_mm,
    )?;
    let owned_bytes = owned_bytes.saturating_add(proximity.bytes());
    // Union the raw pieces first: (A ∪ B) ⊕ D = (A ⊕ D) ∪ (B ⊕ D), and stacked
    // facet hulls then cross only their few own edges, not offset cycles.
    let merged = if staged.pieces.0.len() > 1 {
        let (units, bytes) = geometry_receipt(support, interfaces, &[protected, column]);
        // Packed overlay input and output each hold at most twice the staged
        // coordinates before the merged pieces replace the staged copy.
        super::remaining_geometry_units(
            retained.saturating_add(units).saturating_add(3 * staged.points),
            owned_bytes
                .saturating_add(bytes)
                .saturating_add(3 * staged.owned_bytes()),
        )?;
        boolean_budget(&staged.pieces, &staged.pieces, cancel)?;
        let merged = union_pieces(&staged.pieces);
        region::check_size(&merged)?;
        merged
    } else {
        staged.pieces.clone()
    };
    let clearance = OwnedMaterial::new(offset_owned(
        &merged,
        distance,
        retained.saturating_add(staged.points),
        owned_bytes.saturating_add(staged.owned_bytes()),
        support,
        interfaces,
        &[protected, column],
        cancel,
    )?);
    drop(staged);
    drop(merged);
    remaining_geometry_budget(
        retained,
        owned_bytes,
        support,
        interfaces,
        &[protected, column, &clearance],
    )?;
    charge_boolean(column, &clearance, work, foreign_work, cancel)?;
    let next = OwnedMaterial::new(column.material.difference(&clearance.material));
    remaining_geometry_budget(
        retained,
        owned_bytes,
        support,
        interfaces,
        &[protected, column, &clearance, &next],
    )?;
    *column = next;
    region::check_size(column)?;
    drop(clearance);
    *proximity = index_column(
        column,
        retained,
        owned_bytes,
        support,
        interfaces,
        &[protected, column],
    )?;
    Ok(())
}

/// A clearance offset reaches at most its circumscribed disk plus overlay
/// padding beyond `distance`; twice the offset tolerance encloses both.
fn separation_reach(distance: f64) -> f64 {
    (distance + 2.0 * super::OFFSET_TOLERANCE_MM).next_up()
}

/// Admit the transient column edge index alongside the live owners it serves.
fn index_column(
    column: &MultiPolygon<f64>,
    retained: usize,
    owned_bytes: usize,
    support: &[Rc<OwnedMaterial>],
    interfaces: &[Rc<OwnedMaterial>],
    temporary: &[&OwnedMaterial],
) -> Result<proximity::ColumnProximity, ManufacturingError> {
    let estimate = proximity::ColumnProximity::bytes_for(column);
    remaining_geometry_budget(
        retained,
        owned_bytes.saturating_add(estimate),
        support,
        interfaces,
        temporary,
    )?;
    Ok(proximity::ColumnProximity::new(column))
}

fn void_reaches_corridor(
    scope: &spiling_geometry::ProtectedVoidHeightScope,
    corridor_lower: f64,
    corridor_upper: f64,
) -> bool {
    match scope {
        spiling_geometry::ProtectedVoidHeightScope::Bounded { lower_mm, upper_mm } => {
            *lower_mm <= corridor_upper && *upper_mm >= corridor_lower
        }
        spiling_geometry::ProtectedVoidHeightScope::OpenBelow { upper_mm } => {
            *upper_mm >= corridor_lower
        }
    }
}

fn candidate_clearance_distance(
    occupied: &Polygon<f64>,
    column: &MultiPolygon<f64>,
    requested: f64,
) -> Result<Option<f64>, ManufacturingError> {
    let a = occupied.bounding_rect().ok_or_else(|| {
        error(
            ManufacturingErrorCode::NativeAnalysisIndeterminate,
            "native outer operand has no finite bounds",
        )
    })?;
    let Some(b) = column.bounding_rect() else {
        return Ok(None);
    };
    let distance = clearance_distance_in_frame(
        [[a.min().x, a.min().y], [a.max().x, a.max().y]],
        [[b.min().x, b.min().y], [b.max().x, b.max().y]],
        requested,
    )?;
    // A disjoint outward expanded box cannot affect this candidate. Avoid
    // offset/overlay work, while final raw-source capsule proof stays separate.
    if (a.max().x + distance).next_up() < b.min().x
        || (a.min().x - distance).next_down() > b.max().x
        || (a.max().y + distance).next_up() < b.min().y
        || (a.min().y - distance).next_down() > b.max().y
    {
        return Ok(None);
    }
    Ok(Some(distance))
}

fn clearance_distance_in_frame(
    a: [[f64; 2]; 2],
    b: [[f64; 2]; 2],
    requested: f64,
) -> Result<f64, ManufacturingError> {
    // Offset allowance and Boolean padding only propose a candidate work
    // frame. They do not prove raw-C containment after overlay. Final actual
    // XYZ6 support capsules have a separate shared geometry proof.
    let reserve = super::OFFSET_TOLERANCE_MM + 2.0f64.sqrt() * (super::LAYER_QUANTIZATION_MM * 0.5);
    let base = (requested + reserve).next_up();
    let min = [a[0][0].min(b[0][0]), a[0][1].min(b[0][1])];
    let max = [a[1][0].max(b[1][0]), a[1][1].max(b[1][1])];
    let padding = super::boolean::overlay_padding(
        min.map(|v| (v - base - super::OFFSET_TOLERANCE_MM).next_down()),
        max.map(|v| (v + base + super::OFFSET_TOLERANCE_MM).next_up()),
    )?;
    let guard = (2.0 * 2.0f64.sqrt() * padding).next_up();
    let distance = (base + guard).next_up();
    let actual_padding = super::boolean::overlay_padding(
        min.map(|v| (v - distance - super::OFFSET_TOLERANCE_MM).next_down()),
        max.map(|v| (v + distance + super::OFFSET_TOLERANCE_MM).next_up()),
    )?;
    if !distance.is_finite() || (2.0f64.sqrt() * actual_padding).next_up() > guard {
        return Err(error(
            ManufacturingErrorCode::NativeConditioningFailure,
            "support exclusion candidate cannot admit its offset/overlay/XYZ6 work frame",
        ));
    }
    Ok(distance)
}

#[cfg(test)]
mod tests {
    use super::*;
    use geo::{line_string, polygon};

    #[test]
    fn support_owned_work_refuses_global_planar_counter_overflow() {
        let material = MultiPolygon(vec![polygon![
            (x: 0.0, y: 0.0), (x: 3.0, y: 0.0),
            (x: 3.0, y: 2.0), (x: 0.0, y: 2.0),
        ]]);
        let pairs = region::point_count(&material).pow(2);
        let mut bank = SupportWork {
            planar: usize::MAX - pairs + 1,
            ..SupportWork::default()
        };
        let mut work = bank.planar;
        let foreign = bank.foreign_work().unwrap();
        let failure = charge_boolean(
            &material,
            &material,
            &mut work,
            foreign,
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(failure.code, ManufacturingErrorCode::ResourceLimit);
        let before = bank.planar;
        bank.finish(work, foreign, 0).unwrap();
        assert_eq!(bank.planar, before);
        assert_eq!(bank.support_owned, 0);
    }

    #[test]
    fn retained_support_capacity_is_byte_bounded_and_shared_once() {
        let owner = || {
            OwnedMaterial::new(MultiPolygon(vec![Polygon::new(
                geo::LineString(Vec::with_capacity(60_001)),
                Vec::new(),
            )]))
            .into_shared()
        };
        let a = owner();
        let b = owner();
        let support = vec![Rc::clone(&a), Rc::clone(&a), Rc::clone(&b)];
        let interfaces = vec![Rc::clone(&b), Rc::clone(&a)];
        let (units, bytes) = geometry_receipt(&support, &interfaces, &[]);
        assert_eq!(units, a.units + b.units);
        assert_eq!(bytes, a.bytes + a.header_bytes + b.bytes + b.header_bytes);
        assert!(remaining_geometry_budget(0, 0, &support, &interfaces, &[]).is_ok());
        assert!(
            remaining_geometry_budget(
                0,
                MAX_MANUFACTURING_RETAINED_BYTES as usize,
                &support,
                &interfaces,
                &[]
            )
            .is_err()
        );
    }

    fn row(kind: DepositionKind, a: f64, b: f64, z: f64) -> DepositionPath {
        DepositionPath {
            material_domain: None,
            kind,
            width: WidthProfile::Constant { width_mm: 0.6 },
            physical_z_mm: [z - 0.4, z],
            owner: match kind {
                DepositionKind::BottomSkin => Some(DepositionOwner::MandatorySkin {
                    skin: ProcessSkinKind::BottomSkin,
                }),
                DepositionKind::TopSkin => Some(DepositionOwner::MandatorySkin {
                    skin: ProcessSkinKind::TopSkin,
                }),
                _ => None,
            },
            points_mm: vec![[a, 0.0, z], [b, 0.0, z]],
            arcs: Vec::new(),
        }
    }

    #[test]
    fn genuine_bridge_rows_join_existing_ownership_pieces_without_extra_fill() {
        let mut candidate = RasterCandidate {
            skin_command_fits: Vec::new(),
            skin_bridge_domains: Vec::new(),
            paths: vec![
                row(DepositionKind::SolidFill, 1.0, 3.0, 0.8),
                row(DepositionKind::BottomSkin, 3.0, 7.0, 0.8),
                row(DepositionKind::SolidFill, 7.0, 9.0, 0.8),
            ],
            whole_rows: vec![row(DepositionKind::Bridge, 1.0, 9.0, 0.8)],
            process_interfaces: Vec::new(),
        };
        let previous = [
            row(DepositionKind::SolidFill, 0.0, 3.0, 0.4),
            row(DepositionKind::SolidFill, 7.0, 10.0, 0.4),
        ];
        let recipe = crate::planner::tests::recipe(1.0);
        let mut query = AnchorWorkspace::default();
        assert!(
            coalesce_bridge_rows(
                &mut candidate,
                BridgeRowContext {
                    axis: region::HatchAxis::X,
                    demanded_reorientation: false,
                    retained_points: 0,
                    owned_bytes: 0,
                    recipe: &recipe,
                },
                previous.iter(),
                std::iter::empty(),
                &mut query,
                &mut 0,
                &AtomicBool::new(false)
            )
            .unwrap()
        );
        assert_eq!(candidate.paths.len(), 1);
        assert_eq!(candidate.paths[0].kind, DepositionKind::Bridge);
        assert_eq!(candidate.paths[0].owner, None);
        assert_eq!(
            candidate.paths[0].points_mm,
            [[1.0, 0.0, 0.8], [9.0, 0.0, 0.8]]
        );
    }

    #[test]
    fn structured_infill_keeps_roads_while_skin_rows_join() {
        for policy in [SparseInfillPolicy::Grid, SparseInfillPolicy::web_roof()] {
            let mut sparse = row(DepositionKind::SparseFill, 12.0, 15.0, 0.8);
            sparse.owner = Some(DepositionOwner::Regional { region_index: 0 });
            let mut sparse_row = sparse.clone();
            sparse_row.kind = DepositionKind::Bridge;
            let mut candidate = RasterCandidate {
                skin_command_fits: Vec::new(),
                skin_bridge_domains: Vec::new(),
                paths: vec![
                    row(DepositionKind::SolidFill, 1.0, 3.0, 0.8),
                    row(DepositionKind::BottomSkin, 3.0, 7.0, 0.8),
                    row(DepositionKind::SolidFill, 7.0, 9.0, 0.8),
                    sparse.clone(),
                ],
                whole_rows: vec![row(DepositionKind::Bridge, 1.0, 9.0, 0.8), sparse_row],
                process_interfaces: Vec::new(),
            };
            let previous = [
                row(DepositionKind::SolidFill, 0.0, 3.0, 0.4),
                row(DepositionKind::SolidFill, 7.0, 16.0, 0.4),
            ];
            let mut recipe = crate::planner::tests::recipe(0.08);
            recipe.sparse_infill = policy;
            assert!(
                coalesce_bridge_rows(
                    &mut candidate,
                    BridgeRowContext {
                        axis: region::HatchAxis::X,
                        demanded_reorientation: false,
                        retained_points: 0,
                        owned_bytes: 0,
                        recipe: &recipe,
                    },
                    previous.iter(),
                    std::iter::empty(),
                    &mut AnchorWorkspace::default(),
                    &mut 0,
                    &AtomicBool::new(false),
                )
                .unwrap()
            );
            assert_eq!(candidate.paths.len(), 2);
            assert!(candidate.paths.contains(&sparse));
            let bridge = candidate
                .paths
                .iter()
                .find(|path| path.kind == DepositionKind::Bridge)
                .unwrap();
            assert_eq!(bridge.points_mm, [[1.0, 0.0, 0.8], [9.0, 0.0, 0.8]]);
        }
    }

    #[test]
    fn structured_infill_is_a_barrier_to_skin_row_joining() {
        for policy in [SparseInfillPolicy::Grid, SparseInfillPolicy::web_roof()] {
            let mut sparse = row(DepositionKind::SparseFill, 3.0, 7.0, 0.8);
            sparse.owner = Some(DepositionOwner::Regional { region_index: 0 });
            let mut candidate = RasterCandidate {
                skin_command_fits: Vec::new(),
                skin_bridge_domains: Vec::new(),
                paths: vec![
                    row(DepositionKind::BottomSkin, 1.0, 3.0, 0.8),
                    sparse.clone(),
                    row(DepositionKind::BottomSkin, 7.0, 9.0, 0.8),
                ],
                whole_rows: vec![row(DepositionKind::Bridge, 1.0, 9.0, 0.8)],
                process_interfaces: Vec::new(),
            };
            let previous = [
                row(DepositionKind::SolidFill, 0.0, 1.2, 0.4),
                row(DepositionKind::SolidFill, 2.8, 7.2, 0.4),
                row(DepositionKind::SolidFill, 8.8, 10.0, 0.4),
            ];
            let mut recipe = crate::planner::tests::recipe(0.08);
            recipe.sparse_infill = policy;
            assert!(
                !coalesce_bridge_rows(
                    &mut candidate,
                    BridgeRowContext {
                        axis: region::HatchAxis::X,
                        demanded_reorientation: false,
                        retained_points: 0,
                        owned_bytes: 0,
                        recipe: &recipe,
                    },
                    previous.iter(),
                    std::iter::empty(),
                    &mut AnchorWorkspace::default(),
                    &mut 0,
                    &AtomicBool::new(false),
                )
                .unwrap()
            );
            assert!(candidate.paths.contains(&sparse));
            assert_eq!(candidate.paths.len(), 3);
            assert!(
                candidate
                    .paths
                    .iter()
                    .filter(|path| path.kind == DepositionKind::Bridge)
                    .all(|path| {
                        let start = path.points_mm[0][0].min(path.points_mm[1][0]);
                        let stop = path.points_mm[0][0].max(path.points_mm[1][0]);
                        stop <= 3.0 || start >= 7.0
                    })
            );
        }
    }

    #[test]
    fn bridge_core_origin_requires_unanimous_original_mandatory_skin() {
        use DepositionKind::{BottomSkin, Bridge, SparseFill, TopSkin};
        let previous = [
            row(DepositionKind::SolidFill, 0.0, 3.0, 0.4),
            row(DepositionKind::SolidFill, 7.0, 10.0, 0.4),
        ];
        let recipe = crate::planner::tests::recipe(1.0);
        for (kinds, expected) in [
            (
                [TopSkin, TopSkin, TopSkin],
                Some(DepositionOwner::MandatorySkin {
                    skin: ProcessSkinKind::TopSkin,
                }),
            ),
            (
                [BottomSkin, BottomSkin, BottomSkin],
                Some(DepositionOwner::MandatorySkin {
                    skin: ProcessSkinKind::BottomSkin,
                }),
            ),
            ([TopSkin, BottomSkin, TopSkin], None),
            ([SparseFill, TopSkin, TopSkin], None),
            ([TopSkin, TopSkin, SparseFill], None),
            ([Bridge, Bridge, Bridge], None),
        ] {
            for converted in [false, true] {
                let mut paths = vec![
                    row(kinds[0], 1.0, 3.0, 0.8),
                    row(kinds[1], 3.0, 7.0, 0.8),
                    row(kinds[2], 7.0, 9.0, 0.8),
                ];
                if converted {
                    for path in &mut paths {
                        path.kind = Bridge;
                    }
                }
                let mut candidate = RasterCandidate {
                    skin_command_fits: Vec::new(),
                    skin_bridge_domains: Vec::new(),
                    paths,
                    whole_rows: vec![row(Bridge, 1.0, 9.0, 0.8)],
                    process_interfaces: Vec::new(),
                };
                assert!(
                    coalesce_bridge_rows(
                        &mut candidate,
                        BridgeRowContext {
                            axis: region::HatchAxis::X,
                            demanded_reorientation: false,
                            retained_points: 0,
                            owned_bytes: 0,
                            recipe: &recipe,
                        },
                        previous.iter(),
                        std::iter::empty(),
                        &mut AnchorWorkspace::default(),
                        &mut 0,
                        &AtomicBool::new(false),
                    )
                    .unwrap()
                );
                assert_eq!(candidate.paths.len(), 1);
                assert_eq!(candidate.paths[0].kind, Bridge);
                assert_eq!(candidate.paths[0].owner, expected);
            }
        }
    }

    #[test]
    fn chosen_skin_witness_matches_only_unchanged_role_origin_and_exact_xy() {
        let bond = ProcessEndpointBond::Skin {
            skin_kind: ProcessSkinKind::TopSkin,
            process_kind: DepositionKind::Bridge,
            skin_segment_mm: [[3.0, 0.0], [7.0, 0.0]],
            wall_segment_mm: [[3.0, -1.0], [3.0, 1.0]],
        };
        let mut path = row(DepositionKind::TopSkin, 3.0, 7.0, 0.8);
        path.kind = DepositionKind::Bridge;
        assert!(matches_skin_witness(&path, &bond));
        path.points_mm.reverse();
        assert!(matches_skin_witness(&path, &bond));
        path.points_mm[0][0] += 0.000001;
        assert!(!matches_skin_witness(&path, &bond));
        path.points_mm[0][0] = 7.0;
        path.kind = DepositionKind::TopSkin;
        assert!(!matches_skin_witness(&path, &bond));
        path.kind = DepositionKind::Bridge;
        path.owner = None;
        assert!(!matches_skin_witness(&path, &bond));
        path.owner = Some(DepositionOwner::MandatorySkin {
            skin: ProcessSkinKind::BottomSkin,
        });
        assert!(!matches_skin_witness(&path, &bond));
    }

    #[test]
    fn bridge_row_candidates_cannot_fill_a_missing_sparse_piece() {
        let mut candidate = RasterCandidate {
            skin_command_fits: Vec::new(),
            skin_bridge_domains: Vec::new(),
            paths: vec![
                row(DepositionKind::SparseFill, 1.0, 3.0, 0.8),
                row(DepositionKind::SparseFill, 7.0, 9.0, 0.8),
            ],
            whole_rows: vec![row(DepositionKind::Bridge, 1.0, 9.0, 0.8)],
            process_interfaces: Vec::new(),
        };
        let previous = [
            row(DepositionKind::SolidFill, 0.0, 3.0, 0.4),
            row(DepositionKind::SolidFill, 7.0, 10.0, 0.4),
        ];
        let recipe = crate::planner::tests::recipe(0.2);
        let mut query = AnchorWorkspace::default();
        assert!(
            !coalesce_bridge_rows(
                &mut candidate,
                BridgeRowContext {
                    axis: region::HatchAxis::X,
                    demanded_reorientation: false,
                    retained_points: 0,
                    owned_bytes: 0,
                    recipe: &recipe,
                },
                previous.iter(),
                std::iter::empty(),
                &mut query,
                &mut 0,
                &AtomicBool::new(false)
            )
            .unwrap()
        );
        assert_eq!(candidate.paths.len(), 2);
        assert!(
            candidate
                .paths
                .iter()
                .all(|path| path.kind == DepositionKind::SparseFill)
        );
    }

    #[test]
    fn overlapping_row_groups_preserve_required_material_and_unrequested_gaps() {
        let recipe = crate::planner::tests::recipe(0.2);
        let cancel = AtomicBool::new(false);
        let previous = [3.0, 7.5].map(|x| DepositionPath {
            material_domain: None,
            kind: DepositionKind::SparseFill,
            width: WidthProfile::Constant { width_mm: 0.6 },
            physical_z_mm: [0.0, 0.4],
            owner: None,
            points_mm: vec![[x, -1.0, 0.4], [x, 1.0, 0.4]],
            arcs: Vec::new(),
        });
        for nested in [false, true] {
            let required = vec![
                row(DepositionKind::SparseFill, 1.0, 1.1, 0.8),
                row(
                    DepositionKind::TopSkin,
                    3.2,
                    if nested { 6.8 } else { 5.1 },
                    0.8,
                ),
                row(
                    DepositionKind::SparseFill,
                    if nested { 4.0 } else { 6.8 },
                    if nested { 3.4 } else { 4.9 },
                    0.8,
                ),
            ];
            let mut candidate = RasterCandidate {
                skin_command_fits: Vec::new(),
                skin_bridge_domains: Vec::new(),
                paths: required.clone(),
                whole_rows: vec![row(DepositionKind::Bridge, 0.5, 9.0, 0.8)],
                process_interfaces: Vec::new(),
            };
            let mut anchors = AnchorWorkspace::default();
            let mut work = 0;
            assert!(
                coalesce_bridge_rows(
                    &mut candidate,
                    BridgeRowContext {
                        axis: region::HatchAxis::X,
                        demanded_reorientation: false,
                        retained_points: 0,
                        owned_bytes: 0,
                        recipe: &recipe,
                    },
                    previous.iter(),
                    std::iter::empty(),
                    &mut anchors,
                    &mut work,
                    &cancel,
                )
                .unwrap()
            );
            for owner in &required {
                let start = owner.points_mm[0][0].min(owner.points_mm[1][0]);
                let end = owner.points_mm[0][0].max(owner.points_mm[1][0]);
                assert!(
                    candidate.paths.iter().any(|actual| {
                        actual.points_mm[0][0].min(actual.points_mm[1][0]) <= start
                            && actual.points_mm[0][0].max(actual.points_mm[1][0]) >= end
                    }),
                    "overlap union must retain every required owner"
                );
            }
            assert!(
                candidate.paths.iter().all(|actual| {
                    actual.points_mm[0][0].max(actual.points_mm[1][0]) <= 1.1
                        || actual.points_mm[0][0].min(actual.points_mm[1][0]) >= 3.2
                }),
                "a separate required group must not fill the unrequested interval"
            );
            let repaired: Vec<_> = candidate
                .paths
                .iter()
                .filter(|path| path.points_mm.iter().all(|point| point[0] >= 3.2))
                .cloned()
                .collect();
            assert!(
                strokes::analyze(
                    &repaired,
                    previous.iter(),
                    std::iter::empty(),
                    &recipe,
                    &mut anchors,
                    &mut work,
                    &cancel,
                )
                .unwrap()
                .failure
                .is_none(),
                "an unrelated unanchored group must not prevent a real bridge"
            );
            assert!(
                strokes::analyze(
                    &candidate.paths,
                    previous.iter(),
                    std::iter::empty(),
                    &recipe,
                    &mut anchors,
                    &mut work,
                    &cancel,
                )
                .unwrap()
                .failure
                .is_some(),
                "repairing one group cannot suppress another group's demand"
            );
        }
    }

    #[test]
    fn separate_required_groups_extend_only_to_nearest_real_interior_anchors() {
        let recipe = crate::planner::tests::recipe(0.2);
        let cancel = AtomicBool::new(false);
        let required = vec![
            row(DepositionKind::TopSkin, 2.0, 2.4, 0.8),
            row(DepositionKind::SparseFill, 8.0, 9.0, 0.8),
        ];
        for interior_anchor in [None, Some(4.0), Some(7.7)] {
            let previous: Vec<_> = [Some(1.7), interior_anchor, Some(8.5)]
                .into_iter()
                .flatten()
                .map(|x| DepositionPath {
                    material_domain: None,
                    kind: DepositionKind::SparseFill,
                    width: WidthProfile::Constant { width_mm: 0.6 },
                    physical_z_mm: [0.0, 0.4],
                    owner: None,
                    points_mm: vec![[x, -1.0, 0.4], [x, 1.0, 0.4]],
                    arcs: Vec::new(),
                })
                .collect();
            let mut candidate = RasterCandidate {
                skin_command_fits: Vec::new(),
                skin_bridge_domains: Vec::new(),
                paths: required.clone(),
                whole_rows: vec![row(DepositionKind::Bridge, 1.0, 10.0, 0.8)],
                process_interfaces: Vec::new(),
            };
            let mut anchors = AnchorWorkspace::default();
            let mut work = 0;
            assert!(
                strokes::analyze(
                    &candidate.paths,
                    previous.iter(),
                    std::iter::empty(),
                    &recipe,
                    &mut anchors,
                    &mut work,
                    &cancel,
                )
                .unwrap()
                .failure
                .is_some(),
                "one endpoint anchor cannot support the original over-limit overhang"
            );
            coalesce_bridge_rows(
                &mut candidate,
                BridgeRowContext {
                    axis: region::HatchAxis::X,
                    demanded_reorientation: false,
                    retained_points: 0,
                    owned_bytes: 0,
                    recipe: &recipe,
                },
                previous.iter(),
                std::iter::empty(),
                &mut anchors,
                &mut work,
                &cancel,
            )
            .unwrap();
            let admitted = strokes::analyze(
                &candidate.paths,
                previous.iter(),
                std::iter::empty(),
                &recipe,
                &mut anchors,
                &mut work,
                &cancel,
            )
            .unwrap();
            if interior_anchor == Some(4.0) {
                assert!(admitted.failure.is_none());
                assert!(
                    candidate.paths.iter().all(|path| {
                        path.points_mm.iter().all(|point| point[0] <= 4.0)
                            || path.points_mm.iter().all(|point| point[0] >= 8.0)
                    }),
                    "the nearest anchor must stop extension well before the unrequested gap is filled"
                );
                for owner in &required {
                    let start = owner.points_mm[0][0].min(owner.points_mm[1][0]);
                    let end = owner.points_mm[0][0].max(owner.points_mm[1][0]);
                    assert!(candidate.paths.iter().any(|actual| {
                        actual.points_mm[0][0].min(actual.points_mm[1][0]) <= start
                            && actual.points_mm[0][0].max(actual.points_mm[1][0]) >= end
                    }));
                }
            } else {
                assert!(admitted.failure.is_some());
                assert_eq!(
                    candidate.paths, required,
                    "absent interior anchors or an extension that erases the current footprint gap cannot repair demand"
                );
            }
        }
    }

    #[test]
    fn current_row_footprint_overlap_is_strict_and_never_a_previous_anchor() {
        let mut recipe = crate::planner::tests::recipe(0.2);
        let cancel = AtomicBool::new(false);
        let unit = super::super::LAYER_QUANTIZATION_MM;
        let end = 4.0;
        for (axis, width) in [
            (region::HatchAxis::X, 0.5),
            (region::HatchAxis::Y, 0.5),
            (region::HatchAxis::X, 0.6),
            (region::HatchAxis::Y, 0.6),
        ] {
            recipe.bead_width_mm = width;
            let orient = |mut path: DepositionPath| {
                path.width = WidthProfile::Constant { width_mm: width };
                if axis == region::HatchAxis::Y {
                    for point in &mut path.points_mm {
                        point.swap(0, 1);
                    }
                }
                path
            };
            let previous = [2.5, 7.5].map(|x| {
                orient(DepositionPath {
                    material_domain: None,
                    kind: DepositionKind::SparseFill,
                    width: WidthProfile::Constant { width_mm: width },
                    physical_z_mm: [0.0, 0.4],
                    owner: None,
                    points_mm: vec![[x, -1.0, 0.4], [x, 1.0, 0.4]],
                    arcs: Vec::new(),
                })
            });
            for (left_end, right_start, connected) in [
                (end, end + recipe.bead_width_mm - unit, true),
                // Non-dyadic decimal tangency must not become a positive
                // overlap merely because decoded subtraction rounds below it.
                (end, end + recipe.bead_width_mm, false),
                (end, end + recipe.bead_width_mm + unit, false),
                // A nominal overlap may disappear on the actual emitted row.
                (
                    end + 0.49 * unit,
                    end + recipe.bead_width_mm + 0.01 * unit,
                    false,
                ),
                // A tiny nominal gap may straddle the rounding half-grid.
                (end + 0.49 * unit, end + 0.51 * unit, true),
            ] {
                for missing_previous in [false, true] {
                    let required = vec![
                        orient(row(DepositionKind::TopSkin, 3.2, left_end, 0.8)),
                        orient(row(DepositionKind::SparseFill, right_start, 6.8, 0.8)),
                    ];
                    let mut candidate = RasterCandidate {
                        skin_command_fits: Vec::new(),
                        skin_bridge_domains: Vec::new(),
                        paths: required.clone(),
                        whole_rows: vec![orient(row(DepositionKind::Bridge, 1.0, 9.0, 0.8))],
                        process_interfaces: Vec::new(),
                    };
                    let prior = previous.iter().filter(|_| !missing_previous);
                    let mut anchors = AnchorWorkspace::default();
                    let mut work = 0;
                    assert!(
                        strokes::analyze(
                            &candidate.paths,
                            prior.clone(),
                            std::iter::empty(),
                            &recipe,
                            &mut anchors,
                            &mut work,
                            &cancel,
                        )
                        .unwrap()
                        .failure
                        .is_some()
                    );
                    coalesce_bridge_rows(
                        &mut candidate,
                        BridgeRowContext {
                            axis,
                            demanded_reorientation: false,
                            retained_points: 0,
                            owned_bytes: 0,
                            recipe: &recipe,
                        },
                        prior.clone(),
                        std::iter::empty(),
                        &mut anchors,
                        &mut work,
                        &cancel,
                    )
                    .unwrap();
                    let admitted = strokes::analyze(
                        &candidate.paths,
                        prior,
                        std::iter::empty(),
                        &recipe,
                        &mut anchors,
                        &mut work,
                        &cancel,
                    )
                    .unwrap();
                    assert_eq!(
                        admitted.failure.is_none(),
                        connected && !missing_previous,
                        "only positive current footprint connectivity plus real previous anchors can repair the row"
                    );
                    if !connected || missing_previous {
                        assert_eq!(
                            candidate.paths, required,
                            "tangency, missing current material or absent previous anchors cannot create a connector"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn demanded_reorientation_constructs_real_bridges_from_supported_overhangs() {
        let cancel = AtomicBool::new(false);
        let fill = MultiPolygon(vec![polygon![
            (x: 1.1, y: -2.0), (x: 4.0, y: -2.0),
            (x: 4.0, y: 2.0), (x: 1.1, y: 2.0), (x: 1.1, y: -2.0),
        ]]);
        // Ordinary admission allows this short one-ended overhang. Reorienting
        // another demanded row cannot simply relabel that supported alternative.
        let required = vec![row(DepositionKind::TopSkin, 1.1, 1.355, 0.8)];
        let prior = [0.75, 3.0].map(|x| DepositionPath {
            material_domain: None,
            kind: DepositionKind::SparseFill,
            owner: None,
            width: WidthProfile::PerEdge {
                widths_mm: vec![0.6],
            },
            physical_z_mm: [0.0, 0.4],
            points_mm: vec![[x, -1.0, 0.4], [x, 1.0, 0.4]],
            arcs: Vec::new(),
        });
        for (reorientation, missing_anchor, limit) in [
            (false, false, 6.0),
            (true, false, 6.0),
            (true, true, 6.0),
            (true, false, 1.0),
        ] {
            let mut recipe = crate::planner::tests::recipe(0.1);
            recipe.process.max_bridge_span_mm = limit;
            let layer = region::HatchLayer {
                roof: None,
                width_mm: recipe.bead_width_mm,
                z: 0.8,
                lower_z: 0.4,
                height: 0.4,
                axis: region::HatchAxis::X,
                top_axis: region::HatchAxis::X,
                recipe: &recipe,
            };
            let previous = prior
                .iter()
                .enumerate()
                .filter(|(index, _)| !missing_anchor || *index == 0)
                .map(|(_, path)| path);
            let mut candidate = RasterCandidate {
                paths: required.clone(),
                whole_rows: region::bridge_rows(&fill, &[], &required, &layer, &mut 0, &cancel)
                    .unwrap(),
                process_interfaces: Vec::new(),
                skin_bridge_domains: Vec::new(),
                skin_command_fits: Vec::new(),
            };
            let mut anchors = AnchorWorkspace::default();
            let mut work = 0;
            let proof = component_admission(
                (&mut candidate, region::HatchAxis::X, 100_000, reorientation),
                previous.clone(),
                std::iter::empty(),
                &recipe,
                &mut anchors,
                &mut work,
                &cancel,
            )
            .unwrap();
            assert!(proof.supported);
            if reorientation && !missing_anchor && limit == 6.0 {
                assert_eq!(proof.bridges, [0]);
                let path = &candidate.paths[0];
                assert_eq!(path.kind, DepositionKind::Bridge);
                assert_eq!(path.owner, required[0].owner);
                assert_eq!(path.width, required[0].width);
                assert_eq!(path.physical_z_mm, required[0].physical_z_mm);
                assert_eq!(path.points_mm[0], required[0].points_mm[0]);
                assert!(path.points_mm[1][0] > required[0].points_mm[1][0]);
                assert!(path.points_mm[1][0] < prior[1].points_mm[0][0]);
                let actual = strokes::analyze(
                    &candidate.paths,
                    previous,
                    std::iter::empty(),
                    &recipe,
                    &mut anchors,
                    &mut work,
                    &cancel,
                )
                .unwrap();
                assert!(actual.failure.is_none());
                assert_eq!(actual.bridges, [0]);
            } else {
                assert!(proof.bridges.is_empty());
                assert_eq!(candidate.paths, required);
            }
        }
    }

    #[test]
    fn top_skin_rows_extend_only_to_actual_sparse_anchors() {
        let cancel = AtomicBool::new(false);
        let recipe = crate::planner::tests::recipe(0.1);
        let fill = MultiPolygon(vec![polygon![
            (x: 1.0, y: -2.0), (x: 9.0, y: -2.0),
            (x: 9.0, y: 2.0), (x: 1.0, y: 2.0), (x: 1.0, y: -2.0),
        ]]);
        let required = vec![row(DepositionKind::TopSkin, 3.2, 6.8, 0.8)];
        let prior = [2.5, 7.5].map(|x| DepositionPath {
            material_domain: None,
            kind: DepositionKind::SparseFill,
            width: WidthProfile::Constant { width_mm: 0.6 },
            physical_z_mm: [0.0, 0.4],
            owner: None,
            points_mm: vec![[x, -1.0, 0.4], [x, 1.0, 0.4]],
            arcs: Vec::new(),
        });
        let layer = region::HatchLayer {
            roof: None,
            width_mm: recipe.bead_width_mm,
            z: 0.8,
            lower_z: 0.4,
            height: 0.4,
            axis: region::HatchAxis::X,
            top_axis: region::HatchAxis::X,
            recipe: &recipe,
        };
        for missing_anchor in [false, true] {
            let previous = prior.iter().filter(|_| !missing_anchor);
            let mut candidate = RasterCandidate {
                skin_command_fits: Vec::new(),
                skin_bridge_domains: Vec::new(),
                whole_rows: region::bridge_rows(&fill, &[], &required, &layer, &mut 0, &cancel)
                    .unwrap(),
                paths: required.clone(),
                process_interfaces: Vec::new(),
            };
            let mut anchors = AnchorWorkspace::default();
            let mut work = 0;
            assert!(
                strokes::analyze(
                    &candidate.paths,
                    previous.clone(),
                    std::iter::empty(),
                    &recipe,
                    &mut anchors,
                    &mut work,
                    &cancel,
                )
                .unwrap()
                .failure
                .is_some()
            );
            coalesce_bridge_rows(
                &mut candidate,
                BridgeRowContext {
                    axis: region::HatchAxis::X,
                    demanded_reorientation: false,
                    retained_points: 0,
                    owned_bytes: 0,
                    recipe: &recipe,
                },
                previous.clone(),
                std::iter::empty(),
                &mut anchors,
                &mut work,
                &cancel,
            )
            .unwrap();
            let admission = strokes::analyze(
                &candidate.paths,
                previous,
                std::iter::empty(),
                &recipe,
                &mut anchors,
                &mut work,
                &cancel,
            )
            .unwrap();
            if missing_anchor {
                assert!(
                    admission.failure.is_some(),
                    "source fill cannot supply an anchor"
                );
            } else {
                assert!(admission.failure.is_none());
                assert!(
                    candidate.paths.iter().any(|actual| {
                        actual.kind == DepositionKind::Bridge
                            && actual.owner
                                == Some(DepositionOwner::MandatorySkin {
                                    skin: ProcessSkinKind::TopSkin,
                                })
                            && actual.points_mm[0][0].min(actual.points_mm[1][0]) <= 3.2
                            && actual.points_mm[0][0].max(actual.points_mm[1][0]) >= 6.8
                            && actual
                                .points_mm
                                .iter()
                                .all(|point| (1.0..=9.0).contains(&point[0]))
                    }),
                    "a genuinely anchored bridge must retain the required skin inside positive fill"
                );
            }
        }
    }

    #[test]
    fn pre_floor_skin_deposits_anchors_under_upcoming_pocket_perimeters() {
        use crate::planner::classification::{ClassifiedRegions, fixed_owned_budget};
        let cancel = AtomicBool::new(false);
        let recipe = crate::planner::tests::recipe(0.25);
        let mut section = MultiPolygon(vec![polygon![
            (x: 0.0, y: 0.0), (x: 10.0, y: 0.0),
            (x: 10.0, y: 10.0), (x: 0.0, y: 10.0), (x: 0.0, y: 0.0),
        ]]);
        let mut upper = MultiPolygon(vec![Polygon::new(
            section.0[0].exterior().clone(),
            vec![line_string![
                (x: 4.0, y: 4.0), (x: 4.0, y: 6.0),
                (x: 6.0, y: 6.0), (x: 6.0, y: 4.0), (x: 4.0, y: 4.0),
            ]],
        )]);
        let layout = |material: &MultiPolygon<f64>| {
            std::sync::Arc::new(
                crate::planner::tests::wall_plan(
                    material,
                    &region::RequiredMaterial::from_section(material),
                    &recipe,
                    region::point_count(material),
                    &cancel,
                )
                .unwrap(),
            )
        };
        let inset = (recipe.perimeter_count as f64 - 0.5) * recipe.bead_width_mm;
        let mut section_inset = region::offset(&section, -inset, &cancel).unwrap();
        let mut upper_inset = region::offset(&upper, -inset, &cancel).unwrap();
        let fixture_bytes = crate::planner::native::material_bytes(&mut section)
            + crate::planner::native::material_bytes(&mut upper)
            + crate::planner::native::material_bytes(&mut section_inset)
            + crate::planner::native::material_bytes(&mut upper_inset);
        let fixture_units = material_units(&section)
            + material_units(&upper)
            + material_units(&section_inset)
            + material_units(&upper_inset);
        let previous_layout = layout(&section);
        let previous_units = fixture_units + previous_layout.retained_points;
        let previous_bytes =
            fixture_bytes + previous_layout.retained_bytes + 2 * std::mem::size_of::<usize>();
        let mut previous_regions = ClassifiedRegions::new(
            previous_layout,
            &section_inset,
            &upper_inset,
            fixed_owned_budget(previous_units, previous_bytes),
            &recipe,
            &cancel,
        )
        .unwrap();
        let classified_bytes = previous_regions.retained_bytes();
        let previous = previous_regions
            .paths(
                0.4,
                0.0,
                region::HatchAxis::X,
                region::HatchAxis::X,
                &recipe,
                None,
                region::OrdinaryHatchPhase::Centered,
                classified_bytes,
                &[],
                &mut 0,
                |units, bytes, _| {
                    crate::planner::remaining_geometry_units(units, bytes).map(|_| ())
                },
                &cancel,
            )
            .unwrap()
            .0;
        let upcoming_layout = layout(&upper);
        let upcoming_units = fixture_units
            + upcoming_layout.retained_points
            + previous
                .iter()
                .map(|path| path.points_mm.capacity())
                .sum::<usize>();
        let upcoming_bytes = fixture_bytes
            + upcoming_layout.retained_bytes
            + 2 * std::mem::size_of::<usize>()
            + crate::planner::bonds::path_bytes(&previous);
        let mut upcoming_regions = ClassifiedRegions::new(
            upcoming_layout,
            &section_inset,
            &upper_inset,
            fixed_owned_budget(upcoming_units, upcoming_bytes),
            &recipe,
            &cancel,
        )
        .unwrap();
        let classified_bytes = upcoming_regions.retained_bytes();
        let upcoming = upcoming_regions
            .paths(
                0.8,
                0.4,
                region::HatchAxis::Y,
                region::HatchAxis::Y,
                &recipe,
                None,
                region::OrdinaryHatchPhase::Centered,
                classified_bytes,
                &[],
                &mut 0,
                |units, bytes, _| {
                    crate::planner::remaining_geometry_units(units, bytes).map(|_| ())
                },
                &cancel,
            )
            .unwrap()
            .0
            .into_iter()
            .filter(|path| path.kind == DepositionKind::Perimeter)
            .collect::<Vec<_>>();
        let admission = strokes::analyze(
            &upcoming,
            previous.iter(),
            std::iter::empty(),
            &recipe,
            &mut AnchorWorkspace::default(),
            &mut 0,
            &cancel,
        )
        .unwrap();
        assert!(
            admission.failure.is_none(),
            "future walls need real deposited anchors"
        );
        assert!(
            admission.bridges.is_empty(),
            "a curved pocket rim cannot borrow straight-bridge authority"
        );
    }

    #[test]
    fn anchor_extension_cannot_cross_a_positive_fill_hole() {
        let cancel = AtomicBool::new(false);
        let recipe = crate::planner::tests::recipe(0.1);
        let exterior = geo::line_string![
            (x: 1.0, y: -2.0), (x: 9.0, y: -2.0),
            (x: 9.0, y: 2.0), (x: 1.0, y: 2.0), (x: 1.0, y: -2.0),
        ];
        let hole = geo::line_string![
            (x: 4.2, y: -1.0), (x: 4.2, y: 1.0),
            (x: 5.8, y: 1.0), (x: 5.8, y: -1.0), (x: 4.2, y: -1.0),
        ];
        let fill = MultiPolygon(vec![Polygon::new(exterior, vec![hole])]);
        let required = vec![
            row(DepositionKind::TopSkin, 3.0, 4.0, 0.8),
            row(DepositionKind::TopSkin, 6.0, 7.0, 0.8),
        ];
        let mut candidate = RasterCandidate {
            skin_command_fits: Vec::new(),
            skin_bridge_domains: Vec::new(),
            whole_rows: region::bridge_rows(
                &fill,
                &[],
                &required,
                &region::HatchLayer {
                    roof: None,
                    width_mm: recipe.bead_width_mm,
                    z: 0.8,
                    lower_z: 0.4,
                    height: 0.4,
                    axis: region::HatchAxis::X,
                    top_axis: region::HatchAxis::X,
                    recipe: &recipe,
                },
                &mut 0,
                &cancel,
            )
            .unwrap(),
            paths: required.clone(),
            process_interfaces: Vec::new(),
        };
        assert_eq!(candidate.whole_rows.len(), 2);
        let previous = [
            row(DepositionKind::SparseFill, 1.0, 2.0, 0.4),
            row(DepositionKind::SparseFill, 8.0, 9.0, 0.4),
        ];
        let mut anchors = AnchorWorkspace::default();
        assert!(
            !coalesce_bridge_rows(
                &mut candidate,
                BridgeRowContext {
                    axis: region::HatchAxis::X,
                    demanded_reorientation: false,
                    retained_points: 0,
                    owned_bytes: 0,
                    recipe: &recipe,
                },
                previous.iter(),
                std::iter::empty(),
                &mut anchors,
                &mut 0,
                &cancel,
            )
            .unwrap()
        );
        assert_eq!(candidate.paths, required);
        assert!(
            strokes::analyze(
                &candidate.paths,
                previous.iter(),
                std::iter::empty(),
                &recipe,
                &mut anchors,
                &mut 0,
                &cancel,
            )
            .unwrap()
            .failure
            .is_some()
        );
    }

    #[test]
    fn auxiliary_bridge_bounds_ignore_unrequested_tangent_and_thin_intervals() {
        let cancel = AtomicBool::new(false);
        let recipe = crate::planner::tests::recipe(0.1);
        let layer = region::HatchLayer {
            roof: None,
            width_mm: recipe.bead_width_mm,
            z: 0.8,
            lower_z: 0.4,
            height: 0.4,
            axis: region::HatchAxis::X,
            top_axis: region::HatchAxis::X,
            recipe: &recipe,
        };
        let required = vec![DepositionPath {
            material_domain: None,
            kind: DepositionKind::TopSkin,
            width: WidthProfile::Constant {
                width_mm: recipe.bead_width_mm,
            },
            physical_z_mm: [0.4, 0.8],
            owner: Some(DepositionOwner::MandatorySkin {
                skin: ProcessSkinKind::TopSkin,
            }),
            points_mm: vec![[1.0, 2.0, 0.8], [9.0, 2.0, 0.8]],
            arcs: Vec::new(),
        }];
        let polygon = |points: Vec<(f64, f64)>| Polygon::new(geo::LineString::from(points), vec![]);
        for tip in [2.0, 2.0 - 0.00000001] {
            let isolated = MultiPolygon(vec![
                polygon(vec![
                    (0.0, 0.0),
                    (10.0, 0.0),
                    (10.0, 4.0),
                    (0.0, 4.0),
                    (0.0, 0.0),
                ]),
                polygon(vec![(20.0, 4.0), (22.0, tip), (24.0, 4.0), (20.0, 4.0)]),
            ]);
            let connected = MultiPolygon(vec![polygon(vec![
                (0.0, 0.0),
                (10.0, 0.0),
                (10.0, 3.0),
                (20.0, 3.0),
                (22.0, tip),
                (24.0, 3.0),
                (24.0, 4.0),
                (0.0, 4.0),
                (0.0, 0.0),
            ])]);
            for fill in [&isolated, &connected] {
                let rows =
                    region::bridge_rows(fill, &[], &required, &layer, &mut 0, &cancel).unwrap();
                assert_eq!(rows.len(), 1);
                assert_eq!(rows[0].points_mm, [[0.0, 2.0, 0.8], [10.0, 2.0, 0.8]]);
            }
            // The same thin interval is still a genuine error if ordinary
            // deposition requests it. Only auxiliary, unowned bounds are skipped.
            assert_eq!(
                region::hatch(
                    &connected,
                    DepositionKind::SparseFill,
                    f64::MIN_POSITIVE,
                    &layer,
                    &mut Vec::new(),
                    &mut 0,
                    &cancel,
                )
                .unwrap_err()
                .code,
                ManufacturingErrorCode::UnsupportedGeometry,
            );
        }
    }
}
