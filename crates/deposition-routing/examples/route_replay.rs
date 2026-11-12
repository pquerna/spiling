// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
//! Research replay (autoresearch routing track): schedules single-tool inputs
//! dumped by `SPILING_ROUTING_DUMP` and prints time and modelled cost.
use spiling_deposition_routing::*;
use std::{sync::atomic::AtomicBool, time::Instant};

struct Reader<'a>(&'a [u8]);
impl Reader<'_> {
    fn take(&mut self, n: usize) -> &[u8] {
        let (a, b) = self.0.split_at(n);
        self.0 = b;
        a
    }
    fn u8(&mut self) -> u8 {
        self.take(1)[0]
    }
    fn u32(&mut self) -> u32 {
        u32::from_le_bytes(self.take(4).try_into().unwrap())
    }
    fn u64(&mut self) -> u64 {
        u64::from_le_bytes(self.take(8).try_into().unwrap())
    }
    fn f64(&mut self) -> f64 {
        f64::from_le_bytes(self.take(8).try_into().unwrap())
    }
    fn point(&mut self) -> PointMm {
        [self.f64(), self.f64(), self.f64()]
    }
}

enum OwnedEnds {
    Open(PointMm, PointMm, bool),
    Loop(Vec<PointMm>),
}
struct OwnedTask {
    ends: OwnedEnds,
    tool: u32,
    barrier: u32,
    prerequisites: Vec<u32>,
    duration_s: f64,
    seam_policy: Option<SeamPolicy>,
    z: [f64; 2],
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let mut total = (0., 0., 0.);
    for path in &arguments {
        let bytes = std::fs::read(path).unwrap();
        let mut r = Reader(&bytes);
        assert_eq!(r.take(5), b"SPRD1");
        let has_start = r.u8() == 1;
        let start = r.point();
        let speed_mm_s = r.f64();
        let lift_mm = r.f64();
        let has_retraction = r.u8() == 1;
        let retraction = Retraction {
            minimum_mm: r.f64(),
            seconds: r.f64(),
            lift_mm: r.f64(),
        };
        let budget = r.u64();
        let n = r.u32() as usize;
        let mut owned = Vec::with_capacity(n);
        for _ in 0..n {
            let ends = match r.u8() {
                0 => OwnedEnds::Open(r.point(), r.point(), r.u8() == 1),
                _ => {
                    let k = r.u32() as usize;
                    OwnedEnds::Loop((0..k).map(|_| r.point()).collect())
                }
            };
            let tool = r.u32();
            let barrier = r.u32();
            let k = r.u32() as usize;
            let prerequisites = (0..k).map(|_| r.u32()).collect();
            let duration_s = r.f64();
            let seam_policy = match r.u8() {
                0 => None,
                1 => Some(SeamPolicy::Nearest),
                2 => Some(SeamPolicy::PreferredBuildDirection {
                    xy: [r.f64(), r.f64()],
                }),
                _ => Some(SeamPolicy::Distributed),
            };
            let z = [r.f64(), r.f64()];
            owned.push(OwnedTask {
                ends,
                tool,
                barrier,
                prerequisites,
                duration_s,
                seam_policy,
                z,
            });
        }
        let tasks: Vec<Task<'_>> = owned
            .iter()
            .map(|t| Task {
                ends: match &t.ends {
                    OwnedEnds::Open(first, last, reversible) => Ends::Open {
                        first: *first,
                        last: *last,
                        reversible: *reversible,
                    },
                    OwnedEnds::Loop(seams) => Ends::Loop { seams },
                },
                tool: t.tool,
                barrier: t.barrier,
                prerequisites: &t.prerequisites,
                duration_s: t.duration_s,
                seam_policy: t.seam_policy,
                physical_z_mm: t.z,
            })
            .collect();
        let machine = Machine {
            start: has_start.then_some(start),
            initial_tool: Some(0),
            tools: 1,
            travel: Travel {
                speed_mm_s,
                lift_mm,
                retraction: has_retraction.then_some(retraction),
            },
            change: ToolChange {
                swap_s: 0.,
                prewarm_lead_s: None,
                service_s: 0.,
                service: Service::WhenUsed,
                stations_xy: vec![None],
            },
        };
        let cancel = AtomicBool::new(false);
        let started = Instant::now();
        let schedule = schedule(
            &tasks,
            &machine,
            ToolOrder::Free,
            Budget { work: budget },
            &cancel,
        )
        .unwrap();
        let seconds = started.elapsed().as_secs_f64();
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for v in &schedule.visits {
            for x in [v.task, v.reverse as u32, v.seam] {
                hash = (hash ^ x as u64).wrapping_mul(0x100_0000_01b3);
            }
        }
        println!(
            "{path}\ttasks={n}\troute_s={seconds:.3}\ttravel_s={:.3}\ttravel_mm={:.1}\twork={}\thash={hash:016x}",
            schedule.cost.travel_s, schedule.cost.travel_mm, schedule.work
        );
        total.0 += seconds;
        total.1 += schedule.cost.travel_s;
        total.2 += schedule.cost.travel_mm;
    }
    println!(
        "TOTAL\troute_s={:.3}\ttravel_s={:.3}\ttravel_mm={:.1}",
        total.0, total.1, total.2
    );
}
