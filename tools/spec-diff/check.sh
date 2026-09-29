#!/usr/bin/env bash
# Builds every contract at <base-ref> and at the working tree, then
# diffs each contract's spec (public ABI) and reports the wasm size delta.
# Usage: tools/spec-diff/check.sh [base-ref]   (default: origin/main)
set -euo pipefail

base_ref=${1:-origin/main}
root=$(git rev-parse --show-toplevel)
tmp=$(mktemp -d)
git -C "$root" worktree add --detach "$tmp/base" "$base_ref" >/dev/null
trap 'git -C "$root" worktree remove --force "$tmp/base"' EXIT

build() {
  for c in multisig price_oracle vault options_market; do
    (cd "$1/$c" && cargo build -q --target wasm32-unknown-unknown --release)
  done
}
build "$tmp/base"
build "$root"
cargo build -q --release --manifest-path "$root/tools/spec-diff/Cargo.toml"

status=0
for c in multisig price_oracle vault options_market; do
  wasm="target/wasm32-unknown-unknown/release/zenith_$c.wasm"
  allow=$(awk -v c="$c" '$1 == c { printf "--allow %s ", $2 }' "$root/tools/spec-diff/allow.txt")
  echo "== $c: $(wc -c <"$tmp/base/$c/$wasm") -> $(wc -c <"$root/$c/$wasm") bytes"
  # shellcheck disable=SC2086
  "$root/tools/spec-diff/target/release/spec-diff" "$tmp/base/$c/$wasm" "$root/$c/$wasm" $allow || status=1
done
exit $status
