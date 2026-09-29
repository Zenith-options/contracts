#!/usr/bin/env bash
# Stage 3 → 4: a governor proposal calling the timelock's lock_in, which
# permanently removes the guardian. IRREVERSIBLE.
# PHASE=propose → vote (VOTER, SUPPORT=1) → queue → execute.
source "$(dirname "$0")/common.sh"
require PHASE

DESC=$(op_id "Lock in governance handover")
TARGETS="[\"$TIMELOCK_ID\"]"
FNS='["lock_in"]'
ARGS='[[]]'
case "$PHASE" in
  propose)
    require PROPOSER
    invoke "$GOVERNOR_ID" "$PROPOSER" propose --proposer "$PROPOSER" \
      --targets "$TARGETS" --fns "$FNS" --args "$ARGS" --description_hash "$DESC"
    ;;
  vote)
    require VOTER PROPOSAL_ID
    invoke "$GOVERNOR_ID" "$VOTER" cast_vote --voter "$VOTER" \
      --id "$PROPOSAL_ID" --support "${SUPPORT:-1}"
    ;;
  queue)
    require PROPOSAL_ID EXECUTOR
    invoke "$GOVERNOR_ID" "$EXECUTOR" queue --id "$PROPOSAL_ID"
    ;;
  execute)
    require PROPOSAL_ID EXECUTOR
    tl_execute "$EXECUTOR" "$PROPOSAL_ID"
    echo "locked in: $(view "$TIMELOCK_ID" is_locked_in)"
    ;;
esac
