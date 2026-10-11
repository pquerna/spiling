<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Native diagnostics, authoring and inspection consumer rules

- Use the shared EngineClient/EngineRpc exclusively: authenticated typed gRPC services, Google Operations names and Artifacts/ByteStream. No framed transport, JSON command tunnel, second scheduler, geometry/kernel dependency or alternate planner.
- Diagnose observes real readiness; triangle remains an explicit synthetic diagnostic. Job watches the existing durable diagnostic operation and fetches incremental immutable outputs. Optional --store retains operations across fresh runs; --request-id is the caller's AIP-155 UUID, never an automatic retry with a new identity.
- Geometry accepts a schema-version-1 evaluation recipe or repeated native --source paths, optional native --section and exclusive --out directory. Validate keys, references, poses and numeric inputs before spawning; import parts sequentially, then add instances. Recipes are evaluation inputs, not saved projects.
- Project create/edit/save/open/inspect/recover commands orchestrate typed project, geometry and manufacturing services. Ordered flags perform imports/shared placements/removal/undo/redo/checkpoints and --intent JSON_FILE/--compile/--verify/--manufacturing-out NEWDIR. Observe authoritative project and manufacturing status after every operation. Intent files are bounded inert JSON, never executed TypeScript; Rust service validation owns acceptance. Inspect/read-only permits replay/export but rejects intent mutation, compile publication and saves locally. Never access manifests/assets or implement domain persistence here.
- Filesystem operations preserve native OsString bytes/code units; only bounded JSON display labels are lossy UTF-8. The default engine is beside the CLI. Commands start private fresh child processes, including reopen; retained operation storage does not restore native session handles.
- Always fetch each unique mesh/section chunk through its returned immutable resource descriptor and ByteStream, even without --out. Validate hash, size, packed layout, session/artifact/revision identity and complete ordered manifests through borrowed contract views. Keep one payload live; enforce pagination/count/identity budgets. Report native f64 per-occurrence areas and placements, never geometry manufactured from display triangles.
- Native operation observation polls standard Operations by name every 100 ms without a second watchdog. Match operation name/session and classify typed geometry/project/manufacturing errors. On recoverable manufacturing errors reconcile authoritative project/intent/artifact status before cleanup.
- On recoverable save failure query authoritative project status without retry. Preserve dirty/save_uncertain evidence: a committed-but-unsynced checkpoint is not a confirmed save or assumed rollback. No success stdout or completion manifest on save error.
- --out and --manufacturing-out admit previously nonexistent directories before spawn, use exclusive files and roll back only their own handles/inodes on failure, without recursive deletion or following substituted symlinks. Never overwrite/delete foreign replacements. Completion manifests and final stdout follow successful observed child reap; output failure still rolls back owned files.
- Manufacturing export independently replays through the engine and retrieves the descriptor-bound, size/hash/input/schema/summary-validated bundle through EngineClient. Export plan.json/program.gcode/verification.json/provenance.json; verification.json is the exact fresh replay report, not the persisted flag. Identify exported content even if later ordered edits invalidate the active artifact. Every manufacturing report states software_only and not_machine_ready; no physical readiness or printability claims.
- Create/edit checkpoint final state; open does not implicitly save. Save preserves undo/redo history. Fresh inspection reopens engine-owned source assets after originals are removed.
- Exercise node --import tsx tools/smoke-cli.mjs against built binaries and SPILING_TEST_ENGINE=PATH cargo test --locked -p spiling-cli -- --ignored --test-threads=1. Actual-child regressions cover source deletion/reopen, ordered checkpoints and history, transfer/sections/faces, failure/reap/rollback, fresh manufacturing replay/export and non-UTF8 paths. Parent integration verification runs these sequentially under its resource limits; do not claim unexecuted checks.

Software-only operator workflow:

```sh
spiling-cli project create PROJECT --import fixtures/geometry/box-mm.step --intent fixtures/manufacturing/solid-fill.intent.json --compile --verify --manufacturing-out NEW_EXPORT --engine ENGINE
spiling-cli project inspect PROJECT --verify --manufacturing-out NEW_REOPEN_EXPORT --engine ENGINE
```

See [development](../../docs/quality/development.md#real-engine-and-cli), [control protocol](../../docs/protocol/control.md) and [project format](../../docs/protocol/project.md).
