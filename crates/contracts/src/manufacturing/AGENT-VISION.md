<!-- SPDX-FileCopyrightText: 2026 Spiling contributors -->
<!-- SPDX-License-Identifier: OSL-3.0 -->

# Manufacturing contract vision

Provide one bounded inert Rust-defined language for printer/job data, explicit planar recipes, classified paths and independently replayed durable bundles. Core/compiler/native clients consume the same strict schema; protobuf adapters and generated shell DTOs do not establish another authority.

One single-extruder complete Marlin 2 job declares named machine/firmware/configuration/calibration provenance and typed native-home, thermal-wait, prime and final-park data. Diagnostic body-only compilation is an explicit distinct software workflow. Recipe skins request physical thickness with conservative layer rounding; sparse fraction and per-layer material ownership accounting preserve protected holes and intentional nominal void. Classification metadata is not strength or physical deposition certification.

Opt-in SPLR schema2 inspection carries actual object and lifecycle XYZ/E/F, source lines and validated kind attribution without another program parser or giant copied motion vector. Body deposition/path counts remain separate from lifecycle/prime counts; homing trajectories are firmware-owned and excluded.

Independent software decoding remains mandatory on compile and reopen. Command-space/modal/wait/barrier completeness does not certify firmware configuration truth, calibration, thermal sensor behavior, collision clearance or physical printability. Hardware selection, actual metadata/measurements and supervised physical prints remain unresolved external gates; no connector, arbitrary macros, multiple extruders, universal scheduler or physical qualification claim.
