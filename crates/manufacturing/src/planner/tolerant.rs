// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! ADR012 tolerant spans: one layer's material from each occurrence's placed
//! tessellation, in the same shape the exact native collector produces, so
//! walls, skins, infill and supports are unchanged. No native certificate is
//! produced or consumed here.

use super::{ContinuousOccupied, LayerElevation, native, region};
use crate::{check_cancel, error};
use geo::{BooleanOps, LineString, MultiPolygon, Polygon};
use spiling_contracts::geometry::{OccurrenceId, OccurrenceRecord};
use spiling_contracts::manufacturing::*;
use spiling_geometry::tolerant::{TolerantModel, TolerantSection};
use std::collections::BTreeMap;
use std::mem::size_of;
use std::sync::{Arc, atomic::AtomicBool};

/// Section loops as polygons: each hole belongs to the smallest outer loop
/// that contains it.
pub(super) fn polygons(section: &TolerantSection) -> MultiPolygon<f64> {
    let ring = |points: &[[f64; 2]]| -> LineString<f64> {
        let mut coords: Vec<geo::Coord<f64>> = points
            .iter()
            .map(|p| geo::Coord { x: p[0], y: p[1] })
            .collect();
        if let Some(first) = coords.first().copied() {
            coords.push(first);
        }
        LineString(coords)
    };
    let outers: Vec<usize> = (0..section.loops.len())
        .filter(|&i| section.loops[i].area > 0.0)
        .collect();
    let mut holes: Vec<Vec<LineString<f64>>> = vec![Vec::new(); outers.len()];
    for hole in section.loops.iter().filter(|l| l.area < 0.0) {
        let probe = hole.points[0];
        let owner = outers
            .iter()
            .enumerate()
            .filter(|(_, o)| contains(&section.loops[**o].points, probe))
            .min_by(|a, b| {
                section.loops[*a.1]
                    .area
                    .total_cmp(&section.loops[*b.1].area)
            })
            .map(|(k, _)| k);
        if let Some(k) = owner {
            holes[k].push(ring(&hole.points));
        }
    }
    MultiPolygon(
        outers
            .iter()
            .zip(holes)
            .map(|(o, h)| Polygon::new(ring(&section.loops[*o].points), h))
            .collect(),
    )
}

fn contains(ring: &[[f64; 2]], p: [f64; 2]) -> bool {
    let mut inside = false;
    let mut j = ring.len() - 1;
    for i in 0..ring.len() {
        let (a, b) = (ring[i], ring[j]);
        if (a[1] > p[1]) != (b[1] > p[1])
            && p[0] < (b[0] - a[0]) * (p[1] - a[1]) / (b[1] - a[1]) + a[0]
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

pub(super) fn section_at(
    model: &TolerantModel,
    z: f64,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    let section = model.section(z).map_err(|e| {
        error(
            ManufacturingErrorCode::UnsupportedGeometry,
            format!("tolerant section at Z {z} mm: {}", e.message),
        )
    })?;
    Ok(material_from_section(model, &section))
}

pub(super) fn material_from_section(
    model: &TolerantModel,
    section: &TolerantSection,
) -> MultiPolygon<f64> {
    let mut result = polygons(section);
    // Chords of flat faces cross the plane in runs of collinear points, and
    // freeform faces in sub-micrometre steps. Drop vertices within a quarter
    // of the profile tolerance, the sag analytic curves were densified to, so
    // walls see the section's shape rather than its triangle fans. Kept
    // vertices stay on the BREP.
    // Simplification keeps each ring's first point, so start every ring at
    // its lexicographically least vertex, a corner of its hull, not at
    // whichever mesh crossing the chain began on: a prism's straight edges
    // then lose every crossing and its layers share one bit-identical section.
    let epsilon = model.policy().profile_mm * 0.25;
    for polygon in &mut result.0 {
        polygon.exterior_mut(start_at_least);
        polygon.interiors_mut(|rings| rings.iter_mut().for_each(start_at_least));
    }
    result = geo::Simplify::simplify(&result, epsilon);
    result
        .0
        .retain(|polygon| geo::Area::unsigned_area(polygon) > epsilon * epsilon);
    result
}

fn start_at_least(ring: &mut LineString<f64>) {
    if ring.0.len() < 4 {
        return;
    }
    ring.0.pop();
    let start = (0..ring.0.len())
        .min_by(|&a, &b| {
            ring.0[a]
                .x
                .total_cmp(&ring.0[b].x)
                .then(ring.0[a].y.total_cmp(&ring.0[b].y))
        })
        .unwrap_or(0);
    ring.0.rotate_left(start);
    ring.0.push(ring.0[0]);
}

/// Raw converted sections at one exact Z, in placed occurrence order. The
/// slope union and held-span comparison each canonicalize at their old boundary.
#[derive(Default)]
pub(super) struct SampleRow {
    pub sections: Vec<MultiPolygon<f64>>,
    pub bytes: usize,
    pub work: usize,
    canonical: bool,
}

/// Compile-local store, bound to the immutable placed models of one admission.
/// A schedule split changes the lookup key, so a stale sample cannot authorize
/// the new layer. Consumed rows release their material immediately.
#[derive(Default)]
pub(super) struct SectionStore {
    rows: Vec<(u64, Option<SampleRow>)>,
    payload: usize,
}

type Models = BTreeMap<OccurrenceId, Arc<TolerantModel>>;

impl SectionStore {
    pub(super) fn bytes(&self) -> usize {
        self.payload.saturating_add(
            self.rows
                .capacity()
                .saturating_mul(size_of::<(u64, Option<SampleRow>)>()),
        )
    }

    fn position(&self, z: f64) -> Result<usize, usize> {
        self.rows
            .binary_search_by_key(&z.to_bits(), |&(key, _)| key)
    }

    pub(super) fn take(&mut self, z: f64) -> Option<SampleRow> {
        let index = self.position(z).ok()?;
        let row = self.rows[index].1.take()?;
        self.payload -= row.bytes;
        Some(row)
    }

    pub(super) fn remember(&mut self, z: f64, row: SampleRow, limit: usize) {
        if self.bytes().saturating_add(row.bytes) > limit {
            return;
        }
        let bytes = row.bytes;
        match self.position(z) {
            Ok(index) => {
                if self.rows[index].1.is_some() {
                    return;
                }
                self.rows[index].1 = Some(row);
            }
            Err(index) => {
                if self.rows.len() == self.rows.capacity() {
                    let extra = self.rows.capacity().max(4);
                    // The old buffer may remain live during reallocation.
                    let next = self.rows.capacity().saturating_add(extra);
                    if self
                        .bytes()
                        .saturating_add(row.bytes)
                        .saturating_add(next.saturating_mul(size_of::<(u64, Option<SampleRow>)>()))
                        > limit
                        || self.rows.try_reserve_exact(extra).is_err()
                    {
                        return;
                    }
                }
                self.rows.insert(index, (z.to_bits(), Some(row)));
            }
        }
        self.payload += bytes;
    }

    /// Prepare one ordered wave. Each worker has an independent share of
    /// remaining bytes; a row too large for its lane is computed in order
    /// with the whole available allowance when it is needed.
    pub(super) fn prefetch(
        &mut self,
        models: &Models,
        layers: &[LayerElevation],
        allowance: usize,
        cancel: &AtomicBool,
    ) {
        use rayon::prelude::*;
        let lanes = rayon::current_num_threads().max(1);
        let layers = &layers[..layers.len().min(lanes)];
        let header = layers.len().saturating_mul(size_of::<Option<SampleRow>>());
        let growth = allowance
            .saturating_sub(self.bytes())
            .saturating_sub(header);
        let Ok(lane) = crate::working_set::WorkingSet::new(growth / layers.len().max(1)) else {
            return;
        };
        let mut rows: Vec<_> = layers
            .par_iter()
            .map(|layer| {
                let z = layer.sample;
                if self.has(z) {
                    return None;
                }
                crate::working_set::scope(lane, || sample_row(models, z, lane.bytes(), cancel).ok())
            })
            .collect();
        let mut pending = rows.iter().flatten().map(|row| row.bytes).sum::<usize>();
        for (layer, row) in layers.iter().zip(&mut rows) {
            if let Some(row) = row.take() {
                pending -= row.bytes;
                self.remember(
                    layer.sample,
                    row,
                    allowance.saturating_sub(header).saturating_sub(pending),
                );
            }
        }
    }

    pub(super) fn sample(
        &mut self,
        models: &Models,
        z: f64,
        available: usize,
        cancel: &AtomicBool,
    ) -> Result<SampleRow, ManufacturingError> {
        check_cancel(cancel)?;
        if self.bytes() > available {
            *self = Self::default();
        }
        if let Some(row) = self.take(z) {
            return Ok(row);
        }
        let result = sample_row(models, z, available.saturating_sub(self.bytes()), cancel);
        if self.bytes() != 0
            && result
                .as_ref()
                .is_err_and(|failure| failure.code == ManufacturingErrorCode::ResourceLimit)
        {
            *self = Self::default();
            return sample_row(models, z, available, cancel);
        }
        result
    }

    pub(super) fn keep_scheduled(&mut self, schedule: &[LayerElevation]) {
        self.rows.retain(|&(bits, _)| {
            schedule
                .binary_search_by(|layer| layer.sample.total_cmp(&f64::from_bits(bits)))
                .is_ok()
        });
        self.payload = self
            .rows
            .iter()
            .filter_map(|(_, row)| row.as_ref())
            .map(|row| row.bytes)
            .sum();
    }

    fn has(&self, z: f64) -> bool {
        self.position(z)
            .is_ok_and(|index| self.rows[index].1.is_some())
    }
}

pub(super) fn sample_row(
    models: &Models,
    z: f64,
    allowance: usize,
    cancel: &AtomicBool,
) -> Result<SampleRow, ManufacturingError> {
    let mut row = SampleRow::default();
    let size = size_of::<MultiPolygon<f64>>();
    admit_section(models.len().saturating_mul(size), allowance)?;
    row.sections
        .try_reserve_exact(models.len())
        .map_err(|_| section_resource())?;
    row.bytes = row.sections.capacity().saturating_mul(size);
    for model in models.values() {
        let (section, bytes, work) =
            sample_model(model, z, allowance.saturating_sub(row.bytes), cancel)?;
        row.sections.push(section);
        row.bytes = row.bytes.saturating_add(bytes);
        super::boolean::charge(&mut row.work, work)?;
    }
    Ok(row)
}

fn section_resource() -> ManufacturingError {
    error(
        ManufacturingErrorCode::ResourceLimit,
        "tolerant section exceeds live byte allowance",
    )
}

fn admit_section(bytes: usize, allowance: usize) -> Result<(), ManufacturingError> {
    if bytes > allowance {
        Err(section_resource())
    } else {
        Ok(())
    }
}

fn sample_model(
    model: &TolerantModel,
    z: f64,
    allowance: usize,
    cancel: &AtomicBool,
) -> Result<(MultiPolygon<f64>, usize, usize), ManufacturingError> {
    use spiling_contracts::geometry::{GeometryError, GeometryErrorCode};
    check_cancel(cancel)?;
    let convert_error =
        |failure| super::native_error(failure, &format!("tolerant section at Z {z} mm"));
    let demand = model.section_demand(z).map_err(convert_error)?;
    let mut reserved = demand.scratch_bytes.saturating_add(demand.output_bytes);
    admit_section(reserved, allowance)?;
    let section = model
        .section_admitted(z, cancel, &mut |bytes| {
            reserved = reserved.saturating_add(bytes);
            if reserved > allowance {
                Err(GeometryError::new(
                    GeometryErrorCode::ResourceLimit,
                    "tolerant section exceeds live byte allowance",
                ))
            } else {
                Ok(())
            }
        })
        .map_err(convert_error)?;
    let (conversion_bytes, work) = super::slopes::section_conversion(&section)?;
    admit_section(
        super::slopes::section_bytes(&section).saturating_add(conversion_bytes),
        allowance,
    )?;
    let mut material = material_from_section(model, &section);
    check_cancel(cancel)?;
    drop(section);
    let bytes = native::material_bytes(&mut material);
    admit_section(bytes, allowance)?;
    let work = demand
        .triangles
        .checked_add(work)
        .ok_or_else(section_resource)?;
    Ok((material, bytes, work))
}

impl SampleRow {
    fn canonicalize(&mut self) {
        if !self.canonical {
            self.sections.iter_mut().for_each(region::canonicalize);
            self.canonical = true;
        }
    }
}

fn canonical_view(raw: &SampleRow) -> Vec<MultiPolygon<f64>> {
    let mut sections = raw.sections.clone();
    sections.iter_mut().for_each(region::canonicalize);
    sections
}

fn held_sections_match(a: &[MultiPolygon<f64>], b: &[MultiPolygon<f64>], profile: f64) -> bool {
    a.iter()
        .zip(b)
        .all(|(a, b)| a.0 == b.0 || super::same_within(a, b, profile))
}

fn read_sample(
    store: &mut SectionStore,
    models: &Models,
    layers: &[LayerElevation],
    available: usize,
    cancel: &AtomicBool,
) -> Result<SampleRow, ManufacturingError> {
    if !store.has(layers[0].sample) {
        store.prefetch(models, layers, available, cancel);
    }
    store.sample(models, layers[0].sample, available, cancel)
}

#[derive(Default)]
struct Midpoints {
    rows: Vec<SampleRow>,
    payload: usize,
}

impl Midpoints {
    fn bytes(&self) -> usize {
        self.payload
            .saturating_add(self.rows.capacity().saturating_mul(size_of::<SampleRow>()))
    }

    fn push(&mut self, row: SampleRow, allowance: usize) -> Result<(), ManufacturingError> {
        if self.rows.len() == self.rows.capacity() {
            let extra = self.rows.capacity().max(4);
            let new = self
                .rows
                .capacity()
                .saturating_add(extra)
                .saturating_mul(size_of::<SampleRow>());
            admit_section(
                self.bytes().saturating_add(row.bytes).saturating_add(new),
                allowance,
            )?;
            self.rows
                .try_reserve_exact(extra)
                .map_err(|_| section_resource())?;
        }
        self.payload = self.payload.saturating_add(row.bytes);
        admit_section(self.bytes(), allowance)?;
        self.rows.push(row);
        Ok(())
    }
}

/// Preserve the legacy occurrence/layer/plane operand order exactly. The
/// midpoint here is the raw row already used for held-span comparison, not a
/// clone of the canonical representative or a recomputed source section.
fn off_occupancy(
    occurrences: &[&OccurrenceRecord],
    admission: &super::preflight::Admission,
    layers: &[LayerElevation],
    mut midpoints: Midpoints,
    allowance: usize,
    cancel: &AtomicBool,
) -> Result<ContinuousOccupied, ManufacturingError> {
    let interval = [layers[0].lower_z, layers.last().unwrap().z];
    let mut occupied = ContinuousOccupied::default();
    // Midpoint geometry moves to occupied; it is already in `midpoint_bytes`.
    let midpoint_bytes = midpoints.bytes();
    let mut side_bytes = 0usize;
    for (ordinal, occurrence) in occurrences.iter().enumerate() {
        let bounds = admission.native_bounds[&occurrence.occurrence_id];
        let height = [
            interval[0].max(bounds.min[2]),
            interval[1].min(bounds.max[2]),
        ];
        if height[0] > height[1] {
            continue;
        }
        let model = &admission.models[&occurrence.occurrence_id];
        for (layer, midpoint) in layers.iter().zip(&mut midpoints.rows) {
            check_cancel(cancel)?;
            let inset = (0.25 * (layer.z - layer.lower_z)).min(0.01);
            for (plane, z) in [layer.lower_z + inset, layer.sample, layer.z - inset]
                .into_iter()
                .enumerate()
            {
                let held = midpoint_bytes.saturating_add(side_bytes).saturating_add(
                    occupied
                        .pieces
                        .capacity()
                        .saturating_mul(size_of::<super::OccupiedPiece>()),
                );
                let (polygons, bytes) = if plane == 1 {
                    (
                        std::mem::replace(
                            &mut midpoint.sections[ordinal],
                            MultiPolygon(Vec::new()),
                        ),
                        0,
                    )
                } else {
                    let (polygons, bytes, _) =
                        sample_model(model, z, allowance.saturating_sub(held), cancel)?;
                    (polygons, bytes)
                };
                let need = occupied.pieces.len().saturating_add(polygons.0.len());
                if need > occupied.pieces.capacity() {
                    let capacity = occupied
                        .pieces
                        .capacity()
                        .saturating_mul(2)
                        .max(4)
                        .max(need);
                    admit_section(
                        held.saturating_add(bytes).saturating_add(
                            capacity.saturating_mul(size_of::<super::OccupiedPiece>()),
                        ),
                        allowance,
                    )?;
                    occupied
                        .pieces
                        .try_reserve_exact(capacity - occupied.pieces.len())
                        .map_err(|_| section_resource())?;
                }
                side_bytes = side_bytes.saturating_add(bytes);
                for polygon in polygons.0 {
                    occupied.push(polygon, height)?;
                }
            }
        }
    }
    Ok(occupied)
}

/// A span of layers as a native-shaped span: material at its first layer's
/// sample, and an occupied enclosure streamed from the source over the
/// complete interval when supports are enabled; sampled occupancy remains for
/// ordinary classification. The first section is held over each following
/// layer of `layers` whose own sections agree with it within the profile
/// tolerance, compared against the held section so error never accumulates;
/// the span ends at the first layer that does not. A source prism thus
/// slices as one span, as its vertical faces do: walls, permanent plate
/// extents, combined bands and material constructions see one section.
#[allow(clippy::too_many_arguments)]
pub(super) fn collect_span(
    occurrences: &[&OccurrenceRecord],
    models: &BTreeMap<OccurrenceId, Arc<TolerantModel>>,
    admission: &super::preflight::Admission,
    layers: &[LayerElevation],
    index: u32,
    store: &mut SectionStore,
    allowance: usize,
    supports: bool,
    cancel: &AtomicBool,
) -> Result<native::NativeSpan, ManufacturingError> {
    let profile = spiling_geometry::tolerant::TolerantPolicy::default().profile_mm;
    let mut off_midpoints = Midpoints::default();
    let mut first = read_sample(store, models, layers, allowance, cancel)?;
    let (sections, section_bytes) = if supports {
        first.canonicalize();
        (first.sections, first.bytes)
    } else {
        admit_section(
            first.bytes.saturating_mul(2),
            allowance.saturating_sub(store.bytes()),
        )?;
        let sections = canonical_view(&first);
        let bytes = first.bytes;
        off_midpoints.push(
            first,
            allowance
                .saturating_sub(store.bytes())
                .saturating_sub(bytes),
        )?;
        (sections, bytes)
    };
    let mut held = 1;
    while held < layers.len() {
        let local = allowance
            .saturating_sub(section_bytes)
            .saturating_sub(off_midpoints.bytes());
        let mut next = read_sample(store, models, &layers[held..], local, cancel)?;
        let same = if supports {
            next.canonicalize();
            held_sections_match(&sections, &next.sections, profile)
        } else {
            admit_section(
                next.bytes.saturating_mul(2),
                local.saturating_sub(store.bytes()),
            )?;
            held_sections_match(&sections, &canonical_view(&next), profile)
        };
        if !same {
            store.remember(layers[held].sample, next, local);
            break;
        }
        if !supports {
            off_midpoints.push(
                next,
                local
                    .saturating_sub(store.bytes())
                    .saturating_add(off_midpoints.bytes()),
            )?;
        }
        held += 1;
    }
    let layers = &layers[..held];
    let (lower, upper) = (layers[0].lower_z, layers[held - 1].z);
    let mut occupied = if supports {
        ContinuousOccupied::default()
    } else {
        off_occupancy(
            occurrences,
            admission,
            layers,
            off_midpoints,
            allowance
                .saturating_sub(section_bytes)
                .saturating_sub(store.bytes()),
            cancel,
        )?
    };
    let mut material = MultiPolygon(Vec::new());
    let mut contributions = Vec::with_capacity(occurrences.len());
    let mut finished = Vec::new();
    let mut finished_bytes = 0usize;
    let mut finished_area_mm2 = 0.0;
    for (occurrence, mut placed) in occurrences.iter().zip(sections) {
        check_cancel(cancel)?;
        if supports {
            occupied.tolerant.push(super::TolerantOccupied {
                model: Arc::clone(&models[&occurrence.occurrence_id]),
                interval: [lower, upper],
            });
        }
        if placed.0.is_empty() {
            continue;
        }
        if admission
            .surface_fields
            .contains_key(&occurrence.occurrence_id)
        {
            // Finished occurrences stay their own M: preflight keeps their
            // expanded bounds clear of every other occurrence, and D = M
            // plus the finish displacement is realized per layer.
            finished_area_mm2 += geo::Area::unsigned_area(&placed);
            finished_bytes = finished_bytes
                .saturating_add(native::material_bytes(&mut placed))
                .saturating_add(native::MATERIAL_ARC_BYTES);
            finished.push((occurrence.occurrence_id, Arc::new(placed)));
            continue;
        }
        material = if material.0.is_empty() {
            placed.clone()
        } else {
            material.union(&placed)
        };
        let bytes = native::material_bytes(&mut placed);
        contributions.push((occurrence.occurrence_id, Arc::new(placed), bytes));
    }
    if contributions.len() > 1 {
        // Solids that touch (a shared face, a vertex on a face) print fused.
        // Their union can leave them as separate rings a few nanometres apart;
        // closing by half the profile tolerance fuses them and moves nothing
        // farther than that.
        let fuse = 0.5 * super::OFFSET_TOLERANCE_MM;
        let admit =
            |_: usize, _: usize, _: Option<(&MultiPolygon<f64>, &MultiPolygon<f64>)>| Ok(());
        let grown = region::offset_admitted(&material, fuse, admit, cancel)?;
        material = region::offset_admitted(&grown, -fuse, admit, cancel)?;
    }
    region::canonicalize(&mut material);
    region::check_size(&material)?;
    // Overlapping occurrences share material: the source area is the union's.
    let source_area_mm2 = geo::Area::unsigned_area(&material) + finished_area_mm2;
    let material_bytes = native::material_bytes(&mut material);
    let point_count = finished
        .iter()
        .map(|(_, placed)| region::point_count(placed))
        .fold(region::point_count(&material), usize::saturating_add);
    Ok(native::NativeSpan::tolerant(
        index + held as u32,
        Arc::new(material),
        contributions,
        finished,
        finished_bytes,
        Arc::new(occupied),
        source_area_mm2,
        point_count,
        material_bytes,
    ))
}

/// ADR012 full-height inner material: `eligible` grown by `radius` lies
/// inside the union of `occurrences`' material over the whole slab
/// `interval`, within the profile tolerance. An occurrence's material over
/// the slab is its section there minus the projection of every facet that
/// crosses the slab (`TolerantModel::slab_crossings`); flat faces within the
/// printer's Z step of either end bound the slab rather than cross it, as
/// they bound its layers. This is the guarantee a permanent plate or a
/// combined sparse band needs: no wall, overhang or roof inside its height.
pub(super) fn inner_material_admits(
    models: &BTreeMap<OccurrenceId, Arc<TolerantModel>>,
    occurrences: impl IntoIterator<Item = OccurrenceId>,
    eligible: &MultiPolygon<f64>,
    interval: [f64; 2],
    radius: f64,
    height_mm: f64,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    use geo::BoundingRect;
    let admit = |_: usize, _: usize, _: Option<(&MultiPolygon<f64>, &MultiPolygon<f64>)>| Ok(());
    let profile = spiling_geometry::tolerant::TolerantPolicy::default().profile_mm;
    let slab = [interval[0] + height_mm, interval[1] - height_mm];
    if eligible.0.is_empty() || slab[0] >= slab[1] || !radius.is_finite() {
        return Ok(false);
    }
    let required = region::offset_admitted(eligible, (radius - profile).max(0.0), admit, cancel)?;
    let Some(bounds) = required.bounding_rect() else {
        return Ok(false);
    };
    let within = [
        [bounds.min().x, bounds.min().y],
        [bounds.max().x, bounds.max().y],
    ];
    let mut inner = MultiPolygon(Vec::new());
    for occurrence_id in occurrences {
        check_cancel(cancel)?;
        let Some(model) = models.get(&occurrence_id) else {
            return Ok(false);
        };
        let section = section_at(model, 0.5 * (slab[0] + slab[1]))?;
        let crossings: Vec<Polygon<f64>> = model
            .slab_crossings(slab, within, cancel)
            .map_err(|e| super::native_error(e, "tolerant full-height inner material"))?
            .into_iter()
            .map(polygon)
            .collect();
        check_cancel(cancel)?;
        let material = section.difference(&geo::unary_union(&crossings));
        inner = if inner.0.is_empty() {
            material
        } else {
            inner.union(&material)
        };
    }
    Ok(required
        .difference(&inner)
        .0
        .iter()
        .all(|residue| geo::Area::unsigned_area(residue) <= profile * profile))
}

pub(super) fn polygon(points: Vec<[f64; 2]>) -> Polygon<f64> {
    let mut coords: Vec<geo::Coord<f64>> = points
        .into_iter()
        .map(|[x, y]| geo::Coord { x, y })
        .collect();
    if let Some(first) = coords.first().copied() {
        coords.push(first);
    }
    Polygon::new(LineString(coords), Vec::new())
}

#[derive(serde::Serialize, serde::Deserialize)]
struct ProjectedVoid {
    outer: Polygon<f64>,
    bounds: geo::Rect<f64>,
    height: spiling_geometry::ProtectedVoidHeightScope,
    roof: Option<Arc<spiling_geometry::tolerant::TolerantVoidRoof>>,
}

/// Each occurrence's all-height void projections, independent of demand.
/// The facet inventory never changes across schedule retries of one lane, so
/// a model is projected once, on its first relevant demand, and its admitted
/// outer masks are reused by every later pass.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(super) struct VoidMemo {
    models: BTreeMap<OccurrenceId, Vec<ProjectedVoid>>,
    points: usize,
    bytes: usize,
}

impl VoidMemo {
    pub(super) fn retained_bytes(&self) -> usize {
        self.bytes
    }
}

fn project_model(
    model: &TolerantModel,
    roof_budget_bytes: usize,
    cancel: &AtomicBool,
) -> Result<Vec<ProjectedVoid>, ManufacturingError> {
    use geo::{Area, BoundingRect};
    let mut projected = Vec::new();
    for void in model
        .support_voids_with_roof_budget(roof_budget_bytes, cancel)
        .map_err(|e| super::native_error(e, "tolerant all-height void projection"))?
    {
        check_cancel(cancel)?;
        let outer = polygon(void.polygon);
        let Some(bounds) = outer.bounding_rect() else {
            continue;
        };
        let bottom = MultiPolygon(vec![polygon(void.bottom)]);
        let floor_parts: Vec<_> = void
            .floor
            .into_iter()
            .map(|triangle| polygon(triangle.to_vec()))
            .collect();
        let floor = geo::unary_union(&floor_parts);
        let closed = bottom.unsigned_area() == 0.0 || bottom.difference(&floor).0.is_empty();
        let roof = void
            .roof
            .filter(|_| !closed && (floor.0.is_empty() || bottom.intersection(&floor).0.is_empty()))
            .map(Arc::new);
        let scope = if closed {
            spiling_geometry::ProtectedVoidHeightScope::Bounded {
                lower_mm: void.height_mm[0],
                upper_mm: void.height_mm[1],
            }
        } else {
            spiling_geometry::ProtectedVoidHeightScope::OpenBelow {
                rim_mm: void.height_mm[0],
                upper_mm: void.height_mm[1],
            }
        };
        projected.push(ProjectedVoid {
            outer,
            bounds,
            height: scope,
            roof,
        });
    }
    Ok(projected)
}

pub(super) fn project_protected_voids(
    models: &BTreeMap<OccurrenceId, Arc<TolerantModel>>,
    memo: &mut VoidMemo,
    demand_bounds: &[[[f64; 2]; 2]],
    retained_units: usize,
    live_bytes: usize,
    cancel: &AtomicBool,
) -> Result<super::support::ProtectedVoidSet, ManufacturingError> {
    let mut regions = Vec::new();
    let mut scopes = Vec::new();
    let mut points = 0usize;
    for (occurrence_id, model) in models {
        check_cancel(cancel)?;
        let (min, max) = model.bounds();
        let claimed_model = demand_bounds
            .iter()
            .any(|b| (0..2).all(|a| min[a] <= b[a][1] && max[a] >= b[a][0]));
        if !claimed_model {
            continue;
        }
        if !memo.models.contains_key(occurrence_id) {
            let roof_budget_bytes = crate::working_set::bytes().saturating_sub(
                live_bytes
                    .saturating_add(memo.bytes)
                    .saturating_add(points.saturating_mul(std::mem::size_of::<geo::Coord<f64>>()))
                    .saturating_add(
                        regions
                            .capacity()
                            .saturating_mul(std::mem::size_of::<Polygon<f64>>()),
                    )
                    .saturating_add(
                        scopes
                            .capacity()
                            .saturating_mul(std::mem::size_of::<super::support::VoidScope>()),
                    ),
            );
            let projected = project_model(model, roof_budget_bytes, cancel)?;
            let added_points = projected
                .iter()
                .map(|void| void.outer.exterior().0.len())
                .sum::<usize>();
            let added_bytes = projected.capacity() * std::mem::size_of::<ProjectedVoid>()
                + added_points * std::mem::size_of::<geo::Coord<f64>>()
                + projected
                    .iter()
                    .filter_map(|void| void.roof.as_ref())
                    .map(|roof| {
                        std::mem::size_of::<spiling_geometry::tolerant::TolerantVoidRoof>()
                            + 2 * std::mem::size_of::<usize>()
                            + roof.facets.capacity() * std::mem::size_of::<[[f64; 2]; 3]>()
                    })
                    .sum::<usize>();
            let memo_points = memo.points.saturating_add(added_points);
            let memo_bytes = memo.bytes.saturating_add(added_bytes);
            super::remaining_geometry_units(
                retained_units.saturating_add(memo_points),
                live_bytes.saturating_add(memo_bytes),
            )?;
            memo.points = memo_points;
            memo.bytes = memo_bytes;
            memo.models.insert(*occurrence_id, projected);
        }
        for void in &memo.models[occurrence_id] {
            check_cancel(cancel)?;
            let bounds = void.bounds;
            if !claimed_model
                || !demand_bounds.iter().any(|b| {
                    bounds.min().x <= b[0][1]
                        && bounds.max().x >= b[0][0]
                        && bounds.min().y <= b[1][1]
                        && bounds.max().y >= b[1][0]
                })
            {
                continue;
            }
            points = points
                .checked_add(void.outer.exterior().0.len())
                .ok_or_else(|| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "tolerant void projection size overflow",
                    )
                })?;
            super::remaining_geometry_units(
                retained_units + points,
                live_bytes
                    .saturating_add(memo.bytes)
                    .saturating_add(points * std::mem::size_of::<geo::Coord<f64>>())
                    .saturating_add(
                        scopes.len().max(2) * 2 * std::mem::size_of::<super::support::VoidScope>(),
                    ),
            )?;
            regions.push(void.outer.clone());
            scopes.push(super::support::VoidScope::new(
                void.height,
                void.roof.clone(),
            ));
        }
    }
    Ok(super::support::ProtectedVoidSet {
        regions: MultiPolygon(regions),
        scopes,
    })
}

/// Linear accounting for source-stage owners. Shared payloads may be charged
/// twice in the fast bound; the caller can retry a refused query using the
/// existing exact owner ledger. Do not scan the growing prefix for aliases at
/// every sample: that would add superlinear work to the section pass.
#[derive(Default)]
pub(super) struct PublishedSections {
    payload: usize,
}

impl PublishedSections {
    pub(super) fn add_span(&mut self, span: &super::SectionSpan) {
        self.payload = self
            .payload
            .saturating_add(native::MATERIAL_ARC_BYTES)
            .saturating_add(
                span.process_domains
                    .capacity()
                    .saturating_mul(size_of::<super::ProcessDomain>()),
            );
        for domain in &span.process_domains {
            self.payload = self
                .payload
                .saturating_add(domain.bytes)
                .saturating_add(native::MATERIAL_ARC_BYTES)
                .saturating_add(
                    domain
                        .coverage
                        .contributions
                        .capacity()
                        .saturating_mul(size_of::<ProcessRegionContribution>()),
                );
            for source in &domain.coverage.contributions {
                self.payload = self
                    .payload
                    .saturating_add(source.definition_id.allocated_bytes());
            }
        }
        if let Some(occupied) = &span.occupied {
            self.payload = self
                .payload
                .saturating_add(occupied.payload_bytes)
                .saturating_add(
                    occupied
                        .pieces
                        .capacity()
                        .saturating_mul(size_of::<super::OccupiedPiece>()),
                )
                .saturating_add(
                    occupied
                        .tolerant
                        .capacity()
                        .saturating_mul(size_of::<super::TolerantOccupied>()),
                );
        }
    }

    pub(super) fn add_surface(&mut self, surface: &super::NativeSurfaceSpan) {
        self.payload =
            self.payload
                .saturating_add(surface.native_bytes)
                .saturating_add(
                    surface
                        .occurrences
                        .capacity()
                        .saturating_mul(size_of::<(OccurrenceId, Arc<MultiPolygon<f64>>)>()),
                )
                .saturating_add(surface.image_islands.capacity().saturating_mul(size_of::<(
                    OccurrenceId,
                    usize,
                )>(
                )))
                .saturating_add(surface.required.len().saturating_mul(
                    16 * (size_of::<(OccurrenceId, Arc<MultiPolygon<f64>>)>() + size_of::<usize>()),
                ));
    }

    pub(super) fn bytes(
        &self,
        span_capacity: usize,
        surface_capacity: usize,
        material_bytes: usize,
    ) -> usize {
        self.payload
            .saturating_add(span_capacity.saturating_mul(size_of::<super::SectionSpan>()))
            .saturating_add(surface_capacity.saturating_mul(size_of::<super::NativeSurfaceSpan>()))
            .saturating_add(material_bytes)
    }
}
