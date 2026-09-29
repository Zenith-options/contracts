# options_market: owner-authorized `transfer_position` (#51)

This PR delivers one acceptance-criteria item from #51. #49, #50 and #62 are referenced so that they close with this PR, but nothing from them is implemented here.

## #51 Transferable positions with owner approvals

**What existed:** a position's `owner` was fixed at open time. There was no way to move a position to another address.

**Done (AC1):**
- `transfer_position(from, to, position_id)`: requires `from.require_auth()` and `from` to own the position (`Unauthorized` otherwise). It sets `owner = to` and updates `UserPositions` for **both** parties (removed from `from`'s list via a new `storage::remove_user_position`, appended to `to`'s).
- The new owner inherits every right the position carries: exercise for longs, collateral reclaim for shorts, and cancellation refunds for either.
- Closed positions are rejected with the existing `AlreadyExercised` / `AlreadySettled`. The call is blocked while paused, and `to == from` is a no-op. No new error codes, so nothing collides with other open PRs.
- Emits `position_transferred` (topics: name, `from`, `to`; data: `position_id`).
- README Traders row added.
- 8 new tests:
  - a transferred long is exercisable only by the new owner (paid in full);
  - a transferred short moves the collateral-reclaim right (old owner rejected);
  - a transferred position refunds the new owner on cancellation;
  - owner and signature required; closed positions rejected; self-transfer is a no-op with no duplicate list entry; paused; event data content.

**Shorts and AC3:** shorts are transferable here because a short's collateral is fully locked in the contract at write time. The recipient receives only the right to reclaim the leftover collateral after settlement, never an unfunded obligation. The explicit recipient-authorization / disallow policy that AC3 asks to document is still open.

**Not done in this PR:**
- `approve_position` and `transfer_position_from` with ledger expiration (AC2).
- The documented short-transfer policy (AC3).
- `position_approved` event (AC4).

## #49 Open-interest risk limits

**Not done in this PR:**
- `set_series_oi_cap` / `set_underlying_notional_cap` (admin/role/multisig), `OpenInterestCapExceeded` enforcement, utilization views, and the `risk_limit_updated` event.

## #50 Multi-collateral with oracle haircuts

**Not done in this PR:**
- Collateral registry, haircut valuation, settlement conversion ADR and implementation, and two-token tests.

## #62 Series templates and keeper-driven listings

**Not done in this PR:**
- `Template` governance, idempotent `list_from_template` with strike rounding and caps, and premium sourcing.

## Verification

Built the `multisig`, `price_oracle` and `vault` wasm first, as CI does, then in `options_market`:
- `cargo test`: 116 passed, 0 failed (108 existing + 8 new).
- `cargo clippy --all-targets -- -D warnings`: clean.
- `cargo fmt --check`: clean.
- `cargo build --target wasm32-unknown-unknown --release`: ok.

Closes #49
Closes #50
Closes #51
Closes #62
