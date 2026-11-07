// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;

// Exact XY identity, independent of traversal direction and signed zero.
fn row_key(path: &DepositionPath) -> Result<[[u64; 2]; 2], ManufacturingError> {
    if path.points_mm.len() != 2 {
        return Err(invalid(
            "landing-aligned construction requires complete straight rows",
        ));
    }
    let mut key: [[u64; 2]; 2] = std::array::from_fn(|i| {
        std::array::from_fn(|j| {
            let v = path.points_mm[i][j];
            if v == 0.0 { 0 } else { v.to_bits() }
        })
    });
    key.sort_unstable();
    Ok(key)
}

pub(super) fn validate_landing_alignment(
    plan: &NormalizedPrintPlan,
    intent: &ManufacturingIntent,
    band: &SparseBandDecision,
) -> Result<(), ManufacturingError> {
    if !band.landing_aligned {
        return Ok(());
    }
    let JobWorkflow::FixedDualMarlin { job } = &intent.printer.workflow else {
        return Err(invalid("landing alignment requires a bound interior tool"));
    };
    if !job.has_bound_interior()
        || band.disposition != SparseBandDisposition::Combined
        || band.first_layer == 0
        || band.first_layer >= band.last_layer
    {
        return Err(invalid("invalid landing-aligned sparse band"));
    }
    let terminal = plan
        .layers
        .get(band.last_layer as usize)
        .ok_or_else(|| invalid("landing-aligned terminal is absent"))?;
    let landing = plan
        .layers
        .get(band.last_layer as usize + 1)
        .ok_or_else(|| invalid("landing-aligned band must retain a fine landing"))?;
    let Some(DepositionOwner::Regional { region_index }) = band.owner else {
        return Err(invalid("landing alignment requires regional ownership"));
    };
    let region = terminal
        .region_coverage
        .process_regions
        .get(usize::from(region_index))
        .ok_or_else(|| invalid("landing-aligned owner is absent"))?;
    let matches_region = |candidate: &&ProcessRegionCoverage| {
        candidate.fraction == region.fraction && candidate.contributions == region.contributions
    };
    let mut owners = landing
        .region_coverage
        .process_regions
        .iter()
        .enumerate()
        .filter(|(_, candidate)| matches_region(candidate));
    let Some((landing_index, _)) = owners.next() else {
        return Err(invalid("landing-aligned source/profile owner is absent"));
    };
    if owners.next().is_some() {
        return Err(invalid(
            "landing alignment requires one complete source/profile owner",
        ));
    }
    let landing_owner = Some(DepositionOwner::Regional {
        region_index: landing_index as u16,
    });
    let binding = job.assignments[0]
        .interior
        .as_ref()
        .expect("checked interior binding");
    let mut coarse_rows = BTreeSet::new();
    let mut landing_rows = BTreeSet::new();
    let mut volume = 0.0;
    for path in terminal.paths.iter().filter(|p| p.owner == band.owner) {
        if !matches!(
            path.kind,
            DepositionKind::SparseFill | DepositionKind::Bridge
        ) || path.width.constant_width_mm() != Some(binding.bead_width_mm)
            || !coarse_rows.insert(row_key(path)?)
        {
            return Err(invalid(
                "landing-aligned coarse rows are incomplete or duplicated",
            ));
        }
        let process = intent.process_for_tool(Some(job.tool_for_path(path)?))?;
        let height = path.height_mm();
        let area = (binding.bead_width_mm - height) * height
            + std::f64::consts::PI * (height * 0.5).powi(2);
        volume += (path.points_mm[1][0] - path.points_mm[0][0])
            .hypot(path.points_mm[1][1] - path.points_mm[0][1])
            * area
            * process.flow_multiplier
            * if path.kind == DepositionKind::Bridge {
                process.policy.bridge_flow_factor
            } else {
                1.0
            };
    }
    for path in landing.paths.iter().filter(|p| p.owner == landing_owner) {
        if !matches!(
            path.kind,
            DepositionKind::SparseFill | DepositionKind::Bridge
        ) || path.physical_z_mm != [landing.lower_z_mm, landing.z_mm]
            || path.width.constant_width_mm() != Some(binding.bead_width_mm)
            || !landing_rows.insert(row_key(path)?)
        {
            return Err(invalid(
                "landing-aligned fine rows are incomplete or duplicated",
            ));
        }
    }
    if coarse_rows.is_empty()
        || coarse_rows != landing_rows
        || plan.sparse_bands.iter().any(|other| {
            other.disposition == SparseBandDisposition::Combined
                && other.first_layer <= landing.index
                && landing.index <= other.last_layer
        })
    {
        return Err(invalid(
            "coarse construction must biject to an untouched complete fine landing",
        ));
    }
    let mut target = 0.0;
    for layer in &plan.layers[band.first_layer as usize..=band.last_layer as usize] {
        let mut regions = layer
            .region_coverage
            .process_regions
            .iter()
            .filter(matches_region);
        let r = regions
            .next()
            .ok_or_else(|| invalid("landing alignment changes source/profile"))?;
        if regions.next().is_some() {
            return Err(invalid(
                "landing alignment has ambiguous effective ownership",
            ));
        }
        target += r.area_mm2 * r.fraction * (layer.z_mm - layer.lower_z_mm);
    }
    if !target.is_finite() || target <= 0.0 || !volume.is_finite() || volume < target {
        return Err(invalid(
            "landing-aligned construction falls below its nominal interior dose",
        ));
    }
    Ok(())
}
