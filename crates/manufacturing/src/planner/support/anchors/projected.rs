use super::{AnchorCandidate, AnchorGap, AnchorInput, AnchorWorkspace, charge_pairs};
use crate::check_cancel;
use spiling_contracts::manufacturing::{
    DepositionEdges, DepositionPath, ManufacturingError, WidthProfile,
    program_coordinates::{
        MAX_XYZ6_COORDINATE_MM, XYZ6_PROJECTION_SCREEN_MM, project_xyz6_point_mm,
    },
};
use std::{mem::size_of, ops::Range, sync::atomic::AtomicBool};

pub(crate) struct ProjectedAnchorPaths<'a> {
    paths: Vec<ProjectedPath<'a>>,
    points: Vec<[f64; 3]>,
}

struct ProjectedPath<'a> {
    path: &'a DepositionPath,
    direct: bool,
    projected: Option<Range<usize>>,
    bounds: [[f64; 2]; 2],
    width: f64,
}

type PathScreen = ([[f64; 2]; 2], f64);

fn charge(work: &mut usize, count: usize, cancel: &AtomicBool) -> Result<(), ManufacturingError> {
    check_cancel(cancel)?;
    charge_pairs(work, count)
}

fn growth_capacity(mut capacity: usize, needed: usize) -> Option<usize> {
    while capacity < needed {
        capacity = capacity.checked_mul(2)?.max(4);
    }
    Some(capacity)
}

fn admitted(
    metadata_bytes: usize,
    edges: usize,
    owned_bytes: usize,
    workspace: &AnchorWorkspace,
) -> bool {
    let Some(remaining) = owned_bytes
        .checked_add(metadata_bytes)
        .and_then(|bytes| crate::working_set::bytes().checked_sub(bytes))
    else {
        return false;
    };
    let records = (remaining / (2 * size_of::<AnchorGap>().max(size_of::<(f64, f64, bool)>())))
        .min(crate::working_set::units());
    let needed = growth_capacity(workspace.intervals.capacity(), edges).and_then(|intervals| {
        growth_capacity(workspace.uncovered.capacity(), edges.checked_add(1)?)
            .and_then(|gaps| intervals.checked_add(gaps))
    });
    needed.is_some_and(|needed| needed <= records)
}

impl<'a> ProjectedAnchorPaths<'a> {
    pub(crate) fn build(
        previous: impl Iterator<Item = &'a DepositionPath> + Clone,
        contacts: impl Iterator<Item = &'a DepositionPath> + Clone,
        workspace: &AnchorWorkspace,
        owned_bytes: usize,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<Option<Self>, ManufacturingError> {
        let mut path_count = 0usize;
        let mut point_count = 0usize;
        let mut edge_count = 0usize;
        for path in previous.clone().chain(contacts.clone()) {
            charge(work, 1, cancel)?;
            let Some(counts) = path_count
                .checked_add(1)
                .zip(point_count.checked_add(path.points_mm.len()))
                .zip(edge_count.checked_add(path.points_mm.len().saturating_sub(1)))
            else {
                return Ok(None);
            };
            ((path_count, point_count), edge_count) = counts;
        }
        if path_count == 0 {
            return Ok(None);
        }
        let Some(bytes) = path_count
            .checked_mul(size_of::<ProjectedPath<'a>>())
            .and_then(|bytes| {
                point_count
                    .checked_mul(size_of::<[f64; 3]>())?
                    .checked_add(bytes)
            })
            .and_then(|bytes| bytes.checked_add(size_of::<Self>()))
        else {
            return Ok(None);
        };
        if !admitted(bytes, edge_count, owned_bytes, workspace) {
            return Ok(None);
        }
        let mut prepared = Self {
            paths: Vec::new(),
            points: Vec::new(),
        };
        if prepared.paths.try_reserve_exact(path_count).is_err()
            || prepared.points.try_reserve_exact(point_count).is_err()
            || !admitted(
                prepared.retained_bytes(),
                edge_count,
                owned_bytes,
                workspace,
            )
        {
            return Ok(None);
        }
        let mut cached = false;
        for (path, direct) in previous
            .map(|path| (path, true))
            .chain(contacts.map(|path| (path, false)))
        {
            charge(work, 1, cancel)?;
            let start = prepared.points.len();
            let metadata = prepare_path(path, &mut prepared.points, work, cancel)?;
            let (projected, bounds, width) = match metadata {
                Some((bounds, width)) => {
                    cached = true;
                    (Some(start..prepared.points.len()), bounds, width)
                }
                None => {
                    prepared.points.truncate(start);
                    (None, [[0.0; 2]; 2], 0.0)
                }
            };
            prepared.paths.push(ProjectedPath {
                path,
                direct,
                projected,
                bounds,
                width,
            });
        }
        check_cancel(cancel)?;
        Ok(cached.then_some(prepared))
    }

    pub(crate) fn retained_bytes(&self) -> usize {
        size_of::<Self>()
            + self.paths.capacity() * size_of::<ProjectedPath<'a>>()
            + self.points.capacity() * size_of::<[f64; 3]>()
    }

    fn inputs(
        &self,
        candidate: AnchorCandidate,
        direct_floor: f64,
        contacts: bool,
    ) -> impl Iterator<Item = AnchorInput> + '_ {
        let mut paths = self.paths.iter();
        let mut current: Option<(&ProjectedPath<'a>, usize)> = None;
        let mut raw: Option<DepositionEdges<'_>> = None;
        std::iter::from_fn(move || {
            loop {
                if let Some((path, ordinal)) = current.as_mut() {
                    if let Some(projected) = &path.projected {
                        let index = *ordinal;
                        if index + 1 < projected.len() {
                            *ordinal += 1;
                            let width = match &path.path.width {
                                WidthProfile::Constant { width_mm } => *width_mm,
                                WidthProfile::PerEdge { widths_mm } => widths_mm[index],
                            };
                            let first = path.path.points_mm[index];
                            let last = path.path.points_mm[index + 1];
                            if separated(candidate, edge_bounds([first, last]), width) {
                                return Some(AnchorInput::Skipped(1));
                            }
                            return Some(AnchorInput::Edge((
                                Ok([
                                    self.points[projected.start + index],
                                    self.points[projected.start + index + 1],
                                ]),
                                path.direct,
                                width,
                            )));
                        }
                    } else if let Some(edge) = raw.as_mut().and_then(Iterator::next) {
                        return Some(raw_input(
                            candidate,
                            [edge.points_mm[0], edge.points_mm[1]],
                            path.direct,
                            edge.width_mm,
                        ));
                    }
                    current = None;
                    raw = None;
                }
                let path = paths.next()?;
                if if path.direct {
                    path.path.physical_z_mm[1] != direct_floor
                } else {
                    !contacts
                } {
                    continue;
                }
                if let Some(projected) = &path.projected {
                    if separated(candidate, path.bounds, path.width) {
                        return Some(AnchorInput::Skipped(projected.len() - 1));
                    }
                } else {
                    match path.path.checked_edges() {
                        Ok(edges) => raw = Some(edges),
                        Err(failure) => {
                            return Some(AnchorInput::Edge((Err(failure), path.direct, 0.0)));
                        }
                    }
                }
                current = Some((path, 0));
            }
        })
    }
}

fn prepare_path(
    path: &DepositionPath,
    points: &mut Vec<[f64; 3]>,
    work: &mut usize,
    cancel: &AtomicBool,
) -> Result<Option<PathScreen>, ManufacturingError> {
    charge(work, 1, cancel)?;
    let height = path.height_mm();
    if !path.physical_z_mm.iter().all(|value| value.is_finite())
        || !height.is_finite()
        || height <= 0.0
        || path.points_mm.len() < 2
    {
        return Ok(None);
    }
    let valid_width = |width: f64| width.is_finite() && width * 0.5 > 0.0 && height <= width;
    let width = match &path.width {
        WidthProfile::Constant { width_mm } if valid_width(*width_mm) => *width_mm,
        WidthProfile::Constant { .. } => return Ok(None),
        WidthProfile::PerEdge { widths_mm } => {
            if widths_mm.len() != path.points_mm.len() - 1
                || widths_mm.len() > spiling_contracts::manufacturing::MAX_PLAN_COUNT
            {
                return Ok(None);
            }
            let mut maximum = 0.0_f64;
            for &width in widths_mm {
                charge(work, 1, cancel)?;
                if !valid_width(width) {
                    return Ok(None);
                }
                maximum = maximum.max(width);
            }
            charge(work, widths_mm.len(), cancel)?;
            maximum
        }
    };
    if path.checked_edges().is_err() || width * 0.5 <= 0.0 {
        return Ok(None);
    }
    let mut bounds = [[f64::INFINITY; 2], [f64::NEG_INFINITY; 2]];
    for (ordinal, &point) in path.points_mm.iter().enumerate() {
        charge(work, 1, cancel)?;
        if point
            .iter()
            .any(|value| !value.is_finite() || value.abs() > MAX_XYZ6_COORDINATE_MM)
            || (ordinal != 0
                && !(0..2).any(|axis| {
                    (point[axis] - path.points_mm[ordinal - 1][axis]).abs()
                        > 2.0 * XYZ6_PROJECTION_SCREEN_MM
                }))
        {
            return Ok(None);
        }
        let Ok(projected) = project_xyz6_point_mm(point) else {
            return Ok(None);
        };
        points.push(projected);
        for axis in 0..2 {
            bounds[0][axis] = bounds[0][axis].min(point[axis]);
            bounds[1][axis] = bounds[1][axis].max(point[axis]);
        }
    }
    Ok(Some((bounds, width)))
}

fn edge_bounds([first, last]: [[f64; 3]; 2]) -> [[f64; 2]; 2] {
    std::array::from_fn(|side| {
        std::array::from_fn(|axis| {
            if side == 0 {
                first[axis].min(last[axis])
            } else {
                first[axis].max(last[axis])
            }
        })
    })
}

fn separated(candidate: AnchorCandidate, bounds: [[f64; 2]; 2], width: f64) -> bool {
    let reach = ((candidate.bead_width_mm * 0.5 + width * 0.5).next_up()
        + XYZ6_PROJECTION_SCREEN_MM)
        .next_up();
    let [first, last] = candidate.endpoints_mm;
    (0..2).any(|axis| {
        first[axis].min(last[axis]) > (bounds[1][axis] + reach).next_up()
            || first[axis].max(last[axis]) < (bounds[0][axis] - reach).next_down()
    })
}

fn raw_input(
    candidate: AnchorCandidate,
    points: [[f64; 3]; 2],
    direct: bool,
    width: f64,
) -> AnchorInput {
    if points
        .iter()
        .flatten()
        .all(|value| value.is_finite() && value.abs() <= MAX_XYZ6_COORDINATE_MM)
        && width.is_finite()
        && width * 0.5 > 0.0
        && (0..2)
            .any(|axis| (points[0][axis] - points[1][axis]).abs() > 2.0 * XYZ6_PROJECTION_SCREEN_MM)
        && separated(candidate, edge_bounds(points), width)
    {
        return AnchorInput::Skipped(1);
    }
    AnchorInput::Edge((
        (|| {
            Ok([
                project_xyz6_point_mm(points[0])?,
                project_xyz6_point_mm(points[1])?,
            ])
        })(),
        direct,
        width,
    ))
}

impl AnchorWorkspace {
    pub(crate) fn gaps_reused_projected(
        &mut self,
        candidate: AnchorCandidate,
        direct_floor: f64,
        contacts: bool,
        paths: &ProjectedAnchorPaths<'_>,
        work: &mut usize,
        cancel: &AtomicBool,
    ) -> Result<&[AnchorGap], ManufacturingError> {
        self.gaps_inputs(
            candidate,
            paths.inputs(candidate, direct_floor, contacts),
            work,
            cancel,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiling_contracts::manufacturing::{DepositionKind, ManufacturingErrorCode};
    use std::sync::atomic::Ordering;

    fn path(coordinate_x: f64) -> DepositionPath {
        DepositionPath {
            material_domain: None,
            kind: DepositionKind::SupportInterface,
            owner: None,
            width: WidthProfile::Constant { width_mm: 0.5 },
            physical_z_mm: [0.0, 0.2],
            points_mm: vec![[coordinate_x, 0.0, 0.2], [coordinate_x + 1.0, 0.0, 0.2]],
            arcs: Vec::new(),
            exposed: false,
            connectors: Vec::new(),
        }
    }

    fn compare(
        candidate: AnchorCandidate,
        previous: &[DepositionPath],
        contacts: &[DepositionPath],
        floor: f64,
        with_contacts: bool,
    ) {
        let cancel = AtomicBool::new(false);
        let mut scalar = AnchorWorkspace::default();
        let mut indexed = AnchorWorkspace::default();
        let prepared = ProjectedAnchorPaths::build(
            previous.iter(),
            contacts.iter(),
            &indexed,
            0,
            &mut 0,
            &cancel,
        )
        .unwrap()
        .unwrap();
        indexed.admit_beside(prepared.retained_bytes()).unwrap();
        let mut scalar_work = 0;
        let mut indexed_work = 0;
        let expected = scalar.gaps_projected(
            candidate,
            previous
                .iter()
                .filter(|path| path.physical_z_mm[1] == floor),
            contacts.iter().filter(|_| with_contacts),
            &mut scalar_work,
            &cancel,
        );
        let actual = indexed.gaps_reused_projected(
            candidate,
            floor,
            with_contacts,
            &prepared,
            &mut indexed_work,
            &cancel,
        );
        match (expected, actual) {
            (Ok(expected), Ok(actual)) => {
                let shape = |gap: &AnchorGap| {
                    (
                        gap.start.to_bits(),
                        gap.end.to_bits(),
                        gap.left,
                        gap.right,
                        gap.direct_ends,
                    )
                };
                assert_eq!(
                    expected.iter().map(shape).collect::<Vec<_>>(),
                    actual.iter().map(shape).collect::<Vec<_>>()
                );
            }
            (Err(expected), Err(actual)) => {
                assert_eq!(expected.code, actual.code);
                assert_eq!(expected.message, actual.message);
            }
            _ => panic!("projected inventory changed anchor acceptance"),
        }
        assert_eq!(scalar_work, indexed_work);
    }

    #[test]
    fn projected_inventory_preserves_gaps_provenance_widths_and_floor_selection() {
        let mut previous = vec![path(100.0), path(0.0), path(4.0)];
        previous[1].points_mm.push([2.0, 0.0, 0.2]);
        previous[1].width = WidthProfile::PerEdge {
            widths_mm: vec![0.5, 0.8],
        };
        previous[2].physical_z_mm = [0.2, 0.4];
        let mut contacts = vec![path(4.0)];
        contacts[0].physical_z_mm = [0.4, 0.6];
        for floor in [0.2, 0.4, 0.6] {
            for with_contacts in [false, true] {
                for width in [0.3, 0.5, 1.0] {
                    for y in [
                        0.0,
                        0.25,
                        0.5_f64.next_down(),
                        0.5,
                        0.5_f64.next_up(),
                        100.0,
                    ] {
                        compare(
                            AnchorCandidate {
                                endpoints_mm: [[0.0, y, 1.0], [5.0, y, 1.0]],
                                bead_width_mm: width,
                            },
                            &previous,
                            &contacts,
                            floor,
                            with_contacts,
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn unsafe_paths_keep_original_position_and_errors() {
        for variant in 0..8 {
            for reverse in [false, true] {
                let mut unsafe_path = path(100.0);
                match variant {
                    0 => unsafe_path.points_mm.clear(),
                    1 => unsafe_path.width = WidthProfile::PerEdge { widths_mm: vec![] },
                    2 => unsafe_path.width = WidthProfile::Constant { width_mm: f64::NAN },
                    3 => unsafe_path.points_mm[0][0] = f64::NAN,
                    4 => unsafe_path.points_mm[1][0] = MAX_XYZ6_COORDINATE_MM.next_up(),
                    5 => unsafe_path.points_mm[1] = unsafe_path.points_mm[0],
                    6 => unsafe_path.physical_z_mm[0] = f64::NEG_INFINITY,
                    _ => unsafe_path.points_mm[1][0] = unsafe_path.points_mm[0][0] + 0.000_000_1,
                }
                let mut lower = vec![path(0.0), unsafe_path, path(100.0)];
                if reverse {
                    lower.reverse();
                }
                compare(
                    AnchorCandidate {
                        endpoints_mm: [[0.0, 0.0, 0.4], [5.0, 0.0, 0.4]],
                        bead_width_mm: 0.5,
                    },
                    &lower,
                    &[],
                    0.2,
                    true,
                );
            }
        }
    }

    #[test]
    fn malformed_width_and_projection_errors_keep_original_first_error() {
        let mut malformed_width = path(100.0);
        malformed_width.width = WidthProfile::PerEdge { widths_mm: vec![] };
        let mut malformed_point = path(100.0);
        malformed_point.points_mm[0][0] = f64::NAN;
        for reverse in [false, true] {
            let mut previous = vec![
                path(100.0),
                malformed_width.clone(),
                malformed_point.clone(),
            ];
            if reverse {
                previous.reverse();
            }
            compare(
                AnchorCandidate {
                    endpoints_mm: [[0.0, 0.0, 0.4], [5.0, 0.0, 0.4]],
                    bead_width_mm: 0.5,
                },
                &previous,
                &[],
                0.2,
                true,
            );
        }
    }

    #[test]
    fn a_malformed_contact_is_not_promoted_or_visited_in_direct_only_queries() {
        let previous = [path(0.0)];
        let mut malformed = path(0.0);
        malformed.width = WidthProfile::PerEdge { widths_mm: vec![] };
        let candidate = AnchorCandidate {
            endpoints_mm: [[0.0, 0.0, 0.4], [5.0, 0.0, 0.4]],
            bead_width_mm: 0.5,
        };
        compare(candidate, &previous, &[malformed.clone()], 0.2, false);
        compare(candidate, &previous, &[malformed], 0.2, true);
    }

    #[test]
    fn exact_byte_admission_includes_metadata_and_worst_case_scratch_growth() {
        let paths = [path(0.0), path(1.0), path(2.0)];
        let cancel = AtomicBool::new(false);
        let workspace = AnchorWorkspace::default();
        let prepared = ProjectedAnchorPaths::build(
            paths.iter(),
            std::iter::empty(),
            &workspace,
            0,
            &mut 0,
            &cancel,
        )
        .unwrap()
        .unwrap();
        let scratch = 8 * (2 * size_of::<AnchorGap>().max(size_of::<(f64, f64, bool)>()));
        let budget = prepared.retained_bytes() + scratch;
        crate::working_set::scope(crate::working_set::WorkingSet::new(budget).unwrap(), || {
            assert!(
                ProjectedAnchorPaths::build(
                    paths.iter(),
                    std::iter::empty(),
                    &workspace,
                    0,
                    &mut 0,
                    &cancel
                )
                .unwrap()
                .is_some()
            );
        });
        crate::working_set::scope(
            crate::working_set::WorkingSet::new(budget - 1).unwrap(),
            || {
                assert!(
                    ProjectedAnchorPaths::build(
                        paths.iter(),
                        std::iter::empty(),
                        &workspace,
                        0,
                        &mut 0,
                        &cancel
                    )
                    .unwrap()
                    .is_none()
                );
            },
        );
        assert!(
            ProjectedAnchorPaths::build(
                paths.iter(),
                std::iter::empty(),
                &workspace,
                usize::MAX,
                &mut 0,
                &cancel
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn admission_preserves_existing_workspace_capacities() {
        let paths = [path(0.0)];
        let cancel = AtomicBool::new(false);
        let mut workspace = AnchorWorkspace::default();
        workspace.intervals.try_reserve_exact(32).unwrap();
        workspace.uncovered.try_reserve_exact(16).unwrap();
        let prepared = ProjectedAnchorPaths::build(
            paths.iter(),
            std::iter::empty(),
            &workspace,
            0,
            &mut 0,
            &cancel,
        )
        .unwrap()
        .unwrap();
        let retained = workspace.retained_bytes();
        workspace.admit_beside(prepared.retained_bytes()).unwrap();
        assert_eq!(workspace.retained_bytes(), retained);
    }

    #[test]
    fn construction_work_cancellation_and_optional_decline_are_explicit() {
        let paths = [path(0.0)];
        let cancel = AtomicBool::new(false);
        let workspace = AnchorWorkspace::default();
        let mut work = 0;
        assert!(
            ProjectedAnchorPaths::build(
                paths.iter(),
                std::iter::empty(),
                &workspace,
                0,
                &mut work,
                &cancel
            )
            .unwrap()
            .is_some()
        );
        assert!(work >= 5);
        let mut overflow_work = usize::MAX;
        let error = ProjectedAnchorPaths::build(
            paths.iter(),
            std::iter::empty(),
            &workspace,
            0,
            &mut overflow_work,
            &cancel,
        )
        .err()
        .unwrap();
        assert_eq!(error.code, ManufacturingErrorCode::ResourceLimit);
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            ProjectedAnchorPaths::build(
                paths.iter(),
                std::iter::empty(),
                &workspace,
                0,
                &mut 0,
                &cancel
            )
            .err()
            .unwrap()
            .code,
            ManufacturingErrorCode::Cancelled
        );
    }
}
