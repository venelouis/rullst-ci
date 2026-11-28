#!/usr/bin/env bash
# Writes the patched crates a variant needs into DIR/crates (nothing for the unpatched "before").
# Usage: crates.sh VARIANT DIR   VARIANT: rullst | rullst-path | rullst-html1 | rullst-html2 | rullst-html2-cap
#   rullst-path        control: both published crates as path dependencies, not patched
#   rullst-html2-cap   html2 with the expansion's first capacity raised to at least 2048 bytes: an experiment that
#                      shows what the remaining reallocations cost; not a proposal and not in the Rullst branch.
set -euo pipefail
variant="$1"
dir="$2"
RV=13.0.0-rc.1
here="$(cd "$(dirname "$0")" && pwd)"
[ "$variant" = rullst ] && exit 0
fetch() {
  local crate="$1" sha="$2" out="$3"
  curl --fail --silent --show-error --location --retry 3 -o "$out.crate" \
    "https://static.crates.io/crates/$crate/$crate-$RV.crate"
  echo "$sha  $out.crate" | sha256sum --check --quiet
  mkdir -p "$out"
  tar -xzf "$out.crate" -C "$out" --strip-components=1
  rm "$out.crate"
}
mkdir -p "$dir/crates"
fetch rullst-macros f432fe7a4979208445ac5b9db06885ff3e411d4b221cd07aa03a05ea075779e1 "$dir/crates/rullst-macros"
case "$variant" in
  rullst-path)
    fetch rullst-core b9a238e41ddf20b0fac6944f2ccb2429e3bcfb7de68ec972655230e1d99ac860 "$dir/crates/rullst-core"
    ;;
  rullst-html1)
    (cd "$dir/crates/rullst-macros" && patch -p2 --batch --quiet < "$here/patches/html1-macros.patch")
    ;;
  rullst-html2|rullst-html2-cap)
    fetch rullst-core b9a238e41ddf20b0fac6944f2ccb2429e3bcfb7de68ec972655230e1d99ac860 "$dir/crates/rullst-core"
    (cd "$dir/crates/rullst-macros" && patch -p2 --batch --quiet < "$here/patches/html2-macros.patch")
    (cd "$dir/crates/rullst-core" && patch -p2 --batch --quiet < "$here/patches/html2-core.patch")
    if [ "$variant" = rullst-html2-cap ]; then
      file="$dir/crates/rullst-macros/src/html_parser.rs"
      sed -i 's|let capacity = self.nodes.iter().map(HtmlNode::static_size).sum::<usize>();|let capacity = self.nodes.iter().map(HtmlNode::static_size).sum::<usize>().max(2048);|' "$file"
      grep -q 'max(2048)' "$file"
    fi
    ;;
  *) echo "unknown variant $variant" >&2; exit 2;;
esac
