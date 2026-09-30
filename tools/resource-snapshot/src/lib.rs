//! Resource-budget snapshots for contract test suites.
//!
//! A `test_resources` module in each contract crate calls [`measure`]
//! around exactly one contract invocation, then [`Suite::check`] compares
//! the result with `<crate>/snapshots/resources/<scenario>.json`:
//!
//! - any metric above `snapshot * (1 + tolerance_pct / 100)` fails the
//!   test (default tolerance [`DEFAULT_TOLERANCE_PCT`], configurable per
//!   scenario with [`Suite::check_with_tolerance`]);
//! - a missing snapshot, or one recorded in a different execution mode,
//!   fails too;
//! - `UPDATE_SNAPSHOTS=1` rewrites the snapshots instead of comparing.
//!
//! Every run also writes the fresh numbers to
//! `<crate>/target/resource-report/<scenario>.json`, which
//! `scripts/resource-diff` turns into the CI summary table.
//!
//! **Native vs wasm.** Budget numbers differ a lot between a contract
//! registered natively (`register_contract`) and as wasm
//! (`register_contract_wasm`): native runs skip VM instantiation and
//! undercount instructions. Each snapshot records its `mode`, and a
//! comparison across modes is refused. CI measures in `wasm` mode.
//!
//! **Footprint.** Read/write entries and bytes come from the host's
//! recording footprint, reset before the measured call, so they cover
//! exactly that invocation. Bytes are the XDR size of each ledger entry
//! (the post-call entry for writes), matching how Soroban charges
//! read/write bytes. A deleted entry counts as a write of 0 bytes.

use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use soroban_sdk::{
    xdr::{Limits, WriteXdr},
    Env,
};

pub const DEFAULT_TOLERANCE_PCT: f64 = 5.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Metrics {
    pub cpu_insns: u64,
    pub mem_bytes: u64,
    pub read_entries: u64,
    pub write_entries: u64,
    pub read_bytes: u64,
    pub write_bytes: u64,
}

impl Metrics {
    pub fn fields(&self) -> [(&'static str, u64); 6] {
        [
            ("cpu_insns", self.cpu_insns),
            ("mem_bytes", self.mem_bytes),
            ("read_entries", self.read_entries),
            ("write_entries", self.write_entries),
            ("read_bytes", self.read_bytes),
            ("write_bytes", self.write_bytes),
        ]
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub scenario: String,
    /// `"wasm"` or `"native"`, see the module docs.
    pub mode: String,
    pub tolerance_pct: f64,
    pub metrics: Metrics,
}

/// Runs `f` (which should make exactly one contract call) and returns its
/// result with the resources that call consumed. The budget is reset to
/// unlimited first, so worst-case scenarios can't trip the default limit
/// before they are measured.
pub fn measure<R>(env: &Env, f: impl FnOnce() -> R) -> (R, Metrics) {
    env.host()
        .with_mut_storage(|s| {
            s.footprint = Default::default();
            Ok(())
        })
        .unwrap();
    env.budget().reset_unlimited();

    let result = f();

    let budget = env.budget();
    let mut metrics = Metrics {
        cpu_insns: budget.cpu_instruction_cost(),
        mem_bytes: budget.memory_bytes_cost(),
        ..Metrics::default()
    };

    let host_budget = env.host().budget_cloned();
    env.host()
        .with_mut_storage(|s| {
            // XDR-encoded key -> XDR size of the entry now in storage.
            let mut entry_sizes: BTreeMap<Vec<u8>, u64> = BTreeMap::new();
            for (key, value) in s.map.iter(&host_budget)? {
                if let Some((entry, _live_until)) = value {
                    entry_sizes.insert(xdr_bytes(&**key), xdr_bytes(&**entry).len() as u64);
                }
            }
            for (key, access) in s.footprint.0.iter(&host_budget)? {
                let size = entry_sizes.get(&xdr_bytes(&**key)).copied().unwrap_or(0);
                // AccessType isn't nameable from here (soroban-env-host is
                // not a direct dependency); its Debug form is stable.
                if format!("{access:?}") == "ReadWrite" {
                    metrics.write_entries += 1;
                    metrics.write_bytes += size;
                } else {
                    metrics.read_entries += 1;
                    metrics.read_bytes += size;
                }
            }
            Ok(())
        })
        .unwrap();

    (result, metrics)
}

fn xdr_bytes<T: WriteXdr>(value: &T) -> Vec<u8> {
    value.to_xdr(Limits::none()).unwrap()
}

pub struct Suite {
    snapshot_dir: PathBuf,
    report_dir: PathBuf,
    mode: &'static str,
}

impl Suite {
    /// `manifest_dir` is the crate's `env!("CARGO_MANIFEST_DIR")`; `mode`
    /// is `"wasm"` or `"native"` depending on how the contract was
    /// registered.
    pub fn new(manifest_dir: &str, mode: &'static str) -> Self {
        let root = Path::new(manifest_dir);
        Self {
            snapshot_dir: root.join("snapshots").join("resources"),
            report_dir: root.join("target").join("resource-report"),
            mode,
        }
    }

    pub fn check(&self, scenario: &str, metrics: Metrics) {
        self.check_with_tolerance(scenario, metrics, DEFAULT_TOLERANCE_PCT);
    }

    pub fn check_with_tolerance(&self, scenario: &str, metrics: Metrics, tolerance_pct: f64) {
        let current = Snapshot {
            scenario: scenario.to_string(),
            mode: self.mode.to_string(),
            tolerance_pct,
            metrics,
        };
        write_json(&self.report_dir.join(format!("{scenario}.json")), &current);

        let path = self.snapshot_dir.join(format!("{scenario}.json"));
        if std::env::var("UPDATE_SNAPSHOTS").as_deref() == Ok("1") {
            write_json(&path, &current);
            return;
        }

        let baseline: Snapshot = match fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text)
                .unwrap_or_else(|e| panic!("{}: invalid snapshot: {e}", path.display())),
            Err(_) => panic!(
                "missing resource snapshot {}; record it with \
                 `UPDATE_SNAPSHOTS=1 cargo test --features resource-wasm resource_snapshots`",
                path.display()
            ),
        };
        if baseline.mode != self.mode {
            panic!(
                "{scenario}: snapshot was recorded in {} mode but this run measures {} mode; \
                 run with --features resource-wasm (after building the wasm) or regenerate",
                baseline.mode, self.mode
            );
        }

        let regressions = regressions(&baseline.metrics, &metrics, tolerance_pct);
        if !regressions.is_empty() {
            panic!(
                "{scenario}: resource regression beyond {tolerance_pct}% \
                 (update with UPDATE_SNAPSHOTS=1 if intended):\n{}",
                regressions.join("\n")
            );
        }
    }
}

/// One line per metric that grew beyond the tolerance.
pub fn regressions(baseline: &Metrics, current: &Metrics, tolerance_pct: f64) -> Vec<String> {
    baseline
        .fields()
        .iter()
        .zip(current.fields().iter())
        .filter_map(|((name, base), (_, cur))| {
            let allowed = *base as f64 * (1.0 + tolerance_pct / 100.0);
            (*cur > *base && *cur as f64 > allowed).then(|| {
                format!(
                    "  {name}: {base} -> {cur} ({:+.2}%)",
                    pct_change(*base, *cur)
                )
            })
        })
        .collect()
}

pub fn pct_change(base: u64, cur: u64) -> f64 {
    if base == 0 {
        if cur == 0 {
            0.0
        } else {
            100.0
        }
    } else {
        (cur as f64 - base as f64) * 100.0 / base as f64
    }
}

fn write_json(path: &Path, snapshot: &Snapshot) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut text = serde_json::to_string_pretty(snapshot).unwrap();
    text.push('\n');
    fs::write(path, text).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics(cpu: u64, entries: u64) -> Metrics {
        Metrics {
            cpu_insns: cpu,
            write_entries: entries,
            ..Metrics::default()
        }
    }

    #[test]
    fn growth_within_tolerance_passes() {
        assert!(regressions(&metrics(1000, 2), &metrics(1049, 2), 5.0).is_empty());
    }

    #[test]
    fn growth_beyond_tolerance_is_reported() {
        let r = regressions(&metrics(1000, 2), &metrics(1051, 2), 5.0);
        assert_eq!(r.len(), 1);
        assert!(r[0].contains("cpu_insns"));
    }

    #[test]
    fn one_extra_ledger_entry_is_a_regression() {
        let r = regressions(&metrics(1000, 2), &metrics(1000, 3), 5.0);
        assert_eq!(r.len(), 1);
        assert!(r[0].contains("write_entries"));
    }

    #[test]
    fn improvements_never_fail() {
        assert!(regressions(&metrics(1000, 3), &metrics(10, 1), 5.0).is_empty());
    }
}
