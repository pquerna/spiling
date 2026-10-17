<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Geometry authority and code map

Geometry has separate admission, inspection, display and material-proof boundaries.
A successful import, pick or section is **not manufacturing acceptance**. The
[parity RFC](../adr/004-native-geometry-parity.md) defines the intended G0–G3
contract; those gates are not completely qualified. See
[manufacturing](manufacturing-evidence.md) for the compiler/replay boundary and
[development](development.md) for build and desktop prerequisites. No geometry
result establishes physical printability or machine readiness.

## Entry points and immutable source identity

- Start at [`crates/geometry/src/lib.rs`](../../crates/geometry/src/lib.rs):
  `import_step`, `ImportLimits`, `Definition`, prepared-source APIs and query
  results are the public facade. Native representation stays inside the crate;
  `KERNEL_VERSION` / `KERNEL_REVISION` identify the actual producer.
- [`import/mod.rs`](../../crates/geometry/src/import/mod.rs) owns STEP admission,
  lossless source/native face-use correspondence and diagnostic conversion.
  [`ast.rs`](../../crates/geometry/src/import/ast.rs) bounds reachable entities;
  [`units.rs`](../../crates/geometry/src/import/units.rs) owns physical unit
  conversion. Unsupported conversion is not evidence of malformed source.
- [`kernel/mod.rs`](../../crates/geometry/src/kernel/mod.rs) owns immutable
  `Definition`, `FaceBoundaryRefs`, `BoundaryCoedgeRef` and source parameter
  witnesses. Original face/use identity is not regenerated from display
  triangles or section-loop ordinals. Occurrences add poses, not new definitions.
- [`source_trim.rs`](../../crates/geometry/src/import/source_trim.rs) retains
  literal source PCURVEs and original continuations. `OriginalCoedgeCorrespondence`
  in [`trim_certificate.rs`](../../crates/geometry/src/import/trim_certificate.rs)
  proves whole-range physical position and relative tangent correspondence under
  the captured parameter map. Samples, inverse guesses and chart guides do not
  supply this authority; correspondence alone does not prove trim topology,
  inverse coverage or native incidence.

Generic `FaceInfo.geometry` may carry optional exact analytic metadata. That
metadata describes a face; it does not authorize a finish, slab reuse or a
family-wide operation. Current import/display profiles and wire ownership are
specified by the [RFC](../adr/004-native-geometry-parity.md) and
[display protocol](../protocol/display.md).

## Prepared owners and placed inspection

| Owner / operation                          | Invariant and navigation                                                                                                                                                                                                            |
| ------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `PreparedGeometrySource`                   | [`analysis/material_source.rs`](../../crates/geometry/src/analysis/material_source.rs): original trim domains and canonical source-vertex plans, without native Boolean seeds. Supports placed material bounds and height analysis. |
| `PreparedGeometrySource::prepare_sections` | Consumes the geometry owner, moves its domains/plans and adds boundary inventory plus native seeds. Its allowance is the total replacement-source allowance, not extra capacity.                                                    |
| `PreparedMaterialSource`                   | Same module: job-local shared immutable preparation across occurrences/retries; does not cache posed slab, metric or incidence authority.                                                                                           |
| `PreparedSectionSource`                    | [`section/mod.rs`](../../crates/geometry/src/section/mod.rs): bounded native Boolean preparation for raw sections. Raw inspection does not require material admission.                                                              |
| Placed queries                             | [`analysis/placed.rs`](../../crates/geometry/src/analysis/placed.rs): bounds, height and section transforms use the actual accepted quaternion polynomial, not an assumed exactly orthogonal transpose.                             |

`NativeSection` in the facade carries plane/frame, original-face lineage and
achieved chord/plane errors. A raw native intersection is not closure of
source material. Conservative slab envelopes are explicitly distinguishable
from intersections and cannot be promoted into manufactured layers.

[`analysis::section_regularity`](../../crates/geometry/src/analysis/mod.rs) uses
the import-certified native bounds only to exclude empty cuts; literal critical
census and complete crossing certification remain mandatory. Native carrier
callbacks hold a non-reentrant mutex: comparison queries that acquire the same
carrier must finish before entering `Face::with_surface`.

[`tessellation/mod.rs`](../../crates/geometry/src/tessellation/mod.rs), its
`coverage` / `boundary` modules and the display protocol own whole-triangle
coverage and packed-mesh checks. Display is not a slicing representation.
[`apps/engine/src/geometry/worker.rs`](../../apps/engine/src/geometry/worker.rs)
keeps native admission independent of display failure: `DefinitionDisplay::Unavailable`
means no mesh artifact or GPU face picking, not an empty successful mesh.
Native source records and supported queries remain available; cancellation
must never be converted into display unavailability.

## Source material and proof boundaries

- **Source-only G:** `OriginalMaterialRealization` / `MaterialRealizationIdentity`
  in `kernel/mod.rs` bind the implemented source-relative transport recipe and
  exact source-allowance bits. Query radial witnesses cannot change that recipe.
  [`critical/material_link.rs`](../../crates/geometry/src/analysis/critical/material_link.rs)
  and its transport/vertex owners prove original-use material embedding; a
  retained chart or enclosure-only recipe cannot supply embedding authority.
- **Uncertainty:** [`analysis/bounds.rs`](../../crates/geometry/src/analysis/bounds.rs)
  and `analysis/placed.rs` distinguish geometric from realized-material bounds.
  Source realization error must survive actual affine conversion, placement
  arithmetic and plane drift. A complete bound is not made cheaper by omitting
  uncertainty or charging it in a different coordinate system.
- **Regular slabs:** [`analysis/slab.rs`](../../crates/geometry/src/analysis/slab.rs)
  binds the actual source, interval, reference and policy. Height invariance is
  local to regular partitions; it does not cross critical-event brackets.
  [`placed/material_coordinates.rs`](../../crates/geometry/src/analysis/placed/material_coordinates.rs)
  owns actual-coordinate capture, not a nominal orthogonal-cylinder surrogate.
- **Radial products:** [`critical/material_link/radial_product.rs`](../../crates/geometry/src/analysis/critical/material_link/radial_product.rs)
  and [`joint/radial_metric.rs`](../../crates/geometry/src/analysis/joint/radial_metric.rs)
  pair closed topology with a complete filled-material metric and calibrate the
  actual used polygon in matching physical coordinates. Boundary nearness,
  directed/core-only metrics and separately isotopic annuli are insufficient.
- **Critical sets and collars:** [`analysis/critical.rs`](../../crates/geometry/src/analysis/critical.rs)
  covers original face/coedge complexes; [`source_incidence.rs`](../../crates/geometry/src/analysis/source_incidence.rs)
  owns actual edge/vertex use incidence. [`analysis/collar.rs`](../../crates/geometry/src/analysis/collar.rs)
  binds nonzero collars and closed endpoints to actual neighboring material.
  A regular radial product does not discharge births/deaths, pocket floors,
  disappearing voids or cross-occurrence attachment; loop counts do not prove incidence.
- **Final composition:** [`analysis/joint.rs`](../../crates/geometry/src/analysis/joint.rs)
  owns source leaves, union captures and `NativeUnionBindingLedger`. Receipts
  bind source/realization, occurrence, pose, interval/reference, policy and actual
  candidate geometry. Changed polygons or stale captures cannot inherit proof.
  Native material reuse does not establish realized finish or deposited support;
  those remain independent [manufacturing obligations](../protocol/manufacturing.md).

## Refusals and support boundary

`GeometryWorkBudget` in [`analysis/mod.rs`](../../crates/geometry/src/analysis/mod.rs)
is cumulative across retries/subdivision. Prepared sources, pinned proofs,
coexisting query scratch and output have separate retained-capacity accounting;
a packed-output allowance cannot fund native preparation. These are not RSS
limits: native intermediate objects also require host-process memory bounds.
Resource exhaustion, cancellation, unavailable parameter correspondence,
conditioning, unsupported operations and invalid geometry remain distinct failures.
No partial proof, stale job or failed display grants successful material authority.

Broad raw import/mesh/section inspection includes the tapered fitting and rounded
housing, but their manufacturing remains unqualified: fitting exhausts the
cumulative geometry-work allowance; housing has a closed-source vertex radial-sign
refusal on original face 27. Inward-ramp collar conditioning is also unresolved.
Additional mixed-source, tilted-placement, critical-collar and support constraints
remain; a working box or cylinder workflow does not generalize to those cases.
Singular/nonmanifold geometry, unsupported STEP conversions and unproved charts
retain explicit refusals. General AP242 document/assembly expansion and arbitrary
freeform finishes are not implied by generic face descriptors.

## Minimal user flows

After the canonical build, run from the repository root:

```sh
target/debug/spiling-cli geometry \
  --scene fixtures/geometry/scenes/realistic-native.scene.json \
  --section 0,0,6:0,0,1 --engine "$PWD/target/debug/spiling-engine"
node tools/smoke-desktop.mjs --executable "$PWD/target/debug/spiling-desktop" \
  --scenario geometry --scene fixtures/geometry/scenes/realistic-native.scene.json \
  --port 9222
```

The existing scene supplies fitting, housing and a repeated fitting occurrence.
CLI inspection exercises native transfer/query paths; the desktop flow exercises
visible geometry and picking with the hardware setup in development. Neither
command is a manufacturing or packaged-platform qualification gate.
