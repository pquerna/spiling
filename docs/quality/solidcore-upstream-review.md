<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# SolidCore fork: upstream issue assessment

Maintain one long-lived `pquerna/*` branch per logical change in
`pquerna/solidcore`, compose them on `pquerna/integration-spiling-tracking`, and
publish the tested integration directly to fork `main`, without requiring
GitHub issues or PRs. The long-term goal is to retire every
local overlay through upstream adoption or elimination of its consumer need.
An upstreaming recommendation is not authorization to open an upstream PR or
issue. Current Spiling manufacturing refusals need a standalone kernel contract
violation before they become upstream bug candidates.

Upstream development lives in
[`tangency-labs/solidcraft/solidcore/`](https://github.com/tangency-labs/solidcraft/tree/main/solidcore).
`tangency-labs/solidcore` is its automatic read-only mirror, retained as our
standalone synchronization source; mirror commits record their SolidCraft source
in `solidcraft:` trailers. Any explicitly approved upstream issue or PR belongs
in SolidCraft, with kernel-only PRs against `kernel`. Historical mirror issue/PR
links below are context, not submission destinations.

## Baselines and refresh state

| Role                                      | Exact identity                                                                                                                                                                                                                                                          |
| ----------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Upstream baseline                         | [`bca7a3fb8e937481a2b23644aa931445e46cd822`](https://github.com/tangency-labs/solidcore/commit/bca7a3fb8e937481a2b23644aa931445e46cd822), mirroring [SolidCraft `786b1d3`](https://github.com/tangency-labs/solidcraft/commit/786b1d3e50f731393ef0df43c0b83adc4c20bd90) |
| Published fork `main` and integration tip | [`f941fa80a8370d6895168c6e2d265781a9ae60c1`](https://github.com/pquerna/solidcore/commit/f941fa80a8370d6895168c6e2d265781a9ae60c1), branch [`pquerna/integration-spiling-tracking`](https://github.com/pquerna/solidcore/tree/pquerna/integration-spiling-tracking)     |
| Qualified Spiling consumer pin            | [`f941fa80a8370d6895168c6e2d265781a9ae60c1`](https://github.com/pquerna/solidcore/commit/f941fa80a8370d6895168c6e2d265781a9ae60c1); both declarations in [`Cargo.toml`](../../Cargo.toml), their lockfile sources and `geometry::KERNEL_REVISION` agree                 |

Refreshing the fork does not automatically promote its moving tip into Spiling.
Promotion requires native source/query controls and real engine/CLI consumer
workflows, not merely a successful build. Native import/display/sections, ordinary
and source-zoned manufacturing, fine/genuine combined H1 controls, independent
verification and source-deleted recovery retain separate acceptance obligations.
The [geometry](geometry-evidence.md) and [manufacturing](manufacturing-evidence.md)
runbooks own the current support envelope. Passing selected upgrade controls does
not clear remaining development gates or qualify every native geometry family.

Keep surviving upstream `Kernel::fixes` at their defaults; new optional fixes
require their own consumer qualification. Retired flags and off paths are not
restored: their unconditional replacements require consumer qualification with
the new pin. Spiling vendors no SolidCraft source: STEP reading is the pinned
`solidcore-step`.

Standalone native tight-box and direct imported-STEP point-classification APIs
are not qualified by a dependency refresh or successful adapter workflows.
Upstream query fixes do not authorize replacing the adapter's admitted paths
with a vertex hull, ignored native error or failed tight-box fallback. A
standalone valid-domain kernel reproducer is required before attributing an
upstream defect.

The integration contains inherited tessellator work plus the separate Spiling
budget and STEP-copy overlays. Preserve the tessellator's original authors and
Apache-2.0 notices; it is not Spiling-authored. Old published candidates and
consumer pins remain reachable, but are not the maintained queue tip.

Fork `main` publishes the tested composed integration. Stable patch lanes and
their upstreaming criteria are below; the [maintenance procedure](development.md#solidcore-fork-freshness)
owns synchronization, exact-pin promotion and published-history safety.

## Fork patch lanes

| Long-lived branch | Why the current consumer needs it | Upstream/removal criterion |
| --- | --- | --- |
| [`pquerna/kernel-trims`](https://github.com/pquerna/solidcore/tree/pquerna/kernel-trims) | Spiling's import reads STEP through upstream `solidcore-step` `read_solid_with` with `solidcore-ops::trims::KernelTrims`: the Rust kernel makes the pcurves of uses with no closed form (one-face sheet through `to_kernel_with_bare_uses`/`to_record`), as SolidCraft's `SmlibTrims` does through SMLib; `eval::pcurve::on_interval` puts each on the edge's interval. Also builds a face closed on itself (a whole torus between seams) as a closed sheet. | Upstream a kernel-backed `Trims` (or equivalent) so a reader needs no application crate; retire when upstream provides one. |
| [`pquerna/record-dead-knots`](https://github.com/pquerna/solidcore/tree/pquerna/record-dead-knots) | A kernel spline's record (`record_trim`, curve writer) leaves out control points on spans of no length, so `to_record` writes valid records for trims with a knot repeated past degree + 1 (corpus part 12). | Upstream the writer change; retire when upstream has it. |
| [`pquerna/step-split-bands`](https://github.com/pquerna/solidcore/tree/pquerna/step-split-bands) | A face split along an edge off it (each half winding once round an open surface, an Inventor thread root band) gets the cut laid inside the two faces, noted; adds `eval::pcurve::interpolate` (corpus part 26). Builds on kernel-trims. | Upstream the reader rule; retire when upstream reads such pairs. |
| [`pquerna/step-closed-spline-seams`](https://github.com/pquerna/solidcore/tree/pquerna/step-closed-spline-seams) | A closed spline's seam uses lie on the sides their loop meets (NURBS cylinders/tori whose end rows meet with unequal weights). Builds on kernel-trims. | Upstream the reader rule. |
| [`pquerna/step-forms-on-frame-quarter`](https://github.com/pquerna/solidcore/tree/pquerna/step-forms-on-frame-quarter) | Round faces' label frames are turned by whole quarters exactly, so the kernel's `AnalyticSupport::build` keeps their analytic support instead of a bare NURBS. | Upstream the exact quarter turn. |
| [`pquerna/step-form-range-on-pcurves`](https://github.com/pquerna/solidcore/tree/pquerna/step-form-range-on-pcurves) | A cylinder's or cone's form along its axis ends exactly on its pcurves' heights. Builds on step-forms-on-frame-quarter. | Upstream the reader rule. |
| [`pquerna/convert-conic-form-piece`](https://github.com/pquerna/solidcore/tree/pquerna/convert-conic-form-piece) | `to_kernel` builds a closed conic on its form's piece over the edge interval (thread seam edges past a turn). | Upstream the conversion fix. |
| [`pquerna/tess-work-control`](https://github.com/pquerna/solidcore/tree/pquerna/tess-work-control) | `solidcore-tess` `plan_with`/`evaluate_with` under a caller's `Control`: counted work, cooperative cancellation and byte admission before retained growth, typed terminal refusal; `TessPlan.edge_params`. Spiling's display candidates and tolerant sections plan through it. | Agree on an upstream tess control contract; retire when upstream has one. |
| [`pquerna/tess-cut-grid-thin-band`](https://github.com/pquerna/solidcore/tree/pquerna/tess-cut-grid-thin-band) | A trace along the boundary moves no grid line onto itself (thin bands). | Upstream the mesher fix. |
| [`pquerna/tess-refine-boundary-gap`](https://github.com/pquerna/solidcore/tree/pquerna/tess-refine-boundary-gap) | Refinement also covers the surface's departure beyond the corners' gaps. | Upstream the mesher fix. |
| [`pquerna/tess-chord-unshaped`](https://github.com/pquerna/solidcore/tree/pquerna/tess-chord-unshaped) | A chord-only caller (Spiling's tolerant model asks for no facet angle) gets no angle shaping of the Delaunay fill: shaping round a trim corner far sharper than its angle crowded points to coincidence, leaving welded triangles with no area (corpus part 24, a plane-cut cone). The sag refinement still holds every triangle to the tolerance. | Upstream the mesher fix. |
| [`pquerna/brep-tessellator`](https://github.com/pquerna/solidcore/tree/pquerna/brep-tessellator) | No Spiling consumer since the import and meshing moved onto `solidcore-step`/`solidcore-tess`; kept in integration only as history. | Drop from the next integration refresh. |
| [`pquerna/mesh-query-budget`](https://github.com/pquerna/solidcore/tree/pquerna/mesh-query-budget) | No Spiling consumer (see `tess-work-control`). | Drop from the next integration refresh. |
| [`pquerna/step-surface-owned-copy`](https://github.com/pquerna/solidcore/tree/pquerna/step-surface-owned-copy) | No Spiling consumer: the import no longer places STEP wrappers in BREP input. | Drop from the next integration refresh. |

Every lane merges upstream `origin/main` (bca7a3fb8) or the lane named as its base. Merge
them into integration, exercise the combined tree, and normally fast-forward it directly
to fork `main`. No issue, PR or GitHub API authentication is required. Keep the requirement,
consumer call path, dependency/base, known limits, proposed upstream contract and
lane-retirement condition in this registry; publication commit notes and ignored evidence
retain run-specific verification.

## Submission decisions

| Candidate                                                                           | Evidence and ownership                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                | Decision                                                                                                                                                                                                                                            |
| ----------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Independent copies of `StepSurface` wrappers                                        | Upstream [`Surface::copy`](https://github.com/tangency-labs/solidcore/blob/2d37267fea023715d4e15e3baf59bef7b4e399f9/crates/solidcore/src/surface/surface/p2.rs#L1317-L1330) and `copy_and_add_analytics` intentionally route `Step` to the failing base implementation. The identical native consumer refuses on upstream and succeeds on the rebased patch. The original [owned-copy overlay](https://github.com/pquerna/solidcore/commit/18ad8bd1b9d6a1db5e51cf0439bb3db5df31dcd4) is small and independent of the unmerged mesher. | **Yes: a narrow capability issue/proposal.** Do not describe it as corruption in a successful copy or as a demonstrated C++-parity regression. Confirm maintainers want this Rust-owned-wrapper behavior before proposing a PR.                     |
| Cumulative work, cooperative cancellation and visible-storage admission for meshing | The fork lane `pquerna/tess-work-control` adds a `Control` (`charge`/`admit`, a typed `Stopped` refusal) to upstream `solidcore-tess` `plan_with`/`evaluate_with`, with the edge nodes' curve parameters. Spiling no longer uses the inherited `solidcore-mesh` or its resource-control overlay. | **Feature/API discussion, not a correctness bug.** Small additive interface on upstream tess; offer it when upstream PRs are approved. |
| Housing source/domain/section qualification in source5/planner24                    | Source allowance provenance, native zero-set coverage and original operand/material correspondence are Spiling-owned in the current reconstructed-source producer. Existing bounded charts qualify ordinary/plate/H1 flows, not arbitrary quotient cuts.                                                                                                                                                                                                                                                                              | **Do not file upstream.** No independently established native primitive violation is demonstrated. ADR005's pending native-first material law does not inherit the custom reconstruction requirement.                                               |
| Source5 corner reference-star construction                                          | Spiling owns the reconstructed material's quadratic comparison, closed-apex proof, source-only cost admission and original cap enclosure.                                                                                                                                                                                                                                                                                                                                                                                             | **Not a kernel bug.** This construction is being retired under the adopted native-first decision, not ported into SolidCore.                                                                                                                        |

### Existing related upstream work

- [PR #633](https://github.com/tangency-labs/solidcore/pull/633) contains merged
  dense-grid work; it is not the unmerged BREP mesher.
- [Issue #77](https://github.com/tangency-labs/solidcore/issues/77) discusses
  cooperative cancellation generally.
- [Issue #509](https://github.com/tangency-labs/solidcore/issues/509) mentions
  bounded-budget tessellation as a regression-runner extension. It does not
  supply a standalone SolidCore reproducer for our caller-budget contract.
- Targeted primary-source searches found no exact STEP-copy or combined
  mesh-budget duplicate. This is a search-limited result, not proof of absence;
  check current issue/PR state immediately before filing.

## Why the patches exist, and their SMLib relationship

**STEP copying is needed by the current importer, not by an inherent geometry
requirement.** [`from_schema4_brep`](../../crates/geometry/src/import/brep_data_bridge.rs)
puts cylinders, cones, spheres and tori inside `StepSurface` and passes them to
`make_topology_from_data`. The native builder's
[`face_surface`](https://github.com/tangency-labs/solidcore/blob/2d37267fea023715d4e15e3baf59bef7b4e399f9/crates/solidcore/src/topology/brep/p10.rs#L783-L850)
copies each input carrier into the BREP. Disabling analytic enrichment just
switches from `copy_and_add_analytics` to `copy`; it does not avoid the copy.
Spiling's later borrowed face getter does not bypass this construction step.

The wrapper itself is **not a new Rust concept**.
SMLib's [`SmSTEPSurface.h`](https://github.com/NVIDIA-Omniverse/usd-brep/blob/c9c5979/source/SMLib/inc/SmSTEPSurface.h)
documents an evaluator that forwards to `EvaluateSTEP`, primarily for lifting
STEP-parameterized UV curves into 3D. Its analytic angular parameters are degrees.
The class does not override `Copy` or `CopyAndAddAnalytics`.
The inherited [`SmSurface.cpp`](https://github.com/NVIDIA-Omniverse/usd-brep/blob/c9c5979/source/SMLib/src/SmSurface.cpp#L746-L819)
returns `SM_ERR` from `Copy`, and `CopyAndAddAnalytics` delegates to that virtual
copy. SolidCore's refusal therefore follows the C++ baseline. Our independently
owned wrapper copy extends an inherited capability gap; it does not repair a
demonstrated Rust-port regression. The source does not establish why SMLib omitted
the override, so do not attribute a deliberate prohibition or historical motive.

**Alternative:** import ordinary native carriers and reparameterize every
affected UV curve/domain into their native charts, or introduce a consuming
topology-construction API. Neither is a flag change. Existing
[`convert_uv_from_step_to_nurbs`](https://github.com/tangency-labs/solidcore/blob/2d37267fea023715d4e15e3baf59bef7b4e399f9/crates/solidcore/src/surface/cone/p1.rs#L1300-L1355)
uses a polar conversion, not merely degrees-to-radians scaling. Unwrapping a
carrier while retaining its STEP UVs changes geometry evaluation and derivatives.
No patch-free importer has been qualified here.

**Decision:** retain the small owned-copy extension for the current importer and
ask upstream about that capability explicitly. Reconsider it when a simpler,
fully exercised native-chart import path exists; do not rewrite the importer
merely to reduce the patch count.

The meshing resource contract is now upstream `solidcore-tess` plus a small
control interface: `plan_with` and `evaluate_with` take a `Control` whose
`charge` counts the planner's own work units and whose `admit` admits retained
output before it is kept; a stop refuses the plan whole. That interface is an
extension, not a restoration of an SMLib API. The fork lanes
`brep-tessellator`, `mesh-query-budget` and `step-surface-owned-copy` (the
inherited `solidcore-mesh` and its resource overlay) have no Spiling consumer.

## Suggested STEP-copy issue

**Title:** Support independently owned copies of STEP-wrapped surfaces while
preserving their chart semantics.

**Actual:** both public copy entrypoints return `SolidError::Upstream` with code
`1001` on the reviewed upstream, for wrappers constructed with either original
ownership flag. This follows the documented base-default dispatch.

**Requested capability:** recursively copy the wrapped carrier, keep the STEP
wrapper and coordinate law, and give the result independent ownership. The
current Spiling BREP construction path calls these copy entrypoints.

Minimal native reproduction, using a checkout at the exact reviewed upstream:

```rust
use solidcore::{
    geom::axis2_placement::Axis2Placement,
    kernel::Kernel,
    surface::{cone::Cone, surface::{p1::StepSurface, Surface}},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let kernel = Kernel::default();
    let cylinder = Cone::create_canonical_cylinder(
        &kernel, &Axis2Placement::IDENTITY, 2.0,
    )?;
    let source = Surface::from(StepSurface::new(Surface::from(cylinder), true)?);
    println!("copy: {:?}", source.copy(&kernel).map(|_| ()));
    println!("copy+analytics: {:?}", source.copy_and_add_analytics(&kernel).map(|_| ()));
    Ok(())
}
```

The exercised patched consumer additionally drops the original before evaluating
both copies at `(90 degrees, 3 mm)`. It checks position `(0, 2, 3)`,
`du = (-2*pi/180, 0, 0)` and `dv = (0, 0, 1)`, for both original ownership flags.
This proves the bounded copied-cylinder path, not every wrapper or C++ parity.

**Before a PR:** obtain agreement on port-versus-extension policy, add the
consumer regression to the owning tests, cover nested wrappers and a real BREP
copy consumer, preserve typed invalid-wrapper refusal, and run upstream's
required checks. Do not mark a port unit green without its differential gate.

## Proposed meshing contract, not an OOM claim

The useful proposal is one cumulative caller allowance across edge/face candidate
generation and optional grid/deviation work; cooperative checkpoints; typed sticky
terminal refusal; fallible visible-capacity admission; and accounting for retained
owners that coexist with the query. Backend errors must not become geometric
fallback success. The fork changes `FaceBackend` budget parameters and the
fallible deviation result, so downstream callers need explicit migration.

The byte allowance does **not** bound Spade-private triangulation allocations or
all native evaluator/cache storage. Dependency output capacity is reconciled after
the dependency returns. Cancellation is cooperative, not hard preemption or a
wall-time guarantee. Process limits and independent geometry/display audits remain
Spiling obligations. Do not advertise bounded total RAM from this interface.

## Spiling consumer authority

[ADR005](../adr/ADR005-native-first-development-geometry.md) adopts the imported
SolidCore BREP as the single DEVELOPMENT material authority. The
[`geometry` adapter](../../crates/geometry/src/lib.rs) owns admission, original
STEP identity/provenance, bounded source/query controls and display publication;
kernel meshing and owned surface copies do not confer manufacturing eligibility.
Preserve the independent incidence, metric, void, resource, cancellation and
actual emitted-program replay obligations in their owning contracts. A complete
bore, box or housing program does not discharge another geometry family's gates.

Only a standalone, valid-domain kernel call with an independently established
wrong result or a specified resource-contract violation should be promoted to a
new kernel issue. Keep current support and reproduction procedures in the
[manufacturing evidence](manufacturing-evidence.md) and
[development guide](development.md), not inferred from upstream issue status.
