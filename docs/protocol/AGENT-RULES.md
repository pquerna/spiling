<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Wire specifications rules

- Keep control.md and implemented geometry/project/manufacturing specifications synchronized with generated Protobuf native services, checked Rust domain adapters, generated TypeScript views and engine/client behavior. Protobuf owns RPC authority; persistence JSON and packed binary formats have separate explicit schemas. Do not create a second executable wire model here.
- Distinguish configured future kernel identity from actual capabilities; document bounded failure and cleanup behavior explicitly.
- Verification uses contract regressions, actual child tests, and CLI cross-language smoke, not documentation assertions.
