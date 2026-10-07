<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Supervised native gRPC client rules

- Share EngineRpc between CLI and shell. Cloneable RPC access does not own the process; EngineClient exclusively owns child lifetime and the stdin liveness pipe.
- Authenticate with a per-launch capability delivered on inherited stdin, never arguments/logs/files. Validate bounded startup metadata, IPv4 loopback endpoint, OS child PID and instance identity.
- Unary exchanges have five-second deadlines; watchers are long-lived. Dropping/expiring a call never cancels accepted work or kills the child. CancelOperation explicitly requests cancellation.
- Binary reads enforce allocation limits, exact total length and SHA-256. Consume the standard ByteStream API; no base64 or second framing implementation.
- Shutdown waits for clean observed exit; terminate and owner Drop kill/reap the owned child. Temporary diagnostic stores last only for the owner; spawn_in uses explicit retained storage.
- Exercise real child lifecycle, retries, cancellation, recovery and streaming via cargo test -p spiling-engine.
