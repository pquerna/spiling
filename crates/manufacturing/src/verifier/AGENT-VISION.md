<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# Independent replay vision

Provide bounded software evidence that the actual emitted absolute Cartesian program realizes the validated normalized plan and explicit printer/recipe data. Independence from the emitter catches coordinate, extrusion, feed, mode and command-sequence defects rather than approving a preview or comparing matching emitter output.

The current verifier covers exact supported state transitions, millimetre/absolute modes, ordered path semantics, command-space bounds, speeds and nominal volumetric accounting. It streams actual commands against borrowed expected points and never trusts a persisted verification report. The first positioning command has no known physical start position; startup, homing, collision, thermal, firmware, calibration, mechanical behavior and physical support/printability remain excluded. A successful report is not permission to send the program to a printer.
