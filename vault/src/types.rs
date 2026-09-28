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
    /// Immutable after initialize: how long (cumulatively) the vault may
    /// stay paused before beneficiaries can emergency_withdraw.
    MaxPauseDuration,
    /// Start of the current (or most recent) pause.
    PausedAt,
    /// Pause seconds accrued by earlier pauses; reset only after the
    /// vault stays unpaused for a full MaxPauseDuration.
    PauseAccrued,
    UnpausedAt,
    /// First depositor to a tag — the only address that can register
    /// its beneficiary.
    TagOwner(u64),
    /// Who may emergency_withdraw a tag's balance after a long pause.
    Beneficiary(u64),
}
