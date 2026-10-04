<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Supervised native client rules

- Share this client between CLI and desktop shell; do not duplicate pipe/protocol state machines in either consumer.
- One mutable client permits one request at a time. Correlate response IDs strictly and bound reads before allocation. Five-second request/exit timeouts are part of the contract.
- Spawn performs mandatory hello; observe OS process status. Failure/cancellation kills the child; explicit shutdown waits for acknowledgement and successful process exit. Drop uses Tokio kill-on-drop.
- Stderr is inherited; stdout is reserved for frames. No Tauri or domain/planner dependencies.
- Run `cargo test -p spiling-engine-client` and `cargo test -p spiling-engine` for actual child lifecycle coverage. See [control protocol](../../docs/protocol/control.md).
