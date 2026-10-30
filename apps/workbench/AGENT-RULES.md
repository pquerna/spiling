<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Native workbench

- `host` exclusively owns EngineClient on two Tokio workers with bounded 8 MiB stacks for typed debug frames. Eight bounded controls and coalesced checked snapshots separate asynchronous native work from drawing. Dispatch effects in `App::logic`; drawing may run repeatedly. Request acknowledgements are distinct from background publication.
- Accepted native work retains its Google operation name through an observed terminal outcome. Resume observation or explicitly Cancel; never retry uncertain mutations. Only the appropriate Open/ProjectSaved receipt attaches a route, including receipt-owned uncertain saves. Dirty replacement binds identity/revision and rechecks before discard. Close awaits staged cleanup and child reap; Drop also cleans up.
- Paths retain NativePath encoding and originate from parented pickers or explicit native launch arguments. Preferences contain theme/license assent, never project authority. Diagnostics use their own checked operation/artifact contract; the unitless triangle never substitutes for BREP.
- Forms capture revisions, retain rejected drafts and commit once on Apply. Preserve the full declared intent, source scopes, ordered layers and explicit construction permission. Core owns inheritance/history. Canonical image previews use checked engine assets; remapping never requires the original image.
- `scene` atomically publishes checked pages/chunks; shared occurrences reuse immutable definitions. Combined active/staged source/section/replay allocations share the frozen budget. Rebase f64 before f32; refuse excess projection error. Stale/paused displays cannot pick or edit. Geometry-only definitions retain native face-list inspection.
- `manufacture` captures fresh independent inspection separately from stored verification and immutable input-revision provenance. Bind observed revision, session, active record/resource and request capture after retrieval and before exclusive export commit. Edits/history invalidate motion. SPLR role/tool batches preserve event ordinals; scrubbing changes ranges, epochs never connect, pressure events are stationary markers. Dimensions and captured catalog tints are nominal declarations.
- Menus, command palette and manufacturing buttons share typed Action enablement/dispatch. Text/IME keeps editing shortcuts; canvas keys act only on its own focus. Compact layout retains a canvas and one toggled control pane.
- `ui` supplies production/gallery Graphite/Paper controls. Inspection stays off in distributions. [Development](../../docs/quality/development.md#native-egui-workbench) owns commands, actual-child/window procedures and platform limits; [ADR006](../../docs/adr/006-native-egui-workbench.md) owns migration boundaries.
