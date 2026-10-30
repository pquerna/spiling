<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Native workbench

- `host` exclusively owns EngineClient: two Tokio workers with 8 MiB stacks, eight bounded controls and coalesced snapshots. Admit effects in `App::logic`; redraws repeat. Request acknowledgements differ from background publication.
- Retain accepted Google operation names through observed terminal outcomes. Resume/Cancel explicitly; never retry uncertain mutations. Only matching Open/ProjectSaved receipts attach routes, including receipt-owned uncertain saves. Dirty discard rechecks identity/revision. Close/Drop await cleanup and child reap.
- NativePath preserves encoding; parented pickers/launch arguments supply paths. Preferences contain theme/license assent, never project authority. Diagnostics have separate checked artifacts; triangles never substitute for BREP.
- Forms preserve full intent/scopes/layers, capture revisions, retain rejected drafts and commit once on Apply. Core owns scope/inheritance/history. Load bounded inert profiles without mutation; engine resolution preserves capture and explicit override provenance. Material-bound recipe fields have one editor. Canonical image previews use checked stored assets, never require original files.
- `scene` atomically publishes checked pages/chunks; occurrences share immutable definitions. Active/staged source/section/replay share the frozen budget. Rebase f64 before f32; refuse excess error. Stale/paused displays cannot pick/edit. Geometry-only definitions retain face-list inspection.
- `manufacture` separates fresh inspection from stored verification and input-revision provenance. Bind revision/session/record/resource after retrieval and before exclusive export; edits/history invalidate motion. SPLR batches preserve event ordinals and immutable filter ranges; epochs never connect and pressure events are stationary. Nominal beads retain endpoints/physical intervals and explicit breaks; bounded projection/chord/work/CPU/GPU refusals preserve centerlines. Envelopes/tints never establish mass or physical shape.
- Menus, palette and buttons share typed Action enablement/dispatch. Text/IME retains editing shortcuts; canvas keys require its focus. Compact layout preserves the canvas and one toggled pane.
- `ui` shares production/gallery Graphite/Paper controls. Inspection stays off in distributions. [Development](../../docs/quality/development.md#native-egui-workbench) owns commands, actual-surface procedures and limits; [ADR006](../../docs/adr/006-native-egui-workbench.md) owns architecture.
