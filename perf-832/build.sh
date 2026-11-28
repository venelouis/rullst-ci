#!/usr/bin/env bash
# Builds one image of the 13.1 phase-2 html! measurements (Rullst issue #832, direction 5).
# Usage: build.sh NAME   (BENCH = checkout of Rullst/Benchmarks, OUT = output dir)
#   rullst         frameworks/rullst as it is, on crates.io 13.0.0-rc.1 (before)
#   rullst-html1   the same with the published rullst-macros 13.0.0-rc.1 source patched by
#                  perf-832/patches/html1-macros.patch (one buffer; macro only, rullst-core as published)
#   rullst-html2   the same with rullst-macros and rullst-core 13.0.0-rc.1 patched by html2-macros.patch and
#                  html2-core.patch (one buffer, values written into it)
#   rullst-path    control: rullst-macros and rullst-core 13.0.0-rc.1 as path dependencies, not patched
#   rullst-minimal, rullst-minimal-html2   frameworks/rullst-minimal as it is, and with the html2 crates
# The patches are the Rullst branch perf/v13-1-html-single-buffer against its merge base, whose rullst-macros and
# rullst-core html sources equal the 13.0.0-rc.1 crates (both crates are SHA-256 checked).
set -euo pipefail
name="$1"
: "${BENCH:?}" "${OUT:?}"
RV=13.0.0-rc.1
here="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"
mkdir -p "$OUT"
case "$name" in
  rullst-minimal*) app=rullst-minimal; variant="rullst${name#rullst-minimal}";;
  *) app=rullst; variant="$name";;
esac
cp -r "$BENCH/frameworks/$app" "$work/ctx"
"$here/crates.sh" "$variant" "$work/ctx"
if [ "$variant" != rullst ]; then
  {
    printf '\n[patch.crates-io]\nrullst-macros = { path = "crates/rullst-macros" }\n'
    if [ -d "$work/ctx/crates/rullst-core" ]; then
      printf 'rullst-core = { path = "crates/rullst-core" }\n'
    fi
  } >> "$work/ctx/Cargo.toml"
  sed -i 's|^COPY src ./src$|COPY src ./src\nCOPY crates ./crates|' "$work/ctx/Dockerfile"
fi
grep -n "^ENV\|^COPY" "$work/ctx/Dockerfile" || true
tail -4 "$work/ctx/Cargo.toml"
started=$(date +%s)
docker build --pull --progress=plain --build-arg "RULLST_VERSION=$RV" -t "bench/$name:local" "$work/ctx"
echo "$(( $(date +%s) - started ))" > "$OUT/$name.build-seconds"
cid="$(docker create "bench/$name:local")"
docker cp "$cid:/bench/versions.txt" "$OUT/$name.versions.txt" || true
docker cp "$cid:/app/server" "$OUT/$name.server"
docker rm "$cid" > /dev/null
sha256sum "$OUT/$name.server" | tee "$OUT/$name.server.sha256"
stat -c '%s' "$OUT/$name.server" > "$OUT/$name.server-bytes"
size -A "$OUT/$name.server" | awk '$1 == ".text" {print $2}' > "$OUT/$name.server-text-bytes"
ls -la "$OUT"
