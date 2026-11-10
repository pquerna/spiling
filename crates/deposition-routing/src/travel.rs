// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
use crate::*;

pub(crate) fn path(
    problem: &Problem,
    built: &[BoundsMm],
    built_top_mm: f64,
    from: PointMm,
    to: PointMm,
    portals: &[PointMm],
    meter: &mut Meter<'_>,
) -> Result<Option<Vec<PointMm>>, Error> {
    if clearance::clear(problem, built, built_top_mm, from, to, meter)? {
        return Ok(Some(if from == to {
            vec![from]
        } else {
            vec![from, to]
        }));
    }
    // Checked visibility graph: supplied portals plus one conservative high
    // plane proposal. A ceiling can invalidate that plane; never teleport.
    let mut nodes = vec![from, to];
    nodes.extend(problem.waypoints_mm.iter().chain(portals).copied());
    let top = problem
        .obstacles_mm
        .iter()
        .chain(built)
        .map(|b| b.max[2])
        .fold(from.0[2].max(to.0[2]), f64::max);
    let bottom = problem.head_mm.iter().map(|b| b.min[2]).fold(0., f64::min);
    let z = top - bottom + 0.1;
    if z <= problem.envelope_mm.max[2] {
        nodes.push(PointMm([from.0[0], from.0[1], z]));
        nodes.push(PointMm([to.0[0], to.0[1], z]));
    }
    let n = nodes.len();
    let mut cost = vec![f64::INFINITY; n];
    let mut previous = vec![usize::MAX; n];
    let mut visited = vec![false; n];
    cost[0] = 0.;
    for _ in 0..n {
        meter.charge(n as u64)?;
        let Some(i) = (0..n)
            .filter(|i| !visited[*i] && cost[*i].is_finite())
            .min_by(|a, b| cost[*a].total_cmp(&cost[*b]).then(a.cmp(b)))
        else {
            return Ok(None);
        };
        if i == 1 {
            let mut route = vec![to];
            let mut cursor = 1;
            while cursor != 0 {
                cursor = previous[cursor];
                route.push(nodes[cursor]);
            }
            route.reverse();
            return Ok(Some(route));
        }
        visited[i] = true;
        for j in 0..n {
            meter.charge(1)?;
            if !visited[j]
                && clearance::clear(problem, built, built_top_mm, nodes[i], nodes[j], meter)?
            {
                let proposed = cost[i] + nodes[i].distance(nodes[j]);
                if proposed < cost[j] {
                    cost[j] = proposed;
                    previous[j] = i;
                }
            }
        }
    }
    Ok(None)
}
