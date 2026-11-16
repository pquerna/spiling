// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! `TolerantModel::section_demand` plus what `section_admitted` asks to admit bounds what a
//! section allocates, measured by this binary's allocator on this thread: the most heap held at
//! once during the section, and the heap its result keeps.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use spiling_contracts::geometry::{GeometryError, GeometryErrorCode};
use spiling_geometry::from_step_bytes;
use spiling_geometry::tolerant::{TolerantModel, TolerantPolicy};

struct Counting;

thread_local! {
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
}

fn record(delta: isize) {
    // Accounting must not allocate; a thread being torn down is not measured.
    let _ = LIVE.try_with(|live| {
        let now = live.get() + delta;
        live.set(now);
        let _ = PEAK.try_with(|peak| peak.set(peak.get().max(now)));
    });
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            record(layout.size() as isize);
        }
        p
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
        record(-(layout.size() as isize));
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // A move is a new buffer with the old still live; count it so.
        let p = unsafe { System.realloc(ptr, layout, new_size) };
        if !p.is_null() {
            record(new_size as isize);
            record(-(layout.size() as isize));
        }
        p
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn model(name: &str, policy: TolerantPolicy) -> TolerantModel {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../fixtures/geometry")
        .join(name);
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let definition = from_step_bytes(&bytes).unwrap_or_else(|e| panic!("{name}: {}", e.message));
    TolerantModel::build(&definition, policy, &AtomicBool::new(false))
        .unwrap_or_else(|e| panic!("{name}: {}", e.message))
}

/// Peak heap held during the section, heap its result keeps (both from before it), bytes it
/// asked to admit, and its loops.
fn measured(model: &TolerantModel, z: f64) -> (usize, usize, usize, usize) {
    let cancel = AtomicBool::new(false);
    let mut admitted = 0usize;
    let mut admit = |bytes: usize| -> Result<(), GeometryError> {
        admitted += bytes;
        Ok(())
    };
    let before = LIVE.with(Cell::get);
    PEAK.with(|peak| peak.set(before));
    let section = model
        .section_admitted(z, &cancel, &mut admit)
        .expect("sections");
    let peak = PEAK.with(Cell::get) - before;
    let kept = LIVE.with(Cell::get) - before;
    let loops = section.loops.len();
    drop(section);
    (peak.max(0) as usize, kept.max(0) as usize, admitted, loops)
}

#[test]
fn a_section_allocates_within_its_demand() {
    let fine = TolerantPolicy {
        join_mm: 0.01,
        profile_mm: 0.0005,
    };
    let cases = [
        ("cylinder.step", TolerantPolicy::default()),
        ("cylinder.step", fine),
        ("through-hole.step", TolerantPolicy::default()),
        ("perforated-plate.step", TolerantPolicy::default()),
        ("analysis-hook.step", TolerantPolicy::default()),
        ("independent-exporters/occt-cylinder-mm.step", fine),
    ];
    let mut sectioned = 0;
    for (name, policy) in cases {
        let model = model(name, policy);
        let (min, max) = model.bounds();
        for k in 1..20 {
            let z = min[2] + (max[2] - min[2]) * f64::from(k) / 20.0;
            let demand = model.section_demand(z).expect("demand");
            let (peak, kept, admitted, loops) = measured(&model, z);
            assert!(
                peak <= demand.scratch_bytes + demand.output_bytes + admitted,
                "{name} at {z}: peak {peak} over {demand:?} and {admitted} admitted"
            );
            assert!(
                kept <= demand.output_bytes + admitted,
                "{name} at {z}: kept {kept} over {demand:?} and {admitted} admitted"
            );
            if loops > 0 {
                sectioned += 1;
            }
        }
    }
    assert!(sectioned > 40, "the fixtures were sectioned: {sectioned}");
}

#[test]
fn a_cancelled_section_stops() {
    let model = model("cylinder.step", TolerantPolicy::default());
    let (min, max) = model.bounds();
    let cancel = AtomicBool::new(true);
    let error = model
        .section_admitted((min[2] + max[2]) * 0.5, &cancel, &mut |_| Ok(()))
        .expect_err("cancelled");
    assert_eq!(error.code, GeometryErrorCode::Cancelled);
}

/// A refusal from the caller's admission stops the section and comes back unchanged.
#[test]
fn a_refused_admission_stops_the_section() {
    let model = model("cylinder.step", TolerantPolicy::default());
    let (min, max) = model.bounds();
    let cancel = AtomicBool::new(false);
    let mut asked = 0;
    let error = model
        .section_admitted((min[2] + max[2]) * 0.5, &cancel, &mut |_| {
            asked += 1;
            Err(GeometryError::new(
                GeometryErrorCode::ResourceLimit,
                "caller budget",
            ))
        })
        .expect_err("refused");
    assert_eq!(
        (error.code, error.message.as_str()),
        (GeometryErrorCode::ResourceLimit, "caller budget")
    );
    assert_eq!(asked, 1, "the section stops at the first refusal");
}

#[test]
fn a_height_off_the_model_demands_nothing() {
    let model = model("cylinder.step", TolerantPolicy::default());
    let (_, max) = model.bounds();
    let demand = model.section_demand(max[2] + 10.0).expect("demand");
    assert_eq!(demand.triangles, 0);
    assert!(model.section_demand(f64::NAN).is_err());
}
