# Zenith Protocol Formal Threat Model & Adversarial Attack Suite

This document defines the formal threat model, security invariants, trust boundaries, and adversarial test scenarios across Zenith options protocol contracts (`options_market`, `price_oracle`, `vault`, `multisig`).

---

## 1. Trust Boundaries & Privileged Actors

- **Untrusted Actors**: Any external caller executing permissionless entrypoints (`deposit`, `mint_options`, `exercise`, `settle`).
- **Oracle Feeders**: Whitelisted price reporters providing asset spot prices under round-based consensus.
- **Protocol Guardian**: Restricted role with pause-only powers and automatic time-based pause expiration (#18).
- **Admin & Multisig**: Quorum-governed multisig requiring threshold signatures bound to specific action hashes (#8, #10).

---

## 2. Core Security Invariants & Attack Mitigations

### 2.1 Rate-Limited Outflows & Circuit Breakers (Closes #19)
- **Attack Vector**: Rapid liquidation or reentrancy exploit attempting to drain vault capital in a single window.
- **Invariant**: Per-window cumulative outflow limits enforce an automated withdrawal circuit breaker on both `vault` and `options_market`.

### 2.2 Protocol Guardian & Auto-Expiring Pauses (Closes #18)
- **Attack Vector**: Indefinite admin lockout or griefing via permanent contract pause.
- **Invariant**: Pauses automatically expire after `MAX_PAUSE_DURATION` (e.g. 24 hours) unless ratified by multisig.

### 2.3 Granular Access Control (`zenith-access`) (Closes #17)
- **Attack Vector**: Single admin private key compromise exposing all contracts.
- **Invariant**: Strict separation between `GUARDIAN`, `ORACLE_ADMIN`, `FEE_MANAGER`, and `EMERGENCY_SETTLER` roles.

### 2.4 Adversarial Scenario Test Invariants (Closes #16)
- **Flash-Loan Resistance**: Settlement price calculation cannot be manipulated within the same ledger sequence.
- **Solvency**: Total collateral in vault must always equal or exceed the protocol's aggregate option liabilities.
