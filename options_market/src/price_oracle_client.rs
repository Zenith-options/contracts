//! Cross-contract client for price_oracle, generated from its compiled
//! wasm's own interface spec via contractimport! — NOT from a normal
//! source dependency on the price_oracle crate. A normal dependency would
//! pull in price_oracle's actual #[contractimpl] functions (get_admin,
//! pause, transfer_admin, ...) and link them into THIS contract's wasm
//! too, colliding with options_market's own functions of the same name at
//! link time. contractimport! avoids that: it only generates the thin
//! Client type needed to invoke the other contract by address at runtime.
//!
//! Requires price_oracle's wasm in the shared workspace target/ before
//! this crate can compile at all, since the macro reads that file at
//! compile time. `cargo xtask build` builds it first (see
//! `[package.metadata.zenith] wasm-deps` in ../Cargo.toml).
soroban_sdk::contractimport!(
    file = "../target/wasm32-unknown-unknown/release/zenith_price_oracle.wasm"
);
