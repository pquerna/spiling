<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Desktop engine supervision vision

Give the workbench an honest recoverable protocol-4 authoring session: dead engines never report running; typed geometry/project/manufacturing failures preserve a healthy child; stop differs from interruption. Shared EngineClient owns transport/supervision; core owns transactions/storage; this module translates short project/geometry exchanges and common job metadata into native invokes without a manufacturing invoke surface.

One lazily started child begins with a new unattached project. Source/project NativePaths remain shell-private behind bounded, consumed, intent/session-bound tokens. The shell remembers only an attached checkpoint route for explicit restart/reopen; it never implements persistence or replays unsaved edits. Startup-gated fixed-root fixtures exercise production admission without widening filesystem authority.

Native jobs remain in the engine. The shell performs short exchanges and releases its mutex between status/cancel calls. It retains no kernel objects, scheduler, project persistence, unbounded transfer accumulator or command replay.

Acceptance requires real-process interoperability, fatal/recoverable error distinctions, actual dialog/token operations and no surviving sidecar after stop/window close—including an in-flight exchange or native job. Frontend/GPU and distributable platform evidence remain separate gates.
