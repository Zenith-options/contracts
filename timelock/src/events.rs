use soroban_sdk::{Address, BytesN, Env, Symbol};

use crate::types::Role;

pub fn scheduled(env: &Env, id: BytesN<32>, ready_at: u64) {
    env.events()
        .publish((Symbol::new(env, "scheduled"), id), ready_at);
}

pub fn executed(env: &Env, id: BytesN<32>) {
    env.events().publish((Symbol::new(env, "executed"), id), ());
}

pub fn cancelled(env: &Env, id: BytesN<32>) {
    env.events()
        .publish((Symbol::new(env, "cancelled"), id), ());
}

pub fn min_delay_changed(env: &Env, old_delay: u64, new_delay: u64) {
    env.events().publish(
        (Symbol::new(env, "min_delay_changed"),),
        (old_delay, new_delay),
    );
}

pub fn role_changed(env: &Env, role: Role, account: Address, granted: bool) {
    env.events()
        .publish((Symbol::new(env, "role_changed"), role, account), granted);
}
