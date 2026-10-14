<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Spiling

Spiling aims to turn authoritative CAD/BREP geometry and explicit manufacturing requirements into inspectable, verified manufacturing artifacts—not just a mesh preview. The intended workbench preserves geometry and source-face identity from import through planning, then independently verifies the emitted machine program.

The first intended product is a recoverable STEP-to-print workflow for one tested machine configuration, with a desktop workbench and CLI using the same native Rust engine.

## Status

**Spiling is not a functional manufacturing product yet. It is not recommended for use.**

The engine, CLI and CEF workbench implement source-backed recoverable STEP projects and a software-only slicer through authenticated gRPC, Google Operations and ByteStream. Operators load explicit inert printer intent, prepare the bed, run read-only Preflight, Compile and independently replay/export actual G-code. Project4/manufacturing5/surface3/planner6 persists ordered face finishes and content-addressed height images. Certified upright full-height exterior planar/cylindrical faces support ribs and imported image relief varying in both source U and Z, with source BREP unchanged. Import/apply is atomic and undoable; canonical preview, remapping, save and fresh read-only reopening do not require the original image afterward. Image events and adaptive displacement-error bounds feed ordinary walls/skins/infill and final-shape support corridors. Native geometry and realized material have independent cache identities; nominal-bead resolution advisories are not physical texture verification. See the [image-relief interfaces and support dependencies](docs/adr/003-spatial-image-relief.md) and [measured manufacturing evidence](docs/quality/manufacturing-evidence.md). Emitter/verifier4 and SPLR3 remain unchanged. Programs are **not machine ready**; configuration/calibration hashes are declarations, not measurements. Dedicated procedural texture controls, material/color painting, branching/fin supports and printer execution remain outside implemented support. Synthetic Triangle is an explicit diagnostic, never an import fallback.

Packaged WebGPU presentation and physical-platform acceptance remain unverified. Windows, macOS, and Linux are intended targets, not certified support claims. See [geometry evidence](docs/quality/geometry-evidence.md), [software manufacturing evidence](docs/quality/manufacturing-evidence.md), [operation evidence](docs/quality/engine-operations.md) and [bootstrap evidence](docs/quality/B0-evidence.md) for exercised results, support limits and unmet gates.

## Experimental installation from source

This setup is for contributors evaluating the unfinished native workbench and software workflows, not for manufacturing or operating a machine.

Install Node **24.x**, Rustup, and the [platform-native prerequisites](docs/quality/development.md#prerequisites), then:

```sh
git clone https://github.com/pquerna/spiling.git
cd spiling
npm install --global pnpm@10.32.1
pnpm bootstrap
```

The repository pins the Rust toolchain and dependencies. The first native build downloads a substantial CEF distribution. See the [development and packaging guide](docs/quality/development.md) for launching the development shell, checks, and experimental packages.

## Documentation

- [Development, packaging, and contributing](docs/quality/development.md)
- [Architecture and bootstrap acceptance](docs/quality/B0-plan.md)
- [Engine control protocol](docs/protocol/control.md)
- [Measured status and support limits](docs/quality/B0-evidence.md)
- [Native geometry support and evidence](docs/quality/geometry-evidence.md)
- [Project format and persistence](docs/protocol/project.md)
- [Software manufacturing contracts](docs/protocol/manufacturing.md)
- [Cylindrical finish interfaces and general-purpose foundations](docs/adr/002-cylindrical-grip-ribs.md)
- [Mapped image relief, durable assets and final-shape support flow](docs/adr/003-spatial-image-relief.md)

## License

Copyright (c) 2026 Spiling contributors.
Licensed under the Open Software License version 3.0.

**SPDX-License-Identifier: `OSL-3.0`**

The canonical license text is in [LICENSE.md](LICENSE.md). See [docs/license.md](docs/license.md) for file headers, Cargo/npm metadata, attribution, and distribution guidance. Third-party materials retain their own licenses.
