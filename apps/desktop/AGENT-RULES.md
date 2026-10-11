<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Desktop rules

- Own transient React authoring/inspection forms and disposable display state only. Core is the sole project authority. `src/native.ts` alone invokes the CEF shell; generated Rust shell DTOs adapt typed Protobuf RPCs, not gRPC-Web or a JSON command tunnel.
- Ship canonical root LICENSE.md offline. Explicit checkbox assent precedes mounting; persist only local versioned acknowledgement. Reviewing assent unmounts the session and stops its engine.
- Browser-only use shows desktop-required. Initialize strictly WebGPU geometry rendering before starting/restarting the sidecar; unsupported devices fail visibly, never fall back to WebGL. The separate diagnostic triangle is explicitly unitless synthetic output, never imported geometry.
- Serialize short authoring invokes, not entire native operations or transfer workflows. Observe accepted native work by Google Operations names and checked NativeOperationView; cancellation is independently callable during ByteStream retrieval. Diagnostic OperationView remains a distinct codec/UI. Generation guards reject stale results; status polling is nonoverlapping and detects external crashes.
- Begin with a new unattached project. Token-selected imports are sequential; later failures preserve earlier admissions and name the failed source. Scene/project revisions, dirty/history/read-only/uncertain state are engine authority. Placement forms convert Euler XYZ degrees to canonical unit quaternions.
- Pull bounded metadata and one unique-definition chunk at a time. Bind immutable resource descriptors to packed hashes/sizes; the native client validates ByteStream bytes and the frontend independently validates packed layout/session/revision before GPU upload. Instances reuse definitions; occurrence mutations request no mesh bytes. Retain borrowed bytes for the viewport's lifetime.
- Revision changes clear face selection and sections. Failed/paused scene upload preserves the prior display explicitly stale/noninteractive; resume re-reads current engine state without reimport. Section-only cancellation discards staged lines and retains the interactive scene and prior labelled plane.
- Await viewport teardown on replacement, close or GPU loss; GPU loss stops the engine. Typed geometry/project/manufacturing errors and decoder admission failures retain a healthy session. Fatal process/transport failure tears it down. Handle every native result/error/status, including Interrupted; manufacturing results are explicitly unsupported desktop operations, never checkpoint receipts. No manufacturing UI/invokes/planner/replay.
- Dirty new/open/recover/stop/restart require explicit discard intent. First save selects a new directory; normal save uses attached storage. Save and intent/artifact-only undo/redo do not invalidate/retransfer geometry or clear history. Read-only preserves inspection/sections but disables mutation/save/history actions.
- After project operation failure query ProjectInfo. Postcommit failure can attach published storage while reporting save_uncertain/dirty; retain the confirmed saved revision, warn explicitly and never blindly retry. Restart explicitly reopens the shell-private saved route, never unsaved edits or prior-session history.
- Fixture controls require native startup canonical-root plus CEF-debug opt-in; renderer paths are relative/confined and mint production tokens. No unrestricted filesystem access.
- `dev` serves port 1420; `check` typechecks; `build` typechecks/bundles. `tools/smoke-desktop.mjs` covers native diagnostic durability, project bridge and geometry UI separately. Real CEF/WebGPU display/picking/dialog/cancellation/lifecycle evidence is required; software-GPU bridge smoke is not hardware or packaging certification.
- Preserve OSL source notices and JSON license sidecars; unminified Vite legal comments and distributions obey ../../docs/license.md.
