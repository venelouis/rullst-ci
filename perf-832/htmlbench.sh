#!/usr/bin/env bash
# perf-832/htmlbench (the phase-1 microbenchmark: /fortunes rendered without HTTP or a database, counting allocator
# calls) built once per variant of perf-832/crates.sh in the Benchmarks' pinned Rust image, then run on one pinned
# CPU, the order rotated every run. Usage: htmlbench.sh OUT_DIR
set -euo pipefail
out="$1"
here="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"
mkdir -p "$out"
image=rust:1.99.1-slim-bookworm@sha256:f2c004e3d2d8b547e8cf7ffbc29205385f4e71d23e53e4e383065db5e9f822cd
variants=(rullst rullst-html1 rullst-html2 rullst-html2-cap)
for variant in "${variants[@]}"; do
  dir="$work/$variant"
  cp -r "$here/htmlbench" "$dir"
  "$here/crates.sh" "$variant" "$dir"
  if [ "$variant" != rullst ]; then
    printf '\n[patch.crates-io]\nrullst-macros = { path = "crates/rullst-macros" }\n' >> "$dir/Cargo.toml"
    if [ -d "$dir/crates/rullst-core" ]; then
      printf 'rullst-core = { path = "crates/rullst-core" }\n' >> "$dir/Cargo.toml"
    fi
  fi
  docker run --rm -v "$dir:/src" -w /src -e CARGO_TARGET_DIR=/src/target "$image" \
    cargo build --release 2>&1 | tail -3
  cp "$dir/Cargo.lock" "$out/$variant.Cargo.lock"
done
{ nproc; lscpu | grep -i "model name"; docker run --rm "$image" cargo --version; } | tee "$out/host.txt"
for run in 1 2 3 4 5; do
  shift_by=$(( (run - 1) % ${#variants[@]} ))
  order=("${variants[@]:shift_by}" "${variants[@]:0:shift_by}")
  for variant in "${order[@]}"; do
    docker run --rm --cpuset-cpus 0 -v "$work/$variant:/src" -w /src "$image" /src/target/release/htmlbench \
      | grep -v '^html!   \|^askama  ' | sed "s/^/run $run $variant: /" | tee -a "$out/htmlbench.txt"
  done
done
