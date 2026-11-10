<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Tolerant BREP slicing, curved toolpaths and feature-first heights

Status: accepted. Amends [ADR005](ADR005-native-first-development-geometry.md)
and [ADR008](008-construction-goals.md) on acceptance, not authority: the one
imported SolidCore BREP stays the only material source. What changes is how we
decide a slice is good enough to print.

## Context

A pinned corpus of 30 real printed-part STEP exports
([`tools/realistic-corpus`](../../tools/realistic-corpus/)) compiles 0 times in
Spiling and slices 25 times in PrusaSlicer. After import packaging, every
refusal is a proof that could not finish, not geometry that cannot be printed:
bitwise loop closure and exact coefficient identity at chart seams, interval
arithmetic that cannot separate from zero, two required heights 1e-14 mm apart
that may never merge, and a per-slab certificate that a section holds across
its whole interval. The file's declared `UNCERTAINTY_MEASURE` reaches only a few
checks indirectly; section chaining never uses a tolerance.

FDM resolves roughly 0.01-0.05 mm laterally and a few micrometres vertically.
Requirements far below that resolution protect no print.

The BREP still matters. It gives exact feature heights, true curves, face
identity on every wall stroke and exact normals; that is why Spiling slices the
BREP and not a mesh. Today those curves stop in geometry: plan paths are
polylines and the program is G0/G1 only.

## Decision

### Guarantees we keep

1. **Material fidelity, measured.** Deposited regions approximate the source
   solid within a declared lateral tolerance and the layer schedule's vertical
   quantization. Each layer records the measured maximum deviation and how it
   was obtained.
2. **Sane topology.** Every section is closed, non-self-intersecting and
   correctly nested, so holes stay holes and protected features keep their
   clearance. This is checked on the result, not proved on the way to it.
3. **Exact output, independently replayed.** The verifier decodes the actual
   program and checks it against the plan, as today. This is the guarantee that
   protects the printer and the user.
4. **Integrity and determinism.** Hash-bound sources and results, atomic saves,
   the same output for the same input.
5. **Honest provenance.** Results say which tolerance applied and where any
   fallback was used. A layer whose exact certificate also succeeds may carry a
   "certified" mark. Certification is never a gate.

### One tolerance policy

Three values, named once, recorded in provenance:

| Tolerance      | Default                                           | Governs                                                      |
| -------------- | ------------------------------------------------- | ------------------------------------------------------------ |
| Join `τ`       | max(source uncertainty, 1e-6 mm), at most 0.01 mm | Treating vertices, edge ends and seam gaps as the same point |
| Profile `ε_xy` | 0.01 mm, at most bead width / 20                  | Curve-to-segment approximation and section deviation         |
| Height `ε_z`   | 0.001 mm                                          | Merging near-coincident feature heights into one event       |

Gaps within `τ` are joined. A gap larger than `τ` in an otherwise closed loop is
a defect in the source: that layer refuses with the location and size. No other
per-site tolerance, proof precondition or work-unit ceiling decides
admissibility. Resource use is bounded by the session memory envelope and
cancellation.

### Sections

For each face, intersect the layer plane with its surface, trim in the face's
parameter domain, and chain spans across faces by coedge identity where present
and by endpoint proximity within `τ` otherwise.

- Planes, vertical cylinders, cones, spheres and tori with vertical axes give
  exact lines and circular arcs, carried as such.
- General NURBS and tilted analytic surfaces are traced with error control to
  `ε_xy`.
- If one face's intersection cannot be traced, that face alone uses its
  tessellation, still tagged with its face identity, and the layer records it.
  The layer refuses only if the result is not a valid closed section.

Then validate and orient the loops, nest them, drop loops with area below
`ε_xy²` and record what was dropped. Slabs are sampled sections, not interval
proofs. Near a feature height, the sample sits inside the layer and the feature
height bounds the layer, so no section is taken through a degenerate plane.

### Feature-first heights

1. Collect feature heights: horizontal faces (up- and down-facing), vertical
   extents of curved features, and authored process boundaries. Merge any
   within `ε_z`.
2. Place skins first. A top surface's last layer ends at its height; a bottom
   surface's first layer starts at it; the declared skin thickness counts down
   or up from there.
3. Fill each interval between anchors with layers of equal height inside the
   recipe envelope, as ADR008 already allows. Where an interval cannot fit, move
   the nearest non-protected anchor by at most `ε_z` before refusing.
4. Later: limit the height change between adjacent layers, and adapt height to
   surface slope on domes and chamfers.

### Curves in the plan and the program

- A deposition path is a sequence of segments, each a line or a planar circular
  arc at constant Z (end point, centre, direction). Arcs come from analytic
  section curves; wall offsets of an arc stay arcs; fills stay lines.
- Printers declare arc support as a capability (for example Marlin
  `ARC_SUPPORT`, Klipper `[gcode_arcs]`). With it the emitter writes `G2`/`G3`
  with `I`/`J`. Without it the emitter linearizes each arc to `ε_xy`. The plan
  is the same either way.
- The verifier decodes `G2`/`G3`, checks the endpoint against the plan on the
  output lattice, checks that the centre is equidistant from both endpoints
  within the lattice error, and computes extrusion from arc length.

### What other slicers do

PrusaSlicer (`arc_fitting`), Bambu Studio and OrcaSlicer (`enable_arc_fitting`)
all emit `G2`/`G3` by fitting arcs afterwards to polylines sliced from a mesh,
with the G-code resolution as tolerance. All default to off in code; Prusa's
own MK3, MK3.5, MK4, XL and MINI profiles turn it on, Bambu Studio's common
profile leaves it off, and OrcaSlicer's Bambu profile turns it on. OrcaSlicer
recommends off for Klipper, which splits arcs back into segments. Firmware
subdivides arcs into short segments in any case, so the benefit is smoother,
more compact motion, not a different surface.

Spiling derives arcs from exact BREP circles instead of refitting triangle
edges. Arc emission follows the selected printer: on for Prusa- and
Marlin-class firmware that declares arc support, off by default for Klipper.

## Consequences

- ADR005's "there is no mesh slicing fallback" becomes: a single face may use
  its own tessellation when its intersection cannot be traced, reported per
  layer; there is still no whole-part mesh path and no second material source.
  `AGENTS.md` and the geometry rules are updated with this change.
- Existing exact paths remain and supply the "certified" mark. Proof-only
  refusals (exact closure, interval separation, unmergeable heights) stop being
  terminal.
- Plan, contracts, verifier, SPLR and TypeScript consumers change together for
  segment kinds; format numbers stay as the pre-release policy says.
- Some existing tests assert proof refusals as expected behaviour. They are
  rewritten to assert the guarantees above, or removed.

## Tolerant mode as delivered

Step 1 is a job-wide mode, not yet per face. When the exact native path
refuses for a geometric reason or exhausts its proof work, and the job uses no
finish, generated support or fixed-dual routing, the planner reslices the whole
job within tolerance:

- Sections come from the BREP's own watertight tessellation at half the
  profile tolerance, chained by mesh edge identity, joined within `τ`, then
  simplified within the remaining profile budget. Each segment keeps its face.
- Feature heights are flat faces and part extremes, merged within `ε_z`; a
  height within `ε_z` of the bed is bed contact.
- Walls are classic constant-width perimeters, the section offset inward to
  each bead centreline, with one thinner wall at the narrowest admitted width
  where a section is thinner than a bead. The exact variable-width wall
  planner remains the exact path's.
- The plan carries `PreflightEvidence::Tolerant` and claims no native
  certificate. Fresh independent replay is unchanged.

Two outcomes apply in both modes. With supports disabled, an unanchored stroke
prints, as other slicers do, and its span is reported on the layer as
`unsupported_overhang_mm`. A fill, skin or bridge row shorter than one minimum
command is omitted, since it holds less material than the profile tolerance.

Cathedral (`web_roof`) infill builds on general parts in both modes; the
[manufacturing contract](../protocol/manufacturing.md#web-and-roof-infill) owns
its construction.

## Delivery and measure

Each step lands on `main` with the corpus result. The measure is corpus parts
reaching source-absent verified export, compared with PrusaSlicer's 25 and with
material, time and travel counts on the same parts.

1. Tolerance policy and height merging (`ε_z`, `τ`), with tolerant section
   chaining. Delivered as the job-wide tolerant mode above.
2. Tolerant seam and edge correspondence within `τ`; per-face fallback.
3. Import packaging: single-solid assembly wrappers, multi-body parts, AP242,
   assemblies as plate occurrences.
4. Arc segments in the plan, emitter and verifier, from analytic walls.
5. Height smoothing and slope adaptation.

## Alternatives rejected

- **Whole-part tessellation slicing.** It would print the corpus fastest but
  loses exact heights, curves and face identity, which are the reason to slice
  a BREP.
- **More exact proof per surface family.** Each family takes significant work
  and real exports keep producing new ones; the corpus shows the approach does
  not converge.
- **Raising individual tolerances where tests fail.** It keeps the scattered
  policy that produced these refusals.
