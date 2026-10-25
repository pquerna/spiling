<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Original native geometry corpus rules

Own original redistributable STEP sources, frozen bytes/hashes and explicit valid/adversarial outcomes. The controlled corpus retains historical Monstertruck provenance; its builders are retired. `independent-exporters/` separately owns original external-exporter diagnostic recipes, never copied third-party CAD. Changed source needs a new name/version. Preserve OSL sidecars and exporter licensing; never relabel upstream or customer assets.

`recipe.json` records historical construction; `scenes/` contains identity/placement evaluation recipes, not saved projects. Run `node tools/verify-fixtures.mjs` to check frozen manifest hashes, recorded byte counts and license sidecars without constructing geometry. Historical expected outcomes and producer revisions do not qualify SolidCore. `inspect_corpus` exercises the current facade independently. Source geometry, certified enclosures, packed transfer workloads and manufacturing acceptance remain distinct.

Exercise `cargo test --locked -p spiling-geometry` and the real native corpus smoke; see [geometry evidence](../../docs/quality/geometry-evidence.md).
