<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Tooling rules

- Root commands are implemented in `run.mjs`; keep command failures fatal and preserve child exit status.
- Use Node 24 and pnpm 10.32.1, Rust 1.99.0 and committed lockfiles. Never silently substitute runtime/kernel/toolchain backends.
- Stage native sidecars using the Rust target triple and ship license/source/provenance notices. Regenerate the release notice directory without stale dependencies. Retain the native CEF archive's original LICENSE.txt and CREDITS.html as well as crate/npm license texts; the flattened upstream cache omits LICENSE.txt. Extract it once, stopping after the member with GNU tar `--occurrence=1` or Windows/macOS BSD tar `--fast-read`. Tool-managed artifacts stay ignored.
- Build operations must be portable across Windows, macOS and Linux; shell-specific acceptance runners are explicitly platform-labeled.
- Contract generation owns generated files. Check drift without rewriting output.
- Packaging compiles release without bundling, collects notices after CEF is available, then bundles the existing binaries. Do not require a debug bootstrap build just to package release.
- Exercise actual engine/CLI and packaged desktop commands. Refer to `docs/quality/B0-plan.md` for acceptance; report unexecuted platforms as unverified.
