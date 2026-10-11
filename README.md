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

The engine and CLI implement a restricted native STEP/geometry workflow, source-backed recoverable projects, explicit printer/recipe intent, constrained planar compilation, emitted-program verification and durable software-only exports. They share authenticated gRPC services, Google Operations and ByteStream. The desktop provides project and geometry inspection controls, not manufacturing UI. Programs are **not machine ready**; no physical printer support or printability is established. Synthetic Triangle is an explicit diagnostic, never an import fallback.

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

## License

Copyright (c) 2026 Spiling contributors.
Licensed under the Open Software License version 3.0.

**SPDX-License-Identifier: `OSL-3.0`**

The canonical license text is in [LICENSE.md](LICENSE.md). See [docs/license.md](docs/license.md) for file headers, Cargo/npm metadata, attribution, and distribution guidance. Third-party materials retain their own licenses.
