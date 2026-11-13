// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::{LayerElevation, preflight};
use crate::{check_cancel, error};
use geo::MultiPolygon;
use spiling_contracts::geometry::OccurrenceId;
use spiling_contracts::manufacturing::program_coordinates::{
    project_xyz6_bead_height_mm, project_xyz6_coordinate_mm, project_xyz6_segment_bead_height_mm,
    xyz6_coordinate_error_mm,
};
use spiling_contracts::manufacturing::*;
use std::sync::{Arc, atomic::AtomicBool};

pub(super) const MATERIAL_ARC_BYTES: usize =
    std::mem::size_of::<MultiPolygon<f64>>() + 2 * std::mem::size_of::<usize>();

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
    pub material: Arc<MultiPolygon<f64>>,
    pub finished: Vec<(OccurrenceId, Arc<MultiPolygon<f64>>)>,
    pub contributions: Vec<(OccurrenceId, Arc<MultiPolygon<f64>>, usize)>,
    pub occupied: Arc<super::ContinuousOccupied>,
    pub source_area_mm2: f64,
    pub point_count: usize,
    pub material_bytes: usize,
    pub finished_bytes: usize,
}

impl NativeSpan {
    /// A one-layer ADR012 tolerant span: no invariance or inner prism.
    #[expect(clippy::too_many_arguments)]
    pub(super) fn tolerant(
        end: u32,
        material: Arc<MultiPolygon<f64>>,
        contributions: Vec<(OccurrenceId, Arc<MultiPolygon<f64>>, usize)>,
        finished: Vec<(OccurrenceId, Arc<MultiPolygon<f64>>)>,
        finished_bytes: usize,
        occupied: Arc<super::ContinuousOccupied>,
        source_area_mm2: f64,
        point_count: usize,
        material_bytes: usize,
    ) -> Self {
        Self {
            end,
            material,
            finished,
            contributions,
            occupied,
            source_area_mm2,
            point_count,
            material_bytes,
            finished_bytes,
        }
    }
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
        .saturating_add(
            occupied.tolerant.capacity() * std::mem::size_of::<super::TolerantOccupied>(),
        )
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
    split_resolved_layer(schedule, index, 0.0, context)
}

/// The first mapped field whose vertical error over this scheduled layer
/// exceeds its declared bound, as the refinement cause. Both lanes refine the
/// layer until none does.
pub(super) fn surface_vertical_error(
    admission: &preflight::Admission,
    schedule: &[LayerElevation],
    index: u32,
) -> Result<Option<ManufacturingError>, ManufacturingError> {
    let layer = &schedule[index as usize];
    for (occurrence_id, fields) in &admission.surface_fields {
        for field in fields {
            if let Some(allowed) = field.allowed_vertical_error() {
                let bound = field.vertical_error_bound(layer.lower_z, layer.z, layer.sample)?;
                if bound > allowed {
                    return Ok(Some(error(
                        ManufacturingErrorCode::NativeShapeErrorExceeded,
                        format!(
                            "surface field at layer {index}, interval [{:.17e}, {:.17e}] mm, sample {:.17e} mm, occurrence {}: vertical error bound {bound:.17e} mm exceeds policy {allowed:.17e} mm",
                            layer.lower_z,
                            layer.z,
                            layer.sample,
                            occurrence_id.get()
                        ),
                    )));
                }
            }
        }
    }
    Ok(None)
}

/// Split one tolerant layer for a mapped field's vertical error, at the
/// captured process minimum, keeping the cause in any refusal.
pub(super) fn refine_surface_layer(
    schedule: &mut Vec<LayerElevation>,
    index: u32,
    minimum_height_mm: f64,
    cause: &ManufacturingError,
) -> Result<(), ManufacturingError> {
    split_resolved_layer(
        schedule,
        index,
        minimum_height_mm,
        "surface field vertical error",
    )
    .map_err(|mut failure| {
        failure.message.push_str("; ");
        failure.message.push_str(&cause.to_string());
        failure
    })
}

fn split_resolved_layer(
    schedule: &mut Vec<LayerElevation>,
    index: u32,
    minimum_height_mm: f64,
    context: &str,
) -> Result<(), ManufacturingError> {
    let layer = &schedule[index as usize];
    let midpoint = layer.lower_z + (layer.z - layer.lower_z) * 0.5;
    let lower_height = midpoint - layer.lower_z;
    let upper_height = layer.z - midpoint;
    let lower_sample = layer.lower_z + lower_height * 0.5;
    let upper_sample = midpoint + upper_height * 0.5;
    let unresolved = || {
        error(
            ManufacturingErrorCode::NativeShapeErrorExceeded,
            format!(
                "{context} cannot certify [{:.17e}, {:.17e}] mm at process-resolvable thickness",
                layer.lower_z, layer.z
            ),
        )
    };
    if !minimum_height_mm.is_finite()
        || minimum_height_mm < 0.0
        || !(layer.lower_z < lower_sample
            && lower_sample < midpoint
            && midpoint < upper_sample
            && upper_sample < layer.z)
    {
        return Err(unresolved());
    }
    let endpoints = [layer.lower_z, midpoint, layer.z];
    let commanded = [
        project_xyz6_bead_height_mm(endpoints[0], 0.0)?,
        project_xyz6_bead_height_mm(endpoints[1], 0.0)?,
        project_xyz6_bead_height_mm(endpoints[2], 0.0)?,
    ];
    for child in 0..2 {
        let physical =
            program_coordinates::enclosed_vertical_gap_mm(endpoints[child + 1], endpoints[child])?;
        let command = program_coordinates::enclosed_vertical_gap_mm(
            commanded[child + 1][0],
            commanded[child][1],
        )?;
        if physical <= 0.0
            || physical < minimum_height_mm
            || command <= 0.0
            || command < minimum_height_mm
        {
            return Err(unresolved());
        }
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
        sample: upper_sample,
        height: upper_height,
        course: layer.course,
    };
    let lower = LayerElevation {
        lower_z: layer.lower_z,
        z: midpoint,
        sample: lower_sample,
        height: lower_height,
        course: layer.course,
    };
    schedule[index as usize] = lower;
    schedule.insert(index as usize + 1, upper);
    Ok(())
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
        span.visit_occupied(cancel, |polygon, piece_scope| {
            let height = piece_scope.map_or(interval, |height| {
                [interval[0].max(height[0]), interval[1].min(height[1])]
            });
            if height[0] > height[1] {
                return Ok(true);
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
            Ok(true)
        })?;
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

/// Outward enclosure of an emitted XYZ6 centerline point's displacement from
/// its binary64 source plus its remaining physical-decimal endpoint error
/// (each at most about one half micrometre per axis). Bounds screens only.
pub(super) const XYZ6_SCREEN_SLACK_MM: f64 = 1e-5;

/// Raw XY bounds of a validated path and its largest bead width.
fn path_xy_bounds(path: &DepositionPath) -> Option<([[f64; 2]; 2], f64)> {
    let width = match &path.width {
        WidthProfile::Constant { width_mm } => *width_mm,
        WidthProfile::PerEdge { widths_mm } => widths_mm.iter().copied().fold(0.0, f64::max),
    };
    let mut bounds = [[f64::INFINITY; 2], [f64::NEG_INFINITY; 2]];
    for p in &path.points_mm {
        for axis in 0..2 {
            bounds[0][axis] = bounds[0][axis].min(p[axis]);
            bounds[1][axis] = bounds[1][axis].max(p[axis]);
        }
    }
    (width.is_finite() && bounds.iter().flatten().all(|v| v.is_finite())).then_some((bounds, width))
}

fn edge_xy_bounds(edge: &[[f64; 3]]) -> [[f64; 2]; 2] {
    [
        [edge[0][0].min(edge[1][0]), edge[0][1].min(edge[1][1])],
        [edge[0][0].max(edge[1][0]), edge[0][1].max(edge[1][1])],
    ]
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
    let forbidden_bounds = geo::BoundingRect::bounding_rect(polygon)
        .map(|b| [[b.min().x, b.min().y], [b.max().x, b.max().y]]);
    // Encode and validate the borrowed forbidden polygon lazily, once per scan.
    let encoded = std::cell::OnceCell::<Vec<spiling_geometry::SectionLoopMm>>::new();
    let mut prepared = None;
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
            let edges = path.checked_edges()?;
            if forbidden_bounds.is_some_and(|forbidden| {
                path_xy_bounds(path).is_some_and(|(bounds, width)| {
                    spiling_geometry::planar_bounds_clear(
                        bounds,
                        forbidden,
                        width * 0.5 + XYZ6_SCREEN_SLACK_MM,
                        clearance,
                    )
                })
            }) {
                continue;
            }
            for local in edges {
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
                if encoded.get().is_none() {
                    let _ = encoded.set(encode_polygons(std::iter::once(polygon), 0.0, cancel)?);
                }
                let loops = encoded.get().expect("encoded actual forbidden polygon");
                if prepared.is_none() {
                    prepared = Some(
                        spiling_geometry::prepare_planar_forbidden(loops, native_work, cancel)
                            .map_err(|failure| {
                                super::native_error(
                                    failure,
                                    &format!(
                                        "actual emitted support layer {} against {label}",
                                        layer.index
                                    ),
                                )
                            })?,
                    );
                }
                spiling_geometry::certify_planar_capsule_clearance_prepared(
                    segment,
                    prepared
                        .as_ref()
                        .expect("prepared actual forbidden polygon"),
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
    *work = work.checked_add(1).ok_or_else(|| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "cumulative actual support-clearance scan work counter overflow",
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
                let support_bounds = edge_xy_bounds(support_edge);
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
                            if spiling_geometry::planar_bounds_clear(
                                support_bounds,
                                edge_xy_bounds(object_edge),
                                (support_local.width_mm + object_local.width_mm) * 0.5
                                    + 2.0 * XYZ6_SCREEN_SLACK_MM,
                                xy_gap,
                            ) {
                                continue;
                            }
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
                layers.push(PrintLayer { skin_command_fits: Vec::new(), index, hatch_course: index,
                z_mm: z,
                lower_z_mm: lower,
                height_mm: height,
                section_z_mm: lower + height * 0.5,
                region_coverage: PlanarRegionCoverage {
            web_roof_domains: Vec::new(), skin_bridge_domains: Vec::new(), process_diaphragms: Vec::new(), diaphragm_area_mm2: 0.0, section_area_mm2: object_area,
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
                unsupported_overhang_mm: 0.0,
                support_omitted_wall_mm: 0.0,
                process_regions: if object_area > 0.0 {
                    vec![ProcessRegionCoverage {
                        functional_solid_zone: None,
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
                arcs: Vec::new(), }], });
                schedule.push(LayerElevation {
                    z,
                    lower_z: lower,
                    sample: lower + height * 0.5,
                    height,
                    course: schedule.len() as u32,
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
