#!/usr/bin/env bash
# Stage 2 → 3: the multisig schedules a timelock self-call making the
# governor the only proposer. Run with PHASE=schedule, then after TL_DELAY
# with PHASE=execute. Rollback: stage3_rollback.sh (immediate, guardian).
source "$(dirname "$0")/common.sh"
require MULTISIG_ACCOUNT PHASE

ID=$(op_id "stage3-$TIMELOCK_ID")
case "$PHASE" in
  schedule)
    tl_schedule "$MULTISIG_ACCOUNT" "$ID" \
      "[\"$TIMELOCK_ID\"]" '["set_prop"]' "[[{\"address\":\"$GOVERNOR_ID\"}]]"
    ;;
  execute)
    tl_execute "$MULTISIG_ACCOUNT" "$ID"
    echo "timelock proposer: $(view "$TIMELOCK_ID" get_proposer) (expect $GOVERNOR_ID)"
    ;;
esac
