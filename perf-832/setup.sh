#!/usr/bin/env bash
# Shared job setup: the harness's wrk (built from its SHA-256-pinned source), Docker images from the build jobs.
# Usage: setup.sh [oha]
set -euo pipefail
: "${BENCH:?}"
mkdir -p "$RUNNER_TEMP/bin"
echo "WRK=$("$BENCH/bench/install-loadgen.sh" "$RUNNER_TEMP/bin")" >> "$GITHUB_ENV"
if [ "${1:-}" = oha ]; then
  echo "OHA=$("$BENCH/bench/install-loadgen.sh" "$RUNNER_TEMP/bin" oha)" >> "$GITHUB_ENV"
fi
shopt -s nullglob
for tarball in images/*.tar.gz; do
  gunzip -c "$tarball" | docker load
done
docker images
nproc
lscpu | head -20
