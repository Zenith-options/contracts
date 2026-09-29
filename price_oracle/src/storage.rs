use soroban_sdk::{Address, Env, Vec};

use crate::types::DataKey;

// Feeders (issue #101): moved out of instance storage — which is loaded in
// full on every invocation, including the hot `report_price`/`get_price`
// paths — into dedicated persistent entries, with an O(1) membership map
// instead of `Vec::contains`.

pub fn is_feeder(env: &Env, feeder: &Address) -> bool {
    env.storage()
        .persistent()
        .get(&DataKey::IsFeeder(feeder.clone()))
        .unwrap_or(false)
}

pub fn feeder_list(env: &Env) -> Vec<Address> {
    env.storage()
        .persistent()
        .get(&DataKey::Feeders)
        .unwrap_or(Vec::new(env))
}

pub fn add_feeder(env: &Env, feeder: &Address) {
    env.storage()
        .persistent()
        .set(&DataKey::IsFeeder(feeder.clone()), &true);
    let mut list = feeder_list(env);
    list.push_back(feeder.clone());
    env.storage().persistent().set(&DataKey::Feeders, &list);
}

pub fn remove_feeder(env: &Env, feeder: &Address) {
    env.storage()
        .persistent()
        .remove(&DataKey::IsFeeder(feeder.clone()));
    let list = feeder_list(env);
    if let Some(idx) = list.iter().position(|f| &f == feeder) {
        let mut list = list;
        list.remove(idx as u32);
        env.storage().persistent().set(&DataKey::Feeders, &list);
    }
}
