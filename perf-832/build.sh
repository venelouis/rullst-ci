#!/usr/bin/env bash
# Builds one image of the 13.1 save() measurements (Rullst issue #832, phase 2, direction 7).
# Usage: build.sh NAME   (BENCH = checkout of Rullst/Benchmarks, OUT = output dir)
#   rullst            frameworks/rullst as it is, on crates.io 13.0.0-rc.1 (before)
#   rullst-save-trim  the same with the published rullst-orm-macros 13.0.0-rc.1 source patched by
#   rullst-save       perf-832/patches/<save-trim|save>.patch: the Rullst branch's diff against main (whose
#                     rullst-orm-macros source is the 13.0.0-rc.1 source) after its RETURNING commit
#                     (save-trim) and after its last commit (save)
# The two patched images also read PERF832_QUERY_TIMEOUT_OFF at start-up: when it is set, the app calls
# Orm::set_query_timeout(0) after Orm::init_with_options (a measurement switch, not part of the change; the
# ORM's default query timeout is 30 s, and the change runs a save without a transaction only without one).
set -euo pipefail
name="$1"
: "${BENCH:?}" "${OUT:?}"
RV=13.0.0-rc.1
here="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"
mkdir -p "$OUT"
cp -r "$BENCH/frameworks/rullst" "$work/ctx"
case "$name" in
  rullst) ;;
  rullst-save|rullst-save-trim)
    crate_sha=b9f263b69c32eefd46a4be7553df58afb60c63797ecc66f22094f1ce4fa76c4c
    curl --fail --silent --show-error --location --retry 3 -o "$work/macros.crate" \
      "https://static.crates.io/crates/rullst-orm-macros/rullst-orm-macros-$RV.crate"
    echo "$crate_sha  $work/macros.crate" | sha256sum --check --quiet
    mkdir "$work/ctx/rullst-orm-macros-patched"
    tar -xzf "$work/macros.crate" -C "$work/ctx/rullst-orm-macros-patched" --strip-components=1
    (cd "$work/ctx/rullst-orm-macros-patched" && patch -p2 --batch < "$here/patches/${name#rullst-}.patch")
    printf '\n[patch.crates-io]\nrullst-orm-macros = { path = "rullst-orm-macros-patched" }\n' >> "$work/ctx/Cargo.toml"
    sed -i 's|^COPY src ./src$|COPY src ./src\nCOPY rullst-orm-macros-patched ./rullst-orm-macros-patched|' "$work/ctx/Dockerfile"
    init='    Orm::init_with_options(\&database_url, env_or("DB_POOL_SIZE", 16), 10).await?;'
    switch='    if std::env::var_os("PERF832_QUERY_TIMEOUT_OFF").is_some() { Orm::set_query_timeout(0); }'
    sed -i "s|^${init}\$|&\n${switch}|" "$work/ctx/src/main.rs"
    grep -c PERF832_QUERY_TIMEOUT_OFF "$work/ctx/src/main.rs" | grep -qx 1
    ;;
  *) echo "unknown image $name" >&2; exit 2;;
esac
grep -n "^ENV\|^COPY" "$work/ctx/Dockerfile" || true
grep -n "PERF832\|init_with_options" "$work/ctx/src/main.rs" || true
docker build --pull --progress=plain --build-arg "RULLST_VERSION=$RV" -t "bench/$name:local" "$work/ctx"
cid="$(docker create "bench/$name:local")"
docker cp "$cid:/bench/versions.txt" "$OUT/$name.versions.txt" || true
docker cp "$cid:/app/server" "$OUT/$name.server"
docker rm "$cid" > /dev/null
sha256sum "$OUT/$name.server" | tee "$OUT/$name.server.sha256"
ls -la "$OUT"
