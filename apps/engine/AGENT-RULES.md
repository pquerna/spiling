<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Native engine sidecar rules

- Reserve stdout exclusively for frames. Emit diagnostics as one JSON object per stderr line.
- Require exactly one successful hello before commands; reject mismatch as upgrade_required and exit nonzero without executing commands. Request IDs strictly increase.
- Use contracts framing and schema; never expose Rust memory layouts. B0 reports monstertruck as configured future default with empty capabilities, not as a linked CAD kernel.
- Fatal malformed requests/frames exit nonzero; EOF between frames exits cleanly. Shutdown acknowledges then exits successfully.
- Run `cargo test -p spiling-engine` for real child/pipe tests. See [control protocol](../../docs/protocol/control.md).
