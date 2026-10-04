<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# CLI diagnostic consumer rules

- Use EngineClient for all engine operations; do not implement a second handshake/transport or geometry path.
- Support diagnose and triangle, engine path override, protocol version override, and triangle output. stdout reports machine-readable JSON only after successful shutdown; errors are stderr JSON and nonzero exit.
- With no override, resolve the engine beside the CLI executable. File-output failures must clean up the child.
- Exercise real CLI against a built engine: `spiling-cli diagnose --engine PATH`, `spiling-cli triangle --engine PATH --output FILE`, and mismatch with `--protocol-version 2`. See [control protocol](../../docs/protocol/control.md).
