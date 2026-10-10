<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# Cartesian software backend rules

Emit one constrained absolute Cartesian dialect from validated normalized paths and intent. Explicit setup is G21, G90, M82, G92 E0. Every path starts with G0, then each subsequent point is G1. All moves carry XYZ and explicit F in mm/min; only deposition carries monotonically increasing absolute E. XYZ/F precision is six decimals and E precision eight. No implicit path closure or trailing lifecycle moves.

Nominal deposited volume is path length times rounded bead area `(width-height)*height + PI*(height/2)^2`, times flow multiplier; divide by round filament area for E. Compute from normalized path distances; independent verifier compares rounded emitted coordinates and extrusion with declared error bounds. Reject precision collapse, nonfinite/overflow, byte/segment limits and invalid speed/flow/envelope. Cancellation is checked per path/segment. Stream directly into the bounded program, never duplicate a giant motion vector.

Programs conspicuously say SOFTWARE VALIDATION ONLY, NOT MACHINE READY. Never emit home, heat, startup/end macros, retraction, execution commands or printer-ready claims. Public `emit` does not certify replay; compiler mandates independent verification.
