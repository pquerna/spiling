// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::{LayerElevation, boolean, preflight, region};
use crate::{CompilerInput, check_cancel, error};
use geo::{Area, BooleanOps, BoundingRect, Intersects, MultiPolygon};
use spiling_contracts::geometry::{
    DefinitionId, GeometryErrorCode, OccurrenceId, OccurrenceRecord, RigidPoseMm,
};
use spiling_contracts::manufacturing::program_coordinates::{
    project_xyz6_bead_height_mm, project_xyz6_coordinate_mm, project_xyz6_segment_bead_height_mm,
    xyz6_coordinate_error_mm,
};
use spiling_contracts::manufacturing::*;
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, atomic::AtomicBool},
};

pub(super) const MATERIAL_ARC_BYTES: usize =
    std::mem::size_of::<MultiPolygon<f64>>() + 2 * std::mem::size_of::<usize>();

/// Original compiler sources shared by capture and final native certification.
pub(super) struct NativeSourceInputs<'a, 'model> {
    pub input: &'a CompilerInput<'model>,
    pub occurrences: &'a [&'a OccurrenceRecord],
    pub sources: &'a BTreeMap<DefinitionId, spiling_geometry::PreparedSolid<'model>>,
}

pub(super) struct NativeSpanRequest {
    pub index: u32,
    pub retained_points: usize,
    pub retained_output_bytes: usize,
}

struct SpanCaptureRequest {
    index: u32,
    end: u32,
    partition_end_z_bits: u64,
    retained_points: usize,
    retained_bytes: usize,
}

struct AdjacentSourceMaterials<'a, I> {
    lower: I,
    upper: &'a [SourceMaterial],
}

pub(super) struct ProtectedVoidDemand<'a> {
    pub native_bounds: &'a BTreeMap<OccurrenceId, spiling_contracts::geometry::AabbMm>,
    pub demand_bounds: &'a [[[f64; 2]; 2]],
    pub point_limit: usize,
    pub retained_native_owner_bytes: usize,
}

pub(super) struct SupportClearanceGeometry<'a> {
    pub sections: &'a [super::SectionSpan],
    pub schedule: &'a [LayerElevation],
    pub protected: &'a MultiPolygon<f64>,
    pub scopes: &'a [spiling_geometry::ProtectedVoidHeightScope],
}

struct ForbiddenPolygonScope<'a> {
    polygon: &'a geo::Polygon<f64>,
    height: spiling_geometry::ProtectedVoidHeightScope,
    label: &'a str,
}

struct ActualDepositionScope<'a> {
    layers: &'a [PrintLayer],
    schedule: &'a [LayerElevation],
    vertical: [f64; 2],
}

pub(super) struct NativeSpan {
    pub end: u32,
    /// All original source slabs carry exact closed-height invariance, not
    /// merely a bounded moving radial material product.
    pub invariant: bool,
    pub inner_material: Option<Arc<Vec<spiling_geometry::NativeInnerMaterialPrism>>>,
    pub material: Arc<MultiPolygon<f64>>,
    pub finished: Vec<(OccurrenceId, Arc<MultiPolygon<f64>>)>,
    pub contributions: Vec<(OccurrenceId, Arc<MultiPolygon<f64>>, usize)>,
    contribution_bytes: usize,
    pub occupied: Arc<super::ContinuousOccupied>,
    pub source_area_mm2: f64,
    pub point_count: usize,
    pub material_bytes: usize,
    pub finished_bytes: usize,
}

pub(super) struct CompletedCoverage {
    pub slab_count: u32,
    pub achieved_shape_error_mm: f64,
    pub source_features: NativeFeatureCoverageSummary,
    pub protected_voids: Vec<CapturedProtectedVoid>,
    /// Compact accepted source/recipe identities and stream binding remain
    /// owned until all continuous-target support consumers have finished.
    pub material_proof: spiling_geometry::CompletedNativeUnionProofSet,
}

pub(super) struct CapturedProtectedVoid {
    pub occurrence_id: OccurrenceId,
    pub capture: spiling_geometry::NativeVoidProjectionCapture,
}

fn inner_capture_bytes(captures: &Vec<spiling_geometry::NativeInnerMaterialPrism>) -> usize {
    std::mem::size_of_val(captures)
        + 2 * std::mem::size_of::<usize>()
        + captures.capacity() * std::mem::size_of::<spiling_geometry::NativeInnerMaterialPrism>()
        + captures
            .iter()
            .map(|capture| capture.retained_bytes())
            .sum::<usize>()
}

pub(super) fn void_capture_bytes(captures: &Vec<CapturedProtectedVoid>) -> usize {
    captures
        .capacity()
        .saturating_mul(std::mem::size_of::<CapturedProtectedVoid>())
        .saturating_add(
            captures
                .iter()
                .map(|value| value.capture.retained_bytes())
                .sum::<usize>(),
        )
}

/// Actual per-occurrence material passed unchanged into composition. Retain its
/// existing canonical source-M encoding for adjacent critical-collar targets;
/// never re-section or substitute the final union for an individual source.
struct SourceMaterial {
    leaf: spiling_geometry::NativeSourceLeafReceipt,
    actual: Vec<spiling_geometry::SectionLoopMm>,
}

impl SourceMaterial {
    fn retained_bytes(&self) -> usize {
        self.leaf
            .retained_bytes()
            .saturating_add(loop_bytes(&self.actual))
    }
}

pub(super) fn loop_bytes(loops: &Vec<spiling_geometry::SectionLoopMm>) -> usize {
    loops
        .capacity()
        .saturating_mul(std::mem::size_of::<spiling_geometry::SectionLoopMm>())
        .saturating_add(
            loops
                .iter()
                .map(|ring| {
                    ring.points_mm
                        .capacity()
                        .saturating_mul(std::mem::size_of::<[f64; 3]>())
                })
                .sum::<usize>(),
        )
}

pub(super) fn polygon_bytes(polygon: &mut geo::Polygon<f64>) -> usize {
    // Temporarily move the owned buffers out to observe the otherwise hidden
    // interior-record capacity. Reconstruction reuses every allocation.
    let (exterior, interiors) = std::mem::replace(
        polygon,
        geo::Polygon::new(geo::LineString(Vec::new()), Vec::new()),
    )
    .into_inner();
    let bytes = exterior
        .0
        .capacity()
        .saturating_mul(std::mem::size_of::<geo::Coord<f64>>())
        .saturating_add(
            interiors
                .capacity()
                .saturating_mul(std::mem::size_of::<geo::LineString<f64>>()),
        )
        .saturating_add(interiors.iter().fold(0usize, |bytes, ring| {
            bytes.saturating_add(
                ring.0
                    .capacity()
                    .saturating_mul(std::mem::size_of::<geo::Coord<f64>>()),
            )
        }));
    *polygon = geo::Polygon::new(exterior, interiors);
    bytes
}

pub(super) fn material_bytes(material: &mut MultiPolygon<f64>) -> usize {
    let bytes = material
        .0
        .capacity()
        .saturating_mul(std::mem::size_of::<geo::Polygon<f64>>());
    material.0.iter_mut().fold(bytes, |bytes, polygon| {
        bytes.saturating_add(polygon_bytes(polygon))
    })
}

fn occupied_bytes(occupied: &super::ContinuousOccupied) -> usize {
    occupied
        .pieces
        .capacity()
        .saturating_mul(std::mem::size_of::<super::OccupiedPiece>())
        .saturating_add(occupied.payload_bytes)
}

/// Caller-owned D/C/M buffers coexist with the next native query. Shared
/// continuous occupancy and image-only mandatory M are charged only once.
pub(super) fn output_bytes(
    sections: &Vec<super::SectionSpan>,
    surfaces: &Vec<super::NativeSurfaceSpan>,
    section_material_bytes: usize,
) -> usize {
    let mut bytes = section_material_bytes
        .saturating_add(
            sections
                .capacity()
                .saturating_mul(std::mem::size_of::<super::SectionSpan>()),
        )
        .saturating_add(
            surfaces
                .capacity()
                .saturating_mul(std::mem::size_of::<super::NativeSurfaceSpan>()),
        );
    for (index, section) in sections.iter().enumerate() {
        if let Some(captures) = &section.inner_material
            && !sections[..index].iter().any(|prior| {
                prior
                    .inner_material
                    .as_ref()
                    .is_some_and(|other| Arc::ptr_eq(captures, other))
            })
        {
            bytes = bytes.saturating_add(inner_capture_bytes(captures));
        }
        if !sections[..index]
            .iter()
            .any(|prior| Arc::ptr_eq(&prior.material, &section.material))
        {
            bytes = bytes.saturating_add(MATERIAL_ARC_BYTES);
        }
        bytes = bytes.saturating_add(
            section.process_domains.capacity() * std::mem::size_of::<super::ProcessDomain>(),
        );
        for (domain_index, domain) in section.process_domains.iter().enumerate() {
            bytes = bytes
                .saturating_add(
                    domain.coverage.contributions.capacity()
                        * std::mem::size_of::<ProcessRegionContribution>(),
                )
                .saturating_add(
                    domain
                        .coverage
                        .contributions
                        .iter()
                        .map(|source| source.definition_id.allocated_bytes())
                        .sum::<usize>(),
                );
            let shared = Arc::ptr_eq(&domain.material, &section.material)
                || section.process_domains[..domain_index]
                    .iter()
                    .any(|other| Arc::ptr_eq(&domain.material, &other.material))
                || sections[..index].iter().any(|prior| {
                    Arc::ptr_eq(&domain.material, &prior.material)
                        || prior
                            .process_domains
                            .iter()
                            .any(|other| Arc::ptr_eq(&domain.material, &other.material))
                });
            if !shared {
                bytes = bytes
                    .saturating_add(domain.bytes)
                    .saturating_add(MATERIAL_ARC_BYTES);
            }
        }
        if let Some(occupied) = &section.occupied
            && !sections[..index].iter().any(|prior| {
                prior
                    .occupied
                    .as_ref()
                    .is_some_and(|prior| Arc::ptr_eq(prior, occupied))
            })
        {
            bytes = bytes.saturating_add(occupied_bytes(occupied));
        }
    }
    for surface in surfaces {
        bytes = bytes
            .saturating_add(
                surface
                    .occurrences
                    .capacity()
                    .saturating_mul(std::mem::size_of::<(OccurrenceId, Arc<MultiPolygon<f64>>)>()),
            )
            .saturating_add(
                surface
                    .image_islands
                    .capacity()
                    .saturating_mul(std::mem::size_of::<(OccurrenceId, usize)>()),
            );
        bytes = bytes.saturating_add(surface.native_bytes);
        // Conservatively cover the retained BTree nodes, not only their values.
        // A sparse std BTree node still owns all 11 key/value slots and,
        // for internal nodes, 12 child slots plus its header. Reserving 16
        // (entry + pointer) units per live entry also covers a one-entry root;
        // every allocated node contains at least one entry.
        bytes = bytes.saturating_add(surface.required.len().saturating_mul(
            16 * (std::mem::size_of::<(OccurrenceId, Arc<MultiPolygon<f64>>)>()
                + std::mem::size_of::<usize>()),
        ));
    }
    bytes
}

struct StageMetric {
    layers: u32,
    slabs: u32,
    coverage: spiling_geometry::NativeUnionCoverage,
    owned_critical_slabs: u32,
}

/// A scheduling hint only: every proposed prefix still needs fresh proof.
#[derive(Default)]
pub(super) struct NativeSpanSizing {
    limits: Vec<(u64, Option<u32>)>,
    active: Option<usize>,
}

impl NativeSpanSizing {
    pub(super) fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.limits.capacity() * std::mem::size_of::<(u64, Option<u32>)>()
    }

    // Bounds learned from actual geometry are only proposal limits. Retaining
    // them across an anchor-driven schedule change avoids repeating known
    // over-wide source queries; every new interval still needs fresh proof.
    fn begin_partition(
        &mut self,
        upper_z_bits: u64,
        live_bytes: usize,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<usize, ManufacturingError> {
        if self
            .active
            .is_some_and(|index| self.limits[index].0 == upper_z_bits)
        {
            return Ok(0);
        }
        preflight::charge_capture_proposal_work(
            work,
            (usize::BITS - self.limits.len().leading_zeros()) as usize + 1,
            cancel,
        )?;
        let old_bytes = self.retained_bytes();
        let index = match self
            .limits
            .binary_search_by_key(&upper_z_bits, |limit| limit.0)
        {
            Ok(index) => index,
            Err(index) => {
                if self.limits.len() >= MAX_PRINT_LAYERS as usize {
                    return Err(error(
                        ManufacturingErrorCode::ResourceLimit,
                        "native partition proposal hints exceed the layer budget",
                    ));
                }
                let growth = if self.limits.len() == self.limits.capacity() {
                    std::mem::size_of::<(u64, Option<u32>)>()
                } else {
                    0
                };
                admit_bytes(live_bytes, growth, 0)?;
                preflight::charge_capture_proposal_work(
                    work,
                    self.limits.len() - index + 1,
                    cancel,
                )?;
                self.limits.try_reserve_exact(1).map_err(|_| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "native partition proposal hint allocation refused",
                    )
                })?;
                self.limits.insert(index, (upper_z_bits, None));
                index
            }
        };
        self.active = Some(index);
        let growth = self.retained_bytes() - old_bytes;
        admit_bytes(live_bytes, growth, 0)?;
        Ok(growth)
    }

    fn limit_end(&self, index: u32, partition_end: u32) -> u32 {
        self.active
            .and_then(|active| self.limits[active].1)
            .map_or(partition_end, |layers| {
                index.saturating_add(layers.max(1)).min(partition_end)
            })
    }

    fn tighten(&mut self, upper_z_bits: u64, layers: u32) {
        if let Some(index) = self.active
            && layers > 0
            && self.limits[index].0 == upper_z_bits
        {
            let limit = &mut self.limits[index].1;
            *limit = Some(limit.map_or(layers, |previous| previous.min(layers)));
        }
    }
}

/// Lookahead retains real candidates, never a placeholder receipt or a second
/// slicer. The geometry ledger alone resolves the accepted stream's authority.
pub(super) struct NativeStageState {
    previous_positive: Vec<SourceMaterial>,
    previous_bytes: usize,
    ready: VecDeque<ReadyStage>,
    ready_points: usize,
    ready_bytes: usize,
    ledger: spiling_geometry::NativeUnionBindingLedger,
    terminal_z: f64,
    metrics: Vec<StageMetric>,
    protected_voids: Vec<CapturedProtectedVoid>,
    void_bytes: usize,
    sizing: NativeSpanSizing,
    defer_voids: bool,
    pending_voids: Vec<CapturedEvidence>,
    pending_void_payload_bytes: usize,
}

impl NativeStageState {
    pub fn new(interval: [f64; 2]) -> Result<Self, ManufacturingError> {
        let ledger = spiling_geometry::NativeUnionBindingLedger::new(
            interval,
            spiling_geometry::MAX_PREPARED_SECTION_SOURCE_BYTES,
        )
        .map_err(|failure| super::native_error(failure, "native stream binding admission"))?;
        Ok(Self {
            previous_positive: Vec::new(),
            previous_bytes: 0,
            ready: VecDeque::new(),
            ready_points: 0,
            ready_bytes: 0,
            ledger,
            terminal_z: interval[1],
            metrics: Vec::new(),
            protected_voids: Vec::new(),
            void_bytes: 0,
            sizing: NativeSpanSizing::default(),
            defer_voids: false,
            pending_voids: Vec::new(),
            pending_void_payload_bytes: 0,
        })
    }

    pub fn retained_point_count(&self) -> usize {
        self.ready_points
    }

    pub(super) fn set_capture_terminal(&mut self, upper: f64) {
        self.terminal_z = upper;
    }

    pub(super) fn defer_void_capture(&mut self, defer: bool) {
        self.defer_voids = defer;
    }

    pub(super) fn take_recyclable_slabs(&mut self) -> RecyclableSlabs {
        self.pending_void_payload_bytes = 0;
        RecyclableSlabs(std::mem::take(&mut self.pending_voids))
    }

    /// Resolve every intact native void obligation before projection or
    /// accepted-prefix handoff. `live_bytes` includes this stage and all caller
    /// and pool owners; conversion scratch must fit beside the raw input.
    pub(super) fn materialize_pending_voids(
        &mut self,
        input: &CompilerInput<'_>,
        mut live_bytes: usize,
        work: &mut spiling_geometry::GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        check_cancel(cancel)?;
        if self.pending_voids.is_empty() {
            return Ok(());
        }
        let required = self
            .protected_voids
            .len()
            .checked_add(self.pending_voids.len())
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "native void obligation count overflow",
                )
            })?;
        let old_capacity = self.protected_voids.capacity();
        let growth = required
            .saturating_sub(old_capacity)
            .saturating_mul(std::mem::size_of::<CapturedProtectedVoid>());
        admit_bytes(live_bytes, growth, 0)?;
        self.protected_voids
            .try_reserve_exact(self.pending_voids.len())
            .map_err(|_| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "pending native void allocation refused",
                )
            })?;
        let actual_growth = (self.protected_voids.capacity() - old_capacity)
            .saturating_mul(std::mem::size_of::<CapturedProtectedVoid>());
        live_bytes = admit_bytes(live_bytes, actual_growth, 0)?;
        self.void_bytes = void_capture_bytes(&self.protected_voids);
        for evidence in self.pending_voids.drain(..) {
            check_cancel(cancel)?;
            let payload = evidence.retained_bytes();
            self.pending_void_payload_bytes = self
                .pending_void_payload_bytes
                .checked_sub(payload)
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "pending native void payload accounting mismatch",
                    )
                })?;
            work.charge(input.occurrences.len() as u64, cancel)
                .map_err(|failure| {
                    super::native_error(failure, "pending native void source binding")
                })?;
            let occurrence = input
                .occurrences
                .iter()
                .find(|occurrence| occurrence.occurrence_id == evidence.occurrence_id)
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::NativeAnalysisIndeterminate,
                        "pending native void lost its source occurrence",
                    )
                })?;
            let available = spiling_geometry::MAX_PREPARED_SECTION_BYTES
                .checked_sub(live_bytes)
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "pending native void has no conversion scratch beside retained raw M",
                    )
                })?;
            let capture = if let Some(collar) = evidence.collar {
                collar.capture_projected_voids(evidence.analysis, available as u32, work, cancel)
            } else {
                spiling_geometry::capture_projected_voids(
                    evidence.analysis,
                    available as u32,
                    work,
                    cancel,
                )
            }
            .map_err(|failure| super::native_error(failure, "pending native void capture"))?;
            let bytes = if let Some(capture) = capture {
                if capture.definition_id() != &occurrence.definition_id
                    || capture.placed_pose() != Some(occurrence.pose)
                {
                    return Err(error(
                        ManufacturingErrorCode::NativeAnalysisIndeterminate,
                        "pending native void changed its source/pose binding",
                    ));
                }
                let bytes = capture.retained_bytes();
                self.protected_voids.push(CapturedProtectedVoid {
                    occurrence_id: occurrence.occurrence_id,
                    capture,
                });
                bytes
            } else {
                0
            };
            live_bytes = live_bytes
                .checked_sub(payload)
                .and_then(|n| n.checked_add(bytes))
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "pending native void live-owner accounting mismatch",
                    )
                })?;
            self.void_bytes = void_capture_bytes(&self.protected_voids);
            admit_bytes(live_bytes, 0, 0)?;
        }
        drop(std::mem::take(&mut self.pending_voids));
        Ok(())
    }

    pub(super) fn replace_span_sizing(&mut self, sizing: NativeSpanSizing) {
        self.sizing = sizing;
    }

    pub(super) fn take_span_sizing(&mut self) -> NativeSpanSizing {
        std::mem::take(&mut self.sizing)
    }

    pub(super) fn source_captures(&self) -> &[spiling_geometry::NativeUnionSourceCapture] {
        self.ledger.source_captures()
    }

    pub(super) fn protected_voids_mut(
        &mut self,
    ) -> Result<&mut Vec<CapturedProtectedVoid>, ManufacturingError> {
        if !self.pending_voids.is_empty() {
            return Err(error(
                ManufacturingErrorCode::NativeAnalysisIndeterminate,
                "native void projection requires all pending raw obligations to materialize",
            ));
        }
        Ok(&mut self.protected_voids)
    }

    pub(super) fn refresh_void_bytes(&mut self) {
        self.void_bytes = void_capture_bytes(&self.protected_voids);
    }

    pub(super) fn retained_proof_bytes(&self) -> usize {
        self.previous_bytes
            .saturating_add(self.void_bytes)
            .saturating_add(self.pending_void_payload_bytes)
            .saturating_add(
                self.pending_voids
                    .capacity()
                    .saturating_mul(std::mem::size_of::<CapturedEvidence>()),
            )
            .saturating_add(self.sizing.retained_bytes())
            .saturating_add(self.ledger.retained_bytes())
            .saturating_add(
                self.metrics
                    .capacity()
                    .saturating_mul(std::mem::size_of::<StageMetric>()),
            )
            .saturating_add(
                self.ready
                    .capacity()
                    .saturating_mul(std::mem::size_of::<ReadyStage>()),
            )
    }

    pub fn finalize(
        self,
        work: &mut spiling_geometry::GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<CompletedCoverage, ManufacturingError> {
        if !self.pending_voids.is_empty() {
            return Err(error(
                ManufacturingErrorCode::NativeAnalysisIndeterminate,
                "completed native coverage still owns unresolved raw void obligations",
            ));
        }
        if !self.ready.is_empty() {
            return Err(error(
                ManufacturingErrorCode::NativeAnalysisIndeterminate,
                "native endpoint lookahead has unconsumed actual candidates",
            ));
        }
        let completed = self.ledger.finalize(work, cancel).map_err(|failure| {
            super::native_error(failure, "complete native stream dependency binding")
        })?;
        if completed.slab_count() != self.metrics.len() {
            return Err(error(
                ManufacturingErrorCode::NativeAnalysisIndeterminate,
                "completed native stream does not match actual accepted stage coverage",
            ));
        }
        // No persisted/countable coverage is produced before all actual leaf
        // and final-M dependencies have been resolved by geometry.
        let mut result = CompletedCoverage {
            slab_count: 0,
            achieved_shape_error_mm: completed.achieved_shape_error_mm(),
            source_features: NativeFeatureCoverageSummary {
                disjoint_source_layer_count: 0,
                invariant_source_union_layer_count: 0,
                native_interface_complex_layer_count: 0,
                owned_critical_slab_count: 0,
            },
            protected_voids: self.protected_voids,
            material_proof: completed,
        };
        for metric in self.metrics {
            check_cancel(cancel)?;
            result.slab_count = add_count(result.slab_count, metric.slabs)?;
            result.source_features.owned_critical_slab_count = add_count(
                result.source_features.owned_critical_slab_count,
                metric.owned_critical_slabs,
            )?;
            let count = match metric.coverage {
                spiling_geometry::NativeUnionCoverage::DisjointSources => {
                    &mut result.source_features.disjoint_source_layer_count
                }
                spiling_geometry::NativeUnionCoverage::InvariantUnion => {
                    &mut result.source_features.invariant_source_union_layer_count
                }
                spiling_geometry::NativeUnionCoverage::NativeInterfaceComplex => {
                    &mut result.source_features.native_interface_complex_layer_count
                }
            };
            *count = add_count(*count, metric.layers)?;
        }
        if result.material_proof.owned_critical_slab_count()
            != result.source_features.owned_critical_slab_count as usize
        {
            return Err(error(
                ManufacturingErrorCode::NativeAnalysisIndeterminate,
                "completed native stream does not match actual owned critical-slab coverage",
            ));
        }
        Ok(result)
    }
}

fn add_count(a: u32, b: u32) -> Result<u32, ManufacturingError> {
    a.checked_add(b).ok_or_else(|| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "native accepted coverage counter overflow",
        )
    })
}

struct CapturedEvidence {
    occurrence_id: OccurrenceId,
    analysis: spiling_geometry::SlabAnalysis,
    collar: Option<spiling_geometry::NativeCriticalCollarIncidence>,
}

impl CapturedEvidence {
    fn retained_bytes(&self) -> usize {
        self.analysis.retained_bytes().saturating_add(
            self.collar
                .as_ref()
                .map_or(0, |collar| collar.retained_bytes()),
        )
    }
}

fn evidence_bytes(evidence: &Vec<CapturedEvidence>) -> usize {
    evidence
        .capacity()
        .saturating_mul(std::mem::size_of::<CapturedEvidence>())
        .saturating_add(
            evidence
                .iter()
                .map(CapturedEvidence::retained_bytes)
                .sum::<usize>(),
        )
}

/// Intact M proofs leaving a failed attempt; no D or ledger authority survives.
#[derive(Default)]
pub(super) struct RecyclableSlabs(Vec<CapturedEvidence>);

/// One compile and one immutable PreparedSources owner. Slots own raw M only;
/// actual source leaves, collars, D and composition are always rebuilt.
#[derive(Default)]
pub(super) struct NativeSlabPool {
    entries: Vec<CapturedEvidence>,
    payload_bytes: usize,
}

impl NativeSlabPool {
    pub(super) fn retained_bytes(&self) -> usize {
        self.payload_bytes.saturating_add(
            self.entries
                .capacity()
                .saturating_mul(std::mem::size_of::<CapturedEvidence>()),
        )
    }

    pub(super) fn clear(&mut self) {
        drop(std::mem::take(&mut self.entries));
        self.payload_bytes = 0;
    }

    fn take(
        &mut self,
        occurrence: &OccurrenceRecord,
        interval: [f64; 2],
        sample: f64,
        policy: f64,
        work: &mut spiling_geometry::GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<Option<CapturedEvidence>, ManufacturingError> {
        check_cancel(cancel)?;
        for index in 0..self.entries.len() {
            work.charge(1, cancel).map_err(|failure| {
                super::native_error(failure, "native placed-slab reuse lookup")
            })?;
            let entry = &self.entries[index];
            let analysis = &entry.analysis;
            if entry.occurrence_id == occurrence.occurrence_id
                && analysis.definition_id() == &occurrence.definition_id
                && analysis.placed_pose().is_some_and(|pose| {
                    pose.translation_mm.map(f64::to_bits)
                        == occurrence.pose.translation_mm.map(f64::to_bits)
                        && pose.rotation_xyzw.map(f64::to_bits)
                            == occurrence.pose.rotation_xyzw.map(f64::to_bits)
                })
                && analysis
                    .world_height_interval_mm()
                    .map(|v| v.map(f64::to_bits))
                    == Some(interval.map(f64::to_bits))
                && analysis.sample_world_z_mm().map(f64::to_bits) == Some(sample.to_bits())
                && analysis.world_shape_error_policy_mm().map(f64::to_bits)
                    == Some(policy.to_bits())
            {
                let entry = self.entries.swap_remove(index);
                self.payload_bytes -= entry.retained_bytes();
                return Ok(Some(entry));
            }
        }
        Ok(None)
    }

    pub(super) fn recycle(
        &mut self,
        slabs: RecyclableSlabs,
        outside_bytes: usize,
        work: &mut spiling_geometry::GeometryWorkBudget,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        let mut entries = slabs.0;
        work.charge(entries.len() as u64, cancel)
            .map_err(|failure| {
                super::native_error(failure, "native placed-slab proof recycling")
            })?;
        for entry in &mut entries {
            // This receipt binds the discarded attempt's actual M neighbours.
            drop(entry.collar.take());
        }
        let live = outside_bytes
            .saturating_add(self.retained_bytes())
            .saturating_add(evidence_bytes(&entries));
        admit_bytes(live, 0, 0)?;
        let payload = entries
            .iter()
            .map(CapturedEvidence::retained_bytes)
            .sum::<usize>();
        let count = entries.len();
        if self.entries.is_empty() {
            drop(std::mem::replace(&mut self.entries, entries));
        } else {
            let required = self.entries.len().checked_add(count).ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "native proof recycle count overflow",
                )
            })?;
            let growth = required
                .saturating_sub(self.entries.capacity())
                .saturating_mul(std::mem::size_of::<CapturedEvidence>());
            admit_bytes(live, growth, 0)?;
            let old_capacity = self.entries.capacity();
            self.entries.try_reserve_exact(count).map_err(|_| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "native proof recycle allocation refused",
                )
            })?;
            admit_bytes(
                live,
                (self.entries.capacity() - old_capacity)
                    .saturating_mul(std::mem::size_of::<CapturedEvidence>()),
                0,
            )?;
            self.entries.append(&mut entries);
        }
        self.payload_bytes = if self.entries.len() == count {
            payload
        } else {
            self.payload_bytes.saturating_add(payload)
        };
        Ok(())
    }
}

struct DraftStage {
    index: u32,
    end: u32,
    partition_end_z_bits: u64,
    interval: [f64; 2],
    native_sample: f64,
    analyses: Vec<CapturedEvidence>,
    leaves: Vec<SourceMaterial>,
    material: Arc<MultiPolygon<f64>>,
    finished: Vec<(OccurrenceId, Arc<MultiPolygon<f64>>)>,
    contributions: Vec<(OccurrenceId, Arc<MultiPolygon<f64>>, usize)>,
    contribution_bytes: usize,
    occupied: super::ContinuousOccupied,
    source_area_mm2: f64,
    coordinate_error_request_mm: f64,
    point_count: usize,
    native_bytes: usize,
    material_bytes: usize,
    finished_bytes: usize,
}

impl DraftStage {
    fn needs_successor(&self, has_future: bool) -> bool {
        has_future
            && self
                .analyses
                .iter()
                .any(|evidence| evidence.analysis.has_critical_collar())
    }
    fn positive_materials(&self) -> impl Iterator<Item = &SourceMaterial> + Clone {
        self.leaves
            .iter()
            .filter(|material| material.leaf.is_positive())
    }

    fn retained_bytes(&self) -> usize {
        self.native_bytes
            .saturating_add(
                if self
                    .contributions
                    .iter()
                    .any(|(_, material, _)| Arc::ptr_eq(material, &self.material))
                {
                    0
                } else {
                    self.material_bytes.saturating_add(MATERIAL_ARC_BYTES)
                },
            )
            .saturating_add(occupied_bytes(&self.occupied))
            .saturating_add(
                self.finished
                    .capacity()
                    .saturating_mul(std::mem::size_of::<(OccurrenceId, Arc<MultiPolygon<f64>>)>()),
            )
            .saturating_add(self.finished_bytes)
            .saturating_add(self.contribution_bytes)
            .saturating_add(
                self.contributions.capacity()
                    * std::mem::size_of::<(OccurrenceId, Arc<MultiPolygon<f64>>, usize)>(),
            )
    }
}

struct ReadyStage {
    index: u32,
    partition_end_z_bits: u64,
    span: NativeSpan,
    pending: spiling_geometry::PendingNativeUnionProof,
    candidate: Vec<spiling_geometry::SectionLoopMm>,
    positive: Vec<SourceMaterial>,
    protected_voids: Vec<CapturedProtectedVoid>,
    pending_voids: Vec<CapturedEvidence>,
}

impl ReadyStage {
    fn retained_bytes(&self) -> usize {
        let inner_bytes = self
            .span
            .inner_material
            .as_ref()
            .map_or(0, |value| inner_capture_bytes(value));
        self.pending
            .retained_bytes()
            .saturating_add(inner_bytes)
            .saturating_add(loop_bytes(&self.candidate))
            .saturating_add(
                self.positive
                    .iter()
                    .map(|leaf| leaf.retained_bytes())
                    .sum::<usize>(),
            )
            .saturating_add(void_capture_bytes(&self.protected_voids))
            .saturating_add(evidence_bytes(&self.pending_voids))
            .saturating_add(
                self.positive
                    .capacity()
                    .saturating_mul(std::mem::size_of::<SourceMaterial>()),
            )
            .saturating_add(
                if self
                    .span
                    .contributions
                    .iter()
                    .any(|(_, material, _)| Arc::ptr_eq(material, &self.span.material))
                {
                    0
                } else {
                    self.span.material_bytes.saturating_add(MATERIAL_ARC_BYTES)
                },
            )
            .saturating_add(occupied_bytes(&self.span.occupied))
            .saturating_add(
                self.span
                    .finished
                    .capacity()
                    .saturating_mul(std::mem::size_of::<(OccurrenceId, Arc<MultiPolygon<f64>>)>()),
            )
            .saturating_add(self.span.finished_bytes)
            .saturating_add(self.span.contribution_bytes)
            .saturating_add(
                self.span.contributions.capacity()
                    * std::mem::size_of::<(OccurrenceId, Arc<MultiPolygon<f64>>, usize)>(),
            )
    }
}

/// Settle a bounded actual successor frontier before recording the current
/// candidate. Real source leaves break the contact cycle; the final ordered
/// ledger still requires their actual use in accepted final-M candidates.
pub(super) fn collect_span(
    source: &NativeSourceInputs<'_, '_>,
    admission: &mut preflight::Admission,
    schedule: &mut Vec<LayerElevation>,
    request: &NativeSpanRequest,
    state: &mut NativeStageState,
    surface_work: &mut usize,
    pool: &mut NativeSlabPool,
    cancel: &AtomicBool,
) -> Result<NativeSpan, ManufacturingError> {
    let NativeSourceInputs {
        input,
        occurrences,
        sources,
    } = *source;
    let NativeSpanRequest {
        index,
        retained_points,
        retained_output_bytes,
    } = *request;
    // Each map value is an actual prepared owner. Warm it BEFORE freezing the
    // resident byte baseline reused through frontier retries, not per occurrence
    // or by assuming equal definition IDs imply shared allocations.
    let mut resident_bytes = sources.values().fold(
        retained_output_bytes.saturating_add(admission.native_capture_proposal_bytes()),
        |bytes, source| bytes.saturating_add(source.retained_bytes()),
    );
    let mut map_allowance = spiling_geometry::MAX_PREPARED_SECTION_SOURCE_BYTES.saturating_sub(
        resident_bytes
            .saturating_add(state.retained_proof_bytes())
            .saturating_add(state.ready_bytes)
            .saturating_add(pool.retained_bytes()),
    );
    for prepared in sources.values() {
        let remaining = prepared
            .prepare_native_position_maps(map_allowance, &mut admission.geometry_work, cancel)
            .map_err(|failure| {
                super::native_error(failure, "native positional query-map preparation")
            })?;
        resident_bytes = resident_bytes.saturating_add(map_allowance - remaining);
        map_allowance = remaining;
    }
    if state.ready.is_empty() {
        let partition_end = span_end(
            schedule,
            admission,
            input.intent,
            index,
            surface_work,
            cancel,
        )?
        .min(schedule.partition_point(|layer| layer.z <= state.terminal_z) as u32);
        let partition_end_z_bits = schedule[(partition_end - 1) as usize].z.to_bits();
        state.sizing.begin_partition(
            partition_end_z_bits,
            resident_bytes
                .saturating_add(state.retained_proof_bytes())
                .saturating_add(state.ready_bytes)
                .saturating_add(pool.retained_bytes()),
            surface_work,
            cancel,
        )?;
        let end = state.sizing.limit_end(index, partition_end);
        let first = capture_base(
            source,
            admission,
            schedule,
            &SpanCaptureRequest {
                index,
                end,
                partition_end_z_bits,
                retained_points,
                retained_bytes: resident_bytes.saturating_add(state.retained_proof_bytes()),
            },
            &mut state.sizing,
            pool,
            surface_work,
            cancel,
        )?;
        let mut frontier = vec![first];
        let mut settled: VecDeque<ReadyStage> = VecDeque::new();
        loop {
            check_cancel(cancel)?;
            let current = frontier
                .last()
                .expect("nonempty native dependency frontier");
            let successor_matches = settled
                .front()
                .is_some_and(|next| next.index == current.end);
            if current.needs_successor(current.interval[1] < state.terminal_z) && !successor_matches
            {
                let next_index = current.end;
                if next_index >= schedule.len() as u32
                    || schedule[next_index as usize].lower_z >= state.terminal_z
                {
                    return Err(error(
                        ManufacturingErrorCode::NativeAnalysisIndeterminate,
                        "native upper endpoint has no actual positive successor stratum",
                    ));
                }
                let points = frontier
                    .iter()
                    .map(|stage| stage.point_count)
                    .sum::<usize>()
                    .checked_add(settled.iter().map(|stage| stage.span.point_count).sum())
                    .and_then(|n| retained_points.checked_add(n))
                    .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
                    .ok_or_else(|| {
                        error(
                            ManufacturingErrorCode::ResourceLimit,
                            "native endpoint frontier retained point budget exceeded",
                        )
                    })?;
                let bytes = frontier
                    .iter()
                    .fold(resident_bytes, |bytes, stage| {
                        bytes.saturating_add(stage.retained_bytes())
                    })
                    .checked_add(settled.iter().fold(0usize, |bytes, stage| {
                        bytes.saturating_add(stage.retained_bytes())
                    }))
                    .and_then(|n| n.checked_add(state.retained_proof_bytes()))
                    .and_then(|n| {
                        n.checked_add(
                            frontier
                                .capacity()
                                .saturating_mul(std::mem::size_of::<DraftStage>()),
                        )
                    })
                    .and_then(|n| {
                        n.checked_add(
                            settled
                                .capacity()
                                .saturating_mul(std::mem::size_of::<ReadyStage>()),
                        )
                    })
                    .filter(|n| {
                        n.checked_add(pool.retained_bytes()).is_some_and(|total| {
                            total <= spiling_geometry::MAX_PREPARED_SECTION_BYTES
                        })
                    })
                    .ok_or_else(|| {
                        error(
                            ManufacturingErrorCode::ResourceLimit,
                            "native endpoint frontier retained proof capacity exceeded",
                        )
                    })?;
                if frontier.len() + settled.len() >= MAX_PRINT_LAYERS as usize {
                    return Err(error(
                        ManufacturingErrorCode::ResourceLimit,
                        "native endpoint dependency frontier exceeds the layer budget",
                    ));
                }
                let partition_end = span_end(
                    schedule,
                    admission,
                    input.intent,
                    next_index,
                    surface_work,
                    cancel,
                )?
                .min(schedule.partition_point(|layer| layer.z <= state.terminal_z) as u32);
                let partition_end_z_bits = schedule[(partition_end - 1) as usize].z.to_bits();
                let growth = state.sizing.begin_partition(
                    partition_end_z_bits,
                    bytes.saturating_add(pool.retained_bytes()),
                    surface_work,
                    cancel,
                )?;
                let bytes = bytes.saturating_add(growth);
                let next_end = state.sizing.limit_end(next_index, partition_end);
                let next = capture_base(
                    source,
                    admission,
                    schedule,
                    &SpanCaptureRequest {
                        index: next_index,
                        end: next_end,
                        partition_end_z_bits,
                        retained_points: points,
                        retained_bytes: bytes,
                    },
                    &mut state.sizing,
                    pool,
                    surface_work,
                    cancel,
                )?;
                frontier.push(next);
                continue;
            }
            let mut current = frontier.pop().expect("nonempty native dependency frontier");
            let candidate = union_loops(
                &current.material,
                &current.finished,
                current.native_sample,
                cancel,
            )?;
            let upper = settled
                .front()
                .filter(|stage| stage.index == current.end)
                .map_or(&[][..], |stage| stage.positive.as_slice());
            let coexisting = resident_bytes
                .saturating_add(pool.retained_bytes())
                .saturating_add(state.retained_proof_bytes())
                .saturating_add(
                    frontier
                        .capacity()
                        .saturating_mul(std::mem::size_of::<DraftStage>()),
                )
                .saturating_add(
                    settled
                        .capacity()
                        .saturating_mul(std::mem::size_of::<ReadyStage>()),
                )
                .saturating_add(frontier.iter().fold(0usize, |bytes, stage| {
                    bytes.saturating_add(stage.retained_bytes())
                }))
                .saturating_add(settled.iter().fold(0usize, |bytes, stage| {
                    bytes.saturating_add(stage.retained_bytes())
                }));
            let proof = if let Some(previous) = frontier.last() {
                certify_stage(
                    source,
                    &mut current,
                    &candidate,
                    AdjacentSourceMaterials {
                        lower: previous.positive_materials(),
                        upper,
                    },
                    coexisting,
                    &mut admission.geometry_work,
                    cancel,
                )
            } else {
                certify_stage(
                    source,
                    &mut current,
                    &candidate,
                    AdjacentSourceMaterials {
                        lower: state.previous_positive.iter(),
                        upper,
                    },
                    coexisting,
                    &mut admission.geometry_work,
                    cancel,
                )
            };
            let pending = match proof {
                Ok(pending) => pending,
                Err(failure)
                    if failure.code == ManufacturingErrorCode::NativeShapeErrorExceeded =>
                {
                    // Every future leaf tied to the old request is discarded.
                    // Rebuild the actual dependency frontier after refinement.
                    settled.clear();
                    let retry_index = current.index;
                    let mut end = current.end;
                    let partition_end_z_bits = current.partition_end_z_bits;
                    state.sizing.begin_partition(
                        partition_end_z_bits,
                        coexisting
                            .saturating_add(current.retained_bytes())
                            .saturating_add(loop_bytes(&candidate)),
                        surface_work,
                        cancel,
                    )?;
                    refine_candidate(
                        schedule,
                        retry_index,
                        &mut end,
                        &mut state.sizing,
                        partition_end_z_bits,
                        "composed native shape-error refinement",
                    )?;
                    drop(candidate);
                    drop(current);
                    let points = retained_points
                        + frontier
                            .iter()
                            .map(|stage| stage.point_count)
                            .sum::<usize>();
                    let bytes = frontier
                        .iter()
                        .fold(resident_bytes, |bytes, stage| {
                            bytes.saturating_add(stage.retained_bytes())
                        })
                        .saturating_add(state.retained_proof_bytes())
                        .saturating_add(
                            frontier
                                .capacity()
                                .saturating_mul(std::mem::size_of::<DraftStage>()),
                        )
                        .saturating_add(
                            settled
                                .capacity()
                                .saturating_mul(std::mem::size_of::<ReadyStage>()),
                        );
                    let next = capture_base(
                        source,
                        admission,
                        schedule,
                        &SpanCaptureRequest {
                            index: retry_index,
                            end,
                            partition_end_z_bits,
                            retained_points: points,
                            retained_bytes: bytes,
                        },
                        &mut state.sizing,
                        pool,
                        surface_work,
                        cancel,
                    )?;
                    frontier.push(next);
                    continue;
                }
                Err(failure) => return Err(failure),
            };
            consume_closed_source_projection(
                source,
                &mut current,
                &pending,
                &candidate,
                coexisting,
                surface_work,
                cancel,
            )?;
            let mut protected_voids = Vec::new();
            let mut allocation_live = coexisting
                .checked_add(current.retained_bytes())
                .and_then(|n| n.checked_add(pending.retained_bytes()))
                .and_then(|n| n.checked_add(loop_bytes(&candidate)))
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "native stage conversion owner overflow",
                    )
                })?;
            let analysis_wrappers = current
                .analyses
                .capacity()
                .checked_mul(std::mem::size_of::<CapturedEvidence>())
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "native stage analysis wrapper overflow",
                    )
                })?;
            let invariant = current
                .analyses
                .iter()
                .all(|evidence| evidence.analysis.invariance().is_some());
            let pending_voids = if state.defer_voids {
                let mut analyses = current.analyses;
                for evidence in &mut analyses {
                    // Preserve exactly the receipt selected by today's void
                    // path, not a collar from a previous schedule attempt.
                    evidence.collar = pending
                        .critical_collars()
                        .iter()
                        .find(|collar| {
                            collar.current_leaf().occurrence_id() == evidence.occurrence_id
                        })
                        .cloned();
                }
                analyses
            } else {
                for (occurrence, evidence) in occurrences.iter().zip(current.analyses) {
                    let raw_bytes = evidence.retained_bytes();
                    if matches!(
                        input.intent.recipe.supports,
                        SupportPolicy::Breakaway { .. }
                    ) {
                        let available = spiling_geometry::MAX_PREPARED_SECTION_SOURCE_BYTES
                            .checked_sub(allocation_live)
                            .ok_or_else(|| {
                                error(
                                    ManufacturingErrorCode::ResourceLimit,
                                    "deferred native void has no retained-byte allowance",
                                )
                            })?;
                        let capture = if let Some(collar) =
                            pending.critical_collars().iter().find(|collar| {
                                collar.current_leaf().occurrence_id() == occurrence.occurrence_id
                            }) {
                            collar.capture_projected_voids(
                                evidence.analysis,
                                available as u32,
                                &mut admission.geometry_work,
                                cancel,
                            )
                        } else {
                            spiling_geometry::capture_projected_voids(
                                evidence.analysis,
                                available as u32,
                                &mut admission.geometry_work,
                                cancel,
                            )
                        }
                        .map_err(|failure| {
                            super::native_error(failure, "deferred native void capture")
                        })?;
                        let capture_bytes = capture
                            .as_ref()
                            .map_or(0, |capture| capture.retained_bytes());
                        allocation_live = allocation_live
                            .checked_sub(raw_bytes)
                            .and_then(|n| n.checked_add(capture_bytes))
                            .ok_or_else(|| {
                                error(
                                    ManufacturingErrorCode::ResourceLimit,
                                    "native stage void owner replacement mismatch",
                                )
                            })?;
                        if let Some(capture) = capture {
                            if capture.definition_id() != &occurrence.definition_id
                                || capture.placed_pose() != Some(occurrence.pose)
                            {
                                return Err(error(
                                    ManufacturingErrorCode::NativeAnalysisIndeterminate,
                                    "deferred native void capture disagrees with original source/pose",
                                ));
                            }
                            if protected_voids.capacity() == 0 {
                                let wrappers = occurrences
                                    .len()
                                    .saturating_mul(std::mem::size_of::<CapturedProtectedVoid>());
                                admit_bytes(allocation_live, wrappers, 0)?;
                                protected_voids
                                    .try_reserve_exact(occurrences.len())
                                    .map_err(|_| {
                                        error(
                                            ManufacturingErrorCode::ResourceLimit,
                                            "deferred native void wrapper allocation refused",
                                        )
                                    })?;
                                allocation_live =
                                    admit_bytes(
                                        allocation_live,
                                        protected_voids.capacity().saturating_mul(
                                            std::mem::size_of::<CapturedProtectedVoid>(),
                                        ),
                                        0,
                                    )?;
                            }
                            protected_voids.push(CapturedProtectedVoid {
                                occurrence_id: occurrence.occurrence_id,
                                capture,
                            });
                        }
                    } else {
                        allocation_live =
                            allocation_live.checked_sub(raw_bytes).ok_or_else(|| {
                                error(
                                    ManufacturingErrorCode::ResourceLimit,
                                    "native stage discarded-analysis accounting mismatch",
                                )
                            })?;
                    }
                }
                allocation_live =
                    allocation_live
                        .checked_sub(analysis_wrappers)
                        .ok_or_else(|| {
                            error(
                                ManufacturingErrorCode::ResourceLimit,
                                "native stage analysis wrapper release mismatch",
                            )
                        })?;
                Vec::new()
            };
            let mut inner_material = Vec::new();
            for material in &current.leaves {
                if !current
                    .contributions
                    .iter()
                    .any(|(id, _, _)| *id == material.leaf.occurrence_id())
                {
                    continue;
                }
                if let Some(capture) = material.leaf.capture_constant_inner_prism() {
                    let capture_bytes = capture.retained_bytes();
                    if inner_material.capacity() == 0 {
                        let wrappers = current
                            .leaves
                            .len()
                            .checked_mul(std::mem::size_of::<
                                spiling_geometry::NativeInnerMaterialPrism,
                            >())
                            .ok_or_else(|| {
                                error(
                                    ManufacturingErrorCode::ResourceLimit,
                                    "native inner-material wrapper overflow",
                                )
                            })?;
                        admit_bytes(allocation_live, capture_bytes, wrappers)?;
                        inner_material
                            .try_reserve_exact(current.leaves.len())
                            .map_err(|_| {
                                error(
                                    ManufacturingErrorCode::ResourceLimit,
                                    "native inner-material capture allocation refused",
                                )
                            })?;
                        allocation_live = admit_bytes(
                            allocation_live,
                            inner_material
                                .capacity()
                                .saturating_mul(std::mem::size_of::<
                                    spiling_geometry::NativeInnerMaterialPrism,
                                >()),
                            0,
                        )?;
                    }
                    allocation_live = admit_bytes(allocation_live, capture_bytes, 0)?;
                    inner_material.push(capture);
                }
            }
            let inner_material = (!inner_material.is_empty()).then(|| Arc::new(inner_material));
            let positive = current
                .leaves
                .into_iter()
                .filter(|material| material.leaf.is_positive())
                .collect();
            let ready = ReadyStage {
                index: current.index,
                partition_end_z_bits: current.partition_end_z_bits,
                pending,
                candidate,
                positive,
                protected_voids,
                pending_voids,
                span: NativeSpan {
                    end: current.end,
                    invariant,
                    inner_material,
                    material: current.material,
                    finished: current.finished,
                    contributions: current.contributions,
                    contribution_bytes: current.contribution_bytes,
                    occupied: Arc::new(current.occupied),
                    source_area_mm2: current.source_area_mm2,
                    point_count: current.point_count,
                    material_bytes: current.material_bytes,
                    finished_bytes: current.finished_bytes,
                },
            };
            let points = retained_points
                + frontier
                    .iter()
                    .map(|stage| stage.point_count)
                    .sum::<usize>()
                + settled
                    .iter()
                    .map(|stage| stage.span.point_count)
                    .sum::<usize>()
                + ready.span.point_count;
            let bytes = frontier
                .iter()
                .fold(resident_bytes, |bytes, stage| {
                    bytes.saturating_add(stage.retained_bytes())
                })
                .saturating_add(settled.iter().fold(0usize, |bytes, stage| {
                    bytes.saturating_add(stage.retained_bytes())
                }))
                .saturating_add(ready.retained_bytes())
                .saturating_add(pool.retained_bytes())
                .saturating_add(state.retained_proof_bytes());
            if points > MAX_DEPOSITION_SEGMENTS as usize
                || bytes > spiling_geometry::MAX_PREPARED_SECTION_SOURCE_BYTES
            {
                return Err(error(
                    ManufacturingErrorCode::ResourceLimit,
                    "native settled endpoint frontier exceeds retained geometry/proof budget",
                ));
            }
            settled.push_front(ready);
            if frontier.is_empty() {
                state.ready_points = settled.iter().map(|stage| stage.span.point_count).sum();
                state.ready_bytes = settled.iter().map(ReadyStage::retained_bytes).sum();
                state.ready = settled;
                break;
            }
        }
    }
    let mut ready = state.ready.pop_front().ok_or_else(|| {
        error(
            ManufacturingErrorCode::NativeAnalysisIndeterminate,
            "native source stage produced no actual candidate",
        )
    })?;
    state.ready_points -= ready.span.point_count;
    state.ready_bytes -= ready.retained_bytes();
    if retained_points
        .checked_add(state.ready_points)
        .and_then(|n| n.checked_add(ready.span.point_count))
        .is_none_or(|n| n > MAX_DEPOSITION_SEGMENTS as usize)
        || state
            .ready_bytes
            .checked_add(ready.retained_bytes())
            .and_then(|n| n.checked_add(state.retained_proof_bytes()))
            .and_then(|n| n.checked_add(resident_bytes))
            .and_then(|n| n.checked_add(pool.retained_bytes()))
            .is_none_or(|n| n > spiling_geometry::MAX_PREPARED_SECTION_SOURCE_BYTES)
    {
        return Err(error(
            ManufacturingErrorCode::ResourceLimit,
            "accepted native frontier plus live scene/ledger exceeds retained budget",
        ));
    }
    let (interval, native_sample) = native_span_reference(schedule, index, ready.span.end)?;
    if ready.index != index
        || ready.pending.world_height_interval_mm() != interval
        || ready.pending.sample_world_z_mm() != native_sample
    {
        return Err(error(
            ManufacturingErrorCode::NativeAnalysisIndeterminate,
            "settled native lookahead no longer matches the actual shared schedule",
        ));
    }
    if !ready.pending_voids.is_empty() {
        let live = resident_bytes
            .saturating_add(pool.retained_bytes())
            .saturating_add(state.ready_bytes)
            .saturating_add(state.retained_proof_bytes())
            .saturating_add(ready.retained_bytes());
        let required = state
            .pending_voids
            .len()
            .checked_add(ready.pending_voids.len())
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "pending native slab count overflow",
                )
            })?;
        let old_capacity = state.pending_voids.capacity();
        let growth = required
            .saturating_sub(old_capacity)
            .saturating_mul(std::mem::size_of::<CapturedEvidence>());
        admit_bytes(live, growth, 0)?;
        state
            .pending_voids
            .try_reserve_exact(ready.pending_voids.len())
            .map_err(|_| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "pending native slab allocation refused",
                )
            })?;
        admit_bytes(
            live,
            (state.pending_voids.capacity() - old_capacity)
                .saturating_mul(std::mem::size_of::<CapturedEvidence>()),
            0,
        )?;
    }

    let metric = StageMetric {
        layers: ready.span.end - index,
        slabs: occurrences.len() as u32,
        coverage: ready.pending.coverage(),
        owned_critical_slabs: ready.pending.owned_critical_slab_count() as u32,
    };
    admit_bytes(
        resident_bytes
            .saturating_add(pool.retained_bytes())
            .saturating_add(state.ready_bytes)
            .saturating_add(state.retained_proof_bytes())
            .saturating_add(ready.retained_bytes()),
        ready.positive.len().saturating_mul(std::mem::size_of::<
            &spiling_geometry::NativeSourceLeafReceipt,
        >()),
        0,
    )?;
    let positive = ready
        .positive
        .iter()
        .map(|material| &material.leaf)
        .collect::<Vec<_>>();
    state
        .ledger
        .record(
            ready.pending,
            &ready.candidate,
            &positive,
            &mut admission.geometry_work,
            cancel,
        )
        .map_err(|failure| {
            super::native_error(failure, "actual native contributor/final-M binding")
        })?;
    state.pending_void_payload_bytes = state.pending_void_payload_bytes.saturating_add(
        ready
            .pending_voids
            .iter()
            .map(CapturedEvidence::retained_bytes)
            .sum::<usize>(),
    );
    state.pending_voids.append(&mut ready.pending_voids);
    if metric.owned_critical_slabs == 0 {
        state
            .sizing
            .tighten(ready.partition_end_z_bits, metric.layers);
    }
    state.previous_bytes = ready
        .positive
        .capacity()
        .saturating_mul(std::mem::size_of::<SourceMaterial>())
        .saturating_add(
            ready
                .positive
                .iter()
                .map(SourceMaterial::retained_bytes)
                .sum::<usize>(),
        );
    state.previous_positive = ready.positive;
    if !ready.protected_voids.is_empty() {
        let payload = ready
            .protected_voids
            .iter()
            .map(|value| value.capture.retained_bytes())
            .sum::<usize>();
        let old_capacity = state.protected_voids.capacity();
        let required = state
            .protected_voids
            .len()
            .checked_add(ready.protected_voids.len())
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "native void capture count overflow",
                )
            })?;
        let capacity = if required > old_capacity {
            required.max(old_capacity.saturating_mul(2))
        } else {
            old_capacity
        };
        let wrappers =
            (capacity - old_capacity).saturating_mul(std::mem::size_of::<CapturedProtectedVoid>());
        let bytes = admit_bytes(
            state.void_bytes,
            payload.saturating_add(wrappers),
            state
                .ready_bytes
                .saturating_add(state.ledger.retained_bytes())
                .saturating_add(state.previous_bytes)
                .saturating_add(resident_bytes)
                .saturating_add(pool.retained_bytes())
                .saturating_add(state.pending_void_payload_bytes)
                .saturating_add(
                    state
                        .pending_voids
                        .capacity()
                        .saturating_mul(std::mem::size_of::<CapturedEvidence>()),
                )
                .saturating_add(
                    ready
                        .protected_voids
                        .capacity()
                        .saturating_mul(std::mem::size_of::<CapturedProtectedVoid>()),
                ),
        )?;
        if capacity > old_capacity {
            state
                .protected_voids
                .try_reserve_exact(capacity - state.protected_voids.len())
                .map_err(|_| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "accepted native void capture allocation refused",
                    )
                })?;
        }
        state.protected_voids.extend(ready.protected_voids);
        state.void_bytes = bytes;
    }
    state.metrics.push(metric);
    Ok(ready.span)
}

fn capture_base(
    source: &NativeSourceInputs<'_, '_>,
    admission: &mut preflight::Admission,
    schedule: &mut Vec<LayerElevation>,
    request: &SpanCaptureRequest,
    sizing: &mut NativeSpanSizing,
    pool: &mut NativeSlabPool,
    surface_work: &mut usize,
    cancel: &AtomicBool,
) -> Result<DraftStage, ManufacturingError> {
    let NativeSourceInputs {
        input,
        occurrences,
        sources,
    } = *source;
    let SpanCaptureRequest {
        index,
        mut end,
        partition_end_z_bits,
        retained_points,
        retained_bytes,
    } = *request;
    loop {
        check_cancel(cancel)?;
        let (interval, sample) = native_span_reference(schedule, index, end)?;
        let layer = &schedule[index as usize];
        let mut refine = false;
        for fields in admission.surface_fields.values() {
            for field in fields {
                if let Some(allowed) = field.allowed_vertical_error() {
                    refine |=
                        field.vertical_error_bound(layer.lower_z, layer.z, layer.sample)? > allowed;
                }
            }
        }
        let initial_bytes = occurrences
            .len()
            .checked_mul(
                std::mem::size_of::<CapturedEvidence>() + std::mem::size_of::<SourceMaterial>(),
            )
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "native source stage identity capacity overflow",
                )
            })?;
        let mut native_bytes = admit_bytes(
            0,
            initial_bytes,
            retained_bytes.saturating_add(pool.retained_bytes()),
        )?;
        let mut analyses = Vec::with_capacity(occurrences.len());
        if !refine {
            for occurrence in occurrences {
                check_cancel(cancel)?;
                let cached = pool.take(
                    occurrence,
                    interval,
                    sample,
                    input.intent.recipe.native_shape_error_mm,
                    &mut admission.geometry_work,
                    cancel,
                )?;
                let evidence = if let Some(evidence) = cached {
                    evidence
                } else {
                    let query_byte_limit = spiling_geometry::MAX_PREPARED_SECTION_BYTES
                        .checked_sub(retained_bytes)
                        .and_then(|n| n.checked_sub(native_bytes))
                        .and_then(|n| n.checked_sub(pool.retained_bytes()))
                        .filter(|n| *n > 0)
                        .ok_or_else(|| {
                            error(
                                ManufacturingErrorCode::ResourceLimit,
                                "native slab has no remaining shared source/query byte allowance",
                            )
                        })?;
                    let analysis = sources[&occurrence.definition_id].analyze_placed_slab(
                        &spiling_geometry::PlacedSlabRequest {
                            pose: occurrence.pose,
                            world_height_interval_mm: interval,
                            sample_world_z_mm: sample,
                            native_shape_error_mm: input.intent.recipe.native_shape_error_mm,
                        },
                        query_byte_limit,
                        &mut admission.geometry_work,
                        cancel,
                    );
                    let analysis = match analysis {
                        Ok(analysis) => analysis,
                        Err(failure) if failure.code == GeometryErrorCode::ShapeErrorExceeded => {
                            refine = true;
                            break;
                        }
                        Err(failure) => {
                            return Err(super::native_error(
                                failure,
                                &format!(
                                    "native slab at layer {index}, world interval [{:.17e}, {:.17e}] mm, sample {:.17e} mm, occurrence {}",
                                    interval[0],
                                    interval[1],
                                    sample,
                                    occurrence.occurrence_id.get()
                                ),
                            ));
                        }
                    };
                    CapturedEvidence {
                        occurrence_id: occurrence.occurrence_id,
                        analysis,
                        collar: None,
                    }
                };
                let analysis = &evidence.analysis;
                if analysis.definition_id() != &occurrence.definition_id
                    || analysis.placed_pose() != Some(occurrence.pose)
                    || analysis.world_height_interval_mm() != Some(interval)
                    || analysis.sample_world_z_mm() != Some(sample)
                    || analysis.world_shape_error_policy_mm()
                        != Some(input.intent.recipe.native_shape_error_mm)
                {
                    return Err(error(
                        ManufacturingErrorCode::NativeAnalysisIndeterminate,
                        "native slab evidence does not bind the captured source/pose/height/sample/policy",
                    ));
                }
                native_bytes = native_bytes
                    .checked_add(analysis.retained_bytes())
                    .filter(|n| {
                        retained_bytes
                            .checked_add(pool.retained_bytes())
                            .and_then(|bytes| bytes.checked_add(*n))
                            .is_some_and(|total| {
                                total <= spiling_geometry::MAX_PREPARED_SECTION_BYTES
                            })
                    })
                    .ok_or_else(|| {
                        error(
                            ManufacturingErrorCode::ResourceLimit,
                            "coexisting native dependency result capacity exceeded",
                        )
                    })?;
                if !analysis.endpoint_strata().is_empty() && !analysis.has_critical_collar() {
                    return Err(error(
                        ManufacturingErrorCode::NativeAnalysisIndeterminate,
                        "native endpoint strata lack a genuine closed-source collar complex",
                    ));
                }
                analyses.push(evidence);
            }
        }
        if refine {
            refine_candidate(
                schedule,
                index,
                &mut end,
                sizing,
                partition_end_z_bits,
                "native/field shape-error refinement",
            )?;
            continue;
        }
        if end > index + 1
            && analyses.iter().any(|evidence| {
                evidence.analysis.invariance().is_none()
                    && !evidence.analysis.has_closed_radial_material_product()
            })
        {
            end = index + (end - index) / 2;
            sizing.tighten(partition_end_z_bits, end - index);
            continue;
        }
        // Pool checkouts changed idle ownership. All later realization and
        // source-leaf scratch uses its current payload plus unused capacity.
        let retained_bytes = retained_bytes.saturating_add(pool.retained_bytes());
        let mut material = Arc::new(MultiPolygon(Vec::new()));
        let mut composed_bytes = 0usize;
        let mut finished: Vec<(OccurrenceId, Arc<MultiPolygon<f64>>)> = Vec::new();
        let mut finished_bytes = 0usize;
        let mut contributions: Vec<(OccurrenceId, Arc<MultiPolygon<f64>>, usize)> = Vec::new();
        let mut contribution_bytes = 0usize;
        let mut occupied = super::ContinuousOccupied::default();
        let mut coordinate_error_request_mm = 0.0;
        let mut native_points = 0usize;
        let mut source_area_mm2 = 0.0;
        let mut leaves = Vec::with_capacity(occurrences.len());
        for (occurrence, evidence) in occurrences.iter().zip(&analyses) {
            let analysis = &evidence.analysis;
            let mut placed =
                region::from_native(analysis.section(), &RigidPoseMm::IDENTITY, sample, cancel)?;
            let outer = match analysis.occupied() {
                Some(outer) => region::from_native(outer, &RigidPoseMm::IDENTITY, sample, cancel)?,
                None if analysis.invariance().is_some()
                    && analysis.closed_shape_error_mm() == Some(0.0) =>
                {
                    placed.clone()
                }
                None => {
                    return Err(error(
                        ManufacturingErrorCode::NativeAnalysisIndeterminate,
                        "native slab has no certified continuous occupied envelope",
                    ));
                }
            };
            let bounds = admission.native_bounds[&occurrence.occurrence_id];
            let height = [
                interval[0].max(bounds.min[2]),
                interval[1].min(bounds.max[2]),
            ];
            if height[0] <= height[1] {
                let points = occupied
                    .point_count
                    .checked_add(region::point_count(&outer))
                    .ok_or_else(|| {
                        error(
                            ManufacturingErrorCode::ResourceLimit,
                            "native continuous occupied point count overflow",
                        )
                    })?;
                admit_points(
                    retained_points,
                    native_points,
                    points,
                    region::point_count(&placed),
                )?;
                for polygon in outer.0 {
                    occupied.push(polygon, height, Some(occurrence.occurrence_id))?;
                }
            }
            let actual_source_loops = union_loops(&placed, &[], sample, cancel)?;
            let certify_leaf = if analysis.has_critical_collar() {
                spiling_geometry::certify_native_one_sided_source_leaf
            } else {
                spiling_geometry::certify_native_source_leaf
            };
            let leaf = match certify_leaf(
                &spiling_geometry::SourceLeafRequest {
                    source: &sources[&occurrence.definition_id],
                    occurrence_id: &occurrence.occurrence_id,
                    pose: occurrence.pose,
                    analysis,
                    actual_used_source_loops: &actual_source_loops,
                    policy: spiling_geometry::NativeUnionPolicy {
                        candidate_coordinate_error_mm: 0.0,
                    },
                },
                &mut admission.geometry_work,
                cancel,
            ) {
                Ok(leaf) => leaf,
                Err(failure) if failure.code == GeometryErrorCode::ShapeErrorExceeded => {
                    refine = true;
                    break;
                }
                Err(failure) => {
                    return Err(super::native_error(
                        failure,
                        &format!(
                            "actual source-M leaf at layer {index}, interval [{:.17e}, {:.17e}], sample {:.17e}, occurrence {}, pose {:?}",
                            interval[0],
                            interval[1],
                            sample,
                            occurrence.occurrence_id.get(),
                            occurrence.pose
                        ),
                    ));
                }
            };
            let source = SourceMaterial {
                leaf,
                actual: actual_source_loops,
            };
            native_bytes = native_bytes
                .checked_add(source.retained_bytes())
                .filter(|n| {
                    retained_bytes
                        .checked_add(*n)
                        .is_some_and(|total| total <= spiling_geometry::MAX_PREPARED_SECTION_BYTES)
                })
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "native source leaf retention capacity exceeded",
                    )
                })?;
            leaves.push(source);
            if placed.0.is_empty() {
                continue;
            }
            if input
                .process_document
                .layers
                .iter()
                .any(|layer| layer.enabled)
            {
                let fraction = input
                    .process_document
                    .resolve(
                        &occurrence.definition_id,
                        occurrence.occurrence_id,
                        input.intent.recipe.infill_fraction,
                    )
                    .fraction;
                for (prior_id, prior) in contributions
                    .iter()
                    .map(|(id, material, _)| (*id, material))
                    .chain(finished.iter().map(|(id, material)| (*id, material)))
                {
                    let prior_occurrence = input
                        .occurrences
                        .iter()
                        .find(|o| o.occurrence_id == prior_id)
                        .expect("captured occurrence");
                    let prior_fraction = input
                        .process_document
                        .resolve(
                            &prior_occurrence.definition_id,
                            prior_id,
                            input.intent.recipe.infill_fraction,
                        )
                        .fraction;
                    if fraction == prior_fraction {
                        continue;
                    }
                    let (Some(a), Some(b)) = (placed.bounding_rect(), prior.bounding_rect()) else {
                        continue;
                    };
                    if !a.intersects(&b) {
                        continue;
                    }
                    let overlap = super::classification::intersection_accumulated(
                        &placed,
                        prior,
                        surface_work,
                        cancel,
                    )?;
                    admit_points(
                        retained_points,
                        native_points,
                        occupied.point_count,
                        region::point_count(&overlap),
                    )?;
                    if overlap.unsigned_area() > 0.0 {
                        // Additive finishes preserve M, so positive original-M
                        // overlap already proves incompatible final-D overlap.
                        return Err(error(
                            ManufacturingErrorCode::InvalidSpecification,
                            format!(
                                "incompatible whole-volume sparse fractions overlap for occurrences {} and {} in [{},{}] mm",
                                prior_id.get(),
                                occurrence.occurrence_id.get(),
                                interval[0],
                                interval[1],
                            ),
                        ));
                    }
                }
            }
            if admission
                .surface_fields
                .contains_key(&occurrence.occurrence_id)
            {
                native_points = native_points
                    .checked_add(region::point_count(&placed))
                    .ok_or_else(|| {
                        error(
                            ManufacturingErrorCode::ResourceLimit,
                            "native footprint point count overflow",
                        )
                    })?;
                source_area_mm2 += placed.unsigned_area();
                finished_bytes = finished_bytes
                    .saturating_add(material_bytes(&mut placed))
                    .saturating_add(MATERIAL_ARC_BYTES);
                finished.push((occurrence.occurrence_id, Arc::new(placed)));
            } else {
                // Default-process construction needs the same original source
                // owners as modifiers; shared material is admitted only once.
                let bytes = material_bytes(&mut placed);
                contribution_bytes = contribution_bytes
                    .saturating_add(bytes)
                    .saturating_add(MATERIAL_ARC_BYTES);
                admit_bytes(
                    retained_bytes.saturating_add(native_bytes),
                    contribution_bytes,
                    composed_bytes,
                )?;
                let placed = Arc::new(placed);
                native_points = native_points
                    .checked_add(region::point_count(&placed))
                    .ok_or_else(|| {
                        error(
                            ManufacturingErrorCode::ResourceLimit,
                            "process contribution point count overflow",
                        )
                    })?;
                let live_bytes = retained_bytes
                    .saturating_add(native_bytes)
                    .saturating_add(contribution_bytes)
                    .saturating_add(
                        if contributions
                            .iter()
                            .any(|(_, source, _)| Arc::ptr_eq(source, &material))
                        {
                            0
                        } else {
                            composed_bytes.saturating_add(MATERIAL_ARC_BYTES)
                        },
                    );
                merge_shared(
                    &mut material,
                    &placed,
                    &mut coordinate_error_request_mm,
                    live_bytes,
                    composed_bytes,
                    bytes,
                    surface_work,
                    cancel,
                )?;
                composed_bytes = if let Some(output) = Arc::get_mut(&mut material) {
                    material_bytes(output)
                } else {
                    bytes
                };
                contributions.push((occurrence.occurrence_id, placed, bytes));
            }
            admit_points(
                retained_points,
                native_points,
                occupied.point_count,
                if contributions
                    .iter()
                    .any(|(_, source, _)| Arc::ptr_eq(source, &material))
                {
                    0
                } else {
                    region::point_count(&material)
                },
            )?;
        }
        if refine {
            refine_candidate(
                schedule,
                index,
                &mut end,
                sizing,
                partition_end_z_bits,
                "actual source-M leaf shape-error refinement",
            )?;
            continue;
        }
        if let Some(material) = Arc::get_mut(&mut material) {
            region::canonicalize(material);
        }
        source_area_mm2 += material.unsigned_area();
        let point_count = native_points
            .checked_add(
                if contributions
                    .iter()
                    .any(|(_, source, _)| Arc::ptr_eq(source, &material))
                {
                    0
                } else {
                    region::point_count(&material)
                },
            )
            .and_then(|n| n.checked_add(occupied.point_count))
            .filter(|n| {
                retained_points
                    .checked_add(*n)
                    .is_some_and(|total| total <= MAX_DEPOSITION_SEGMENTS as usize)
            })
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "retained native material/occupancy frontier point budget exceeded",
                )
            })?;
        let material_bytes = if let Some(material) = Arc::get_mut(&mut material) {
            material_bytes(material)
        } else {
            contributions
                .iter()
                .find(|(_, contribution, _)| Arc::ptr_eq(contribution, &material))
                .map(|(_, _, bytes)| *bytes)
                .expect("shared native material is an original contribution")
        };
        return Ok(DraftStage {
            index,
            end,
            partition_end_z_bits,
            interval,
            native_sample: sample,
            analyses,
            leaves,
            material,
            finished,
            contributions,
            contribution_bytes,
            occupied,
            source_area_mm2,
            coordinate_error_request_mm,
            point_count,
            native_bytes,
            material_bytes,
            finished_bytes,
        });
    }
}

fn native_span_reference(
    schedule: &[LayerElevation],
    index: u32,
    end: u32,
) -> Result<([f64; 2], f64), ManufacturingError> {
    if index >= end || end as usize > schedule.len() {
        return Err(error(
            ManufacturingErrorCode::NativeAnalysisIndeterminate,
            "settled native lookahead no longer matches the actual shared schedule",
        ));
    }
    let first = &schedule[index as usize];
    let interval = [first.lower_z, schedule[(end - 1) as usize].z];
    let sample = if end == index + 1 {
        first.sample
    } else {
        interval[0] + (interval[1] - interval[0]) * 0.5
    };
    Ok((interval, sample))
}

fn refine_candidate(
    schedule: &mut Vec<LayerElevation>,
    index: u32,
    end: &mut u32,
    sizing: &mut NativeSpanSizing,
    partition_end_z_bits: u64,
    context: &str,
) -> Result<(), ManufacturingError> {
    if *end > index + 1 {
        *end = index + (*end - index) / 2;
        sizing.tighten(partition_end_z_bits, *end - index);
        return Ok(());
    }
    sizing.tighten(partition_end_z_bits, 1);
    split_layer(schedule, index, context)?;
    *end = index + 1;
    Ok(())
}

fn admit_points(
    retained: usize,
    native: usize,
    occupied: usize,
    additional: usize,
) -> Result<(), ManufacturingError> {
    retained
        .checked_add(native)
        .and_then(|n| n.checked_add(occupied))
        .and_then(|n| n.checked_add(additional))
        .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "native material/occupied staging point budget exceeded",
            )
        })?;
    Ok(())
}

struct MergeNode {
    polygon: usize,
    parent: usize,
    size: usize,
    next: usize,
    tail: usize,
    bounds: geo::Rect<f64>,
    edges: usize,
    predicate_conditioned: bool,
}

// Pinned geo0.32 uses robust1.2 orient2d for filled-polygon intersection.
// That implementation explicitly does not guarantee underflow cases. Within
// this fixed dyadic arithmetic envelope, every input is a multiple of 2^-180,
// every exact orientation term a multiple of 2^-360; splitter intermediates,
// nonzero products/error bounds stay normal/finite. This is predicate conditioning,
// not a geometric tolerance. Outside it the pair stays connected, not disjoint.
pub(super) fn merge_predicate_coordinate(value: f64) -> bool {
    const LOWER: f64 = f64::from_bits((1023 - 128) << 52);
    const UPPER: f64 = f64::from_bits((1023 + 128) << 52);
    value == 0.0 || (LOWER..=UPPER).contains(&value.abs())
}

// A borrowed component of the two ordinary Boolean operands, not another
// geometry representation. In particular, no native coordinates are cloned.
struct MergeOperand<'a> {
    polygons: &'a [geo::Polygon<f64>],
    incoming: &'a [geo::Polygon<f64>],
    nodes: &'a [MergeNode],
    first: usize,
    split: usize,
    subject: bool,
}
impl BooleanOps for MergeOperand<'_> {
    type Scalar = f64;
    fn rings(&self) -> impl Iterator<Item = &geo::LineString<f64>> {
        std::iter::successors(Some(self.first), |&index| {
            let next = self.nodes[index].next;
            (next != usize::MAX).then_some(next)
        })
        .filter(move |&index| (self.nodes[index].polygon < self.split) == self.subject)
        .flat_map(move |index| {
            let index = self.nodes[index].polygon;
            let polygon = if index < self.split {
                &self.polygons[index]
            } else {
                &self.incoming[index - self.split]
            };
            std::iter::once(polygon.exterior()).chain(polygon.interiors())
        })
    }
}

fn merge_work(
    work: &mut usize,
    amount: usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    *work = work
        .checked_add(amount)
        .filter(|n| *n <= super::MAX_BOOLEAN_PAIRS)
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "native M component work budget exceeded",
            )
        })?;
    Ok(())
}

fn merge_root(
    nodes: &mut [MergeNode],
    mut index: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<usize, ManufacturingError> {
    while nodes[index].parent != index {
        merge_work(work, 1, cancel)?;
        let parent = nodes[index].parent;
        nodes[index].parent = nodes[parent].parent;
        index = nodes[index].parent;
    }
    Ok(index)
}

fn merge_shared(
    target: &mut Arc<MultiPolygon<f64>>,
    incoming: &Arc<MultiPolygon<f64>>,
    coordinate_error_request: &mut f64,
    live_bytes: usize,
    target_bytes: usize,
    incoming_bytes: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    if incoming.0.is_empty() {
        return Ok(());
    }
    if target.0.is_empty() {
        *target = Arc::clone(incoming);
        return Ok(());
    }
    // Only the composed output is mutable. Original admitted occurrence
    // operands remain shared and borrowed by the ordinary component union.
    let cloned_target_bytes = if Arc::strong_count(target) > 1 {
        target_bytes.saturating_add(MATERIAL_ARC_BYTES)
    } else {
        0
    };
    // The borrowed operand can become a required output singleton or exact
    // disjoint append. Reserve that coordinate/header clone before into_owned
    // or any Vec growth, along with a shared target's new output allocation.
    admit_bytes(live_bytes, cloned_target_bytes, incoming_bytes)?;
    let live_bytes = live_bytes
        .saturating_add(cloned_target_bytes)
        .saturating_add(incoming_bytes);
    let output = Arc::make_mut(target);
    merge_operand(
        output,
        std::borrow::Cow::Borrowed(incoming.as_ref()),
        coordinate_error_request,
        live_bytes,
        work,
        cancel,
    )
}

fn merge_operand(
    target: &mut MultiPolygon<f64>,
    mut incoming: std::borrow::Cow<'_, MultiPolygon<f64>>,
    coordinate_error_request: &mut f64,
    live_bytes: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    if incoming.0.is_empty() {
        return Ok(());
    }
    if target.0.is_empty() {
        *target = incoming.into_owned();
        return Ok(());
    }
    let a = target.bounding_rect().expect("nonempty admitted region");
    let b = incoming.bounding_rect().expect("nonempty admitted region");
    if !a.intersects(&b) {
        // No Boolean conversion: preserve all actual native source vertices.
        if matches!(&incoming, std::borrow::Cow::Borrowed(_)) {
            let count = target
                .0
                .len()
                .checked_add(incoming.0.len())
                .filter(|count| *count <= MAX_DEPOSITION_SEGMENTS as usize)
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "shared native M output record limit exceeded",
                    )
                })?;
            let headers = count
                .checked_mul(std::mem::size_of::<geo::Polygon<f64>>())
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "shared native M output record bytes overflow",
                    )
                })?;
            admit_bytes(live_bytes, headers, 0)?;
            target.0.try_reserve_exact(incoming.0.len()).map_err(|_| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "shared native M output record allocation refused",
                )
            })?;
        }
        target.0.extend(incoming.into_owned().0);
        return region::check_size(target);
    }
    // This admission covers the ordinary overlays of disjoint components as
    // well: each original edge participates in at most one such overlay.
    boolean::budget_accumulated(target, &incoming, work, cancel)?;
    let split = target.0.len();
    let count = split
        .checked_add(incoming.0.len())
        .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "native M component count exceeded",
            )
        })?;
    // Account our new index records, possible flattened-record allocation and
    // output records before growth. Existing polygon coordinate buffers move.
    let scratch_bytes = count
        .checked_mul(
            std::mem::size_of::<MergeNode>() + 2 * std::mem::size_of::<geo::Polygon<f64>>(),
        )
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "native M component byte count overflow",
            )
        })?;
    admit_bytes(live_bytes, scratch_bytes, 0)?;
    let mut nodes = Vec::new();
    nodes.try_reserve_exact(count).map_err(|_| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "native M component allocation refused",
        )
    })?;
    for (index, polygon) in target.0.iter().chain(&incoming.0).enumerate() {
        let points = std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .map(|ring| ring.0.len())
            .sum::<usize>();
        merge_work(
            work,
            points.checked_mul(2).ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "native M predicate scan work overflow",
                )
            })?,
            cancel,
        )?;
        let predicate_conditioned = std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .flat_map(|ring| &ring.0)
            .all(|point| {
                merge_predicate_coordinate(point.x) && merge_predicate_coordinate(point.y)
            });
        let bounds = polygon.bounding_rect().ok_or_else(|| {
            error(
                ManufacturingErrorCode::NativeAnalysisIndeterminate,
                "native M contains an empty polygon",
            )
        })?;
        nodes.push(MergeNode {
            polygon: index,
            parent: index,
            size: 1,
            next: usize::MAX,
            tail: index,
            bounds,
            edges: points,
            predicate_conditioned,
        });
    }
    let use_y = a.max().y.max(b.max().y) - a.min().y.min(b.min().y)
        > a.max().x.max(b.max().x) - a.min().x.min(b.min().x);
    boolean::sort_by(
        &mut nodes,
        |node| {
            if use_y {
                node.bounds.min().y
            } else {
                node.bounds.min().x
            }
        },
        work,
        cancel,
    )?;
    for (index, node) in nodes.iter_mut().enumerate() {
        merge_work(work, 1, cancel)?;
        node.parent = index;
        node.tail = index;
    }
    for i in 0..count {
        for j in i + 1..count {
            merge_work(work, 1, cancel)?;
            let lower = if use_y {
                nodes[j].bounds.min().y
            } else {
                nodes[j].bounds.min().x
            };
            let upper = if use_y {
                nodes[i].bounds.max().y
            } else {
                nodes[i].bounds.max().x
            };
            if lower > upper {
                break;
            }
            if !nodes[i].bounds.intersects(&nodes[j].bounds) {
                continue;
            }
            // Pinned geo's filled-polygon predicate includes holes, boundary
            // touch and containment. Precharge its full possible edge product,
            // bounding scans and point-in-polygon scans, not only true hits.
            let cost = nodes[i]
                .edges
                .checked_add(4)
                .and_then(|a| nodes[j].edges.checked_add(4).and_then(|b| a.checked_mul(b)))
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "native M predicate work overflow",
                    )
                })?;
            merge_work(work, cost, cancel)?;
            if nodes[i].predicate_conditioned
                && nodes[j].predicate_conditioned
                && !merge_polygon(target, &incoming, split, nodes[i].polygon)
                    .intersects(merge_polygon(target, &incoming, split, nodes[j].polygon))
            {
                continue;
            }
            let mut a = merge_root(&mut nodes, i, work, cancel)?;
            let mut b = merge_root(&mut nodes, j, work, cancel)?;
            if a == b {
                continue;
            }
            if nodes[a].size < nodes[b].size {
                std::mem::swap(&mut a, &mut b);
            }
            let tail = nodes[a].tail;
            nodes[tail].next = b;
            nodes[a].tail = nodes[b].tail;
            nodes[a].size += nodes[b].size;
            nodes[b].parent = a;
        }
    }
    let mut merged = MultiPolygon(Vec::new());
    merged.0.try_reserve_exact(count).map_err(|_| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "native M output record allocation refused",
        )
    })?;
    let mut conversion_request: f64 = 0.0;
    for first in 0..count {
        merge_work(work, 1, cancel)?;
        if nodes[first].parent != first {
            continue;
        }
        if nodes[first].size == 1 {
            let index = nodes[first].polygon;
            let polygon = if index < split {
                std::mem::replace(
                    &mut target.0[index],
                    geo::Polygon::new(geo::LineString(Vec::new()), Vec::new()),
                )
            } else if let std::borrow::Cow::Owned(incoming) = &mut incoming {
                std::mem::replace(
                    &mut incoming.0[index - split],
                    geo::Polygon::new(geo::LineString(Vec::new()), Vec::new()),
                )
            } else {
                incoming.0[index - split].clone()
            };
            merged.0.push(polygon);
            continue;
        }
        let mut min = [f64::INFINITY; 2];
        let mut max = [f64::NEG_INFINITY; 2];
        let mut index = first;
        loop {
            merge_work(work, 1, cancel)?;
            let bounds = nodes[index].bounds;
            min[0] = min[0].min(bounds.min().x);
            min[1] = min[1].min(bounds.min().y);
            max[0] = max[0].max(bounds.max().x);
            max[1] = max[1].max(bounds.max().y);
            index = nodes[index].next;
            if index == usize::MAX {
                break;
            }
        }
        let subject = MergeOperand {
            polygons: &target.0,
            incoming: &incoming.0,
            nodes: &nodes,
            first,
            split,
            subject: true,
        };
        let clip = MergeOperand {
            subject: false,
            ..subject
        };
        let output = subject.union(&clip);
        region::check_size(&output)?;
        let required = merged.0.len().checked_add(output.0.len()).ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "native M output record count overflow",
            )
        })?;
        let additional_bytes = required
            .saturating_sub(count)
            .checked_mul(std::mem::size_of::<geo::Polygon<f64>>())
            .and_then(|n| scratch_bytes.checked_add(n))
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "native M output record bytes overflow",
                )
            })?;
        admit_bytes(live_bytes, additional_bytes, 0)?;
        merged.0.try_reserve_exact(output.0.len()).map_err(|_| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "native M output growth refused",
            )
        })?;
        merged.0.extend(output.0);
        conversion_request =
            conversion_request.max((boolean::overlay_padding(min, max)? * 2.0f64.sqrt()).next_up());
    }
    if conversion_request > 0.0 {
        // A proposal only. Geometry independently binds every original native
        // source to this exact final M and certifies its feature/incidence graph.
        *coordinate_error_request = (*coordinate_error_request + conversion_request).next_up();
        if !coordinate_error_request.is_finite() {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "native union coordinate error overflow",
            ));
        }
    }
    *target = merged;
    region::check_size(target)
}

fn merge_polygon<'a>(
    target: &'a MultiPolygon<f64>,
    incoming: &'a MultiPolygon<f64>,
    split: usize,
    index: usize,
) -> &'a geo::Polygon<f64> {
    if index < split {
        &target.0[index]
    } else {
        &incoming.0[index - split]
    }
}

fn union_loops(
    material: &MultiPolygon<f64>,
    finished: &[(
        spiling_contracts::geometry::OccurrenceId,
        Arc<MultiPolygon<f64>>,
    )],
    sample: f64,
    cancel: &AtomicBool,
) -> Result<Vec<spiling_geometry::SectionLoopMm>, ManufacturingError> {
    encode_polygons(
        std::iter::once(material)
            .chain(finished.iter().map(|(_, region)| region.as_ref()))
            .flat_map(|region| region.0.iter()),
        sample,
        cancel,
    )
}

pub(super) fn encode_polygons<'a>(
    polygons: impl Iterator<Item = &'a geo::Polygon<f64>> + Clone,
    sample: f64,
    cancel: &AtomicBool,
) -> Result<Vec<spiling_geometry::SectionLoopMm>, ManufacturingError> {
    let mut ring_count = 0usize;
    let mut point_count = 0usize;
    for polygon in polygons.clone() {
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            check_cancel(cancel)?;
            ring_count = ring_count.checked_add(1).ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "native polygon ring count overflow",
                )
            })?;
            point_count = point_count
                .checked_add(ring.0.len())
                .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "native polygon point budget exceeded",
                    )
                })?;
        }
    }
    ring_count
        .checked_mul(std::mem::size_of::<spiling_geometry::SectionLoopMm>())
        .and_then(|n| {
            point_count
                .checked_mul(std::mem::size_of::<[f64; 3]>())
                .and_then(|points| n.checked_add(points))
        })
        .filter(|n| *n <= spiling_geometry::MAX_PREPARED_SECTION_SOURCE_BYTES)
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "native polygon encoding byte budget exceeded",
            )
        })?;
    let mut loops = Vec::with_capacity(ring_count);
    for polygon in polygons {
        for (is_hole, ring) in std::iter::once((false, polygon.exterior()))
            .chain(polygon.interiors().iter().map(|ring| (true, ring)))
        {
            let mut vertices = Vec::with_capacity(ring.0.len());
            for point in &ring.0 {
                check_cancel(cancel)?;
                vertices.push([point.x, point.y, sample]);
            }
            loops.push(spiling_geometry::SectionLoopMm {
                is_hole,
                points_mm: vertices,
            });
        }
    }
    Ok(loops)
}

fn span_end(
    schedule: &[LayerElevation],
    admission: &preflight::Admission,
    intent: &ManufacturingIntent,
    index: u32,
    proposal_work: &mut usize,
    cancel: &AtomicBool,
) -> Result<u32, ManufacturingError> {
    // Event partitions propose spans, never authorize reuse. capture_base
    // requires exact invariance or a complete closed radial material product;
    // varying realized finishes retain their separate per-layer invariance gate.
    let mut end = schedule.len() as u32;
    let mut boundary = |candidate: usize| {
        let candidate = candidate as u32;
        if candidate > index {
            end = end.min(candidate);
        }
    };
    // Charge an upper bound on each binary search as well as the band scan.
    // This is the same cumulative planner work budget used downstream.
    let search_work = (usize::BITS - schedule.len().leading_zeros()) as usize + 1;
    for event in &admission.horizontal_events {
        preflight::charge_capture_proposal_work(proposal_work, search_work + 1, cancel)?;
        boundary(schedule.partition_point(|layer| layer.z <= *event));
        if let SupportPolicy::Breakaway {
            z_gap_mm,
            interface_thickness_mm,
            ..
        } = intent.recipe.supports
        {
            for stop in [event - z_gap_mm, event - z_gap_mm - interface_thickness_mm] {
                preflight::charge_capture_proposal_work(proposal_work, search_work, cancel)?;
                boundary(schedule.partition_point(|layer| layer.z <= stop));
            }
        }
    }
    for (band, proposal_only) in admission
        .native_event_bands
        .iter()
        .map(|band| (band, false))
        .chain(
            admission
                .native_capture_proposal_bands
                .iter()
                .map(|band| (band, true)),
        )
    {
        preflight::charge_capture_proposal_work(proposal_work, 1, cancel)?;
        if proposal_only || band[0] != band[1] {
            preflight::charge_capture_proposal_work(proposal_work, search_work * 2, cancel)?;
            let first = schedule.partition_point(|layer| layer.z < band[0]);
            let after = schedule.partition_point(|layer| layer.lower_z <= band[1]);
            boundary(first);
            boundary(after);
            // Even an exact literal candidate may touch two CLOSED layers.
            // Analyze each existing contact layer independently rather than
            // placing a multi-layer reference on the candidate itself. This
            // changes no physical layer, material event or proof obligation.
            if proposal_only && first <= index as usize && (index as usize) < after {
                boundary(index as usize + 1);
            }
        }
    }
    Ok(end)
}

pub(super) fn split_layer(
    schedule: &mut Vec<LayerElevation>,
    index: u32,
    context: &str,
) -> Result<(), ManufacturingError> {
    let layer = &schedule[index as usize];
    let midpoint = layer.lower_z + (layer.z - layer.lower_z) * 0.5;
    let lower_height = midpoint - layer.lower_z;
    let upper_height = layer.z - midpoint;
    if lower_height.min(upper_height)
        <= 2.0 * spiling_contracts::geometry::SECTION_PLANE_TOLERANCE_MM
    {
        return Err(error(
            ManufacturingErrorCode::NativeShapeErrorExceeded,
            format!(
                "{context} cannot certify [{:.9}, {:.9}] mm at process-resolvable thickness",
                layer.lower_z, layer.z
            ),
        ));
    }
    if schedule.len() >= MAX_PRINT_LAYERS as usize {
        return Err(error(
            ManufacturingErrorCode::ResourceLimit,
            "native/field refinement exceeds the layer budget",
        ));
    }
    let upper = LayerElevation {
        lower_z: midpoint,
        z: layer.z,
        sample: midpoint + upper_height * 0.5,
        height: upper_height,
    };
    let lower = LayerElevation {
        lower_z: layer.lower_z,
        z: midpoint,
        sample: layer.lower_z + lower_height * 0.5,
        height: lower_height,
    };
    schedule[index as usize] = lower;
    schedule.insert(index as usize + 1, upper);
    Ok(())
}

fn admit_bytes(
    live: usize,
    additional: usize,
    coexisting: usize,
) -> Result<usize, ManufacturingError> {
    live.checked_add(additional)
        .filter(|n| {
            coexisting
                .checked_add(*n)
                .is_some_and(|total| total <= spiling_geometry::MAX_PREPARED_SECTION_BYTES)
        })
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "native source/stratum dependency retention capacity exceeded",
            )
        })
}

/// Project each relevant original-source void capture once, only after actual
/// deposited strokes established demand. These are scoped exclusion masks,
/// never material, anchor or approximate all-domain source certificates.
pub(super) fn project_protected_voids(
    input: &CompilerInput<'_>,
    sources: &BTreeMap<DefinitionId, spiling_geometry::PreparedSolid<'_>>,
    captures: &mut Vec<CapturedProtectedVoid>,
    demand: &ProtectedVoidDemand<'_>,
    preserve_captures: bool,
    support_work: &mut usize,
    work: &mut spiling_geometry::GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<super::support::ProtectedVoidSet, ManufacturingError> {
    let ProtectedVoidDemand {
        native_bounds,
        demand_bounds,
        point_limit,
        retained_native_owner_bytes,
    } = *demand;
    // Accepted material proof and immutable source owners coexist with every
    // projection and converted mask, including conversion-time admission.
    let mut retained_native = captures
        .capacity()
        .saturating_mul(std::mem::size_of::<CapturedProtectedVoid>())
        .saturating_add(
            captures
                .iter()
                .map(|value| value.capture.retained_bytes())
                .sum::<usize>(),
        )
        .saturating_add(retained_native_owner_bytes);
    let mut regions = Vec::new();
    let mut scopes = Vec::new();
    let mut points = 0usize;
    let mut converted_bytes = 0usize;
    let clearance = match input.intent.recipe.supports {
        SupportPolicy::Breakaway {
            xy_clearance_mm, ..
        } => xy_clearance_mm,
        SupportPolicy::Disabled => {
            return Err(error(
                ManufacturingErrorCode::InvalidSpecification,
                "native protected projection requested with disabled supports",
            ));
        }
    };
    let mut project_one = |captured: &CapturedProtectedVoid,
                           retained_native: usize|
     -> Result<(), ManufacturingError> {
        check_cancel(cancel)?;
        let bounds = native_bounds.get(&captured.occurrence_id).ok_or_else(|| {
            error(
                ManufacturingErrorCode::NativeAnalysisIndeterminate,
                "native void source has no captured placed bounds",
            )
        })?;
        let mut relevant = false;
        let reserve = clearance
            + super::OFFSET_TOLERANCE_MM
            + spiling_contracts::geometry::SECTION_SAMPLING_TOLERANCE_MM;
        for demand in demand_bounds {
            check_cancel(cancel)?;
            *support_work = support_work
                .checked_add(1)
                .filter(|n| *n <= super::MAX_BOOLEAN_PAIRS)
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "cumulative native void/demand projection work budget exceeded",
                    )
                })?;
            if (0..2).all(|axis| {
                bounds.min[axis] - reserve <= demand[axis][1]
                    && bounds.max[axis] + reserve >= demand[axis][0]
            }) {
                relevant = true;
                break;
            }
        }
        if relevant {
            let definition = input
                .definitions
                .get(captured.capture.definition_id())
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::NativeAnalysisIndeterminate,
                        "native void capture lost its original definition",
                    )
                })?;
            let available = spiling_geometry::MAX_PREPARED_SECTION_SOURCE_BYTES
                .checked_sub(retained_native)
                .and_then(|n| n.checked_sub(converted_bytes))
                .filter(|n| *n > 0)
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "native void projection has no retained-byte allowance",
                    )
                })?;
            let source = sources
                .get(captured.capture.definition_id())
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::NativeAnalysisIndeterminate,
                        "native void capture lost its retained original material owner",
                    )
                })?;
            let projected = source
                .projected_protected_void_envelopes(
                    &captured.capture,
                    spiling_geometry::ProtectedVoidPolicy {
                        maximum_xy_error_mm:
                            spiling_contracts::geometry::SECTION_SAMPLING_TOLERANCE_MM,
                        byte_limit: available as u32,
                    },
                    work,
                    cancel,
                )
                .map_err(|failure| {
                    super::native_error(
                        failure,
                        &format!(
                            "native protected void projection occurrence {}",
                            captured.occurrence_id.get()
                        ),
                    )
                })?;
            if projected.definition_id() != captured.capture.definition_id()
                || !projected.is_compatible_with_definition(definition)
                || projected.placed_pose() != captured.capture.placed_pose()
                || projected.world_height_interval_mm()
                    != captured.capture.world_height_interval_mm()
                || projected.sample_world_z_mm() != captured.capture.sample_world_z_mm()
            {
                return Err(error(
                    ManufacturingErrorCode::NativeAnalysisIndeterminate,
                    "native protected projection lost source/recipe/pose/query capture",
                ));
            }
            let required = regions
                .len()
                .checked_add(projected.envelopes().len())
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "native protected mask count overflow",
                    )
                })?;
            if required > regions.capacity() || required > scopes.capacity() {
                let capacity =
                    required.max(regions.capacity().max(scopes.capacity()).saturating_mul(2));
                let wrappers = (capacity - regions.capacity())
                    .saturating_mul(std::mem::size_of::<geo::Polygon<f64>>())
                    .saturating_add((capacity - scopes.capacity()).saturating_mul(
                        std::mem::size_of::<spiling_geometry::ProtectedVoidHeightScope>(),
                    ));
                admit_bytes(
                    retained_native,
                    projected.retained_bytes(),
                    converted_bytes.saturating_add(wrappers),
                )?;
                regions
                    .try_reserve_exact(capacity - regions.len())
                    .map_err(|_| {
                        error(
                            ManufacturingErrorCode::ResourceLimit,
                            "native protected mask allocation refused",
                        )
                    })?;
                scopes
                    .try_reserve_exact(capacity - scopes.len())
                    .map_err(|_| {
                        error(
                            ManufacturingErrorCode::ResourceLimit,
                            "native protected scope allocation refused",
                        )
                    })?;
                converted_bytes = converted_bytes.saturating_add(wrappers);
            }
            for envelope in projected.envelopes() {
                check_cancel(cancel)?;
                let boundary = envelope.world_xy_outer();
                points = points
                    .checked_add(boundary.points_mm.len())
                    .filter(|n| *n <= point_limit)
                    .ok_or_else(|| {
                        error(
                            ManufacturingErrorCode::ResourceLimit,
                            "native protected mask point budget exceeded",
                        )
                    })?;
                let additional = boundary
                    .points_mm
                    .len()
                    .saturating_mul(std::mem::size_of::<geo::Coord<f64>>());
                admit_bytes(
                    retained_native,
                    projected.retained_bytes(),
                    converted_bytes.saturating_add(additional),
                )?;
                let scope = envelope.height_scope();
                let valid = match scope {
                    spiling_geometry::ProtectedVoidHeightScope::Bounded { lower_mm, upper_mm } => {
                        lower_mm.is_finite() && upper_mm.is_finite() && lower_mm <= upper_mm
                    }
                    spiling_geometry::ProtectedVoidHeightScope::OpenBelow { upper_mm } => {
                        upper_mm.is_finite()
                    }
                };
                if !valid {
                    return Err(error(
                        ManufacturingErrorCode::NativeAnalysisIndeterminate,
                        "native protected void height scope is invalid",
                    ));
                }
                regions.push(region::from_world_outer(boundary, cancel)?);
                scopes.push(scope);
                converted_bytes = converted_bytes.saturating_add(additional);
            }
        }
        Ok(())
    };
    if preserve_captures {
        for captured in captures.iter() {
            project_one(captured, retained_native)?;
        }
    } else {
        for captured in captures.drain(..) {
            let payload_bytes = captured.capture.retained_bytes();
            project_one(&captured, retained_native)?;
            drop(captured);
            retained_native -= payload_bytes;
        }
    }
    drop(project_one);
    Ok(super::support::ProtectedVoidSet {
        regions: MultiPolygon(regions),
        scopes,
    })
}

// Native placed occupancy is an outward WORLD rectangle. Recognize that exact
// storage invariant before intersecting independent closed-M enclosures; the
// bounding box of an arbitrary polygon cannot replace its occupied set.
fn occupied_rectangle_bounds(polygon: &geo::Polygon<f64>) -> Option<[[f64; 2]; 2]> {
    let ring = &polygon.exterior().0;
    if !polygon.interiors().is_empty() || ring.len() != 5 || ring[0] != ring[4] {
        return None;
    }
    let mut bounds = [[f64::INFINITY, f64::NEG_INFINITY]; 2];
    for point in &ring[..4] {
        for (axis, value) in [point.x, point.y].into_iter().enumerate() {
            if !value.is_finite() {
                return None;
            }
            bounds[axis][0] = bounds[axis][0].min(value);
            bounds[axis][1] = bounds[axis][1].max(value);
        }
    }
    if bounds.iter().any(|axis| axis[0] >= axis[1]) {
        return None;
    }
    let mut corners = 0u8;
    for edge in ring.windows(2) {
        if edge[0].x != edge[1].x && edge[0].y != edge[1].y {
            return None;
        }
        let x = if edge[0].x == bounds[0][0] {
            0
        } else if edge[0].x == bounds[0][1] {
            1
        } else {
            return None;
        };
        let y = if edge[0].y == bounds[1][0] {
            0
        } else if edge[0].y == bounds[1][1] {
            1
        } else {
            return None;
        };
        let corner = 1u8 << (x + 2 * y);
        if corners & corner != 0 {
            return None;
        }
        corners |= corner;
    }
    (corners == 15).then_some(bounds)
}

fn consume_closed_source_projection(
    source: &NativeSourceInputs<'_, '_>,
    current: &mut DraftStage,
    pending: &spiling_geometry::PendingNativeUnionProof,
    candidate: &Vec<spiling_geometry::SectionLoopMm>,
    outside_bytes: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    if pending.critical_collars().is_empty() {
        return Ok(()); // Regular spans keep their existing path unchanged.
    }
    // Called only after full union certification. Neither these exclusions nor
    // their refinement participate in constructing their own native witness.
    let live = outside_bytes
        .checked_add(current.retained_bytes())
        .and_then(|n| n.checked_add(pending.retained_bytes()))
        .and_then(|n| n.checked_add(loop_bytes(candidate)))
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "qualified native projection live-byte overflow",
            )
        })?;
    admit_bytes(live, 0, 0)?;
    let old_points = current.occupied.point_count;
    for ((occurrence, evidence), material) in source
        .occurrences
        .iter()
        .zip(&current.analyses)
        .zip(&current.leaves)
    {
        check_cancel(cancel)?;
        let Some(fresh) = &evidence.collar else {
            continue;
        };
        super::boolean::charge(work, pending.critical_collars().len())?;
        let Some(incidence) = pending.critical_collars().iter().find(|bound| {
            bound.current_leaf().occurrence_id() == occurrence.occurrence_id
                && bound.incidence_sha256() == fresh.incidence_sha256()
        }) else {
            continue;
        };
        let leaf = incidence.current_leaf();
        let collar = incidence.native_collar();
        let pose_matches = |pose: RigidPoseMm| {
            pose.translation_mm.map(f64::to_bits)
                == occurrence.pose.translation_mm.map(f64::to_bits)
                && pose.rotation_xyzw.map(f64::to_bits)
                    == occurrence.pose.rotation_xyzw.map(f64::to_bits)
        };
        let interval_bits = current.interval.map(f64::to_bits);
        if evidence.occurrence_id != occurrence.occurrence_id
            || leaf.definition_id() != &occurrence.definition_id
            || material.leaf.occurrence_id() != occurrence.occurrence_id
            || material.leaf.definition_id() != leaf.definition_id()
            || !pose_matches(leaf.pose())
            || !pose_matches(material.leaf.pose())
            || collar.definition_id() != leaf.definition_id()
            || !pose_matches(collar.placed_pose())
            || leaf.world_height_interval_mm().map(f64::to_bits) != interval_bits
            || material.leaf.world_height_interval_mm().map(f64::to_bits) != interval_bits
            || collar.world_height_interval_mm().map(f64::to_bits) != interval_bits
            || leaf.sample_world_z_mm().to_bits() != current.native_sample.to_bits()
            || material.leaf.sample_world_z_mm().to_bits() != current.native_sample.to_bits()
            || collar.sample_world_z_mm().to_bits() != current.native_sample.to_bits()
            || leaf.native_sample_sha256() != material.leaf.native_sample_sha256()
            || collar.native_sample_sha256() != leaf.native_sample_sha256()
            || leaf.candidate_geometry_sha256() != material.leaf.candidate_geometry_sha256()
            || collar.world_shape_error_policy_mm().to_bits()
                != source.input.intent.recipe.native_shape_error_mm.to_bits()
            || evidence.analysis.definition_id() != leaf.definition_id()
            || !evidence.analysis.placed_pose().is_some_and(pose_matches)
            || evidence
                .analysis
                .world_height_interval_mm()
                .map(|v| v.map(f64::to_bits))
                != Some(interval_bits)
            || evidence.analysis.sample_world_z_mm().map(f64::to_bits)
                != Some(current.native_sample.to_bits())
            || evidence
                .analysis
                .world_shape_error_policy_mm()
                .map(f64::to_bits)
                != Some(collar.world_shape_error_policy_mm().to_bits())
        {
            return Err(error(
                ManufacturingErrorCode::NativeAnalysisIndeterminate,
                "qualified native projection changed its source/pose/closed-cut/sample/policy binding",
            ));
        }
        let projection = collar.source_occupied_xy_bounds_mm();
        if projection
            .iter()
            .any(|axis| !axis[0].is_finite() || !axis[1].is_finite() || axis[0] >= axis[1])
        {
            continue; // No positive rectangle authority: retain the old outer C.
        }
        super::boolean::charge(work, current.occupied.pieces.len())?;
        for piece in &mut current.occupied.pieces {
            if piece.raw_native_occurrence != Some(occurrence.occurrence_id) {
                continue;
            }
            if piece.height_mm[0] < current.interval[0] || piece.height_mm[1] > current.interval[1]
            {
                return Err(error(
                    ManufacturingErrorCode::NativeAnalysisIndeterminate,
                    "native occupied operand escaped its qualified closed-height scope",
                ));
            }
            super::boolean::charge(work, 15)?;
            let Some(outer) = occupied_rectangle_bounds(&piece.polygon) else {
                continue;
            };
            let bounds = std::array::from_fn::<_, 2, _>(|axis| {
                [
                    outer[axis][0].max(projection[axis][0]),
                    outer[axis][1].min(projection[axis][1]),
                ]
            });
            if bounds.iter().any(|axis| axis[0] >= axis[1]) || bounds == outer {
                continue; // Never infer empty material from an empty intersection.
            }
            let prior_points = piece.polygon.exterior().0.len();
            let prior_bytes = polygon_bytes(&mut piece.polygon);
            // Five closed rectangle coordinates already exist. Rewrite them
            // without allocating another envelope beside the old live owners.
            piece.polygon.exterior_mut(|ring| {
                ring.0[0] = geo::Coord {
                    x: bounds[0][0],
                    y: bounds[1][0],
                };
                ring.0[1] = geo::Coord {
                    x: bounds[0][1],
                    y: bounds[1][0],
                };
                ring.0[2] = geo::Coord {
                    x: bounds[0][1],
                    y: bounds[1][1],
                };
                ring.0[3] = geo::Coord {
                    x: bounds[0][0],
                    y: bounds[1][1],
                };
                ring.0[4] = ring.0[0];
            });
            let points = piece.polygon.exterior().0.len();
            let bytes = polygon_bytes(&mut piece.polygon);
            current.occupied.point_count = current
                .occupied
                .point_count
                .checked_sub(prior_points)
                .and_then(|n| n.checked_add(points))
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "native occupied projection point accounting mismatch",
                    )
                })?;
            current.occupied.payload_bytes = current
                .occupied
                .payload_bytes
                .checked_sub(prior_bytes)
                .and_then(|n| n.checked_add(bytes))
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "native occupied projection byte accounting mismatch",
                    )
                })?;
            // Retain the original closed-height intersection, including caps.
        }
    }
    current.point_count = current
        .point_count
        .checked_sub(old_points)
        .and_then(|n| n.checked_add(current.occupied.point_count))
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "native stage occupied projection point accounting mismatch",
            )
        })?;
    Ok(())
}

fn certify_stage<'a>(
    source: &NativeSourceInputs<'_, '_>,
    current: &mut DraftStage,
    candidate: &Vec<spiling_geometry::SectionLoopMm>,
    adjacent: AdjacentSourceMaterials<'_, impl Iterator<Item = &'a SourceMaterial> + Clone>,
    coexisting: usize,
    work: &mut spiling_geometry::GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<spiling_geometry::PendingNativeUnionProof, ManufacturingError> {
    let NativeSourceInputs {
        input,
        occurrences,
        sources: prepared_sources,
    } = *source;
    let AdjacentSourceMaterials { lower, upper } = adjacent;
    for ((occurrence, evidence), material) in occurrences
        .iter()
        .zip(&mut current.analyses)
        .zip(&current.leaves)
    {
        let CapturedEvidence {
            analysis, collar, ..
        } = evidence;
        if !analysis.has_critical_collar() {
            continue;
        }
        let current_material = material;
        let adjacent_lower = lower.clone().find(|material| {
            material.leaf.occurrence_id() == occurrence.occurrence_id
                && material.leaf.world_height_interval_mm()[1] == current.interval[0]
        });
        let adjacent_upper = upper.iter().find(|material| {
            material.leaf.occurrence_id() == occurrence.occurrence_id
                && material.leaf.world_height_interval_mm()[0] == current.interval[1]
        });
        let sources = [
            current_material
                .leaf
                .is_positive()
                .then_some(current_material),
            adjacent_lower,
            adjacent_upper,
        ];
        let first = sources.iter().flatten().next().ok_or_else(|| error(
            ManufacturingErrorCode::NativeAnalysisIndeterminate,
            "critical native closed material has no actual positive current or adjacent source target"))?;
        let mut target_storage: [_; 3] =
            std::array::from_fn(|_| spiling_geometry::NativeCriticalCollarTarget {
                leaf: &first.leaf,
                actual_material: &first.actual,
            });
        let mut target_count = 0;
        for source in sources.into_iter().flatten() {
            target_storage[target_count] = spiling_geometry::NativeCriticalCollarTarget {
                leaf: &source.leaf,
                actual_material: &source.actual,
            };
            target_count += 1;
        }
        let targets = &target_storage[..target_count];
        let live = coexisting
            .saturating_add(current.native_bytes)
            .saturating_add(loop_bytes(candidate));
        let available = spiling_geometry::MAX_PREPARED_SECTION_SOURCE_BYTES
            .checked_sub(live)
            .filter(|n| *n > 0)
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "critical native collar has no retained-byte allowance",
                )
            })?;
        let metric = prepared_sources[&occurrence.definition_id].certify_native_critical_collar(
            analysis,
            targets,
            spiling_geometry::NativeCriticalCollarPolicy {
                maximum_xy_cover_error_mm: input.intent.recipe.native_shape_error_mm * 0.25,
                maximum_distance_mm: input.intent.recipe.native_shape_error_mm,
                byte_limit: available as u32,
            },
            work,
            cancel,
        );
        let metric = metric.map_err(|failure| {
            super::native_error(
                failure,
                &format!(
                    "critical native closed-source cover, occurrence {}",
                    occurrence.occurrence_id.get()
                ),
            )
        })?;
        let incidence = spiling_geometry::certify_native_collar_incidence(
            analysis,
            &current_material.leaf,
            &metric,
            targets,
            work,
            cancel,
        );
        let incidence = incidence.map_err(|failure| {
            super::native_error(
                failure,
                &format!(
                    "critical native incidence, occurrence {}",
                    occurrence.occurrence_id.get()
                ),
            )
        })?;
        current.native_bytes =
            admit_bytes(current.native_bytes, incidence.retained_bytes(), coexisting)?;
        *collar = Some(incidence);
    }
    let mut captures = Vec::with_capacity(occurrences.len());
    for ((occurrence, evidence), material) in occurrences
        .iter()
        .zip(&current.analyses)
        .zip(&current.leaves)
    {
        let CapturedEvidence {
            analysis, collar, ..
        } = evidence;
        let leaf = &material.leaf;
        let evidence = if let Some(collar) = collar {
            spiling_geometry::NativeUnionOccurrenceEvidence::SlabWithCollar {
                analysis,
                leaf,
                endpoint_ownerships: &[],
                collar,
            }
        } else {
            spiling_geometry::NativeUnionOccurrenceEvidence::Slab {
                analysis,
                leaf,
                endpoint_ownerships: &[],
            }
        };
        captures.push(spiling_geometry::NativeUnionOccurrence {
            occurrence_id: occurrence.occurrence_id,
            source: &prepared_sources[&occurrence.definition_id],
            pose: occurrence.pose,
            evidence,
        });
    }
    let mut pending = spiling_geometry::certify_native_union(
        &captures,
        candidate,
        spiling_geometry::NativeUnionPolicy {
            candidate_coordinate_error_mm: current.coordinate_error_request_mm,
        },
        work,
        cancel,
    )
    .map_err(|failure| {
        super::native_error(
            failure,
            &format!(
                "composed native metric/feature gate at layer {}",
                current.index
            ),
        )
    })?;
    drop(captures);
    // Pending owns its captures. All other current/frontier storage and the
    // actual candidate still coexist with pair scratch and retained receipts.
    let pair_byte_limit = coexisting
        .checked_add(current.retained_bytes())
        .and_then(|live| live.checked_add(loop_bytes(candidate)))
        .and_then(|live| spiling_geometry::MAX_PREPARED_SECTION_SOURCE_BYTES.checked_sub(live));
    for material in current.positive_materials() {
        let Some(previous) = lower.clone().find(|previous| {
            previous.leaf.occurrence_id() == material.leaf.occurrence_id()
                && previous.leaf.world_height_interval_mm()[1].to_bits()
                    == current.interval[0].to_bits()
        }) else {
            continue;
        };
        let byte_limit = pair_byte_limit.filter(|limit| *limit > 0).ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "actual adjacent native source pair has no retained-byte allowance",
            )
        })?;
        // False grants no authority; the existing ledger still owns every
        // unmatched regular, critical, coface and whole-closed-cut obligation.
        pending
            .certify_regular_interface_pair(
                spiling_geometry::NativeCriticalCollarTarget {
                    leaf: &previous.leaf,
                    actual_material: &previous.actual,
                },
                spiling_geometry::NativeCriticalCollarTarget {
                    leaf: &material.leaf,
                    actual_material: &material.actual,
                },
                byte_limit,
                work,
                cancel,
            )
            .map_err(|failure| {
                super::native_error(
                    failure,
                    &format!(
                        "actual adjacent native source pair at layer {}, occurrence {}",
                        current.index,
                        material.leaf.occurrence_id().get(),
                    ),
                )
            })?;
    }
    Ok(pending)
}

/// Final publication gate over actual emitted support capsules. Boolean masks
/// are candidate construction only; the shared geometry predicate sees every
/// raw continuous occupied piece and every relevant positive protected void.
pub(super) fn certify_support_clearance(
    layers: &[PrintLayer],
    geometry: &SupportClearanceGeometry<'_>,
    recipe: &PlanarPrintRecipe,
    native_work: &mut spiling_geometry::GeometryWorkBudget,
    support_work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let SupportClearanceGeometry {
        sections,
        schedule,
        protected,
        scopes,
    } = *geometry;
    if protected.0.len() != scopes.len() {
        return Err(error(
            ManufacturingErrorCode::NativeAnalysisIndeterminate,
            "actual support clearance requires one certified height scope for every protected native void mask",
        ));
    }
    if layers.len() != schedule.len() {
        return Err(error(
            ManufacturingErrorCode::NativeAnalysisIndeterminate,
            "actual support clearance lost the complete actual-height schedule",
        ));
    }
    let SupportPolicy::Breakaway {
        xy_clearance_mm,
        z_gap_mm,
        ..
    } = recipe.supports
    else {
        return Ok(());
    };
    let Some(vertical) = deposition_vertical_extensions(layers, schedule, support_work, cancel)?
    else {
        // Exhausted the actual paths: there are no emitted support segments,
        // rather than trusting a generated-support flag or skipping a proof.
        return Ok(());
    };
    for span in sections {
        let interval = [
            schedule[span.start as usize].lower_z,
            schedule[(span.end - 1) as usize].z,
        ];
        for (polygon, piece_scope) in span.occupied_pieces() {
            let height = piece_scope.map_or(interval, |height| {
                [interval[0].max(height[0]), interval[1].min(height[1])]
            });
            if height[0] > height[1] {
                continue;
            }
            let scope = spiling_geometry::ProtectedVoidHeightScope::Bounded {
                lower_mm: height[0],
                upper_mm: height[1],
            };
            let applicable =
                scoped_support_layers(layers, schedule, scope, vertical, support_work, cancel)?;
            certify_polygon_capsules(
                applicable,
                &ForbiddenPolygonScope {
                    polygon,
                    height: scope,
                    label: "continuous native/finish outer occupancy",
                },
                xy_clearance_mm,
                native_work,
                support_work,
                cancel,
            )?;
        }
    }
    for (polygon, scope) in protected.0.iter().zip(scopes) {
        let applicable =
            scoped_support_layers(layers, schedule, *scope, vertical, support_work, cancel)?;
        certify_polygon_capsules(
            applicable,
            &ForbiddenPolygonScope {
                polygon,
                height: *scope,
                label: "projected protected native void",
            },
            xy_clearance_mm,
            native_work,
            support_work,
            cancel,
        )?;
    }
    // A bounded-fidelity M/D prism or emitted bead can extend outside true T.
    // Native target exclusion therefore does not replace this distinct actual
    // nominal-program footprint proof.
    certify_deposited_clearance(
        &ActualDepositionScope {
            layers,
            schedule,
            vertical,
        },
        xy_clearance_mm,
        z_gap_mm,
        native_work,
        support_work,
        cancel,
    )?;
    Ok(())
}

fn certify_polygon_capsules(
    layers: &[PrintLayer],
    forbidden: &ForbiddenPolygonScope<'_>,
    clearance: f64,
    native_work: &mut spiling_geometry::GeometryWorkBudget,
    support_work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let ForbiddenPolygonScope {
        polygon,
        height: scope,
        label,
    } = *forbidden;
    let mut encoded = None;
    for layer in layers {
        charge_support_clearance(support_work, cancel)?;
        for path in &layer.paths {
            charge_support_clearance(support_work, cancel)?;
            if !matches!(
                path.kind,
                DepositionKind::Support | DepositionKind::SupportInterface
            ) {
                continue;
            }
            for local in path.checked_edges()? {
                let edge = local.points_mm;
                charge_support_clearance(support_work, cancel)?;
                let [lower, upper] =
                    project_xyz6_segment_bead_height_mm(edge[0][2], edge[1][2], path.height_mm())?;
                let relevant = match scope {
                    spiling_geometry::ProtectedVoidHeightScope::Bounded { lower_mm, upper_mm } => {
                        lower_mm <= upper && upper_mm >= lower
                    }
                    spiling_geometry::ProtectedVoidHeightScope::OpenBelow { upper_mm } => {
                        upper_mm >= lower
                    }
                };
                if !relevant {
                    continue;
                }
                let (segment, endpoint_error_mm) = emitted_xy_segment(edge)?;
                if encoded.is_none() {
                    encoded = Some(encode_polygons(std::iter::once(polygon), 0.0, cancel)?);
                }
                spiling_geometry::certify_planar_capsule_clearance(
                    segment,
                    encoded.as_ref().expect("encoded actual forbidden polygon"),
                    spiling_geometry::PlanarCapsuleClearancePolicy {
                        radius_mm: local.width_mm * 0.5,
                        required_gap_mm: clearance,
                        endpoint_error_mm,
                    },
                    native_work,
                    cancel,
                )
                .map_err(|failure| {
                    super::native_error(
                        failure,
                        &format!(
                            "actual emitted support layer {} against {label}",
                            layer.index
                        ),
                    )
                })?;
            }
        }
    }
    Ok(())
}

fn emitted_xy_segment(edge: &[[f64; 3]]) -> Result<([[f64; 2]; 2], f64), ManufacturingError> {
    let segment = [
        [
            project_xyz6_coordinate_mm(edge[0][0])?,
            project_xyz6_coordinate_mm(edge[0][1])?,
        ],
        [
            project_xyz6_coordinate_mm(edge[1][0])?,
            project_xyz6_coordinate_mm(edge[1][1])?,
        ],
    ];
    let mut error = 0.0f64;
    for point in &segment {
        let radius = xyz6_coordinate_error_mm(point[0])?.hypot(xyz6_coordinate_error_mm(point[1])?);
        error = error.max(if radius == 0.0 { 0.0 } else { radius.next_up() });
    }
    Ok((segment, error))
}

pub(super) fn charge_support_clearance(
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    *work = work
        .checked_add(1)
        .filter(|n| *n <= super::MAX_BOOLEAN_PAIRS)
        .ok_or_else(|| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "cumulative actual support-clearance scan work exceeded",
            )
        })?;
    Ok(())
}

/// One linear scan derives two scalar vertical representation enclosures from
/// every actual object/support endpoint and the true schedule. No coordinate
/// index/cache or copied path buffer is retained. Uniform extensions preserve
/// monotonicity for charged scope searches, including actual deposited pairs.
fn deposition_vertical_extensions(
    layers: &[PrintLayer],
    schedule: &[LayerElevation],
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<[f64; 2]>, ManufacturingError> {
    let mut extensions = [0.0f64; 2];
    let mut any = false;
    for (layer, elevation) in layers.iter().zip(schedule) {
        charge_support_clearance(work, cancel)?;
        for path in &layer.paths {
            charge_support_clearance(work, cancel)?;
            if path.points_mm.len() < 2 {
                continue;
            }
            any |= matches!(
                path.kind,
                DepositionKind::Support | DepositionKind::SupportInterface
            );
            for point in &path.points_mm {
                charge_support_clearance(work, cancel)?;
                let [lower, upper] = project_xyz6_bead_height_mm(point[2], path.height_mm())?;
                extensions[0] = extensions[0].max((elevation.lower_z - lower).next_up());
                extensions[1] = extensions[1].max((upper - elevation.z).next_up());
            }
        }
    }
    if extensions.iter().any(|value| !value.is_finite()) {
        return Err(error(
            ManufacturingErrorCode::ProgramCoordinateConditioningFailure,
            "actual emitted deposition vertical representation cannot be enclosed",
        ));
    }
    Ok(any.then_some(extensions))
}

fn scoped_support_layers<'a>(
    layers: &'a [PrintLayer],
    schedule: &[LayerElevation],
    scope: spiling_geometry::ProtectedVoidHeightScope,
    extensions: [f64; 2],
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<&'a [PrintLayer], ManufacturingError> {
    let (lower, upper) = match scope {
        spiling_geometry::ProtectedVoidHeightScope::Bounded { lower_mm, upper_mm } => {
            (Some(lower_mm), upper_mm)
        }
        spiling_geometry::ProtectedVoidHeightScope::OpenBelow { upper_mm } => (None, upper_mm),
    };
    let first = if let Some(lower) = lower {
        partition_schedule(
            schedule,
            |layer| (layer.z + extensions[1]).next_up() < lower,
            work,
            cancel,
        )?
    } else {
        0
    };
    let end = partition_schedule(
        schedule,
        |layer| (layer.lower_z - extensions[0]).next_down() <= upper,
        work,
        cancel,
    )?;
    if first > end {
        return Err(error(
            ManufacturingErrorCode::NativeAnalysisIndeterminate,
            "native support-clearance height scope is not ordered",
        ));
    }
    Ok(&layers[first..end])
}

fn partition_schedule(
    schedule: &[LayerElevation],
    mut before: impl FnMut(&LayerElevation) -> bool,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<usize, ManufacturingError> {
    let mut first = 0;
    let mut end = schedule.len();
    while first < end {
        charge_support_clearance(work, cancel)?;
        let middle = first + (end - first) / 2;
        if before(&schedule[middle]) {
            first = middle + 1;
        } else {
            end = middle;
        }
    }
    Ok(first)
}

fn certify_deposited_clearance(
    deposition: &ActualDepositionScope<'_>,
    xy_gap: f64,
    z_gap: f64,
    native_work: &mut spiling_geometry::GeometryWorkBudget,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let ActualDepositionScope {
        layers,
        schedule,
        vertical,
    } = *deposition;
    for (index, support_layer) in layers.iter().enumerate() {
        charge_support_clearance(work, cancel)?;
        let mut applicable = None;
        for support in &support_layer.paths {
            charge_support_clearance(work, cancel)?;
            if !matches!(
                support.kind,
                DepositionKind::Support | DepositionKind::SupportInterface
            ) || support.points_mm.len() < 2
            {
                continue;
            }
            if applicable.is_none() {
                let lower = (schedule[index].lower_z - vertical[0]).next_down();
                let upper = (schedule[index].z + vertical[1]).next_up();
                // These are already outward physical bead bounds: expand by
                // the minimum Z gap, never reproject them as XYZ6 coordinates.
                let scope = spiling_geometry::ProtectedVoidHeightScope::Bounded {
                    lower_mm: program_coordinates::enclosed_vertical_gap_mm(lower, z_gap)?,
                    upper_mm: -program_coordinates::enclosed_vertical_gap_mm(-upper, z_gap)?,
                };
                applicable = Some(scoped_support_layers(
                    layers, schedule, scope, vertical, work, cancel,
                )?);
            }
            let objects =
                applicable.expect("actual support selected a complete applicable layer slice");
            for support_local in support.checked_edges()? {
                let support_edge = support_local.points_mm;
                charge_support_clearance(work, cancel)?;
                let support_height = project_xyz6_segment_bead_height_mm(
                    support_edge[0][2],
                    support_edge[1][2],
                    support.height_mm(),
                )?;
                let (support_xy, support_error) = emitted_xy_segment(support_edge)?;
                for object_layer in objects {
                    charge_support_clearance(work, cancel)?;
                    for object in &object_layer.paths {
                        charge_support_clearance(work, cancel)?;
                        if matches!(
                            object.kind,
                            DepositionKind::Support | DepositionKind::SupportInterface
                        ) {
                            continue;
                        }
                        for object_local in object.checked_edges()? {
                            let object_edge = object_local.points_mm;
                            charge_support_clearance(work, cancel)?;
                            let object_height = project_xyz6_segment_bead_height_mm(
                                object_edge[0][2],
                                object_edge[1][2],
                                object.height_mm(),
                            )?;
                            let above = program_coordinates::enclosed_vertical_gap_mm(
                                object_height[0],
                                support_height[1],
                            )?;
                            let below = program_coordinates::enclosed_vertical_gap_mm(
                                support_height[0],
                                object_height[1],
                            )?;
                            // All actual D matters, not only C or intervals
                            // that overlap. Exact zero-gap touching is not an
                            // interior collision when the requested gap is zero.
                            if above >= z_gap || below >= z_gap {
                                continue;
                            }
                            let (object_xy, object_error) = emitted_xy_segment(object_edge)?;
                            spiling_geometry::certify_planar_capsule_pair_clearance([
                                spiling_geometry::PlanarCapsuleMm {
                                    segment_xy_mm: support_xy, radius_mm: support_local.width_mm * 0.5,
                                    endpoint_error_mm: support_error,
                                },
                                spiling_geometry::PlanarCapsuleMm {
                                    segment_xy_mm: object_xy, radius_mm: object_local.width_mm * 0.5,
                                    endpoint_error_mm: object_error,
                                },
                            ], xy_gap, native_work, cancel).map_err(|failure| super::native_error(failure,
                                &format!("actual emitted support layer {} versus deposited object/bridge layer {}",
                                    support_layer.index, object_layer.index)))?;
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod actual_deposition_tests {
    use super::*;

    #[test]
    fn actual_bridge_and_support_strokes_require_z_gap_even_without_z_overlap() {
        let cancel = AtomicBool::new(false);
        for (gap, required, object_x, support_above, accepted) in [
            (0.1, 0.2, 0.0, false, false),
            (0.25, 0.2, 0.0, false, true),
            (0.25, 0.25, 0.0, false, true),
            (0.1, 0.2, 2.0, false, true),
            (0.1, 0.2, 0.0, true, false),
            (0.25, 0.2, 0.0, true, true),
            (0.0, 0.0, 0.0, false, true),
        ] {
            let mut layers = Vec::new();
            let mut schedule = Vec::new();
            let mut lower = 0.75;
            let mut push = |kind: DepositionKind, height: f64, x: f64| {
                let z = lower + height;
                let support = matches!(
                    kind,
                    DepositionKind::Support | DepositionKind::SupportInterface
                );
                let object_area = if support { 0.0 } else { 1.0 };
                let support_area = if support { 1.0 } else { 0.0 };
                let volume =
                    (0.5 - height) * height + std::f64::consts::PI * (height * 0.5).powi(2);
                let index = layers.len() as u32;
                layers.push(PrintLayer { skin_command_fits: Vec::new(), index,
                z_mm: z,
                lower_z_mm: lower,
                height_mm: height,
                section_z_mm: lower + height * 0.5,
                region_coverage: PlanarRegionCoverage { skin_bridge_domains: Vec::new(), process_diaphragms: Vec::new(), diaphragm_area_mm2: 0.0, section_area_mm2: object_area,
                source_section_area_mm2: object_area,
                surface_added_area_mm2: 0.0,
                separator_area_mm2: 0.0,
                process_interfaces: Vec::new(),
                wall_area_mm2: 0.0,
                bottom_skin_area_mm2: 0.0,
                top_skin_area_mm2: 0.0,
                solid_interior_area_mm2: object_area,
                sparse_interior_area_mm2: 0.0,
                intentional_void_area_mm2: 0.0,
                protected_hole_area_mm2: 0.0,
                nominal_deposition_volume_mm3: object_area * volume,
                support_area_mm2: support_area,
                support_nominal_deposition_volume_mm3: support_area * volume,
                process_regions: if object_area > 0.0 {
                    vec![ProcessRegionCoverage {
                material_domain: None,
                        area_mm2: object_area,
                        fraction: 1.0,
                        contributions: Vec::new(),
                    }]
                } else {
                    Vec::new()
                }, },
                paths: vec![DepositionPath {
                    material_domain: None,
                    kind,
                    owner: match kind { DepositionKind::BottomSkin => Some(spiling_contracts::manufacturing::DepositionOwner::MandatorySkin { skin: spiling_contracts::manufacturing::ProcessSkinKind::BottomSkin }), DepositionKind::TopSkin => Some(spiling_contracts::manufacturing::DepositionOwner::MandatorySkin { skin: spiling_contracts::manufacturing::ProcessSkinKind::TopSkin }), _ => None },
                    width: spiling_contracts::manufacturing::WidthProfile::Constant { width_mm: 0.5 },
                    physical_z_mm: [lower, z],
                    points_mm: vec![[x, 0.0, z], [x + 1.0, 0.0, z]],
                }], });
                schedule.push(LayerElevation {
                    z,
                    lower_z: lower,
                    sample: lower + height * 0.5,
                    height,
                });
                lower = z;
            };
            let support = DepositionKind::SupportInterface;
            let object = DepositionKind::Bridge;
            push(
                if support_above { object } else { support },
                0.25,
                if support_above { object_x } else { 0.0 },
            );
            if gap > 0.0 {
                // Another real far-XY stroke fills the intervening scheduled
                // layer. This is not an invalid globally-empty contact gap.
                push(DepositionKind::SolidFill, gap, 10.0);
            }
            push(
                if support_above { support } else { object },
                0.25,
                if support_above { 0.0 } else { object_x },
            );
            let mut work = 0;
            let mut native_work = spiling_geometry::GeometryWorkBudget::new(10_000);
            let vertical = deposition_vertical_extensions(&layers, &schedule, &mut work, &cancel)
                .unwrap()
                .unwrap();
            // Exercise the actual-deposition consumer and real native capsule
            // proof, independently of C or contact-anchor authority.
            let result = certify_deposited_clearance(
                &ActualDepositionScope {
                    layers: &layers,
                    schedule: &schedule,
                    vertical,
                },
                0.2,
                required,
                &mut native_work,
                &mut work,
                &cancel,
            );
            assert_eq!(
                result.is_ok(),
                accepted,
                "actual Z gap={gap}, required={required}, object X={object_x}, support above={support_above}: {result:?}"
            );
            if !accepted {
                assert_eq!(
                    result.unwrap_err().code,
                    ManufacturingErrorCode::NativeAnalysisIndeterminate
                );
            }
        }
    }
}

#[cfg(test)]
mod allowance_tests {
    use super::*;

    #[test]
    fn native_pool_admission_rejects_cumulative_retention_and_overflow() {
        let limit = spiling_geometry::MAX_PREPARED_SECTION_BYTES;
        assert_eq!(admit_bytes(32, 16, limit - 48).unwrap(), 48);
        assert_eq!(
            admit_bytes(32, 17, limit - 48).unwrap_err().code,
            ManufacturingErrorCode::ResourceLimit
        );
        assert_eq!(
            admit_bytes(usize::MAX, 1, 0).unwrap_err().code,
            ManufacturingErrorCode::ResourceLimit
        );
    }
}

#[cfg(test)]
mod capture_proposal_tests {
    use super::*;

    fn admission(band: [f64; 2]) -> preflight::Admission {
        preflight::Admission {
            bounds: None,
            horizontal_events: Vec::new(),
            native_bounds: BTreeMap::new(),
            native_event_bands: Vec::new(),
            native_capture_proposal_bands: vec![band],
            material_domains: BTreeMap::new(),
            surface_fields: BTreeMap::new(),
            report: PreflightReport {
                input_revision: spiling_contracts::project::ProjectRevision(1),
                disposition: PreflightDisposition::Admitted,
                software_only: true,
                checks: Vec::new(),
            },
            geometry_work: spiling_geometry::GeometryWorkBudget::new(
                super::super::MAX_NATIVE_WORK_UNITS,
            ),
        }
    }

    fn schedule() -> Vec<LayerElevation> {
        (0..5)
            .map(|index| {
                let lower_z = index as f64 * 0.5;
                LayerElevation {
                    lower_z,
                    z: lower_z + 0.5,
                    sample: lower_z + 0.25,
                    height: 0.5,
                }
            })
            .collect()
    }

    #[test]
    fn capture_proposal_search_preserves_cumulative_work_limits_and_cancellation() {
        let admission = admission([1.0, 1.0]);
        let schedule = schedule();
        let intent: ManufacturingIntent = serde_json::from_slice(include_bytes!(
            "../../../../fixtures/manufacturing/complete-sparse.intent.json"
        ))
        .unwrap();
        let mut work = super::super::MAX_BOOLEAN_PAIRS;
        assert_eq!(
            span_end(
                &schedule,
                &admission,
                &intent,
                0,
                &mut work,
                &AtomicBool::new(false),
            )
            .unwrap_err()
            .code,
            ManufacturingErrorCode::ResourceLimit
        );
        assert_eq!(work, super::super::MAX_BOOLEAN_PAIRS);
        let mut work = 17;
        assert_eq!(
            span_end(
                &schedule,
                &admission,
                &intent,
                0,
                &mut work,
                &AtomicBool::new(true),
            )
            .unwrap_err()
            .code,
            ManufacturingErrorCode::Cancelled
        );
        assert_eq!(work, 17);
    }
}
