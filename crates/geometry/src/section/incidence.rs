// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

use super::{Admission, SectionEndpoint, indeterminate};
use crate::{GeometryError, GeometryWorkBudget, compute};
use std::sync::atomic::AtomicBool;

struct EndpointUse {
    endpoint: SectionEndpoint,
    incoming: bool,
    arc: usize,
}

pub(super) fn successors(
    count: usize,
    endpoints: impl Fn(usize) -> Option<[SectionEndpoint; 2]>,
    admission: &mut Admission,
    work: &GeometryWorkBudget,
    cancel: &AtomicBool,
) -> Result<Vec<usize>, GeometryError> {
    work.charge(1, cancel)?;
    let mut uses = Vec::new();
    let mut next = Vec::new();
    for arc in 0..count {
        work.charge(1, cancel)?;
        let endpoints = endpoints(arc)
            .ok_or_else(|| indeterminate("native section span was consumed before assembly"))?;
        for (side, endpoint) in endpoints.into_iter().enumerate() {
            admission.push(
                &mut uses,
                EndpointUse {
                    endpoint,
                    incoming: side == 1,
                    arc,
                },
            )?;
        }
        admission.push(&mut next, usize::MAX)?;
    }
    compute::sort_unstable_by(
        &mut uses,
        |left, right| {
            left.endpoint
                .cmp(&right.endpoint)
                .then(left.incoming.cmp(&right.incoming))
                .then(left.arc.cmp(&right.arc))
        },
        work,
        cancel,
    )?;
    let mut start = 0;
    while start < uses.len() {
        work.charge(1, cancel)?;
        let mut end = start + 1;
        while end < uses.len() && uses[end].endpoint == uses[start].endpoint {
            work.charge(1, cancel)?;
            end += 1;
        }
        if end - start != 2 || uses[start].incoming || !uses[start + 1].incoming {
            return Err(indeterminate(
                "native section shared-edge root does not have one material-left predecessor/successor",
            ));
        }
        next[uses[start + 1].arc] = uses[start].arc;
        start = end;
    }
    admission.used -= uses.capacity() * std::mem::size_of::<EndpointUse>();
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use spiling_contracts::geometry::GeometryErrorCode;

    fn edge() -> solidcore::EdgeId {
        let definition = crate::from_step_bytes_with_cancel(
            include_bytes!("../../../../fixtures/geometry/box-mm.step"),
            &AtomicBool::new(false),
        )
        .unwrap();
        definition.brep().edge_list()[0]
    }

    #[test]
    fn exact_incidence_index_scales_and_preserves_cycles() {
        let edge = edge();
        let cancel = AtomicBool::new(false);
        let mut previous_work = 0;
        for count in [1024, 4096, 16384] {
            let endpoint = |root| SectionEndpoint::EdgeRoot { edge, root };
            let work = GeometryWorkBudget::new(10_000_000);
            let mut admission = Admission::new(crate::MAX_PREPARED_SECTION_BYTES).unwrap();
            let before = admission.used;
            let next = successors(
                count,
                |arc| {
                    let root = (arc * 17 % count) as u32;
                    Some([endpoint(root), endpoint((root + 1) % count as u32)])
                },
                &mut admission,
                &work,
                &cancel,
            )
            .unwrap();
            assert_eq!(
                admission.used - before,
                next.capacity() * size_of::<usize>()
            );
            for (arc, &following) in next.iter().enumerate() {
                assert_eq!((following * 17) % count, ((arc * 17) + 1) % count);
            }
            if previous_work != 0 {
                assert!(work.consumed() < previous_work * 6);
            }
            previous_work = work.consumed();
            eprintln!("exact incidence arcs={count} work={previous_work}");
        }
    }

    #[test]
    fn index_retains_branch_open_consumed_and_resource_refusals() {
        let edge = edge();
        let endpoint = |root| SectionEndpoint::EdgeRoot { edge, root };
        let valid = [[endpoint(0), endpoint(1)], [endpoint(1), endpoint(0)]];
        for arcs in [
            vec![Some(valid[0])],
            vec![Some(valid[0]), Some(valid[1]), Some(valid[0])],
            vec![Some(valid[0]), None],
            vec![Some(valid[0]), Some([endpoint(0), endpoint(1)])],
        ] {
            let error = successors(
                arcs.len(),
                |index| arcs[index],
                &mut Admission::new(crate::MAX_PREPARED_SECTION_BYTES).unwrap(),
                &GeometryWorkBudget::query(),
                &AtomicBool::new(false),
            )
            .unwrap_err();
            assert_eq!(error.code, GeometryErrorCode::AnalysisIndeterminate);
        }
        for (limit, cancelled, bytes, expected) in [
            (0, false, usize::MAX, GeometryErrorCode::ResourceLimit),
            (100, true, usize::MAX, GeometryErrorCode::Cancelled),
            (100, false, 0, GeometryErrorCode::ResourceLimit),
        ] {
            let mut admission = Admission::new(crate::MAX_PREPARED_SECTION_BYTES).unwrap();
            admission.limit = admission.used.saturating_add(bytes);
            assert_eq!(
                successors(
                    2,
                    |index| Some(valid[index]),
                    &mut admission,
                    &GeometryWorkBudget::new(limit),
                    &AtomicBool::new(cancelled),
                )
                .unwrap_err()
                .code,
                expected,
            );
        }
    }
}
