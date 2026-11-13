<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
-->

# macOS development builds

ARM64 development builds use ordinary Cargo and one adjacent workbench/engine/CLI
layout. They are unsigned local test artifacts. Signing, notarization, release
DMGs require a separately qualified Mac release builder. Intel macOS and universal
output are excluded. The application metadata and proposed macOS 13 floor live in
[`app.json`](app.json); Cargo owns the version. This floor requires native testing,
not just a successful cross-build.

## Native Apple Silicon Mac

Install Xcode command-line tools, Node/pnpm from the root manifests and the pinned
Rust toolchain. No OSXCross environment should be sourced on a Mac.

```sh
rustup target add aarch64-apple-darwin
pnpm install --frozen-lockfile
pnpm macos doctor
pnpm macos build
pnpm macos bundle --output /absolute/new/Spiling-dev.app
open /absolute/new/Spiling-dev.app
```

These commands build all three binaries in the **dev** profile. Ordinary
`cargo build`, `cargo mac-arm`, and `pnpm dev` remain available. `pnpm dev` builds
adjacent host binaries before launching. `cargo run -p spiling-workbench` alone
does not build its engine helper. `CARGO_TARGET_DIR` is honored. Source/binary
identity, dirty status, toolchain and Mach-O audits are recorded inside the dev
bundle. A dev bundle permits dirty source and is not a distributable package;
`pnpm package` retains clean-source release/notices/source-archive requirements.

The current CubeCL CPU backend provisions Tracel LLVM automatically on a native
Mac. Metal is enabled for **workbench rendering**. Compute currently uses Vulkan
when available or the CubeCL CPU backend; native Metal compute is not implemented.
No feature removes CPU geometry/planning to make a cross-build easier.

## Linux prerequisites

An Apple SDK and target LLVM libraries are private operator-provisioned toolchain
inputs. The repository neither downloads nor redistributes an SDK. Keep these in
one deliberate prefix outside the checkout, or mount that prefix into a private
container. No global Apple `CC`, `CXX`, `AR`, pkg-config override or system install
is required. Target overrides are scoped to the Cargo child process.

Use OSXCross revision `00bbf6b090a1f5fd8710adfe5d7da56da8282f96`, inspected with
its `README.SDK.md` and `README.BUILD-DEPENDENCIES.md`. Follow its SDK packaging
instructions under applicable Apple terms. A representative build, after
installing its documented Linux host dependencies, is:

```sh
# Choose an owned, persistent toolchain prefix and a privately supplied SDK archive.
git clone https://github.com/tpoechtrager/osxcross.git "$toolchain_prefix/osxcross"
git -C "$toolchain_prefix/osxcross" checkout 00bbf6b090a1f5fd8710adfe5d7da56da8282f96
cp "$private_sdk_archive" "$toolchain_prefix/osxcross/tarballs/"
(cd "$toolchain_prefix/osxcross" && \
  UNATTENDED=1 BUILD_FLAVOR=stable ENABLE_ARCHS=arm64 OSX_VERSION_MIN=13.0 ./build.sh)
```

The actual source build with a private SDK remains a provisioning step; this
recipe is not evidence it ran on this worker. Record the SDK archive SHA-256 and
version, installed wrapper paths, source revision and compiler identity. The
OSXCross `target/bin/arm64-apple-darwinXX-clang`, `clang++`, `ar`, `ranlib` and
`target/SDK/MacOSX*.sdk` names depend on that SDK; use the installed paths in the
private receipt, not an assumed `/opt/macos` layout. Do not use raw `ld` as Cargo's
linker. No public Docker image containing an SDK is supplied.

### Target LLVM capture

Spiling uses CubeCL's CPU JIT, which links LLVM. A Linux LLVM archive is not a Mac
library, and a Mac `llvm-config` cannot be run on Linux. On an ARM64 Mac, after a
native build has installed the matching LLVM bundle:

```sh
pnpm macos capture-llvm --output /absolute/new/mac-llvm-queries
# --prefix /path/to/tracel-llvm-23.1.0-4 may override the normal Mac cache path.
```

Copy that **matching** Tracel LLVM `include/` and `lib/` tree plus the query capture
to the private Linux toolchain prefix. Preserve original LLVM notices and the
bundle/archive checksum with the operator's toolchain inventory. The capture
records the exact llvm-config hash and Mac version; queries relocate the LLVM
prefix and record the ARM64 target and LLVM version. It never runs copied Mac
programs on Linux. Required queries are version, include/lib directories, static
library order, system libraries and C/C++ flags.

The attributed `vendor/tracel-llvm-bundler` adaptation only changes Darwin
**cross-build** link configuration: it requires explicit target archives/captured
queries and chooses the target C++ runtime. Host build scripts/proc macros keep
host libraries. Native provisioning is retained. Cross metadata is trusted build
input, not executable application input; use the capture from the actual bundle,
not a hand-invented list of libraries.

### Build from a private receipt

Copy [`toolchain.example.json`](toolchain.example.json) outside the repository and
replace every placeholder with your actual absolute paths and fingerprints.
Do not commit SDK paths/assets or credentials. Then:

```sh
rustup target add aarch64-apple-darwin
pnpm macos doctor --toolchain /private/toolchain.json
pnpm macos build --toolchain /private/toolchain.json
pnpm macos bundle --toolchain /private/toolchain.json --output /absolute/new/Spiling-dev.app
```

`SPILING_MACOS_TOOLCHAIN` can supply the receipt path. The driver checks SDK
settings/version, target LLVM identity, compiler target and required paths before
building all actual binaries. It records the receipt/SDK-settings hashes and
compiler version. This is development provenance, not a claim that a pre-existing
SDK directory has been independently reconstructed from its recorded archive.
Use the receipt's checksum-verified original archives when reprovisioning.

The SDK-independent step is useful immediately on Linux:

```sh
pnpm macos check-core
```

It checks the real model-import/contracts crates for ARM64 macOS and executes
host protoc/proc macros normally. It does **not** qualify engine/workbench linking,
SQLite/Zstandard, native windows or a Mac runtime. A missing SDK/target-LLVM
receipt is an explicit blocker to full graph acceptance, not a successful
cross-build.

## Bundle checks and Mac execution

The shared bundler requires all three Mach-O executables before creating an app.
It independently reads load-command bounds, ARM64 architecture, deployment target,
system dylib references and rpaths. Missing/ELF/Intel helpers, a higher OS floor or
Homebrew/developer/unresolved dylibs fail. Initially the binaries must link only
system dynamic libraries; package a new dependency's transitive closure explicitly
before admitting it. The tool does not silently copy or rewrite dylibs.

Transfer an unsigned dev bundle using an archive that preserves executable modes:

```sh
tar -czf Spiling-dev-macos-arm64.tar.gz -C /absolute/parent Spiling-dev.app
```

On a Mac, extract it and launch from a different working directory without an
engine override. Verify first-run license assent; real file dialogs and Keychain;
Metal source/toolpath rendering and input; import/compile/save; remove source,
reopen/freshly verify/export; close and observe the adjacent engine exit. Run
macOS storage and native client tests. Cross-compiled tests cannot execute on
Linux. A copied dev app is not a Gatekeeper/notarization or oldest-OS acceptance
result. Diagnose downloaded/quarantined unsigned artifacts on a test Mac; do not
turn off machine-wide security to make a smoke pass.

## Release boundary

A later Mac builder must qualify clean release packaging, nested engine/CLI
signing, the CPU JIT's actual hardened-runtime/entitlement requirements, Developer
ID notarization/stapling, DMGs and the downloaded/quarantined distribution path.
No blanket JIT/library-validation entitlements or Linux signing credentials are
added by this development tool. Keep the selected unsigned build/input identity
separate from signing/notarization timestamps and final archive identity.
