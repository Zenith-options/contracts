# LP Token: SEP-41 conformance

`lp_token` is the share token for `lp_pool`. `initialize(pool, decimal,
name, symbol)` pins the pool. It requires the pool's own authorization,
so nobody can claim a freshly deployed token for a pool that didn't
agree to it. Only the pinned pool can `mint(to, amount)`.

## Why a direct implementation, not OpenZeppelin `fungible`

The token implements `soroban_sdk::token::Interface` in a single file,
following the reference `soroban-examples/token` layout. Every Zenith
contract pins soroban-sdk 21, the interface is small, and doing it
directly keeps the audited surface to this crate with no extra
dependency. Migrating to OpenZeppelin's `fungible` module stays an
option once the workspace moves to a matching SDK.

## Conformance checklist

| SEP-41 requirement | Status | Test |
|---|---|---|
| `allowance(from, spender)` returns 0 once the expiration ledger has passed | ✅ | `allowance_expires_at_its_expiration_ledger` |
| `approve(from, spender, amount, expiration_ledger)` requires `from` auth, emits `approve` with topics `[approve, from, spender]` and data `(amount, expiration_ledger)` | ✅ | `transfer_approve_transfer_from_burn` |
| `approve` rejects a nonzero amount with a past `expiration_ledger`, but accepts it for a zero amount | ✅ | `a_nonzero_allowance_cannot_be_already_expired`, `a_zero_allowance_may_carry_a_past_expiration` |
| `balance(id)` | ✅ | all |
| `transfer(from, to, amount)` requires `from` auth, emits `[transfer, from, to]` / `amount` | ✅ | `transfer_approve_transfer_from_burn` |
| `transfer_from(spender, from, to, amount)` requires `spender` auth and consumes allowance | ✅ | `transfer_approve_transfer_from_burn`, `transfer_from_over_allowance_fails`, `transfer_from_an_expired_allowance_fails` |
| `burn(from, amount)` requires `from` auth, emits `[burn, from]` / `amount` | ✅ | `transfer_approve_transfer_from_burn` |
| `burn_from(spender, from, amount)` requires `spender` auth and consumes allowance | ✅ | `transfer_approve_transfer_from_burn`, `burn_from_over_allowance_fails` |
| `decimals`, `name`, `symbol` | ✅ | `metadata` |
| Negative amounts rejected | ✅ | `negative_amounts_are_rejected` |
| Works through the generic `token::TokenClient` | ✅ | `works_through_the_generic_token_client` |
| `mint` restricted to the pinned pool; emits `[mint, pool, to]` / `amount` | ✅ | `mint_requires_the_pool_and_emits_mint`, `mint_without_pool_auth_fails` |

## TTL management

- **Instance** (pool, metadata): extended on every state-changing call.
- **Balances**: persistent. Extended to 90 days whenever read or
  written, once fewer than 60 days remain.
- **Allowances**: temporary storage, live exactly until
  `expiration_ledger`. An expired allowance reads as zero, and the
  network drops the entry without anyone paying to delete it.
