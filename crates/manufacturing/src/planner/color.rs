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
}

fn unsupported(message: &str) -> ManufacturingError {
    error(
        ManufacturingErrorCode::UnsupportedGeometry,
        format!("surface construction: {message}"),
    )
}

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
        if construction.faces.len() != 1 {
            return Err(unsupported(
                "shared-corner material construction remains unqualified; select one planar face per region",
            ));
        }
    }
    let bounds = native_bounds[&occurrence.occurrence_id];
    let mut result = Construction::default();
    let mut bands = Vec::new();
    for (construction_index, construction) in job.surface_constructions.iter().enumerate() {
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
            bands.push((
                strip(face.skin_depth_mm),
                strip(face.skin_depth_mm + face.backing_depth_mm),
            ));
        }
    }
    result.bytes = result.frames.capacity() * size_of::<SurfaceColorFrame>()
        + result
            .frames
            .iter()
            .map(|f| f.face_id.as_str().len())
            .sum::<usize>();
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
            owned_bytes + result.bytes + staging_bytes,
        )?;
        let mut shells = Vec::new();
        let mut all_shell = MultiPolygon(Vec::new());
        let mut all_backed = MultiPolygon(Vec::new());
        for (construction_index, (shell_strip, backed_strip)) in bands.iter().enumerate() {
            boolean::budget_accumulated(&section.material, shell_strip, work, cancel)?;
            let shell = section.material.intersection(shell_strip);
            boolean::budget_accumulated(&section.material, backed_strip, work, cancel)?;
            let backed = section.material.intersection(backed_strip);
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
                owned_bytes + result.bytes,
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
            let required = region::RequiredMaterial::from_section(&domain.material);
            let mut layout = walls::plan_material_domain_walls(
                walls::WallRequest {
                    material: &domain.material,
                    required: &required,
                    owned_bytes: request.owned_bytes + retained_bytes,
                    retained_points: request.retained_points + retained_points,
                    ..request
                },
                work,
                cancel,
            )
            .map_err(|mut e| {
                e.message = format!("{:?} material domain: {}", domain.reference.role, e.message);
                e
            })?;
            for path in &mut layout.paths {
                path.material_domain = Some(domain.reference);
            }
            retained_points += layout.retained_points;
            retained_bytes += layout.retained_bytes;
            let merged_bytes = request.owned_bytes
                + retained_bytes
                + paths.capacity() * size_of::<DepositionPath>()
                + fill.0.capacity() * size_of::<Polygon<f64>>();
            super::remaining_geometry_units(
                request.retained_points + retained_points,
                merged_bytes,
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
                    if contains(&domain.material, path, work, cancel)? {
                        if owner.replace(domain.reference).is_some() {
                            return Err(unsupported(
                                "complete bead has ambiguous material ownership",
                            ));
                        }
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
