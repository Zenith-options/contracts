#!/usr/bin/env bash
# Stage 1 → 2: the multisig account hands admin to the timelock (whose
# proposer and guardian are the multisig account).
# Rollback: stage2_rollback.sh.
source "$(dirname "$0")/common.sh"
require MULTISIG_ACCOUNT

echo "timelock proposer: $(view "$TIMELOCK_ID" get_proposer) (expect $MULTISIG_ACCOUNT)"
echo "timelock guardian: $(view "$TIMELOCK_ID" get_guardian) (expect $MULTISIG_ACCOUNT)"
invoke "$MARKET_ID" "$MULTISIG_ACCOUNT" transfer_admin --new_admin "$TIMELOCK_ID"
invoke "$ORACLE_ID" "$MULTISIG_ACCOUNT" transfer_admin --new_admin "$TIMELOCK_ID"
show_admins
