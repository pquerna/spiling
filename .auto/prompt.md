# Autoresearch: Spiling compile speed with supports on (real STEP corpus)

## Objective
Make supports-on compiles of real VoronUsers STEP parts much faster without changing what is
manufactured beyond declared tolerances or weakening any proof/verification. Work counters stay
uncapped (ADR009); slowness is fixed by algorithms, memoization, parallelism and CubeCL screening.

## Metrics
- Primary: `total_s` (sum of per-part engine+CLI CPU seconds via wait4 rusage; robust to concurrent benchmark sets over the benchmark set), lower is better.
- Secondary: `max_s` (slowest part), `ok` (parts compiling; must not drop).
Parts run concurrently, one engine each; the planner is single-threaded today, so per-part time is a
CPU-work proxy. Parallelism ideas are judged on per-part wall time with the set still concurrent.

## How to Run
`./.auto/measure.sh quick` (11 parts, ~5 min) prints `METRIC` lines; `slow` = 19 20 23 24 12
(tens of minutes, milestone only); `full` = all 30. Then `./.auto/checks.sh quick`.
Log with `./.auto/log.py keep|discard|crash|checks_failed "what" "why/insight"`.
Profile a part: `/tmp/sprof/prof.sh NN` then `fold.sh` + `tree.py` (see /tmp/sprof).

## Files in Scope
crates/manufacturing/src/planner/** (region.rs offsets, walls/**, support/**, boolean.rs, mod.rs,
tolerant.rs, classification/**), crates/deposition-routing/src/**, crates/compute/src/** (new CubeCL
kernels), crates/geometry/src/tolerant/**, crates/manufacturing/src/verifier/** (constant factors
only), Cargo.toml dependency additions (rayon is in the lockfile).

## Off Limits
Vendored sources (vendor/**), contracts/wire formats, PLANNER/VERIFIER identity bumps except when
output semantics change on purpose (bump once when shipping), the corpus harness semantics, tests'
intent (do not delete tests to pass; first-principles review if a test pins old slow behaviour).
Never `cargo fmt --all`.

## Constraints
- checks.sh must pass: same outcome class per part; ok parts' filament/volume within 1%, segments 5%,
  support filament 3%, identical layer count; manufacturing lib tests green (two known pre-existing
  failures excepted).
- Independent verifier semantics unchanged. No fixed work/time ceilings. No sampled shortcuts in
  proofs. Determinism: same input -> same program (parallel code must reduce in order).
- Simpler with equal speed = keep. Ugly complexity for <2% = discard.

## What's Been Tried
(see log.jsonl; summarize wins/dead ends here as they happen)
- Routing (autoresearch/routing): search-local sqrt pricing, coordinate-major vectorizable insertion and
  reversal scans, and second-price skipping (prices are non-negative + monotone rounding) took paired
  routing CPU 209.6 s -> 18.2 s (1.3% of compile) with bit-identical decisions to sqrt pricing. Route quality:
  the search is chaotic on big layers (per-layer travel swings +-25% from ulp changes), so compare travel over
  shuffled-input seeds (`ROUTE_SEED` in examples/route_replay.rs on SPILING_ROUTING_DUMP inputs), not one run.
  Dead ends: k-nearest neighbour lists for Or-opt (k=10..24) lose 1.6-3.3% travel because exhaustive
  insertion finds long-jump repairs; neighbour-limited 2-opt alone is -0.2%; squared-distance prefilter
  (no sqrt pass) is slower than the amortized sqrt pass; snapshot restore after kicks ~3-5% of routing only.
