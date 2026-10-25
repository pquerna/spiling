// SPDX-FileCopyrightText: 2026 Spiling contributors
// SPDX-License-Identifier: OSL-3.0
// Licensed under the Open Software License version 3.0

//! Immutable original inputs. Historical writer provenance lives beside the bytes;
//! fixture presence grants no current geometry or manufacturing authority.

pub(crate) const BLIND_POCKET: &[u8] =
    include_bytes!("../../../fixtures/geometry/retained-sources/native-parity-blind-pocket.step");
pub(crate) const BLIND_POCKET_NURBS_FLOOR: &[u8] = include_bytes!(
    "../../../fixtures/geometry/retained-sources/native-parity-blind-pocket-nurbs-floor.step"
);
pub(crate) const DENOMINATOR_ZERO: &[u8] = include_bytes!(
    "../../../fixtures/geometry/retained-sources/native-parity-denominator-zero.step"
);
