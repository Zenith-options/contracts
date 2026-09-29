#!/usr/bin/env bash
# Restores an options_market series: the instance, Series(id) and
# SeriesEscrow(id).
#   CONTRACT_ID=C... SOURCE=me ./restore_series.sh <series_id>
source "$(dirname "$0")/common.sh"
SERIES_ID="${1:?usage: $0 <series_id>}"

restore \
  "$(key_xdr Series u64 "$SERIES_ID")" \
  "$(key_xdr SeriesEscrow u64 "$SERIES_ID")"
