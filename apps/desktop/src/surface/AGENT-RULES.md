<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Surface finish workbench rules

- Own transient whole-face drafts only. Generated SurfaceFinish/ImageReliefParameters/SurfaceScope types and ProjectInfo.surface_overlays are sole domain authority; never retain a local layer/asset store or allocate IDs.
- Use existing serialized workbench lifecycle with current ProjectRevision plus live FaceRef. Image selection exposes only a generated SelectedSource opaque token/label/session; ImportSurfaceImageFinish is built by the dedicated native token bridge, never generic renderer NativePath. Read-only/stale/busy prevents mutation; cancelled picker preserves selection, accepted operation consumes it. Definition versus occurrence scope remains explicit.
- Native source-face tables and InspectFace permit explicit selection without GPU ray picking, including the startup-gated manufacturing diagnostic attachment. Diagnostics retain the no-GPU/picking/hardware label and do not introduce another backend.
- Apply appends, Remove removes, Enable/Disable toggles. Present authoritative ordered layers and sole image metadata table, including disabled layers. Enabled definition layers precede occurrence layers; last matching finish wins, including Smooth.
- Metadata-only receipts refresh manufacturing authority/freshness without remeshing or clearing native face/section leases. The image authoring order is Choose → Import image and apply → canonical captured preview → Compile. Import is undoable and atomic; preview failure must not mislabel a committed layer as rejected.
- Existing captured images can be selected for metadata-only AddSurfaceFinish remapping/reuse on another eligible face. Never require the original source file or re-normalize/re-upload canonical pixels to edit mapping intent.
- Show every image mapping field with units/source frame, pixel-node bilinear U/Z wrapping, top toward +source Z, source +X/+Z cylinder U, canonical planar tangent, protected planar margin and separate layer displacement-error budget. Compile owns hard carrier/process/Z/field/chord/retention/support admission.
- Dispatch non-Smooth finishes only from geometry.analytic: exact Plane for planar ribs, exact Cylinder for cylindrical ribs, either for image relief. Absent analytic descriptions and all other analytic kinds visibly refuse; geometry.family is descriptive, never finish eligibility. Native carrier certification remains required during Compile.
- GetSurfaceImage uses stored canonical content, current revision and standard operations/checked ByteStream. Strict protocol P5 decoder precedes Canvas; no browser source normalization or manufactured-shape/shader fallback. Fresh read-only preview must work without original PNG or session import token. Fresh independent actual replay, not image Canvas, shows manufactured paths.
- Exercise real CEF controls/native metadata/compile/replay/history/save/reopen flows. No wiring/default-only tests, fixture-specific production recognition or browser planners. Resolution diagnostics are sampled nominal-bead advisories, not independent image verification or physical qualification.
