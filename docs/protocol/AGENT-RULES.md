<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Wire specifications rules

- Keep control.md synchronized with Rust contracts, generated TypeScript, engine behavior, and client lifecycle. Do not create a second executable schema here.
- Distinguish configured future kernel identity from actual capabilities; document bounded failure and cleanup behavior explicitly.
- Verification uses contract regressions, actual child tests, and CLI cross-language smoke, not documentation assertions.
- Keep [packed display layouts](display.md) synchronized with Rust encoders/validators and independent TypeScript decoders. Algebraic interoperability fixtures are not native STEP/section acceptance evidence.
- Keep [software manufacturing](manufacturing.md) and [project format](project.md) synchronized with strict v4/format2 contracts. Independent replay on compile/reopen is mandatory; stored report flags, profiles and component metadata do not establish physical support.
