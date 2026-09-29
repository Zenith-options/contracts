#![no_std]

use soroban_sdk::{Vec, Env};

pub struct PriceDispersionResult {
    pub filtered_median: i128,
    pub mad: i128,
    pub dispersion_bps: u32,
    pub valid_reports_count: u32,
}

pub struct OutlierFilter;

impl OutlierFilter {
    /// Filters out price reports exceeding `k * MAD` (Median Absolute Deviation),
    /// mitigating flash crashes and collusive feeder outliers.
    pub fn compute_robust_price(env: &Env, mut reports: Vec<i128>, k: u32) -> Option<PriceDispersionResult> {
        let n = reports.len();
        if n == 0 {
            return None;
        }

        // 1. Sort to determine preliminary median
        reports.sort();
        let median = reports.get(n / 2)?;

        // 2. Compute absolute deviations from median
        let mut abs_devs = Vec::new(env);
        for i in 0..n {
            let val = reports.get(i)?;
            let diff = if val >= median { val - median } else { median - val };
            abs_devs.push_back(diff);
        }
        abs_devs.sort();
        let mad = abs_devs.get(n / 2)?;

        // 3. Drop reports > k * MAD
        let threshold = mad.checked_mul(k as i128)?;
        let mut valid_sum = 0i128;
        let mut valid_count = 0u32;

        for i in 0..n {
            let val = reports.get(i)?;
            let diff = if val >= median { val - median } else { median - val };
            if diff <= threshold {
                valid_sum = valid_sum.checked_add(val)?;
                valid_count += 1;
            }
        }

        if valid_count == 0 {
            return None;
        }

        let filtered_median = valid_sum / valid_count as i128;
        let dispersion_bps = if filtered_median > 0 {
            ((mad * 10_000) / filtered_median) as u32
        } else {
            0
        };

        Some(PriceDispersionResult {
            filtered_median,
            mad,
            dispersion_bps,
            valid_reports_count: valid_count,
        })
    }
}
