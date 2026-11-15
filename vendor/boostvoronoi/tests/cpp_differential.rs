// SPDX-License-Identifier:BSL-1.0
//
// Fidelity to C++ Boost.Polygon: each diagram here is digested (counts, every cell's site,
// every vertex's coordinate bits, every edge's cell, vertex, twin, next, prev and kind) and
// compared with the digest C++ Boost.Polygon 1.86 (g++, libstdc++) computes for the same input,
// recorded in tests/data/cpp_reference_digests.txt by tests/reference/boost_digest.cpp.
//
// The beach line's comparator is non-monotone across arcs Boost keeps behind the sweep (also in
// C++), so the diagram depends on the exact probes the beach line map and the event queues
// make. These tests hold the port to the reference's: a change to the map, the queues or the
// predicates that alters any diagram shows up here.

use boostvoronoi::prelude::*;
use std::collections::BTreeMap;

const DATA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data");

struct Fnv(u64);
impl Fnv {
    fn word(&mut self, w: u64) {
        for i in 0..8 {
            self.0 ^= (w >> (8 * i)) & 0xff;
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}

/// (cells, vertices, edges, digest) of the diagram of `segments`, as boost_digest.cpp computes
/// them.
fn digest(segments: &[[i32; 4]]) -> (usize, usize, usize, u64) {
    let d = Builder::<i32>::default()
        .with_segments(segments.iter().copied())
        .and_then(|b| b.build())
        .expect("a diagram");
    let mut f = Fnv(0xcbf2_9ce4_8422_2325);
    f.word(d.num_cells() as u64);
    f.word(d.num_vertices() as u64);
    f.word(d.num_edges() as u64);
    // Each cell by the site it stands for (the libraries number sources differently): a point
    // (1, x, y) or a segment (3, x0, y0, x1, y1) of the input.
    let i = |v: i32| i64::from(v) as u64;
    for c in d.cells() {
        let s = segments[c.source_index().usize()];
        let words: Vec<u64> = match c.source_category() {
            SourceCategory::SegmentStart => vec![1, i(s[0]), i(s[1])],
            SourceCategory::SegmentEnd => vec![1, i(s[2]), i(s[3])],
            SourceCategory::Segment | SourceCategory::SinglePoint => {
                vec![3, i(s[0]), i(s[1]), i(s[2]), i(s[3])]
            }
        };
        for w in words {
            f.word(w);
        }
    }
    for v in d.vertices() {
        f.word(v.x().to_bits());
        f.word(v.y().to_bits());
    }
    for e in d.edges() {
        f.word(e.cell().expect("an edge's cell").usize() as u64);
        f.word(e.vertex0().map_or(u64::MAX, |v| v.usize() as u64));
        f.word(e.twin().expect("an edge's twin").usize() as u64);
        f.word(e.next().expect("an edge's next").usize() as u64);
        f.word(e.prev().expect("an edge's prev").usize() as u64);
        f.word(u64::from(e.is_primary()));
        f.word(u64::from(e.is_linear()));
    }
    (d.num_cells(), d.num_vertices(), d.num_edges(), f.0)
}

fn reference() -> BTreeMap<String, (usize, usize, usize, u64)> {
    std::fs::read_to_string(format!("{DATA}/cpp_reference_digests.txt"))
        .expect("reference digests")
        .lines()
        .map(|l| {
            let t: Vec<&str> = l.split_whitespace().collect();
            let n = |i: usize| t[i].parse::<usize>().expect("a count");
            (
                t[0].to_string(),
                (
                    n(1),
                    n(2),
                    n(3),
                    u64::from_str_radix(t[4], 16).expect("a digest"),
                ),
            )
        })
        .collect()
}

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }
    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next() % (hi - lo + 1) as u32) as i32
    }
}

/// Rectangles on a grid (some left out), some with a cut corner and some with an extra vertex
/// on their left side: many vertical and collinear segments, none crossing. Must stay identical
/// to `fuzz_case` in tests/reference/boost_digest.cpp.
fn fuzz_case(seed: u64) -> Vec<[i32; 4]> {
    let mut r = Lcg(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15).wrapping_add(1));
    let mut segs = Vec::new();
    let (n, m) = (r.range(2, 9), r.range(2, 9));
    let w = [1000, 100_000, 50_000_000][r.range(0, 2) as usize];
    for i in 0..n {
        for j in 0..m {
            if r.range(0, 9) < 3 {
                continue;
            }
            let x0 = i * w + r.range(1, w / 4);
            let x1 = (i + 1) * w - r.range(1, w / 4);
            let y0 = j * w + r.range(1, w / 4);
            let y1 = (j + 1) * w - r.range(1, w / 4);
            let mut pts = vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
            if r.range(0, 1) == 1 {
                let c = r.range(1, ((x1 - x0) / 3).max(1));
                pts = vec![[x0, y0], [x1 - c, y0], [x1, y0 + c], [x1, y1], [x0, y1]];
            }
            if r.range(0, 9) < 3 {
                let k = r.range(1, ((y1 - y0) / 3).max(1));
                pts.push([x0, y1 - k]);
            }
            for a in 0..pts.len() {
                let (p, q) = (pts[a], pts[(a + 1) % pts.len()]);
                if p != q {
                    segs.push([p[0], p[1], q[0], q[1]]);
                }
            }
        }
    }
    segs
}

/// A real part's wall section (1234 segments): the first input on which the port's skip-list
/// beach line stopped inside the comparator's non-monotone stretch and lost circle events.
#[test]
fn part28_wall_section_matches_cpp_boost() {
    let segments: Vec<[i32; 4]> =
        std::fs::read_to_string(format!("{DATA}/part28_wall_segments.txt"))
            .expect("segments")
            .lines()
            .map(|l| {
                let v: Vec<i32> = l
                    .split_whitespace()
                    .map(|t| t.parse().expect("an i32"))
                    .collect();
                [v[0], v[1], v[2], v[3]]
            })
            .collect();
    assert_eq!(segments.len(), 1234);
    assert_eq!(digest(&segments), reference()["part28"]);
}

#[test]
fn fuzzed_rectilinear_sets_match_cpp_boost() {
    let reference = reference();
    let mut mismatched = Vec::new();
    for seed in 0..400 {
        let name = format!("case{seed}");
        if digest(&fuzz_case(seed)) != reference[&name] {
            mismatched.push(name);
        }
    }
    assert!(
        mismatched.is_empty(),
        "differ from C++ Boost.Polygon: {mismatched:?}"
    );
}
