#![no_std]

//! Zenith Timelock — delays every privileged call by at least `min_delay`
//! so users can react (or a guardian can cancel) before it lands.
//!
//! A single proposer schedules batches of contract calls; once ready,
//! anyone may execute them (an "open executor", as in OpenZeppelin's
//! TimelockController). Execution being permissionless is also what lets a
//! governor proposal target the governor itself: the governor never sits
//! in the call stack, so Soroban's no-re-entry rule is not tripped.
//! Contracts that hand their admin to this timelock accept those calls
//! because the timelock is the direct invoker, so `admin.require_auth()`
//! passes without any signature.
//!
//! Soroban forbids a contract re-entering itself, so the timelock's own
//! configuration (`set_proposer`, `set_min_delay`, `lock_in`) is changed by
//! scheduling an operation whose target is the timelock itself; `execute`
//! recognises those and applies them in place instead of cross-calling.
//!
//! Until `lock_in`, the guardian can cancel any pending operation and
//! reassign the proposer (the rollback path of the governance handover,
//! see docs/governance-handover.md). After `lock_in` the guardian is gone
//! for good.

use soroban_sdk::{
    contract, contractimpl, panic_with_error, symbol_short, Address, BytesN, Env, Symbol,
    TryFromVal, Val, Vec,
};

#[cfg(test)]
mod test;

mod error;
mod events;
mod types;

use error::Error;
use types::DataKey;
pub use types::Operation;

#[contract]
pub struct Timelock;

fn proposer(env: &Env) -> Address {
    env.storage().instance().get(&DataKey::Proposer).unwrap()
}

fn set_proposer(env: &Env, new: Address) {
    events::proposer_set(env, proposer(env), new.clone());
    env.storage().instance().set(&DataKey::Proposer, &new);
}

fn arg<T: TryFromVal<Env, Val>>(env: &Env, args: &Vec<Val>, i: u32) -> T {
    args.get(i)
        .and_then(|v| T::try_from_val(env, &v).ok())
        .unwrap_or_else(|| panic_with_error!(env, Error::InvalidOperation))
}

/// Applies an operation targeting the timelock itself.
fn self_call(env: &Env, func: &Symbol, args: &Vec<Val>) {
    if *func == symbol_short!("set_prop") {
        set_proposer(env, arg(env, args, 0));
    } else if *func == symbol_short!("set_delay") {
        let delay: u64 = arg(env, args, 0);
        env.storage().instance().set(&DataKey::MinDelay, &delay);
    } else if *func == symbol_short!("lock_in") {
        env.storage().instance().remove(&DataKey::Guardian);
        events::locked_in(env);
    } else {
        panic_with_error!(env, Error::UnknownSelfCall);
    }
}

#[contractimpl]
impl Timelock {
    pub fn initialize(
        env: Env,
        proposer: Address,
        guardian: Address,
        min_delay: u64,
        grace_period: u64,
    ) {
        if env.storage().instance().has(&DataKey::Proposer) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Proposer, &proposer);
        env.storage().instance().set(&DataKey::Guardian, &guardian);
        env.storage().instance().set(&DataKey::MinDelay, &min_delay);
        env.storage()
            .instance()
            .set(&DataKey::GracePeriod, &grace_period);
    }

    /// Proposer schedules a batch of calls, executable from
    /// `now + delay` until `now + delay + grace_period`. Calls targeting
    /// the timelock itself may use: `set_prop(Address)`,
    /// `set_delay(u64)`, `lock_in()`.
    pub fn schedule(
        env: Env,
        op_id: BytesN<32>,
        targets: Vec<Address>,
        fns: Vec<Symbol>,
        args: Vec<Vec<Val>>,
        delay: u64,
    ) -> u64 {
        proposer(&env).require_auth();
        if targets.is_empty() || targets.len() != fns.len() || targets.len() != args.len() {
            panic_with_error!(&env, Error::InvalidOperation);
        }
        if delay < Self::get_min_delay(env.clone()) {
            panic_with_error!(&env, Error::DelayTooShort);
        }
        let key = DataKey::Operation(op_id.clone());
        if env.storage().persistent().has(&key) {
            panic_with_error!(&env, Error::OperationExists);
        }
        let ready_at = env.ledger().timestamp() + delay;
        env.storage().persistent().set(
            &key,
            &Operation {
                targets,
                fns,
                args,
                ready_at,
                done: false,
            },
        );
        events::scheduled(&env, op_id, ready_at);
        ready_at
    }

    /// Anyone executes a ready, unexpired operation.
    pub fn execute(env: Env, op_id: BytesN<32>) {
        let key = DataKey::Operation(op_id.clone());
        let mut op: Operation = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, Error::OperationNotFound));
        if op.done {
            panic_with_error!(&env, Error::AlreadyDone);
        }
        let now = env.ledger().timestamp();
        if now < op.ready_at {
            panic_with_error!(&env, Error::NotReady);
        }
        if now > op.ready_at + Self::get_grace_period(env.clone()) {
            panic_with_error!(&env, Error::Expired);
        }
        op.done = true;
        env.storage().persistent().set(&key, &op);

        let this = env.current_contract_address();
        for i in 0..op.targets.len() {
            let target = op.targets.get(i).unwrap();
            let func = op.fns.get(i).unwrap();
            let args = op.args.get(i).unwrap();
            if target == this {
                self_call(&env, &func, &args);
            } else {
                env.invoke_contract::<Val>(&target, &func, args);
            }
        }
        events::executed(&env, op_id);
    }

    /// Proposer or (before lock-in) guardian cancels a pending operation.
    pub fn cancel(env: Env, caller: Address, op_id: BytesN<32>) {
        caller.require_auth();
        if caller != proposer(&env) && Some(caller.clone()) != Self::get_guardian(env.clone()) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        let key = DataKey::Operation(op_id.clone());
        let op: Operation = env
            .storage()
            .persistent()
            .get(&key)
            .unwrap_or_else(|| panic_with_error!(&env, Error::OperationNotFound));
        if op.done {
            panic_with_error!(&env, Error::AlreadyDone);
        }
        env.storage().persistent().remove(&key);
        events::canceled(&env, op_id, caller);
    }

    /// Guardian-only rollback: reassigns the proposer immediately, e.g.
    /// from the governor back to the multisig. Unavailable after lock-in.
    pub fn guardian_set_proposer(env: Env, new_proposer: Address) {
        let guardian = Self::get_guardian(env.clone())
            .unwrap_or_else(|| panic_with_error!(&env, Error::LockedIn));
        guardian.require_auth();
        set_proposer(&env, new_proposer);
    }

    // ── Views ────────────────────────────────────────────────────────────────

    pub fn get_proposer(env: Env) -> Address {
        proposer(&env)
    }

    /// None once locked in.
    pub fn get_guardian(env: Env) -> Option<Address> {
        env.storage().instance().get(&DataKey::Guardian)
    }

    pub fn is_locked_in(env: Env) -> bool {
        !env.storage().instance().has(&DataKey::Guardian)
    }

    pub fn get_min_delay(env: Env) -> u64 {
        env.storage().instance().get(&DataKey::MinDelay).unwrap()
    }

    pub fn get_grace_period(env: Env) -> u64 {
        env.storage().instance().get(&DataKey::GracePeriod).unwrap()
    }

    /// True once the operation has been executed.
    pub fn is_done(env: Env, op_id: BytesN<32>) -> bool {
        Self::get_operation(env, op_id).is_some_and(|op| op.done)
    }

    pub fn get_operation(env: Env, op_id: BytesN<32>) -> Option<Operation> {
        env.storage().persistent().get(&DataKey::Operation(op_id))
    }
}
