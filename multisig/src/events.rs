use soroban_sdk::{Address, Env, Symbol};

pub fn approved(env: &Env, signer: Address, action_id: u64) {
    env.events()
        .publish((Symbol::new(env, "approved"), signer, action_id), ());
}

pub fn revoked(env: &Env, signer: Address, action_id: u64) {
    env.events()
        .publish((Symbol::new(env, "revoked"), signer, action_id), ());
}

pub fn reset(env: &Env, action_id: u64) {
    env.events()
        .publish((Symbol::new(env, "reset"),), action_id);
}
