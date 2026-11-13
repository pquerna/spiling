<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# CI rules

- Actions are pinned to immutable commits; native packaging uses the pinned Rust toolchain without Node/pnpm.
- Automatic remote CI is disabled. Local checks remain required; native packaging is explicit workflow dispatch.
- Use read-only repository permissions. Never expose credentials to untrusted pull-request code.
- Uploaded development packages are not signed/public releases or hardware acceptance evidence. Physical GPU support requires recorded reference-machine runs.
- Native packaging is manually dispatched for a selected Windows/macOS/Linux platform; `all` is an explicit three-build cost choice. Keep Linux native prerequisites explicit. Native `cargo xtask check` / `test` and optional `pnpm check:tooling` / `test:tooling` remain local pre-merge/release obligations.
- Bound checks to 10 minutes and packaging to 30 minutes. Cache lean Rust artifacts/dependency downloads by pinned toolchain and lockfile, not every commit; do not cache installers. Uploaded packages expire after three days.
- Fail on checked contract drift, compiler/type errors, behavior or packaging failures; never hide an unsupported platform with a fallback runtime.
