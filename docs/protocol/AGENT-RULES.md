<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Wire specifications rules

- Keep control.md and implemented geometry/project/manufacturing specifications synchronized with generated Protobuf native services, checked Rust domain adapters, generated TypeScript views and engine/client behavior. Protobuf owns RPC authority; persistence JSON and packed binary formats have separate explicit schemas. Do not create a second executable wire model here.
- replay.md freezes SPLR actual-command display layout, bounds, first unknown-start endpoint and coincident-travel semantics. Keep descriptor binding/admission and native-generated cross-language golden provenance explicit; packed validation alone is not program verification or hardware proof.
- Image metadata is not image content: specify canonical PGM/hash/bounds, atomic Core admission, captured read-only native publication and verified ByteStream decoding separately. Keep native M, realized D, adaptive image scheduling and actual-deposition support dependencies distinct.
- Distinguish configured future kernel identity from actual capabilities; document bounded failure and cleanup behavior explicitly.
- Verification uses contract regressions, actual child tests, and CLI cross-language smoke, not documentation assertions.
