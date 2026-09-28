#![no_std]

//! Zenith Vault — a per-tag escrow ledger for a single token.
//!
//! Built to close a gap discovered while testing options_market: that
//! contract holds every writer's collateral and every buyer's premium in
//! one undifferentiated token balance, with no accounting of which
//! balance is actually earmarked for which position. This contract is
//! that missing accounting layer — deposit/withdraw against a caller-
//! defined `tag` (e.g. a position_id), so "how much is earmarked for tag
//! 7" is always answerable instead of inferred from the token contract's
//! raw balance.

use soroban_sdk::{contract, contractimpl, panic_with_error, token, Address, Env, Map, Vec};

#[cfg(test)]
mod test;

mod error;
mod events;
mod multisig_client;
mod types;

use error::Error;
use types::DataKey;

fn require_not_paused(env: &Env) {
    let paused: bool = env
        .storage()
        .instance()
        .get(&DataKey::Paused)
        .unwrap_or(false);
    if paused {
        panic_with_error!(env, Error::ContractPaused);
    }
}

/// Max entries in one withdraw_batch/transfer_tag_batch call.
pub const MAX_BATCH: u32 = 50;
/// Max tags returned by one get_tags/verify_ledger page.
pub const MAX_PAGE: u32 = 100;

fn escrow_of(env: &Env, tag: u64) -> i128 {
    env.storage()
        .persistent()
        .get(&DataKey::Escrow(tag))
        .unwrap_or(0)
}

fn tag_count(env: &Env) -> u32 {
    env.storage()
        .instance()
        .get(&DataKey::TagCount)
        .unwrap_or(0)
}

/// The only place a tag's escrow balance is written. Keeps the ActiveTags
/// index in sync: a tag joins it on its first nonzero balance and leaves
/// it (swap-remove, O(1)) when its balance returns to zero.
fn set_escrow(env: &Env, tag: u64, new_balance: i128) {
    let old_balance = escrow_of(env, tag);
    let store = env.storage().persistent();
    if new_balance == 0 {
        store.remove(&DataKey::Escrow(tag));
        if old_balance != 0 {
            let pos: u32 = store.get(&DataKey::TagPos(tag)).unwrap();
            let last = tag_count(env) - 1;
            if pos != last {
                let last_tag: u64 = store.get(&DataKey::TagAt(last)).unwrap();
                store.set(&DataKey::TagAt(pos), &last_tag);
                store.set(&DataKey::TagPos(last_tag), &pos);
            }
            store.remove(&DataKey::TagAt(last));
            store.remove(&DataKey::TagPos(tag));
            env.storage().instance().set(&DataKey::TagCount, &last);
        }
    } else {
        store.set(&DataKey::Escrow(tag), &new_balance);
        if old_balance == 0 {
            let n = tag_count(env);
            store.set(&DataKey::TagAt(n), &tag);
            store.set(&DataKey::TagPos(tag), &n);
            env.storage()
                .instance()
                .set(&DataKey::TagCount, &(n.checked_add(1).unwrap()));
        }
    }
}

fn add_total(env: &Env, delta: i128) {
    let total: i128 = env
        .storage()
        .instance()
        .get(&DataKey::TotalEscrowed)
        .unwrap();
    env.storage()
        .instance()
        .set(&DataKey::TotalEscrowed, &total.checked_add(delta).unwrap());
}

fn page_bounds(env: &Env, cursor: u32, limit: u32) -> (u32, u32) {
    let n = tag_count(env);
    let start = cursor.min(n);
    let end = start.saturating_add(limit.min(MAX_PAGE)).min(n);
    (start, end)
}

/// Post-condition on every mutating entrypoint, compiled in only with
/// `--features invariants`: the ActiveTags index is consistent, and
/// `sum(Escrow(tag)) == TotalEscrowed <= token.balance(vault)`. O(tags),
/// so it's for tests/fuzzing, never production wasm.
#[cfg(feature = "invariants")]
fn check_invariants(env: &Env) {
    let store = env.storage().persistent();
    let mut sum: i128 = 0;
    for i in 0..tag_count(env) {
        let tag: u64 = store.get(&DataKey::TagAt(i)).unwrap();
        let pos: u32 = store.get(&DataKey::TagPos(tag)).unwrap();
        let bal = escrow_of(env, tag);
        if pos != i || bal <= 0 {
            panic!("vault invariant violated: tag index");
        }
        sum = sum.checked_add(bal).unwrap();
    }
    let total: i128 = env
        .storage()
        .instance()
        .get(&DataKey::TotalEscrowed)
        .unwrap();
    let token_address: Address = env.storage().instance().get(&DataKey::Token).unwrap();
    let actual = token::Client::new(env, &token_address).balance(&env.current_contract_address());
    if sum != total || total > actual {
        panic!("vault invariant violated: ledger sum");
    }
}

#[cfg(not(feature = "invariants"))]
fn check_invariants(_env: &Env) {}

/// Shared core of withdraw/withdraw_via_multisig; auth is the caller's job.
fn withdraw_one(env: &Env, tag: u64, to: Address, amount: i128) {
    if amount <= 0 {
        panic_with_error!(env, Error::InvalidAmount);
    }
    let balance = escrow_of(env, tag);
    if balance < amount {
        panic_with_error!(env, Error::InsufficientEscrowBalance);
    }
    set_escrow(env, tag, balance.checked_sub(amount).unwrap());
    add_total(env, -amount);

    let token_address: Address = env.storage().instance().get(&DataKey::Token).unwrap();
    token::Client::new(env, &token_address).transfer(&env.current_contract_address(), &to, &amount);

    events::withdrawn(env, to, tag, amount);
    check_invariants(env);
}

/// Shared core of transfer_tag/transfer_tag_via_multisig. TotalEscrowed is
/// unaffected — nothing entered or left the vault, only which tag it's
/// earmarked under changed.
fn transfer_tag_one(env: &Env, from_tag: u64, to_tag: u64, amount: i128) {
    if amount <= 0 {
        panic_with_error!(env, Error::InvalidAmount);
    }
    let from_balance = escrow_of(env, from_tag);
    if from_balance < amount {
        panic_with_error!(env, Error::InsufficientEscrowBalance);
    }
    set_escrow(env, from_tag, from_balance.checked_sub(amount).unwrap());
    let to_balance = escrow_of(env, to_tag);
    set_escrow(env, to_tag, to_balance.checked_add(amount).unwrap());

    events::tag_transferred(env, from_tag, to_tag, amount);
    check_invariants(env);
}

fn check_batch_size(env: &Env, len: u32) {
    if len == 0 || len > MAX_BATCH {
        panic_with_error!(env, Error::InvalidBatchSize);
    }
}

/// Working balance for `tag` inside a batch: the in-flight value if the
/// batch already touched it, otherwise what's in storage.
fn working_balance(env: &Env, working: &Map<u64, i128>, tag: u64) -> i128 {
    working.get(tag).unwrap_or_else(|| escrow_of(env, tag))
}

#[contract]
pub struct Vault;

#[contractimpl]
impl Vault {
    pub fn initialize(env: Env, admin: Address, token: Address) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage()
            .instance()
            .set(&DataKey::TotalEscrowed, &0i128);
    }

    /// Hands off control to a new address. Requires the CURRENT admin's
    /// signature, not the incoming one. Since `admin` also gates every
    /// withdraw, this is how the calling contract a vault serves would
    /// change (e.g. after an options_market upgrade to a new contract
    /// address).
    pub fn transfer_admin(env: Env, new_admin: Address) {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        events::admin_transferred(&env, admin, new_admin);
    }

    /// Permissionless alternative to transfer_admin: cross-calls a
    /// deployed Multisig and checks is_approved(action_id) instead of
    /// requiring the current admin's own signature — same pattern as
    /// options_market's and price_oracle's transfer_admin_via_multisig.
    pub fn transfer_admin_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        new_admin: Address,
    ) {
        let multisig = multisig_client::Client::new(&env, &multisig_contract);
        if !multisig.is_approved(&action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        events::admin_transferred(&env, admin, new_admin);
    }

    /// Emergency stop: blocks deposit and withdraw. Both sides, unlike
    /// options_market's pause (which only blocks new exposure, not
    /// winding existing positions down) — a vault holding real funds
    /// should be freezable outright if something's gone wrong, since
    /// there's no "existing position" here that needs an exit path
    /// independent of the vault itself.
    pub fn pause(env: Env) {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &true);
        events::paused(&env);
    }

    pub fn unpause(env: Env) {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &false);
        events::unpaused(&env);
    }

    /// Permissionless alternative to pause(), same rationale and pattern
    /// as options_market's and price_oracle's pause_via_multisig.
    pub fn pause_via_multisig(env: Env, multisig_contract: Address, action_id: u64) {
        let multisig = multisig_client::Client::new(&env, &multisig_contract);
        if !multisig.is_approved(&action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        env.storage().instance().set(&DataKey::Paused, &true);
        events::paused(&env);
    }

    pub fn unpause_via_multisig(env: Env, multisig_contract: Address, action_id: u64) {
        let multisig = multisig_client::Client::new(&env, &multisig_contract);
        if !multisig.is_approved(&action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        env.storage().instance().set(&DataKey::Paused, &false);
        events::unpaused(&env);
    }

    pub fn is_paused(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::Paused)
            .unwrap_or(false)
    }

    /// Pulls `amount` of the vault's token from `from` into the vault's
    /// own balance, crediting `tag`'s escrow ledger. `from` must authorize
    /// the transfer — this contract never moves funds without the source
    /// account's own signature.
    pub fn deposit(env: Env, from: Address, tag: u64, amount: i128) {
        require_not_paused(&env);
        from.require_auth();
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }

        let token_address: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        token::Client::new(&env, &token_address).transfer(
            &from,
            &env.current_contract_address(),
            &amount,
        );

        set_escrow(&env, tag, escrow_of(&env, tag).checked_add(amount).unwrap());
        add_total(&env, amount);

        events::deposited(&env, from, tag, amount);
        check_invariants(&env);
    }

    /// Pays `amount` of `tag`'s escrowed balance out to `to`, debiting the
    /// ledger. Gated on the admin's signature — set at initialize time to
    /// whichever address is trusted to trigger payouts. In the intended
    /// integration, that's the calling contract's own address (e.g.
    /// options_market's), so a contract-to-contract call satisfies
    /// require_auth() through the call itself, the same way any Soroban
    /// contract authorizes its own outgoing calls, without needing a
    /// human signature on every single settlement.
    ///
    /// Panics with InsufficientEscrowBalance if `tag` doesn't have that
    /// much earmarked — this is the check that actually closes the gap:
    /// a withdrawal can never draw down more than was specifically
    /// deposited under this tag, regardless of what the vault's raw token
    /// balance happens to be from OTHER tags' deposits.
    pub fn withdraw(env: Env, tag: u64, to: Address, amount: i128) {
        require_not_paused(&env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        withdraw_one(&env, tag, to, amount);
    }

    /// Permissionless alternative to withdraw: cross-calls a deployed
    /// Multisig and checks is_approved(action_id) instead of requiring
    /// the admin's own signature. Useful for a manual-recovery or
    /// migration path where the intended calling contract itself can't
    /// produce the admin's signature (e.g. it's being replaced), so an
    /// M-of-N-approved payout is the only way to move funds out. Same
    /// InsufficientEscrowBalance cap applies — approval changes who can
    /// call this, not how much any tag is entitled to.
    pub fn withdraw_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        tag: u64,
        to: Address,
        amount: i128,
    ) {
        require_not_paused(&env);
        let multisig = multisig_client::Client::new(&env, &multisig_contract);
        if !multisig.is_approved(&action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        withdraw_one(&env, tag, to, amount);
    }

    /// Moves `amount` of escrow from `from_tag` to `to_tag` without any
    /// token movement at all — a pure ledger reassignment. Meant for
    /// exactly the case options_market's roll_position represents: a
    /// position closes and its replacement opens in the same breath, so
    /// the collateral doesn't need to leave the vault and come back, it
    /// just needs to be re-earmarked under the new position's tag.
    /// Admin-gated, same as withdraw.
    pub fn transfer_tag(env: Env, from_tag: u64, to_tag: u64, amount: i128) {
        require_not_paused(&env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        transfer_tag_one(&env, from_tag, to_tag, amount);
    }

    /// Permissionless alternative to transfer_tag: cross-calls a deployed
    /// Multisig and checks is_approved(action_id) instead of requiring
    /// the admin's own signature. Same rationale as withdraw_via_multisig.
    pub fn transfer_tag_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        from_tag: u64,
        to_tag: u64,
        amount: i128,
    ) {
        require_not_paused(&env);
        let multisig = multisig_client::Client::new(&env, &multisig_contract);
        if !multisig.is_approved(&action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        transfer_tag_one(&env, from_tag, to_tag, amount);
    }

    /// Batched withdraw: one admin auth check for the whole batch, every
    /// entry validated (against in-flight balances, so the same tag twice
    /// is handled correctly) before any storage write or token transfer,
    /// and one token transfer per distinct recipient. All-or-nothing.
    /// Emits one `withdrawn` event per entry, same as sequential withdraws.
    pub fn withdraw_batch(env: Env, ops: Vec<(u64, Address, i128)>) {
        require_not_paused(&env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        check_batch_size(&env, ops.len());

        let mut working: Map<u64, i128> = Map::new(&env);
        let mut payouts: Map<Address, i128> = Map::new(&env);
        let mut total_out: i128 = 0;
        for (tag, to, amount) in ops.iter() {
            if amount <= 0 {
                panic_with_error!(&env, Error::InvalidAmount);
            }
            let balance = working_balance(&env, &working, tag);
            if balance < amount {
                panic_with_error!(&env, Error::InsufficientEscrowBalance);
            }
            working.set(tag, balance - amount);
            let owed = payouts.get(to.clone()).unwrap_or(0);
            payouts.set(to, owed.checked_add(amount).unwrap());
            total_out = total_out.checked_add(amount).unwrap();
        }

        for (tag, balance) in working.iter() {
            set_escrow(&env, tag, balance);
        }
        add_total(&env, -total_out);

        let token_address: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        let token_client = token::Client::new(&env, &token_address);
        for (to, amount) in payouts.iter() {
            token_client.transfer(&env.current_contract_address(), &to, &amount);
        }
        for (tag, to, amount) in ops.iter() {
            events::withdrawn(&env, to, tag, amount);
        }
        check_invariants(&env);
    }

    /// Batched transfer_tag: one admin auth check, every entry validated
    /// against in-flight balances before any write, all-or-nothing. No
    /// token movement. Emits one `tag_transferred` event per entry.
    pub fn transfer_tag_batch(env: Env, ops: Vec<(u64, u64, i128)>) {
        require_not_paused(&env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        check_batch_size(&env, ops.len());

        let mut working: Map<u64, i128> = Map::new(&env);
        for (from_tag, to_tag, amount) in ops.iter() {
            if amount <= 0 {
                panic_with_error!(&env, Error::InvalidAmount);
            }
            let from_balance = working_balance(&env, &working, from_tag);
            if from_balance < amount {
                panic_with_error!(&env, Error::InsufficientEscrowBalance);
            }
            working.set(from_tag, from_balance - amount);
            let to_balance = working_balance(&env, &working, to_tag);
            working.set(to_tag, to_balance.checked_add(amount).unwrap());
        }

        for (tag, balance) in working.iter() {
            set_escrow(&env, tag, balance);
        }
        for (from_tag, to_tag, amount) in ops.iter() {
            events::tag_transferred(&env, from_tag, to_tag, amount);
        }
        check_invariants(&env);
    }

    /// Number of tags currently holding a nonzero escrow balance.
    pub fn get_tag_count(env: Env) -> u32 {
        tag_count(&env)
    }

    /// Page of the ActiveTags index: tags at positions
    /// `cursor..cursor+limit` (limit capped at MAX_PAGE). Order is not
    /// stable across mutations (swap-remove), so paginate against a
    /// single ledger snapshot.
    pub fn get_tags(env: Env, cursor: u32, limit: u32) -> Vec<u64> {
        let (start, end) = page_bounds(&env, cursor, limit);
        let mut tags = Vec::new(&env);
        for i in start..end {
            tags.push_back(env.storage().persistent().get(&DataKey::TagAt(i)).unwrap());
        }
        tags
    }

    /// Sum of escrow balances for the tags at positions
    /// `cursor..cursor+limit`, plus the cursor to pass next. Paging until
    /// `next_cursor == get_tag_count()` and adding the partial sums must
    /// equal get_total_escrowed() — see the README's reconciliation example.
    pub fn verify_ledger(env: Env, cursor: u32, limit: u32) -> (i128, u32) {
        let (start, end) = page_bounds(&env, cursor, limit);
        let mut partial_sum: i128 = 0;
        for i in start..end {
            let tag: u64 = env.storage().persistent().get(&DataKey::TagAt(i)).unwrap();
            partial_sum = partial_sum.checked_add(escrow_of(&env, tag)).unwrap();
        }
        (partial_sum, end)
    }

    pub fn balance_of(env: Env, tag: u64) -> i128 {
        escrow_of(&env, tag)
    }

    pub fn get_total_escrowed(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::TotalEscrowed)
            .unwrap()
    }

    pub fn get_admin(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Admin).unwrap()
    }

    pub fn get_token(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Token).unwrap()
    }

    /// Recovers tokens that landed on the vault's own address OUTSIDE
    /// deposit() — e.g. a direct token transfer sent straight to this
    /// contract's address by mistake, rather than through deposit(),
    /// which is the only path that actually credits a tag's ledger. Those
    /// tokens sit in the vault's real balance but aren't accounted for
    /// under any tag, so they'd otherwise be stuck forever: no tag's
    /// withdraw could ever reach them (withdraw is capped at that tag's
    /// OWN escrowed balance), and get_total_escrowed's ledger sum would
    /// permanently under-report the vault's actual token balance.
    pub fn sweep_untagged(env: Env, to: Address) -> i128 {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        let token_address: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        let token_client = token::Client::new(&env, &token_address);
        let actual_balance = token_client.balance(&env.current_contract_address());
        let total_escrowed: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalEscrowed)
            .unwrap();

        let untagged = actual_balance.checked_sub(total_escrowed).unwrap();
        if untagged <= 0 {
            panic_with_error!(&env, Error::NoUntaggedFunds);
        }

        token_client.transfer(&env.current_contract_address(), &to, &untagged);
        events::swept_untagged(&env, to, untagged);
        check_invariants(&env);
        untagged
    }

    /// Permissionless alternative to sweep_untagged: cross-calls a
    /// deployed Multisig and checks is_approved(action_id) instead of
    /// requiring the admin's own signature. Same rationale as
    /// withdraw_via_multisig.
    pub fn sweep_untagged_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        to: Address,
    ) -> i128 {
        let multisig = multisig_client::Client::new(&env, &multisig_contract);
        if !multisig.is_approved(&action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }

        let token_address: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        let token_client = token::Client::new(&env, &token_address);
        let actual_balance = token_client.balance(&env.current_contract_address());
        let total_escrowed: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalEscrowed)
            .unwrap();

        let untagged = actual_balance.checked_sub(total_escrowed).unwrap();
        if untagged <= 0 {
            panic_with_error!(&env, Error::NoUntaggedFunds);
        }

        token_client.transfer(&env.current_contract_address(), &to, &untagged);
        events::swept_untagged(&env, to, untagged);
        check_invariants(&env);
        untagged
    }
}
