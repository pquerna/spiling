// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Chunked stages split into a wide part and a thin part. The wide
//! preparation of a chunk reads only final inputs, so chunk k+1 is prepared on
//! the pool (its batches and kernel rounds included) while the thin, ordered
//! consumption of chunk k runs on this thread. Nothing is speculative: every
//! prepared chunk is consumed exactly once, in order.

/// Prepare each chunk in order and consume each prepared chunk in order,
/// preparing chunk k+1 while chunk k is consumed. `prepare` and `consume`
/// share no mutable state, so the result equals the serial loop
/// `for c in chunks { consume(c, prepare(c)?)? }`. The first failure in that
/// serial order is returned; `prepare` may have run one chunk further.
pub(crate) fn pipelined<C, P, E>(
    chunks: impl IntoIterator<Item = C>,
    mut prepare: impl FnMut(C) -> Result<P, E> + Send,
    mut consume: impl FnMut(C, P) -> Result<(), E>,
) -> Result<(), E>
where
    C: Copy + Send,
    P: Send,
    E: Send,
{
    let mut chunks = chunks.into_iter();
    let Some(first) = chunks.next() else {
        return Ok(());
    };
    let mut current = (first, prepare(first)?);
    loop {
        let next = chunks.next();
        let mut prepared = None;
        let (chunk, value) = current;
        // The consuming closure stays on this thread, so it may hold
        // thread-local state; only the preparation crosses to the pool.
        let consumed = rayon::in_place_scope(|scope| {
            if let Some(next) = next {
                let prepare = &mut prepare;
                let prepared = &mut prepared;
                scope.spawn(move |_| {
                    *prepared = Some(prepare(next).map(|value| (next, value)));
                });
            }
            consume(chunk, value)
        });
        consumed?;
        match prepared {
            Some(result) => current = result?,
            None => return Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consumes_every_prepared_chunk_once_in_order() {
        let mut prepared = Vec::new();
        let mut consumed = Vec::new();
        let result: Result<(), ()> = pipelined(
            0..7,
            |chunk| {
                prepared.push(chunk);
                Ok(chunk * 10)
            },
            |chunk, value| {
                consumed.push((chunk, value));
                Ok(())
            },
        );
        assert_eq!(result, Ok(()));
        assert_eq!(prepared, (0..7).collect::<Vec<_>>());
        assert_eq!(consumed, (0..7).map(|c| (c, c * 10)).collect::<Vec<_>>());
    }

    #[test]
    fn returns_the_first_failure_in_serial_order() {
        let mut consumed = Vec::new();
        let result = pipelined(
            0..5,
            |chunk| if chunk == 3 { Err(chunk) } else { Ok(chunk) },
            |chunk, _| {
                consumed.push(chunk);
                if chunk == 2 { Err(100 + chunk) } else { Ok(()) }
            },
        );
        // Consuming chunk 2 fails before chunk 3's preparation counts.
        assert_eq!(result, Err(102));
        assert_eq!(consumed, [0, 1, 2]);
        let result = pipelined(
            0..5,
            |chunk| if chunk == 3 { Err(chunk) } else { Ok(chunk) },
            |_, _| Ok(()),
        );
        assert_eq!(result, Err(3));
        let empty: Result<(), ()> = pipelined(0..0, |_| Ok(()), |_, ()| Ok(()));
        assert_eq!(empty, Ok(()));
    }
}
