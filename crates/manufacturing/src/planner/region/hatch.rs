// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

use super::*;
use spiling_compute::{MAX_BATCH_ITEMS, MAX_MASK_WORDS, MIN_BATCH_PAIRS, MaskRows};

pub(super) struct PreparedHatch {
    edges: Vec<[[f64; 2]; 2]>,
    ranges: Vec<[f64; 2]>,
}

impl PreparedHatch {
    pub(super) fn new(
        polygon: &Polygon<f64>,
        rows: &HatchRows,
        axis: HatchAxis,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, ManufacturingError> {
        Self::for_queries(polygon, rows.count, axis, cancel)
    }

    pub(super) fn for_queries(
        polygon: &Polygon<f64>,
        queries: usize,
        axis: HatchAxis,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, ManufacturingError> {
        check_cancel(cancel)?;
        let edge_count = std::iter::once(polygon.exterior())
            .chain(polygon.interiors())
            .map(|ring| ring.0.len().saturating_sub(1))
            .sum::<usize>();
        if edge_count == 0
            || edge_count > MAX_BATCH_ITEMS
            || queries.min(256).saturating_mul(edge_count) < MIN_BATCH_PAIRS
        {
            return Ok(None);
        }
        let mut edges = Vec::new();
        let mut ranges = Vec::new();
        edges
            .try_reserve_exact(edge_count)
            .map_err(|_| resource("compute hatch edge allocation refused"))?;
        ranges
            .try_reserve_exact(edge_count)
            .map_err(|_| resource("compute hatch range allocation refused"))?;
        let swap = axis == HatchAxis::Y;
        for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
            for edge in ring.0.windows(2) {
                check_cancel(cancel)?;
                let start = if swap {
                    [edge[0].y, edge[0].x]
                } else {
                    [edge[0].x, edge[0].y]
                };
                let end = if swap {
                    [edge[1].y, edge[1].x]
                } else {
                    [edge[1].x, edge[1].y]
                };
                edges.push([start, end]);
                ranges.push([start[1], end[1]]);
            }
        }
        if edges.len() != edge_count {
            return Err(unsupported("compute hatch edge inventory mismatch"));
        }
        Ok(Some(Self { edges, ranges }))
    }

    pub(super) fn tile_rows(&self) -> usize {
        (MAX_MASK_WORDS / self.edges.len().div_ceil(32)).clamp(1, 256)
    }

    pub(super) fn mask(
        &self,
        first: usize,
        count: usize,
        rows: &HatchRows,
        cancel: &AtomicBool,
    ) -> Result<Option<MaskRows>, ManufacturingError> {
        let mut coordinates = Vec::new();
        coordinates
            .try_reserve_exact(count)
            .map_err(|_| resource("compute hatch row allocation refused"))?;
        for index in first..first + count {
            check_cancel(cancel)?;
            coordinates.push(rows.start + index as f64 * rows.spacing);
        }
        self.mask_coordinates(&coordinates, cancel)
    }

    pub(super) fn mask_coordinates(
        &self,
        coordinates: &[f64],
        cancel: &AtomicBool,
    ) -> Result<Option<MaskRows>, ManufacturingError> {
        spiling_compute::row_crossings(&self.ranges, coordinates, cancel)
            .map_err(crate::compute_error)
    }

    pub(super) fn intersections(
        &self,
        mask: &MaskRows,
        row: usize,
        coordinate: f64,
        hits: &mut Vec<f64>,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        if mask.columns() != self.edges.len() {
            return Err(unsupported("compute hatch mask has incorrect columns"));
        }
        let selected = mask
            .selected_columns(row)
            .ok_or_else(|| unsupported("compute hatch mask has no requested row"))?;
        for index in selected {
            check_cancel(cancel)?;
            let [start, end] = self
                .edges
                .get(index)
                .ok_or_else(|| unsupported("compute hatch selected edge outside inventory"))?;
            hits.push(
                start[0] + (coordinate - start[1]) * (end[0] - start[0]) / (end[1] - start[1]),
            );
        }
        hits.sort_unstable_by(f64::total_cmp);
        if !hits.len().is_multiple_of(2) {
            return Err(unsupported(
                "rectilinear hatch clipping has odd boundary parity",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cubecl_hatch_masks_preserve_original_interpolation_and_holes() {
        let cancel = AtomicBool::new(false);
        let ring = |radius: f64, reverse: bool| {
            let mut points = (0..=256)
                .map(|index| {
                    let angle = std::f64::consts::TAU * f64::from(index % 256) / 256.0;
                    Coord {
                        x: radius * angle.cos(),
                        y: radius * angle.sin(),
                    }
                })
                .collect::<Vec<_>>();
            if reverse {
                points.reverse();
            }
            LineString::new(points)
        };
        let polygon = Polygon::new(ring(20.0, false), vec![ring(7.0, true)]);
        let rows = HatchRows {
            count: 256,
            start: -20.0,
            spacing: 40.0 / 255.0,
            edges: 512,
        };
        for axis in [HatchAxis::X, HatchAxis::Y] {
            let prepared = PreparedHatch::new(&polygon, &rows, axis, &cancel)
                .unwrap()
                .unwrap();
            let mask = prepared
                .mask(0, rows.count, &rows, &cancel)
                .unwrap()
                .unwrap();
            for row in 0..rows.count {
                let coordinate = rows.start + row as f64 * rows.spacing;
                let mut scalar = Vec::new();
                let mut actual = Vec::new();
                row_intersections(&polygon, coordinate, axis, &mut scalar, &cancel).unwrap();
                prepared
                    .intersections(&mask, row, coordinate, &mut actual, &cancel)
                    .unwrap();
                assert_eq!(scalar, actual);
            }
            let recipe = crate::planner::tests::recipe(1.0);
            let layer = HatchLayer {
                roof: None,
                width_mm: recipe.bead_width_mm,
                z: recipe.layer_height_mm,
                lower_z: 0.0,
                height: recipe.layer_height_mm,
                axis,
                top_axis: axis,
                recipe: &recipe,
            };
            let full_rows = HatchRows {
                count: 257,
                start: -19.5,
                spacing: 39.0 / 256.0,
                ..rows
            };
            let mut expected = Vec::new();
            let mut expected_segments = 0;
            let mut actual = Vec::new();
            let mut actual_segments = 0;
            for row in 0..full_rows.count {
                let coordinate = full_rows.start + row as f64 * full_rows.spacing;
                let mut hits = Vec::new();
                row_intersections(&polygon, coordinate, axis, &mut hits, &cancel).unwrap();
                append_hatch_row(
                    &hits,
                    coordinate,
                    DepositionKind::SolidFill,
                    &layer,
                    row % 2 == 1,
                    &mut expected,
                    &mut expected_segments,
                )
                .unwrap();
            }
            hatch_polygon(
                &polygon,
                DepositionKind::SolidFill,
                &full_rows,
                &layer,
                &mut actual,
                &mut actual_segments,
                &cancel,
            )
            .unwrap();
            assert_eq!(actual_segments, expected_segments);
            assert_eq!(
                serde_json::to_value(actual).unwrap(),
                serde_json::to_value(expected).unwrap()
            );
        }
    }

    #[test]
    fn small_hatch_keeps_allocation_free_preparation_and_cancellation_refuses() {
        let polygon = Polygon::new(
            LineString::from(vec![
                (0.0, 0.0),
                (4.0, 0.0),
                (4.0, 4.0),
                (0.0, 4.0),
                (0.0, 0.0),
            ]),
            vec![],
        );
        let rows = HatchRows {
            count: 4,
            start: 0.5,
            spacing: 1.0,
            edges: 4,
        };
        assert!(
            PreparedHatch::new(&polygon, &rows, HatchAxis::X, &AtomicBool::new(false))
                .unwrap()
                .is_none()
        );
        let rows = HatchRows {
            count: 2048,
            ..rows
        };
        let failure = PreparedHatch::new(&polygon, &rows, HatchAxis::X, &AtomicBool::new(true))
            .err()
            .unwrap();
        assert_eq!(failure.code, ManufacturingErrorCode::Cancelled);
    }

    #[test]
    fn cubecl_bridge_tiles_preserve_irregular_rows_holes_exclusions_and_owners() {
        let ring = |radius: f64| {
            let corners = [
                [-radius, -radius],
                [radius, -radius],
                [radius, radius],
                [-radius, radius],
                [-radius, -radius],
            ];
            let mut points = Vec::new();
            for edge in corners.windows(2) {
                for index in 0..128 {
                    let fraction = f64::from(index) / 128.0;
                    points.push(Coord {
                        x: edge[0][0] + fraction * (edge[1][0] - edge[0][0]),
                        y: edge[0][1] + fraction * (edge[1][1] - edge[0][1]),
                    });
                }
            }
            points.push(points[0]);
            LineString::new(points)
        };
        let fill = MultiPolygon::new(vec![Polygon::new(ring(20.0), vec![ring(8.0)])]);
        let recipe = crate::planner::tests::recipe(1.0);
        let cancel = AtomicBool::new(false);
        for axis in [HatchAxis::X, HatchAxis::Y] {
            let layer = HatchLayer {
                roof: None,
                width_mm: recipe.bead_width_mm,
                z: recipe.layer_height_mm,
                lower_z: 0.0,
                height: recipe.layer_height_mm,
                axis,
                top_axis: axis,
                recipe: &recipe,
            };
            let point = |along: f64, fixed: f64| {
                if axis == HatchAxis::X {
                    (along, fixed)
                } else {
                    (fixed, along)
                }
            };
            let exclusion = Polygon::new(
                LineString::from(vec![
                    point(-4.0, -20.0),
                    point(4.0, -20.0),
                    point(4.0, 20.0),
                    point(-4.0, 20.0),
                    point(-4.0, -20.0),
                ]),
                vec![],
            );
            let mut required = Vec::new();
            let mut expected = Vec::new();
            let mut required_segments = 0;
            let mut expected_segments = 0;
            for index in 0..300 {
                let coordinate = (f64::from(index) - 150.0) / 8.0;
                append_hatch_row(
                    &[-19.0, -18.0, -17.0, -16.0],
                    coordinate,
                    DepositionKind::SparseFill,
                    &layer,
                    false,
                    &mut required,
                    &mut required_segments,
                )
                .unwrap();
                let end = if (-8.0..8.0).contains(&coordinate) {
                    -8.0
                } else {
                    -4.0
                };
                append_hatch_row(
                    &[-20.0, end],
                    coordinate,
                    DepositionKind::Bridge,
                    &layer,
                    false,
                    &mut expected,
                    &mut expected_segments,
                )
                .unwrap();
            }
            assert!(
                PreparedHatch::for_queries(&fill.0[0], required.len(), axis, &cancel)
                    .unwrap()
                    .is_some()
            );
            let mut actual_segments = 0;
            let actual = bridge_rows(
                &fill,
                &[MultiPolygon::new(vec![exclusion])],
                &required,
                &layer,
                &mut actual_segments,
                &cancel,
            )
            .unwrap();
            assert_eq!(actual_segments, expected_segments);
            assert_eq!(
                serde_json::to_value(actual).unwrap(),
                serde_json::to_value(expected).unwrap()
            );
        }
    }
}
