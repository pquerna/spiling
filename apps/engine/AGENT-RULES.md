<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Native engine sidecar rules

- Serve authenticated gRPC on ephemeral IPv4 loopback. Stdout is one bounded startup JSON message; structured diagnostics use stderr. stdin carries the launch capability and owner-liveness EOF.
- Parent closure and Shutdown stop the server with bounded drain. Do not block the async runtime on non-cancellable stdin reads.
- Compose operation/artifact services without Tauri. Use standard Google Operations and ByteStream; domain failures do not terminate the process.
- The worker is a bounded synthetic task, not linked CAD or isolated native geometry. Keep advertised geometry capabilities empty.
- Run cargo test -p spiling-engine for actual process/service coverage; see [protocol](../../docs/protocol/control.md).
