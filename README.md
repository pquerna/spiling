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

The native engine and CLI implement source-backed STEP project authoring over protocol v4: shared rigid instances, bounded undo/redo, exclusive writers, save/open, read-only inspection and explicit checkpoint recovery. They also compile explicit printer/recipe intent through native sections into a constrained planar solid-fill plan, independently replay the emitted program, persist the immutable bundle and export it after fresh-process reopen. **These are software-validation artifacts, not machine-ready programs or physical printer support.** Geometry remains a narrow planar/cylindrical subset, not general STEP; manufacturing additionally rejects slopes, unsupported growth, partial top layers and unresolved thin regions. The thin CEF/WebGPU workbench provides project/inspection controls, not manufacturing UI. [Manufacturing evidence](docs/quality/manufacturing-evidence.md), [historical authoring evidence](docs/quality/authoring-evidence.md) and [geometry evidence](docs/quality/geometry-evidence.md) delimit acceptance.

Packaged WebGPU presentation and physical-platform acceptance remain unverified. Windows, macOS, and Linux are intended targets, not certified support claims. See the [acceptance evidence](docs/quality/B0-evidence.md) for measured results and unmet gates.

## Experimental installation from source

This setup is for contributors evaluating unfinished native authoring, geometry inspection and software-only manufacturing, never for operating a machine.

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
- [Native geometry boundary and acceptance evidence](docs/quality/geometry-evidence.md)
- [Recoverable project authoring and measured support limits](docs/quality/authoring-evidence.md)
- [Source-backed project format and durability contract](docs/protocol/project.md)
- [Software-only manufacturing workflow, proof and limitations](docs/quality/manufacturing-evidence.md)
- [Printer authoring, normalized plans and independent replay contract](docs/protocol/manufacturing.md)

## License

Copyright (c) 2026 Spiling contributors.
Licensed under the Open Software License version 3.0.

**SPDX-License-Identifier: `OSL-3.0`**

The canonical license text is in [LICENSE.md](LICENSE.md). See [docs/license.md](docs/license.md) for file headers, Cargo/npm metadata, attribution, and distribution guidance. Third-party materials retain their own licenses.
