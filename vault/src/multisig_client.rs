//! Cross-contract client for multisig, generated from its compiled
//! wasm's own interface spec via contractimport! — same reasoning as
//! options_market's and price_oracle's src/multisig_client.rs: a normal
//! source dependency would link multisig's own #[contractimpl]
//! functions into this contract's wasm and collide on shared names
//! (pause, transfer_admin).
//!
//! Requires multisig's wasm to already be built (`cd ../multisig &&
//! cargo build --target wasm32-unknown-unknown --release`) before this
//! crate can compile at all — see the repo README for the build order
//! this implies.
soroban_sdk::contractimport!(
    file = "../multisig/target/wasm32-unknown-unknown/release/zenith_multisig.wasm"
);
