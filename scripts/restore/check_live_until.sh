#!/usr/bin/env bash
# Prints the live-until ledger of an entry next to the latest ledger, via
# RPC getLedgerEntries. An entry whose liveUntilLedgerSeq is below the
# latest ledger is archived and must be restored before use.
#   RPC_URL=https://soroban-testnet.stellar.org ./check_live_until.sh <ledger_key_xdr_base64>
set -euo pipefail
RPC_URL="${RPC_URL:-https://soroban-testnet.stellar.org}"
KEY="${1:?usage: $0 <ledger_key_xdr_base64>}"

curl -s "$RPC_URL" -H 'Content-Type: application/json' -d "{
  \"jsonrpc\": \"2.0\", \"id\": 1, \"method\": \"getLedgerEntries\",
  \"params\": { \"keys\": [\"$KEY\"] }
}" | jq '{latestLedger: .result.latestLedger, entries: [.result.entries[]? | {liveUntilLedgerSeq}]}'
