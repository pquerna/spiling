<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Original native geometry corpus rules

Own original redistributable STEP recipes, frozen bytes/hashes and explicit valid/adversarial outcomes. The controlled numerical corpus is generated through crates/geometry/examples/generate_corpus.rs; `independent-exporters/` separately owns original external-exporter diagnostic recipes, never copied third-party CAD. Freeze names/hashes before acceptance; changed source needs a new version/name. Perforation sizing must yield real >4 MiB native display bytes within frozen caps. Preserve OSL sidecars and exporter licensing; never relabel upstream or customer assets.

`recipe.json` is the original construction specification; `scenes/` contains identity/placement evaluation recipes, not saved projects. The generator validates all valid/adversarial outcomes and maintains canonical `.license` SPDX sidecars plus a computed SHA-256 `manifest.json`; `--check` requires exact deterministic drift equality. Existing frozen STEP bytes are never overwritten by regeneration. The 32×16 plate's source geometry is frozen after genuine bounded native packing; its required packed-byte range remains an acceptance invariant, not permission to alter geometry or limits. Physical recipe bounds and conservative native certified enclosures are distinct.

Exercise `cargo test --locked -p spiling-geometry` and the real native corpus smoke; see [geometry evidence](../../docs/quality/geometry-evidence.md).
