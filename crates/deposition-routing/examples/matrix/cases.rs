// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Frozen lab panel: declared fixtures, seeded lattices, captured slicer and
//! native layers, and declared multi-tool machines. None is machine support.
use spiling_deposition_routing::*;

pub const DEPOSITION_MM_S: f64 = 40.;

pub struct Stroke {
    pub points: Vec<PointMm>,
    pub reversible: bool,
    /// Closed path whose seam may be any vertex.
    pub free_seam: bool,
    pub tool: u32,
    pub barrier: u32,
    pub prerequisites: Vec<u32>,
    pub physical_z_mm: [f64; 2],
}
impl Stroke {
    pub fn length(&self) -> f64 {
        self.points.windows(2).map(|p| distance(p[0], p[1])).sum()
    }
}
pub fn distance(a: PointMm, b: PointMm) -> f64 {
    (a[0] - b[0]).hypot(a[1] - b[1]).hypot(a[2] - b[2])
}
pub struct Case {
    pub name: &'static str,
    pub split: &'static str,
    pub strokes: Vec<Stroke>,
    pub machine: Machine,
    pub order: ToolOrder,
    pub feasible: bool,
}
impl Case {
    pub fn tasks(&self) -> Vec<Task<'_>> {
        self.strokes
            .iter()
            .map(|s| Task {
                ends: if s.free_seam {
                    Ends::Loop {
                        seams: &s.points[..s.points.len() - 1],
                    }
                } else {
                    Ends::Open {
                        first: s.points[0],
                        last: *s.points.last().unwrap(),
                        reversible: s.reversible,
                    }
                },
                tool: s.tool,
                barrier: s.barrier,
                prerequisites: &s.prerequisites,
                duration_s: s.length() / DEPOSITION_MM_S,
                seam_policy: None,
                physical_z_mm: s.physical_z_mm,
            })
            .collect()
    }
}

fn stroke(points: Vec<PointMm>) -> Stroke {
    let upper = points.first().map_or(0.2, |p| p[2]);
    Stroke {
        physical_z_mm: [upper - 0.2, upper],
        points,
        reversible: true,
        free_seam: false,
        tool: 0,
        barrier: 0,
        prerequisites: vec![],
    }
}
/// Declared single-tool lab machine with thresholded retraction.
fn single(start: PointMm) -> Machine {
    Machine {
        start: Some(start),
        initial_tool: Some(0),
        tools: 1,
        travel: Travel {
            speed_mm_s: 150.,
            lift_mm: 0.,
            retraction: Some(Retraction {
                minimum_mm: 2.,
                seconds: 0.1,
                lift_mm: 0.,
            }),
        },
        change: ToolChange {
            swap_s: 0.,
            prewarm_lead_s: None,
            service_s: 0.,
            service: Service::WhenUsed,
            stations_xy: vec![None],
        },
    }
}
fn chain(n: usize) -> Vec<Stroke> {
    // Input order is interleaved; geometry is one exact chain.
    (0..n)
        .step_by(2)
        .chain((1..n).step_by(2))
        .map(|i| {
            stroke(vec![
                [i as f64 * 2., 0., 0.2],
                [(i + 1) as f64 * 2., 0., 0.2],
            ])
        })
        .collect()
}
fn islands(n: usize) -> Vec<Stroke> {
    (0..n)
        .map(|i| {
            let x = if i % 2 == 0 { 0. } else { 30. };
            let y = (i / 2) as f64 * 2.;
            stroke(vec![[x, y, 0.2], [x + 6., y, 0.2]])
        })
        .collect()
}
fn nested_loops() -> Vec<Stroke> {
    [(0., 24.), (4., 16.), (8., 8.)]
        .into_iter()
        .map(|(o, side)| Stroke {
            reversible: false,
            free_seam: true,
            ..stroke(vec![
                [o, o, 0.2],
                [o + side, o, 0.2],
                [o + side, o + side, 0.2],
                [o, o + side, 0.2],
                [o, o, 0.2],
            ])
        })
        .collect()
}
fn locked_cycle() -> Vec<Stroke> {
    [
        ((0., 0.), (-3., -3.)),
        ((0., 0.), (10., 0.)),
        ((10., 0.), (10., 10.)),
        ((10., 10.), (0., 10.)),
        ((0., 10.), (0., 0.)),
    ]
    .into_iter()
    .map(|(a, b)| Stroke {
        reversible: false,
        ..stroke(vec![[a.0, a.1, 0.2], [b.0, b.1, 0.2]])
    })
    .collect()
}
struct Random(u64);
impl Random {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}
/// Seeded junction graph: every kept lattice edge is its own reversible stroke.
fn lattice(n: usize, seed: u64) -> Vec<Stroke> {
    let mut random = Random::new(seed);
    let mut strokes = Vec::new();
    for i in 0..n {
        for j in 0..n {
            let at = |i: usize, j: usize| [i as f64 * 4., j as f64 * 4., 0.2];
            for (a, b) in [(i + 1, j), (i, j + 1)] {
                if a < n && b < n && !random.next().is_multiple_of(5) {
                    strokes.push(stroke(vec![at(i, j), at(a, b)]));
                }
            }
        }
    }
    strokes
}
/// Seeded short strokes on several layers, spread over `tools` tools.
fn field(
    tools: u32,
    layers: u32,
    per_layer: usize,
    seed: u64,
    skip: Option<(u32, u32)>,
) -> Vec<Stroke> {
    let mut random = Random::new(seed);
    let mut strokes = Vec::new();
    for layer in 0..layers {
        let z = 0.2 * (layer + 1) as f64;
        for _ in 0..per_layer {
            let tool = (random.next() % tools as u64) as u32;
            let x = (random.next() % 400) as f64 * 0.1;
            let y = (random.next() % 400) as f64 * 0.1;
            if skip == Some((layer, tool)) {
                continue;
            }
            strokes.push(Stroke {
                tool,
                barrier: layer,
                ..stroke(vec![[x, y, z], [x + 3., y + 1., z]])
            });
        }
    }
    strokes
}
/// Declared rigid dual carriage: both tools serviced at their waste lane on
/// every layer; a selection is modelled at two seconds.
fn dual_carriage() -> Machine {
    Machine {
        start: None,
        change: ToolChange {
            swap_s: 2.,
            prewarm_lead_s: None,
            service_s: 0.5,
            service: Service::EveryBarrier,
            stations_xy: vec![Some([-6., 0.]), Some([-6., 12.])],
        },
        tools: 2,
        travel: Travel {
            speed_mm_s: 150.,
            lift_mm: 0.,
            retraction: None,
        },
        initial_tool: Some(0),
    }
}
/// Declared three-tool changer: tools appear only when used, a swap is
/// modelled at seven seconds and an incoming tool pre-warms twenty ahead.
fn changer() -> Machine {
    Machine {
        tools: 3,
        change: ToolChange {
            swap_s: 7.,
            prewarm_lead_s: Some(20.),
            service_s: 0.,
            service: Service::WhenUsed,
            stations_xy: vec![None; 3],
        },
        ..single([-5., -5., 0.2])
    }
}
fn captured(text: &str) -> Vec<Stroke> {
    let mut strokes: Vec<Stroke> = Vec::new();
    let mut tops: Vec<f64> = Vec::new();
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let mut words = line.split(' ');
        let kind = words.next().unwrap();
        let number = |words: &mut std::str::Split<'_, char>| -> f64 {
            words.next().unwrap().parse().unwrap()
        };
        match kind {
            "S" => {
                let _id = number(&mut words);
                let low = number(&mut words);
                let top = number(&mut words);
                tops.push(top);
                let locked = number(&mut words) != 0.;
                strokes.push(Stroke {
                    physical_z_mm: [low, top],
                    reversible: !locked,
                    ..stroke(vec![])
                });
            }
            "P" => {
                let z = *tops.last().unwrap();
                let v = [number(&mut words), number(&mut words), z];
                let s = strokes.last_mut().unwrap();
                if s.points.last() != Some(&v) {
                    s.points.push(v);
                }
            }
            "R" => {
                let id = number(&mut words) as usize;
                strokes[id].prerequisites = words.map(|w| w.parse().unwrap()).collect();
            }
            _ => panic!("unknown captured record"),
        }
    }
    // One barrier per distinct physical top, as production schedules layers.
    let mut levels = tops.clone();
    levels.sort_by(f64::total_cmp);
    levels.dedup();
    for (s, top) in strokes.iter_mut().zip(&tops) {
        s.barrier = levels.iter().position(|l| l == top).unwrap() as u32;
    }
    strokes
}
fn start_of(strokes: &[Stroke]) -> PointMm {
    let all = || strokes.iter().flat_map(|s| &s.points);
    let low = |axis: usize| all().map(|v| v[axis]).fold(f64::INFINITY, f64::min);
    [low(0) - 5., low(1) - 5., low(2)]
}
/// Sealed-panel isometry: axis swap, mirror and translation chosen by seed.
fn sealed(case: &mut Case, seed: u64) {
    let swap = seed & 1 == 1;
    let mirror = seed & 2 == 2;
    let shift = [(seed >> 2) % 7, (seed >> 5) % 7].map(|v| v as f64 * 0.5);
    let turn = |v: PointMm| {
        let x = if mirror { 100. - v[0] } else { v[0] };
        let (x, y) = if swap { (v[1], x) } else { (x, v[1]) };
        [x + shift[0], y + shift[1], v[2]]
    };
    for s in &mut case.strokes {
        for v in &mut s.points {
            *v = turn(*v);
        }
    }
    case.machine.start = case.machine.start.map(turn);
    for station in case.machine.change.stations_xy.iter_mut().flatten() {
        let v = turn([station[0], station[1], 0.]);
        *station = [v[0], v[1]];
    }
}
fn case(
    name: &'static str,
    split: &'static str,
    strokes: Vec<Stroke>,
    machine: Option<Machine>,
) -> Case {
    let machine = machine.unwrap_or_else(|| single(start_of(&strokes)));
    let order = if machine.tools > 1 {
        ToolOrder::Free
    } else {
        ToolOrder::Fixed
    };
    Case {
        name,
        split,
        strokes,
        machine,
        order,
        feasible: true,
    }
}
fn refused() -> Case {
    // Each stroke requires the other: no order exists, and none may be invented.
    let mut strokes = islands(2);
    strokes[0].prerequisites = vec![1];
    strokes[1].prerequisites = vec![0];
    Case {
        feasible: false,
        ..case("prerequisite-cycle", "negative", strokes, None)
    }
}
macro_rules! file {
    ($name:literal) => {
        captured(include_str!(concat!("captured/", $name, ".txt")))
    };
}
/// `seed` 0 is the frozen visible panel. Any other value regenerates seeded
/// families and applies an isometry, for promotion checks unseen by an editor.
fn fine_coarse(scale: f64) -> Vec<Stroke> {
    let mut paths = vec![
        stroke(vec![[0., 0., 0.2], [12. * scale, 0., 0.2]]),
        stroke(vec![[0., 0., 0.4], [12. * scale, 0., 0.4]]),
        stroke(vec![[1., 1., 0.4], [11. * scale, 1., 0.4]]),
        stroke(vec![[0., 0., 0.6], [12. * scale, 0., 0.6]]),
    ];
    paths[1].barrier = 1;
    paths[1].prerequisites = vec![0];
    paths[2].barrier = 1;
    paths[2].tool = 1;
    paths[2].physical_z_mm = [0., 0.4];
    paths[2].prerequisites = vec![0];
    paths[3].barrier = 2;
    paths[3].prerequisites = vec![1, 2];
    paths
}

pub fn panel(seed: u64) -> Vec<Case> {
    let mut cases = vec![
        case(
            "chain",
            "discovery",
            chain(12),
            Some(single([-4., -4., 0.2])),
        ),
        case(
            "mirrored-chain",
            "holdout",
            chain(17).into_iter().rev().collect(),
            Some(single([40., 6., 0.2])),
        ),
        case("islands", "discovery", islands(12), None),
        case("dense-fragments", "holdout", islands(32), None),
        case(
            "nested-loops",
            "discovery",
            nested_loops(),
            Some(single([30., 30., 0.2])),
        ),
        case("locked-cycle", "discovery", locked_cycle(), None),
        case("lattice", "discovery", lattice(7, 11 ^ seed), None),
        case("lattice-other", "holdout", lattice(8, 29 ^ seed), None),
        case(
            "dual-carriage",
            "discovery",
            field(2, 6, 14, 3 ^ seed, Some((2, 1))),
            Some(dual_carriage()),
        ),
        case(
            "dual-carriage-other",
            "holdout",
            field(2, 9, 10, 17 ^ seed, Some((4, 0))),
            Some(dual_carriage()),
        ),
        case(
            "changer-three",
            "discovery",
            field(3, 6, 12, 5 ^ seed, Some((3, 2))),
            Some(changer()),
        ),
        case(
            "changer-three-other",
            "holdout",
            field(3, 8, 9, 23 ^ seed, Some((1, 0))),
            Some(changer()),
        ),
        case(
            "fine-coarse-tools",
            "discovery",
            fine_coarse(1.),
            Some(dual_carriage()),
        ),
        case(
            "fine-coarse-tools-other",
            "holdout",
            fine_coarse(1.37),
            Some(dual_carriage()),
        ),
        case("slicer-block", "discovery", file!("block"), None),
        case("slicer-grille", "discovery", file!("grille"), None),
        case("slicer-grille-wide", "holdout", file!("grille-wide"), None),
        case("slicer-grille-top", "discovery", file!("grille-top"), None),
        case(
            "slicer-grille-top-wide",
            "holdout",
            file!("grille-top-wide"),
            None,
        ),
        case("slicer-bracket", "discovery", file!("bracket"), None),
        case(
            "slicer-bracket-long",
            "holdout",
            file!("bracket-long"),
            None,
        ),
        case("slicer-pins", "discovery", file!("pins"), None),
        case("slicer-pins-wide", "holdout", file!("pins-wide"), None),
        case("slicer-pin-stack", "discovery", file!("pin-stack"), None),
        case(
            "slicer-pin-stack-wide",
            "holdout",
            file!("pin-stack-wide"),
            None,
        ),
        case("slicer-code", "discovery", file!("code"), None),
        case("slicer-code-other", "holdout", file!("code-other"), None),
        case("slicer-fins", "discovery", file!("fins"), None),
        case("slicer-fins-many", "holdout", file!("fins-many"), None),
        case("cathedral-web", "discovery", file!("cathedral-web"), None),
        case(
            "cathedral-web-round",
            "holdout",
            file!("cathedral-web-round"),
            None,
        ),
        case("cathedral-roof", "discovery", file!("cathedral-roof"), None),
        case(
            "cathedral-roof-neck",
            "holdout",
            file!("cathedral-roof-neck"),
            None,
        ),
        case(
            "cathedral-roof-u",
            "discovery",
            file!("cathedral-roof-u"),
            None,
        ),
        case(
            "cathedral-roof-l",
            "holdout",
            file!("cathedral-roof-l"),
            None,
        ),
        case(
            "cathedral-roof-round",
            "discovery",
            file!("cathedral-roof-round"),
            None,
        ),
        case(
            "cathedral-roof-annulus",
            "holdout",
            file!("cathedral-roof-annulus"),
            None,
        ),
        case(
            "cathedral-stack",
            "discovery",
            file!("cathedral-stack"),
            None,
        ),
        case(
            "cathedral-stack-u",
            "holdout",
            file!("cathedral-stack-u"),
            None,
        ),
        refused(),
    ];
    if seed != 0 {
        for c in &mut cases {
            sealed(c, seed);
        }
    }
    cases
}
#[cfg(test)]
pub fn matrix() -> Vec<Case> {
    panel(0)
}
