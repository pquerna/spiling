<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
-->

# Captured deposition fixtures

These are generated nominal toolpaths from project-authored shapes, not imported
customer models or copies of slicer source. Each text header records its producer
and options or engine hash. `tools/route-lab/capture.py` owns regeneration; start a
new frozen cohort when changing captures.

The PrusaSlicer captures retain extrusion geometry, variable widths, roles and
print order from its G-code. Cathedral captures retain Spiling plan geometry and
physical intervals after checking path-start order against exported extrusion
runs. Walls retain direction and seam; native bridges retain direction. The
capture checker is an order witness, not an independent full-program verifier.
Dependencies between captured bands are conservative XY bead-box overlaps with
the band immediately below, not native anchor proofs.

The Rust matrix declares head geometry and travel behavior separately. These
fixtures do not reproduce a producer's travel, retraction, pressure, startup or
whole-part manufacturing behavior. Native captures remain bound to their
historical producer; production adapter qualification needs current native
inputs and independent actual-program replay.
