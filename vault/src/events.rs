use soroban_sdk::{Address, Env, Symbol};

pub fn deposited(env: &Env, from: Address, tag: u64, amount: i128) {
    env.events()
        .publish((Symbol::new(env, "deposited"), from, tag), amount);
}

pub fn withdrawn(env: &Env, to: Address, tag: u64, amount: i128) {
    env.events()
        .publish((Symbol::new(env, "withdrawn"), to, tag), amount);
}

pub fn admin_transferred(env: &Env, old_admin: Address, new_admin: Address) {
    env.events().publish(
        (Symbol::new(env, "admin_transferred"),),
        (old_admin, new_admin),
    );
}

pub fn paused(env: &Env) {
    env.events().publish((Symbol::new(env, "paused"),), ());
}

pub fn unpaused(env: &Env) {
    env.events().publish((Symbol::new(env, "unpaused"),), ());
}
