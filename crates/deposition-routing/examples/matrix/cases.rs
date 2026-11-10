// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
use spiling_deposition_routing::*;

pub struct Case {
    pub name: &'static str,
    pub split: &'static str,
    pub problem: Problem,
    pub feasible: bool,
}
pub fn point(x: f64, y: f64, z: f64) -> PointMm {
    PointMm([x, y, z])
}
pub fn bounds(min: [f64; 3], max: [f64; 3]) -> BoundsMm {
    BoundsMm { min, max }
}
fn stroke(id: u64, points: Vec<PointMm>, height: f64) -> Stroke {
    let z = points[0].0[2];
    let edges = points.len() - 1;
    Stroke {
        id: StrokeId(id),
        points_mm: points,
        widths_mm: vec![0.6; edges],
        physical_z_mm: [z - height, z],
        process: ProcessKey {
            tool: 0,
            material: 0,
            continuity_group: 0,
        },
        prerequisites: vec![],
        allow_reverse: true,
        allow_seam_rotation: false,
    }
}
fn base() -> Problem {
    Problem {
        strokes: vec![],
        start_mm: point(-4., -4., 1.),
        envelope_mm: bounds([-5., -5., 0.], [60., 60., 20.]),
        head_mm: vec![bounds([-0.3, -0.3, 0.], [0.3, 0.3, 2.])],
        obstacles_mm: vec![],
        waypoints_mm: vec![],
        travel_speed_mm_s: 100.,
        deposition_speed_mm_s: 40.,
    }
}
fn connected(n: usize) -> Problem {
    let mut p = base();
    // Input order is intentionally interleaved; geometry remains one chain.
    for i in (0..n).step_by(2).chain((1..n).step_by(2)) {
        p.strokes.push(stroke(
            i as u64,
            vec![
                point(i as f64 * 2., 0., 0.2),
                point((i + 1) as f64 * 2., 0., 0.2),
            ],
            0.2,
        ));
    }
    p
}
fn islands(n: usize) -> Problem {
    let mut p = base();
    for i in 0..n {
        let x = if i % 2 == 0 { 0. } else { 30. };
        let y = (i / 2) as f64 * 2.;
        p.strokes.push(stroke(
            i as u64,
            vec![point(x, y, 0.2), point(x + 6., y, 0.2)],
            0.2,
        ));
    }
    p
}
fn walls() -> Problem {
    let mut p = base();
    for (id, x, y, side) in [(0, 0., 0., 24.), (1, 4., 4., 16.), (2, 8., 8., 8.)] {
        let mut s = stroke(
            id,
            vec![
                point(x, y, 0.2),
                point(x + side, y, 0.2),
                point(x + side, y + side, 0.2),
                point(x, y + side, 0.2),
                point(x, y, 0.2),
            ],
            0.2,
        );
        s.allow_reverse = false;
        s.allow_seam_rotation = id != 0;
        s.widths_mm = vec![0.55, 0.65, 0.7, 0.6];
        p.strokes.push(s);
    }
    p
}
fn mixed() -> Problem {
    let mut p = base();
    for i in 0..4 {
        let mut fine = stroke(
            i,
            vec![
                point(0., i as f64 * 3., 0.2),
                point(12., i as f64 * 3., 0.2),
            ],
            0.2,
        );
        fine.widths_mm[0] = 0.45;
        p.strokes.push(fine);
        let mut coarse = stroke(
            10 + i,
            vec![
                point(0., i as f64 * 3., 0.6),
                point(12., i as f64 * 3., 0.6),
            ],
            0.4,
        );
        coarse.prerequisites = vec![StrokeId(i)];
        coarse.widths_mm[0] = 0.8;
        p.strokes.push(coarse);
    }
    p
}
fn tall() -> Problem {
    let mut p = base();
    p.start_mm = point(0., 10., 0.2);
    p.obstacles_mm.push(bounds([9., 0., 0.], [11., 20., 8.]));
    p.strokes.push(stroke(
        0,
        vec![point(20., 10., 0.2), point(25., 10., 0.2)],
        0.2,
    ));
    p
}
fn corridor() -> Problem {
    let mut p = base();
    p.start_mm = point(0., 2., 1.);
    p.envelope_mm = bounds([-1., -1., 0.], [25., 21., 6.]);
    // A ceiling prohibits lifting over the alternating walls.
    p.obstacles_mm = vec![
        bounds([-1., -1., 4.], [25., 21., 6.]),
        bounds([6., -1., 0.], [8., 14., 4.]),
        bounds([14., 6., 0.], [16., 21., 4.]),
    ];
    p.waypoints_mm = vec![
        point(5., 16., 1.),
        point(10., 16., 1.),
        point(12., 3., 1.),
        point(18., 3., 1.),
    ];
    p.strokes.push(stroke(
        0,
        vec![point(21., 16., 1.), point(23., 16., 1.)],
        0.2,
    ));
    p
}
fn cavity(feasible: bool) -> Problem {
    let mut p = base();
    p.start_mm = point(0., 5., 0.2);
    p.obstacles_mm = vec![
        bounds([4., 0., 0.], [5., 10., 10.]),
        bounds([4., 0., 0.], [16., 1., 10.]),
        bounds([15., 0., 0.], [16., 10., 10.]),
        bounds([4., 9., 0.], [16., 10., 10.]),
    ];
    if !feasible {
        p.obstacles_mm.push(bounds([4., 0., 8.], [16., 10., 10.]));
    }
    p.strokes.push(stroke(
        0,
        vec![point(8., 5., 0.2), point(12., 5., 0.2)],
        0.2,
    ));
    p
}
fn junction_cycle() -> Problem {
    let mut p = base();
    let edges = [
        ((0., 0.), (-3., -3.)),
        ((0., 0.), (10., 0.)),
        ((10., 0.), (10., 10.)),
        ((10., 10.), (0., 10.)),
        ((0., 10.), (0., 0.)),
    ];
    for (i, (a, b)) in edges.into_iter().enumerate() {
        let mut s = stroke(
            i as u64,
            vec![point(a.0, a.1, 0.2), point(b.0, b.1, 0.2)],
            0.2,
        );
        s.allow_reverse = false;
        p.strokes.push(s);
    }
    p
}
fn incompatible() -> Problem {
    let mut p = connected(5);
    for (i, s) in p.strokes.iter_mut().enumerate() {
        s.process.continuity_group = i as u32;
        s.widths_mm[0] = 0.45 + i as f64 * 0.04;
    }
    p
}
fn permuted(mut p: Problem) -> Problem {
    // Mirror/translation in XY preserves explicit widths and actual Z bands.
    let transform = |v: PointMm| point(40. - v.0[0], v.0[1] + 10., v.0[2]);
    p.start_mm = transform(p.start_mm);
    for s in &mut p.strokes {
        for v in &mut s.points_mm {
            *v = transform(*v);
        }
    }
    p.strokes.reverse();
    p
}
fn access_trap() -> Problem {
    let mut p = base();
    p.start_mm = point(0., 0., 0.2);
    p.head_mm = vec![bounds([-1., -1., 0.], [1., 1., 2.])];
    let mut tall = stroke(0, vec![point(0., 0., 0.6), point(0., 6., 0.6)], 0.6);
    tall.widths_mm[0] = 0.8;
    let low = stroke(1, vec![point(1., 6., 0.2), point(1., 0., 0.2)], 0.2);
    p.strokes = vec![tall, low];
    p
}
fn local_lift(shift: f64) -> Problem {
    let mut p = base();
    p.start_mm = point(0. + shift, 5., 0.2);
    p.envelope_mm.max[2] = 6.;
    p.obstacles_mm = vec![
        bounds([4. + shift, 0., 0.], [6. + shift, 10., 1.]),
        bounds([-5., -5., 4.], [60., 60., 6.]),
    ];
    p.strokes = vec![stroke(
        0,
        vec![point(12. + shift, 5., 0.2), point(16. + shift, 5., 0.2)],
        0.2,
    )];
    p
}
/// Declared reference head: a nozzle below a wider heater block. Not calibrated.
fn captured(text: &str) -> Problem {
    // PrusaSlicer roles, then native Spiling plan path kinds.
    const ROLES: [&str; 11] = [
        "Perimeter",
        "External_perimeter",
        "Overhang_perimeter",
        "Internal_infill",
        "Solid_infill",
        "Top_solid_infill",
        "perimeter",
        "sparse_fill",
        "bottom_skin",
        "top_skin",
        "bridge",
    ];
    let mut p = base();
    p.head_mm = vec![
        bounds([-0.3, -0.3, 0.], [0.3, 0.3, 2.]),
        bounds([-8., -8., 2.], [8., 8., 12.]),
    ];
    p.travel_speed_mm_s = 150.;
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let mut words = line.split(' ');
        let kind = words.next().unwrap();
        let number = |words: &mut std::str::Split<'_, char>| -> f64 {
            words.next().unwrap().parse().unwrap()
        };
        match kind {
            "S" => {
                let id = number(&mut words) as u64;
                let z = [number(&mut words), number(&mut words)];
                let locked = number(&mut words) != 0.;
                let role = words.next().unwrap();
                p.strokes.push(Stroke {
                    id: StrokeId(id),
                    points_mm: vec![],
                    widths_mm: vec![],
                    physical_z_mm: z,
                    process: ProcessKey {
                        tool: 0,
                        material: 0,
                        continuity_group: ROLES
                            .iter()
                            .position(|r| *r == role)
                            .unwrap_or(ROLES.len())
                            as u32,
                    },
                    prerequisites: vec![],
                    allow_reverse: !locked,
                    allow_seam_rotation: false,
                });
            }
            "P" => {
                let s = p.strokes.last_mut().unwrap();
                let v = point(number(&mut words), number(&mut words), s.physical_z_mm[1]);
                let width = number(&mut words);
                if s.points_mm.last() != Some(&v) {
                    if !s.points_mm.is_empty() {
                        s.widths_mm.push(width);
                    }
                    s.points_mm.push(v);
                }
            }
            "R" => {
                let id = StrokeId(number(&mut words) as u64);
                let s = p.strokes.iter_mut().find(|s| s.id == id).unwrap();
                s.prerequisites = words.map(|w| StrokeId(w.parse().unwrap())).collect();
            }
            _ => panic!("unknown captured record"),
        }
    }
    let all = || p.strokes.iter().flat_map(|s| &s.points_mm);
    let low = |axis: usize| all().map(|v| v.0[axis]).fold(f64::INFINITY, f64::min);
    let high = |axis: usize| all().map(|v| v.0[axis]).fold(f64::NEG_INFINITY, f64::max);
    p.envelope_mm = bounds(
        [low(0) - 10., low(1) - 10., 0.],
        [high(0) + 10., high(1) + 10., 20.],
    );
    p.start_mm = point(low(0) - 5., low(1) - 5., p.strokes[0].physical_z_mm[1]);
    p
}
/// Seeded junction graph: every lattice edge is its own reversible stroke.
fn lattice(n: usize, seed: u64) -> Problem {
    let mut p = base();
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut keep = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        !state.is_multiple_of(5)
    };
    for i in 0..n {
        for j in 0..n {
            let at = |i: usize, j: usize| point(i as f64 * 4., j as f64 * 4., 0.2);
            for (a, b) in [(i + 1, j), (i, j + 1)] {
                if a < n && b < n && keep() {
                    let id = p.strokes.len() as u64;
                    p.strokes.push(stroke(id, vec![at(i, j), at(a, b)], 0.2));
                }
            }
        }
    }
    p
}
/// Sealed-panel isometry: axis swap, mirror and translation chosen by seed.
/// Axis-aligned boxes and exact shared endpoints are preserved.
fn sealed(mut p: Problem, seed: u64) -> Problem {
    let swap = seed & 1 == 1;
    let mirror = seed & 2 == 2;
    let shift = [(seed >> 2) % 7, (seed >> 5) % 7].map(|v| v as f64 * 0.5);
    let center = p.envelope_mm.min[0] + p.envelope_mm.max[0];
    let turn = |v: [f64; 3]| {
        let x = if mirror { center - v[0] } else { v[0] };
        let (x, y) = if swap { (v[1], x) } else { (x, v[1]) };
        [x + shift[0], y + shift[1], v[2]]
    };
    let turn_box = |b: BoundsMm, f: &dyn Fn([f64; 3]) -> [f64; 3]| {
        let (a, c) = (f(b.min), f(b.max));
        bounds(
            std::array::from_fn(|i| a[i].min(c[i])),
            std::array::from_fn(|i| a[i].max(c[i])),
        )
    };
    let relative = |v: [f64; 3]| {
        let x = if mirror { -v[0] } else { v[0] };
        if swap {
            [v[1], x, v[2]]
        } else {
            [x, v[1], v[2]]
        }
    };
    p.start_mm = PointMm(turn(p.start_mm.0));
    p.envelope_mm = turn_box(p.envelope_mm, &turn);
    for b in &mut p.obstacles_mm {
        *b = turn_box(*b, &turn);
    }
    for b in &mut p.head_mm {
        *b = turn_box(*b, &relative);
    }
    for v in &mut p.waypoints_mm {
        *v = PointMm(turn(v.0));
    }
    for s in &mut p.strokes {
        for v in &mut s.points_mm {
            *v = PointMm(turn(v.0));
        }
    }
    p
}
fn case(name: &'static str, split: &'static str, problem: Problem, feasible: bool) -> Case {
    Case {
        name,
        split,
        problem,
        feasible,
    }
}
/// `seed` 0 is the frozen visible panel. Any other value regenerates seeded
/// families and applies an isometry, for promotion checks unseen by an editor.
pub fn panel(seed: u64) -> Vec<Case> {
    let mut cases = vec![
        case("access-trap", "discovery", access_trap(), true),
        case("local-lift", "discovery", local_lift(0.), true),
        case("mirrored-access", "holdout", permuted(access_trap()), true),
        case("translated-lift", "holdout", local_lift(7.), true),
        case("junction-cycle", "discovery", junction_cycle(), true),
        case("chain", "discovery", connected(12), true),
        case("islands", "discovery", islands(12), true),
        case("nested-walls", "discovery", walls(), true),
        case("fine-coarse", "discovery", mixed(), true),
        case("obstacle-lift", "discovery", tall(), true),
        case("multi-hop", "discovery", corridor(), true),
        case("inside-hole", "discovery", cavity(true), true),
        case("incompatible-joins", "discovery", incompatible(), true),
        case("dense-fragments", "holdout", islands(32), true),
        case("mirrored-chain", "holdout", permuted(connected(17)), true),
        case("mirrored-mixed", "holdout", permuted(mixed()), true),
        case("mirrored-walls", "holdout", permuted(walls()), true),
        case("sealed-hole", "negative", cavity(false), false),
        case("lattice", "discovery", lattice(7, 11 ^ seed), true),
        case("lattice-other", "holdout", lattice(8, 29 ^ seed), true),
        case(
            "slicer-block",
            "discovery",
            captured(include_str!("captured/block.txt")),
            true,
        ),
        case(
            "slicer-grille",
            "discovery",
            captured(include_str!("captured/grille.txt")),
            true,
        ),
        case(
            "slicer-grille-wide",
            "holdout",
            captured(include_str!("captured/grille-wide.txt")),
            true,
        ),
        case(
            "slicer-grille-top",
            "discovery",
            captured(include_str!("captured/grille-top.txt")),
            true,
        ),
        case(
            "slicer-grille-top-wide",
            "holdout",
            captured(include_str!("captured/grille-top-wide.txt")),
            true,
        ),
        case(
            "slicer-bracket",
            "discovery",
            captured(include_str!("captured/bracket.txt")),
            true,
        ),
        case(
            "slicer-bracket-long",
            "holdout",
            captured(include_str!("captured/bracket-long.txt")),
            true,
        ),
        case(
            "slicer-pins",
            "discovery",
            captured(include_str!("captured/pins.txt")),
            true,
        ),
        case(
            "slicer-pins-wide",
            "holdout",
            captured(include_str!("captured/pins-wide.txt")),
            true,
        ),
        case(
            "slicer-pin-stack",
            "discovery",
            captured(include_str!("captured/pin-stack.txt")),
            true,
        ),
        case(
            "slicer-pin-stack-wide",
            "holdout",
            captured(include_str!("captured/pin-stack-wide.txt")),
            true,
        ),
        case(
            "slicer-code",
            "discovery",
            captured(include_str!("captured/code.txt")),
            true,
        ),
        case(
            "slicer-code-other",
            "holdout",
            captured(include_str!("captured/code-other.txt")),
            true,
        ),
        case(
            "slicer-fins",
            "discovery",
            captured(include_str!("captured/fins.txt")),
            true,
        ),
        case(
            "slicer-fins-many",
            "holdout",
            captured(include_str!("captured/fins-many.txt")),
            true,
        ),
        case(
            "cathedral-web",
            "discovery",
            captured(include_str!("captured/cathedral-web.txt")),
            true,
        ),
        case(
            "cathedral-web-round",
            "holdout",
            captured(include_str!("captured/cathedral-web-round.txt")),
            true,
        ),
        case(
            "cathedral-roof",
            "discovery",
            captured(include_str!("captured/cathedral-roof.txt")),
            true,
        ),
        case(
            "cathedral-roof-neck",
            "holdout",
            captured(include_str!("captured/cathedral-roof-neck.txt")),
            true,
        ),
        case(
            "cathedral-roof-u",
            "discovery",
            captured(include_str!("captured/cathedral-roof-u.txt")),
            true,
        ),
        case(
            "cathedral-roof-l",
            "holdout",
            captured(include_str!("captured/cathedral-roof-l.txt")),
            true,
        ),
        case(
            "cathedral-roof-round",
            "discovery",
            captured(include_str!("captured/cathedral-roof-round.txt")),
            true,
        ),
        case(
            "cathedral-roof-annulus",
            "holdout",
            captured(include_str!("captured/cathedral-roof-annulus.txt")),
            true,
        ),
        case(
            "cathedral-stack",
            "discovery",
            captured(include_str!("captured/cathedral-stack.txt")),
            true,
        ),
        case(
            "cathedral-stack-u",
            "holdout",
            captured(include_str!("captured/cathedral-stack-u.txt")),
            true,
        ),
    ];
    if seed != 0 {
        for c in &mut cases {
            c.problem = sealed(c.problem.clone(), seed);
        }
    }
    cases
}
#[cfg(test)]
pub fn matrix() -> Vec<Case> {
    panel(0)
}
