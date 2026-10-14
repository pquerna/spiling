<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Surface finish workbench rules

- Own transient whole-face finish drafts only. Generated SurfaceFinish/SurfaceScope types and ProjectInfo.surface_overlays are authority; never retain a local layer store or allocate IDs.
- Use the existing native.project typed bridge through the serialized workbench workflow. Submit current persistent project revision plus current native FaceRef; retain native typed rejection details. Read-only, stale scene and busy state prevent mutation. Definition scope covers every instance; occurrence scope covers only the selected instance.
- Native source-face tables and InspectFace permit explicit selection without GPU ray picking, including the startup-gated manufacturing diagnostic attachment. Diagnostics retain the no-GPU/picking/hardware label and do not introduce another backend.
- Apply appends, Remove removes, Enable/Disable toggles. Present the authoritative ordered document including disabled layers. Enabled definition layers precede occurrence layers and last matching fill wins; Smooth overrides ribs.
- Metadata receipts refresh project authority and trigger existing manufacturing authority/freshness handling without native scene remeshing. Undo/open use the same project authority. No shader texture or fake physical preview: only fresh independently replayed emitted motion displays manufactured changes.
- Show both full-height rectangular supporting planar-wall and complete exterior cylindrical-face envelopes under upright/bed-contact placement. Closed-cylinder phase is a source-local cycle fraction, never a kernel UV/seam; count is integral. Explicit source metadata selection is carrier independent; Compile owns native trim/process/clearance/work rejection.
- Exercise real CEF controls/native metadata/compile/replay/save/reopen flows. No wiring/default-only tests, fixture-specific production recognition or browser planners belong here.
