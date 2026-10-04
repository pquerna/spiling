<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# CI rules

- Actions are pinned to immutable commits; Rust, Node and pnpm versions match repository tooling.
- Automatic CI is one Linux job for material source/configuration changes: contract drift, TypeScript, selected engine/protocol behavior and real CLI interoperability. Do not compile CEF or run workspace Clippy on every push.
- Use read-only repository permissions. Never expose credentials to untrusted pull-request code.
- Uploaded development packages are not signed/public releases or hardware acceptance evidence. Physical GPU support requires recorded reference-machine runs.
- Native CEF packaging is manually dispatched for a selected Windows/macOS/Linux platform; `all` is an explicit three-build cost choice. Keep Linux native prerequisites explicit. Full `pnpm check` and `pnpm test` remain local pre-merge/release obligations.
- Bound checks to 10 minutes and packaging to 30 minutes. Cache lean Rust artifacts/dependency downloads by pinned toolchain and lockfile, not every commit; do not cache CEF distributions or installers. Uploaded packages expire after three days.
- Fail on checked contract drift, compiler/type errors, behavior or packaging failures; never hide an unsupported platform with a fallback runtime.
