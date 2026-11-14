// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Skirt and brim: closed first-layer loops of recipe bead width, planned
//! after every model and support path of the first layer is final, as
//! intentional non-model deposition.
//!
//! The brim follows the outer outline of first-layer islands (holes are
//! never brimmed): exact outward offsets of the union of those outlines at
//! bead spacing, the innermost at the recipe separation plus half a spacing,
//! so it slightly overlaps the part's outer bead. Offsets of the outline union
//! stay outside every first-layer material polygon. With `NarrowTall`
//! placement only islands whose part height reaches
//! [`AUTOMATIC_BRIM_ASPECT`] times the narrower side of their base (the
//! first-layer island grown by the material above it within the bottom skin
//! thickness), and every island within brim reach of them, are brimmed.
//!
//! The skirt follows the convex outline of every first-layer bead (part,
//! brim, support) at the recipe distance plus both half widths. A loop that
//! would leave the build envelope is not planned; the summary records how
//! many requested loops were left out.
use super::boolean_budget;
use super::region::offset_admitted;
use super::{SectionSpan, span_at};
use crate::{check_cancel, error};
use geo::{Area, BoundingRect, ConvexHull, LineString, MultiPoint, MultiPolygon, Polygon, Rect};
use spiling_contracts::manufacturing::*;
use std::sync::atomic::AtomicBool;

pub(super) fn plan(
    layers: &mut [PrintLayer],
    sections: &[SectionSpan],
    intent: &ManufacturingIntent,
    cancel: &AtomicBool,
) -> Result<AdhesionSummary, ManufacturingError> {
    let recipe = &intent.recipe;
    let mut summary = AdhesionSummary::default();
    if (recipe.skirt.is_none() && recipe.brim.is_none()) || layers.is_empty() {
        return Ok(summary);
    }
    let first = &layers[0];
    if !first.paths.iter().any(|p| p.kind.is_model()) {
        return Ok(summary);
    }
    let (z, lower) = (first.z_mm, first.lower_z_mm);
    let width = recipe.bead_width_mm;
    // Centre spacing of adjacent stadium beads of equal area.
    let spacing = (width - (z - lower) * (1.0 - std::f64::consts::FRAC_PI_4)).max(0.5 * width);
    let envelope = &intent.printer.build_envelope;
    let fits = |ring: &LineString<f64>| {
        ring.0.iter().all(|c| {
            c.x - 0.5 * width >= envelope.min[0]
                && c.x + 0.5 * width <= envelope.max[0]
                && c.y - 0.5 * width >= envelope.min[1]
                && c.y + 0.5 * width <= envelope.max[1]
        })
    };
    let loop_path = |kind: DepositionKind, ring: &LineString<f64>| DepositionPath {
        material_domain: None,
        kind,
        owner: None,
        width: WidthProfile::Constant { width_mm: width },
        physical_z_mm: [lower, z],
        points_mm: ring.0.iter().map(|c| [c.x, c.y, z]).collect(),
        arcs: Vec::new(),
        exposed: false,
    };
    let mut added = Vec::new();

    if let Some(brim) = &recipe.brim {
        let material = &sections[span_at(sections, 0)].material;
        let islands = brimmed_islands(material, brim, layers, sections, spacing, recipe, cancel)?;
        // A brim that wraps the support base wraps every island as well, so
        // none of its loops passes over an unbrimmed island near support.
        let support = support_footprint(&layers[0], cancel)?;
        let islands = if !islands.is_empty() && !support.0.is_empty() {
            (0..material.0.len()).collect()
        } else {
            islands
        };
        if !islands.is_empty() {
            let mut outline = MultiPolygon(
                islands
                    .iter()
                    .map(|&i| Polygon::new(material.0[i].exterior().clone(), Vec::new()))
                    .collect(),
            );
            // The brim surrounds the first layer's support base too, so no
            // loop runs over a support bead.
            if !support.0.is_empty() {
                boolean_budget(&outline, &support, cancel)?;
                outline = geo::BooleanOps::union(&outline, &support);
            }
            let loops = ((brim.width_mm / spacing).round() as u32).max(1);
            let mut planned = 0;
            for k in 0..loops {
                check_cancel(cancel)?;
                let distance = brim.separation_mm + 0.5 * spacing + f64::from(k) * spacing;
                let rings = offset_rings(&outline, distance, cancel)?;
                if rings.iter().any(|ring| !fits(ring)) {
                    break;
                }
                added.extend(
                    rings
                        .iter()
                        .map(|ring| loop_path(DepositionKind::Brim, ring)),
                );
                planned += 1;
            }
            summary.brim_loops_outside_envelope = loops - planned;
            summary.brim_paths = added.len() as u32;
            if summary.brim_paths > 0 {
                summary.brim_islands = islands.len() as u32;
            }
        }
    }

    if let Some(skirt) = &recipe.skirt {
        let layer = &layers[0];
        let points = MultiPoint(
            layer
                .paths
                .iter()
                .chain(&added)
                .flat_map(|path| path.points_mm.iter().map(|p| geo::Point::new(p[0], p[1])))
                .collect(),
        );
        let widest = layer
            .paths
            .iter()
            .chain(&added)
            .try_fold(width, |widest, path| {
                path.checked_edges()
                    .map(|edges| edges.fold(widest, |w, edge| w.max(edge.width_mm)))
            })?;
        let hull = points.convex_hull();
        let perimeter: f64 = hull
            .exterior()
            .lines()
            .map(|line| line.dx().hypot(line.dy()))
            .sum();
        // Dropping vertices for printable edges may cut a chord inward by up
        // to one minimum loop edge; start that much further out.
        let first_distance = skirt.distance_mm + 0.5 * widest + 0.5 * width + MIN_LOOP_EDGE_MM;
        // Loops of a convex outline offset by r are its perimeter plus 2 pi r.
        let mut requested = 0u32;
        let mut length = 0.0;
        while requested < skirt.loops || length < skirt.minimum_length_mm {
            length += perimeter
                + std::f64::consts::TAU * (first_distance + f64::from(requested) * spacing);
            requested = requested
                .checked_add(1)
                .ok_or_else(|| error(ManufacturingErrorCode::ResourceLimit, "skirt loop count"))?;
        }
        let outline = MultiPolygon(vec![hull]);
        let mut planned = 0;
        let mut skirt_paths = Vec::new();
        for k in 0..requested {
            check_cancel(cancel)?;
            let rings = offset_rings(&outline, first_distance + f64::from(k) * spacing, cancel)?;
            if rings.iter().any(|ring| !fits(ring)) {
                break;
            }
            skirt_paths.extend(
                rings
                    .iter()
                    .map(|ring| loop_path(DepositionKind::Skirt, ring)),
            );
            planned += 1;
        }
        summary.skirt_loops_outside_envelope = requested - planned;
        summary.skirt_paths = skirt_paths.len() as u32;
        skirt_paths.append(&mut added);
        added = skirt_paths;
    }

    let paths = &mut layers[0].paths;
    paths
        .try_reserve_exact(added.len())
        .map_err(|_| error(ManufacturingErrorCode::ResourceLimit, "adhesion paths"))?;
    paths.splice(0..0, added);
    Ok(summary)
}

/// Closed rings of the exact outward offset of `outline` (outer boundaries
/// and any pockets the offset encloses).
fn offset_rings(
    outline: &MultiPolygon<f64>,
    distance: f64,
    cancel: &AtomicBool,
) -> Result<Vec<LineString<f64>>, ManufacturingError> {
    let offset = offset_admitted(
        outline,
        distance,
        |_, bytes, overlay| {
            if let Some((a, b)) = overlay {
                boolean_budget(a, b, cancel)?;
            }
            if bytes > crate::working_set::bytes() {
                return Err(error(
                    ManufacturingErrorCode::ResourceLimit,
                    "adhesion offset exceeds the working set",
                ));
            }
            Ok(())
        },
        cancel,
    )?;
    Ok(offset
        .0
        .into_iter()
        .flat_map(|polygon| {
            let (exterior, interiors) = polygon.into_inner();
            std::iter::once(exterior).chain(interiors)
        })
        .map(printable_ring)
        .filter(|ring| ring.0.len() >= 4)
        .collect())
}

/// Shortest loop edge kept: well above the minimum command length, so XYZ6
/// rounding never shortens one below it.
pub(super) const MIN_LOOP_EDGE_MM: f64 =
    spiling_contracts::manufacturing::program_coordinates::MIN_DEPOSITION_COMMAND_MM * 2.5;

/// Drop vertices closer than [`MIN_LOOP_EDGE_MM`] to the last kept one, so
/// every edge (including the closing one) is printable. Each chord stays
/// within that length of the dropped vertices.
fn printable_ring(ring: LineString<f64>) -> LineString<f64> {
    let points = &ring.0;
    if points.len() < 4 {
        return ring;
    }
    let far =
        |a: &geo::Coord<f64>, b: &geo::Coord<f64>| (b.x - a.x).hypot(b.y - a.y) >= MIN_LOOP_EDGE_MM;
    let mut kept = vec![points[0]];
    for point in &points[1..points.len() - 1] {
        if far(kept.last().expect("first point kept"), point) {
            kept.push(*point);
        }
    }
    while kept.len() > 1 && !far(kept.last().expect("kept"), &kept[0]) {
        kept.pop();
    }
    kept.push(kept[0]);
    LineString(kept)
}

/// The union of rectangles enclosing every first-layer support bead.
fn support_footprint(
    layer: &PrintLayer,
    cancel: &AtomicBool,
) -> Result<MultiPolygon<f64>, ManufacturingError> {
    let mut rects = Vec::new();
    for path in layer.paths.iter().filter(|p| p.kind.is_support()) {
        check_cancel(cancel)?;
        for edge in path.checked_edges()? {
            let (a, b) = (edge.points_mm[0], edge.points_mm[1]);
            let half = 0.5 * edge.width_mm;
            let length = (b[0] - a[0]).hypot(b[1] - a[1]);
            if length <= 0.0 {
                continue;
            }
            let d = [(b[0] - a[0]) / length * half, (b[1] - a[1]) / length * half];
            let n = [-d[1], d[0]];
            let corner = |p: [f64; 3], s: f64, t: f64| {
                geo::coord! { x: p[0] + s * d[0] + t * n[0], y: p[1] + s * d[1] + t * n[1] }
            };
            rects.push(Polygon::new(
                LineString(vec![
                    corner(a, -1.0, -1.0),
                    corner(b, 1.0, -1.0),
                    corner(b, 1.0, 1.0),
                    corner(a, -1.0, 1.0),
                    corner(a, -1.0, -1.0),
                ]),
                Vec::new(),
            ));
        }
    }
    if rects.is_empty() {
        return Ok(MultiPolygon(Vec::new()));
    }
    let bytes = rects.len() * 5 * std::mem::size_of::<geo::Coord<f64>>() * 4;
    if bytes > crate::working_set::bytes() {
        return Err(error(
            ManufacturingErrorCode::ResourceLimit,
            "support footprint exceeds the working set",
        ));
    }
    Ok(geo::unary_union(&rects))
}

/// Indexes of the first-layer material polygons the brim surrounds.
fn brimmed_islands(
    material: &MultiPolygon<f64>,
    brim: &BrimPolicy,
    layers: &[PrintLayer],
    sections: &[SectionSpan],
    spacing: f64,
    recipe: &PlanarPrintRecipe,
    cancel: &AtomicBool,
) -> Result<Vec<usize>, ManufacturingError> {
    let (width, bottom_skin_thickness_mm) = (recipe.bead_width_mm, recipe.bottom_skin_thickness_mm);
    let bounds: Vec<Rect<f64>> = material
        .0
        .iter()
        .filter_map(|polygon| polygon.bounding_rect())
        .collect();
    if bounds.len() != material.0.len() || material.0.iter().any(|p| p.unsigned_area() <= 0.0) {
        return Err(error(
            ManufacturingErrorCode::InvalidSpecification,
            "first-layer island has no bounds",
        ));
    }
    let mut selected: Vec<bool> = match brim.placement {
        BrimPlacement::Always => vec![true; bounds.len()],
        BrimPlacement::NarrowTall => {
            // An island's base is its first-layer outline grown by the
            // material above it within the bottom skin thickness, so a part
            // touching the bed along a fillet or chamfer is measured by the
            // base it stands on, not by its first-layer sliver.
            let bed = layers[0].lower_z_mm;
            let base_top = bed + bottom_skin_thickness_mm;
            let mut top = vec![layers[0].z_mm; bounds.len()];
            let mut base = bounds.clone();
            for span in sections {
                check_cancel(cancel)?;
                let z = layers[span.end as usize - 1].z_mm;
                let in_base = layers[span.start as usize].lower_z_mm < base_top;
                for polygon in &span.material.0 {
                    let Some(rect) = polygon.bounding_rect() else {
                        continue;
                    };
                    for (island, bound) in bounds.iter().enumerate() {
                        if overlaps(&rect, bound, 0.0) {
                            top[island] = top[island].max(z);
                            if in_base {
                                base[island] = Rect::new(
                                    geo::coord! {
                                        x: base[island].min().x.min(rect.min().x),
                                        y: base[island].min().y.min(rect.min().y),
                                    },
                                    geo::coord! {
                                        x: base[island].max().x.max(rect.max().x),
                                        y: base[island].max().y.max(rect.max().y),
                                    },
                                );
                            }
                        }
                    }
                }
            }
            base.iter()
                .zip(&top)
                .map(|(base, top)| {
                    top - bed >= AUTOMATIC_BRIM_ASPECT * base.width().min(base.height())
                })
                .collect()
        }
    };
    // Anything within reach of a brimmed outline joins it, so no brim loop
    // passes over an unbrimmed island.
    let reach = brim.separation_mm + brim.width_mm + spacing + width;
    let mut changed = selected.iter().any(|&s| s);
    while changed {
        check_cancel(cancel)?;
        changed = false;
        for j in 0..bounds.len() {
            if !selected[j]
                && (0..bounds.len()).any(|i| selected[i] && overlaps(&bounds[i], &bounds[j], reach))
            {
                selected[j] = true;
                changed = true;
            }
        }
    }
    Ok((0..bounds.len()).filter(|&i| selected[i]).collect())
}

fn overlaps(a: &Rect<f64>, b: &Rect<f64>, margin: f64) -> bool {
    a.min().x - margin <= b.max().x
        && b.min().x - margin <= a.max().x
        && a.min().y - margin <= b.max().y
        && b.min().y - margin <= a.max().y
}
