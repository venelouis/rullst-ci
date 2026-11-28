#!/usr/bin/env bash
# Compile time, code size and output of Rullst's website generator (site/, 402 html! calls, the largest html!
# consumer in the Rullst repository), built against each variant of perf-832/crates.sh with Rust 1.99.1.
# Usage: compile.sh RULLST_CHECKOUT OUT_DIR
#   cold.txt          time of the first release build of each variant (dependencies included)
#   rebuild.txt       time of the site crate alone after `touch src/main.rs`, release and dev, 5 rounds rotated
#   size.txt          release binary size, .text size and lines of LLVM IR of the site crate
#   output.txt        SHA-256 of every page each variant writes (the same input for every variant)
set -euo pipefail
rullst="$(cd "$1" && pwd)"
out="$(mkdir -p "$2" && cd "$2" && pwd)"
here="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"
variants=(rullst rullst-html1 rullst-html2)
toolchain=1.99.1
for variant in "${variants[@]}"; do
  root="$work/$variant"
  mkdir -p "$root/docs"
  cp -r "$rullst/site" "$root/site"
  cp "$rullst/docs/site.css" "$rullst/docs/site.js" "$root/docs/"
  cp -r "$rullst/docs/flags" "$root/docs/"
  "$here/crates.sh" "$variant" "$root/site"
  if [ "$variant" != rullst ]; then
    printf '\n[patch.crates-io]\nrullst-macros = { path = "crates/rullst-macros" }\n' >> "$root/site/Cargo.toml"
    if [ -d "$root/site/crates/rullst-core" ]; then
      printf 'rullst-core = { path = "crates/rullst-core" }\n' >> "$root/site/Cargo.toml"
    fi
  fi
  (cd "$root/site" && cargo "+$toolchain" fetch --quiet)
done
seconds() {
  local started
  started=$(date +%s.%N)
  "$@" > /dev/null 2>&1
  python3 -c 'import sys; print(f"{float(sys.argv[1]) - float(sys.argv[2]):.2f}")' "$(date +%s.%N)" "$started"
}
for variant in "${variants[@]}"; do
  site="$work/$variant/site"
  echo "$variant release $(cd "$site" && seconds cargo "+$toolchain" build --release --offline)" | tee -a "$out/cold.txt"
  echo "$variant dev $(cd "$site" && seconds cargo "+$toolchain" build --offline)" | tee -a "$out/cold.txt"
done
for round in 1 2 3 4 5; do
  shift_by=$(( (round - 1) % ${#variants[@]} ))
  order=("${variants[@]:shift_by}" "${variants[@]:0:shift_by}")
  for variant in "${order[@]}"; do
    site="$work/$variant/site"
    for profile in release dev; do
      flag=(); [ "$profile" = release ] && flag=(--release)
      touch "$site/src/main.rs"
      echo "round $round $variant $profile $(cd "$site" && seconds cargo "+$toolchain" build "${flag[@]}" --offline)" \
        | tee -a "$out/rebuild.txt"
    done
  done
done
for variant in "${variants[@]}"; do
  site="$work/$variant/site"
  binary="$site/target/release/rullst-site"
  pages="$work/pages-$variant"
  "$binary" --out "$pages" --today 2026-11-27 > /dev/null 2>&1
  (cd "$pages" && find . -name '*.html' | sort | xargs sha256sum) | sed "s/^/$variant /" | tee -a "$out/output.txt"
  text=$(size -A "$binary" | awk '$1 == ".text" {print $2}')
  bytes=$(stat -c %s "$binary")
  # One codegen unit, so the IR of the site crate is one file.
  (cd "$site" && touch src/main.rs && cargo "+$toolchain" rustc --release --offline --bin rullst-site -- \
    --emit=llvm-ir -C codegen-units=1 > /dev/null 2>&1)
  ir=$(cat "$site"/target/release/deps/rullst_site*.ll | wc -l)
  echo "$variant binary_bytes=$bytes text_bytes=$text llvm_ir_lines=$ir" | tee -a "$out/size.txt"
  grep -A2 'name = "rullst-macros"\|name = "rullst-core"' "$site/Cargo.lock" > "$out/$variant.lock.txt"
done
{ nproc; lscpu | grep -i "model name"; cargo "+$toolchain" --version; } | tee "$out/host.txt"
