use serde_json::{Value, json};
use std::{collections::BTreeMap, time::Instant};

struct Road {
    optional: bool,
    width: f64,
    length: f64,
    kind: u64,
    upper: f64,
    axis: usize,
    coordinate: f64,
    cost: f64,
}

struct Proof {
    bed: bool,
    intervals: Vec<(usize, f64, f64)>,
    endpoints: Vec<(usize, bool, bool)>,
    baseline_bad: Vec<(f64, f64)>,
}

fn number(value: &Value, field: &str) -> f64 {
    value[field].as_f64().expect("frozen numeric field")
}

fn uncovered(proof: &Proof, active: &[bool]) -> Vec<(f64, f64, bool, bool)> {
    let mut intervals: Vec<_> = proof
        .intervals
        .iter()
        .filter(|entry| active[entry.0])
        .map(|entry| (entry.1, entry.2))
        .collect();
    intervals.sort_unstable_by(|first, second| first.0.total_cmp(&second.0));
    let mut endpoints = [false; 2];
    for &(owner, first, second) in &proof.endpoints {
        if active[owner] {
            endpoints[0] |= first;
            endpoints[1] |= second;
        }
    }
    let mut gaps = Vec::new();
    let mut cursor = 0.0_f64;
    for (low, high) in intervals {
        if low > cursor {
            gaps.push((cursor, low, cursor > 0.0 || endpoints[0], true));
        }
        cursor = cursor.max(high);
    }
    if cursor < 1.0 {
        gaps.push((cursor, 1.0, cursor > 0.0 || endpoints[0], endpoints[1]));
    }
    gaps
}

fn valid(road: &Road, proof: &Proof, active: &[bool], limit: f64) -> bool {
    if proof.bed {
        return true;
    }
    for (low, high, left, right) in uncovered(proof, active) {
        let span = (high - low) * road.length;
        let supported = (left || right)
            && (span <= 0.01 || (!(left && right) && span <= road.width / 2.0 + 0.01));
        let bridge = left && right && road.kind == 8 && span <= limit;
        if !(supported
            || bridge
            || proof.baseline_bad.iter().any(|&(begin, end)| {
                begin <= low + 0.000002 / road.length && high <= end + 0.000002 / road.length
            }))
        {
            return false;
        }
    }
    true
}

fn hash(index: usize, seed: u64) -> u64 {
    let mut value = (index as u64).wrapping_add(seed.wrapping_mul(0x9e3779b97f4a7c15));
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
    value ^ (value >> 31)
}

pub fn select(context: &Value, policy: &Value) -> Value {
    let started = Instant::now();
    let roads: Vec<_> = context["roads"]
        .as_array()
        .expect("frozen roads")
        .iter()
        .map(|entry| {
            let axis = usize::from(entry["start"][0] == entry["end"][0]);
            Road {
                optional: entry["optional"].as_bool().expect("optional flag"),
                width: number(entry, "width"),
                length: number(entry, "length"),
                kind: entry["kind"].as_u64().expect("kind"),
                upper: number(entry, "upper"),
                axis,
                coordinate: entry["start"][1 - axis].as_f64().expect("coordinate"),
                cost: number(entry, "extruded_filament_mm"),
            }
        })
        .collect();
    let proofs: Vec<_> = context["proofs"]
        .as_array()
        .expect("frozen proofs")
        .iter()
        .map(|entry| Proof {
            bed: entry["bed"].as_bool().expect("bed"),
            intervals: entry["intervals"]
                .as_array()
                .expect("intervals")
                .iter()
                .map(|item| {
                    (
                        item[0].as_u64().expect("owner") as usize,
                        item[1].as_f64().expect("low"),
                        item[2].as_f64().expect("high"),
                    )
                })
                .collect(),
            endpoints: entry["endpoints"]
                .as_array()
                .expect("endpoints")
                .iter()
                .map(|item| {
                    (
                        item[0].as_u64().expect("owner") as usize,
                        item[1].as_bool().expect("left"),
                        item[2].as_bool().expect("right"),
                    )
                })
                .collect(),
            baseline_bad: entry["baseline_bad"]
                .as_array()
                .expect("baseline gaps")
                .iter()
                .map(|item| {
                    (
                        item[0].as_f64().expect("low"),
                        item[1].as_f64().expect("high"),
                    )
                })
                .collect(),
        })
        .collect();
    let mut children = vec![Vec::new(); roads.len()];
    for (index, proof) in proofs.iter().enumerate() {
        for &(owner, _, _) in &proof.intervals {
            children[owner].push(index);
        }
        for &(owner, _, _) in &proof.endpoints {
            children[owner].push(index);
        }
    }
    for entries in &mut children {
        entries.sort_unstable();
        entries.dedup();
    }
    let mut active = vec![true; roads.len()];
    let seed = policy["seed"].as_u64().unwrap_or(1);
    let family = policy["family"].as_str().unwrap_or("material");
    let target = policy["bridge_limit_mm"]
        .as_f64()
        .unwrap_or(number(context, "bridge_limit_mm"));
    let limits: Vec<_> = roads
        .iter()
        .zip(&proofs)
        .map(|(road, proof)| {
            uncovered(proof, &active)
                .iter()
                .filter(|gap| road.kind == 8 && gap.2 && gap.3)
                .map(|gap| (gap.1 - gap.0) * road.length)
                .fold(target, f64::max)
                .min(number(context, "bridge_limit_mm"))
        })
        .collect();
    let mut order: Vec<_> = roads
        .iter()
        .enumerate()
        .filter(|(_, road)| road.optional)
        .map(|(index, _)| index)
        .collect();
    order.retain(|&index| match family {
        "axis_x" => roads[index].axis == 0,
        "axis_y" => roads[index].axis == 1,
        "phase" => !hash((roads[index].upper * 1000.0).round() as usize, seed).is_multiple_of(3),
        "control" => false,
        _ => true,
    });
    order.sort_unstable_by(|&first, &second| {
        let by_height = roads[second].upper.total_cmp(&roads[first].upper);
        let within = match family {
            "random" | "phase" => hash(first, seed).cmp(&hash(second, seed)),
            "shortest" => roads[first].length.total_cmp(&roads[second].length),
            "deadline" => roads[first].coordinate.total_cmp(&roads[second].coordinate),
            _ => roads[second].cost.total_cmp(&roads[first].cost),
        };
        if family == "random" {
            within
        } else {
            by_height.then(within)
        }
    });
    let mut attempted = 0;
    let mut removed = 0;
    let mut checks = 0;
    let mut groups: Vec<Vec<usize>> = order.iter().map(|&index| vec![index]).collect();
    if family == "bundle" {
        let band = policy["band_mm"].as_f64().unwrap_or(0.4).max(0.01);
        let mut columns = BTreeMap::<(i64, usize, u64), Vec<usize>>::new();
        for &index in &order {
            columns
                .entry((
                    (roads[index].upper / band).floor() as i64,
                    roads[index].axis,
                    roads[index].coordinate.to_bits(),
                ))
                .or_default()
                .push(index);
        }
        groups = columns.into_values().rev().collect();
    }
    for group in groups {
        attempted += 1;
        for &index in &group {
            active[index] = false;
        }
        let mut affected: Vec<_> = group
            .iter()
            .flat_map(|&index| children[index].iter().copied())
            .collect();
        affected.sort_unstable();
        affected.dedup();
        let accepted = affected
            .into_iter()
            .filter(|&child| active[child])
            .all(|child| {
                checks += 1;
                valid(&roads[child], &proofs[child], &active, limits[child])
            });
        if accepted {
            removed += group.len();
        } else {
            for &index in &group {
                active[index] = true;
            }
        }
    }
    let selected: Vec<_> = active
        .iter()
        .enumerate()
        .filter(|(_, enabled)| **enabled)
        .map(|(index, _)| index)
        .collect();
    json!({"selected":selected,"attempted":attempted,"removed":removed,"interval_checks":checks,
        "selector_s":started.elapsed().as_secs_f64(),"family":family})
}
