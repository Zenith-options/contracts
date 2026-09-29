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

contracts=(multisig price_oracle vault options_market)
release=target/wasm32-unknown-unknown/release

# A tree with a root Cargo.toml is the workspace layout: `cargo xtask build`
# handles dependency order and writes every wasm to the shared target/.
# Otherwise it predates the workspace (standalone crates, each with its own
# target/), so build by hand in the old order.
build() {
  if [ -f "$1/Cargo.toml" ]; then
    (cd "$1" && cargo xtask build $(printf -- '-p %s ' "${contracts[@]}"))
  else
    for c in multisig price_oracle vault params options_market; do
      (cd "$1/$c" && cargo build -q --target wasm32-unknown-unknown --release)
    done
  fi
}
wasm_path() {
  if [ -f "$1/Cargo.toml" ]; then
    echo "$1/$release/zenith_$2.wasm"
  else
    echo "$1/$2/$release/zenith_$2.wasm"
  fi
}

build "$tmp/base"
build "$root"
(cd "$root" && CARGO_TARGET_DIR="$root/target" cargo build -q --release -p spec-diff)

status=0
for c in "${contracts[@]}"; do
  base=$(wasm_path "$tmp/base" "$c")
  head=$(wasm_path "$root" "$c")
  allow=$(awk -v c="$c" '$1 == c { printf "--allow %s ", $2 }' "$root/tools/spec-diff/allow.txt")
  echo "== $c: $(wc -c <"$base") -> $(wc -c <"$head") bytes"
  # shellcheck disable=SC2086
  "$root/target/release/spec-diff" "$base" "$head" $allow || status=1
done
exit $status
