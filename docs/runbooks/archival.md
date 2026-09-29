# Runbook: archived contract state

Soroban archives any persistent or instance entry whose TTL lapses. The
[TTL policy](../../README.md#storage-ttl-policy) keeps anything that is
used (or keeper-bumped with `bump`) alive, but dormant entries — an
untouched position, an idle vault tag, a whole contract nobody has
called for a month — will still archive. Archived entries are **not
lost**: their value is kept in the archive and `RestoreFootprint` brings
it back byte-for-byte. The tests `archived_*` in every crate prove that
the contracts behave identically after a restore.

## What archival looks like

| Archived | Symptom |
|---|---|
| Contract instance (admin, counters, pool, feeders, signers, ...) | Every call to the contract fails before execution. The whole contract is bricked until restored. |
| One persistent entry (a `Position`, `Series`, vault `Escrow` tag, multisig `Approval`, oracle `PriceReport`) | Only transactions whose footprint includes that key fail. Simulation (`simulateTransaction`) returns a `restorePreamble` naming the keys to restore. |

Temporary storage is never used, so nothing here can be permanently
deleted by expiry.

## Detection

1. **Simulation.** Any wallet/CLI call that touches an archived key gets
   a `restorePreamble` in its simulation result. stellar-cli restores
   automatically in that case when run interactively.
2. **Proactive monitoring.** Query `getLedgerEntries` for the keys you
   care about and compare `liveUntilLedgerSeq` against `latestLedger`:

   ```sh
   # LedgerKey XDR for options_market Position(7):
   KEY=$(echo '{"contract_data":{"contract":"C...","key":{"vec":[{"symbol":"Position"},{"u64":7}]},"durability":"persistent"}}' \
     | stellar xdr encode --type LedgerKey --input json --output single-base64)
   RPC_URL=https://soroban-testnet.stellar.org scripts/restore/check_live_until.sh "$KEY"
   ```

   `liveUntilLedgerSeq < latestLedger` means archived. Anything within
   ~30 days (≈518,400 ledgers) of expiry should be keeper-bumped with the
   contract's `bump` entrypoint instead of waiting for it to archive —
   extending a live entry is much cheaper than restoring an archived one.

## Restore

All scripts need stellar-cli, `CONTRACT_ID`, `SOURCE` (the identity
that signs and pays) and optionally `NETWORK` (default `testnet`). Each
one restores the contract instance first (a no-op if it is live), then
the listed persistent keys in a single `RestoreFootprint` transaction.

| Situation | Command |
|---|---|
| Whole contract archived | `stellar contract restore --id $CONTRACT_ID --source $SOURCE --network $NETWORK` |
| options_market series | `scripts/restore/restore_series.sh <series_id>` |
| options_market position (before exercise / reclaim / refund) | `scripts/restore/restore_position.sh <position_id> <series_id> <owner>` |
| vault escrow tag | `scripts/restore/restore_vault_tag.sh <tag>` |
| Any other key | `stellar contract restore --id $CONTRACT_ID --durability persistent --key-xdr <ScVal base64> ...` (`key_xdr` in `scripts/restore/common.sh` builds the XDR) |

A restored entry comes back with the network's minimum persistent TTL
(`min_persistent_entry_ttl`). The first contract call that reads it
re-extends it to the full policy value, so follow a restore with the
transaction the user actually wanted (or a `bump`).

## Cost estimation

`RestoreFootprint` is charged like a write of the restored bytes plus
rent for the new TTL. Get the exact fee before submitting by simulating:

```sh
stellar contract restore --id $CONTRACT_ID --durability persistent \
  --key-xdr <key> --source $SOURCE --network $NETWORK --sim-only
```

The simulated `minResourceFee` is what the restore will cost. Entries
here are small (a position or series is a few hundred bytes), so a
restore costs on the order of a normal write; restoring the instance of
options_market also restores its (much larger) wasm code entry and is
the most expensive case.

## Who pays

| Entry | Who pays | Why |
|---|---|---|
| A user's own position / user index | The user (or their wallet, via the simulation `restorePreamble`) | They are the one who needs it to exercise/reclaim; funds are never at risk while they wait. |
| Series, oracle reports, multisig approvals | Protocol operators (keeper account) | Shared state many users depend on. |
| Vault tags | The integrating protocol (options_market operators) | The tag backs protocol-level escrow. |
| Contract instance | Protocol operators, immediately | The whole contract is down until it is restored. |

Anyone *may* pay for any restore — `RestoreFootprint` is permissionless
and never changes the stored value.

## Integration testing

CI currently has no local quickstart network, so restore behavior is
covered by the unit tests (`archived_*` in each crate's `test.rs`), which
push entries past `live_until` with the testutils ledger and simulate
`RestoreFootprint`. To exercise the scripts end to end locally:

```sh
docker run --rm -p 8000:8000 stellar/quickstart --local --limits testnet
stellar network add local --rpc-url http://localhost:8000/soroban/rpc \
  --network-passphrase "Standalone Network ; February 2017"
```

Deploy with `--network local`, then let entries lapse (or deploy on a
network configured with a small `min_persistent_entry_ttl`) and run the
scripts with `NETWORK=local`.
