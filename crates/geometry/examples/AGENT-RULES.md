<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Native geometry examples: rules

- `generate_corpus` owns reproducible original analytic STEP construction; use the pinned public kernel writer and preserve genuine analytic carriers. It is not an import repair path.
- `inspect_corpus` exercises the public geometry facade and reports observed admission, packed mesh audits and native section numerics. It must not access private kernel storage or invent engine/session behavior.
- `generate_native_goldens` encodes actual native box/through-hole output through contracts' builders with fixed fixture-only identities. Keep these separate from kernel-independent algebraic vectors; generator `--check` detects byte drift. It does not simulate engine/session lifecycle.
- Keep deterministic fixture recipes and licensing with [the corpus](../../../fixtures/geometry/AGENT-RULES.md); acceptance evidence belongs in [geometry evidence](../../../docs/quality/geometry-evidence.md).
- Exercise these binaries with `cargo run --locked -p spiling-geometry --example <name> -- <arguments>`; compilation or unit tests alone are not facade evidence.
