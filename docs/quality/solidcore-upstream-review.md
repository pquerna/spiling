<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# SolidCore fork: upstream issue assessment

**Recommendation:** propose independently owned STEP-wrapper copies upstream as a
small capability request. Discuss caller-controlled meshing with the existing
mesher work rather than opening a generic mesh correctness bug. Do not file the
current Spiling manufacturing refusals against SolidCore without a standalone
kernel contract violation. No issue or PR has been submitted by this assessment.

## Baselines and refresh state

| Role | Exact identity |
| --- | --- |
| Reviewed upstream and fast-forwarded fork `main` | [`2d37267fea023715d4e15e3baf59bef7b4e399f9`](https://github.com/tangency-labs/solidcore/commit/2d37267fea023715d4e15e3baf59bef7b4e399f9) |
| Rebased, published patch candidate | [`1e9e917c234b982e8df1aaea4eb1857016fa1365`](https://github.com/pquerna/solidcore/commit/1e9e917c234b982e8df1aaea4eb1857016fa1365), branch [`spiling/refresh-2026-10-25-2d37267fe`](https://github.com/pquerna/solidcore/tree/spiling/refresh-2026-10-25-2d37267fe) |
| Spiling's unchanged runtime pin | [`18ad8bd1b9d6a1db5e51cf0439bb3db5df31dcd4`](https://github.com/pquerna/solidcore/commit/18ad8bd1b9d6a1db5e51cf0439bb3db5df31dcd4) |

The refresh retained upstream's `ssi` and `nurbs-grid-mesh` workspace members and
replayed the inherited tessellator plus the two Spiling overlays. The tessellator
is upstream contributor work, not Spiling-authored: preserve its authorship and
Apache-2.0 notices. Its mainline prerequisite is still separate from Spiling's
resource-control proposal. No shared branch or old pin was force-pushed.

A refreshed candidate is not a qualified replacement pin. Focused native mesh
resource/backend tests and the native STEP-copy consumer were exercised; full
C++ differential parity, the complete kernel suite and Spiling engine/CLI
qualification against the new candidate have not been run. Follow the
[daily refresh procedure](development.md#solidcore-fork-freshness) before changing
this assessment or promoting the dependency.

## Submission decisions

| Candidate | Evidence and ownership | Decision |
| --- | --- | --- |
| Independent copies of `StepSurface` wrappers | Upstream [`Surface::copy`](https://github.com/tangency-labs/solidcore/blob/2d37267fea023715d4e15e3baf59bef7b4e399f9/crates/solidcore/src/surface/surface/p2.rs#L1317-L1330) and `copy_and_add_analytics` intentionally route `Step` to the failing base implementation. The identical native consumer refuses on upstream and succeeds on the rebased patch. The original [owned-copy overlay](https://github.com/pquerna/solidcore/commit/18ad8bd1b9d6a1db5e51cf0439bb3db5df31dcd4) is small and independent of the unmerged mesher. | **Yes: a narrow capability issue/proposal.** Do not describe it as corruption in a successful copy or as a demonstrated C++-parity regression. Confirm maintainers want this Rust-owned-wrapper behavior before proposing a PR. |
| Cumulative work, cooperative cancellation and visible-storage admission for meshing | The [resource-control overlay](https://github.com/pquerna/solidcore/commit/1fef444cbaad61c0f414fe126314c079175da046) extends inherited `solidcore-mesh`, which is absent from the reviewed upstream `main`. It also controls optional grid/deviation work. Its backend trait changes are not purely additive. | **Feature/API discussion, not a proven correctness bug.** Coordinate with [draft mesher PR #636](https://github.com/tangency-labs/solidcore/pull/636); keep the large implementation in the fork until the contract and dependency path are agreed. |
| Current housing spherical quotient-cut refusal | A Spiling exclusion overcover is evaluated through Spiling's selected-trim provider instead of its coefficient-sheet interface. The selected-domain guard correctly refuses the query. | **Do not file upstream.** Fix the local caller while retaining literal/natural restrictions and independent material ownership. |
| Current bore reference-star Jacobian refusal | Spiling constructs the comparison star and its interval determinant; the enclosure straddles zero. The receipt does not establish whether the actual recipe folds or the enclosure loses dependency. | **Do not file upstream yet.** Isolate a kernel primitive contract violation first; failure of a local strict-sign proof is insufficient. |

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

## Suggested STEP-copy issue

**Title:** Support independently owned copies of STEP-wrapped surfaces while
preserving their chart semantics.

**Actual:** both public copy entrypoints return `SolidError::Upstream` with code
`1001` on the reviewed upstream, for wrappers constructed with either original
ownership flag. This follows the documented base-default dispatch.

**Requested capability:** recursively copy the wrapped carrier, keep the STEP
wrapper and coordinate law, and give the result independent ownership. Native
BREP surface extraction/duplication consumers call these copy entrypoints.

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

## Manufacturing failures that remain Spiling-owned

- **Housing:** `pole_comparison::misses_original_box` receives a both-inward-signs
  foreign collar overcover, then calls strict `surface_enclosure` through the
  selected original domain. The refused box crosses that domain. See
  [`pole_comparison.rs`](../../crates/geometry/src/analysis/critical/material_link/vertex_transport/uv_ownership/pole_comparison.rs)
  and [`material_link.rs`](../../crates/geometry/src/analysis/critical/material_link.rs).
- **Bore:** `regular_closed_star` fails on the middle reference piece near
  `theta=0`; its factored determinant encloses both signs while its angular
  determinant is positive. This is local reference construction/proof, not a
  measured kernel surface-Jacobian error. See
  [`uv_ownership.rs`](../../crates/geometry/src/analysis/critical/material_link/vertex_transport/uv_ownership.rs).
- The earlier needless local-extremum search, trim-versus-carrier queries,
  missing original cylindrical inverse, affine-gauge assumption and horizontal
  selected-PCurve constancy reduction were also Spiling-owned. Their owning
  changes are [70a8cad](https://github.com/pquerna/spiling/commit/70a8cad),
  [82cf916](https://github.com/pquerna/spiling/commit/82cf916),
  [e638dbd](https://github.com/pquerna/spiling/commit/e638dbd),
  [f20c139](https://github.com/pquerna/spiling/commit/f20c139) and
  [0fd4ea9](https://github.com/pquerna/spiling/commit/0fd4ea9).

Only a standalone, valid-domain kernel call with an independently established
wrong result or a specified resource-contract violation should be promoted to a
new kernel issue. Full bore and rounded-housing manufacturing remain unqualified;
working box output and bore source preparation do not close that gate.
