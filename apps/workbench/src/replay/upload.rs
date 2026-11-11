// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! One cancellable, backpressured page decode stream for the current Review.
use super::{Batch, beads::Instance};
use std::sync::{
    Arc,
    mpsc::{self, Receiver},
};
pub type Prepared = Result<(usize, Arc<Vec<Instance>>), String>;
pub fn start(batches: &[Batch]) -> Result<Receiver<Prepared>, String> {
    let tables = batches
        .iter()
        .map(|b| b.instances.clone())
        .collect::<Vec<_>>();
    let (send, receive) = mpsc::sync_channel(2);
    std::thread::Builder::new()
        .name("replay-pages".into())
        .spawn(move || {
            for (index, table) in tables.into_iter().enumerate() {
                let page = table.load().map(|data| (index, data));
                let failed = page.is_err();
                if send.send(page).is_err() || failed {
                    break;
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(receive)
}
