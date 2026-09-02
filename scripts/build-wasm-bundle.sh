#!/usr/bin/env bash
# Build the browser bundle: cargo (profile `wasm`) → wasm-bindgen (target
# web) → wasm-opt → manifest.json → tarball. Same command locally and in
# CI; the release workflow attaches the tarball to the tagged release.
#
# Output: target/wasm-bundle/dist/ and target/wasm-bundle/<name>.tar.gz
# Requires: the pinned toolchain (rust-toolchain.toml, wasm32 target
# included), wasm-bindgen CLI at EXACTLY the crate's pinned version, and
# binaryen's wasm-opt.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

crate_toml="crates/wasm/Cargo.toml"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$crate_toml" | head -1)"
pin="$(sed -n 's/^wasm-bindgen = "=\(.*\)"/\1/p' "$crate_toml" | head -1)"
[ -n "$version" ] || { echo "cannot read version from $crate_toml" >&2; exit 1; }
[ -n "$pin" ] || { echo "cannot read the wasm-bindgen pin from $crate_toml" >&2; exit 1; }

have="$(wasm-bindgen --version | awk '{print $2}')"
if [ "$have" != "$pin" ]; then
  echo "wasm-bindgen CLI is $have but the crate pins $pin; install the matching CLI:" >&2
  echo "  cargo install wasm-bindgen-cli --version $pin --locked" >&2
  exit 1
fi
command -v wasm-opt >/dev/null || { echo "wasm-opt (binaryen) is required" >&2; exit 1; }

out="target/wasm-bundle"
dist="$out/dist"
rm -rf "$out"
mkdir -p "$dist"

cargo build -p mtc-wasm --profile wasm --target wasm32-unknown-unknown
wasm-bindgen --target web --out-dir "$dist" --out-name mtc_wasm \
  target/wasm32-unknown-unknown/wasm/mtc_wasm.wasm
# Name every feature the Rust wasm32 target emits by default (sign-ext,
# mutable-globals, reference-types, multivalue, bulk-memory,
# nontrapping-fptoint): an older wasm-opt validates against the MVP set
# unless told otherwise and refuses a module it could have optimised.
echo "using $(wasm-opt --version)"
wasm-opt -Oz --enable-sign-ext --enable-mutable-globals --enable-reference-types \
  --enable-multivalue --enable-bulk-memory --enable-nontrapping-float-to-int \
  -o "$dist/mtc_wasm_bg.wasm" "$dist/mtc_wasm_bg.wasm"

# wasm-bindgen's web target may also emit a *_bg.wasm.d.ts; keep only the
# four files the manifest names.
find "$dist" -type f ! -name 'mtc_wasm_bg.wasm' ! -name 'mtc_wasm.js' ! -name 'mtc_wasm.d.ts' -delete

sha256() {
  if command -v sha256sum >/dev/null; then sha256sum "$1" | awk '{print $1}'
  else shasum -a 256 "$1" | awk '{print $1}'; fi
}
commit="$(git rev-parse HEAD 2>/dev/null || echo unknown)"
{
  echo "{"
  echo "  \"toolchains_version\": \"$version\","
  echo "  \"crate_version\": \"$version\","
  echo "  \"wasm_bindgen_version\": \"$pin\","
  echo "  \"built_from\": \"$commit\","
  echo "  \"files\": {"
  echo "    \"mtc_wasm_bg.wasm\": \"$(sha256 "$dist/mtc_wasm_bg.wasm")\","
  echo "    \"mtc_wasm.js\": \"$(sha256 "$dist/mtc_wasm.js")\","
  echo "    \"mtc_wasm.d.ts\": \"$(sha256 "$dist/mtc_wasm.d.ts")\""
  echo "  }"
  echo "}"
} > "$dist/manifest.json"

name="machine-toolchains-wasm-v$version"
# GNU tar supports --transform; BSD tar (macOS) supports -s with the same
# sed-style expression but rejects --transform outright. Try GNU-style
# first, then BSD-style, then fall back to a plain copy-and-rename so the
# script works on both CI (Ubuntu/GNU tar) and local macOS (BSD tar).
if tar --version 2>/dev/null | grep -qi gnu; then
  tar -czf "$out/$name.tar.gz" -C "$out" --transform "s,^dist,$name," dist
elif tar -czf "$out/$name.tar.gz" -C "$out" -s ",^dist,$name," dist 2>/dev/null; then
  :
else
  cp -r "$dist" "$out/$name"
  tar -czf "$out/$name.tar.gz" -C "$out" "$name"
  rm -rf "$out/$name"
fi

raw=$(wc -c < "$dist/mtc_wasm_bg.wasm")
gz=$(gzip -9 -c "$dist/mtc_wasm_bg.wasm" | wc -c)
echo "bundle: $out/$name.tar.gz"
echo "wasm:   $raw bytes, $gz bytes gzipped"
