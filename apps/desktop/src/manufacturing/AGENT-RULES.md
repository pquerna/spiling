<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Manufacturing workbench rules

- Own transient scalar drafts, native operation workflows and operator review only. Generated schema-2 domain types are authoritative; native.ts alone invokes Tauri. Expose explicit physical-thickness top/bottom skins and finite interior infill fraction (0,1]. Preserve complete data-loaded job declarations and provenance through scalar edits, never synthesize machine defaults or reinterpret hashes as physical qualification. No renderer filesystem, planner, emitter, program parser, scheduler or machine submission.
- The bundled software-only reference requires explicit operator selection. Apply submits one complete changed intent, never per-keystroke commands; unchanged drafts submit nothing. Failed Apply retains the draft. Authority changes on reopen/session/project replacement reset drafts; concurrent authoritative intent changes require explicit reload. Core owns Undo/Redo/Save/Reopen.
- Read-only permits fresh Verify and Export, but not Apply/Compile. Stored verified flags are never fresh evidence. Report-only requests no motion; report-and-motion uses the independently checked SPLR decoder and viewport callbacks.
- Freshness binds session, project/open epoch, current server-selected artifact hash/input hash and fresh request generation. Capture revision is provenance, not content freshness. Recheck after transport/decode/GPU staging. Superseded replay is cleared; historical reports are labelled inactive and release packed display bytes. Unchanged checkpoint bookkeeping does not invalidate matching content.
- Controls select bounded zero-based object layers and event prefixes and toggle object/lifecycle travel without replaying/regenerating geometry. SPLR2 lifecycle travel/prime are separately counted and attributed to no object layer; display only in all-layer view. First unknown-start travel is an endpoint only; homing trajectory is excluded. Actual coordinate/feed/extrusion inspection comes from validated program replay, not plan echo. Priming filament remains separate from object totals.
- Native dialog-only single-use tokens authorize immutable four-file export. Confined fixture admission and explicit no-GPU attachment require native startup capabilities. Diagnostic transport is labelled no GPU/picking/hardware acceptance; normal product WebGPU admission remains mandatory.
- Display independent coverage, provenance, exclusions, units and software-only limits; no Print action. Exercise consumer state invariants with `node --test apps/desktop/test/manufacturing-state.test.mjs` and the real CEF panel with `tools/smoke-desktop.mjs --scenario manufacturing`.
