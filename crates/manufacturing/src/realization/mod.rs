// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Pure normalized-path canonicalization and streamed process realization.
use crate::{check_cancel, error};
use spiling_contracts::manufacturing::program_coordinates::{
    project_xyz6_coordinate_mm, project_xyz6_point_mm,
};
use spiling_contracts::manufacturing::*;
use std::sync::atomic::AtomicBool;

#[derive(Clone, Copy)]
pub(crate) enum Event {
    Fan(u8),
    Travel([f64; 3], f64),
    Extrusion(f64, f64), // stationary absolute E, speed
    Deposit([f64; 3], f64, f64),
}
fn rank(kind: DepositionKind) -> u8 {
    match kind {
        DepositionKind::Support => 0,
        DepositionKind::SupportInterface => 1,
        DepositionKind::Perimeter => 2,
        DepositionKind::BottomSkin => 3,
        DepositionKind::SolidFill => 4,
        DepositionKind::SparseFill => 5,
        DepositionKind::TopSkin => 6,
        DepositionKind::Bridge => 7,
    }
}
fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1]).hypot(a[2] - b[2])
}
/// Canonical geometry traversal is stored in the normalized plan, never hidden
/// in the emitter. In-place sort/rotation/reversal requires no motion/index vector.
pub(crate) fn canonicalize(
    plan: &mut NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    plan.validate(intent)?;
    let mut position = match &intent.printer.workflow {
        JobWorkflow::DiagnosticBodyOnly => None,
        JobWorkflow::CompleteMarlin { job } => Some(project_xyz6_point_mm(job.prime_end_mm)?),
    };
    for layer in &mut plan.layers {
        check_cancel(cancel)?;
        layer.paths.sort_unstable_by(|a, b| {
            rank(a.kind)
                .cmp(&rank(b.kind))
                .then_with(|| a.points_mm[0][0].total_cmp(&b.points_mm[0][0]))
                .then_with(|| a.points_mm[0][1].total_cmp(&b.points_mm[0][1]))
                .then_with(|| a.points_mm.len().cmp(&b.points_mm.len()))
                .then_with(|| {
                    a.points_mm
                        .iter()
                        .flatten()
                        .zip(b.points_mm.iter().flatten())
                        .find_map(|(a, b)| {
                            let c = a.total_cmp(b);
                            (c != std::cmp::Ordering::Equal).then_some(c)
                        })
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });
        for path in &mut layer.paths {
            check_cancel(cancel)?;
            if let Some(from) = position {
                let n = path.points_mm.len();
                if path.points_mm[0] == path.points_mm[n - 1] {
                    let mut seam = 0;
                    for i in 1..n - 1 {
                        if distance(from, path.points_mm[i]) < distance(from, path.points_mm[seam])
                        {
                            seam = i;
                        }
                    }
                    let _ = path.points_mm.pop();
                    path.points_mm.rotate_left(seam);
                    path.points_mm.push(path.points_mm[0]);
                } else if distance(from, path.points_mm[n - 1]) < distance(from, path.points_mm[0])
                {
                    path.points_mm.reverse();
                }
            }
            position = path.points_mm.last().copied();
        }
    }
    Ok(())
}

pub(crate) fn stream(
    plan: &NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    cancel: &AtomicBool,
    mut position: Option<[f64; 3]>,
    mut extrusion: f64,
    mut emit: impl FnMut(Event) -> Result<(), ManufacturingError>,
) -> Result<(Option<[f64; 3]>, f64), ManufacturingError> {
    let recipe = &intent.recipe;
    let filament_area = std::f64::consts::PI * (recipe.filament_diameter_mm * 0.5).powi(2);
    let mut fan = None;
    let mut travels = 0u32;
    let mut process = 0u32;
    for layer in &plan.layers {
        check_cancel(cancel)?;
        for path in &layer.paths {
            check_cancel(cancel)?;
            let target = path.points_mm[0];
            let pwm = if path.kind == DepositionKind::Bridge {
                recipe.process.bridge_fan_pwm
            } else if layer.index == 0 {
                recipe.process.fan_first_layers
            } else {
                recipe.process.fan_after_first_layers
            };
            if fan != Some(pwm) {
                emit(Event::Fan(pwm))?;
                fan = Some(pwm);
            }
            let retract = match (position, &recipe.process.retraction) {
                (
                    Some(from),
                    RetractionPolicy::Enabled {
                        minimum_travel_mm, ..
                    },
                ) => distance(from, target) >= *minimum_travel_mm,
                _ => false,
            };
            if !retract {
                if travels == MAX_DEPOSITION_SEGMENTS {
                    return Err(error(
                        ManufacturingErrorCode::ResourceLimit,
                        "realized travel budget exceeded",
                    ));
                }
                travels += 1;
                emit(Event::Travel(target, recipe.travel_speed_mm_s))?;
            } else {
                let RetractionPolicy::Enabled {
                    distance_mm,
                    speed_mm_s,
                    z_hop_mm,
                    ..
                } = recipe.process.retraction
                else {
                    unreachable!()
                };
                if process > 100_000 - 2 {
                    return Err(error(
                        ManufacturingErrorCode::ResourceLimit,
                        "process event budget exceeded",
                    ));
                }
                process += 2;
                emit(Event::Extrusion(extrusion - distance_mm, speed_mm_s))?;
                let from = position.unwrap();
                let base_z = from[2].max(target[2]);
                let hop_z = base_z + z_hop_mm;
                if z_hop_mm > 0.0
                    && (!hop_z.is_finite()
                        || project_xyz6_coordinate_mm(hop_z)?
                            <= project_xyz6_coordinate_mm(base_z)?)
                {
                    return Err(error(
                        ManufacturingErrorCode::InvalidSpecification,
                        "Z hop collapses at emitted precision",
                    ));
                }
                let targets = if z_hop_mm > 0.0 {
                    [
                        Some([from[0], from[1], hop_z]),
                        Some([target[0], target[1], hop_z]),
                        Some(target),
                    ]
                } else {
                    [Some(target), None, None]
                };
                for p in targets.into_iter().flatten() {
                    if travels == MAX_DEPOSITION_SEGMENTS {
                        return Err(error(
                            ManufacturingErrorCode::ResourceLimit,
                            "realized travel budget exceeded",
                        ));
                    }
                    travels += 1;
                    emit(Event::Travel(p, recipe.travel_speed_mm_s))?;
                }
                emit(Event::Extrusion(extrusion, speed_mm_s))?;
            }
            let mut speed = recipe.print_speed_mm_s;
            let mut flow = recipe.flow_multiplier;
            if layer.index == 0 {
                speed *= recipe.process.first_layer_speed_factor;
                flow *= recipe.process.first_layer_flow_factor;
            }
            if path.kind == DepositionKind::Bridge {
                speed *= recipe.process.bridge_speed_factor;
                flow *= recipe.process.bridge_flow_factor;
            }
            let height = layer.height_mm;
            let area = ((recipe.bead_width_mm - height) * height
                + std::f64::consts::PI * (height * 0.5).powi(2))
                * flow;
            for edge in path.points_mm.windows(2) {
                check_cancel(cancel)?;
                extrusion += distance(edge[0], edge[1]) * area / filament_area;
                emit(Event::Deposit(edge[1], extrusion, speed))?;
            }
            position = path.points_mm.last().copied();
        }
    }
    Ok((position, extrusion))
}
