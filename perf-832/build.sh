#!/usr/bin/env bash
# Builds one image of the 13.1 phase-1 investigation (Rullst issue #832).
# Usage: build.sh NAME   (BENCH = checkout of Rullst/Benchmarks, OUT = output dir)
#   axum | rullst | rullst-minimal      the Dockerfiles as they are (rullst built from crates.io 13.0.0-rc.1)
#   rullst-ablate                       frameworks/rullst with the published rullst-core 13.0.0-rc.1 patched by
#                                       perf-832/ablate.patch (RULLST_ABLATE switches; measurement instrument)
#   axum-p | rullst-p | rullst-minimal-p   the same sources with symbols and frame pointers for perf and heaptrack
set -euo pipefail
name="$1"
: "${BENCH:?}" "${OUT:?}"
RV=13.0.0-rc.1
here="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"
mkdir -p "$OUT"
profile=false
base="$name"
case "$name" in *-p) profile=true; base="${name%-p}";; esac
build_arg=()
case "$base" in
  rullst|rullst-minimal|rullst-ablate) build_arg=(--build-arg "RULLST_VERSION=$RV");;
esac
src="$base"; [ "$base" = rullst-ablate ] && src=rullst
cp -r "$BENCH/frameworks/$src" "$work/ctx"
if [ "$base" = rullst-ablate ]; then
  crate_sha=b9a238e41ddf20b0fac6944f2ccb2429e3bcfb7de68ec972655230e1d99ac860
  curl --fail --silent --show-error --location --retry 3 -o "$work/core.crate" \
    "https://static.crates.io/crates/rullst-core/rullst-core-$RV.crate"
  echo "$crate_sha  $work/core.crate" | sha256sum --check --quiet
  mkdir "$work/ctx/rullst-core-patched"
  tar -xzf "$work/core.crate" -C "$work/ctx/rullst-core-patched" --strip-components=1
  (cd "$work/ctx/rullst-core-patched" && patch -p2 < "$here/ablate.patch")
  printf '\n[patch.crates-io]\nrullst-core = { path = "rullst-core-patched" }\n' >> "$work/ctx/Cargo.toml"
  sed -i 's|^COPY src ./src$|COPY src ./src\nCOPY rullst-core-patched ./rullst-core-patched|' "$work/ctx/Dockerfile"
fi
if $profile; then
  sed -i '0,/^WORKDIR \/src$/s||WORKDIR /src\nENV CARGO_PROFILE_RELEASE_STRIP=none CARGO_PROFILE_RELEASE_DEBUG=line-tables-only RUSTFLAGS="-C force-frame-pointers=yes"|' "$work/ctx/Dockerfile"
fi
grep -n "^ENV\|^COPY" "$work/ctx/Dockerfile" || true
docker build --pull --progress=plain "${build_arg[@]}" -t "bench/$name:local" "$work/ctx"
docker save "bench/$name:local" | gzip -1 > "$OUT/$name.tar.gz"
cid="$(docker create "bench/$name:local")"
docker cp "$cid:/bench/versions.txt" "$OUT/$name.versions.txt" || true
docker rm "$cid" > /dev/null
ls -la "$OUT"
