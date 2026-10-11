<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Geometry milestone evidence

**Gate decision: overall physical geometry milestone not accepted.** The frozen standalone native corpus, v2 native engine/CLI multi-part inspection and native-derived cross-language goldens are exercised below. Physical WebGPU reference-machine picking/placement, native dialog behavior and packaged geometry certification remain open; packaging polish is deferred. Independent OCCT cylinder/through-hole topology is explicitly rejected, so no general STEP or universal planar/cylindrical exporter support is claimed.

## Pinned source availability: repaired through the public fork

Current source: [pquerna/monstertruck at d87b4d9c](https://github.com/pquerna/monstertruck/tree/d87b4d9ced1f3baf31aa771ac0e7c663efb1c001), branch `spiling-dev`, exact published commit `d87b4d9ced1f3baf31aa771ac0e7c663efb1c001`. It is based on upstream `1fbc7a52555df6ccfe66aee7c877097ab1852146` (`master`, not `main`). The earlier source-availability revision `4d7abd773b5452f1a32cbbb0e491f9dbc2a45e89` removed the inaccessible `.blueprints` gitlink/guidance without changing kernel Rust. The current revision additionally repairs the native numerical paths described below. The public `resources` submodule and Apache-2.0 licensing remain.

All five Monstertruck workspace dependency declarations use this public fork URL and exact revision at version 0.4.1. `crates/geometry` consumes modeling, meshing, solid, topology and STEP I/O; Cargo.lock records their transitive kernel packages. The v2 engine composes that public facade behind its private native worker and advertises only implemented bounded capabilities. The local fork checkout tracks `upstream/master` and pushes its `spiling-dev` branch to `origin`.

The source-availability revision's floating-nightly test invocation failed in registry dependency `branches 0.4.6`: `core::intrinsics::abort` was unavailable. Its modeling library suite passed on Spiling's pinned Rust 1.99.0: 41 passed, one ignored; selected modeling all-target Clippy and native-nightly formatting passed. Those historical checks did not establish the numerical corpus gate.

Fresh-cache Cargo resolution of that source-availability revision was exercised using a throwaway manifest copied from the then-four exact workspace kernel declarations, plus `step-p21 = "0.1"`, on Rust 1.99.0 with an empty `CARGO_HOME`. Cargo fetched the public fork and `resources4truck`, without a private `.blueprints` fetch. The first smoke compilation exposed a `Result` alias collision in the throwaway main, corrected to `std::result::Result`; the final `cargo run --locked` passed against those freshly fetched sources.

The smoke constructed a public native cuboid at `(0,0,0)..(20,10,8)`, checked certified bounds, wrote/parsed its STEP once, required lossless reported shell conversion and six faces, then intersected it with the native clip cuboid `(-1,-1,4)..(21,11,9)` at tolerance `1e-5`. The result passed geometric consistency and contained one planar `z=4` cap with one loop:

```text
fork-source-smoke: box bounds 20x10x8 mm; STEP round-trip 6 faces, zero reported shell loss; native z=4 cut one cap/loop
```

This is source-resolution/native API smoke, not the admitted-profile import/cylinder/hole corpus gate. The throwaway manifest/binary were removed afterward. Registry dependency `proc-macro-error2 2.0.1` reports future compiler incompatibility on this smoke; that warning has not been suppressed.

At the source-availability step, `cargo test --locked -p spiling-contracts -p spiling-engine-client` passed 24 tests and `pnpm contracts --check` passed. That historical check did not add an application geometry capability or protocol cutover; the later v2 evidence is recorded separately below.

### Original upstream source failure

Candidate kernel: Monstertruck 0.4.1, Git revision `1fbc7a52555df6ccfe66aee7c877097ab1852146`. Its public repository clones without recursively fetching submodules. Its `.gitmodules` includes:

```ini
[submodule ".blueprints"]
    path = .blueprints
    url = ../blueprints.git
```

The pinned submodule commit is `c1bc1908be64bb9106dbb43408fa8eaceda2a72c`. Cargo recursively fetches it from `https://github.com/virtualritz/blueprints.git`. Both libgit2 and Git-CLI Cargo fetch modes failed with authentication errors. Direct `git ls-remote https://github.com/virtualritz/blueprints.git` also failed with `could not read Username ... terminal prompts disabled`.

Minimal reproduction outside this repository:

```toml
[package]
name = "spiling-kernel-fetch-reproducer"
version = "0.0.0"
edition = "2024"

[dependencies]
monstertruck-io = { git = "https://github.com/virtualritz/monstertruck", rev = "1fbc7a52555df6ccfe66aee7c877097ab1852146", default-features = false, features = ["step"] }
```

Put this manifest beside `src/main.rs` containing `fn main() {}`. Run `cargo fetch --manifest-path PATH/Cargo.toml`, then retry with `cargo --config net.git-fetch-with-cli=true fetch --manifest-path PATH/Cargo.toml`.

Observed Cargo failure:

```text
failed to update submodule `.blueprints`
failed to fetch submodule `.blueprints` from https://github.com/virtualritz/blueprints.git
revision c1bc1908be64bb9106dbb43408fa8eaceda2a72c not found
failed to authenticate when downloading repository
```

The executed probe used the same Git pin for modeling, solid and STEP I/O plus `step-p21 = "0.1"`; `cargo run` failed at dependency resolution, before compiling its main program. A Git-CLI retry also failed. This is a source-resolution failure, not evidence of a kernel numerical defect.

The original public upstream `master` remained the candidate revision at inspection, with no public upstream repair available. Spiling now uses the explicitly approved public fork above instead of private credentials, Cargo-cache modification, or a substituted local path dependency.

## Exact-checkout diagnostics, not native acceptance

A throwaway path-dependent probe compiled against the public checkout of the exact candidate revision without recursively fetching submodules. This was an adapter investigation only; no path dependency or vendored kernel was added to Spiling.

The original cuboid reported six faces and certified bounds `(0,0,0)..(20,10,8)`. Intersecting it with a padded native cuboid whose plane-side boundary was exactly `z=4`, using public `and(..., 1e-5)`, produced one planar cap with one loop. The next cylinder intersection did not finish within the probe's 120-second command deadline. Through-hole section code was not reached.

The cylinder probe used `primitive::circle` → `builder::try_attach_plane` → `builder::extrude` with modeling `Wire`/`Solid` types. A separate writer diagnostic showed its side carriers were `Surface::NurbsSurface` and emitted `RATIONAL_B_SPLINE_SURFACE` and `RATIONAL_B_SPLINE_CURVE`, not the required analytic STEP carriers/boundaries. Thus this earlier probe does **not** demonstrate a boolean defect in the admitted planar/cylindrical lane or satisfy the cylinder corpus gate. The original corpus generator now writes analytic plane/cylinder carriers and line/circle boundaries; its numerical facade gate is separate from this diagnostic history. No timeout, fixture omission, profile widening, or NURBS acceptance is being substituted for that proof.

## Standalone native facade: frozen corpus proof

The native slice is implemented in `crates/geometry`, not in the engine or desktop. Its public byte-based importer returns an immutable definition with source provenance, mm/f64 native geometry, canonical decimal source-face IDs and certified bounds. Private kernel modules own Monstertruck objects. Tessellation returns identity-free packed chunks; engine composition must attach actual scene/session/artifact identity. Native section queries return definition-local f64 loops, not placed occurrence unions. Contracts own binary layout/validation, not geometry algorithms. The mesh/section codecs and native import/tessellation/section areas each have canonical local rules and visions.

All five original valid sources and fourteen single-property adversaries are frozen under `fixtures/geometry/`. The generator constructs analytic carriers before writing original source, verifies public admission/error outcomes and refuses replacing changed frozen STEP bytes. Source and metadata carry original OSL-3.0 SPDX notices; no third-party CAD is included. `generate_corpus --check` matched all nineteen sources and metadata. Frozen manifest SHA-256: `43e0b7f255dda6c59d806bc296389ac667dfe5554aa7f4047f40802922a36c70`; plate recipe SHA-256: `c7c6f001cb6483cc5d70585390e3e360d4e9717cfdc2d8daf0b9803e6f6dc4e3`.

| Original source         | SHA-256                                                            |
| ----------------------- | ------------------------------------------------------------------ |
| `box-mm.step`           | `1b23042e12183748e12bc4e0195ae546c80296ce4170709eb3f836dce2f4ac98` |
| `box-inch.step`         | `74da493fcabe4c0bc1061eb3645f003227ea861f0196be40cd5748d870f797de` |
| `cylinder.step`         | `95df1da444dc9c49529054f1825c9013c135301df677e30269c3a789ee2b464b` |
| `through-hole.step`     | `eb2112f9ef4be523615f237b86b69569026cef8e1cafbc20e2fdca7e3459cb29` |
| `perforated-plate.step` | `0c293704778df64793df3889a93891c7878525b5874b81080bb883e79f3922dd` |

### Native small-corpus results

The actual `inspect_corpus` executable used the published Git pin and public facade, not a path override. Each import preserved every canonical face use, admitted closed consistent topology and retained immutable source geometry. Strict tessellation audited complete triangles against analytic plane/cylinder carriers and measured f32 displacement separately. The fixed acceptance bounds are 0.025 mm for each component and 0.05 mm combined.

| Source                               | Faces | Packed mesh bytes | Vertices / triangles | Maximum carrier / f32 error (mm) | Native mid-Z signed loop areas (mm²) |
| ------------------------------------ | ----: | ----------------: | -------------------: | -------------------------------- | ------------------------------------ |
| mm box, 20×10×8                      |     6 |               832 |              24 / 12 | 7.95e-14 / 0                     | 200                                  |
| actual-inch equivalent               |     6 |               832 |              24 / 12 | 7.95e-14 / 8.89e-16              | 200                                  |
| cylinder, radius 5, height 8         |     4 |            11,888 |            274 / 328 | 0.018009964 / 4.036e-7           | 78.50861427961186                    |
| 20×20×8 block, radius-3 through-hole |     8 |             9,584 |            218 / 268 | 0.023166370 / 5.458e-7           | outer 400; hole −28.228933885572776  |

The inch source normalizes to `(0,0,0)..(20,10,7.999999999999999)` mm. The cylinder's physical radial extent is ±5 mm; the certified native enclosure is ±5.5901699437494745 mm. The conservative enclosure is not an assertion of a larger physical cylinder or an exact physical AABB.

Sections use native booleans at 1e-5 mm and native-curve sampling at 0.005 mm; no display-mesh fallback is used. All reported loops closed with zero sampled plane residual and closure gap, below the 1e-4 mm limits. The box loops had 37 points, cylinder 132 points, and through-hole outer/hole 5/65 points with opposite winding. Regressions check sampled area/perimeter error bounds against the analytic box/circle/hole, oblique planes, empty versus native tangent/edge/face degeneracy, canonical identity/orientation, cancellation and resource limits. This is sampled section accuracy within the declared tolerance, not an exact polygonal representation of a circle.

One observed debug run reported native section times of 9.130 ms (mm box), 9.109 ms (inch box), 3,733.704 ms (cylinder), and 2,925.658 ms (hole). These are single-run timings on the execution environment below, not distributional benchmarks or cold-cache measurements.

### Genuine bounded plate workload

The source is a 1,978,213-byte STEP solid: a 1640×840×20 mm plate with 32×16 radius-20 through-holes, pitch 50 mm and margin 25 mm. It contains 1,030 canonical faces. The native mesh has 242,281 vertices, 270,348 triangles and **10,140,952 packed bytes**, exceeding the required 4 MiB without duplicated artificial transfer data. Its ten chunk sizes are 1,048,576; 1,048,552; 1,048,544; 1,048,544; 1,048,552; 1,048,536; 1,048,528; 1,048,568; 1,048,568; and 703,984 bytes. Each is at most 1 MiB; the aggregate is below 64 MiB and counts are below one million.

Maximum audited carrier deviation was 0.024090885591100175 mm and maximum f32 displacement 0.00006485545291420198 mm, passing the fixed separate/combined limits. The real public-pin inspector, measured with `/usr/bin/time -v`, exited zero in **23.03 s wall time**, with **70,520 KiB peak native-process RSS**, below the 1 GiB bound. Inspector import/tessellation timings were 2,391.517 / 20,609.468 ms. This debug, mesh-only workload measured native admission, meshing, audits and encoding; it did not execute a plate section, pipe transfer, engine cancellation, GPU upload or scene lifecycle. Per-chunk SHA-256 and numerical diagnostics are emitted by the inspector for reproduction.

### Kernel defects and repairs required by the original sources

The unchanged analytic cylinder exposed native curve/surface projection and duplicate triangle-collision graph events; the unchanged through-hole exposed negative-loop ownership and periodic carrier-coordinate lifting. Repairs evaluate native residuals, deduplicate only identical undirected segment keys, lift periodic UVs continuously and attach each negative loop to the smallest containing positive boundary while excluding its inverse native-edge twin. Intermediate diagnostics retain the failing stage. Every boolean result shell receives release-active topology checking; no failed cap/edge is omitted or reported as an empty success.

The original plate first exceeded the one-million-triangle budget. Keeping straight-axis subdivision bounded while preserving native parameter-division requirements produced the accepted mesh. The first passing implementation took 751.06 s; cached loop UV bounds and a stable spatial index for unchanged constraint-segment predicates reduced this to the published-pin result above. Source geometry, tolerances, accepted counts and audit errors did not change. Earlier payload hashes were not captured, so byte-identical historical outputs are not claimed.

The published fork's selected solid/meshing library suites passed 49 tests with one ignored; selected all-target Clippy with warnings denied and canonical nightly formatting passed. Existing upstream test files/expectations were not changed. This does not claim the full upstream workspace or manufacturing support.

### Exercised native/consumer checks

- `cargo test --locked -p spiling-geometry`: 33 native regressions passed, including original valid/adversarial inputs, lossless canonical face uses, long acyclic/cyclic unit references, normal/winding audits, whole-triangle cylinder error, native sections and the bounded plate.
- `cargo run --locked -p spiling-geometry --example generate_corpus -- --check`: frozen sources and metadata matched.
- `cargo run --locked -p spiling-geometry --example inspect_corpus -- fixtures/geometry/box-mm.step fixtures/geometry/box-inch.step fixtures/geometry/cylinder.step fixtures/geometry/through-hole.step`: actual native import, mesh and section outputs above.
- `/usr/bin/time -v TARGET/debug/examples/inspect_corpus --mesh-only fixtures/geometry/perforated-plate.step`: actual native plate resource result above.
- Selected existing contract (15 unit plus six integration), engine (eight) and shared-client (four) tests passed. `pnpm contracts --check`, protocol type checking and 137 TypeScript decoder tests passed. The generated TypeScript/wire contract did not change.
- Selected geometry/contracts/shared-client/engine/CLI all-target Clippy with warnings denied, `cargo fmt --all -- --check`, `pnpm -r --if-present check` and repository-wide Prettier checking passed. Rust-generated corpus JSON follows the existing generated-fixture formatter exclusion convention and is checked byte-for-byte by the generator. Forty-one local documentation references resolved.
- A throwaway `generate_corpus --out DIRECTORY` run against deliberately different existing box bytes exited with the explicit frozen-source-change diagnostic and preserved those bytes. The smoke directory was removed.
- Real `cargo run --locked -p spiling-cli -- diagnose --engine ENGINE` returned protocol 1, empty geometry capabilities and Pong with clean shutdown. `triangle --engine ENGINE --output FILE` emitted 64 synthetic bytes, independently decoded by the TypeScript protocol decoder with indices `[0,1,2]`. This proves unchanged B0 behavior, not native engine integration; the output file was removed.

Use [development commands](development.md) with the repository's pinned Rust toolchain. Verification used a temporary Cargo home/target directory for disk capacity, but resolved the committed public Git pin; diagnostic local-path overrides are not shipped.

## Version-two real native inspection

The actual CLI negotiated protocol 2, fresh session/PID, Monstertruck 0.4.1 at published `d87b4d9ced1f3baf31aa771ac0e7c663efb1c001`, all four implemented geometry capabilities and frozen limits. These runs used real child jobs and hash/layout-validated every unique chunk, even without --out; successful stdout followed observed clean shutdown.

| Actual CLI input                                        | Native outcome                                                                                                                                                  |
| ------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `two-parts.scene.json`, world z=4                       | Two definitions/three placed occurrences; 12,720 unique mesh bytes/two chunks; areas 200, 78.5086142796119 and 200.00000000000003 mm²; all plane residuals zero |
| `repeated-128.scene.json`                               | One definition/128 rigid occurrences; exactly one 832-byte mesh chunk; bounds approximately (0,0,0)..(600,300,8) mm                                             |
| `large-origin.scene.json`, plane origin (1e9,1e9,1e9+4) | Two definitions/three occurrences; same 12,720 unique mesh bytes; the three section areas above and zero residuals                                              |
| Original `through-hole.step`, z=4                       | One definition/occurrence; 9,584 mesh bytes; outer/hole loops yield material area 371.77106611442724 mm² and zero residuals                                     |
| Independent `occt-box-mm.step`, z=4                     | One definition/occurrence; 832-byte mesh; exact (0,0,0)..(20,10,8) bounds; area 200 mm² and zero residual                                                       |
| Independent `occt-cylinder-mm.step`                     | Nonzero CLI failure, typed InvalidGeometry: `solid is not a connected closed manifold`; no successful fallback or stdout completion                             |

Commands are in [development](development.md#real-engine-and-cli). Evaluation scenes are immutable original recipes, not project persistence or STEP assembly support. Approximate certified cylinder enclosures remain conservative; the physical radius-five circle is not confused with its ±5.5901699437494745 mm enclosure.

The real-child regression `multi_part_world_sections_and_large_origin_preserve_unique_meshes` initially failed because composition added a large world offset before subtracting the plane origin. This also perturbed canonical loop start ordering. Composition now rotates local samples and adds translation-minus-plane-origin directly, then validates/encodes those relative f64 points. The unchanged regression passed afterward, including every point, placed areas and same-session mesh-hash reuse. No tolerance was weakened or loop expectation re-pinned.

The actual shared client imported the original plate, read and validated all ten geometry frames (10,140,952 bytes, 1,030 faces), and then sampled the live engine's Linux `/proc/PID/status` lifetime `VmHWM`: **69,284 KiB**, below the 1 GiB gate. Import/job polling/full transfer took 24,081 ms in this single debug run. This is native-engine RSS, not parent/client/GPU memory or a cold/warm distribution. A separate actual CLI `geometry --source fixtures/geometry/perforated-plate.step --out NEW_DIRECTORY` completed with 242,281 vertices/270,348 triangles and all ten exclusive chunk files plus the final manifest after clean shutdown; owned proof files were removed.

Deep verification used one compiler job and sequential native processes: `cargo test --locked --workspace -- --test-threads=1` passed 98 tests (ten opt-in tests ignored); the real-engine client/CLI opt-in run then passed all ten. Coverage includes committed/staged lifetime accounting, stale publication, cancellation, watchdog termination, repeated definitions, large-origin placement, chunk transfer and output rollback. `generate_native_goldens --check` matched actual native single/multiple-chunk fixtures. The actual v2 CLI diagnostic smoke also passed handshake, Pong, explicit synthetic triangle decoding, mismatch rejection, non-UTF-8 paths and child reaping; synthetic diagnostics are not native geometry proof.

The current workflow YAML parsed and both material filters admitted all six algebraic/native mesh and section fixture paths; read-only permissions and the ten-minute deadline remain. Automatic checks now target contracts/shared-client and independent TypeScript decoders, not CAD engine/CLI or full workspace builds. Deep native evidence above is local; no hosted CI success is inferred. Thirty local references across six current public documents resolved.

Final workspace all-target Clippy passed with warnings denied after reclaiming the byte-identical temporary CEF library copy and keeping staged publication metadata inline rather than adding per-job boxes. Rust formatting, repository Prettier, smoke-script syntax and generated-contract drift checks passed. Existing ts-rs serde-attribute and proc-macro-error2 future-compiler warnings remain visible; they were not suppressed.

After the final inline stage/ACK and ownership-preserving rollback changes, the selected engine/shared-client/CLI suites passed 40 tests, then all ten opt-in real-engine tests passed again. Freshly rebuilt engine/CLI binaries passed the diagnostic smoke and actual two-part placed native section: two definitions/three occurrences, 12,720 unique mesh bytes, three loops and unchanged areas/residuals.

### Independent exporter support envelope

Frozen before numerical evaluation: original analytic geometry created and BRepCheck-validated through optional `cadquery-ocp==7.9.3.1`/OCCT 7.9 tooling, AP214IS, mm, fixed timestamp/name headers. The generator's byte-drift check passed for all three after adding validity checks; no frozen STEP bytes changed. This is tooling only, not another runtime kernel. Bindings: [Apache-2.0](https://github.com/CadQuery/OCP); OCCT: [LGPL-2.1 with exception](https://dev.opencascade.org/resources/licensing). Original outputs retain OSL attribution sidecars.

| Frozen STEP                 | Bytes  | SHA-256                                                            | Current native result                                                                                               |
| --------------------------- | ------ | ------------------------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------- |
| `occt-box-mm.step`          | 15,395 | `809bae57212f4c640016cafe3bfe4487f7f899326ee00e663774be6a673a1b82` | Import/mesh/native section pass; six planar source faces, 24 display vertices/12 triangles, 832 bytes; area 200 mm² |
| `occt-cylinder-mm.step`     | 5,649  | `21ea8ea395b4dd38d04dbe901d2538dd781f6567b404031148302164c85a8a9a` | InvalidGeometry: converted solid is not a connected closed manifold                                                 |
| `occt-through-hole-mm.step` | 19,015 | `71d0276a8ba240c608597baf0a079ecb6078f1bd88ebd61bff88abdaa3c25ff0` | Same converted-topology rejection                                                                                   |

The first probe rejected all three at schema admission. The real AP214 schema qualifier has four positive edition/version arcs, not the hand-authored three; the admitted standard qualifier now accepts variable positive edition arcs after the same AP prefix. The next probe exposed the standard PARAMETRIC_REPRESENTATION_CONTEXT metadata in already-supported pcurve representations; admitting that structural context made the box pass. Carrier/curve policy, uncertainty and tolerances did not widen. The cylinder and through-hole still fail native topology certification; generation validity is not native acceptance, and no healing, omission or replacement model is used.

### Native cross-language goldens

`generate_native_goldens` imported the actual original box and through-hole through the public facade, tessellated the box, computed the native hole z=4 cut and encoded complete loops with fixed fixture-only identities. Native mesh is 24 vertices/12 triangles/832 bytes; the 1,632-byte chunk cap separates the complete outer and hole loops in the multi-chunk fixture. Source hashes, residual ≤1e-4 mm, radius/chordal bounds, signed areas, winding and reordered/overlapping chunk rejection are independently checked by TypeScript.

All TypeScript consumers typechecked and 141 decoder tests passed. Decoder execution does not launch the kernel; generator and real CLI evidence above supply that distinct proof.

| Native fixture JSON              | SHA-256                                                            |
| -------------------------------- | ------------------------------------------------------------------ |
| `native-mesh.json`               | `3a6975b3aa2546916721ff3222df69015e33ed9f467c7f9d5d849d6c217ddb15` |
| `native-section.json`            | `62d1850c68b4bb9113a51ca91b483783e029e1f580d87f9216a19562712062dd` |
| `native-section-multichunk.json` | `5381e0d760c796074b37fe6f331646bee2341d1e866b0a68f32c8b22b1586cbf` |

Current v2 Cargo.lock SHA-256 is `83f31d382ed66063f1d1cb74c28ddfbf1d89d11f36d6b3c01b0f94991e347781`; pnpm-lock.yaml remains `d21980e498a9d3e5829519a28bd70f728245a151f1e3f723424fb591e9c5cf50`. Earlier environment/checks below retain their historical lock identity, not a claim that the v2 worktree is a packaged release.

### Actual CEF bridge and visual limit

The Linux development CEF executable compiled and the frontend typechecked/bundled. Verification used one compiler job, no incremental build, and debug info disabled after a debug-info build stalled at GTK4 under RAM pressure. Owned temporary build/exporter caches were reclaimed; compiler, native workloads and CEF runs did not overlap.

The actual geometry DOM smoke did not pass WebGPU admission: opt-in SwiftShader CEF GPU subprocesses exited with code 11. A separate native CEF window using the installed Mesa llvmpipe environment displayed the real unsupported workbench; its CDP GPU report showed Mesa 25.0.7, ANGLE OpenGL, `webgpu: disabled_off`, and no adapter. Executable-supplied Vulkan arguments were absent from the runtime's effective command line, so this is not claimed as a working Vulkan comparison. The real screenshot showed empty geometry, disabled imports/sections, “No WebGPU adapter is available” and zero owned display bytes. Before diagnostic bypass, native status was stopped with no Hello. No fallback, mocked GPU or successful presentation is claimed.

Main-world CDP/native invokes then deliberately bypassed that unavailable-GPU frontend gate, exercising the actual shell rather than mocked geometry:

- Startup negotiated v2/published fork identity; startup-fixed fixture admission minted active-session tokens for original box and cylinder. Accepted jobs published two native definitions; raw CEF ArrayBuffers of 832 and 11,888 bytes passed the independent TypeScript mesh decoder.
- Native face inspection returned box `step:18`, ADVANCED_FACE/source entity 18, native planar carrier/orientation, mm provenance and 1e-6 mm uncertainty.
- SetInstancePose and AddInstance produced two definitions/three correctly declared occurrences and 12,720 unchanged unique mesh bytes. A world z=4 section returned three native loops in one 5,024-byte raw chunk; independent decoding measured signed areas 199.99999999999997, 78.50861427961189 and 200.0000000000001 mm².
- An old FaceRef returned stale_revision. Independent OCCT cylinder import failed invalid_geometry while preserving the same live PID/session and revision-four committed scene.
- Actual plate job cancellation transitioned cancelling → cancelled before publication, preserving the complete scene. A fixture `../Cargo.toml` traversal was rejected by the native source gate.
- Restart changed PID and UUID and returned revision zero/no objects; old-session controls returned stale_revision. Stop returned stopped/no Hello. Closing the actual native window with another live child exited the CEF application with code zero; original, replaced and final engine PIDs were all observed reaped.

These are native bridge/decoder/job/lifecycle results, not DOM geometry presentation, picking, upload pause/resume or native dialog certification. Those remain unexercised without a working WebGPU presentation device and desktop portal backend. Packaging was not rebuilt or certified for this phase. The material geometry smoke runner now names unsupported admission directly and captures its real failure surface rather than waiting silently for an engine that cannot start.

The final actual `--scenario geometry` run under Xvfb, with explicit unsandboxed diagnostic opt-in and no forced GPU mock, reached that new unsupported branch in 1.89 seconds: exit one with the complete workbench diagnostic, protocol 2/2, no negotiated kernel, zero owned display bytes and `artifacts/geometry-geometry-unsupported.png`. The screenshot was inspected and the native application PID was observed reaped. This confirms failure reporting/teardown, not geometry presentation acceptance.

## Historical standalone execution environment

- Repository base commit: `97f9df1326519e5462dc605b3c46d366c4ef717f`; evidence describes working-tree changes, not a packaged release from that commit.
- OS: Linux `7.0.2-4-pve`, x86_64, container host `CT100`.
- CPU: Intel Core i7-10710U, six cores/twelve logical CPUs reported by `lscpu`.
- Memory: 8,489,271,296 bytes reported by `free -b`; this is host capacity, not measured peak engine RSS.
- Compiler: rustc `1.99.0 (b940084d7 2026-09-28)`, LLVM 23.1.1.
- Cargo.lock SHA-256: `53d0a0e32dfd7c12607137e3a36a9d46cb16d0c6b0fd5b2b55926840adfec0d2`.
- pnpm-lock.yaml SHA-256: `d21980e498a9d3e5829519a28bd70f728245a151f1e3f723424fb591e9c5cf50`.
- LSP: configured `rust-analyzer` could not start because the pinned official toolchain has no such binary. No existing exported contract symbols were renamed.
- This historical standalone run measured inspector RSS and single-run numerical timings only. The later v2 section above separately records actual engine RSS and geometry pipe transfer; no physical GPU/driver-memory or cold-cache numerical timing is claimed.

## Historical prerequisite verification

- `cargo run --locked -p spiling-contracts --bin generate` regenerated authoritative TypeScript and algebraic goldens; generator `--check` passed in the root checks.
- `pnpm check && pnpm test` passed: workspace Clippy with warnings denied, Rust formatting, all TypeScript consumers, repository formatting, 20 contract tests, eight engine child tests, four client tests, existing real CLI smoke and 137 TypeScript decoder tests.
- The actual CLI smoke negotiated protocol 1, returned Pong and the 64-byte synthetic diagnostic, rejected a protocol mismatch and exercised non-UTF-8 native paths/child cleanup. Its Hello had no geometry capabilities.
- A throwaway decoder invocation independently consumed Rust-generated payloads one chunk at a time: 240 mesh bytes, source faces `step:9007199254740993` and `step:42`, two 192-byte section chunks with signed algebraic areas 200 and -4 mm², and explicit SHA-corruption rejection. It did not exercise native sections or a geometry transport frame. The script was removed after the run.
- The finite-plane-normal regression failed before the normalization fix and passed afterward. Normalization now handles overflowing finite vector lengths and subnormal components by scaling before taking the norm.
- Both workflow material filters admitted mesh, section and multi-chunk fixture paths in a parsed YAML/filter smoke; read-only permissions and the ten-minute check deadline remained intact. Existing engine/CLI CI checks are retained until native integration; their planned removal is not claimed complete.
- ts-rs emits warnings about unsupported serde attributes (`transparent`, `deny_unknown_fields`). Checked Rust deserialization and independently validated TypeScript buffer admission remain enforced; warnings have not been suppressed.

Diagnostic fixture file SHA-256 (JSON container, not the separate payload hash):

| File                                        | SHA-256                                                            |
| ------------------------------------------- | ------------------------------------------------------------------ |
| `fixtures/protocol/mesh.json`               | `39d022b20c041c7d1929dba795a50ae8285bc29d2cc631e3375294c1fe923969` |
| `fixtures/protocol/section.json`            | `cd237dcbd095be647aa04dda6bff53f4aed015cfcd13a38608d662c5a4cdcb48` |
| `fixtures/protocol/section-multichunk.json` | `fe194123f11b8ec2e2dabad985aaf0e654c3c35b321e734861f653709c6c8374` |

## Acceptance boundaries

The algebraic packed-layout fixtures exercise interoperability and malformed-buffer rejection only. Separate native-mesh/native-section/native-section-multichunk goldens now derive from actual admitted box/through-hole geometry and independent numerical decoding. Neither family alone proves responsive transport or visible/pickable GPU presentation.

Original native corpus/source-face/carrier/section proof, bounded plate sizing, real v2 CLI scenes, large-origin sections, repeated-definition transfer and native golden decoding are exercised within the recorded envelope. At that frozen v2 gate, the desktop was memory-only and persistence/undo/manufacturing were unimplemented. Subsequent [v3 durable authoring evidence](authoring-evidence.md) and [v4 software-only manufacturing evidence](manufacturing-evidence.md) describe their separate source-backed authority and compiler/replay gates; neither retrospectively certifies this geometry milestone. Physical GPU/dialog/reference-platform and packaged geometry acceptance remain open, as does broader independent-exporter admission. General assemblies/freeform geometry, manufacturing UI and physical printer support remain unimplemented. Existing Triangle diagnostics remain explicit and are never geometry fallbacks.
