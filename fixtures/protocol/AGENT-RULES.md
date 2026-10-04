<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Original protocol interoperability fixtures rules

- triangle.json stores the exact canonical 64-byte synthetic diagnostic payload as hex; its .license sidecar attributes original Spiling authorship. No customer or imported CAD data.
- Rust compares emitted bytes with this fixture; TypeScript decodes the same bytes and exercises adversarial mutations. Update schema authority and both consumers together when changing it.
- Preserve sidecar attribution in distributions. See [control protocol](../../docs/protocol/control.md).
