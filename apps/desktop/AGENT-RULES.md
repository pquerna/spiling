<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Desktop rules

- This directory owns React workbench state and frontend assets; `src-tauri/` separately owns native supervision. Only `src/native.ts` invokes the shell. Shell view types come from `@spiling/protocol`, generated from Rust adapters over authoritative Protobuf.
- Import the canonical root `LICENSE.md` into the shipped bundle. Explicit checkbox assent precedes mounting the workbench. Persist only the versioned local acknowledgement; never transmit it. Reviewing assent unmounts the session and stops its engine.
- A browser is not a native engine host: show desktop-required without mock invokes. Probe WebGPU and initialize a strictly WebGPU renderer before `engine_start` or `engine_restart`. Unsupported hardware is a visible failure; never substitute WebGL.
- Lifecycle/display commands are single-flight; cancellation is independently callable while work runs. Pull bounded operation snapshots and one artifact at a time; fetch again only after decoding/rendering consumes the previous bytes. Every session operation has a generation; stale invokes must not replace current state. Status polling is nonoverlapping and observes external crashes. Stop, teardown, or GPU loss releases the viewport; GPU loss also stops the engine.
- Report configured kernel direction separately from negotiated geometry capabilities. The diagnostic triangle is unitless synthetic display data, never authoritative manufacturing geometry.
- `pnpm --filter @spiling/desktop dev` serves port 1420; `build` typechecks and bundles; `check` typechecks. Root development launches the actual desktop shell and sidecar. Acceptance uses real CEF/WebGPU/native transfers, external PID termination, restart, and graceful stop, not browser fixture mocks.
- Original source retains OSL notices. JSON metadata has adjacent `.license` sidecars; unminified Vite output preserves source legal comments. Distribution must include root licensing/notices and corresponding source as specified by `../../docs/license.md`.
