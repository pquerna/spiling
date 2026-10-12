<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Manufacturing workbench rules

- Own transient scalar drafts, native operation workflows and operator review only. Generated domain types are authoritative; native.ts alone invokes Tauri. No renderer filesystem, planner, emitter, program parser, scheduler or machine submission.
- The bundled software-only reference requires explicit operator selection. Apply submits one complete changed intent, never per-keystroke commands; unchanged drafts submit nothing. Failed Apply retains the draft. Authority changes on reopen/session/project replacement reset drafts; concurrent authoritative intent changes require explicit reload. Core owns Undo/Redo/Save/Reopen.
- Read-only permits fresh Verify and Export, but not Apply/Compile. Stored verified flags are never fresh evidence. Report-only requests no motion; report-and-motion uses the independently checked SPLR decoder and viewport callbacks.
- Freshness binds session, project/open epoch, current server-selected artifact hash/input hash and fresh request generation. Capture revision is provenance, not content freshness. Recheck after transport/decode/GPU staging. Superseded replay is cleared; historical reports are labelled inactive and release packed display bytes. Unchanged checkpoint bookkeeping does not invalidate matching content.
- Controls select bounded zero-based layers and event prefixes and toggle travel without replaying/regenerating geometry. First unknown-start travel is an endpoint only. Actual coordinate/feed/extrusion inspection comes from validated program replay, not plan echo.
- Native dialog-only single-use tokens authorize immutable four-file export. Confined fixture admission and explicit no-GPU attachment require native startup capabilities. Diagnostic transport is labelled no GPU/picking/hardware acceptance; normal product WebGPU admission remains mandatory.
- Display independent coverage, provenance, exclusions, units and software-only limits; no Print action. Exercise consumer state invariants with `node --test apps/desktop/test/manufacturing-state.test.mjs` and the real CEF panel with `tools/smoke-desktop.mjs --scenario manufacturing`.
