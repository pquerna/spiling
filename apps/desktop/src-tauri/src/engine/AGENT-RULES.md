<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Desktop engine supervision rules

- EngineClient exclusively owns native child/startup/auth/liveness/shutdown; EngineRpc is cloneable typed gRPC access. Hold the supervisor mutex only for lifecycle, token admission and short authoring exchanges, never operation observation/download or dialogs. No framing/version override/second scheduler.
- Check closing gate before/after lifecycle ownership. Refresh actual OS status; stopped/running/interrupted remain distinct. Fatal client failures terminate/reap and clear tokens/pending routes; domain/RPC errors preserve a healthy child. Stop/restart/close await previous cleanup.
- Spawn with the application-local engine-operations durable ledger, preserving main diagnostic dedupe/artifacts/restart semantics. No authentication capability enters storage. Keep diagnostic OperationView and native NativeOperationView distinct; operation names, not internal job IDs, own observation/cancellation.
- Token paths are private NativePath with native byte/code-unit fidelity; expose only generated UUID/session/label/intent DTOs. At most 32 sources and one unconsumed project token per intent. Cancellation preserves tokens; accepted operations consume. Clear tokens on lifecycle changes. Generic geometry refuses import; generic project refuses all open/save.
- Native geometry_chunk takes ArtifactView plus ArtifactChunkMetadata/ArtifactSummary and uses shared-client hash/layout/revision validation with native limits before moving raw bytes into IPC. Diagnostic engine_read_artifact retains its separate bounded path. No JSON/base64 or extra whole-artifact copy.
- Fixture admission requires startup canonical root and CEF debug opt-in. Existing source/open paths must be canonical regular-file/directory descendants; new save targets must not exist and have a confined canonical existing parent. Reject absolute/parent/symlink escapes. Runtime exposes only enabled status.
- Remember only attached saved routes. Open/save completion must match the pending operation name and current session; attach only its Scene/ProjectSaved receipt. Failed/cancelled/interrupted save can attach only its own save_uncertain receipt. Sticky earlier ProjectInfo uncertainty never settles a later failure. Section/manufacturing results never attach routes. No manufacturing invokes.
- New clears the attached route; failed/cancelled open preserves it. Restart checks dirty-discard intent, reaps old child and starts fresh; project_reopen explicitly restores saved storage, never unsaved edits or session history. Read-only may switch projects but cannot mutate/save/history.
- Close queries dirty state before main-thread discard confirmation, never treating unknown healthy status as permission to lose edits. Dialog cancellation restores admission. Installed-relative discovery or absolute debug override only; no PATH/cwd search.
- Exercise actual diagnostic and native lifecycle/token/operation/ByteStream paths, concurrent cancellation and close cleanup. Pure source/path-gate tests cover limits/native fidelity/containment, not actual dialogs/GPU/packaging. Verification belongs to integration owner and must respect native memory limits.
