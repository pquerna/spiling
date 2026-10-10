<!--
SPDX-FileCopyrightText: 2026 Spiling contributors
SPDX-License-Identifier: OSL-3.0
Licensed under the Open Software License version 3.0
-->

# Native authoring and inspection consumer rules

- Use EngineClient for all engine operations; do not implement a second handshake/transport or geometry path.
- Geometry accepts a schema-version-1 evaluation recipe or repeated native --source paths, optional native --section and exclusive --out directory. Validate keys, references, poses and numeric inputs before spawning; import parts sequentially, then add instances. Recipes are evaluation inputs, not saved projects.
- Project create/edit/save/open/inspect/recover commands use EngineClient::project, geometry authoring and manufacturing services only. Ordered flags perform imports/shared placements/removal/undo/redo/checkpoints and --intent JSON_FILE/--compile/--verify/--manufacturing-out NEWDIR; observe authoritative project and manufacturing status after each operation. Preserve bounded native OsString paths. Intent files are bounded inert JSON, never executed TypeScript; Rust service validation owns acceptance. Inspect/read-only permits replay/export but rejects intent mutation, compile publication and saves locally. No direct manifest/asset/domain implementation, alternate planner or recipe-to-project persistence substitution.
- Diagnose/triangle remain explicit synthetic diagnostics. stdout reports machine-readable JSON only after successful shutdown; errors are stderr JSON and nonzero exit. Default engine resolution is beside the CLI. Actual filesystem operations preserve native bytes/code units; only display labels are lossy UTF-8.
- Always fetch and hash/layout-validate each unique mesh/section chunk through borrowed contract views, even without --out. Keep only one payload live, enforce pagination/count/identity budgets, and report native f64 per-occurrence loop areas and placements; do not manufacture geometry from display triangles.
- Poll shared engine jobs every 100 ms without imposing another watchdog. Tagged geometry/project/manufacturing failures are classified in stderr JSON; recoverable manufacturing failures reconcile authoritative project/intent/artifact status before cleanup. Command failure still terminates/reaps its private child. No geometry/kernel dependency or alternate transport state machine.
- After a recoverable save failure, query authoritative project status before cleanup and include it in stderr JSON. A committed-but-unsynced checkpoint is dirty/save_uncertain, not a confirmed save or assumed rollback. Never retry automatically; no success stdout or completion manifest on save error.
- --out and --manufacturing-out each admit a previously nonexistent directory before spawn, use create_new files, write their completion manifest only after successful child shutdown, and roll back only their own handles/inodes on failure without recursive deletion or following substituted symlinks. Existing output and foreign replacements are never overwritten/deleted. Manufacturing export replays through the engine, fetches a size/hash/schema-validated bundle through EngineClient, and writes plan.json/program.gcode/verification.json/provenance.json. Reports identify the exported artifact even if later ordered edits invalidate the active artifact. Every manufacturing report clearly states software_only and not_machine_ready; no physical readiness/printability/firmware/collision certification.
- Exercise `spiling-cli geometry --scene fixtures/geometry/scenes/two-parts.scene.json --section 0,0,4:0,0,1 --engine PATH`, project workflows in [development.md](../../docs/quality/development.md#real-engine-and-cli), diagnostics and protocol-version mismatches with 1/current+1. Deep regressions: `SPILING_TEST_ENGINE=PATH cargo test --locked -p spiling-cli -- --ignored --test-threads=1`. See [control protocol](../../docs/protocol/control.md).

Software-only operator workflow (use a built engine and an inert intent compiled from trusted TypeScript by the root authoring tool):

```sh
spiling-cli project create PROJECT --import fixtures/geometry/box-mm.step --intent fixtures/manufacturing/solid-fill.intent.json --compile --verify --manufacturing-out NEW_EXPORT --engine ENGINE
spiling-cli project inspect PROJECT --verify --manufacturing-out NEW_REOPEN_EXPORT --engine ENGINE
```

Create/edit save their final state; open executes the same ordered operations without an implicit checkpoint. Inspect exposes persisted intent and artifact summary; explicit --verify independently replays the emitted program rather than trusting the persisted verified flag.
