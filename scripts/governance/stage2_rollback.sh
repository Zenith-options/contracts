#!/usr/bin/env bash
# Rollback stage 2: the multisig (timelock proposer) schedules
# transfer_admin back to itself. Run with PHASE=schedule, then after
# TL_DELAY with PHASE=execute.
source "$(dirname "$0")/common.sh"
require MULTISIG_ACCOUNT PHASE

ID=$(op_id "rollback-stage2-$MARKET_ID")
ARG="[{\"address\":\"$MULTISIG_ACCOUNT\"}]"
case "$PHASE" in
  schedule)
    tl_schedule "$MULTISIG_ACCOUNT" "$ID" \
      "[\"$MARKET_ID\",\"$ORACLE_ID\"]" '["transfer_admin","transfer_admin"]' "[$ARG,$ARG]"
    ;;
  execute)
    tl_execute "$MULTISIG_ACCOUNT" "$ID"
    show_admins
    ;;
esac
