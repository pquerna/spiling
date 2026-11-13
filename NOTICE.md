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

Packaged dependency provenance and original third-party license texts are under `notices/`. Native Rust UI/GPU/window dependencies, bundled epaint fonts and project icons retain their original notices. The distribution includes the exact public source archive and retrieval procedure. This notice does not relicense dependencies or assert legal compatibility merely from their SPDX identifiers.

The manufacturing segment-diagram dependency `boostvoronoi` retains its Boost Software License 1.0 (BSL-1.0), copyright notices and upstream attribution. Its locked transitive Rust dependencies retain their own notices. The Spiling wall adapter is independently authored OSL-3.0 material; it does not relicense the constructor. The existing Cargo notice collection includes those dependency license texts and exact source/checksum provenance in packaged `notices/`.

`vendor/solidcraft/` contains only SolidCraft's `solidcraft-geom` and `solidcraft-step` libraries and their STEP conformance inputs, not the SolidCraft application or an alternate modeling kernel. Library source retains its upstream MIT OR Apache-2.0 license; the upstream conformance inputs retain CC0 attribution. Original licenses, NOTICE and ATTRIBUTION.md are preserved. `UPSTREAM.toml` identifies the exact snapshot and Spiling's homogeneous-boundary closure patch and standalone-manifest changes. Release provenance includes these local third-party packages and their license texts.

`vendor/cpp_map/` is the `cpp_map` 0.2.0 crate (Eadf, MIT OR Apache-2.0, https://codeberg.org/eadf/cpp_map_rs) that `boostvoronoi` uses for its beach line, substituted through `[patch.crates-io]`. Its upstream headers, README and license texts are preserved. The only source change makes skip-list node levels come from a fixed-seed per-list generator instead of an OS-seeded thread RNG, so identical input builds identical segment diagrams; `UPSTREAM.toml` records the exact snapshot and changes. Release provenance includes it with its license texts.

`crates/appearance/src/prusa_v7.rs` is an allocation-free Rust port of the MIT-licensed `prusa-fdm-mixer` TypeScript v7 reference functions at commit `007bb922994cdf985c324cb734f6504e805f6790` (https://github.com/prusa3d/prusa-fdm-mixer). The original copyright, full MIT notice and exact source identity are retained in that file and `crates/appearance/LICENSE-prusa-fdm-mixer.txt`. The port changes language and storage strategy and caches byte-channel transfer values from the unchanged formula while preserving the named reference's numerical semantics; the surrounding study contracts and engine remain original OSL-3.0 Spiling material. Release provenance records this in-tree component and collects its MIT notice separately. No upstream filament catalog or measurement dataset was imported by this port.

The native `apps/workbench` frontend consumes the unmodified upstream egui/eframe/egui-wgpu 0.37 family from Cargo. Workspace-root MIT/Apache license texts and their exact tag/commit provenance are retained in `apps/workbench/licenses/egui/`; no reference application code was imported. Release provenance collects these texts and the bundled epaint font copyright/license files under their original names, including Ubuntu, Hack, Noto Emoji and egui icon notices. Portable native packaging collects these alongside exact source provenance.

## xkcd color survey

`crates/appearance/data/xkcd-rgb.txt` preserves the 949-entry xkcd color survey from https://xkcd.com/color/rgb.txt unchanged. Its first line dedicates the data under CC0-1.0; retain `crates/appearance/LICENSE-xkcd-CC0.txt` and the dataset's `.license` provenance. The snapshot is identified by SHA-256 `450cca88fa6fa9a1e79c969969e05e6900b41a94f0a3a5f134e3d0b79077f890`. Original Spiling naming code is OSL-3.0. OKLab conversion follows the equations published by Björn Ottosson at https://bottosson.github.io/posts/oklab/; no additional color library or vendor implementation is imported. Release provenance includes the dataset and its CC0 notice independently from the Prusa model port.

## Reference filament catalog snapshots

`data/material-catalogs/` imports the HueForge and OpenPrintTag aggregate JSON files unchanged from Prusa Research's `prusa-fdm-mixer`. The adjacent `manifest.json` records the exact repository revision, source URLs and file hashes. Preserve `LICENSE-prusa-fdm-mixer.txt` (copyright 2026 Ondrej Bartas (Prusa Research s.r.o.) and contributors) and the separate `LICENSE-OpenPrintTag.txt`. That notice's source pin identifies the notice owner, not an original OpenPrintTag data revision. These MIT notices do not relicense original Spiling code. No Fargo3D source file is imported directly. Release provenance records each catalog separately and collects both notices and the manifest. Declared catalog colors and HueForge TD are reference data, not calibrated measurements or manufacturing qualification; [catalog maintenance](docs/quality/material-catalogs.md) owns refresh and interpretation.
