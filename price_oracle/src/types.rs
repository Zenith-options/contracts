use soroban_sdk::{contracttype, Address, Symbol};

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Paused,
    MaxStaleness,
    /// Minimum number of currently-fresh feeder reports get_price requires
    /// before it returns an aggregate at all. Defaults to 1 at initialize
    /// (the original behavior: any single fresh report is enough).
    MinReports,
    /// O(1) membership check — see issue #101. `Feeders` below stays around
    /// only for enumeration (e.g. `get_feeders`), and now lives in
    /// persistent storage instead of instance storage.
    IsFeeder(Address),
    Feeders,
    PriceReport(Symbol, Address),
    AggregatedPrice(Symbol),
}
