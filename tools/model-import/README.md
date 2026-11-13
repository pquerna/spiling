# Import acceptance

`check.py BIN_DIR NEW_WORK_DIR [REAL_STL]` exercises the production engine/CLI:
import, sections, compile/save, removal of the owned external source, read-only
return, fresh verification and export. It retains findings and checks scoped
negative volumes with an independent commanded-void observer. Inputs generated
by `fixtures.py` are original fixtures; private corpus sources stay private.

The native workbench accepts `--import SOURCE` through its ordinary import
action. `--open PROJECT.spil` opens saved work; `--read-only` applies to that
project action. Import never applies a foreign printer profile or script.

Use the existing actual-window harness for raw source loading:

```sh
node tools/bench-open.mjs --source part.stl --user-data EXISTING_NATIVE_DATA \
  --output NEW_DIRECTORY --bin-dir target/debug --samples 3 --warmups 1
```

It requires Linux X11, existing license-assented native preferences and adjacent
coherent workbench/engine binaries. Run native workloads sequentially under the
host's memory/task supervision. The harness does not build, alter the source or
accept the license. Each sample starts a new process and retains observed build,
kernel, adapter, source/binary hashes, phase durations, separate process peak RSS,
readback milestones and a screenshot. Readback is rendered pixels, not compositor
presentation; the graphics line distinguishes CPU rendering from a hardware GPU.
The existing `--project` cohort remains a separate saved-open measurement.

For a checked open/nonmanifold source, add `--interaction failure`. This uses
opt-in rendered-control coordinates and normal X11 pointer events to inspect the
rejected source, select its issue and explicitly Locate it. It checks a failed
import, actual diagnostic drawing, selection/Locate receipts and orderly child
cleanup. Automation never inserts a project part or calls a hidden repair action.

Unresolved-source previews are checked, leased session display artifacts. Their
facet coordinates reflect the normalization attempt, and undrawable zero-area
facets are omitted from display only. They have no engine material definition,
selectable manufacturing faces, project occurrence or slicing capability. Known
issue bounds mark an area, not a claimed defective face. Close releases the lease
and cancels an active transfer; replacement, project mutation and restart expire
it. Failed-attempt previews are not saved in the project. Their immutable chunks
use the existing Operations/ByteStream audit store and its storage/retirement
policy; the live scene handle expires independently of those diagnostic bytes. Adopted import findings
remain persistent provenance and survive source-absent return.

The exact boolmesh dependency is retained under its own license in
`vendor/boolmesh`. Its scoped cancellation checkpoints interrupt operand building,
Boolean work, triangulation and cleanup; ordinary geometric predicates are
unchanged. Cancellation requires unwind builds and its scoped API deliberately
excludes the dependency's optional rayon feature until worker propagation is
implemented. Cancellation is not a timeout or a source-size validity rule.

The ignored workbench test `cancellation_observes_terminal_outcome_before_another_mutation`
accepts `SPILING_TEST_IMPORT_SOURCE` for a real large-source cancellation witness,
in addition to `SPILING_TEST_ENGINE`. It observes normalization entry, cancels,
waits for the worker-authoritative terminal outcome and imports another part in
the same session. It reports latency without a machine-specific assertion. The
Boolean dependency's cancellation tests synchronize inside actual Boolean work.

`inspect_model_import` observes normalization, packed retained return and a
mid-height section through the geometry facade. It prints separate phase times
and the complete packed-source SHA-256. `--display` additionally generates the
production display chunks and reports their hashes, bounds and separate time.
Generate the frozen cylinder with `fixtures.py NEW_DIR --scale`; the separate
`--torus` workload generates a million-facet curved through-hole holdout.
`total_seconds` ends after sectioning; observer hashing and optional display are
outside that number. These phase timings are not native import-to-paint latency.

For algorithm experiments, freeze an observer executable before the edit, rebuild
with the same profile/features, and alternate both against the same source set:

```sh
python3 -B tools/model-import/bench.py --baseline BEFORE_INSPECTOR \
  --candidate AFTER_INSPECTOR --display --output NEW_DIRECTORY --repeats 3 \
  generated/large-cylinder.stl generated/crack.stl generated/nested.stl
```

The comparator warms each executable once, alternates measured execution order,
retains every run and source/binary hash, and requires identical complete retained
bytes, findings, midpoint section and optional display chunks. Do not relax this
judge to hide nondeterministic inputs: Boolean composition can reorder a retained
complex even in repeated baseline runs. Use stable fixtures for this byte-exact
performance comparison and keep Boolean material/void acceptance in `check.py`.
Always finish with the complete actual-window benchmark and real import journeys.
