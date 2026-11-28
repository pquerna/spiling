// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Chunked stages split into a wide part and a thin part. A chunk's load
//! runs in chunk order on its own thread, ahead of consumption; its wide
//! build (batches and kernel rounds included) reads only final inputs, so
//! several chunks build on the pool while the thin, ordered consumption of
//! an earlier chunk runs on this thread. Nothing is speculative: every built
//! chunk is consumed exactly once, in order.

use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::Duration;

/// A built value, filled once by its build and taken once by the consumer.
type Slot<T> = Arc<(Mutex<Option<T>>, Condvar)>;

/// Load each chunk in order on a loader thread, build each loaded chunk on
/// the pool, and consume each built chunk in order on this thread, with
/// about `ahead` chunks loaded and building beyond the one being consumed.
/// The result equals the serial loop
/// `for c in chunks { consume(c, build(c, load(c)?)?)? }`; the first failure
/// in that order is returned. Loading may run that far past a failure, and
/// every chunk loaded is built before this returns.
pub(crate) fn pipelined<C, L, P, E>(
    chunks: impl IntoIterator<Item = C, IntoIter: Send>,
    ahead: usize,
    mut load: impl FnMut(C) -> Result<L, E> + Send,
    build: impl Fn(C, L) -> Result<P, E> + Sync,
    mut consume: impl FnMut(C, P) -> Result<(), E>,
) -> Result<(), E>
where
    C: Copy + Send,
    L: Send,
    P: Send,
    E: Send,
{
    let build = &build;
    let chunks = chunks.into_iter();
    rayon::in_place_scope(|scope| {
        std::thread::scope(|threads| {
            let (sender, receiver) =
                mpsc::sync_channel::<(C, Result<Slot<Result<P, E>>, E>)>(ahead);
            threads.spawn(move || {
                for chunk in chunks {
                    let entry = load(chunk).map(|loaded| {
                        let slot: Slot<Result<P, E>> = Arc::default();
                        let filled = Arc::clone(&slot);
                        scope.spawn(move |_| fill(&filled, build(chunk, loaded)));
                        slot
                    });
                    let last = entry.is_err();
                    // The consumer stopped early: load nothing more.
                    if sender.send((chunk, entry)).is_err() || last {
                        return;
                    }
                }
            });
            // Returning drops the receiver, which ends the loader.
            while let Some((chunk, entry)) = receive(&receiver) {
                consume(chunk, take(&entry?)?)?;
            }
            Ok(())
        })
    })
}

fn fill<T>(slot: &Slot<T>, value: T) {
    let (cell, ready) = &**slot;
    *cell.lock().unwrap_or_else(|poison| poison.into_inner()) = Some(value);
    ready.notify_one();
}

/// How long the consumer blocks before it runs pending pool work instead, so
/// a pool thread waiting here never starves the work it waits for.
const PATIENCE: Duration = Duration::from_millis(1);

/// The next loaded chunk, or `None` once the loader has finished.
fn receive<T>(receiver: &mpsc::Receiver<T>) -> Option<T> {
    loop {
        match receiver.recv_timeout(PATIENCE) {
            Ok(value) => return Some(value),
            Err(mpsc::RecvTimeoutError::Disconnected) => return None,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                rayon::yield_now();
            }
        }
    }
}

/// Take a built value once its build has filled it.
fn take<T>(slot: &Slot<T>) -> T {
    let (cell, ready) = &**slot;
    loop {
        {
            let value = cell.lock().unwrap_or_else(|poison| poison.into_inner());
            let (mut value, _) = ready
                .wait_timeout_while(value, PATIENCE, |value| value.is_none())
                .unwrap_or_else(|poison| poison.into_inner());
            if let Some(value) = value.take() {
                return value;
            }
        }
        // The lock is released: the build may be the pending work run here.
        rayon::yield_now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consumes_every_chunk_once_in_order() {
        for ahead in [0, 1, 3, 16] {
            let mut loaded = Vec::new();
            let mut consumed = Vec::new();
            let built = std::sync::atomic::AtomicUsize::new(0);
            let result: Result<(), ()> = pipelined(
                0..9,
                ahead,
                |chunk| {
                    loaded.push(chunk);
                    Ok(chunk * 10)
                },
                |chunk, value| {
                    built.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    // Later chunks finish first when built together.
                    std::thread::sleep(std::time::Duration::from_micros(200 * (9 - chunk) as u64));
                    Ok(value + 1)
                },
                |chunk, value| {
                    consumed.push((chunk, value));
                    Ok(())
                },
            );
            assert_eq!(result, Ok(()));
            assert_eq!(loaded, (0..9).collect::<Vec<_>>());
            assert_eq!(built.into_inner(), 9);
            assert_eq!(
                consumed,
                (0..9).map(|c| (c, c * 10 + 1)).collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn returns_the_first_failure_in_serial_order() {
        let run = |load_fails: usize, build_fails: usize, consume_fails: usize| {
            let mut consumed = Vec::new();
            let result = pipelined(
                0..8,
                3,
                |chunk| {
                    if chunk == load_fails {
                        Err(("load", chunk))
                    } else {
                        Ok(chunk)
                    }
                },
                |chunk, _| {
                    if chunk == build_fails {
                        Err(("build", chunk))
                    } else {
                        Ok(chunk)
                    }
                },
                |chunk, _| {
                    if chunk == consume_fails {
                        return Err(("consume", chunk));
                    }
                    consumed.push(chunk);
                    Ok(())
                },
            );
            (result, consumed)
        };
        assert_eq!(run(5, 9, 2), (Err(("consume", 2)), vec![0, 1]));
        assert_eq!(run(2, 4, 9), (Err(("load", 2)), vec![0, 1]));
        assert_eq!(run(6, 3, 9), (Err(("build", 3)), vec![0, 1, 2]));
        assert_eq!(run(9, 9, 9), (Ok(()), (0..8).collect()));
    }
}
