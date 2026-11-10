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
pub fn matrix() -> Vec<Case> {
    vec![
        Case {
            name: "access-trap",
            split: "discovery",
            problem: access_trap(),
            feasible: true,
        },
        Case {
            name: "local-lift",
            split: "discovery",
            problem: local_lift(0.),
            feasible: true,
        },
        Case {
            name: "mirrored-access",
            split: "holdout",
            problem: permuted(access_trap()),
            feasible: true,
        },
        Case {
            name: "translated-lift",
            split: "holdout",
            problem: local_lift(7.),
            feasible: true,
        },
        Case {
            name: "junction-cycle",
            split: "discovery",
            problem: junction_cycle(),
            feasible: true,
        },
        Case {
            name: "chain",
            split: "discovery",
            problem: connected(12),
            feasible: true,
        },
        Case {
            name: "islands",
            split: "discovery",
            problem: islands(12),
            feasible: true,
        },
        Case {
            name: "nested-walls",
            split: "discovery",
            problem: walls(),
            feasible: true,
        },
        Case {
            name: "fine-coarse",
            split: "discovery",
            problem: mixed(),
            feasible: true,
        },
        Case {
            name: "obstacle-lift",
            split: "discovery",
            problem: tall(),
            feasible: true,
        },
        Case {
            name: "multi-hop",
            split: "discovery",
            problem: corridor(),
            feasible: true,
        },
        Case {
            name: "inside-hole",
            split: "discovery",
            problem: cavity(true),
            feasible: true,
        },
        Case {
            name: "incompatible-joins",
            split: "discovery",
            problem: incompatible(),
            feasible: true,
        },
        Case {
            name: "dense-fragments",
            split: "holdout",
            problem: islands(32),
            feasible: true,
        },
        Case {
            name: "mirrored-chain",
            split: "holdout",
            problem: permuted(connected(17)),
            feasible: true,
        },
        Case {
            name: "mirrored-mixed",
            split: "holdout",
            problem: permuted(mixed()),
            feasible: true,
        },
        Case {
            name: "mirrored-walls",
            split: "holdout",
            problem: permuted(walls()),
            feasible: true,
        },
        Case {
            name: "sealed-hole",
            split: "negative",
            problem: cavity(false),
            feasible: false,
        },
    ]
}
