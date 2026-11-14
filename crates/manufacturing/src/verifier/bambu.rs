// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0

//! Independent decoding of Bambu Lab display markers in a program: `M73 P R`
//! progress, `M73 L` layer number and the `M991 S0 P` layer-change
//! notification that must follow it. Markers move nothing and change no
//! state the replay relies on; they are checked for form, order and their
//! correspondence to the plan's layers. Progress values are producer
//! display data: only their range and monotonicity are checked.

use super::*;

#[derive(Default)]
pub(super) struct Markers {
    /// Layer markers accepted so far.
    layers: u32,
    /// `M991 S0 P<k>` owed after `M73 L<k+1>`.
    notify: Option<u32>,
    /// Last `M73 P R` values.
    progress: Option<(u32, u32)>,
}

fn integer(word: &str, prefix: char) -> Option<u32> {
    let digits = word.strip_prefix(prefix)?;
    if digits.is_empty()
        || !digits.bytes().all(|b| b.is_ascii_digit())
        || (digits.len() > 1 && digits.starts_with('0'))
    {
        return None;
    }
    digits.parse().ok()
}

impl Markers {
    /// Consume a marker line, returning whether `line` was one. Any other
    /// command while a layer notification is owed fails.
    pub fn accept(&mut self, line: &str, number: usize) -> Result<bool, ManufacturingError> {
        let code = line.split_once(';').map_or(line, |(code, _)| code).trim();
        let words: Vec<&str> = code.split_ascii_whitespace().collect();
        let fail = |what: &str| Err(failed(format!("program line {number}: {what}")));
        match words.as_slice() {
            ["M73", p, r] if p.starts_with('P') => {
                let (Some(p), Some(r)) = (integer(p, 'P'), integer(r, 'R')) else {
                    return fail(&format!("malformed Bambu progress marker {code:?}"));
                };
                if self.notify.is_some()
                    || p > 100
                    || self
                        .progress
                        .is_some_and(|(last_p, last_r)| p < last_p || r > last_r)
                {
                    return fail("Bambu progress must rise to 100 % while time remaining falls");
                }
                self.progress = Some((p, r));
                Ok(true)
            }
            ["M73", l] => {
                let Some(l) = integer(l, 'L') else {
                    return fail("malformed Bambu layer marker");
                };
                if self.notify.is_some() || Some(l) != self.layers.checked_add(1) {
                    return fail("Bambu layer markers must number each layer once, in order");
                }
                self.layers = l;
                self.notify = Some(l - 1);
                Ok(true)
            }
            ["M991", "S0", p] => {
                if integer(p, 'P').is_none() || integer(p, 'P') != self.notify {
                    return fail("Bambu layer notification must follow its layer marker");
                }
                self.notify = None;
                Ok(true)
            }
            [first, ..] if first.starts_with("M73") || first.starts_with("M991") => {
                fail("unsupported Bambu marker form")
            }
            [] => Ok(false),
            _ if self.notify.is_some() => fail("Bambu layer marker without its notification"),
            _ => Ok(false),
        }
    }
    /// A motion of scheduled `layer` requires that layer's marker.
    pub fn before_layer_motion(&self, layer: u32) -> Result<(), ManufacturingError> {
        if self.notify.is_some() || Some(self.layers) != layer.checked_add(1) {
            return Err(failed(
                "Bambu layer marker missing before the layer's first motion",
            ));
        }
        Ok(())
    }
    pub fn finish(&self, layers: usize) -> Result<(), ManufacturingError> {
        if self.layers as usize != layers
            || self.notify.is_some()
            || self.progress != Some((100, 0))
        {
            return Err(failed(
                "Bambu program must mark every layer and end at 100 % progress",
            ));
        }
        Ok(())
    }
}
