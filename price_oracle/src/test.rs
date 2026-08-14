#![cfg(test)]

use crate::{PriceOracle, PriceOracleClient};
use soroban_sdk::{testutils::Address as _, Address, Env};

#[test]
fn initialize_sets_admin() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let contract_id = env.register_contract(None, PriceOracle);
    let client = PriceOracleClient::new(&env, &contract_id);
    client.initialize(&admin);
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")] // AlreadyInitialized
fn initialize_twice_panics() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let contract_id = env.register_contract(None, PriceOracle);
    let client = PriceOracleClient::new(&env, &contract_id);
    client.initialize(&admin);
    client.initialize(&admin);
}
