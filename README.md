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

The engine, CLI and CEF workbench implement source-backed recoverable native STEP projects and software-only planar manufacturing through authenticated gRPC, Google Operations and ByteStream. Operators explicitly load/Apply bounded inert printer intent, prepare parts on the declared bed and request revision-bound read-only Preflight before Compile. Eligibility is not successful slicing or physical qualification. Manufacturing schema 4 records actual layer heights, skin/infill/process/support policy and separate source/realized/added geometry; independent emitted-program verification and SPLR3 replay distinguish object, support, prime and stationary retraction events. Project format 3 / surface schema 2 persists ordered source-face finishes: definition-wide inheritance, occurrence overrides, Smooth, enable/remove and ordinary history. Bounded planar and cylindrical vertical ribs on certified upright full-height exterior native faces change derived deposition without editing source BREP. Reusable analytic trim ownership, periodic profiles and bounded material proofs are separated from carrier mapping and the existing downstream planner; see the [reviewed cylindrical finish decision](docs/adr/002-cylindrical-grip-ribs.md). Planner producer is 5; emitter/verifier remain 4. Fresh verification and exclusive durable export use the same services in CLI and desktop. Version-specific software outcomes and support limits are in [manufacturing evidence](docs/quality/manufacturing-evidence.md), not physical qualification. Programs are **not machine ready**; configuration/calibration hashes are declarations, not measurements. Random fuzzy skin, image relief, helical/crossed knurling and material/color painting remain unsupported. Synthetic Triangle is an explicit diagnostic, never an import fallback.

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

## License

Copyright (c) 2026 Spiling contributors.
Licensed under the Open Software License version 3.0.

**SPDX-License-Identifier: `OSL-3.0`**

The canonical license text is in [LICENSE.md](LICENSE.md). See [docs/license.md](docs/license.md) for file headers, Cargo/npm metadata, attribution, and distribution guidance. Third-party materials retain their own licenses.
