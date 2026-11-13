// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: MPL-2.0
// Cancellation extension to the MPL-2.0 boolmesh library.

#[cfg(panic = "abort")]
compile_error!("boolmesh cooperative cancellation requires panic=unwind");

use std::sync::atomic::{AtomicBool, Ordering};

// scoped-tls restores the previous borrowed flag on every exit, including unwind.
// Its safe scoped API avoids storing an unbounded-lifetime pointer in our code.
scoped_tls::scoped_thread_local!(static CANCEL: AtomicBool);

/// The operation was cooperatively interrupted; all its local storage was dropped.
#[derive(Debug, PartialEq, Eq)]
pub struct Cancelled;

/// Execute synchronous mesh work with cooperative cancellation, including operand
/// construction. Checkpoints inside the algorithm unwind only this scope. Real
/// panics keep their original payload. No panic hook is changed or invoked for
/// cancellation, and input meshes are borrowed rather than modified.
///
/// This API is intentionally unavailable with rayon: a thread-local flag cannot
/// authorize cancellation on another pool thread. A parallel adapter must carry
/// the flag into each worker scope before exposing that combination.
#[cfg(not(feature = "rayon"))]
pub fn with_cancellation<T>(flag: &AtomicBool, work: impl FnOnce() -> T) -> Result<T, Cancelled> {
    CANCEL.set(flag, || {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            checkpoint();
            let value = work();
            checkpoint();
            value
        })) {
            Ok(value) => Ok(value),
            Err(payload) if payload.is::<Cancelled>() => Err(Cancelled),
            Err(payload) => std::panic::resume_unwind(payload),
        }
    })
}

#[inline]
pub(crate) fn checkpoint() {
    #[cfg(test)]
    GATE.with(|gate| {
        let mut gate = gate.borrow_mut();
        if let Some((remaining, _, _)) = gate.as_mut() {
            *remaining -= 1;
            if *remaining == 0 {
                let (_, entered, resume) = gate.take().unwrap();
                entered.send(()).unwrap();
                resume.recv().unwrap();
            }
        }
    });
    if CANCEL.is_set() && CANCEL.with(|flag| flag.load(Ordering::Relaxed)) {
        std::panic::resume_unwind(Box::new(Cancelled));
    }
}

#[cfg(test)]
thread_local! {
    static GATE: std::cell::RefCell<Option<(usize, std::sync::mpsc::SyncSender<()>, std::sync::mpsc::Receiver<()>)>> = const { std::cell::RefCell::new(None) };
}

#[cfg(all(test, not(feature = "rayon")))]
mod tests {
    use super::*;
    #[test]
    fn cancellation_drops_work_and_restores_nested_scopes() {
        let outer = AtomicBool::new(false);
        let inner = AtomicBool::new(true);
        let mut dropped = false;
        struct DropWitness<'a>(&'a mut bool);
        impl Drop for DropWitness<'_> {
            fn drop(&mut self) {
                *self.0 = true;
            }
        }
        assert_eq!(
            with_cancellation(&outer, || {
                assert_eq!(with_cancellation(&inner, || unreachable!()), Err(Cancelled));
                checkpoint();
                let _guard = DropWitness(&mut dropped);
                outer.store(true, Ordering::Relaxed);
                checkpoint();
            }),
            Err(Cancelled)
        );
        assert!(dropped);
        checkpoint(); // No stale flag survives the scope.
    }
}

#[cfg(all(test, not(feature = "rayon")))]
#[test]
fn interrupts_inside_boolean_loops_and_preserves_operands() {
    use crate::{Manifold, OpType, compute_boolean};
    use std::sync::mpsc::sync_channel;
    let positions = [
        0., 0., 0., 1., 0., 0., 1., 1., 0., 0., 1., 0., 0., 0., 1., 1., 0., 1., 1., 1., 1., 0., 1.,
        1.,
    ];
    let indices = [
        0, 2, 1, 0, 3, 2, 4, 5, 6, 4, 6, 7, 0, 1, 5, 0, 5, 4, 1, 2, 6, 1, 6, 5, 2, 3, 7, 2, 7, 6,
        3, 0, 4, 3, 4, 7,
    ];
    let a = Manifold::new(&positions, &indices).unwrap();
    let mut shifted = positions;
    for p in shifted.chunks_mut(3) {
        p[0] += 0.5;
    }
    let b = Manifold::new(&shifted, &indices).unwrap();
    let flag = AtomicBool::new(false);
    let (entered, observe) = sync_channel(0);
    let (resume, waiting) = sync_channel(0);
    std::thread::scope(|scope| {
        let cancellation = &flag;
        scope.spawn(move || {
            observe.recv().unwrap();
            cancellation.store(true, Ordering::Release);
            resume.send(()).unwrap();
        });
        // The first scope/algorithm checkpoints pass; synchronization occurs in
        // actual Boolean loop work, after both input operands were constructed.
        GATE.with(|gate| *gate.borrow_mut() = Some((64, entered, waiting)));
        let result = with_cancellation(&flag, || compute_boolean(&a, &b, OpType::Add));
        assert!(matches!(result, Err(Cancelled)));
    });
    assert!(compute_boolean(&a, &b, OpType::Add).unwrap().is_manifold());
}
