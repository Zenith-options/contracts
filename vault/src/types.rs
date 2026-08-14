use soroban_sdk::contracttype;

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Token,
    Paused,
    /// Per-tag escrow ledger. `tag` is caller-defined — a calling contract
    /// (e.g. options_market) would use its own position_id, so the vault
    /// can tell "funds earmarked for position 7" apart from "funds
    /// earmarked for position 8" instead of pooling everything into one
    /// undifferentiated balance.
    Escrow(u64),
    TotalEscrowed,
}
