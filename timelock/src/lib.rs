#![no_std]

//! Zenith Timelock — holds the admin role on the other Zenith contracts
//! and only executes an operation (a batch of contract calls) once a
//! minimum delay has passed since it was scheduled, so users always get
//! an exit window before a parameter or code change lands. Modelled on
//! OpenZeppelin's TimelockController: proposer / executor / canceller
//! roles, predecessor dependencies, and an operation id of
//! `sha256(xdr((calls, predecessor, salt)))`.
//!
//! Soroban forbids a contract re-entering itself, so the timelock's own
//! administration (`update_delay`, `grant_role`, `revoke_role`) can't be
//! a normal call to its own address. Instead, a scheduled call whose
//! `target` is the timelock itself is dispatched internally by
//! `execute` — which makes those changes reachable *only* through a
//! delayed operation.

use soroban_sdk::{
    contract, contractimpl, panic_with_error, xdr::ToXdr, Address, BytesN, Env, Symbol, TryFromVal,
    Val, Vec,
};

#[cfg(test)]
mod test;

mod error;
mod events;
mod types;

use error::Error;
use types::DataKey;
pub use types::{Call, Role};

const DONE: u64 = 1;

fn timestamp_of(env: &Env, id: &BytesN<32>) -> u64 {
    env.storage()
        .persistent()
        .get(&DataKey::Timestamp(id.clone()))
        .unwrap_or(0)
}

fn has_role(env: &Env, role: Role, account: &Address) -> bool {
    env.storage()
        .persistent()
        .get(&DataKey::Role(role, account.clone()))
        .unwrap_or(false)
}

fn require_role(env: &Env, role: Role, account: &Address) {
    account.require_auth();
    if !has_role(env, role, account) {
        panic_with_error!(env, Error::Unauthorized);
    }
}

fn set_role(env: &Env, role: Role, account: Address, granted: bool) {
    let key = DataKey::Role(role, account.clone());
    if granted {
        env.storage().persistent().set(&key, &true);
    } else {
        env.storage().persistent().remove(&key);
    }
    events::role_changed(env, role, account, granted);
}

fn arg<T: TryFromVal<Env, Val>>(env: &Env, args: &Vec<Val>, i: u32) -> T {
    let v = args
        .get(i)
        .unwrap_or_else(|| panic_with_error!(env, Error::UnknownSelfCall));
    T::try_from_val(env, &v).unwrap_or_else(|_| panic_with_error!(env, Error::UnknownSelfCall))
}

/// Self-administration, reachable only from `execute`.
fn self_call(env: &Env, call: &Call) {
    let f = call.function.clone();
    if f == Symbol::new(env, "update_delay") {
        let new_delay: u64 = arg(env, &call.args, 0);
        let old_delay: u64 = env.storage().instance().get(&DataKey::MinDelay).unwrap();
        env.storage().instance().set(&DataKey::MinDelay, &new_delay);
        events::min_delay_changed(env, old_delay, new_delay);
    } else if f == Symbol::new(env, "grant_role") || f == Symbol::new(env, "revoke_role") {
        let role: Role = arg(env, &call.args, 0);
        let account: Address = arg(env, &call.args, 1);
        set_role(env, role, account, f == Symbol::new(env, "grant_role"));
    } else if f == Symbol::new(env, "set_open_executor") {
        let open: bool = arg(env, &call.args, 0);
        env.storage().instance().set(&DataKey::OpenExecutor, &open);
    } else {
        panic_with_error!(env, Error::UnknownSelfCall);
    }
}

#[contract]
pub struct Timelock;

#[contractimpl]
impl Timelock {
    /// `executors` empty means execution is open to anyone. There is no
    /// admin: every later role or delay change goes through the timelock.
    pub fn initialize(
        env: Env,
        min_delay: u64,
        proposers: Vec<Address>,
        executors: Vec<Address>,
        cancellers: Vec<Address>,
    ) {
        if env.storage().instance().has(&DataKey::MinDelay) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::MinDelay, &min_delay);
        env.storage()
            .instance()
            .set(&DataKey::OpenExecutor, &executors.is_empty());
        for a in proposers.iter() {
            set_role(&env, Role::Proposer, a, true);
        }
        for a in executors.iter() {
            set_role(&env, Role::Executor, a, true);
        }
        for a in cancellers.iter() {
            set_role(&env, Role::Canceller, a, true);
        }
    }

    pub fn hash_operation(
        env: Env,
        calls: Vec<Call>,
        predecessor: Option<BytesN<32>>,
        salt: BytesN<32>,
    ) -> BytesN<32> {
        let bytes = (calls, predecessor, salt).to_xdr(&env);
        env.crypto().sha256(&bytes).into()
    }

    /// Schedules `calls` to become executable `delay` seconds from now.
    /// `delay` must be at least the current min delay. A done operation
    /// can't be scheduled again — use a fresh `salt`.
    pub fn schedule(
        env: Env,
        proposer: Address,
        calls: Vec<Call>,
        predecessor: Option<BytesN<32>>,
        salt: BytesN<32>,
        delay: u64,
    ) -> BytesN<32> {
        require_role(&env, Role::Proposer, &proposer);
        if calls.is_empty() {
            panic_with_error!(&env, Error::EmptyOperation);
        }
        let min_delay: u64 = env.storage().instance().get(&DataKey::MinDelay).unwrap();
        if delay < min_delay {
            panic_with_error!(&env, Error::InsufficientDelay);
        }
        let id = Self::hash_operation(env.clone(), calls, predecessor, salt);
        if timestamp_of(&env, &id) != 0 {
            panic_with_error!(&env, Error::AlreadyScheduled);
        }
        // Never store 0/1 (unset/done) as a real ready time.
        let ready_at = env
            .ledger()
            .timestamp()
            .checked_add(delay)
            .unwrap()
            .max(DONE + 1);
        env.storage()
            .persistent()
            .set(&DataKey::Timestamp(id.clone()), &ready_at);
        events::scheduled(&env, id.clone(), ready_at);
        id
    }

    /// Runs every call in a ready operation, in order. Any reverting call
    /// reverts the whole operation. `executor` is only checked when
    /// execution isn't open.
    pub fn execute(
        env: Env,
        executor: Address,
        calls: Vec<Call>,
        predecessor: Option<BytesN<32>>,
        salt: BytesN<32>,
    ) {
        let open: bool = env
            .storage()
            .instance()
            .get(&DataKey::OpenExecutor)
            .unwrap_or(false);
        if !open {
            require_role(&env, Role::Executor, &executor);
        }
        let id = Self::hash_operation(env.clone(), calls.clone(), predecessor.clone(), salt);
        if !Self::is_operation_ready(env.clone(), id.clone()) {
            panic_with_error!(&env, Error::NotReady);
        }
        if let Some(p) = predecessor {
            if timestamp_of(&env, &p) != DONE {
                panic_with_error!(&env, Error::PredecessorNotDone);
            }
        }
        env.storage()
            .persistent()
            .set(&DataKey::Timestamp(id.clone()), &DONE);

        let this = env.current_contract_address();
        for call in calls.iter() {
            if call.target == this {
                self_call(&env, &call);
            } else {
                env.invoke_contract::<Val>(&call.target, &call.function, call.args.clone());
            }
        }
        events::executed(&env, id);
    }

    pub fn cancel(env: Env, canceller: Address, id: BytesN<32>) {
        require_role(&env, Role::Canceller, &canceller);
        if !Self::is_operation_pending(env.clone(), id.clone()) {
            panic_with_error!(&env, Error::NotPending);
        }
        env.storage()
            .persistent()
            .remove(&DataKey::Timestamp(id.clone()));
        events::cancelled(&env, id);
    }

    /// 0 = unknown, 1 = done, otherwise the ready-at ledger timestamp.
    pub fn get_timestamp(env: Env, id: BytesN<32>) -> u64 {
        timestamp_of(&env, &id)
    }

    pub fn is_operation(env: Env, id: BytesN<32>) -> bool {
        timestamp_of(&env, &id) != 0
    }

    pub fn is_operation_pending(env: Env, id: BytesN<32>) -> bool {
        timestamp_of(&env, &id) > DONE
    }

    pub fn is_operation_ready(env: Env, id: BytesN<32>) -> bool {
        let ts = timestamp_of(&env, &id);
        ts > DONE && ts <= env.ledger().timestamp()
    }

    pub fn is_operation_done(env: Env, id: BytesN<32>) -> bool {
        timestamp_of(&env, &id) == DONE
    }

    pub fn get_min_delay(env: Env) -> u64 {
        env.storage().instance().get(&DataKey::MinDelay).unwrap()
    }

    pub fn has_role(env: Env, role: Role, account: Address) -> bool {
        has_role(&env, role, &account)
    }

    pub fn is_open_executor(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::OpenExecutor)
            .unwrap_or(false)
    }
}

/// Helper for building a `Call` off-chain or in tests.
pub fn call(env: &Env, target: &Address, function: &str, args: Vec<Val>) -> Call {
    Call {
        target: target.clone(),
        function: Symbol::new(env, function),
        args,
    }
}
