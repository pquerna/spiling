<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Desktop engine supervision vision

Give the workbench an honest, recoverable view of its native sidecar: never mistake a dead process for a running session or an intentional stop for a crash. The shared client owns transport; this area translates its lifecycle into desktop-facing states and raw diagnostic transfers.

B0 keeps a single lazily started child, returns its negotiated hello, detects exits on status, and makes stop/restart/interrupt/close cleanup explicit. It does not schedule manufacturing jobs, retain domain data, auto-replay commands or spawn speculative engines. Multiple project sessions need a separately agreed ownership contract rather than expanding this singleton silently.

Acceptance is real-process interoperability with the frontend plus no surviving sidecar after window/application close, including close during an in-flight operation. Failure messages must survive status polling until an explicit recovery action replaces the session.
