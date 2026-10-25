<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Native geometry examples: rules

- Original STEP inputs are frozen source assets, not recipes to rebuild with the current kernel. The retired Monstertruck corpus/operator builders remain historical provenance in manifests. Run `node tools/verify-fixtures.mjs` for immutable bytes, hashes and licensing; this does not qualify current geometry.
- Positive sources must bound outward material, not merely close topologically. Preserve original profile gauges and oriented caps/walls; never compensate for source defects through hidden conversion inversion. Changed source requires a new name.
- `inspect_corpus` exercises the public geometry facade and reports observed admission, packed mesh audits, tight directional support brackets and native section numerics. Face inspection uses `FaceInfo.geometry`, retaining original source-face and source-surface entity kinds. It must not access private kernel storage or invent engine/session behavior.
- `generate_native_goldens` encodes actual native box/through-hole output through contracts' builders with fixed fixture-only identities. Keep these separate from kernel-independent algebraic vectors; generator `--check` detects byte drift. It does not simulate engine/session lifecycle.
- Keep deterministic fixture recipes and licensing with [the corpus](../../../fixtures/geometry/AGENT-RULES.md); acceptance evidence belongs in [geometry evidence](../../../docs/quality/geometry-evidence.md).
- Exercise these binaries with `cargo run --locked -p spiling-geometry --example <name> -- <arguments>`; compilation or unit tests alone are not facade evidence.
