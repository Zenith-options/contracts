use soroban_sdk::{Address, BytesN, Env, Symbol};

pub fn registered(
    env: &Env,
    proposer: Address,
    action_id: BytesN<32>,
    description_hash: BytesN<32>,
) {
    env.events().publish(
        (Symbol::new(env, "registered"), proposer, action_id),
        description_hash,
    );
}

pub fn approved(env: &Env, signer: Address, action_id: BytesN<32>, description_hash: BytesN<32>) {
    env.events().publish(
        (Symbol::new(env, "approved"), signer, action_id),
        description_hash,
    );
}

pub fn revoked(env: &Env, signer: Address, action_id: BytesN<32>, description_hash: BytesN<32>) {
    env.events().publish(
        (Symbol::new(env, "revoked"), signer, action_id),
        description_hash,
    );
}

pub fn reset(env: &Env, action_id: BytesN<32>, description_hash: BytesN<32>) {
    env.events()
        .publish((Symbol::new(env, "reset"), action_id), description_hash);
}

pub fn expired(env: &Env, action_id: BytesN<32>, description_hash: BytesN<32>) {
    env.events()
        .publish((Symbol::new(env, "expired"), action_id), description_hash);
}

pub fn executed(env: &Env, action_id: BytesN<32>, description_hash: BytesN<32>) {
    env.events()
        .publish((Symbol::new(env, "executed"), action_id), description_hash);
}
