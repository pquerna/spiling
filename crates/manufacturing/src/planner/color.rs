// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Construct native-owned material volumes before ordinary walls/skins/fill.
use super::{boolean, classification, forward, native, region, surface, walls};
use crate::{CompilerInput, check_cancel, error};
use geo::{
    Area, BooleanOps, Contains, Coord, Distance, Euclidean, Line, LineString, MultiPolygon, Point,
    Polygon,
};
use spiling_contracts::manufacturing::*;
use std::{mem::size_of, sync::atomic::AtomicBool};

pub(super) struct Domain {
    pub reference: MaterialDomainRef,
    pub physical_z_mm: [f64; 2],
    pub material: MultiPolygon<f64>,
}

#[derive(Default)]
pub(super) struct Construction {
    pub domains: Vec<Domain>,
    pub stored: Vec<ResolvedMaterialDomain>,
    pub frames: Vec<SurfaceColorFrame>,
    pub bytes: usize,
    pub points: usize,
    arrange_corners: bool,
}

fn unsupported(message: &str) -> ManufacturingError {
    error(
        ManufacturingErrorCode::UnsupportedGeometry,
        format!("surface construction: {message}"),
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "Material construction binds captured input, native sections, physical schedule and independent resource counters"
)]
pub(super) fn capture(
    input: &CompilerInput<'_>,
    sections: &[super::SectionSpan],
    schedule: &[super::LayerElevation],
    native_bounds: &std::collections::BTreeMap<
        spiling_contracts::geometry::OccurrenceId,
        spiling_contracts::geometry::AabbMm,
    >,
    retained_points: usize,
    owned_bytes: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Construction, ManufacturingError> {
    let JobWorkflow::FixedDualMarlin { job } = &input.intent.printer.workflow else {
        return Ok(Construction::default());
    };
    if job.surface_constructions.is_empty() {
        return Ok(Construction::default());
    }
    // One bounded arrangement on one native part, with independent regional shells.
    if input.occurrences.len() != 1 || job.surface_constructions.len() > 2 {
        return Err(unsupported(
            "requires one native part and at most two regional constructions",
        ));
    }
    let occurrence = &input.occurrences[0];
    let definition = &input.definitions[&occurrence.definition_id];
    for construction in &job.surface_constructions {
        if construction.occurrence_id != occurrence.occurrence_id
            || construction.definition_id != occurrence.definition_id
            || construction.source_hash != definition.provenance().source_hash
        {
            return Err(unsupported(
                "immutable source/definition/occurrence binding changed",
            ));
        }
        if construction.faces.len() > 2 {
            return Err(unsupported(
                "at most two adjoining planar faces per regional construction",
            ));
        }
    }
    let bounds = native_bounds[&occurrence.occurrence_id];
    let mut result = Construction {
        arrange_corners: job.surface_constructions.iter().any(|c| c.faces.len() > 1),
        ..Construction::default()
    };
    let mut bands = Vec::new();
    for (construction_index, construction) in job.surface_constructions.iter().enumerate() {
        let mut faces = Vec::new();
        for face in &construction.faces {
            check_cancel(cancel)?;
            let cert = definition
                .vertical_wall_face_with_cancel(&face.face_id, cancel)
                .map_err(|e| super::native_error(e, "surface native-face certificate"))?;
            surface::admit_extrusion(
                input,
                occurrence,
                bounds,
                &face.face_id,
                cert.min_z_mm,
                cert.max_z_mm,
                cancel,
            )?;
            let u = face
                .mask
                .as_ref()
                .map_or([0.0, cert.width_mm], |m| [m.min_u_mm, m.max_u_mm]);
            if u[0] < 0.0 || u[0] >= u[1] || u[1] > cert.width_mm {
                return Err(unsupported(
                    "finite mask exceeds its certified native-face frame",
                ));
            }
            let strip = |depth: f64| {
                let points = [
                    [u[0], 0.0],
                    [u[1], 0.0],
                    [u[1], depth],
                    [u[0], depth],
                    [u[0], 0.0],
                ];
                MultiPolygon(vec![Polygon::new(
                    LineString(
                        points
                            .into_iter()
                            .map(|[horizontal, depth]| {
                                let source = std::array::from_fn(|axis| {
                                    cert.origin_mm[axis]
                                        + cert.horizontal_direction[axis] * horizontal
                                        - cert.outward_normal[axis] * depth
                                });
                                let p = forward(&occurrence.pose, source);
                                Coord { x: p[0], y: p[1] }
                            })
                            .collect(),
                    ),
                    Vec::new(),
                )])
            };
            result.frames.push(SurfaceColorFrame {
                construction_index: construction_index as u16,
                face_id: face.face_id.clone(),
                origin_mm: cert.origin_mm,
                horizontal_direction: cert.horizontal_direction,
                outward_normal: cert.outward_normal,
                width_mm: cert.width_mm,
                source_z_mm: [cert.min_z_mm, cert.max_z_mm],
                selected_u_mm: u,
            });
            faces.push((
                strip(face.skin_depth_mm),
                strip(face.skin_depth_mm + face.backing_depth_mm),
            ));
        }
        bands.push(faces);
    }
    result.bytes = result.frames.capacity() * size_of::<SurfaceColorFrame>()
        + result
            .frames
            .iter()
            .map(|f| f.face_id.as_str().len())
            .sum::<usize>();
    let mut band_bytes =
        bands.capacity() * size_of::<Vec<(MultiPolygon<f64>, MultiPolygon<f64>)>>();
    for faces in &mut bands {
        band_bytes += faces.capacity() * size_of::<(MultiPolygon<f64>, MultiPolygon<f64>)>();
        for (shell, backed) in faces {
            band_bytes += native::material_bytes(shell) + native::material_bytes(backed);
        }
    }
    for section in sections {
        check_cancel(cancel)?;
        if section.material.0.is_empty() {
            continue;
        }
        if result.domains.len() + bands.len() + 2 > MAX_MATERIAL_DOMAINS {
            return Err(error(
                ManufacturingErrorCode::ResourceLimit,
                "surface domain count exceeds its bounded table",
            ));
        }
        // A convex four-edge cut has bounded original-edge intersections.
        // Reserve all simultaneous bands, differences and serialized copies
        // before calling the overlay kernel, alongside the native caller.
        let staging_points = region::point_count(&section.material)
            .checked_mul(64)
            .and_then(|n| n.checked_add(256 + bands.len() * 128))
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "surface construction staging overflow",
                )
            })?;
        let staging_bytes = staging_points
            .checked_mul(4 * size_of::<geo::Coord<f64>>())
            .and_then(|n| n.checked_add(16_384))
            .ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "surface construction byte overflow",
                )
            })?;
        super::remaining_geometry_units(
            retained_points + result.points + staging_points,
            owned_bytes + result.bytes + band_bytes + staging_bytes,
        )?;
        let mut shells = Vec::new();
        let mut all_shell = MultiPolygon(Vec::new());
        let mut all_backed = MultiPolygon(Vec::new());
        for (construction_index, face_bands) in bands.iter().enumerate() {
            let mut shell = MultiPolygon(Vec::new());
            let mut backed = MultiPolygon(Vec::new());
            for (face_index, (shell_strip, backed_strip)) in face_bands.iter().enumerate() {
                boolean::budget_accumulated(&section.material, shell_strip, work, cancel)?;
                let face_shell = section.material.intersection(shell_strip);
                boolean::budget_accumulated(&section.material, backed_strip, work, cancel)?;
                let face_backed = section.material.intersection(backed_strip);
                boolean::budget_accumulated(&backed, &face_backed, work, cancel)?;
                if backed.intersection(&face_backed).unsigned_area() > 0.0 {
                    let current = &job.surface_constructions[construction_index].faces[face_index];
                    if job.surface_constructions[construction_index].faces[..face_index]
                        .iter()
                        .any(|f| {
                            f.skin_depth_mm != current.skin_depth_mm
                                || f.backing_depth_mm != current.backing_depth_mm
                        })
                    {
                        return Err(unsupported(
                            "intersecting face constructions require equal physical skin and backing depths",
                        ));
                    }
                }
                boolean::budget_accumulated(&shell, &face_shell, work, cancel)?;
                shell = shell.union(&face_shell);
                boolean::budget_accumulated(&backed, &face_backed, work, cancel)?;
                backed = backed.union(&face_backed);
            }
            boolean::budget_accumulated(&all_backed, &backed, work, cancel)?;
            if all_backed.intersection(&backed).unsigned_area() > 0.0 {
                return Err(unsupported(
                    "independent regional constructions overlap in positive native material",
                ));
            }
            boolean::budget_accumulated(&all_shell, &shell, work, cancel)?;
            all_shell = all_shell.union(&shell);
            boolean::budget_accumulated(&all_backed, &backed, work, cancel)?;
            all_backed = all_backed.union(&backed);
            shells.push((
                MaterialDomainRef {
                    construction_index: construction_index as u16,
                    role: MaterialConstructionRole::Shell,
                },
                shell,
            ));
        }
        boolean::budget_accumulated(&all_backed, &all_shell, work, cancel)?;
        let backing = all_backed.difference(&all_shell);
        boolean::budget_accumulated(&section.material, &all_backed, work, cancel)?;
        let core = section.material.difference(&all_backed);
        let physical_z_mm = [
            schedule[section.start as usize].lower_z,
            schedule[section.end as usize - 1].z,
        ];
        shells.extend([
            (
                MaterialDomainRef {
                    construction_index: 0,
                    role: MaterialConstructionRole::Backing,
                },
                backing,
            ),
            (
                MaterialDomainRef {
                    construction_index: 0,
                    role: MaterialConstructionRole::Core,
                },
                core,
            ),
        ]);
        for (reference, mut material) in shells {
            region::check_size(&material)?;
            if material.0.len() > MAX_MATERIAL_DOMAIN_POLYGONS
                || material.0.iter().any(|p| {
                    p.interiors().len() > MAX_MATERIAL_DOMAIN_HOLES
                        || std::iter::once(p.exterior())
                            .chain(p.interiors())
                            .any(|r| r.0.len() > MAX_MATERIAL_DOMAIN_RING_POINTS)
                })
            {
                return Err(error(
                    ManufacturingErrorCode::ResourceLimit,
                    "surface domain ring/polygon budget exceeded",
                ));
            }
            region::canonicalize(&mut material);
            if material.0.is_empty() || material.unsigned_area() <= 0.0 {
                return Err(unsupported(
                    "skin/backing consumes the core or has no positive native material",
                ));
            }
            result.points += region::point_count(&material);
            result.bytes += native::material_bytes(&mut material) * 2;
            if result.bytes > MAX_MANUFACTURING_RETAINED_BYTES as usize / 4
                || result.points > 65_536
            {
                return Err(error(
                    ManufacturingErrorCode::ResourceLimit,
                    "surface construction retained domain budget exceeded",
                ));
            }
            super::remaining_geometry_units(
                retained_points + result.points,
                owned_bytes + result.bytes + band_bytes,
            )?;
            result.stored.push(ResolvedMaterialDomain {
                reference,
                physical_z_mm,
                polygons: material
                    .0
                    .iter()
                    .map(|p| MaterialDomainPolygon {
                        exterior_mm: p.exterior().0.iter().map(|p| [p.x, p.y]).collect(),
                        holes_mm: p
                            .interiors()
                            .iter()
                            .map(|r| r.0.iter().map(|p| [p.x, p.y]).collect())
                            .collect(),
                    })
                    .collect(),
            });
            result.domains.push(Domain {
                reference,
                physical_z_mm,
                material,
            });
        }
    }
    result.bytes += result.domains.capacity() * size_of::<Domain>()
        + result.stored.capacity() * size_of::<ResolvedMaterialDomain>();
    Ok(result)
}

impl Construction {
    fn active(&self, interval: [f64; 2]) -> impl Iterator<Item = &Domain> {
        self.domains
            .iter()
            .filter(move |d| interval[0] >= d.physical_z_mm[0] && interval[1] <= d.physical_z_mm[1])
    }

    pub(super) fn walls(
        &self,
        request: walls::WallRequest<'_>,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<walls::WallPlan, ManufacturingError> {
        if self.domains.is_empty() {
            return walls::plan_section_walls(request, work, cancel);
        }
        let mut paths = Vec::new();
        let mut fill = MultiPolygon(Vec::new());
        let mut retained_points = 0;
        let mut retained_bytes = 0;
        for domain in self.active([request.lower_z, request.upper_z]) {
            check_cancel(cancel)?;
            let mut cells = if self.arrange_corners {
                corner_cells(
                    &domain.material,
                    request.retained_points + retained_points,
                    request.owned_bytes
                        + retained_bytes
                        + paths.capacity() * size_of::<DepositionPath>()
                        + fill.0.capacity() * size_of::<Polygon<f64>>(),
                    work,
                    cancel,
                )?
            } else {
                None
            };
            let cell_points = cells
                .as_ref()
                .map_or(0, |c| c.iter().map(region::point_count).sum());
            let cell_bytes = cells.as_mut().map_or(0, |c| {
                c.capacity() * size_of::<MultiPolygon<f64>>()
                    + c.iter_mut().map(native::material_bytes).sum::<usize>()
            });
            let materials = cells
                .as_deref()
                .unwrap_or(std::slice::from_ref(&domain.material));
            for material in materials {
                let required = region::RequiredMaterial::from_section(material);
                let mut layout = walls::plan_material_domain_walls(
                    walls::WallRequest {
                        material,
                        required: &required,
                        owned_bytes: request.owned_bytes
                            + retained_bytes
                            + cell_bytes
                            + paths.capacity() * size_of::<DepositionPath>()
                            + fill.0.capacity() * size_of::<Polygon<f64>>(),
                        retained_points: request.retained_points + retained_points + cell_points,
                        ..request
                    },
                    work,
                    cancel,
                )
                .map_err(|mut e| {
                    e.message =
                        format!("{:?} material domain: {}", domain.reference.role, e.message);
                    e
                })?;
                for path in &mut layout.paths {
                    path.material_domain = Some(domain.reference);
                }
                retained_points += layout.retained_points;
                retained_bytes += layout.retained_bytes;
                let merged_bytes = request.owned_bytes
                    + cell_bytes
                    + retained_bytes
                    + paths.capacity() * size_of::<DepositionPath>()
                    + fill.0.capacity() * size_of::<Polygon<f64>>();
                super::remaining_geometry_units(
                    request.retained_points + retained_points + cell_points,
                    merged_bytes,
                )?;
                // Vector growth may retain both old and replacement metadata
                // alongside the still-owned source layout until extension ends.
                let metadata_growth = (paths.len() + layout.paths.len())
                    * size_of::<DepositionPath>()
                    + (fill.0.len() + layout.fill.0.len()) * size_of::<Polygon<f64>>();
                super::remaining_geometry_units(
                    request.retained_points + retained_points + cell_points,
                    merged_bytes + metadata_growth,
                )?;
                paths.try_reserve_exact(layout.paths.len()).map_err(|_| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "material wall path allocation refused",
                    )
                })?;
                fill.0.try_reserve_exact(layout.fill.0.len()).map_err(|_| {
                    error(
                        ManufacturingErrorCode::ResourceLimit,
                        "material fill allocation refused",
                    )
                })?;
                paths.extend(layout.paths);
                fill.0.extend(layout.fill.0);
            }
        }
        if paths.is_empty() {
            return Err(unsupported(
                "no actual material-domain wall construction at this interval",
            ));
        }
        let wall_area_mm2 = request.material.unsigned_area() - fill.unsigned_area();
        Ok(walls::WallPlan {
            retained_bytes: retained_bytes
                + paths.capacity() * size_of::<DepositionPath>()
                + fill.0.capacity() * size_of::<Polygon<f64>>(),
            paths,
            fill,
            correspondence: Vec::new(),
            retained_points,
            section_area_mm2: request.material.unsigned_area(),
            wall_area_mm2,
            protected_hole_area_mm2: request
                .material
                .0
                .iter()
                .flat_map(|p| p.interiors())
                .map(|r| Polygon::new(r.clone(), Vec::new()).unsigned_area())
                .sum(),
        })
    }

    pub(super) fn classify(
        &self,
        classified: &mut classification::ClassifiedRegions,
        interval: [f64; 2],
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        if self.domains.is_empty() {
            return Ok(());
        }
        let mut result = Vec::new();
        for interior in &classified.interiors {
            for domain in self.active(interval) {
                boolean::budget_accumulated(&interior.domain, &domain.material, work, cancel)?;
                let material = interior.domain.intersection(&domain.material);
                let area = material.unsigned_area();
                if material.0.is_empty() {
                    continue;
                }
                let fraction = if domain.reference.role == MaterialConstructionRole::Core {
                    interior.coverage.fraction
                } else {
                    1.0
                };
                result.push(classification::InteriorRegion {
                    domain: material,
                    coverage: ProcessRegionCoverage {
                        material_domain: Some(domain.reference),
                        area_mm2: area,
                        fraction,
                        contributions: interior.coverage.contributions.clone(),
                    },
                    frame_origin_mm: interior.frame_origin_mm,
                });
            }
        }
        classified.coverage.process_regions = result.iter().map(|i| i.coverage.clone()).collect();
        classified.coverage.solid_interior_area_mm2 = result
            .iter()
            .filter(|i| i.coverage.fraction == 1.0)
            .map(|i| i.coverage.area_mm2)
            .sum();
        classified.coverage.sparse_interior_area_mm2 = result
            .iter()
            .filter(|i| i.coverage.fraction < 1.0)
            .map(|i| i.coverage.area_mm2)
            .sum();
        classified.coverage.intentional_void_area_mm2 = result
            .iter()
            .map(|i| i.coverage.area_mm2 * (1.0 - i.coverage.fraction))
            .sum();
        classified.interiors = result;
        Ok(())
    }

    pub(super) fn attribute(
        &self,
        layers: &mut [PrintLayer],
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        if self.domains.is_empty() {
            return Ok(());
        }
        for layer in layers {
            for path in &mut layer.paths {
                let mut owner = None;
                for domain in self.active(path.physical_z_mm) {
                    if contains(&domain.material, path, work, cancel)?
                        && owner.replace(domain.reference).is_some()
                    {
                        return Err(unsupported(
                            "complete bead has ambiguous material ownership",
                        ));
                    }
                }
                path.material_domain = Some(owner.ok_or_else(|| {
                    unsupported("complete bead crosses a material boundary or protected void")
                })?);
            }
        }
        Ok(())
    }
}

fn contains(
    material: &MultiPolygon<f64>,
    path: &DepositionPath,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    for edge in path.checked_edges()? {
        let projected = [
            program_coordinates::project_xyz6_point_mm(edge.points_mm[0])?,
            program_coordinates::project_xyz6_point_mm(edge.points_mm[1])?,
        ];
        let segment = Line::new(
            Coord {
                x: projected[0][0],
                y: projected[0][1],
            },
            Coord {
                x: projected[1][0],
                y: projected[1][1],
            },
        );
        if !material.contains(&Point::from(segment.start))
            || !material.contains(&Point::from(segment.end))
        {
            return Ok(false);
        }
        for boundary in material
            .0
            .iter()
            .flat_map(|p| std::iter::once(p.exterior()).chain(p.interiors()))
            .flat_map(|r| r.lines())
        {
            check_cancel(cancel)?;
            boolean::charge(work, 1)?;
            let distance = Euclidean.distance(&segment, &boundary);
            // Existing native/offset approximation is reported independently;
            // assignment never uses a center-point-only ownership decision.
            if distance + super::OFFSET_TOLERANCE_MM
                < edge.width_mm * 0.5 + program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM
            {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Shared source-edge ledgers partition the complete domain into adjacent X
/// slabs. Every original edge and paired cut is retained; no positive sliver is
/// discarded. The same wall planner consumes each cell, retaining its original
/// material-domain owner for full-bead attribution and independent replay.
fn corner_cells(
    material: &MultiPolygon<f64>,
    retained_points: usize,
    owned_bytes: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<Vec<MultiPolygon<f64>>>, ManufacturingError> {
    let mut orthogonal_corner = false;
    for polygon in &material.0 {
        check_cancel(cancel)?;
        let ring = &polygon.exterior().0;
        boolean::charge(work, ring.len() * 2)?;
        // An orthogonal simple exterior with more than four turns has a
        // reentrant corner. Collinear source subdivisions do not add turns.
        orthogonal_corner |= ring
            .windows(2)
            .all(|e| e[0].x == e[1].x || e[0].y == e[1].y)
            && ring
                .windows(3)
                .filter(|e| (e[0].x == e[1].x) != (e[1].x == e[2].x))
                .count()
                > 3;
    }
    if !orthogonal_corner {
        return Ok(None);
    }
    let exterior_points = material
        .0
        .iter()
        .map(|p| p.exterior().0.len())
        .sum::<usize>();
    super::remaining_geometry_units(
        retained_points + exterior_points,
        owned_bytes + exterior_points * size_of::<f64>(),
    )?;
    boolean::charge(
        work,
        exterior_points.saturating_mul(exterior_points.ilog2() as usize + 2),
    )?;
    let mut cuts = Vec::new();
    cuts.try_reserve_exact(exterior_points).map_err(|_| {
        error(
            ManufacturingErrorCode::ResourceLimit,
            "corner boundary allocation refused",
        )
    })?;
    cuts.extend(material.0.iter().flat_map(|p| &p.exterior().0).map(|p| p.x));
    cuts.sort_by(f64::total_cmp);
    cuts.dedup();
    if cuts.len() > 32 {
        return Err(error(
            ManufacturingErrorCode::ResourceLimit,
            "shared-corner cell partition exceeds 32 source X boundaries",
        ));
    }
    let mut cells = Vec::new();
    let mut points = 0;
    let mut bytes = cuts.capacity() * size_of::<f64>();
    for cut in cuts.windows(2) {
        let [_, mut middle, _] = super::partition::split(
            material,
            0,
            [cut[0], cut[1]],
            |scratch, scratch_bytes| {
                super::remaining_geometry_units(
                    retained_points + points + scratch,
                    owned_bytes
                        + bytes
                        + cells.capacity() * size_of::<MultiPolygon<f64>>()
                        + scratch_bytes,
                )
                .map(|_| ())
            },
            work,
            cancel,
        )?;
        if middle.0.is_empty() {
            continue;
        }
        if middle
            .0
            .iter()
            .any(|polygon| polygon.unsigned_area() <= 0.0)
        {
            return Err(unsupported(
                "shared-corner partition contains a nonpositive cell",
            ));
        }
        points += region::point_count(&middle);
        bytes += native::material_bytes(&mut middle);
        let metadata_peak = (cells.capacity() + cells.len() + 1) * size_of::<MultiPolygon<f64>>();
        super::remaining_geometry_units(
            retained_points + points,
            owned_bytes + bytes + metadata_peak,
        )?;
        cells.try_reserve_exact(1).map_err(|_| {
            error(
                ManufacturingErrorCode::ResourceLimit,
                "corner cell allocation refused",
            )
        })?;
        cells.push(middle);
        super::remaining_geometry_units(
            retained_points + points,
            owned_bytes + bytes + cells.capacity() * size_of::<MultiPolygon<f64>>(),
        )?;
    }
    Ok(Some(cells))
}
