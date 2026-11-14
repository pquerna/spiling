use super::{cancelled, charge, failed};
use geo::{BoundingRect, MultiPolygon, Polygon};
use spiling_contracts::manufacturing::*;
use std::{cmp::Ordering, mem::size_of, sync::atomic::AtomicBool};

pub(super) struct Demand {
    pub material: MaterialDomainRef,
    pub owner: DepositionOwner,
    pub kind: DepositionKind,
    pub fraction: f64,
    pub along: usize,
    pub domain: MultiPolygon<f64>,
}

#[derive(Clone, Debug, PartialEq)]
struct Interval {
    demand: usize,
    along: usize,
    row: f64,
    extent: [f64; 2],
}

fn resource() -> ManufacturingError {
    ManufacturingError::new(
        ManufacturingErrorCode::ResourceLimit,
        "functional complete-row replay inventory exceeded",
    )
}

fn reserve(entries: &mut Vec<Interval>, retained_bytes: usize) -> Result<(), ManufacturingError> {
    let target = if entries.len() == entries.capacity() {
        entries
            .capacity()
            .saturating_mul(2)
            .max(64)
            .min(crate::working_set::units())
    } else {
        entries.capacity()
    };
    if entries.len() >= crate::working_set::units()
        || retained_bytes
            .checked_add(target * size_of::<Interval>())
            .is_none_or(|bytes| bytes as u64 > crate::working_set::bytes_u64())
    {
        return Err(resource());
    }
    if entries.len() == entries.capacity() {
        entries
            .try_reserve_exact(target - entries.len())
            .map_err(|_| resource())?;
    }
    Ok(())
}

fn compare(left: &Interval, right: &Interval) -> Ordering {
    left.demand
        .cmp(&right.demand)
        .then(left.along.cmp(&right.along))
        .then(left.row.total_cmp(&right.row))
        .then(left.extent[0].total_cmp(&right.extent[0]))
        .then(left.extent[1].total_cmp(&right.extent[1]))
}

fn normalize(
    entries: &mut Vec<Interval>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    charge(work, entries.len() * usize::BITS as usize)?;
    entries.sort_unstable_by(compare);
    let mut written = 0;
    for index in 0..entries.len() {
        cancelled(cancel)?;
        if written > 0 {
            let previous = &entries[written - 1];
            let current = &entries[index];
            if previous.demand == current.demand
                && previous.along == current.along
                && previous.row == current.row
            {
                if current.extent[0] < previous.extent[1] {
                    return Ok(false);
                }
                if current.extent[0] == previous.extent[1]
                    && current.extent[0] < current.extent[1]
                    && previous.extent[0] < previous.extent[1]
                {
                    entries[written - 1].extent[1] = current.extent[1];
                    continue;
                }
            }
        }
        if written != index {
            entries.swap(written, index);
        }
        written += 1;
    }
    entries.truncate(written);
    Ok(true)
}

fn projected(value: f64) -> Result<f64, ManufacturingError> {
    program_coordinates::project_xyz6_coordinate_mm(value)
}

fn sweep(
    polygon: &Polygon<f64>,
    along: usize,
    row: f64,
    hits: &mut Vec<f64>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    hits.clear();
    let fixed = 1 - along;
    for ring in std::iter::once(polygon.exterior()).chain(polygon.interiors()) {
        for edge in ring.0.windows(2) {
            cancelled(cancel)?;
            charge(work, 1)?;
            let start = [edge[0].x, edge[0].y];
            let end = [edge[1].x, edge[1].y];
            if (start[fixed] <= row && row < end[fixed])
                || (end[fixed] <= row && row < start[fixed])
            {
                if hits.len() >= crate::working_set::units() {
                    return Err(resource());
                }
                hits.push(
                    start[along]
                        + (row - start[fixed]) * (end[along] - start[along])
                            / (end[fixed] - start[fixed]),
                );
            }
        }
    }
    charge(work, hits.len() * usize::BITS as usize)?;
    hits.sort_unstable_by(f64::total_cmp);
    if !hits.len().is_multiple_of(2) || hits.iter().any(|value| !value.is_finite()) {
        return Err(failed("functional row sweep has incomplete ring parity"));
    }
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "Row demand, lattice, retained allocation and cumulative work remain explicit verifier inputs"
)]
fn append_expected(
    demand_index: usize,
    demand: &Demand,
    pitch: f64,
    phase: u8,
    entries: &mut Vec<Interval>,
    retained_bytes: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<bool, ManufacturingError> {
    if demand.fraction == 0.0 {
        return Ok(true);
    }
    let ordinary = matches!(demand.owner, DepositionOwner::Regional { .. });
    let mut hits = Vec::new();
    for polygon in &demand.domain.0 {
        cancelled(cancel)?;
        let edges = polygon.exterior().0.len()
            + polygon
                .interiors()
                .iter()
                .map(|ring| ring.0.len())
                .sum::<usize>();
        if edges > crate::working_set::units()
            || retained_bytes
                .checked_add(entries.capacity() * size_of::<Interval>())
                .and_then(|bytes| bytes.checked_add(edges.max(hits.capacity()) * size_of::<f64>()))
                .is_none_or(|bytes| bytes as u64 > crate::working_set::bytes_u64())
        {
            return Err(resource());
        }
        hits.clear();
        if hits.capacity() < edges {
            hits.try_reserve_exact(edges).map_err(|_| resource())?;
        }
        let bounds = polygon
            .bounding_rect()
            .ok_or_else(|| failed("functional row demand has no bounds"))?;
        let limits = if demand.along == 0 {
            [bounds.min().y, bounds.max().y]
        } else {
            [bounds.min().x, bounds.max().x]
        };
        let count = (((limits[1] - limits[0]) / pitch) * demand.fraction)
            .ceil()
            .max(1.0);
        if !count.is_finite() || count > crate::working_set::units() as f64 {
            return Err(resource());
        }
        let mut spacing = if count == 1.0 {
            0.0
        } else {
            pitch / demand.fraction
        };
        let mut start = limits[0] + ((limits[1] - limits[0]) - (count - 1.0) * spacing) * 0.5;
        if ordinary && phase != 0 {
            spacing = pitch / demand.fraction;
            let origin = start + f64::from(phase) * 0.25 * spacing;
            let first = ((limits[0] - origin) / spacing).ceil();
            let stop = ((limits[1] - origin) / spacing).ceil();
            if (stop - first).max(0.0) != count {
                return Ok(false);
            }
            start = origin + first * spacing;
        }
        for ordinal in 0..count as usize {
            let row = start + ordinal as f64 * spacing;
            sweep(polygon, demand.along, row, &mut hits, work, cancel)?;
            for pair in hits.as_chunks::<2>().0 {
                if pair[0] >= pair[1] {
                    return Err(failed(
                        "functional complete row has no positive original extent",
                    ));
                }
                reserve(entries, retained_bytes + hits.capacity() * size_of::<f64>())?;
                entries.push(Interval {
                    demand: demand_index,
                    along: demand.along,
                    row: projected(row)?,
                    extent: [projected(pair[0])?, projected(pair[1])?],
                });
            }
        }
    }
    Ok(true)
}

fn actual(
    layer: &PrintLayer,
    demands: &[Demand],
    width: f64,
    retained_bytes: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Vec<Interval>, ManufacturingError> {
    let mut entries = Vec::new();
    let fit_count = layer
        .skin_command_fits
        .iter()
        .try_fold(0usize, |total, domain| {
            total.checked_add(domain.fits.len()).ok_or_else(resource)
        })?;
    for path in &layer.paths {
        cancelled(cancel)?;
        if path.owner == Some(DepositionOwner::Wall) {
            continue;
        }
        charge(work, demands.len())?;
        let mut candidates = demands.iter().enumerate().filter(|(_, demand)| {
            Some(demand.material) == path.material_domain
                && Some(demand.owner) == path.owner
                && (demand.kind == path.kind || path.kind == DepositionKind::Bridge)
        });
        let Some((demand_index, demand)) = candidates.next() else {
            return Err(failed(
                "functional actual stroke has no complete original row owner",
            ));
        };
        if candidates.next().is_some() || path.physical_z_mm != [layer.lower_z_mm, layer.z_mm] {
            return Err(failed(
                "functional row ownership or physical interval is ambiguous",
            ));
        }
        charge(work, fit_count)?;
        let mut fits = layer
            .skin_command_fits
            .iter()
            .flat_map(|domain| &domain.fits)
            .filter(|fit| {
                path.points_mm.as_slice() == fit.fitted_mm.as_slice()
                    || path.points_mm.len() == fit.fitted_mm.len()
                        && path.points_mm.iter().rev().eq(fit.fitted_mm.iter())
            });
        let fit = fits.next();
        if fits.next().is_some() {
            return Err(failed(
                "functional actual row has duplicate minimum-command witnesses",
            ));
        }
        if fit.is_some() && !matches!(demand.owner, DepositionOwner::MandatorySkin { .. }) {
            return Err(failed(
                "functional ordinary row cannot borrow a mandatory-skin fit",
            ));
        }
        let points = fit.map_or(path.points_mm.as_slice(), |fit| fit.required_mm.as_slice());
        if path.checked_edges()?.any(|edge| edge.width_mm != width)
            || points.iter().any(|point| point[2] != layer.z_mm)
        {
            return Err(failed(
                "functional row does not retain its original width and height",
            ));
        }
        for pair in points.windows(2) {
            charge(work, 1)?;
            let fixed = 1 - demand.along;
            if pair[0][fixed] != pair[1][fixed] {
                return Err(failed("functional recovery changes the required row axis"));
            }
            reserve(&mut entries, retained_bytes)?;
            let extent = [
                projected(pair[0][demand.along])?,
                projected(pair[1][demand.along])?,
            ];
            entries.push(Interval {
                demand: demand_index,
                along: demand.along,
                row: projected(pair[0][fixed])?,
                extent: [extent[0].min(extent[1]), extent[0].max(extent[1])],
            });
        }
    }
    if !normalize(&mut entries, work, cancel)? {
        return Err(failed(
            "functional complete-row replay found duplicate actual extrusion",
        ));
    }
    Ok(entries)
}

pub(super) fn verify(
    layer: &PrintLayer,
    demands: &[Demand],
    width: f64,
    retained_bytes: usize,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<(), ManufacturingError> {
    let pitch = ((width - layer.height_mm) * layer.height_mm
        + std::f64::consts::PI * (layer.height_mm * 0.5).powi(2))
        / layer.height_mm;
    if !pitch.is_finite() || pitch <= 0.0 {
        return Err(failed(
            "functional complete-row pitch is not positive and finite",
        ));
    }
    let actual = actual(layer, demands, width, retained_bytes, work, cancel)?;
    let retained_bytes = retained_bytes + actual.capacity() * size_of::<Interval>();
    let mut expected = Vec::new();
    let mut diagnostic = String::new();
    let mut best_prefix = 0;
    for phase in 0..4 {
        expected.clear();
        let mut complete = true;
        for (index, demand) in demands.iter().enumerate() {
            if !append_expected(
                index,
                demand,
                pitch,
                phase,
                &mut expected,
                retained_bytes,
                work,
                cancel,
            )? {
                complete = false;
                break;
            }
        }
        if complete && normalize(&mut expected, work, cancel)? {
            if expected == actual {
                return Ok(());
            }
            let prefix = expected
                .iter()
                .zip(&actual)
                .take_while(|(left, right)| left == right)
                .count();
            if diagnostic.is_empty() || prefix > best_prefix {
                best_prefix = prefix;
                diagnostic = format!(
                    "layer {}, phase {}, expected {} intervals, actual {}, first differing expected {:?}, actual {:?}",
                    layer.index,
                    phase,
                    expected.len(),
                    actual.len(),
                    expected.get(prefix),
                    actual.get(prefix)
                );
            }
        }
    }
    Err(failed(format!(
        "functional solid/sparse construction has missing, shifted, duplicated or unsupported replacement rows; no complete common legal phase matches ({diagnostic})",
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> (PrintLayer, Vec<Demand>) {
        let (_, plan) = super::super::super::tests::fixture();
        let mut layer = plan.layers[0].clone();
        layer.lower_z_mm = 0.0;
        layer.z_mm = 0.2;
        layer.height_mm = 0.2;
        layer.skin_command_fits.clear();
        let material = MaterialDomainRef {
            construction_index: 0,
            role: MaterialConstructionRole::FunctionalBase,
        };
        let owner = DepositionOwner::Regional { region_index: 0 };
        let pitch = 0.2 + std::f64::consts::PI * 0.05;
        let start = (2.0 - 5.0 * pitch) * 0.5;
        layer.paths = (0..6)
            .map(|ordinal| {
                let row = start + f64::from(ordinal) * pitch;
                DepositionPath {
                    material_domain: Some(material),
                    owner: Some(owner),
                    kind: DepositionKind::SolidFill,
                    width: WidthProfile::Constant { width_mm: 0.4 },
                    physical_z_mm: [0.0, 0.2],
                    points_mm: vec![[0.0, row, 0.2], [2.0, row, 0.2]],
                    arcs: Vec::new(),
                    external_wall: false,
                }
            })
            .collect();
        let domain = geo::Rect::new(geo::Coord { x: 0.0, y: 0.0 }, geo::Coord { x: 2.0, y: 2.0 })
            .to_polygon();
        (
            layer,
            vec![Demand {
                material,
                owner,
                kind: DepositionKind::SolidFill,
                fraction: 1.0,
                along: 0,
                domain: MultiPolygon(vec![domain]),
            }],
        )
    }

    fn check(layer: &PrintLayer, demands: &[Demand]) -> Result<(), ManufacturingError> {
        verify(layer, demands, 0.4, 0, &mut 0, &AtomicBool::new(false))
    }

    #[test]
    fn functional_sparse_parent_complete_rows_accept_harmless_subdivision() {
        let (mut layer, demands) = fixture();
        check(&layer, &demands).unwrap();
        let row = layer.paths[0].points_mm[0][1];
        layer.paths[0].points_mm.insert(1, [1.0, row, 0.2]);
        check(&layer, &demands).unwrap();
    }

    #[test]
    fn functional_sparse_parent_complete_rows_refuse_matched_row_deletion() {
        let (mut layer, demands) = fixture();
        layer.paths = layer
            .paths
            .into_iter()
            .enumerate()
            .filter_map(|(index, path)| index.is_multiple_of(2).then_some(path))
            .collect();
        assert!(check(&layer, &demands).is_err());
    }

    #[test]
    fn functional_sparse_parent_complete_rows_refuse_duplicate_and_shortened_strokes() {
        let (layer, demands) = fixture();
        let mut duplicate = layer.clone();
        duplicate.paths.push(duplicate.paths[0].clone());
        assert!(check(&duplicate, &demands).is_err());
        let mut shortened = layer;
        shortened.paths[0].points_mm[1][0] -= 0.2;
        assert!(check(&shortened, &demands).is_err());
    }

    #[test]
    fn functional_sparse_parent_complete_rows_require_one_whole_phase() {
        let (mut layer, demands) = fixture();
        let pitch = 0.2 + std::f64::consts::PI * 0.05;
        for path in &mut layer.paths {
            for point in &mut path.points_mm {
                point[1] += 0.25 * pitch;
            }
        }
        check(&layer, &demands).unwrap();
        for point in &mut layer.paths[0].points_mm {
            point[1] -= 0.25 * pitch;
        }
        assert!(check(&layer, &demands).is_err());
    }

    #[test]
    fn functional_sparse_parent_complete_rows_do_not_accept_moved_same_area_masks() {
        let (layer, mut demands) = fixture();
        let shifted = geo::Rect::new(
            geo::Coord { x: 0.25, y: 0.0 },
            geo::Coord { x: 2.25, y: 2.0 },
        )
        .to_polygon();
        demands[0].domain = MultiPolygon(vec![shifted]);
        assert!(check(&layer, &demands).is_err());
    }

    #[test]
    fn functional_sparse_parent_complete_rows_retain_width_and_material_owner() {
        let (layer, demands) = fixture();
        let mut wrong_width = layer.clone();
        wrong_width.paths[0].width = WidthProfile::Constant { width_mm: 0.3 };
        assert!(check(&wrong_width, &demands).is_err());
        let mut wrong_owner = layer;
        wrong_owner.paths[0].material_domain.as_mut().unwrap().role =
            MaterialConstructionRole::FunctionalRegion;
        assert!(check(&wrong_owner, &demands).is_err());
    }
}
