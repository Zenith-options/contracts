use soroban_sdk::{Env, Symbol};

pub fn param_defined(env: &Env, key: Symbol, value: i128, min: i128, max: i128) {
    env.events()
        .publish((Symbol::new(env, "param_defined"), key), (value, min, max));
}

pub fn param_updated(env: &Env, key: Symbol, old: i128, new: i128) {
    env.events()
        .publish((Symbol::new(env, "param_updated"), key), (old, new));
}

pub fn bounds_proposed(env: &Env, key: Symbol, min: i128, max: i128, eta: u64) {
    env.events()
        .publish((Symbol::new(env, "bounds_proposed"), key), (min, max, eta));
}

pub fn bounds_updated(env: &Env, key: Symbol, min: i128, max: i128) {
    env.events()
        .publish((Symbol::new(env, "bounds_updated"), key), (min, max));
}

pub fn bounds_cancelled(env: &Env, key: Symbol) {
    env.events()
        .publish((Symbol::new(env, "bounds_cancelled"), key), ());
}
