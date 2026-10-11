<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# Cartesian software backend vision

Realize deterministic normalized planar paths as an inspectable software-only Cartesian absolute G-code program, with explicit units, modes, feed and nominal extrusion. TypeScript authors data; Rust validation and this reviewed backend own executable artifact semantics.

The current dialect deliberately excludes machine startup, heating, homing, thermal/firmware/calibration assumptions, collisions and physical execution. Its output is replay evidence, not machine-ready manufacturing support. Additional ordinary profiles remain inert data; genuinely custom executable behavior requires a reviewed backend and independent verification coverage.
