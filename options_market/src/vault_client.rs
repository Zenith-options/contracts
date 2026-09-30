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
