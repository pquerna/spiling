<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Native desktop shell vision

Provide a real, packaged Chromium Embedded Framework host for the Spiling workbench while keeping native manufacturing authority in a separately supervised Rust engine. The shell exposes a narrow binary bridge and clear engine lifecycle diagnostics, not an alternate engine or browser preview.

B0 serves one main workbench window and one lazily started sidecar. It transports the engine's diagnostic triangle as a raw IPC response, reports runtime/build identity, and supports stop/restart, deliberate interruption and protocol-mismatch diagnostics. UI assent and WebGPU admission remain frontend responsibilities. Local packaging includes source-access information, OSL text, attribution and resolved dependency notices.

Windows, macOS and Linux packaging use the upstream CEF-aware Tauri CLI; implementation is not evidence that all three are verified. CEF's Windows sandbox limitation and constrained Linux AppImage behavior remain upstream support limits. Acceptance requires real invokes, a WebGPU-rendered engine triangle, restart recovery, child-process cleanup and inspection of an installed distribution on every claimed reference platform. Root/headless SwiftShader diagnostics remain explicitly experimental, not substitutes for those gates.

No system-webview fallback, kernel linkage, planner, manufacturing command replay or custom CEF runtime belongs here. Future native dialogs must remain capability-scoped and cannot weaken the engine/client ownership boundary.
