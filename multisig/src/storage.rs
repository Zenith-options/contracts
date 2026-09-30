use soroban_sdk::{Address, Env, Vec};

use crate::types::DataKey;

// Signers (issue #101): dedicated persistent entries plus an O(1) membership
// map, instead of a `Signers` list loaded in full from instance storage on
// every invocation and checked with `Vec::contains`.

pub fn is_signer(env: &Env, signer: &Address) -> bool {
    env.storage()
        .persistent()
        .get(&DataKey::IsSigner(signer.clone()))
        .unwrap_or(false)
}

pub fn signer_list(env: &Env) -> Vec<Address> {
    env.storage()
        .persistent()
        .get(&DataKey::Signers)
        .unwrap_or(Vec::new(env))
}

pub fn add_signer(env: &Env, signer: &Address) {
    env.storage()
        .persistent()
        .set(&DataKey::IsSigner(signer.clone()), &true);
    let mut list = signer_list(env);
    list.push_back(signer.clone());
    env.storage().persistent().set(&DataKey::Signers, &list);
}

pub fn remove_signer(env: &Env, signer: &Address) {
    env.storage()
        .persistent()
        .remove(&DataKey::IsSigner(signer.clone()));
    let list = signer_list(env);
    if let Some(idx) = list.iter().position(|s| &s == signer) {
        let mut list = list;
        list.remove(idx as u32);
        env.storage().persistent().set(&DataKey::Signers, &list);
    }
}
