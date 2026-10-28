<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# ADR005: native-first DEVELOPMENT geometry

Status: **adopted; native-first material implementation, producer-specific qualification**.
This decision supersedes ADR004's custom original-source material reconstruction
requirements. It does not broaden supported STEP, finishes, processes or machines,
or establish complete operator qualification merely from implementation.

The implemented material route is source6/planner25 native-first DEVELOPMENT.
Landing requires one coherent native owner, matching published dependency/producer
identities and preserved qualified operators and source-safety controls. Do not
relabel older source5/planner24 reports as current evidence. A stronger standalone
native API is a prerequisite only when the actual consumer needs its contract.

## Decision

The imported SolidCore BREP is the single material geometry authority for
DEVELOPMENT builds. Import, bounds, placement, sections, material classification,
slab/occupancy queries and manufacturing consume that same immutable native
material through the geometry facade. SolidCraft decodes STEP operands; it is
not a second material kernel. Pin SolidCore and solidcore-mesh to one exact
published revision, with matching lockfile sources and `geometry::KERNEL_REVISION`.
Bind material results to that producer, source identity, units, tolerance policy,
occurrence pose, query scope and approximation bounds.

Original STEP bytes, source hash, canonical face/use identity, declared units and
uncertainty remain immutable identity and provenance. Preserve the original finish
input operands, selected-master/declared associations, source-local frames and
face targets required by the existing narrow planar/full-cylinder finish lanes.
These inputs do not constitute a separately reconstructed whole material or an
independent original-STEP topology/embedding certificate.

Query-private mutation, native caches and derived trims must not invisibly alter
the authoritative BREP or choose a different material for different consumers.
Derived charts/curves are query aids tied to the unchanged native geometry, not
original STEP declarations. A geometry-changing operation needs an explicit
owned result and new identity/admission; it cannot be published as an unchanged
query result. Do not hide repair, seam welding, topology replacement or tolerance
relaxation behind a query API.

Unsupported, indeterminate, ill-conditioned and invalid operations refuse
explicitly. Keep cumulative work, live-byte admission and cancellation through
native calls and derived output. A native method's name, successful import or
finite sample is not evidence of its required behavior. There is no mesh slicing
fallback, alternate material path, or inference of physical support from native
occupancy. Actual prior deposited strokes remain software anchor evidence.

## Why this boundary

`OriginalSpatialGTransport` and the associated F/G, seam/corner and material-link
construction attempted to independently reassemble a toleranced original-source
solid alongside the kernel's imported BREP. That duplicates topology, embedding,
trim and geometric-evaluation work, requires two material interpretations to stay
consistent, and makes delivery of ordinary mechanical-part workflows depend on
an open-ended second kernel/proof effort.

The chosen alternative accepts explicit reliance on SolidCore material semantics.
It retains bounded operation admission, source traceability, native-to-output
error accounting and independent emitted-program/process verification. It does
**not** claim the same independent original-source proof under a new name. Keeping
the custom reconstruction as a fallback would preserve the duplicate authority;
a mesh-derived material would change the product contract. Neither is adopted.

## Claims, identity and qualification

Use the existing `ManufacturingProvenance.kernel` (`KernelIdentity`) and planner
identity, plus `VerificationReport.coverage` and `limitations`; no parallel
provenance schema is introduced. Independent decoding still checks actual
commands, dimensions, extrusion, lifecycle, declared process limits and the
supported nominal ownership/contact obligations. Geometry-dependent checks rely
on the captured native material and plan. Fresh Verify does not independently
reconstruct or certify original STEP topology or embedding.

Report coverage states reliance on captured native-kernel BREP material and
independent decoding of actual program/process semantics. Report limitations
explicitly say: **DEVELOPMENT native-first geometry relies on the pinned
kernel/tolerance policy; no independent original-STEP topology or material-embedding
certification**.

The native-material semantic cutover is source6/planner25. Historical
source5/planner23–24 exercised results remain valid for their recorded producer and
contract, not qualification for the new authority. Storage/binary generations do
not change merely because this decision is adopted; executable constants own
freshness and old artifacts must not acquire new authority by relabeling.

Native-first activation and full W1 bracket qualification are separate gates.
Preserve the fitting, ordinary housing, centered physical plate, genuine H1
fine/combined and planar/full-cylinder finish operators, their relevant workbench
flows, fresh verification/export/save and source-deleted recovery. Source identity,
units, protected material/voids, closed-height scope, resource limits and
cancellation remain required. No easier fixture, intent or all-fine substitute
replaces these preserved positives.

Additional refused journeys remain outside this activation by explicit decision:
the pre-existing planar-image wall-dose case, fixed-Y housing Compile, and the
8×8/depth0.35 cylindrical-image workbench Compile. Published source5/planner24
also refuses those inputs; only the planar dose rejection establishes the same
failure, not the earlier baseline refusals of the other journeys. Preserve the
failing paths, diagnostics and unchanged geometry/support/dose gates. The accepted
alternating-XY housing/plate operator, passing cylindrical-image CLI journey and
actual PNG authoring/preview/history remain separate evidence.

The original ribbed bracket remains explicitly unqualified for manufacturing.
Its default-wall Compile → fresh Verify → export, useful variable width/count
regions and junctions, width/history and source-deleted recovery remain the full
W1 acceptance, not prerequisites for this native-first development activation.
Do not add the missing material-sector incidence lemma to this cut or claim W1
complete. Preserve honest refusals and the bracket's separate display/picking
support.

Record exact producer/input identity, resource observations, exercised stages
and remaining refusals in commit/PR notes or ignored evidence. These gates are
requirements, **not performed results of this ADR**.

Noncolumn torus critical-band slab analysis is explicitly outside this activation
bar. Its current native height-event refusal remains unqualified, not a passed
positive or a weaker successful certificate. Separate torus import/display/section
controls do not qualify that material query. Preserve demand deferral for already
qualified slabs, protected noncolumn refusal and closed-height pocket/floor
obligations without requiring this additional torus capability.

Programs remain **SOFTWARE VALIDATION ONLY, NOT MACHINE READY**. This decision
provides no physical printability, adhesion/support, collision, calibration,
firmware or machine-readiness certification. See [geometry boundaries](../quality/geometry-evidence.md),
[manufacturing boundaries](../quality/manufacturing-evidence.md) and the
[verification contract](../protocol/manufacturing.md#emitted-program-and-independent-replay).
