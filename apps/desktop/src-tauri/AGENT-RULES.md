<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Native desktop shell rules

- Own CEF composition, native dialogs, local invoke ACLs, shared-client supervision and binary bridging only. Core owns project/storage; the shell retains private path admission and attached-checkpoint routing, never a kernel, planner, renderer or persistence implementation.
- Pin Tauri 3.0.0-alpha.4, tauri-build 3.0.0-alpha.3, runtime-cef 3.0.0-alpha.5 and rfd 0.17.2 exactly. CEF Rust bindings are 152.3.0+152.0.6 and the matching native distribution is 152.0.6. The direct runtime-cef dependency is upstream packaging detection; retain cef_entry_point, never Wry/custom unsafe helper-process code.
- Register every invoke in build.rs/main.rs and the local-only capability. The only core permission closes through deferred cleanup. No remote/broad shell/process/fs permissions. Diagnostic operation commands and raw artifacts retain main behavior and the application-local durable operation store.
- Schedule rfd picker/discard dialogs on the main application thread without the engine mutex. Enable xdg-portal only, no zenity/GTK fallback. Linux needs xdg-desktop-portal with a working backend. Selection is all-or-nothing, at most 32 sources; picker cancel preserves unconsumed tokens.
- Keep native filesystem bytes/code units private. SelectedSource/SelectedProjectPath expose session/UUID/bounded label and project intent, never NativePath. Source tokens and intent-scoped project tokens consume only on OperationAccepted, clear on stop/restart/interruption/close. Generic geometry refuses ImportPart; generic project refuses Open/Save. Token-only admission and validated raw geometry_chunk are separate invokes.
- Geometry chunks use EngineRpc::read_geometry_chunk with immutable ArtifactView, packed metadata and summary, preserving native hash/layout/revision limits separately from diagnostic engine_read_artifact limits. Raw response moves bytes without JSON/base64. Google Operations names are the only public observation/cancellation handles.
- Startup SPILING_GEOMETRY_FIXTURE_ROOT plus valid CEF debug opt-in enables confined relative fixture admission. Canonicalize existing sources/open directories; save requires a new name under a canonical existing confined parent. Reject absolute/traversing/symlink escape paths; mint production tokens. RuntimeInfo reveals only the gate.
- Debugging is disabled unless SPILING_CEF_DEBUG_PORT=1024..65535; exact SPILING_CEF_UNSANDBOXED=1 and SPILING_CEF_SOFTWARE_GPU=1 enable explicit diagnostics only. Preserve upstream sandbox warnings/limitations; software GPU is not production support evidence.
- Resolve installed sidecar adjacent to actual executable or installation-prefix bin for CEF Debian layout; no PATH/cwd/arbitrary ancestor search. Debug builds alone allow absolute SPILING_ENGINE_PATH. No frame negotiation, protocol override or compatibility shim.
- EngineClient owns child/startup/auth/liveness/shutdown; cloneable EngineRpc owns typed RPCs. Lifecycle mutex is never held over native operation observation or artifact download. Closing blocks admission, queries dirty state and asks native discard confirmation before awaited reap. Cancel restores admission. Frontend GPU admission precedes spawning.
- Forward Hello/native capabilities without a second handshake model. Typed domain errors preserve a healthy child. No manufacturing invoke/planning/replay. Project route attachment requires the current operation's open Scene or save ProjectSaved receipt; postcommit save_uncertain may attach but sticky status from a prior save cannot settle later failure. Restart/reopen restores only attached saved storage.
- Root tools generate contracts/notices, copy the target-suffixed sidecar and use pinned CEF-aware CLI. CEF_PATH defaults to ignored .cache/cef. Ship root license/source/NOTICE and generated release provenance, preserving icon sidecars. No unrequested CEF/package builds.
- Acceptance: actual diagnostic/native token/operation/chunk/cancel/restart/close invokes with no orphan child. tools/smoke-desktop.mjs separates durable diagnostic, project-bridge and visual geometry scenarios. Bridge/source-gate tests certify neither dialogs, physical WebGPU nor distributables.

Protocol: ../../../docs/protocol/control.md. Licensing: ../../../docs/license.md. Supervision: src/engine/AGENT-RULES.md.
