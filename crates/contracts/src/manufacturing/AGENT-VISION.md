<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Software manufacturing contract vision

Provide one runtime-independent, Rust-defined language for TypeScript-authored machine data, explicit planar recipes and durable independently replayed manufacturing bundles. Core, compiler, engine and clients consume the same strict schema rather than maintain alternate models.

Current scope is a software-only Cartesian planar compiler gate with fixed-height, nonempty deposition layers and bounded streaming JSON admission shared across the complete plan. Every declared layer must be represented by deposition before independent replay can establish software coverage; exact 100000-segment plans are admitted without permitting cumulative excess across paths or layers. Machine components and process declarations do not certify collision safety, firmware, startup, thermal behavior, calibration, printability or physical printer support. Execution connectors, supports, multiple extruders and additional process dialects remain outside this contract's implemented support envelope.
