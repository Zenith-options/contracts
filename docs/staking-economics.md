# Staking Economics

`staking/` lets ZEN holders stake and earn a share of protocol fees in
proportion to their stake. The fee splitter pays those fees in (USDC, or any
other token, with up to 10 reward tokens).

## Flow

1. **Stake**: `stake(user, amount)` moves ZEN into the contract. The stake
   starts earning right away.
2. **Fees arrive**: the pinned splitter calls `notify_reward(token, amount)`.
   The fees are shared among current stakers instantly. There is no drip
   period.
3. **Claim**: `claim_rewards(user)` pays out everything the user has earned,
   in every reward token.
4. **Exit**: `request_unstake(user, amount)` stops that amount from earning
   right away and starts the cooldown. When the cooldown ends,
   `withdraw(user)` returns the ZEN. Making another request adds to the
   pending amount and restarts the cooldown.

## Accounting

This uses the Synthetix reward-per-token accumulator, changed so that each
notification is paid out immediately:

```
on notify(token, amount):   rpt[token] += amount × 1e18 / total_staked
earned(user, token)       = owed[user][token]
                          + stake[user] × (rpt[token] − paid[user][token]) / 1e18
```

A user's `owed` is updated to the current accumulator before their stake
changes. Every operation costs O(number of reward tokens), however many
stakers there are.

### Precision

- `ACC_PRECISION = 1e18`. A reward of 1 base unit spread over 1e15 staked
  units (100 M ZEN at 7 decimals) still moves the accumulator.
- `amount × 1e18` fits in an i128 for any `amount` up to about 1.7e20 base
  units, which is 1.7e13 USDC in a single notification. Larger values panic
  instead of wrapping.
- Both divisions round down. Stakers together can never be credited more
  than was notified. Each notification can leave some dust behind: less than
  `total_staked / 1e18` base units, which in practice is zero. That dust stays
  in the contract.
- Very small stakes lose nothing over time. The accumulator keeps the
  fractional part, so a stake too small to earn 1 unit from one notification
  still collects it across several. The test
  `tiny_stake_next_to_huge_stake_keeps_precision` checks this.

### Edge cases

- **Nothing staked**: rewards are held in `undistributed(token)` and handed
  out the moment someone stakes again, or at the next notification.
- **Rewards and cooldown**: ZEN waiting out the cooldown earns nothing. A
  user can't collect fees while already on the way out.

### Guarantees (property-tested)

`prop_no_staker_claims_more_than_share` runs a random mix of stakes,
unstakes, claims and notifications across four stakers. After every step it
checks that:
- each staker's claimed plus claimable amount is at most their exact
  pro-rata entitlement, rounded up at each notification;
- the total across all stakers is at most the total notified.

## Out of scope

- Slashing.
- Voting power for staked ZEN (optional in the issue). Staked ZEN sits in the
  staking contract, so `gov_token` checkpoints won't count it toward its
  owner. A later change could have staking delegate its votes back to each
  staker.
