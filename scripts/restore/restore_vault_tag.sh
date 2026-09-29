#!/usr/bin/env bash
# Restores a vault escrow tag: the instance and Escrow(tag).
#   CONTRACT_ID=C... SOURCE=me ./restore_vault_tag.sh <tag>
source "$(dirname "$0")/common.sh"
TAG="${1:?usage: $0 <tag>}"

restore "$(key_xdr Escrow u64 "$TAG")"
