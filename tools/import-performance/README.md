# Import and saved-return performance

This paired benchmark drives the actual supervised CLI: import STEP, apply the
same explicit intent, center on bed, compile/save, delete the source, then reopen
read-only, freshly verify and export. It does not replace the complete corpus gate
or infer printability from successful output.

Freeze matching engine/CLI binaries for both producers outside the build target.
Record their source revision, toolchain, build command/flags and dependency pins
beside them; the runner additionally records exact executable and input hashes.
Use the same profiling build and resource envelope for both. Frame-pointer builds
allow `perf record --call-graph fp` without sampled-stack copying.

```sh
python3 tools/import-performance/measure.py \
  --corpus /path/to/corpus/step \
  --baseline /path/to/frozen-baseline \
  --candidate /path/to/frozen-candidate \
  --intent fixtures/manufacturing/complete-sparse.intent.json \
  --output /path/on/persistent/disk/new-run
```

The default quick set is 28/25 with three paired repeats and alternating producer
order. Use `--parts 28 25 27 23 13` for the held-out panel, and separately run
`--supports breakaway`. A new output directory is mandatory. Corpus STEP and
generated projects retain source licensing and must not be committed.

Primary `METRIC cpu_seconds_ratio` is the geometric mean of paired compile and
return process-CPU ratios. Samples run serially; CPU includes the CLI and its
reaped supervised-engine descendants. Whole-journey wall time and sampled engine
high-water RSS remain separate secondary observations. A concurrent host load can
still distort both and must be reported. Failed compile/return is a failed run,
not an attractive short timing. Inspect per-part/stage results, not only the mean.

Successful project/export scratch is removed unless `--retain-work` is selected;
source hashes, output hashes, logs and measurements remain. Failed work is retained.
Cancellation terminates the owned CLI; its engine owner-liveness mechanism owns
child cleanup. An external campaign deadline is not a product runtime limit.

For an autoresearch loop, retain a prompt, hypotheses, correctness command and
append-only experiment log in the run's owned scratch. Establish a baseline;
change one mechanism; rebuild/freeze the candidate; measure pairs; inspect
correctness and secondary metrics; keep or reject the change. Do not adopt the
upstream tool's automatic blanket revert/commit behavior in a shared checkout.
Remeasure the accepted producer and complete the ordinary geometry, engine,
manufacturing and full supports-off/on corpus checks before integration.

The optional geometry-owned display probe reports per-definition direct versus
native-polished face counts and the largest accepted sampled carrier/direct and
trim deviations, while checking the packed buffers:

```sh
SPILING_PREVIEW_STEP=/path/to/part.step RUST_MIN_STACK=8388608 \
  cargo test --locked --profile profiling -p spiling-geometry --lib \
  preview_external_step_reports_measurement_paths -- --ignored --nocapture
```

Retain the source hash and producer with this diagnostic. It exercises the
display owner, not engine publication, desktop rendering or manufacturing;
the supervised journeys and complete corpus remain separate acceptance checks.
