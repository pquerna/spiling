<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Tooling rules

- Root commands belong to `run.mjs`; failures are fatal and preserve child status. Build engine/workbench/CLI together; installed lookup uses adjacent outputs. `dev` is native, with no browser runtime or build path.
- Use pinned Node/pnpm/Rust and committed lockfiles. Contract generation owns generated output; check drift without rewriting it. Preserve trusted TypeScript printer/data authoring and independent G-code observers.
- Packaging requires clean committed source, builds release with default features, collects the installed binaries' normal/build dependency closure and original font/icon notices, then includes exact public tracked source. Keep `docs/quality/SOURCE.md` and notices consistent. Never archive private/untracked workspace data. Inspection stays off in distributions.
- Portable archives use adjacent binaries (macOS: Contents/MacOS); Linux also emits Debian with `/opt/spiling` layout. Package generation is not signing/install/upgrade or universal platform/GPU qualification. Native window/picker/replay/child cleanup must be exercised from the produced layout.
- Honor `CARGO_TARGET_DIR`. Default Cargo jobs, Rust test threads, Rayon workers and the trusted TypeScript author's GOMAXPROCS to one, honoring explicit overrides. Actual native workloads additionally need externally enforced memory/task bounds and sequential execution; worker counts alone do not bound memory.
- `test:native` runs actual-child host regressions sequentially. Native window/dialog evidence is separate; keep logs, screenshots, producer/input/environment identities in ignored artifacts. [Development](../docs/quality/development.md) owns reproduction and limits.
- `manufacturing:intent TRUSTED_SOURCE.ts NEW_INTENT.json` type-checks and executes a trusted module with operator permissions, not a sandbox. Emit bounded finite inert JSON without overwriting; the engine independently validates and never executes it.
- Build/package operations must be portable across Windows/macOS/Linux. Platform-specific acceptance/install tools are explicitly labelled; never silently substitute another runtime.
