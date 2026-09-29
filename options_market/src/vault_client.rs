//! Cross-contract client for vault, same pattern and same reason as
//! multisig_client.rs and price_oracle_client.rs: contractimport! against
//! vault's compiled wasm, not a normal source dependency, to avoid linking
//! vault's own #[contractimpl] functions into options_market's wasm and
//! colliding on shared names (pause/transfer_admin again).
//!
//! Requires vault's wasm to already be built (`cd ../vault && cargo build
//! --target wasm32-unknown-unknown --release`) before this crate can
//! compile at all — see the repo README for the build order this implies.
soroban_sdk::contractimport!(
    file = "../vault/target/wasm32-unknown-unknown/release/zenith_vault.wasm"
);

use soroban_sdk::{Env, Symbol};

/// options_market's namespaced vault tag for a series' escrow:
/// `(this contract, "series", series_id)`.
pub fn series_tag(env: &Env, series_id: u64) -> Tag {
    Tag {
        owner: env.current_contract_address(),
        kind: Symbol::new(env, "series"),
        id: series_id,
    }
}

/// `(this contract, "position", position_id)` — reserved for
/// per-position custody, so it can never collide with a series tag that
/// happens to share the same numeric id.
#[allow(dead_code)]
pub fn position_tag(env: &Env, position_id: u64) -> Tag {
    Tag {
        owner: env.current_contract_address(),
        kind: Symbol::new(env, "position"),
        id: position_id,
    }
}
