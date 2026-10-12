<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Native manufacturing bridge rules

- Translate generated ManufacturingCommand into shared EngineClient RPCs; observe the existing durable Google Operation. Capture only the pending current-session fresh inspection result. No planner/parser/verifier, separate scheduler or project storage.
- Return replay bytes only through EngineRpc::fetch_replay_display bound to the captured record/descriptor, with active server-selected record/resource and session checked before and after retrieval. Native lifecycle changes and accepted reopen clear capture/tokens.
- The main-thread native destination picker mints one opaque session/UUID token for a new directory. Renderer paths are never accepted by production export. Consume on admitted export; no overwrite or retry. Use only shared engine-client::export::OutputDirectory, with the captured fresh report, to produce plan.json, program.gcode, provenance.json and verification.json.
- Do not hold lifecycle ownership across transfer or staged file writes. Recheck captured request, current session and server-selected record under the short lifecycle lock before commit; failed staging uses common handle-relative cleanup. Return success only after commit.
- Debug destination admission is confined to the canonical startup fixture root. Explicit no-GPU attach additionally requires SPILING_MANUFACTURING_DIAGNOSTIC=1 plus existing valid CEF debug/root capability; it establishes no display/picking/hardware support. Runtime exposes only the boolean gate.
- Register all commands in main.rs/build.rs and the local-only capability. Changes require targeted native interface checks and the actual CEF panel smoke; preserve warm resource caches and run native workloads sequentially.
