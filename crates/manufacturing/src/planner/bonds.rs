// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Construct open separators against actual current deposition, not skin masks.
use super::{ProcessDomain, boolean, native, region, support};
use crate::{check_cancel, error};
use geo::{Contains, Intersects, Line, Point};
use spiling_contracts::manufacturing::*;
use std::sync::atomic::AtomicBool;

pub(super) fn interface_bytes(interfaces: &Vec<ProcessInterfaceCoverage>) -> usize {
    interfaces.capacity() * std::mem::size_of::<ProcessInterfaceCoverage>()
        + interfaces
            .iter()
            .map(|i| {
                i.definition_id.allocated_bytes()
                    + i.centerlines_mm.capacity() * std::mem::size_of::<[[f64; 2]; 2]>()
                    + i.endpoint_bonds.capacity().max(i.centerlines_mm.len())
                        * std::mem::size_of::<[ProcessEndpointBond; 2]>()
            })
            .sum::<usize>()
}

pub(super) fn coverage_bytes(coverage: &PlanarRegionCoverage) -> usize {
    interface_bytes(&coverage.process_interfaces)
        + coverage.process_diaphragms.capacity() * std::mem::size_of::<ProcessDiaphragmCoverage>()
        + coverage.process_diaphragms.iter().map(ProcessDiaphragmCoverage::allocated_bytes).sum::<usize>()
        + coverage.process_regions.capacity() * std::mem::size_of::<ProcessRegionCoverage>()
        + coverage
            .process_regions
            .iter()
            .map(|r| {
                r.contributions.capacity() * std::mem::size_of::<ProcessRegionContribution>()
                    + r.contributions
                        .iter()
                        .map(|c| c.definition_id.allocated_bytes())
                        .sum::<usize>()
            })
            .sum::<usize>()
}

pub(super) fn path_bytes(paths: &Vec<DepositionPath>) -> usize {
    paths.capacity() * std::mem::size_of::<DepositionPath>()
        + paths
            .iter()
            .map(|p| p.points_mm.capacity() * std::mem::size_of::<[f64; 3]>())
            .sum::<usize>()
}

pub(super) fn construct(
    interfaces: &mut Vec<ProcessInterfaceCoverage>,
    domains: &[ProcessDomain],
    paths: &mut Vec<DepositionPath>,
    z: f64,
    lower_z: f64,
    recipe: &PlanarPrintRecipe,
    segments: &mut usize,
    mut admit: impl FnMut(usize) -> Result<(), ManufacturingError>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    if interfaces.is_empty() {
        return Ok(true);
    }
    let ribs = interfaces
        .iter()
        .map(|i| i.centerlines_mm.len())
        .sum::<usize>();
    let rib_bytes = ribs
        .checked_mul(std::mem::size_of::<DepositionPath>() + 2 * std::mem::size_of::<[f64; 3]>())
        .ok_or_else(|| resource("separator path allocation accounting overflow"))?;
    let metadata = interface_bytes(interfaces)
        .checked_add(
            paths
                .len()
                .checked_mul(2 * std::mem::size_of::<usize>())
                .ok_or_else(|| resource("bond index accounting overflow"))?,
        )
        .and_then(|bytes| bytes.checked_add(rib_bytes))
        .ok_or_else(|| resource("bond metadata accounting overflow"))?;
    admit(metadata.div_ceil(std::mem::size_of::<geo::Coord<f64>>()))?;
    paths
        .try_reserve_exact(ribs)
        .map_err(|_| resource("separator path allocation refused"))?;
    let mut walls = Vec::new();
    let mut skins = Vec::new();
    walls
        .try_reserve_exact(paths.len())
        .map_err(|_| resource("wall witness index allocation refused"))?;
    skins
        .try_reserve_exact(paths.len())
        .map_err(|_| resource("skin witness index allocation refused"))?;
    let mut owner = None;
    for interface in interfaces {
        check_cancel(cancel)?;
        let domain = domains
            .iter()
            .find(|d| {
                d.coverage.contributions.iter().any(|c| {
                    c.occurrence_id == interface.occurrence_id
                        && c.definition_id == interface.definition_id
                })
            })
            .ok_or_else(|| invalid("separator lost its captured occurrence final-D owner"))?;
        if owner != Some(interface.occurrence_id) {
            walls.clear();
            skins.clear();
            for (index, path) in paths.iter().enumerate() {
                if path.kind != DepositionKind::Perimeter
                    && path.owner.and_then(DepositionOwner::skin_kind).is_none()
                {
                    continue;
                }
                if domains.len() == 1 || owned_path(path, &domain.material, work, cancel)? {
                    if path.kind == DepositionKind::Perimeter {
                        walls.push(index);
                    } else {
                        skins.push(index);
                    }
                }
            }
            owner = Some(interface.occurrence_id);
        }
        interface.endpoint_bonds.clear();
        interface
            .endpoint_bonds
            .try_reserve_exact(interface.centerlines_mm.len())
            .map_err(|_| resource("separator witness allocation refused"))?;
        for line in &mut interface.centerlines_mm {
            let raw = line.map(|p| [p[0], p[1], z]);
            let first = terminal(
                raw[0],
                raw[1],
                paths,
                &walls,
                &skins,
                recipe.bead_width_mm,
                work,
                cancel,
            )?;
            let last = terminal(
                raw[1],
                raw[0],
                paths,
                &walls,
                &skins,
                recipe.bead_width_mm,
                work,
                cancel,
            )?;
            let (Some(first), Some(last)) = (first, last) else {
                return Ok(false);
            };
            let ends = [first.0, last.0];
            let along = 1 - usize::from(interface.axis);
            if ends[0][along] >= ends[1][along] {
                return Err(invalid(
                    "UnrealizableRegion: bounded endpoint bonds consume the open separator",
                ));
            }
            *line = ends.map(|p| [p[0], p[1]]);
            interface.endpoint_bonds.push([first.1, last.1]);
            // Only shorten an actual terminating fill row into its own domain.
            // This preserves its common phase and never adds a longitudinal track.
            let fixed = usize::from(interface.axis);
            for path in paths
                .iter_mut()
                .filter(|p| matches!(p.owner, Some(DepositionOwner::Regional { .. })))
            {
                if path.points_mm.len() != 2 || path.points_mm[0][fixed] == path.points_mm[1][fixed]
                {
                    continue;
                }
                for end in 0..2 {
                    let point = path.points_mm[end];
                    let side = if point[fixed] < interface.coordinate_mm {
                        -1.
                    } else {
                        1.
                    };
                    let boundary = interface.coordinate_mm + side * recipe.bead_width_mm * 0.5;
                    let projected = program_coordinates::project_xyz6_coordinate_mm;
                    if projected(point[fixed])? != projected(boundary)? {
                        continue;
                    }
                    boolean::charge(work, 1)?;
                    check_cancel(cancel)?;
                    if let Some(bonded) = support::inward_bond_endpoint(
                        point,
                        path.points_mm[1 - end],
                        ends,
                        (path.bead_width_mm + recipe.bead_width_mm) * 0.5,
                    )? {
                        path.points_mm[end] = bonded;
                    }
                }
            }
            *segments = segments
                .checked_add(1)
                .filter(|n| *n <= MAX_DEPOSITION_SEGMENTS as usize)
                .ok_or_else(|| resource("separator segment budget exceeded"))?;
            paths.push(DepositionPath {
                kind: DepositionKind::SeparatorWall,
                bead_width_mm: recipe.bead_width_mm,
                physical_z_mm: [lower_z, z],
                owner: None,
                points_mm: Vec::from(ends),
            });
        }
    }
    Ok(true)
}

fn terminal(
    endpoint: [f64; 3],
    other: [f64; 3],
    paths: &[DepositionPath],
    walls: &[usize],
    skins: &[usize],
    width: f64,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<([f64; 3], ProcessEndpointBond)>, ManufacturingError> {
    for &index in walls {
        for edge in paths[index].points_mm.windows(2) {
            check_cancel(cancel)?;
            boolean::charge(work, 1)?;
            if let Some(point) =
                support::inward_bond_endpoint(endpoint, other, [edge[0], edge[1]], (width + paths[index].bead_width_mm) * 0.5)?
            {
                return Ok(Some((
                    point,
                    ProcessEndpointBond::Wall {
                        wall_segment_mm: xy([edge[0], edge[1]]),
                    },
                )));
            }
        }
    }
    for &index in skins {
        let skin = &paths[index];
        if skin.points_mm.len() != 2 {
            continue;
        }
        let stroke = [skin.points_mm[0], skin.points_mm[1]];
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        let Some(point) = support::inward_bond_endpoint(endpoint, other, stroke, (width + skin.bead_width_mm) * 0.5)? else {
            continue;
        };
        for &wall in walls {
            for edge in paths[wall].points_mm.windows(2) {
                for &skin_end in &stroke {
                    check_cancel(cancel)?;
                    boolean::charge(work, 1)?;
                    if support::endpoint_bond(skin_end, [edge[0], edge[1]], (skin.bead_width_mm + paths[wall].bead_width_mm) * 0.5)? {
                        return Ok(Some((point,ProcessEndpointBond::Skin{
                            skin_kind:skin.owner.and_then(DepositionOwner::skin_kind).ok_or_else(||invalid("surrounding stroke lost immutable mandatory-skin ownership"))?,
                            process_kind:skin.kind,
                            skin_segment_mm:xy(stroke),wall_segment_mm:xy([edge[0],edge[1]]),
                        })));
                    }
                }
            }
        }
    }
    Ok(None)
}

pub(super) fn unresolved(layer: &PrintLayer) -> ManufacturingError {
    
    let endpoint = layer
        .region_coverage
        .process_interfaces
        .iter()
        .find(|i| i.endpoint_bonds.len() != i.centerlines_mm.len())
        .and_then(|i| {
            i.centerlines_mm
                .last()
                .map(|line| (line[1], i.thickness_mm))
        });
    let mut nearby = layer.paths.iter().filter(|path| {
        path.kind == DepositionKind::Bridge
            && path.points_mm.len() == 2
            && endpoint.is_some_and(|(point, width)| {
                (0..2).all(|axis| {
                    point[axis] >= path.points_mm[0][axis].min(path.points_mm[1][axis]) - width
                        && point[axis]
                            <= path.points_mm[0][axis].max(path.points_mm[1][axis]) + width
                })
            })
    });
    let nearby = [nearby.next(), nearby.next(), nearby.next()];
    invalid(&format!(
        "UnsupportedInterface: layer {} Z {} has no complete actual surrounding-material bond after mandatory-skin recovery; endpoint {endpoint:?}; nearby actual bridges {nearby:?}",
        layer.index, layer.z_mm
    ))
}

fn xy(line: [[f64; 3]; 2]) -> [[f64; 2]; 2] {
    line.map(|p| [p[0], p[1]])
}

/// Role-only recovery may legitimately turn an unchanged mandatory skin stroke
/// into Bridge. Preserve exact geometry/owner; no post-support endpoint mutation
/// can invalidate upper-layer anchors or manufacture a new bond.
pub(super) fn refresh_witness_kinds(layer: &mut PrintLayer) -> Result<(), ManufacturingError> {
    if layer
        .region_coverage
        .process_interfaces
        .iter()
        .any(|i| i.endpoint_bonds.len() != i.centerlines_mm.len())
    {
        return Err(unresolved(layer));
    }
    for bond in layer
        .region_coverage
        .process_interfaces
        .iter_mut()
        .flat_map(|i| i.endpoint_bonds.iter_mut().flatten())
    {
        if let ProcessEndpointBond::Skin {
            skin_kind,
            process_kind,
            skin_segment_mm,
            ..
        } = bond
        {
            let path=layer.paths.iter().find(|p|p.owner==Some(DepositionOwner::MandatorySkin{skin:*skin_kind}) && p.points_mm.len()==2 && {
                let segment=xy([p.points_mm[0],p.points_mm[1]]);
                segment==*skin_segment_mm || segment==[skin_segment_mm[1],skin_segment_mm[0]]
            }).ok_or_else(||invalid("UnsupportedInterface: final recovery invalidated the required continuous skin witness"))?;
            *process_kind = path.kind;
        }
    }
    Ok(())
}

/// Source attribution uses the captured occurrence operand, not a final-union
/// label. Positive interior endpoints plus no boundary intersection certify each
/// connected deposited segment remains in that exact owner, including its holes.
fn owned_path(
    path: &DepositionPath,
    material: &geo::MultiPolygon<f64>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    let count = region::point_count(material);
    if material
        .0
        .iter()
        .flat_map(|p| std::iter::once(p.exterior()).chain(p.interiors()))
        .flat_map(|r| &r.0)
        .any(|p| {
            !native::merge_predicate_coordinate(p.x) || !native::merge_predicate_coordinate(p.y)
        })
    {
        return Err(invalid(
            "bond ownership predicates are outside the conditioned planar envelope",
        ));
    }
    for p in &path.points_mm {
        boolean::charge(work, count)?;
        check_cancel(cancel)?;
        if !native::merge_predicate_coordinate(p[0]) || !native::merge_predicate_coordinate(p[1]) {
            return Err(invalid(
                "bond stroke coordinates are outside the conditioned planar envelope",
            ));
        }
        if !material.contains(&Point::new(p[0], p[1])) {
            return Ok(false);
        }
    }
    for edge in path.points_mm.windows(2) {
        let line = Line::new(
            geo::Coord {
                x: edge[0][0],
                y: edge[0][1],
            },
            geo::Coord {
                x: edge[1][0],
                y: edge[1][1],
            },
        );
        boolean::charge(work, count)?;
        for boundary in material
            .0
            .iter()
            .flat_map(|p| std::iter::once(p.exterior()).chain(p.interiors()))
            .flat_map(|r| r.0.windows(2))
        {
            check_cancel(cancel)?;
            if line.intersects(&Line::new(boundary[0], boundary[1])) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}
fn invalid(message: &str) -> ManufacturingError {
    error(ManufacturingErrorCode::InvalidSpecification, message)
}
fn resource(message: &str) -> ManufacturingError {
    error(ManufacturingErrorCode::ResourceLimit, message)
}
