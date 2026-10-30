// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Sufficient actual-XYZ6 nominal-capsule location proof. Polygon unions can
//! acquire conversion-grid slivers even between exactly tangent commands.
//! This proof covers EVERY current/prior command pair; a decline leaves the
//! existing polygonal location gate unchanged. It proves neither material nor
//! dose, uniqueness, process fidelity or measured physical bead dimensions.
use super::*;

struct Command<'a> {
    points: &'a [[f64; 3]],
    keys: [Key; 2],
    bounds: [[f64; 2]; 2],
    width: f64,
    decimal_width: Option<i64>,
}

// Same canonical-decimal eligibility as support::anchors::exact_decimal_bond.
// Noncanonical widths are NEVER rounded: they use the outward floating proof.
fn grid(value: f64) -> Option<i64> {
    let integer = (value * SITE_SCALE).round();
    (integer.is_finite() && integer.abs() <= (1u64 << 52) as f64 && integer / SITE_SCALE == value)
        .then_some(integer as i64)
}
impl<'a> Command<'a> {
    fn new(points: &'a [[f64; 3]], width: f64) -> Option<Self> {
        let keys = [
            [grid(points[0][0])?, grid(points[0][1])?],
            [grid(points[1][0])?, grid(points[1][1])?],
        ];
        if !width.is_finite() || width <= 0.0 {
            return None;
        }
        let radius = (width * 0.5).next_up();
        let bounds = std::array::from_fn(|axis| {
            [
                (points[0][axis].min(points[1][axis]).next_down() - radius).next_down(),
                (points[0][axis].max(points[1][axis]).next_up() + radius).next_up(),
            ]
        });
        if bounds.iter().flatten().any(|v| !v.is_finite()) {
            return None;
        }
        Some(Self {
            points,
            keys,
            bounds,
            width,
            decimal_width: grid(width),
        })
    }
}

pub(super) fn certificate(
    previous: &[DepositionPath],
    current: &DepositionPath,
    permissions: &[Junction],
    request: &WallRequest<'_>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    let edges = current.checked_edges()?;
    let count = edges.len();
    let Some(bytes) = count
        .checked_mul(std::mem::size_of::<Command<'_>>())
        .and_then(|n| n.checked_add(std::mem::size_of::<Vec<Command<'_>>>()))
    else {
        return Ok(false);
    };
    // Optional storage declines BEFORE allocation. Allocation failures and raw
    // cumulative work/cancellation failures are not caught as proof declines.
    if request
        .owned_bytes
        .checked_add(bytes)
        .is_none_or(|n| n > MAX_MANUFACTURING_RETAINED_BYTES as usize)
    {
        return Ok(false);
    }
    admit_stage(request, bytes)?;
    let mut commands = Vec::new();
    commands
        .try_reserve_exact(count)
        .map_err(|_| resource("wall capsule certificate allocation failed"))?;
    admit_stage(
        request,
        commands.capacity() * std::mem::size_of::<Command<'_>>()
            + std::mem::size_of::<Vec<Command<'_>>>(),
    )?;
    let mut permission_work = 0usize;
    for permission in permissions {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        permission_work = permission_work
            .checked_add(
                permission
                    .transition
                    .as_ref()
                    .map_or(1, |corridor| corridor.points.len().saturating_sub(1))
                    .checked_mul(8)
                    .ok_or_else(|| resource("wall capsule permission work overflow"))?,
            )
            .ok_or_else(|| resource("wall capsule permission inventory overflow"))?;
    }
    for (points, edge) in current.points_mm.windows(2).zip(edges) {
        check_cancel(cancel)?;
        boolean::charge(work, 1)?;
        let Some(command) = Command::new(points, edge.width_mm) else {
            return Ok(false);
        };
        commands.push(command);
    }
    for path in previous {
        let edges = path.checked_edges()?;
        for (points, edge) in path.points_mm.windows(2).zip(edges) {
            check_cancel(cancel)?;
            boolean::charge(work, 1)?;
            let Some(a) = Command::new(points, edge.width_mm) else {
                return Ok(false);
            };
            for b in &commands {
                check_cancel(cancel)?;
                boolean::charge(work, 1)?;
                let intersection: [[f64; 2]; 2] = std::array::from_fn(|axis| {
                    [
                        a.bounds[axis][0].max(b.bounds[axis][0]),
                        a.bounds[axis][1].min(b.bounds[axis][1]),
                    ]
                });
                if intersection.iter().any(|axis| axis[0] > axis[1]) {
                    continue;
                }
                let disjoint = separated(&a, b);
                if disjoint == Some(true) {
                    continue;
                }
                boolean::charge(work, permission_work)?;
                if permitted(&a, b, intersection, permissions) {
                    continue;
                }
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn difference(a: Key, b: Key) -> [i128; 2] {
    [a[0] as i128 - b[0] as i128, a[1] as i128 - b[1] as i128]
}
fn dot_checked(a: [i128; 2], b: [i128; 2]) -> Option<i128> {
    a[0].checked_mul(b[0])?.checked_add(a[1].checked_mul(b[1])?)
}
fn cross_checked(a: [i128; 2], b: [i128; 2]) -> Option<i128> {
    a[0].checked_mul(b[1])?.checked_sub(a[1].checked_mul(b[0])?)
}
fn same_strict_side(a: i128, b: i128) -> bool {
    (a > 0 && b > 0) || (a < 0 && b < 0)
}
fn centerlines_disjoint(a: [Key; 2], b: [Key; 2]) -> Option<bool> {
    if (0..2).any(|axis| {
        a[0][axis].max(a[1][axis]) < b[0][axis].min(b[1][axis])
            || b[0][axis].max(b[1][axis]) < a[0][axis].min(a[1][axis])
    }) {
        return Some(true);
    }
    let v = difference(a[1], a[0]);
    let w = difference(b[1], b[0]);
    Some(
        same_strict_side(
            cross_checked(v, difference(b[0], a[0]))?,
            cross_checked(v, difference(b[1], a[0]))?,
        ) || same_strict_side(
            cross_checked(w, difference(a[0], b[0]))?,
            cross_checked(w, difference(a[1], b[0]))?,
        ),
    )
}
fn down_integer(value: i128) -> Option<f64> {
    (value >= 0).then(|| (value as f64).next_down().max(0.0))
}
fn up_integer(value: i128) -> Option<f64> {
    (value >= 0).then(|| (value as f64).next_up())
}

// For nonintersecting segments the minimum distance is attained by one of
// their four endpoint/segment pairs. Exact canonical widths use the established
// integer rational comparison; other widths keep their FULL original value.
fn separated(a: &Command<'_>, b: &Command<'_>) -> Option<bool> {
    if !centerlines_disjoint(a.keys, b.keys)? {
        return Some(false);
    }
    let decimal_radius = a
        .decimal_width
        .zip(b.decimal_width)
        .and_then(|(a, b)| (a as i128).checked_add(b as i128));
    // Keys below are doubled micrometres. Half-widths therefore sum to the
    // sum of the two canonical width inventories, without rounding a radius.
    let radius_upper = (((a.width * 0.5).next_up() + (b.width * 0.5).next_up()).next_up()
        * (2.0 * SITE_SCALE))
        .next_up();
    if !radius_upper.is_finite() {
        return None;
    }
    for (points, segment) in [(a.keys, b.keys), (b.keys, a.keys)] {
        for point in points {
            if !endpoint_separated(point, segment, decimal_radius, radius_upper)? {
                return Some(false);
            }
        }
    }
    Some(true)
}
fn endpoint_separated(
    point: Key,
    segment: [Key; 2],
    radius: Option<i128>,
    radius_upper: f64,
) -> Option<bool> {
    let v = difference(segment[1], segment[0]);
    let q = difference(point, segment[0]);
    let v = [v[0].checked_mul(2)?, v[1].checked_mul(2)?];
    let q = [q[0].checked_mul(2)?, q[1].checked_mul(2)?];
    let length = dot_checked(v, v)?;
    if length <= 0 {
        return None;
    }
    let projection = dot_checked(q, v)?;
    let (numerator, lower, denominator) = if projection <= 0 {
        let value = dot_checked(q, q)?;
        (Some(value), down_integer(value)?, 1)
    } else if projection >= length {
        let q = [q[0].checked_sub(v[0])?, q[1].checked_sub(v[1])?];
        let value = dot_checked(q, q)?;
        (Some(value), down_integer(value)?, 1)
    } else {
        let cross = cross_checked(q, v)?.checked_abs()?;
        let lower = down_integer(cross)?;
        (
            cross.checked_mul(cross),
            (lower * lower).next_down().max(0.0),
            length,
        )
    };
    if let (Some(numerator), Some(radius)) = (numerator, radius)
        && let Some(right) = radius
            .checked_mul(radius)
            .and_then(|r| r.checked_mul(denominator))
    {
        return Some(numerator >= right);
    }
    let right = ((radius_upper * radius_upper).next_up() * up_integer(denominator)?).next_up();
    if !lower.is_finite() || !right.is_finite() {
        return None;
    }
    Some(lower >= right)
}

fn permitted(
    a: &Command<'_>,
    b: &Command<'_>,
    bounds: [[f64; 2]; 2],
    permissions: &[Junction],
) -> bool {
    let corners = [
        [bounds[0][0], bounds[1][0]],
        [bounds[0][0], bounds[1][1]],
        [bounds[0][1], bounds[1][0]],
        [bounds[0][1], bounds[1][1]],
    ];
    permissions.iter().any(|permission| {
        let full_capsule = |command: &Command<'_>| {
            qualification::permission_contains_capsule(
                permission,
                std::array::from_fn(|i| [command.points[i][0], command.points[i][1]]),
                command.width * 0.5 + program_coordinates::MAX_BEAD_COMMAND_LATERAL_ERROR_MM,
            )
        };
        full_capsule(a)
            || full_capsule(b)
            || (permission.transition.is_none()
                && qualification::junction_contains_capsule(
                    permission.center,
                    [corners[0], corners[3]],
                    0.0,
                    permission.radius_mm,
                )
                && qualification::junction_contains_capsule(
                    permission.center,
                    [corners[1], corners[2]],
                    0.0,
                    permission.radius_mm,
                ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(points: &[[f64; 2]], width: f64) -> DepositionPath {
        DepositionPath {
            material_domain: None,
            kind: DepositionKind::Perimeter,
            owner: Some(DepositionOwner::Wall),
            width: WidthProfile::Constant { width_mm: width },
            physical_z_mm: [0.0, 0.2],
            points_mm: points.iter().map(|p| [p[0], p[1], 0.2]).collect(),
        }
    }

    #[test]
    fn tangent_commands_require_all_pair_and_unrounded_width_authority() {
        let material = MultiPolygon(vec![Polygon::new(
            geo::LineString::from(vec![
                (100.0, 100.0),
                (120.0, 100.0),
                (120.0, 130.0),
                (100.0, 130.0),
                (100.0, 100.0),
            ]),
            Vec::new(),
        )]);
        let required = region::RequiredMaterial::from_section(&material);
        let recipe = super::super::super::tests::recipe(0.25);
        let limits = DepositionLimits {
            min_bead_width_mm: 0.3,
            max_bead_width_mm: 0.75,
            min_bead_height_mm: 0.000001,
            max_bead_height_mm: 0.3,
        };
        let request = WallRequest {
            material: &material,
            required: &required,
            recipe: &recipe,
            limits: &limits,
            nozzle_diameter_mm: 0.4,
            lower_z: 0.0,
            upper_z: 0.2,
            retained_points: region::point_count(&material),
            owned_bytes: 0,
        };
        let above = stroke(&[[107.0, 118.534972], [113.0, 118.534972]], 0.45);
        let below = stroke(&[[107.0, 118.084972], [113.0, 118.084972]], 0.45);
        let cancel = AtomicBool::new(false);
        let mut work = 0;
        assert!(
            certificate(
                std::slice::from_ref(&above),
                &below,
                &[],
                &request,
                &mut work,
                &cancel
            )
            .unwrap(),
            "exact decimal tangency is not positive nominal overlap"
        );
        let wider = stroke(
            &[[107.0, 118.084972], [113.0, 118.084972]],
            0.45_f64.next_up(),
        );
        assert!(
            !certificate(
                std::slice::from_ref(&above),
                &wider,
                &[],
                &request,
                &mut work,
                &cancel
            )
            .unwrap(),
            "an arbitrary noncanonical width must not be rounded down to the tangent radius"
        );
        let narrower = stroke(&[[107.0, 118.084972], [113.0, 118.084972]], 0.449998123456);
        assert!(
            certificate(
                std::slice::from_ref(&above),
                &narrower,
                &[],
                &request,
                &mut work,
                &cancel
            )
            .unwrap(),
            "outward arithmetic can prove a genuinely separated noncanonical width without quantizing it"
        );
        let crossing = stroke(&[[110.0, 118.064972], [110.0, 118.104972]], 0.45);
        let prior = [above, crossing];
        assert!(
            !certificate(&prior, &below, &[], &request, &mut work, &cancel).unwrap(),
            "the tangent pair cannot hide another actual cross-path overlap"
        );
        let permission = Junction {
            center: [110.0, 118.084972],
            radius_mm: 0.75,
            valence: 3,
            ports: Vec::new(),
            transition: None,
        };
        assert!(
            certificate(&prior, &below, &[permission], &request, &mut work, &cancel).unwrap(),
            "a complete source-permitted capsule bounds its whole intersection, not only a probe"
        );
    }
}
