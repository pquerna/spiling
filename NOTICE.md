<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Spiling attribution and source

Copyright (c) 2026 Spiling contributors.
Licensed under the Open Software License version 3.0 (SPDX: OSL-3.0).

Spiling is original project material by its contributors. Individual ownership and imported-material attribution remain in file notices. The project has not adopted an existing application's source as its Original Work; modifications to third-party material must be disclosed with that material's notices.

Canonical source: https://github.com/pquerna/spiling
Canonical license: LICENSE.md
Source retrieval and distribution procedure: docs/quality/SOURCE.md

Packaged dependency provenance and collected third-party license texts are under `notices/`. CEF and Chromium notices supplied by the native distribution remain applicable and must be retained. This notice does not relicense dependencies or assert legal compatibility merely from their SPDX identifiers.

CEF native redistributions include their original BSD license and Chromium third-party credits under `notices/third-party/cef/`. These upstream files are not relicensed under OSL-3.0. Cargo crate notices alone do not replace the native CEF/Chromium notices.

The manufacturing segment-diagram dependency `boostvoronoi` retains its Boost Software License 1.0 (BSL-1.0), copyright notices and upstream attribution. Its locked transitive Rust dependencies retain their own notices. The Spiling wall adapter is independently authored OSL-3.0 material; it does not relicense the constructor. The existing Cargo notice collection includes those dependency license texts and exact source/checksum provenance in packaged `notices/`.

`vendor/solidcraft/` contains only SolidCraft's `solidcraft-geom` and `solidcraft-step` libraries and their STEP conformance inputs, not the SolidCraft application or an alternate modeling kernel. Library source retains its upstream MIT OR Apache-2.0 license; the upstream conformance inputs retain CC0 attribution. Original licenses, NOTICE and ATTRIBUTION.md are preserved. `UPSTREAM.toml` identifies the exact snapshot and Spiling's homogeneous-boundary closure patch and standalone-manifest changes. Release provenance includes these local third-party packages and their license texts.

`crates/appearance/src/prusa_v7.rs` is an allocation-free Rust port of the MIT-licensed `prusa-fdm-mixer` TypeScript v7 reference functions at commit `007bb922994cdf985c324cb734f6504e805f6790` (https://github.com/prusa3d/prusa-fdm-mixer). The original copyright, full MIT notice and exact source identity are retained in that file and `crates/appearance/LICENSE-prusa-fdm-mixer.txt`. The port changes language and storage strategy while preserving the named reference's numerical semantics; the surrounding study contracts and engine remain original OSL-3.0 Spiling material. Release provenance records this in-tree component and collects its MIT notice separately. No upstream filament catalog or measurement dataset was imported by this port.
