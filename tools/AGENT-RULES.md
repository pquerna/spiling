<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Tooling rules

- Bootstrap, runtime, checks and packaging commands are implemented in `run.mjs`; bounded trusted intent authoring uses `compile-manufacturing-intent.mjs`. Keep command failures fatal and preserve child exit status.
- Use Node 24 and pnpm 10.32.1, Rust 1.99.0 and committed lockfiles. Never silently substitute runtime/kernel/toolchain backends.
- Stage native sidecars using the Rust target triple and ship license/source/provenance notices. Regenerate the release notice directory without stale dependencies. Retain the native CEF archive's original LICENSE.txt and CREDITS.html as well as crate/npm license texts; the flattened upstream cache omits LICENSE.txt. Extract it once, stopping after the member with GNU tar `--occurrence=1` or Windows/macOS BSD tar `--fast-read`. Tool-managed artifacts stay ignored.
- Build operations must be portable across Windows, macOS and Linux; shell-specific acceptance runners are explicitly platform-labeled.
- Contract generation owns generated files. Check drift without rewriting output.
- Packaging compiles release without bundling, collects notices after CEF is available, then bundles the existing binaries. Do not require a debug bootstrap build just to package release.
- Respect CARGO_TARGET_DIR for sidecar staging and smoke executable paths. Constrained hosts use CARGO_BUILD_JOBS=1 and RUST_TEST_THREADS=1; do not overlap native workloads and CEF/compiler peaks.
- Exercise actual engine/CLI and CEF desktop commands, not mock geometry. Geometry desktop smoke uses the startup-only fixed fixture root/privileged token gate and real DOM imports, placements, canvas picks, sections, cancellation and restart. Diagnostic Triangle remains explicitly synthetic.
- Automatic CI checks contracts/shared transport/TypeScript only; native geometry and desktop/packaging checks are deliberate local or opt-in evidence. See development and geometry evidence; report unexecuted platforms and physical hardware as unverified.
- `manufacturing:intent` type-checks and executes explicitly trusted TypeScript authoring modules to produce bounded inert JSON. It is not a sandbox. The engine validates generated-contract data in Rust and never executes profile TypeScript; no tool-level success implies physical support.
