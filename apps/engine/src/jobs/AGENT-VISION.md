<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Durable engine operations vision

Own one durable operation admission/observation/cancellation ledger and immutable output store for synthetic diagnostics and typed native geometry/project/manufacturing services. Preserve diagnostic incremental publication while requiring native success, exact fresh verification/provenance and optional packed actual-program motion to publish together only after native promotion ACK/Core commit. Cancellation before the decision discards staging; late cancellation cannot rewrite ACK-winning terminal success.

Separate durable history from live native ownership: completed descriptors/bytes survive reconnect, unfinished work becomes interrupted, and native session handles are never reconstructed or automatically replayed. Explicit budgets account for diagnostic reservations, native immutable output copies and separately bounded live/history/staged native resources. Real child lifecycle, state races, authentication, request retries, immutable range retrieval and software manufacturing recovery—not scheduler shape alone—govern acceptance.
