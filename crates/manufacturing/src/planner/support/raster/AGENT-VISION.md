<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# Support centerline realization vision

Realize one clearance-safe physical support body independently of changing Support/SupportInterface ownership. Sub-bead interface strips and moving role boundaries must preserve real deposited wall anchors instead of becoming artificial empty shafts. Physical exterior/hole walls remain intact; process density and material accounting remain distinct.

Rows are read off the conservative support grid on the layer's own alternating axis and anchored against actual previous support strokes; the parent publishes only what is anchored and reports the rest. General image, native and mixed-corridor jobs use the same realization; no finish-specific exception, global fixed axis, expanded bridge allowance, native-material anchor or branching support is permitted.

Acceptance requires the actual-stroke regressions and genuine public native box/hole support/contact/lower-corridor fixtures. Numerical/resource failures remain explicit and successful software replay makes no physical printability claim.
