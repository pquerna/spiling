<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Spiling

Spiling is an experimental desktop app for preparing FDM jobs from mechanical CAD models. You choose how the parts are filled and finished, inspect the printer instructions it generates, and save the job. The desktop app and CLI use the same native Rust engine.

It is for engineers and workshop users who start from STEP files. Spiling is early software: it is not production-ready, machine-ready, or physically qualified. Do not use it to run a printer.

## What you can do today

- Open a supported STEP model, arrange the parts or copies in the build area, and run a check before generating instructions.
- Set one infill density for every copy of a part, or a different density for just one copy. Choose a preferred seam side and fill direction, and add ribs or image relief to faces the app supports.
- Inspect the actual G-code the app generated, not just the viewport preview.
- Undo changes, save the job, and reopen it without the original CAD or image file.

A saved job keeps its own copy of the imported CAD model and any relief image, so you can move the job folder as a unit. Reopening uses those stored bytes; it does not automatically follow later edits to your CAD file. If a stored file is missing or changed, the app reports an error rather than silently substituting it.

## Current limits

Spiling is software-only. Programs are **not machine ready** or physically qualified: printability, grip, collisions, packaged desktop support, and printer execution remain unverified. Windows, macOS, and Linux are intended targets, not certified support claims.

Only some STEP models are supported; general STEP documents and assemblies are not. Tool or material switching and printer execution are not supported today. Some CAD models, and jobs that combine several complex parts, fail current compute or support limits. See [measured manufacturing status and limits](docs/quality/manufacturing-evidence.md) for the exercised envelope.

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

## Technical documentation

Support, evidence, and development:

- [Measured manufacturing status and limits](docs/quality/manufacturing-evidence.md)
- [Native geometry support and evidence](docs/quality/geometry-evidence.md)
- [Engine operation evidence](docs/quality/engine-operations.md)
- [Bootstrap and packaging evidence](docs/quality/B0-evidence.md)
- [Development, packaging, and contributing](docs/quality/development.md)

Formats and control:

- [Project format and persistence](docs/protocol/project.md)
- [Software manufacturing contracts](docs/protocol/manufacturing.md)
- [Engine control protocol](docs/protocol/control.md)

Design decisions:

- [Architecture and bootstrap acceptance](docs/quality/B0-plan.md)
- [Cylindrical finish interfaces and general-purpose foundations](docs/adr/002-cylindrical-grip-ribs.md)
- [Mapped image relief, durable assets, and final-shape support flow](docs/adr/003-spatial-image-relief.md)

## License

Copyright (c) 2026 Spiling contributors.
Licensed under the Open Software License version 3.0.

**SPDX-License-Identifier: `OSL-3.0`**

The canonical license text is in [LICENSE.md](LICENSE.md). See [docs/license.md](docs/license.md) for file headers, Cargo/npm metadata, attribution, and distribution guidance. Third-party materials retain their own licenses.
