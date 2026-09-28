#!/usr/bin/env bash
# Restores everything exercise/reclaim/claim_refund touch for one
# options_market position: the instance, Position(id), its Series, and
# the owner's UserPositions index.
#   CONTRACT_ID=C... SOURCE=me ./restore_position.sh <position_id> <series_id> <owner_address>
source "$(dirname "$0")/common.sh"
POSITION_ID="${1:?usage: $0 <position_id> <series_id> <owner_address>}"
SERIES_ID="${2:?usage: $0 <position_id> <series_id> <owner_address>}"
OWNER="${3:?usage: $0 <position_id> <series_id> <owner_address>}"

restore \
  "$(key_xdr Position u64 "$POSITION_ID")" \
  "$(key_xdr Series u64 "$SERIES_ID")" \
  "$(key_xdr UserPositions address "$OWNER")"
