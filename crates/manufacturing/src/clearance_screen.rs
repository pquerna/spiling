use spiling_contracts::manufacturing::{
    DepositionKind, DepositionPath, MAX_PLAN_COUNT, ManufacturingError, PrintLayer, WidthProfile,
    program_coordinates::{MAX_XYZ6_COORDINATE_MM, XYZ6_PROJECTION_SCREEN_MM},
};
use std::mem::size_of;

pub(super) const BLOCK_PATHS: usize = 16;

pub(super) struct ObjectClearanceIndex {
    layer_starts: Vec<usize>,
    blocks: Vec<Block>,
}

pub(super) struct Block {
    eligible_paths: usize,
    screen: Option<Screen>,
}

pub(super) fn bounded_height(height: [f64; 2]) -> bool {
    height.into_iter().all(bounded_xyz6)
}

fn bounded_xyz6(value: f64) -> bool {
    value.abs() <= MAX_XYZ6_COORDINATE_MM
}

impl Block {
    pub(super) fn clear(&self, support: [[f64; 2]; 2], width: f64, gap: f64) -> bool {
        self.eligible_paths == 0
            || self.screen.is_some_and(|(object, max_width)| {
                let reserve = (width + max_width) * 0.5 + 2.0 * XYZ6_PROJECTION_SCREEN_MM;
                let required_upper = (reserve + gap).next_up();
                if !(reserve >= 0.0 && gap >= 0.0 && required_upper.is_finite())
                    || !support.iter().flatten().all(|value| value.is_finite())
                {
                    return false;
                }
                (0..2).any(|axis| {
                    let lower = (object[0][axis] - support[1][axis])
                        .max(support[0][axis] - object[1][axis])
                        .next_down();
                    lower > 0.0 && lower >= required_upper
                })
            })
    }
}

type Screen = ([[f64; 2]; 2], f64);

fn screenable_path(
    path: &DepositionPath,
    charge: &mut impl FnMut() -> Result<(), ManufacturingError>,
) -> Result<Option<Screen>, ManufacturingError> {
    let height = path.height_mm();
    if !path.physical_z_mm.iter().all(|z| z.is_finite())
        || !height.is_finite()
        || height <= 0.0
        || path.points_mm.len() < 2
    {
        return Ok(None);
    }
    let widths = match &path.width {
        WidthProfile::Constant { width_mm } => std::slice::from_ref(width_mm),
        WidthProfile::PerEdge { widths_mm }
            if widths_mm.len() <= MAX_PLAN_COUNT && widths_mm.len() == path.points_mm.len() - 1 =>
        {
            widths_mm
        }
        WidthProfile::PerEdge { .. } => return Ok(None),
    };
    let mut max_width = 0.0f64;
    for &width in widths {
        charge()?;
        if !width.is_finite() || width < height {
            return Ok(None);
        }
        max_width = max_width.max(width);
    }
    let mut bounds = [[f64::INFINITY; 2], [f64::NEG_INFINITY; 2]];
    for point in &path.points_mm {
        charge()?;
        if !point.iter().copied().all(bounded_xyz6) {
            return Ok(None);
        }
        for axis in 0..2 {
            bounds[0][axis] = bounds[0][axis].min(point[axis]);
            bounds[1][axis] = bounds[1][axis].max(point[axis]);
        }
    }
    Ok(Some((bounds, max_width)))
}

impl ObjectClearanceIndex {
    pub(super) fn build(
        layers: &[PrintLayer],
        allowance: usize,
        eligible: impl Fn(DepositionKind) -> bool,
        mut charge: impl FnMut() -> Result<(), ManufacturingError>,
    ) -> Result<Option<Self>, ManufacturingError> {
        let mut block_count = 0usize;
        for layer in layers {
            charge()?;
            let Some(next) = block_count.checked_add(layer.paths.len().div_ceil(BLOCK_PATHS))
            else {
                return Ok(None);
            };
            block_count = next;
        }
        let Some(start_count) = layers.len().checked_add(1) else {
            return Ok(None);
        };
        let Some(bytes) = block_count
            .checked_mul(size_of::<Block>())
            .and_then(|bytes| {
                start_count
                    .checked_mul(size_of::<usize>())?
                    .checked_add(bytes)?
                    .checked_add(size_of::<Self>())
            })
        else {
            return Ok(None);
        };
        if block_count == 0 || bytes > allowance {
            return Ok(None);
        }
        let mut index = Self {
            layer_starts: Vec::new(),
            blocks: Vec::new(),
        };
        if index.layer_starts.try_reserve_exact(start_count).is_err()
            || index.blocks.try_reserve_exact(block_count).is_err()
            || index.retained_bytes() > allowance
        {
            return Ok(None);
        }
        for layer in layers {
            index.layer_starts.push(index.blocks.len());
            for paths in layer.paths.chunks(BLOCK_PATHS) {
                charge()?;
                let mut envelope = [[f64::INFINITY; 2], [f64::NEG_INFINITY; 2]];
                let mut width = 0.0f64;
                let mut known = true;
                let mut eligible_paths = 0usize;
                for path in paths.iter().filter(|path| eligible(path.kind)) {
                    charge()?;
                    eligible_paths += 1;
                    if !known {
                        continue;
                    }
                    let Some((bounds, path_width)) = screenable_path(path, &mut charge)? else {
                        known = false;
                        continue;
                    };
                    for axis in 0..2 {
                        envelope[0][axis] = envelope[0][axis].min(bounds[0][axis]);
                        envelope[1][axis] = envelope[1][axis].max(bounds[1][axis]);
                    }
                    width = width.max(path_width);
                }
                index.blocks.push(Block {
                    eligible_paths,
                    screen: known.then_some((envelope, width)),
                });
            }
        }
        index.layer_starts.push(index.blocks.len());
        Ok(Some(index))
    }

    pub(super) fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            + self.layer_starts.capacity() * size_of::<usize>()
            + self.blocks.capacity() * size_of::<Block>()
    }

    pub(super) fn layer(&self, index: usize) -> &[Block] {
        &self.blocks[self.layer_starts[index]..self.layer_starts[index + 1]]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_union_threshold_cannot_hide_a_members_outward_rounding() {
        let reserve = 1.0 + 2.0 * XYZ6_PROJECTION_SCREEN_MM;
        let required = reserve + 0.25;
        let support = [[-required - 1.0, 0.0], [-required, 0.0]];
        let member = [[f64::EPSILON / 8.0, 0.0], [1.0, 0.0]];
        let envelope = [[0.0, 0.0], [1.0, 0.0]];
        assert!(spiling_geometry::planar_bounds_clear(
            support, envelope, reserve, 0.25
        ));
        assert!(!spiling_geometry::planar_bounds_clear(
            support, member, reserve, 0.25
        ));
        let block = Block {
            eligible_paths: 2,
            screen: Some((envelope, 1.0)),
        };
        assert!(!block.clear(support, 1.0, 0.25));
    }
}
