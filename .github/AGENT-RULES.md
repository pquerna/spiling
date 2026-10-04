<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# CI rules

- Actions are pinned to immutable commits; Rust, Node and pnpm versions match repository tooling.
- Windows/macOS/Linux jobs run real engine checks and native CEF packaging. Keep Linux native prerequisites explicit.
- Use read-only repository permissions. Never expose credentials to untrusted pull-request code.
- Uploaded development packages are not signed/public releases or hardware acceptance evidence. Physical GPU support requires recorded reference-machine runs.
- Fail on contract drift, compiler/type errors, tests or packaging failures; never hide an unsupported platform with a fallback runtime.
