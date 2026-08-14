use soroban_sdk::{contracttype, Address, Symbol};

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Paused,
    MaxStaleness,
    Feeders,
    PriceReport(Symbol, Address),
    AggregatedPrice(Symbol),
}
