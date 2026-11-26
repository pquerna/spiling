// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;
use geo::{Area, Contains, Distance, Euclidean, Intersects, Line, Rect, coord};

const CELL: f64 = 0.25;
/// Cells this close to the decision boundary are not judged.
const SLACK: f64 = 1.0e-6;

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next()
    }
}

fn raster<'a>(frame: CellFrame, cancel: &'a AtomicBool) -> Raster<'a> {
    static ADMIT: fn(usize) -> Result<(), ManufacturingError> = |_| Ok(());
    Raster {
        frame,
        cancel,
        admit: &ADMIT,
        work: AtomicUsize::new(0),
    }
}

fn frame() -> CellFrame {
    CellFrame::new([[-1.0, -1.0], [11.0, 11.0]], CELL).unwrap()
}

fn cell_rect(frame: &CellFrame, col: usize, row: usize) -> Polygon<f64> {
    Rect::new(
        coord! { x: frame.x(col as f64), y: frame.y(row as f64) },
        coord! { x: frame.x(col as f64 + 1.0), y: frame.y(row as f64 + 1.0) },
    )
    .to_polygon()
}

/// Exact closed distance between a cell and a polygon region.
fn distance(cell: &Polygon<f64>, polygon: &Polygon<f64>) -> f64 {
    if cell.intersects(polygon) {
        return 0.0;
    }
    edges(polygon)
        .map(|[a, b]| {
            Euclidean.distance(
                &Line::new(coord! {x: a[0], y: a[1]}, coord! {x: b[0], y: b[1]}),
                cell,
            )
        })
        .fold(f64::INFINITY, f64::min)
}

/// Distance from a cell to the polygon's boundary.
fn boundary_distance(cell: &Polygon<f64>, polygon: &Polygon<f64>) -> f64 {
    edges(polygon)
        .map(|[a, b]| {
            Euclidean.distance(
                &Line::new(coord! {x: a[0], y: a[1]}, coord! {x: b[0], y: b[1]}),
                cell,
            )
        })
        .fold(f64::INFINITY, f64::min)
}

/// A random star polygon around `centre`, optionally with a star hole.
fn star(rng: &mut Lcg, centre: [f64; 2], radius: f64, points: usize, hole: bool) -> Polygon<f64> {
    let ring = |rng: &mut Lcg, scale: f64, reverse: bool| {
        let mut coords: Vec<geo::Coord<f64>> = (0..points)
            .map(|i| {
                let angle =
                    std::f64::consts::TAU * (i as f64 + rng.range(0.0, 0.8)) / points as f64;
                let r = scale * rng.range(0.4, 1.0);
                coord! { x: centre[0] + r * angle.cos(), y: centre[1] + r * angle.sin() }
            })
            .collect();
        if reverse {
            coords.reverse();
        }
        LineString::from(coords)
    };
    let exterior = ring(rng, radius, false);
    let holes = if hole {
        vec![ring(rng, radius * 0.3, true)]
    } else {
        Vec::new()
    };
    Polygon::new(exterior, holes)
}

fn adversarial() -> Vec<Polygon<f64>> {
    let poly = |points: &[[f64; 2]]| {
        Polygon::new(
            LineString::from(
                points
                    .iter()
                    .map(|p| coord! {x: p[0], y: p[1]})
                    .collect::<Vec<_>>(),
            ),
            Vec::new(),
        )
    };
    vec![
        // Diagonal sliver far thinner than a cell.
        poly(&[[1.0, 1.0], [9.0, 8.0], [9.0, 8.01]]),
        // Axis sliver lying exactly on a cell line.
        poly(&[[2.0, 5.0], [8.0, 5.0], [8.0, 5.002], [2.0, 5.002]]),
        // Vertices exactly on cell corners and midlines.
        poly(&[[0.0, 0.0], [5.0, 0.0], [5.0, 0.125], [0.0, 0.125]]),
        // Square with a square hole sharing grid lines.
        Polygon::new(
            LineString::from(vec![(1.0, 1.0), (9.0, 1.0), (9.0, 9.0), (1.0, 9.0)]),
            vec![LineString::from(vec![
                (3.0, 3.0),
                (3.0, 7.0),
                (7.0, 7.0),
                (7.0, 3.0),
            ])],
        ),
        // Hole leaving a ring thinner than a cell.
        Polygon::new(
            LineString::from(vec![(2.0, 2.0), (8.0, 2.0), (8.0, 8.0), (2.0, 8.0)]),
            vec![LineString::from(vec![
                (2.1, 2.1),
                (2.1, 7.9),
                (7.9, 7.9),
                (7.9, 2.1),
            ])],
        ),
    ]
}

fn cases() -> Vec<Polygon<f64>> {
    let mut rng = Lcg(7);
    let mut cases = adversarial();
    for i in 0..12 {
        let centre = [rng.range(3.0, 7.0), rng.range(3.0, 7.0)];
        let radius = rng.range(0.5, 4.0);
        cases.push(star(&mut rng, centre, radius, 5 + i, i % 2 == 0));
    }
    cases
}

#[test]
fn outward_and_dilation_match_exact_closed_distance() {
    let frame = frame();
    let cancel = AtomicBool::new(false);
    let raster = raster(frame, &cancel);
    for polygon in cases() {
        for d in [0.0, 0.1, 0.37, 1.3] {
            let bits = raster.outward([&polygon], d).unwrap();
            for row in 0..frame.rows {
                for col in 0..frame.cols {
                    let exact = distance(&cell_rect(&frame, col, row), &polygon);
                    if (exact - d).abs() < SLACK {
                        continue;
                    }
                    assert_eq!(
                        bits.contains(col, row),
                        exact < d || exact == 0.0,
                        "cell ({col}, {row}) at distance {exact} for d {d}"
                    );
                }
            }
        }
    }
}

#[test]
fn inward_keeps_exactly_the_cells_inside() {
    let frame = frame();
    let cancel = AtomicBool::new(false);
    let raster = raster(frame, &cancel);
    for polygon in cases() {
        let bits = raster.inward([&polygon]).unwrap();
        for row in 0..frame.rows {
            for col in 0..frame.cols {
                let cell = cell_rect(&frame, col, row);
                if bits.contains(col, row) {
                    assert!(polygon.contains(&cell), "cell ({col}, {row}) not inside");
                } else if boundary_distance(&cell, &polygon) > SLACK {
                    assert!(
                        !polygon.contains(&cell),
                        "cell ({col}, {row}) inside but dropped"
                    );
                }
            }
        }
    }
}

#[test]
fn slivers_thinner_than_a_cell_round_to_the_safe_side() {
    let frame = frame();
    let cancel = AtomicBool::new(false);
    let raster = raster(frame, &cancel);
    let sliver = &adversarial()[0];
    let outward = raster.outward([sliver], 0.0).unwrap();
    // Continuous: the sliver crosses about 32 columns and 28 rows.
    assert!(outward.count() >= 32);
    assert!(raster.inward([sliver]).unwrap().is_empty());
    // Overlapping pieces union, never cancel.
    let both = raster.outward([sliver, sliver], 0.0).unwrap();
    assert_eq!(both, outward);
}

fn brute(frame: &CellFrame, bits: &Bits) -> Vec<bool> {
    (0..frame.rows)
        .flat_map(|row| (0..frame.cols).map(move |col| (col, row)))
        .map(|(col, row)| bits.contains(col, row))
        .collect()
}

#[test]
fn set_algebra_matches_cellwise_logic() {
    let frame = frame();
    let cancel = AtomicBool::new(false);
    let raster = raster(frame, &cancel);
    let polygons = cases();
    for pair in polygons.windows(2) {
        let a = raster.outward([&pair[0]], 0.2).unwrap();
        let b = raster.outward([&pair[1]], 0.0).unwrap();
        let (ca, cb) = (brute(&frame, &a), brute(&frame, &b));
        let union = brute(&frame, &raster.union(&a, &b).unwrap());
        let meet = brute(&frame, &raster.intersection(&a, &b).unwrap());
        let mut difference = a.clone();
        difference.subtract(&b);
        let difference = brute(&frame, &difference);
        for i in 0..ca.len() {
            assert_eq!(union[i], ca[i] || cb[i]);
            assert_eq!(meet[i], ca[i] && cb[i]);
            assert_eq!(difference[i], ca[i] && !cb[i]);
        }
        assert_eq!(a.intersects(&b), meet.iter().any(|v| *v));
    }
    // Equal sets compare equal whatever produced them.
    let a = raster.outward([&polygons[3]], 0.0).unwrap();
    let mut b = raster
        .union(&a, &raster.outward([&polygons[0]], 0.0).unwrap())
        .unwrap();
    b.subtract(&raster.outward([&polygons[0]], 0.0).unwrap());
    let mut a2 = a.clone();
    a2.subtract(&raster.outward([&polygons[0]], 0.0).unwrap());
    assert_eq!(a2, b);
}

#[test]
fn erosion_matches_brute_force_closed_distance() {
    let frame = frame();
    let cancel = AtomicBool::new(false);
    let raster = raster(frame, &cancel);
    for polygon in cases().iter().take(9) {
        let bits = raster.outward([polygon], 0.3).unwrap();
        for radius in [0.0, CELL, 1.7 * CELL, 3.2 * CELL] {
            let eroded = raster.erode(&bits, radius).unwrap();
            let reach = (radius / CELL).floor() as i64 + 1;
            for row in 0..frame.rows as i64 {
                for col in 0..frame.cols as i64 {
                    let mut keep = bits.contains(col as usize, row as usize);
                    for dr in -reach..=reach {
                        for dc in -reach..=reach {
                            let gap = |d: i64| (d.abs() - 1).max(0) as f64 * CELL;
                            if gap(dr).hypot(gap(dc)) > radius {
                                continue;
                            }
                            let (r, c) = (row + dr, col + dc);
                            let set = r >= 0 && c >= 0 && bits.contains(c as usize, r as usize);
                            keep &= set;
                        }
                    }
                    assert_eq!(eroded.contains(col as usize, row as usize), keep);
                }
            }
        }
    }
}

#[test]
fn contours_stay_inside_the_cells_with_few_vertices() {
    let frame = frame();
    let cancel = AtomicBool::new(false);
    let raster = raster(frame, &cancel);
    let mut rng = Lcg(11);
    for polygon in cases() {
        let bits = raster.outward([&polygon], 0.1).unwrap();
        let contour = raster.contour(&bits).unwrap();
        let core = raster.erode(&bits, 4.0 * CELL).unwrap();
        // Everything deep inside survives.
        let mut covered = 0.0;
        for row in 0..frame.rows {
            for col in 0..frame.cols {
                if core.contains(col, row) {
                    let centre =
                        geo::Point::new(frame.x(col as f64 + 0.5), frame.y(row as f64 + 0.5));
                    assert!(contour.contains(&centre), "deep cell ({col}, {row}) lost");
                    covered += 1.0;
                }
            }
        }
        assert!(contour.unsigned_area() >= covered * CELL * CELL * 0.99);
        // Every contour point lies in a set cell.
        let [lo, hi] = frame.bounds();
        for _ in 0..4000 {
            let point = geo::Point::new(rng.range(lo[0], hi[0]), rng.range(lo[1], hi[1]));
            if contour.contains(&point) {
                let col = ((point.x() - lo[0]) / CELL).floor() as usize;
                let row = ((point.y() - lo[1]) / CELL).floor() as usize;
                assert!(bits.contains(col, row), "contour point outside the cells");
            }
        }
        for polygon in &contour.0 {
            for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
                for p in &ring.0 {
                    let col = (p.x - lo[0]) / CELL;
                    let row = (p.y - lo[1]) / CELL;
                    let near = [(0.0, 0.0), (-1e-9, 0.0), (0.0, -1e-9), (-1e-9, -1e-9)];
                    assert!(
                        near.iter().any(|(dc, dr)| bits
                            .contains((col + dc).floor() as usize, (row + dr).floor() as usize)),
                        "contour vertex outside the cells"
                    );
                }
            }
        }
    }
}

#[test]
fn boundaries_reach_the_set_edge_and_every_edge_stays_in_set_cells() {
    let frame = frame();
    let cancel = AtomicBool::new(false);
    let raster = raster(frame, &cancel);
    let [lo, _] = frame.bounds();
    for polygon in cases() {
        let bits = &raster.outward([&polygon], 0.1).unwrap();
        let boundary = raster.boundary(bits, 0.0).unwrap();
        let contour = raster.contour(bits).unwrap();
        assert!(boundary.unsigned_area() >= contour.unsigned_area());
        // Every boundary edge, sampled densely, stays in closed set cells.
        for polygon in &boundary.0 {
            for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
                for edge in ring.0.windows(2) {
                    for k in 0..=64 {
                        let t = f64::from(k) / 64.0;
                        let x = edge[0].x + (edge[1].x - edge[0].x) * t;
                        let y = edge[0].y + (edge[1].y - edge[0].y) * t;
                        let (col, row) = ((x - lo[0]) / CELL, (y - lo[1]) / CELL);
                        let inside = [-1e-9, 0.0, 1e-9].iter().any(|dc| {
                            [-1e-9, 0.0, 1e-9].iter().any(|dr| {
                                let (c, r) = ((col + dc).floor(), (row + dr).floor());
                                c >= 0.0 && r >= 0.0 && bits.contains(c as usize, r as usize)
                            })
                        });
                        assert!(inside, "boundary edge leaves the cells at ({x}, {y})");
                    }
                }
            }
        }
    }
}

#[test]
fn contour_of_a_disk_has_few_vertices_and_separates_diagonal_cells() {
    let frame = CellFrame::new([[-6.0, -6.0], [6.0, 6.0]], 0.05).unwrap();
    let cancel = AtomicBool::new(false);
    let raster = raster(frame, &cancel);
    let disk = Polygon::new(
        LineString::from(
            (0..720)
                .map(|i| {
                    let a = std::f64::consts::TAU * i as f64 / 720.0;
                    (5.0 * a.cos(), 5.0 * a.sin())
                })
                .collect::<Vec<_>>(),
        ),
        Vec::new(),
    );
    let bits = raster.outward([&disk], 0.0).unwrap();
    let contour = raster.contour(&bits).unwrap();
    assert_eq!(contour.0.len(), 1);
    let vertices = contour.0[0].exterior().0.len();
    assert!(vertices < 200, "{vertices} vertices");
    let expected = std::f64::consts::PI * 25.0;
    assert!((contour.unsigned_area() - expected).abs() < expected * 0.06);

    // Two blocks touching at one corner trace as two rings, a block with a
    // hole as a counterclockwise exterior and a clockwise hole.
    let block = |c0: u32, r0: u32, n: u32| {
        (r0..r0 + n).map(move |row| Span {
            row,
            from: c0,
            to: c0 + n,
        })
    };
    let blocks = raster
        .bits_of(
            &block(10, 10, 40)
                .chain(block(50, 50, 40))
                .collect::<Vec<_>>(),
        )
        .unwrap();
    let rings = raster.trace(&blocks).unwrap();
    assert_eq!(rings.len(), 2);
    assert!(rings.iter().all(|ring| ring.len() == 4));
    let contour = raster.contour(&blocks).unwrap();
    assert_eq!(contour.0.len(), 2);
    for polygon in &contour.0 {
        assert_eq!(polygon.exterior().0.len(), 5);
    }
    let mut framed = raster
        .bits_of(&block(10, 10, 40).collect::<Vec<_>>())
        .unwrap();
    framed.subtract(
        &raster
            .bits_of(&block(20, 20, 20).collect::<Vec<_>>())
            .unwrap(),
    );
    let rings = raster.trace(&framed).unwrap();
    let signed = |ring: &Vec<[i64; 2]>| {
        (0..ring.len())
            .map(|i| {
                let (p, q) = (ring[i], ring[(i + 1) % ring.len()]);
                p[0] * q[1] - q[0] * p[1]
            })
            .sum::<i64>()
    };
    let mut areas: Vec<i64> = rings.iter().map(signed).collect();
    areas.sort_unstable();
    assert_eq!(areas, vec![-2 * 400, 2 * 1600]);
    let contour = raster.contour(&framed).unwrap();
    assert_eq!(contour.0.len(), 1);
    assert_eq!(contour.0[0].interiors().len(), 1);
}

#[test]
fn frame_refuses_unrepresentable_bounds_and_cancels() {
    assert!(CellFrame::new([[0.0, 0.0], [1.0e12, 1.0]], 0.05).is_err());
    assert!(CellFrame::new([[0.0, f64::NAN], [1.0, 1.0]], 0.05).is_err());
    let cancel = AtomicBool::new(true);
    let raster = raster(frame(), &cancel);
    let square = &adversarial()[3];
    assert_eq!(
        raster.outward([square], 0.1).unwrap_err().code,
        ManufacturingErrorCode::Cancelled
    );
    let refuse: fn(usize) -> Result<(), ManufacturingError> =
        |_| Err(error(ManufacturingErrorCode::ResourceLimit, "refused"));
    let refusing = Raster {
        frame: frame(),
        cancel: &AtomicBool::new(false),
        admit: &refuse,
        work: AtomicUsize::new(0),
    };
    assert_eq!(
        refusing.outward([square], 0.1).unwrap_err().code,
        ManufacturingErrorCode::ResourceLimit
    );
}

#[test]
fn escaped_cells_are_exactly_the_outward_capsule_outside_the_kept_set() {
    let cancel = AtomicBool::new(false);
    let frame = frame();
    let raster = raster(frame, &cancel);
    let kept = raster.inward(&cases()[..1]).unwrap();
    let mut rng = Lcg(17);
    for _ in 0..200 {
        let a = [rng.range(0.0, 10.0), rng.range(0.0, 10.0)];
        let b = [rng.range(0.0, 10.0), rng.range(0.0, 10.0)];
        let radius = rng.range(0.0, 0.6);
        let cover = raster.capsules([(a, b, radius)]).unwrap();
        let mut outside = cover.clone();
        outside.subtract(&kept);
        let (escape, work) = frame.escaped(&kept, a, b, radius).unwrap();
        assert!(work > 0);
        assert_eq!(escape.map_or(0, |e| e.cells), outside.count());
        if let Some(escape) = escape {
            for (index, inside) in brute(&frame, &outside).into_iter().enumerate() {
                if inside {
                    let (col, row) = (index % frame.cols, index / frame.cols);
                    let [lo, hi] = escape.bounds;
                    assert!(lo[0] <= frame.x(col as f64) && frame.x(col as f64 + 1.0) <= hi[0]);
                    assert!(lo[1] <= frame.y(row as f64) && frame.y(row as f64 + 1.0) <= hi[1]);
                }
            }
        }
    }
    // Cells beyond the frame are never kept, even for a full kept set.
    let all = raster
        .inward([&Rect::new(coord! { x: -5.0, y: -5.0 }, coord! { x: 20.0, y: 20.0 }).to_polygon()])
        .unwrap();
    let (inside, _) = frame.escaped(&all, [1.0, 1.0], [9.0, 9.0], 0.3).unwrap();
    assert_eq!(inside, None);
    let (escape, _) = frame.escaped(&all, [10.5, 1.0], [12.0, 1.0], 0.1).unwrap();
    let escape = escape.unwrap();
    assert!(escape.cells > 0 && escape.bounds[1][0] >= 12.1);
    assert!(
        frame
            .escaped(&all, [f64::NAN, 0.0], [1.0, 1.0], 0.1)
            .is_err()
    );
}

#[test]
fn closing_seams_fills_two_cell_gaps_only() {
    let cancel = AtomicBool::new(false);
    let raster = raster(frame(), &cancel);
    // Rectangles a hair outside cell boundaries keep their edge cells inward.
    let rect = |x: [f64; 2], y: [f64; 2]| {
        Rect::new(
            coord! { x: x[0] - 0.01, y: y[0] - 0.01 },
            coord! { x: x[1] + 0.01, y: y[1] + 0.01 },
        )
        .to_polygon()
    };
    // Rows: a two-cell gap (1.0..1.5), then a three-cell gap (2.5..3.25).
    // Columns: a two-cell gap (6.0..6.5) beside a three-cell one (8.0..8.75).
    let pieces = [
        rect([0.0, 4.0], [0.0, 1.0]),
        rect([0.0, 4.0], [1.5, 2.5]),
        rect([0.0, 4.0], [3.25, 4.25]),
        rect([5.0, 6.0], [6.0, 7.0]),
        rect([6.5, 8.0], [6.0, 7.0]),
        rect([8.75, 9.5], [6.0, 7.0]),
    ];
    let bits = raster.inward(&pieces).unwrap();
    let closed = raster.close_seams(&bits).unwrap();
    let set = |x: f64, y: f64| raster.holds_all(&closed, [x, y]);
    for y in [1.125, 1.375] {
        assert!(set(2.125, y), "two-row seam at y {y} stays open");
    }
    for y in [2.625, 2.875, 3.125] {
        assert!(!set(2.125, y), "three-row gap at y {y} was filled");
    }
    for x in [6.125, 6.375] {
        assert!(set(x, 6.625), "two-column seam at x {x} stays open");
    }
    for x in [8.125, 8.375, 8.625] {
        assert!(!set(x, 6.625), "three-column gap at x {x} was filled");
    }
    // Closing never grows a set beyond its own extent.
    for p in [
        [2.125, -0.125],
        [4.125, 0.625],
        [-0.125, 0.625],
        [9.625, 6.625],
    ] {
        assert!(!set(p[0], p[1]), "closing grew outward at {p:?}");
    }
    assert!(pieces.iter().all(|piece| {
        raster
            .difference(&raster.inward([piece]).unwrap(), &closed)
            .unwrap()
            .is_empty()
    }));
}

#[test]
fn dilation_components_selection_and_windows_keep_their_cells() {
    let cancel = AtomicBool::new(false);
    let frame = frame();
    let raster = raster(frame, &cancel);
    let square = |x0: f64, y0: f64, x1: f64, y1: f64| {
        Rect::new(coord! { x: x0, y: y0 }, coord! { x: x1, y: y1 }).to_polygon()
    };
    let a = raster.inward([&square(1.0, 1.0, 2.0, 2.0)]).unwrap();
    let b = raster.inward([&square(6.0, 6.0, 7.0, 7.0)]).unwrap();
    let both = raster.union(&a, &b).unwrap();

    // Dilation is the dual of erosion and reaches exactly its radius.
    let grown = raster.dilate(&a, 0.5).unwrap();
    assert!(raster.holds_all(&grown, [2.4, 1.5]));
    assert!(!raster.holds_any(&grown, [2.7, 1.5]));
    assert_eq!(raster.erode(&grown, 0.5).unwrap(), a);

    // Two separate squares are two components, in first-cell order.
    let parts = raster.components(&both).unwrap();
    assert_eq!(parts, vec![a.clone(), b.clone()]);
    // Two blocks meeting only at a corner are one component
    // (8-connectivity).
    let big = raster.inward([&square(0.0, 0.0, 10.0, 10.0)]).unwrap();
    let mid = frame.cell_of([5.0, 5.0]).unwrap();
    let diagonal = raster
        .select(&big, |c, r| (c < mid[0]) == (r < mid[1]))
        .unwrap();
    assert_eq!(raster.components(&diagonal).unwrap().len(), 1);
    let apart = raster
        .select(&big, |c, r| {
            (c + 1 < mid[0] && r + 1 < mid[1]) || (c > mid[0] && r > mid[1])
        })
        .unwrap();
    assert_eq!(raster.components(&apart).unwrap().len(), 2);

    // Selection keeps exactly the cells the predicate keeps.
    let left = raster
        .select(&both, |c, r| frame.centre(c, r)[0] < 4.0)
        .unwrap();
    assert_eq!(left, a);

    // The window around `a` grown by a radius holds everything a dilation
    // of that radius can bring into it, and nothing far away.
    let near = raster.near_window(&both, &a, 0.5).unwrap();
    assert_eq!(near, a);
    let far = raster.near_window(&both, &a, 6.0).unwrap();
    assert_eq!(far, both);
}
