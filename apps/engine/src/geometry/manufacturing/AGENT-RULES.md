<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# Manufacturing runtime rules

Compose core intent/artifact authority and the public native manufacturer; own no planner, verifier, persistence implementation or printer execution. Checked domain commands use captured ProjectRevision/session checks and typed authenticated gRPC adapters. Reuse the single dedicated native dispatcher/worker, cancellation watchdog, private execution correlations and promotion ACK; the durable Jobs ledger alone exposes public Google Operations. Reject cancelled or stale results before committing PublishManufacturing. After successful intent/publication commits, reconcile native pins with core retained_sources because committing may clear redo or evict source-owning history. Intent/publication and their undo/redo do not advance scene epochs or regenerate geometry.

Compile captures only current stored definitions and occurrences; native definitions remain immutable worker-owned Arc pins. Account old/current/history plus staged encoded bundles against the separate 64 MiB retained budget and the 16 MiB per-bundle cap before serialization growth. Open narrows incoming storage allocation to remaining capacity. Failed stages drop all captured/staged resources and preserve current state.

Open and Inspect independently replay exact persisted bundle content through verify_bundle; never trust a stored verified flag. Read-only permits inspection and immutable retrieval but forbids intent changes and compilation publication. After promotion ACK/core commit, expose retained core ManufacturingAsset Arc pins to the host without copying the bundle Vec. The host persists separately budgeted durable bytes and binds real immutable artifact descriptors to status/results; authenticated ByteStream alone retrieves bytes. No framed metadata, raw writer or public native chunk command remains.

Actual-child regressions in apps/engine/tests/manufacturing.rs cover compilation, replay, transfer, history/invalidation, cancellation/staleness, read-only, corruption and fresh-process source-independent reopen. Exercise native engine/client suites and the actual CLI workflow; software validation is not physical printer certification.
