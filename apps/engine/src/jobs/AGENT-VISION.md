<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Durable engine operations vision

Own one durable operation admission/observation/cancellation ledger and immutable output store for synthetic diagnostics and typed native geometry/project/manufacturing services. Preserve diagnostic incremental publication while requiring native success and immutable output publication to follow native promotion ACK/Core commit.

Separate durable history from live native ownership: completed descriptors/bytes survive reconnect, unfinished work becomes interrupted, and native session handles are never reconstructed or automatically replayed. Explicit budgets account for diagnostic reservations, native immutable output copies and separately bounded live/history/staged native resources. Real child lifecycle, state races, authentication, request retries, immutable range retrieval and software manufacturing recovery—not scheduler shape alone—govern acceptance.
