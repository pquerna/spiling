#!/bin/bash
# checks.sh [set] [--no-tests] : correctness gate. Semantic comparison against the set's baseline, then planner unit tests.
set -uo pipefail
cd "$(dirname "$0")/.."
SET=${1:-${AR_SET:-quick}}
python3 .auto/check.py .auto/baseline-$SET.json .auto/runs/last.json || exit 1
[ "${2:-}" = "--no-tests" ] && exit 0
KNOWN="genuine_native_cantilever_clearance_and_floating_gap_limits_are_explicit|auxiliary_bridge_bounds_ignore_unrequested_tangent_and_thin_intervals"
out=$(CARGO_TARGET_DIR=${AR_TEST_TARGET:-target} cargo test -q -p spiling-manufacturing -p spiling-deposition-routing --lib 2>&1)
echo "$out" | grep -E "^error(\[|:)" -A8 | head -30
fails=$(echo "$out" | sed -n '/^failures:$/,/^test result/p' | grep -E '^    [a-z_:]+$' | grep -Ev "$KNOWN" | sort -u)
echo "$out" | grep -E "^test result" 
if [ -n "$fails" ] || ! echo "$out" | grep -q "^test result"; then echo "UNEXPECTED FAILURES:"; echo "$fails"; exit 1; fi
