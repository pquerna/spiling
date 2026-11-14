// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! The declared deposition speed rule. Realization applies it and the
//! independent verifier recomputes it as policy, the way both consume
//! [`super::hatch_alternation_keys`]; the verifier still decodes and bounds
//! actual feed and volumetric flow from the emitted bytes itself.
//!
//! 1. Base speed: a role speed (outer/inner wall, sparse, solid/bottom and
//!    covered top skin, exposed top skin,
//!    support, bridge), capped by the first-layer wall or infill speed on layer 0 and by the
//!    recipe's material print speed; without role speeds, the uniform print
//!    speed times the first-layer/bridge speed factors.
//! 2. Flow ceiling (single tool): an edge whose bead would exceed the
//!    effective volumetric flow limit slows to it, with a 1e-6 relative margin
//!    so rounded commands stay inside it. This is reported, never refused.
//! 3. Cooling (single tool): a layer whose deposition at those speeds takes
//!    less than the minimum layer time slows uniformly until it takes that
//!    long, but never below the minimum print speed (nor above an edge's own
//!    speed). Layer time counts deposition chords at their speeds only.
//!
//! All of these are software declarations, not measured cooling or flow.
use super::*;

/// The role a path's speed and fan follow. With role speeds, a Bridge owned
/// by a sparse process region (fraction below one) is sparse infill spanning
/// the rows beneath it and takes the sparse speed and ordinary fan; every
/// other path keeps its own role. Flow factors and bridge construction
/// checks always use the path's own role.
pub fn speed_role(
    recipe: &PlanarPrintRecipe,
    layer: &PrintLayer,
    kind: DepositionKind,
    owner: Option<DepositionOwner>,
) -> DepositionKind {
    match (kind, owner) {
        (DepositionKind::Bridge, Some(DepositionOwner::Regional { region_index }))
            if recipe.role_speeds.is_some()
                && layer
                    .region_coverage
                    .process_regions
                    .get(usize::from(region_index))
                    .is_some_and(|region| region.fraction < 1.0) =>
        {
            DepositionKind::SparseFill
        }
        _ => kind,
    }
}

/// Relative margin under the volumetric flow ceiling for a flow-limited edge.
pub const FLOW_CEILING_MARGIN: f64 = 1e-6;

/// Nominal bead cross-section `(w-h)h + pi(h/2)^2`, before flow factors.
pub fn bead_area_mm2(width_mm: f64, height_mm: f64) -> f64 {
    (width_mm - height_mm) * height_mm + std::f64::consts::PI * (height_mm * 0.5).powi(2)
}

/// Flow multiplier and first-layer/bridge flow factors applied exactly once.
pub fn flow_factor(
    process: &CapturedDepositionProcess<'_>,
    kind: DepositionKind,
    layer: u32,
) -> f64 {
    let mut flow = process.flow_multiplier;
    if layer == 0 {
        flow *= process.policy.first_layer_flow_factor;
    }
    if kind == DepositionKind::Bridge {
        flow *= process.policy.bridge_flow_factor;
    }
    flow
}

/// The role's speed before flow and cooling limits.
pub fn base_speed_mm_s(
    recipe: &PlanarPrintRecipe,
    process: &CapturedDepositionProcess<'_>,
    kind: DepositionKind,
    exposed: bool,
    layer: u32,
) -> f64 {
    let Some(speeds) = &recipe.role_speeds else {
        let mut speed = process.speed_mm_s;
        if layer == 0 {
            speed *= process.policy.first_layer_speed_factor;
        }
        if kind == DepositionKind::Bridge {
            speed *= process.policy.bridge_speed_factor;
        }
        return speed;
    };
    let mut speed = match kind {
        DepositionKind::Perimeter if exposed => speeds.outer_wall_mm_s,
        DepositionKind::Perimeter | DepositionKind::SeparatorWall => speeds.inner_wall_mm_s,
        DepositionKind::SparseFill => speeds.sparse_infill_mm_s,
        DepositionKind::SolidFill | DepositionKind::BottomSkin => speeds.solid_infill_mm_s,
        // Only the exposed top surface takes the top speed; top skin under
        // the next layer's material is internal solid.
        DepositionKind::TopSkin if exposed => speeds.top_skin_mm_s,
        DepositionKind::TopSkin => speeds.solid_infill_mm_s,
        DepositionKind::Bridge => speeds.bridge_mm_s,
        DepositionKind::Support | DepositionKind::SupportInterface => speeds.support_mm_s,
    };
    if layer == 0 {
        speed = speed.min(match kind {
            DepositionKind::Perimeter | DepositionKind::SeparatorWall => speeds.first_layer_mm_s,
            _ => speeds.first_layer_infill_mm_s,
        });
    }
    speed.min(process.speed_mm_s)
}

/// The edge speed under the volumetric flow ceiling, and whether the ceiling
/// lowered it. Routed (fixed-dual) jobs keep their admitted base speed.
pub fn flow_limited_speed_mm_s(
    base_mm_s: f64,
    width_mm: f64,
    height_mm: f64,
    flow_factor: f64,
    max_volumetric_flow_mm3_s: f64,
    routed: bool,
) -> (f64, bool) {
    if routed {
        return (base_mm_s, false);
    }
    let ceiling = max_volumetric_flow_mm3_s / (bead_area_mm2(width_mm, height_mm) * flow_factor)
        * (1.0 - FLOW_CEILING_MARGIN);
    if ceiling.is_finite() && ceiling > 0.0 && ceiling < base_mm_s {
        (ceiling, true)
    } else {
        (base_mm_s, false)
    }
}

/// One layer's cooling decision.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayerCooling {
    /// Uniform speed scale in (0,1]; 1 when the layer is not slowed.
    pub scale: f64,
    /// The unslowed layer is shorter than the short-layer fan time.
    pub short: bool,
    /// Deposition seconds at unslowed speeds.
    pub unslowed_seconds: f64,
}
impl LayerCooling {
    pub const NONE: Self = Self {
        scale: 1.0,
        short: false,
        unslowed_seconds: 0.0,
    };
    pub fn slowed(&self) -> bool {
        self.scale < 1.0
    }
    /// The cooled speed of an edge whose unslowed speed is `unslowed_mm_s`.
    pub fn speed_mm_s(&self, unslowed_mm_s: f64, minimum_print_speed_mm_s: f64) -> f64 {
        if self.scale < 1.0 {
            (unslowed_mm_s * self.scale).max(minimum_print_speed_mm_s.min(unslowed_mm_s))
        } else {
            unslowed_mm_s
        }
    }
}

/// The unslowed speed of every deposition edge of `path` on layer `layer`,
/// with each edge's chord length: the input to the cooling rule.
pub fn unslowed_edges<'a>(
    intent: &'a ManufacturingIntent,
    path: &'a DepositionPath,
    owning: &PrintLayer,
    layer: u32,
) -> Result<impl Iterator<Item = (f64, f64, bool)> + 'a, ManufacturingError> {
    let process = intent.process_for_path(path)?;
    let routed = matches!(intent.printer.workflow, JobWorkflow::FixedDualMarlin { .. });
    let role = speed_role(&intent.recipe, owning, path.kind, path.owner);
    let base = base_speed_mm_s(&intent.recipe, &process, role, path.exposed, layer)
        .min(intent.printer.max_print_feed_mm_s);
    let flow = flow_factor(&process, path.kind, layer);
    let height = path.height_mm();
    let ceiling = process.max_volumetric_flow_mm3_s;
    Ok(path.checked_edges()?.map(move |edge| {
        let (a, b) = (edge.points_mm[0], edge.points_mm[1]);
        let length = (b[0] - a[0]).hypot(b[1] - a[1]).hypot(b[2] - a[2]);
        let (speed, limited) =
            flow_limited_speed_mm_s(base, edge.width_mm, height, flow, ceiling, routed);
        (length, speed, limited)
    }))
}

/// The cooling decision for one scheduled layer (`index` is its position).
pub fn layer_cooling(
    intent: &ManufacturingIntent,
    layer: &PrintLayer,
    index: u32,
) -> Result<LayerCooling, ManufacturingError> {
    let policy = &intent.recipe.process;
    if matches!(intent.printer.workflow, JobWorkflow::FixedDualMarlin { .. })
        || (policy.minimum_layer_time_s == 0.0 && policy.fan_short_layer_time_s == 0.0)
    {
        return Ok(LayerCooling::NONE);
    }
    let mut seconds = 0.0;
    for path in &layer.paths {
        for (length, speed, _) in unslowed_edges(intent, path, layer, index)? {
            seconds += length / speed;
        }
    }
    if !seconds.is_finite() {
        return Err(invalid("layer deposition time is not finite"));
    }
    let scale = if seconds > 0.0 && seconds < policy.minimum_layer_time_s {
        seconds / policy.minimum_layer_time_s
    } else {
        1.0
    };
    Ok(LayerCooling {
        scale,
        short: seconds > 0.0 && seconds < policy.fan_short_layer_time_s,
        unslowed_seconds: seconds,
    })
}

/// The part-fan PWM queued for a path: bridge fan, first-layer fan, the
/// short-layer (maximum) fan, else the ordinary (minimum) fan.
pub fn path_fan_pwm(policy: &ProcessPolicy, kind: DepositionKind, layer: u32, short: bool) -> u8 {
    if kind == DepositionKind::Bridge {
        policy.bridge_fan_pwm
    } else if layer == 0 {
        policy.fan_first_layers
    } else if short {
        policy.fan_short_layer_pwm
    } else {
        policy.fan_after_first_layers
    }
}
