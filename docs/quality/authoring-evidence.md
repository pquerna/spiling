<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Recoverable source-backed authoring evidence

This is frozen protocol-v3 / project-format-1 authoring evidence. Its source fingerprints, measurements and exclusions describe that evaluation, not current protocol-v4 / format-2 support. Current software-only manufacturing and durable bundle proof is recorded separately in [manufacturing evidence](manufacturing-evidence.md); historical measurements below are not re-pinned.

## Accepted implementation boundary

This wave establishes a reusable native project authority, not a saved viewer recipe or a manufacturing product. `spiling-core` owns source-backed transactions, persistent identity/revision/allocation, semantic dirty comparison, bounded session history and checkpoint storage. The engine composes that authority with disposable native geometry/display caches. CLI and the thin desktop are clients of the same protocol-v3 services; neither implements a second project writer.

Implemented operations are import, shared rigid occurrence creation/placement/removal, new/open/save, undo/redo, read-only inspection and explicit previous-checkpoint recovery. Exact original source assets and persistent references survive reopening. Native BREP and meshes are reconstructed and admitted independently. Packed SPLT/SPLM/SPLS remain schema 1. Control protocol 1/2 and incompatible project formats are rejected, not adapted.

General CAD/topology edits, autosave of arbitrary unsaved work, restart-persistent undo, manufacturing, machine export, alternate kernels and packaged/physical WebGPU certification are outside this implementation. The complete recoverable-authoring/platform milestone is **not accepted** from the Linux proof below.

Canonical contracts: [control](../protocol/control.md), [project format/durability](../protocol/project.md), [packed display](../protocol/display.md). Existing numerical support and independent-exporter failures remain governed by [geometry evidence](geometry-evidence.md).

## Plan and self-review decisions

1. Introduce actual core/state/storage ownership; remove the competing authoritative dispatcher scene model. Persist original bytes and shared occurrence records, not native objects, meshes, paths or session handles.
2. Separate persistent ProjectRevision from ephemeral SceneRevision. Undo/recovery never rewind identity allocation; reopening invalidates old display/face/job references. Dirty compares content, so undo-to-saved can be clean at a later revision.
3. Checkpoint an immutable snapshot with assets first, synchronized temporary records, one validated previous checkpoint, atomic publication and directory synchronization. Use a stable held OS writer-lock inode, never unlink/recreate it.
4. Keep old project/native ownership through staged open and ACK. Enforce the combined old/history/incoming source budget **before capture allocation**, including duplicate definition IDs. Refuse exhaustion rather than discard the old state early.
5. Linearize cancellation against the actual commit gate. Before publication it may win; afterward report the published receipt and durability outcome, never false cancellation/rollback/success. Save preserves session undo.
6. Make first saves exclusive and retryable before publication; cleanup only verified owned staging entries. Concurrent readers capture one complete committed manifest; immutable assets are not eagerly garbage-collected.
7. Recovery is explicit and independently validated. Its previous-content envelope records candidate revision/allocator high-water, preventing reuse of identities from a newer published checkpoint. Normal open never silently falls back.
8. A failed postcommit directory sync attaches actual storage but remains dirty/uncertain with typed IO. The failed job's publication receipt settles its pending native route; an uncertainty flag left by an earlier save cannot settle a later failed Save As.
9. Migrate generated contracts, engine/client/CLI/shell/frontend, tests and local ownership documents as one clean cutover. Verify actual processes and native CEF invokes, not mocks or a substituted browser/viewport.

## Frozen evaluation inputs

Evaluation uses the working-tree implementation after source baseline `97f9df1326519e5462dc605b3c46d366c4ef717f`; it is not a claim that the baseline commit contains these changes. Source fingerprints below identify the evaluated authority/publication code. Original fixtures are redistributable Spiling corpus, not private customer CAD.

- Linux `7.0.2-4-pve`, x86-64, Intel i7-10710U, 6 cores/12 threads, about 8 GiB RAM; 512 MiB swap substantially occupied.
- Rust `1.99.0`; Node `24.15.0`; pnpm `10.32.1`; CEF `152.0.6` Linux x86-64. All Monstertruck dependencies use public fork commit `d87b4d9ced1f3baf31aa771ac0e7c663efb1c001`, version `0.4.1`.
- Single Cargo job, incremental disabled, dev/test debug info disabled, serial Rust tests. Established Cargo home `/tmp/spiling-cargo-git-d87b4d9c`, target `/tmp/spiling-native-probe-target`, CEF root `/root/spiling/.cache/cef`. Native builds/tests and CEF runs were sequential, not concurrent memory workloads.
- Ordinary isolated Rust/CLI/CEF project smoke uses `/tmp` (tmpfs). The injected postpublication directory-sync failure/reopen smoke used an exclusively owned project on `/root/spiling/artifacts` (observed ext4).
- Actual CEF ran under Xvfb with explicit diagnostic-only sandbox disable and privileged loopback debugging. No usable WebGPU adapter was available. This is neither sandbox certification nor physical hardware acceptance.

| Input                               | SHA-256                                                            |
| ----------------------------------- | ------------------------------------------------------------------ |
| Cargo.lock                          | `7361e2fb6183b3b0fd2ee09138ee26d8006fe2be7e6cbae45ebf764603e6dca0` |
| pnpm-lock.yaml                      | `d21980e498a9d3e5829519a28bd70f728245a151f1e3f723424fb591e9c5cf50` |
| box-mm.step                         | `1b23042e12183748e12bc4e0195ae546c80296ce4170709eb3f836dce2f4ac98` |
| cylinder.step                       | `95df1da444dc9c49529054f1825c9013c135301df677e30269c3a789ee2b464b` |
| core/src/lib.rs                     | `f3a9f53ad0e587bfa0868032cc6d9af08315eeb3920492cf6ad3d14300c6e602` |
| core/src/storage/mod.rs             | `7b99e1ea2538c565919fa30264f2dee67fba9ea7dec842ec48bc94e9f3a7b94d` |
| core/src/storage/os.rs              | `369cb43bcbb8428f0468593e1abb2447b24b2e1e2977de9fe9e77f725abb82e4` |
| engine/src/geometry/mod.rs          | `47070ecf2cda2de849fee976f3888c79a555d6239e071c4c880debb51461a9f0` |
| engine/src/geometry/worker.rs       | `b4f2b62f765e2f2e8445150d569b619b73b6d3a3e1c9697f960210b90cb2f6b3` |
| desktop/src-tauri/src/engine/mod.rs | `8207bbc4563df98f019a84d4be36450e812263e48990124a2c71652c4a646028` |

Source rows are relative to `crates/` or `apps/` as named. Frozen bounds are 32 active/history definitions, 256 occurrences, 64 undo/redo transactions, 16 MiB per source and 64 MiB combined retained/staged captured source bytes. Persistent storage is bounded to 128 immutable assets/256 MiB, 1 MiB records and separately bounded temporary entries. These are explicit admission limits, not measured total-process/GPU memory promises.

## Observed native operator workflow

A real CLI sequence used three separate supervised engine processes:

1. Create a new project; import copied original box and cylinder bytes; save.
2. Reopen writer; place cylinder at `[40,0,0]`; add a shared box at `[80,0,0]` with a quarter-turn about Z; place the original box at `[10,20,0]`; undo/redo; save; mutate again and undo to the saved content; save again. Request native per-occurrence sections at `z=4` and inspect box source face `step:18`.
3. Remove both original external source paths; open a fresh read-only engine from immutable project assets; compare project/definition/occurrence IDs, poses, provenance and source-face pages with the prior process.

Observed persistent project revision 9; undo-to-saved at revision 9 compared clean against checkpoint revision 7, with both undo and redo still available. Final save confirmed revision 9 without clearing either history direction. Fresh-process reopen retained project/content identities, used a new session and had no session undo history.

The reopened scene contained two definitions and three occurrences, with **12,720 unique packed mesh bytes**, not a duplicate mesh per shared occurrence. Native section areas were `200`, `78.5086142796119`, and `200.00000000000003 mm²`; reported maximum plane residual was zero for all three. Persistent face identity/provenance survived source deletion. These use the frozen native geometry/section support envelope, not universal cross-platform bitwise equality.

Actual-child tests additionally exercised second-writer rejection, simultaneous committed read-only inspection, rejection of all read-only mutations/save/history operations, switching away from read-only, killed-writer lock release, stale references, failed/cancelled open preservation, and invalid schema/identity/reference/pose/missing/corrupt/nonregular/symlink/native-admission failures.

## Crash and uncertain-save proof

Actual engine `SPILING_PROJECT_FAULT=after_assets|before_manifest_replace|after_manifest_replace` exits with code 86 at real storage transitions. Tests exercised attached saves and exclusive initial publication: before publication current remained a complete prior snapshot (or first target absent); after publication it was a complete new snapshot with intact source references. Explicit recovery loaded independently validated previous content with fresh revision and preserved allocator high-water. Foreign destinations and substituted staging entries were not overwritten/deleted. These are **process-crash** tests, not power-loss certification.

A separate throwaway Linux syscall interposer exercised actual CEF → native bridge → engine → ext4 storage. It returned `EIO` exactly once from the destination parent's `fsync`, only after `published/manifest.json` existed following exclusive initial publication. No project/store implementation was mocked:

- Job 2 reported `failed`, typed project `io`, and a job-specific `project_saved` publication result. Info was dirty/uncertain, attached to the real destination, and had no confirmed saved revision.
- A new Save As token was admitted while its destination did not exist; an owned foreign directory/file was then created before submission. Job 3 failed before commit with `invalid_project`, **no publication result**, and unchanged sticky uncertainty/current attachment.
- Remove the original STEP path; explicitly restart/reopen. The shell reopened the original published checkpoint, **not** the later rejected foreign route, with the same project identity and one native occurrence. Foreign bytes remained unchanged.
- Initial writer PID `1930753` and final writer PID `1930769` were observed reaped; native window close exited cleanly. Interposer, harness, shared library and the exclusive test directory were removed afterward.

Reproduce this failure procedure by intercepting Linux `fsync(int fd)` only for the exclusively owned project parent directory (verify via `fstat`/`/proc/self/fd`), checking that its final child manifest exists, then returning `-1/EIO` once. Compile the throwaway interceptor with `cc -shared -fPIC -Wall -Wextra -Werror -ldl -pthread` and pass it through explicit `LD_PRELOAD` only to the diagnostic CEF process/sidecar. Use startup-confined fixture tokens and the operation sequence above; never enable such injection in normal operation. Deterministic permanent core regressions cover postcommit uncertainty, confirmed-baseline preservation and same-session reopen; the real syscall/route proof is separately qualified.

## Actual desktop surface and transfer

`pnpm --filter @spiling/desktop build` regenerated the embedded frontend before the CEF native executable was rebuilt. A raw Cargo desktop build alone can otherwise embed an older existing `dist`; the first diagnostic exposed old protocol-v2 assets, and they were rebuilt rather than accepted as current UI proof. Smoke waits for mounted React admission, not only `document.readyState`.

`pnpm smoke:desktop --executable ... --scenario project-bridge` passed in **2.10 seconds** with warm executable/resources. It captured actual project controls and the honest unsupported-WebGPU surface, then deliberately bypassed GPU admission only for qualified real native/storage invokes. Two source definitions/three placed shared occurrences were saved; pose mutation then undo was clean; originals were removed; restart reaped the prior writer; reopen preserved identities; **12,720 raw binary mesh bytes** were independently decoded; `step:18` inspection and read-only rejection succeeded. Native window close reaped the final engine.

The ext4/real-fsync-error CEF procedure passed in **1.42 seconds** with warm executable/resources. These are smoke wall times, not frozen cold/warm performance gates. No new total engine/renderer/GPU peak-memory or cold-start certification is claimed. Previous measured geometry scale/transfer evidence remains separate.

Ignored local screenshot: `artifacts/project-native-bridge-surface.png`. It shows actual new/open/save/undo/redo/recovery/read-only controls, protocol `3 / 3`, no negotiated engine before GPU admission, and “WebGPU unavailable”; it is not proof of visible native objects, picking, interactive authoring or real folder dialogs. Native bridge invocation is not a substitute for those remaining hardware/workflow gates.

## Exercised verification commands

Use the environment pins above and actual configured target paths:

```sh
cargo build --locked -p spiling-engine -p spiling-cli -p spiling-desktop
cargo clippy --locked --workspace --all-targets -- -D warnings
SPILING_TEST_ENGINE="$CARGO_TARGET_DIR/debug/spiling-engine" cargo test --locked --workspace -- --include-ignored --test-threads=1
cargo test --locked -p spiling-core -- --test-threads=1
cargo test --locked -p spiling-engine --test project -- --test-threads=1
cargo test --locked -p spiling-desktop project_fixture_admission_confines_open_and_new_save_targets -- --test-threads=1
cargo test --locked -p spiling-cli --test project -- --include-ignored --test-threads=1
cargo run --locked -p spiling-contracts --bin generate -- --check
pnpm -r check
pnpm --filter @spiling/protocol test
pnpm --filter @spiling/desktop build
SPILING_CEF_UNSANDBOXED=1 SPILING_ENGINE_PATH="$CARGO_TARGET_DIR/debug/spiling-engine" LD_LIBRARY_PATH="$CARGO_TARGET_DIR/debug" xvfb-run -a pnpm smoke:desktop --executable "$CARGO_TARGET_DIR/debug/spiling-desktop" --scenario project-bridge
```

Final complete Rust workspace: **157 passed, 18 suites**, including opt-in actual processes and all four native desktop admission tests; command wall time 305.30 seconds, not a performance benchmark. The final core suite contained 29 tests and actual-engine project suite nine. Complete-workspace Clippy passed without suppressing source warnings. Existing ts-rs serde-attribute warnings and upstream proc-macro-error2 future-compiler warning remain visible.

All three TypeScript consumers passed type checking; the independent protocol decoder suite passed **141 tests**, with no skipped tests. Generated contracts passed drift checking. After relocating the substantial CLI workflow into its owning directory/local rule pair, the complete-workspace Clippy check and all three actual-process CLI project tests passed again.

## Remaining support gates

- Physical WebGPU rendering/picking/interactive authoring/dialog/dirty-close acceptance and packaged workflow on declared machines are unverified here; no GPU fallback or visual-success claim was introduced.
- macOS/Windows storage primitives are implemented but unverified; directory-flush refusal must fail explicitly, including published-but-uncertain outcomes. No filesystem/device power-loss, network/cloud-synchronized filesystem or hostile mutation certification.
- Save/recovery does not replay arbitrary unsaved edits or persist undo. First-save crashes can leave unpublished exclusive staging siblings; no unsafe automatic scavenging is claimed.
- Storage exhaustion is explicit; no eager asset GC while readers may hold older snapshots.
- Independent OCCT cylinder/through-hole import rejections remain recorded geometry exclusions. This authoring wave neither changes the kernel nor relaxes the declared STEP envelope.
- Manufacturing requirements/planning/emitted-program verification/physical prints remain unimplemented, not supported by a source-backed project schema.
