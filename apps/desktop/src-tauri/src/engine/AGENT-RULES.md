<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Desktop engine supervision rules

- Use spiling-engine-client exclusively for bounded framed pipes, hello negotiation, request timeouts, shutdown acknowledgement, process wait/termination and kill-on-drop. [Control protocol](../../../../../docs/protocol/control.md) and the contracts crate own wire types.
- One asynchronous mutex serializes child ownership and complete operations, including restart and close cleanup. The closing atomic gate is checked before and after locking so queued commands cannot launch a sidecar during exit.
- Retain distinct stopped (explicit stop/close), running (live negotiated child), and interrupted (unexpected exit, launch/transport failure or diagnostic kill) states. Status checks the OS child before returning a snapshot; absent client means absent hello.
- Starting while live returns the same hello; restart cleans the old child before creating the new one. A failed graceful stop may terminate/reap the child, with a visible diagnostic and stopped state; failed forced cleanup is interrupted and rejects the command. Do not silently retry engine commands.
- Triangle invokes move the client's payload into tauri::ipc::Response. Never turn binary bytes into JSON arrays/base64, synthesize a triangle or define another control model.
- A failed operation removes and terminates the client, preserving the reason. Diagnostic interrupt is intentionally interrupted, not stopped. EngineClient's kill-on-drop remains the final resource-ownership guarantee after cleanup failure.
- Engine discovery is executable-relative in installed builds; development override is explicit and absolute, never a cwd search. Protocol override errors are surfaced, not silently converted to the default.
- Exercise lifecycle and error cases through the actual invoke bridge and OS process tree. Main integration owns builds/tests; no alternate in-process engine is acceptable test evidence.
