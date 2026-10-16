<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Native manufacturing bridge rules

- Translate generated ManufacturingCommand into shared EngineClient RPCs; observe the existing durable Google Operation. Capture only the pending current-session fresh inspection result. No planner/parser/verifier, separate scheduler or project storage.
- Native main-thread intent dialog admits regular inert ManufacturingIntent JSON only, reading at most 32 KiB plus one sentinel byte before strict Rust deserialization/domain validation. Hold one checked full-intent selection behind a session/UUID/label token; no native path leaves the shell. Failed/cancelled selection preserves prior token/draft; successful loading consumes once; lifecycle/reopen clears tokens. Never execute TypeScript or synthesize profiles.
- Bed preparation snapshots native scene pages and stored full intent, validates session/revision/read-only state and returns GeometryResponse::OperationAccepted through the existing typed RPC. The renderer observes/cancels that operation independently. Its terminal BedPosePreparation is a read-only original-source contact proposal preserving rigid orientation; explicit Apply alone submits SetInstancePose with captured revisions. No shell placement algebra, mesh bounds or blocking terminal watch.
- Preflight translates the typed command through the existing operation service and never installs verification/export authority. Native layer review fetches the checked captured bundle, rechecks session/content after retrieval and returns only bounded schedule metadata: actual height, section Z and deposition segment count, including genuine empty contact-gap layers.
- Return replay bytes only through EngineRpc::fetch_replay_display bound to the captured record/descriptor, with active server-selected record/resource and session checked before and after retrieval. Native lifecycle changes and accepted reopen clear capture/tokens.
- The main-thread native destination picker mints one opaque session/UUID token for a new directory. Renderer paths are never accepted by production export. Consume on admitted export; no overwrite or retry. Use only shared engine-client::export::OutputDirectory, with the captured fresh report, to produce plan.json, program.gcode, provenance.json and verification.json.
- Do not hold lifecycle ownership across transfer or staged file writes. Recheck captured request, current session and server-selected record under the short lifecycle lock before commit; failed staging uses common handle-relative cleanup. Return success only after commit.
- Debug destination admission is confined to the canonical startup fixture root. Explicit no-GPU attach additionally requires SPILING_MANUFACTURING_DIAGNOSTIC=1 plus existing valid CEF debug/root capability; it establishes no display/picking/hardware support. Runtime exposes only the boolean gate.
- Register all commands in main.rs/build.rs and the local-only capability. Changes require targeted native interface checks and the actual CEF panel smoke; preserve warm resource caches and run native workloads sequentially.
