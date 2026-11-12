#!/bin/bash
# measure.sh [quick|full|slow] : build profiling binaries, run the benchmark part set, print METRIC lines.
set -euo pipefail
cd "$(dirname "$0")/.."
SET=${1:-${AR_SET:-quick}}
case $SET in
  quick) PARTS="02 05 08 09 11 14 22 29 13 15 03" ;;
  slow)  PARTS="19 20 23 24 12" ;;
  full)  PARTS="$(ls .cache/realistic-corpus/step | cut -c1-2 | tr '\n' ' ')" ;;
esac
cargo build -q --profile profiling -p spiling-cli -p spiling-engine 2>&1 | grep -E "^error" -A5 || true
test -x target/profiling/spiling-engine
mkdir -p .auto/runs
OUT=.auto/runs/$(date +%H%M%S)-$(git rev-parse --short HEAD)-$SET.json
# Paired A/B: `./.auto/measure.sh quick base` snapshots the current build as the baseline binaries;
# afterwards every run executes them concurrently with the experiment (ratio < 1 = faster).
if [ "${2:-}" = "base" ]; then mkdir -p .auto/bin-base && cp target/profiling/spiling-cli target/profiling/spiling-engine .auto/bin-base/; fi
if [ -x .auto/bin-base/spiling-engine ] && [ "${2:-}" != "base" ]; then export AR_BASE_BIN=$PWD/.auto/bin-base; fi
python3 .auto/bench.py "$OUT" $PARTS
ln -sf "$(basename $OUT)" .auto/runs/last.json
