<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Native desktop shell vision

Provide one packaged Chromium Embedded Framework host with narrow typed gRPC authoring and durable diagnostic bridges to a separately supervised engine. EngineClient owns child/auth/startup/liveness; EngineRpc owns authenticated RPCs. Standard Google Operations observation/cancellation and ByteStream retrieval are shared across native and diagnostic work, with separately checked metadata/result views.

Native paths remain byte/code-unit preserving shell-private values behind bounded session/intent tokens. The shell remembers an attached checkpoint route only for explicit reopen, never implements storage or replays unsaved changes. Native import/shared placement/inspection/sections, bounded history and save/open/recovery/read-only snapshots use the same client as CLI. Recoverable domain errors preserve the child; retained historical results cannot become current session state. No manufacturing invokes or machine-ready claims.

Durable diagnostics preserve the application-local operation ledger, incremental triangle artifacts, progress/cancellation and restart interruption semantics. Validated native geometry chunks and diagnostic bytes move into raw IPC responses; no JSON/base64 tunnel or second scheduler exists.

Fixed-root fixture admission requires startup debugging opt-in and production tokens. It proves bridge/storage behavior without granting renderer filesystem authority. Real dialogs, visible WebGPU/picking, cancellation/pause/resume and process cleanup require physical reference-platform evidence. Windows/macOS/Linux upstream CEF packaging remains separately certified; headless software GPU and implemented packaging are not that evidence. No alternate webview or linked native kernel/planner belongs here.
