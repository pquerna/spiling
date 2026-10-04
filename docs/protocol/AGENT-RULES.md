<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Wire specifications rules

- Keep control.md synchronized with Rust contracts, generated TypeScript, engine behavior, and client lifecycle. Do not create a second executable schema here.
- Distinguish configured future kernel identity from actual capabilities; document bounded failure and cleanup behavior explicitly.
- Verification uses contract regressions, actual child tests, and CLI cross-language smoke, not documentation assertions.
