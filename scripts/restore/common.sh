#!/usr/bin/env bash
# Shared helpers for the restore scripts. Requires stellar-cli
# (https://developers.stellar.org/docs/tools/cli/stellar-cli).
#
# Environment:
#   CONTRACT_ID  contract to restore entries for (required)
#   SOURCE       stellar-cli identity that signs and PAYS for the restore (required)
#   NETWORK      stellar-cli network name (default: testnet)
set -euo pipefail

: "${CONTRACT_ID:?set CONTRACT_ID}"
: "${SOURCE:?set SOURCE}"
NETWORK="${NETWORK:-testnet}"

# Encodes a DataKey variant as base64 ScVal XDR.
#   key_xdr Series u64 7                 -> DataKey::Series(7)
#   key_xdr UserPositions address G...   -> DataKey::UserPositions(G...)
key_xdr() {
  local variant="$1" type="$2" value="$3" json
  case "$type" in
    u64) json="{\"vec\":[{\"symbol\":\"$variant\"},{\"u64\":$value}]}" ;;
    address) json="{\"vec\":[{\"symbol\":\"$variant\"},{\"address\":\"$value\"}]}" ;;
    *) echo "unsupported key type: $type" >&2; exit 1 ;;
  esac
  echo "$json" | stellar xdr encode --type ScVal --input json --output single-base64
}

# Restores the contract instance (and its wasm) plus the given persistent keys.
# Restoring an entry that is still live is a no-op.
restore() {
  stellar contract restore --id "$CONTRACT_ID" --source "$SOURCE" --network "$NETWORK"
  local args=()
  for key in "$@"; do args+=(--key-xdr "$key"); done
  if ((${#args[@]})); then
    stellar contract restore --id "$CONTRACT_ID" --durability persistent \
      "${args[@]}" --source "$SOURCE" --network "$NETWORK"
  fi
}
