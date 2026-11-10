// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Cathedral (web + roof) inputs for a whole part: one lattice frame and, per
//! layer, the ceilings above it. A ceiling is material that ends above: a
//! step, pocket floor or hole underside. Corbels close under each one by its
//! own height, so the construction is not limited to constant sections.

use super::region::{Ceiling, Roof};
use crate::{check_cancel, error};
use geo::{Area, BooleanOps, BoundingRect, MultiPolygon, Polygon};
use spiling_contracts::manufacturing::*;
use std::sync::{Arc, atomic::AtomicBool};

pub(super) struct Field {
    roof_z_mm: f64,
    frame_mm: [[f64; 2]; 2],
    /// Layers whose ceilings did not change share one list.
    layers: Vec<Arc<Vec<Ceiling>>>,
    columns: Option<super::region::WebColumns>,
}

impl Field {
    pub(super) fn column_bytes(&self) -> usize {
        self.columns
            .as_ref()
            .map_or(0, super::region::WebColumns::allocated_bytes)
    }

    pub(super) fn stabilize(
        &mut self,
        layers: &mut [PrintLayer],
        recipe: &PlanarPrintRecipe,
        segments: &mut usize,
        owned_bytes: usize,
        cancel: &AtomicBool,
    ) -> Result<(), ManufacturingError> {
        self.columns = None;
        self.columns = super::region::web_roof::synthesize(
            layers,
            recipe,
            owned_bytes.saturating_add(super::layer_payload_bytes(layers)?),
            cancel,
        )?;
        if self.columns.is_none() {
            return Ok(());
        }
        let retained_road = |path: &DepositionPath| {
            matches!(
                path.kind,
                DepositionKind::SparseFill | DepositionKind::Bridge
            ) && matches!(path.owner, Some(DepositionOwner::Regional { .. }))
        };
        for index in 0..layers.len() {
            check_cancel(cancel)?;
            let outside = owned_bytes
                .saturating_add(super::layer_payload_bytes(layers)?)
                .saturating_add(self.column_bytes());
            let layer = &mut layers[index];
            let removed = layer
                .paths
                .iter()
                .filter(|path| retained_road(path))
                .map(|path| path.points_mm.len().saturating_sub(1))
                .sum::<usize>();
            let mut staged_segments = segments.checked_sub(removed).ok_or_else(|| {
                error(
                    ManufacturingErrorCode::ResourceLimit,
                    "Cathedral column segment accounting underflow",
                )
            })?;
            let mut replacement = Vec::new();
            for domain in &layer.region_coverage.web_roof_domains {
                let scratch = domain.allocated_bytes().saturating_mul(2);
                super::remaining_geometry_units(
                    0,
                    outside
                        .saturating_add(scratch)
                        .saturating_add(super::bonds::path_bytes(&replacement)),
                )?;
                let fill = MultiPolygon(
                    domain
                        .extent
                        .iter()
                        .map(super::region::web_roof::decode)
                        .collect(),
                );
                let first = replacement.len();
                super::region::web_roof::hatch_with_budget(
                    &fill,
                    recipe.infill_fraction,
                    &super::region::HatchLayer {
                        roof: Some(self.roof(index)),
                        width_mm: recipe.bead_width_mm,
                        z: layer.z_mm,
                        lower_z: layer.lower_z_mm,
                        height: layer.height_mm,
                        axis: super::region::HatchAxis::X,
                        top_axis: super::region::HatchAxis::X,
                        recipe,
                    },
                    &mut replacement,
                    &mut staged_segments,
                    cancel,
                    outside.saturating_add(scratch),
                )?;
                for path in &mut replacement[first..] {
                    path.owner = Some(DepositionOwner::Regional {
                        region_index: domain.region_index,
                    });
                }
                super::remaining_geometry_units(
                    0,
                    outside
                        .saturating_add(scratch)
                        .saturating_add(super::bonds::path_bytes(&replacement)),
                )?;
            }
            layer.paths.retain(|path| !retained_road(path));
            layer.paths.append(&mut replacement);
            *segments = staged_segments;
            super::remaining_geometry_units(
                0,
                owned_bytes
                    .saturating_add(super::layer_payload_bytes(layers)?)
                    .saturating_add(self.column_bytes()),
            )?;
        }
        Ok(())
    }

    pub(super) fn roof(&self, layer: usize) -> Roof<'_> {
        Roof {
            z_mm: self.roof_z_mm,
            frame_mm: self.frame_mm,
            ceilings: self.layers.get(layer).map_or(&[], |c| c.as_slice()),
            columns: self.columns.as_ref(),
        }
    }

    /// Record this layer's construction inputs for independent replay.
    pub(super) fn stamp(&self, layer: usize, domains: &mut [WebRoofDomain]) {
        let roof = self.roof(layer);
        for domain in domains {
            domain.frame_mm = roof.frame_mm;
            domain.ceilings = roof
                .ceilings
                .iter()
                .map(|ceiling| WebRoofCeiling {
                    roof_z_mm: ceiling.roof_z_mm,
                    extent: ceiling.area.0.iter().map(encode).collect(),
                })
                .collect();
        }
    }
}

fn encode(polygon: &Polygon<f64>) -> DiaphragmPolygonMm {
    DiaphragmPolygonMm {
        outer: polygon.exterior().0.iter().map(|p| [p.x, p.y]).collect(),
        holes: polygon
            .interiors()
            .iter()
            .map(|ring| ring.0.iter().map(|p| [p.x, p.y]).collect())
            .collect(),
    }
}

fn perimeter(polygon: &Polygon<f64>) -> f64 {
    std::iter::once(polygon.exterior())
        .chain(polygon.interiors())
        .flat_map(|ring| ring.0.windows(2))
        .map(|edge| (edge[1].x - edge[0].x).hypot(edge[1].y - edge[0].y))
        .sum()
}

/// `materials[k]` and `tops[k]` are layer k's section and upper Z.
pub(super) fn field(
    materials: &[&Arc<MultiPolygon<f64>>],
    tops: &[f64],
    recipe: &PlanarPrintRecipe,
    bed_z_mm: f64,
    cancel: &AtomicBool,
) -> Result<Field, ManufacturingError> {
    let refuse = |message: &str| error(ManufacturingErrorCode::UnsupportedGeometry, message);
    let top = *tops
        .last()
        .ok_or_else(|| refuse("web + roof requires a sliced part"))?;
    let below_skin = recipe.top_skin_thickness_mm + recipe.layer_height_mm;
    let roof_z_mm = (top - below_skin).max(bed_z_mm);

    let mut frame = [[f64::INFINITY; 2], [f64::NEG_INFINITY; 2]];
    let mut previous: Option<&Arc<MultiPolygon<f64>>> = None;
    for &material in materials {
        if previous.is_some_and(|p| Arc::ptr_eq(p, material)) {
            continue;
        }
        previous = Some(material);
        if let Some(bounds) = material.bounding_rect() {
            frame[0] = [
                frame[0][0].min(bounds.min().x),
                frame[0][1].min(bounds.min().y),
            ];
            frame[1] = [
                frame[1][0].max(bounds.max().x),
                frame[1][1].max(bounds.max().y),
            ];
        }
    }
    // The sparse interior starts inside the walls; anchor the lattice there
    // when the part is wide enough.
    let inset = f64::from(recipe.perimeter_count) * recipe.bead_width_mm;
    for axis in 0..2 {
        if frame[1][axis] - frame[0][axis] > 2.0 * inset + recipe.bead_width_mm {
            frame[0][axis] += inset;
            frame[1][axis] -= inset;
        }
    }
    if frame.iter().flatten().any(|v| !v.is_finite())
        || (0..2).any(|axis| frame[0][axis] >= frame[1][axis])
    {
        return Err(refuse("web + roof requires a part with positive XY extent"));
    }

    // Corbels grow 0.5 mm per millimetre of rise from at most one web gap
    // away, so a ceiling matters only this far below it.
    let nominal = recipe.layer_height_mm;
    let pitch = 2.0 * (recipe.bead_width_mm - nominal + std::f64::consts::PI * nominal / 4.0)
        / recipe.infill_fraction.max(0.01);
    let horizon = 2.0 * (pitch + recipe.web_roof_spacing_mm()) + nominal;

    let mut layers = vec![Arc::new(Vec::new()); materials.len()];
    let mut carried: Arc<Vec<Ceiling>> = Arc::new(Vec::new());
    for k in (0..materials.len()).rev() {
        check_cancel(cancel)?;
        let mut next: Option<Vec<Ceiling>> = None;
        if carried
            .iter()
            .any(|ceiling| tops[k] < ceiling.roof_z_mm - horizon)
        {
            next = Some(
                carried
                    .iter()
                    .filter(|ceiling| tops[k] >= ceiling.roof_z_mm - horizon)
                    .map(|c| Ceiling {
                        roof_z_mm: c.roof_z_mm,
                        area: c.area.clone(),
                    })
                    .collect(),
            );
        }
        if k + 1 < materials.len() && !Arc::ptr_eq(materials[k], materials[k + 1]) {
            let mut ended = materials[k].as_ref().difference(materials[k + 1].as_ref());
            // A ring thinner than a bead is a wall receding by less than a
            // bead per layer; the wall below carries it.
            ended.0.retain(|polygon| {
                let around = perimeter(polygon);
                around > 0.0 && 2.0 * polygon.unsigned_area() / around >= recipe.bead_width_mm
            });
            let roof = (tops[k] - below_skin).max(bed_z_mm);
            if !ended.0.is_empty() && roof < roof_z_mm {
                let list = next.get_or_insert_with(|| {
                    carried
                        .iter()
                        .map(|c| Ceiling {
                            roof_z_mm: c.roof_z_mm,
                            area: c.area.clone(),
                        })
                        .collect()
                });
                list.push(Ceiling {
                    roof_z_mm: roof,
                    area: ended,
                });
            }
        }
        if let Some(list) = next {
            carried = Arc::new(list);
        }
        layers[k] = Arc::clone(&carried);
    }
    Ok(Field {
        roof_z_mm,
        frame_mm: frame,
        layers,
        columns: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_step_gives_the_base_a_lower_ceiling_and_a_prism_none() {
        let mut recipe = super::super::tests::recipe(0.08);
        recipe.sparse_infill = SparseInfillPolicy::web_roof();
        recipe.layer_height_mm = 0.2;
        recipe.bead_width_mm = 0.45;
        recipe.top_skin_thickness_mm = 0.8;
        let base = Arc::new(MultiPolygon(vec![
            geo::Rect::new([0.0, 0.0], [40.0, 20.0]).to_polygon(),
        ]));
        let tower = Arc::new(MultiPolygon(vec![
            geo::Rect::new([0.0, 0.0], [10.0, 20.0]).to_polygon(),
        ]));
        // 50 base layers to Z 10, then 50 tower layers to Z 20.
        let tops: Vec<f64> = (1..=100).map(|k| f64::from(k) * 0.2).collect();
        let stepped: Vec<&Arc<MultiPolygon<f64>>> = (0..100)
            .map(|k| if k < 50 { &base } else { &tower })
            .collect();
        let cancel = AtomicBool::new(false);
        let field = field(&stepped, &tops, &recipe, 0.0, &cancel).unwrap();

        // Under the step, the base's open top is a ceiling 1 mm below Z 10.
        let roof = field.roof(40);
        assert_eq!(roof.ceilings.len(), 1);
        assert!((roof.ceilings[0].roof_z_mm - 9.0).abs() < 1e-9);
        assert!((roof.ceilings[0].area.unsigned_area() - 600.0).abs() < 1e-6);
        // The part roof is still the tower's, and the tower has no ceiling.
        assert!((roof.z_mm - 19.0).abs() < 1e-9);
        assert!(field.roof(60).ceilings.is_empty());
        // One frame for every layer, inside the walls.
        assert_eq!(field.roof(0).frame_mm, field.roof(99).frame_mm);
        assert!(roof.frame_mm[0][0] > 0.0 && roof.frame_mm[1][0] < 40.0);

        let prism: Vec<&Arc<MultiPolygon<f64>>> = (0..100).map(|_| &base).collect();
        let flat = super::field(&prism, &tops, &recipe, 0.0, &cancel).unwrap();
        assert!((0..100).all(|k| flat.roof(k).ceilings.is_empty()));
    }
}
