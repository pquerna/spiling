// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use spiling_contracts::geometry::{AabbMm, GeometryError};
use spiling_geometry::{BoundsPolicy, Definition, GeometryWorkBudget, projected_bounds};
use std::sync::atomic::AtomicBool;

/// Physical dimension assertions need native directional queries. Import bounds
/// are conservative exclusions, with no promise of this endpoint precision.
pub fn physical_bounds(
    definition: &Definition,
    tolerance_mm: f64,
) -> Result<AabbMm, GeometryError> {
    let mut work = GeometryWorkBudget::new(20_000_000);
    let mut result = AabbMm {
        min: [0.0; 3],
        max: [0.0; 3],
    };
    for axis in 0..3 {
        let mut direction = [0.0; 3];
        direction[axis] = 1.0;
        let policy = BoundsPolicy {
            maximum_endpoint_uncertainty_mm: tolerance_mm * 0.5,
        };
        let bounds = projected_bounds(
            definition,
            direction,
            policy,
            &mut work,
            &AtomicBool::new(false),
        )?;
        assert!(bounds.achieved_uncertainty_mm <= policy.maximum_endpoint_uncertainty_mm);
        result.min[axis] = bounds.minimum[0];
        result.max[axis] = bounds.maximum[1];
    }
    result.validate()?;
    Ok(result)
}
