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
    Feeders,
    PriceReport(Symbol, Address),
    AggregatedPrice(Symbol),
}
