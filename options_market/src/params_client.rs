//! Cross-contract client for the params registry, same pattern and same
//! reason as price_oracle_client.rs: contractimport! against params'
//! compiled wasm, not a normal source dependency.
//!
//! Requires params' wasm to already be built (`cd ../params && cargo
//! build --target wasm32-unknown-unknown --release`) before this crate
//! can compile at all.
soroban_sdk::contractimport!(
    file = "../params/target/wasm32-unknown-unknown/release/zenith_params.wasm"
);
