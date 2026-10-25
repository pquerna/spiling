<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# SolidCore fork: upstream issue assessment

Maintain one long-lived `pquerna/*` branch and draft PR per logical change, **in
`pquerna/solidcore` only**, and compose them on
`pquerna/integration-spiling-tracking`. The long-term goal is to retire every
local overlay through upstream adoption or elimination of its consumer need.
An upstreaming recommendation is not authorization to open an upstream PR or
issue. Current Spiling manufacturing refusals need a standalone kernel contract
violation before they become upstream bug candidates.

## Baselines and refresh state

| Role | Exact identity |
| --- | --- |
| Upstream and fast-forwarded fork `main` | [`1cd31cf033ca53948d8f98a4015e95f3581ec7ab`](https://github.com/tangency-labs/solidcore/commit/1cd31cf033ca53948d8f98a4015e95f3581ec7ab) |
| Combined integration producer | [`ee4d9c3feefb3c702ca53b190b7fe4516714c841`](https://github.com/pquerna/solidcore/commit/ee4d9c3feefb3c702ca53b190b7fe4516714c841), branch [`pquerna/integration-spiling-tracking`](https://github.com/pquerna/solidcore/tree/pquerna/integration-spiling-tracking) |
| Consumer pin authority | Both declarations in [`Cargo.toml`](../../Cargo.toml), their lockfile sources and `geometry::KERNEL_REVISION` agree on an exact integration commit, never a floating branch |

The integration contains inherited tessellator work plus the separate Spiling
budget and STEP-copy overlays. Preserve the tessellator's original authors and
Apache-2.0 notices; it is not Spiling-authored. Old published candidates and
consumer pins remain reachable, but are not the maintained queue tip.

Fork `main` stays clean of overlays. Stable review lanes and their upstreaming
criteria are below; the [maintenance procedure](development.md#solidcore-fork-freshness)
owns synchronization, exact-pin promotion and published-history safety.

## Fork review lanes

| Long-lived branch | Why the current consumer needs it | Upstream/removal criterion |
| --- | --- | --- |
| [`pquerna/brep-tessellator`](https://github.com/pquerna/solidcore/tree/pquerna/brep-tessellator) | Inherited native BREP mesher used by Spiling display candidate generation. It is new Rust meshing work, not the C++-parity `SmTess` port; replacement with `SmTess` is not qualified here. | Track the existing contributor work in [upstream PR #636](https://github.com/tangency-labs/solidcore/pull/636), without submitting a duplicate. Retire this lane when upstream supplies the consumed mesher and Spiling's independent display controls pass. |
| [`pquerna/mesh-query-budget`](https://github.com/pquerna/solidcore/tree/pquerna/mesh-query-budget) | Cumulative caller work, cooperative cancellation and visible-storage admission across mesher and optional dense backend work. Host-only checks cannot interrupt internal loops or admit their owned buffers before growth. | Agree on a minimal upstream mesher control contract, not necessarily our exact API. Remove the overlay once that contract covers real callers, typed terminal refusal and retained ownership accounting, then migrate callers without shims. |
| [`pquerna/step-surface-owned-copy`](https://github.com/pquerna/solidcore/tree/pquerna/step-surface-owned-copy) | The current importer places STEP wrappers in BREP input; topology construction copies those carriers while preserving their parameter law. This extends an inherited SMLib gap, not a proven Rust-port regression. | Obtain agreement on owned-wrapper copying or an explicit equivalent construction API. Retire when upstream provides it, or when a fully qualified native-chart importer removes the wrapper dependency. |

Create draft PRs only against our fork: mesher and STEP-copy target `main`;
budget targets `pquerna/brep-tessellator`. Each PR body carries the requirement,
consumer call path, SMLib classification, dependency/base, exercised evidence,
known limits, proposed upstream contract and lane-retirement condition. The
published branches are reviewable now; fork PR creation requires an authenticated
GitHub session and is not implied by their publication.

## Submission decisions

| Candidate | Evidence and ownership | Decision |
| --- | --- | --- |
| Independent copies of `StepSurface` wrappers | Upstream [`Surface::copy`](https://github.com/tangency-labs/solidcore/blob/2d37267fea023715d4e15e3baf59bef7b4e399f9/crates/solidcore/src/surface/surface/p2.rs#L1317-L1330) and `copy_and_add_analytics` intentionally route `Step` to the failing base implementation. The identical native consumer refuses on upstream and succeeds on the rebased patch. The original [owned-copy overlay](https://github.com/pquerna/solidcore/commit/18ad8bd1b9d6a1db5e51cf0439bb3db5df31dcd4) is small and independent of the unmerged mesher. | **Yes: a narrow capability issue/proposal.** Do not describe it as corruption in a successful copy or as a demonstrated C++-parity regression. Confirm maintainers want this Rust-owned-wrapper behavior before proposing a PR. |
| Cumulative work, cooperative cancellation and visible-storage admission for meshing | The [resource-control overlay](https://github.com/pquerna/solidcore/commit/1fef444cbaad61c0f414fe126314c079175da046) extends inherited `solidcore-mesh`, which is absent from the reviewed upstream `main`. It also controls optional grid/deviation work. Its backend trait changes are not purely additive. | **Feature/API discussion, not a proven correctness bug.** Coordinate with [draft mesher PR #636](https://github.com/tangency-labs/solidcore/pull/636); keep the large implementation in the fork until the contract and dependency path are agreed. |
| Housing spherical quotient-cut qualification | The coefficient-sheet position overcovers are Spiling-owned. The remaining central/shared-exit exclusion must prove actual original operand ownership; interval overlap is not an exact identity. | **Do not file upstream.** No independently established native primitive violation is demonstrated. |
| Source corner reference-star construction | A linear radial ribbon can fold at opposed curved germs. Spiling owns the quadratic comparison, complete closed-apex proof, source-only cost admission and whole-original cap enclosure. | **Spiling change, not a kernel bug.** The source-backed bore now has real Compile/fresh-verification/export/replay evidence; this does not qualify spherical quotient cuts. |

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

The meshing resource contract is different: `solidcore-mesh` in PR #636 is a new
Rust mesher, not the C++-parity `SmTess` port. Spiling's cumulative work,
cooperative cancellation and visible-storage admission are real engine
requirements, but this particular `QueryBudget` API is an extension, not a
restoration of an SMLib API. Keep those requirements while seeking the smallest
upstream mesher control interface; do not claim SMLib has no cancellation
mechanisms generally.

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

## Source-material obligations remain Spiling-owned

The local source-star construction, original trim/curve correspondence,
comparison homotopy, admitted error and material-degree proof belong to
[`material_link.rs`](../../crates/geometry/src/analysis/critical/material_link.rs)
and its owners, not the kernel mesher or STEP surface copy API.

- [`uv_ownership.rs`](../../crates/geometry/src/analysis/critical/material_link/vertex_transport/uv_ownership.rs)
  owns the complete reference-star Jacobian and boundary proof. Quadratic radial
  reference ribbons preserve the original apex tangent and outer trace; the
  source-only selector must admit their complete physical cost before freezing G.
- [`atlas.rs`](../../crates/geometry/src/analysis/critical/material_link/atlas.rs)
  and [`pole_comparison.rs`](../../crates/geometry/src/analysis/critical/material_link/vertex_transport/uv_ownership/pole_comparison.rs)
  distinguish auxiliary position overcovers from actual selected-material queries.
  Neither can widen literal coefficient support or acquire ownership from a box.
- Rounded-housing spherical central/shared-exit exclusion remains unqualified.
  A complete bore or box program does not discharge those independent obligations.

Only a standalone, valid-domain kernel call with an independently established
wrong result or a specified resource-contract violation should be promoted to a
new kernel issue. Keep current support and reproduction procedures in the
[manufacturing evidence](manufacturing-evidence.md) and
[development guide](development.md), not inferred from upstream issue status.
