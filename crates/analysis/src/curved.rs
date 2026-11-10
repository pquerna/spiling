// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use crate::{
    Budget,
    mesh::{Facet, Hex, Mesh},
};
use spiling_contracts::analysis::{fastener::*, mechanics::*};
use std::{
    collections::BTreeMap,
    f64::consts::{PI, TAU},
};

fn partition_point(start: f64, end: f64, step: usize, count: usize) -> f64 {
    if step == 0 {
        start
    } else if step == count {
        end
    } else {
        start + (end - start) * step as f64 / count as f64
    }
}

pub(crate) fn build(
    plate: &spiling_geometry::BoredPlate,
    settings: &AnalysisRunSettings,
    rotation: [[f64; 3]; 3],
    budget: &Budget,
) -> Result<Mesh, String> {
    let bounds = plate.outer_xy_mm();
    let center = plate.center_mm();
    let height = plate.height_mm();
    let wall = settings.recipe.perimeter_count as f64 * settings.recipe.bead_width_mm;
    let skin = settings.recipe.top_skin_thickness_mm;
    if wall <= 0.0
        || skin <= 0.0
        || skin != settings.recipe.bottom_skin_thickness_mm
        || 2.0 * skin >= height[1] - height[0]
    {
        return Err("UnsupportedModel: bored-plate construction requires positive equal skins and a remaining core".into());
    }
    let inset = [
        bounds[0].map(|value| value + wall),
        bounds[1].map(|value| value - wall),
    ];
    let inner_wall = plate.radius_mm() + wall;
    if (0..2).any(|axis| {
        center[axis] - inner_wall <= inset[0][axis] || center[axis] + inner_wall >= inset[1][axis]
    }) {
        return Err("UnsupportedModel: wall bands meet around the hole; this partition cannot invent or drop a core ligament".into());
    }
    let radial_hit = |angle: f64, rectangle: [[f64; 2]; 2]| {
        let direction = [angle.cos(), angle.sin()];
        (0..2)
            .filter(|&axis| direction[axis].abs() > 1e-14)
            .map(|axis| {
                (rectangle[usize::from(direction[axis] > 0.0)][axis] - center[axis])
                    / direction[axis]
            })
            .fold(f64::INFINITY, f64::min)
    };
    let mut angles = vec![0.0, TAU];
    for rectangle in [bounds, inset] {
        for x_side in 0..2 {
            for y_side in 0..2 {
                let angle = (rectangle[y_side][1] - center[1])
                    .atan2(rectangle[x_side][0] - center[0])
                    .rem_euclid(TAU);
                angles.push(angle);
            }
        }
    }
    angles.sort_by(f64::total_cmp);
    angles.dedup_by(|first, second| (*first - *second).abs() < 1e-12);
    let maximum_radius = [0, 1]
        .into_iter()
        .flat_map(|x_side| {
            [0, 1].map(move |y_side| {
                (bounds[x_side][0] - center[0]).hypot(bounds[y_side][1] - center[1])
            })
        })
        .fold(0.0, f64::max);
    let maximum_angle = (PI / 8.0).min(settings.maximum_span_mm / maximum_radius);
    let mut refined = vec![0.0];
    for interval in angles.windows(2) {
        let count = ((interval[1] - interval[0]) / maximum_angle).ceil() as usize;
        if count > 256 {
            return Err("ResourceLimit: angular mesh subdivision".into());
        }
        for step in 1..=count {
            refined.push(interval[0] + (interval[1] - interval[0]) * step as f64 / count as f64);
        }
    }
    refined.reverse();
    let angular_cells = refined.len() - 1;
    let maximum_core = (0..angular_cells)
        .map(|index| radial_hit(refined[index].midpoint(refined[index + 1]), inset) - inner_wall)
        .fold(0.0, f64::max);
    let core_cells = (maximum_core / settings.maximum_span_mm).ceil().max(1.0) as usize;
    let inner_wall_cells = (wall / settings.maximum_span_mm).ceil().max(1.0) as usize;
    let outer_wall_cells = inner_wall_cells;
    let radial_cells = inner_wall_cells + core_cells + outer_wall_cells;
    let mut z_cuts = vec![height[0]];
    for interval in [
        [height[0], height[0] + skin],
        [height[0] + skin, height[1] - skin],
        [height[1] - skin, height[1]],
    ] {
        let count = ((interval[1] - interval[0]) / settings.maximum_span_mm)
            .ceil()
            .max(1.0) as usize;
        for step in 1..=count {
            z_cuts.push(interval[0] + (interval[1] - interval[0]) * step as f64 / count as f64);
        }
    }
    let z_cells = z_cuts.len() - 1;
    if angular_cells
        .saturating_mul(radial_cells)
        .saturating_mul(z_cells)
        > MAX_MECHANICAL_ELEMENTS
    {
        return Err("ResourceLimit: complete curved mesh exceeds element admission; choose a larger analysis span".into());
    }
    let mut nodes = Vec::new();
    let mut node_map = BTreeMap::new();
    let mut elements = Vec::new();
    let mut facets = Vec::new();
    for z_cell in 0..z_cells {
        for radial in 0..radial_cells {
            for angular in 0..angular_cells {
                budget.check()?;
                let angles = [
                    refined[angular],
                    refined[angular].midpoint(refined[angular + 1]),
                    refined[angular + 1],
                ];
                let midpoint_weight = 0.5 * (1.0 + (0.5 * (angles[0] - angles[2])).cos());
                let radial_endpoints = |angle: f64| {
                    let inside = radial_hit(angle, inset);
                    let outside = radial_hit(angle, bounds);
                    if radial < inner_wall_cells {
                        [
                            partition_point(
                                plate.radius_mm(),
                                inner_wall,
                                radial,
                                inner_wall_cells,
                            ),
                            partition_point(
                                plate.radius_mm(),
                                inner_wall,
                                radial + 1,
                                inner_wall_cells,
                            ),
                        ]
                    } else if radial < inner_wall_cells + core_cells {
                        let index = radial - inner_wall_cells;
                        [
                            partition_point(inner_wall, inside, index, core_cells),
                            partition_point(inner_wall, inside, index + 1, core_cells),
                        ]
                    } else {
                        let index = radial - inner_wall_cells - core_cells;
                        [
                            partition_point(inside, outside, index, outer_wall_cells),
                            partition_point(inside, outside, index + 1, outer_wall_cells),
                        ]
                    }
                };
                let mut element_nodes = [0; 27];
                let mut positions = [[0.0; 3]; 27];
                for local_z in 0..3 {
                    let z = [
                        z_cuts[z_cell],
                        z_cuts[z_cell].midpoint(z_cuts[z_cell + 1]),
                        z_cuts[z_cell + 1],
                    ][local_z];
                    for (local_angle, &angular_point) in angles.iter().enumerate() {
                        let global_angle = (2 * angular + local_angle) % (2 * angular_cells);
                        let angle = if global_angle == 0 {
                            0.0
                        } else {
                            angular_point
                        };
                        let radii = radial_endpoints(angle);
                        let ends = radii.map(|radius| {
                            [
                                center[0] + radius * angle.cos(),
                                center[1] + radius * angle.sin(),
                            ]
                        });
                        for local_radius in 0..3 {
                            let local = local_angle + 3 * local_radius + 9 * local_z;
                            let xy = match local_radius {
                                0 => ends[0],
                                1 => std::array::from_fn(|axis| {
                                    ends[0][axis].midpoint(ends[1][axis])
                                }),
                                _ => ends[1],
                            };
                            let point = [xy[0], xy[1], z];
                            let key = (
                                global_angle,
                                2 * radial + local_radius,
                                2 * z_cell + local_z,
                            );
                            let node = *node_map.entry(key).or_insert_with(|| {
                                nodes.push(point);
                                nodes.len() - 1
                            });
                            if nodes[node] != point {
                                return Err("UnresolvedModel: adjoining curved cells disagree on a shared node".into());
                            }
                            element_nodes[local] = node;
                            positions[local] = point;
                        }
                    }
                }
                crate::mapped::validate_extruded_cell(&positions, midpoint_weight)?;
                let minimum: [f64; 3] = std::array::from_fn(|axis| {
                    positions
                        .iter()
                        .map(|point| point[axis])
                        .fold(f64::INFINITY, f64::min)
                });
                let maximum: [f64; 3] = std::array::from_fn(|axis| {
                    positions
                        .iter()
                        .map(|point| point[axis])
                        .fold(f64::NEG_INFINITY, f64::max)
                });
                let z_center = z_cuts[z_cell].midpoint(z_cuts[z_cell + 1]);
                let is_wall = radial < inner_wall_cells || radial >= inner_wall_cells + core_cells;
                let role = if z_center < height[0] + skin || z_center > height[1] - skin {
                    1
                } else if is_wall {
                    0
                } else {
                    2
                };
                let element = elements.len();
                elements.push(Hex {
                    midpoint_weight,
                    nodes: element_nodes,
                    lower: minimum,
                    size: std::array::from_fn(|axis| maximum[axis] - minimum[axis]),
                    role,
                    component: 0,
                });
                if radial == 0 {
                    facets.push(Facet {
                        element,
                        axis: 1,
                        side: 0,
                        face: None,
                    });
                }
                if radial + 1 == radial_cells {
                    let outer = positions[7];
                    let owner = (0..4).find(|side| {
                        let axis = side/2;
                        (outer[axis] - bounds[side%2][axis]).abs() < 1e-7
                    }).ok_or("UnmappedLoad: curved outer facet has no complete native rectangle owner")?;
                    facets.push(Facet {
                        element,
                        axis: 1,
                        side: 1,
                        face: Some(plate.outer_faces()[owner].clone()),
                    });
                }
                if z_cell == 0 {
                    facets.push(Facet {
                        element,
                        axis: 2,
                        side: 0,
                        face: Some(plate.cap_faces()[0].clone()),
                    });
                }
                if z_cell + 1 == z_cells {
                    facets.push(Facet {
                        element,
                        axis: 2,
                        side: 1,
                        face: Some(plate.cap_faces()[1].clone()),
                    });
                }
            }
        }
    }
    if 3 * nodes.len() > MAX_MECHANICAL_DOFS {
        return Err("ResourceLimit: complete curved mesh exceeds DOF admission".into());
    }
    let profile = AnalysisBoreProfile {
        hole: AnalysisHole {
            bore_faces: plate.bore_faces().to_vec(),
            cap_faces: plate.cap_faces().clone(),
            center_mm: [center[0], center[1], height[0]],
            radius_mm: plate.radius_mm(),
            depth_mm: height[1] - height[0],
        },
        outer_xy_mm: [[bounds[0][0], bounds[1][0]], [bounds[0][1], bounds[1][1]]],
        outer_faces: plate.outer_faces().clone(),
    };
    profile.validate().map_err(|error| error.to_string())?;
    Ok(Mesh {
        geometry: AnalysisMeshGeometry::RationalBoredPlate { profile },
        nodes,
        elements,
        facets,
        components: 1,
        service_to_print: rotation,
        layer_axis: 2,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radial_interfaces_keep_exact_canonical_endpoints() {
        assert_ne!(1.6 + (7.2 - 1.6), 7.2);
        for count in [1, 2, 3, 7, 11] {
            assert_eq!(partition_point(1.6, 7.2, 0, count), 1.6);
            assert_eq!(partition_point(1.6, 7.2, count, count), 7.2);
            assert_eq!(
                partition_point(1.6, 7.2, count, count),
                partition_point(7.2, 8.0, 0, count)
            );
        }
    }
}
