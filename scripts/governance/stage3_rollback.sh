#!/usr/bin/env bash
# Rollback stage 3: the guardian (multisig) cancels any queued governor
# operation given as CANCEL_OP_ID and immediately restores itself as the
# timelock proposer. Unavailable after stage 4.
source "$(dirname "$0")/common.sh"
require MULTISIG_ACCOUNT

if [[ -n "${CANCEL_OP_ID:-}" ]]; then
  invoke "$TIMELOCK_ID" "$MULTISIG_ACCOUNT" cancel \
    --caller "$MULTISIG_ACCOUNT" --op_id "$CANCEL_OP_ID"
fi
invoke "$TIMELOCK_ID" "$MULTISIG_ACCOUNT" guardian_set_proposer \
  --new_proposer "$MULTISIG_ACCOUNT"
echo "timelock proposer: $(view "$TIMELOCK_ID" get_proposer)"
