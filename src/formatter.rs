// A collection of functions that formats data into the right shape for plotting functions.

use crate::theseus::TheseusResult;
use serde::{Deserialize, Serialize};
// The data format of cohorts.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CohortData {
    pub y: Vec<Vec<i64>>,
    pub ts: Vec<String>,
    pub labels: Vec<String>,
}

pub fn format_cohort_data(result: TheseusResult) -> CohortData {
    sum_commit_data_by_year(result)
}

pub fn sum_commit_data_by_year(result: TheseusResult) -> CohortData {
    let commit_infos = result.commit_cohort_info;
    let snapshots = result.cohort_data;

    let ts: Vec<String> = commit_infos
        .iter()
        .map(|info| info.time_string.clone())
        .collect();

    let all_blame_years: std::collections::BTreeSet<u32> =
        commit_infos.iter().map(|info| info.year).collect();
    let sorted_blame_years: Vec<u32> = all_blame_years.into_iter().collect();
    let labels: Vec<String> = sorted_blame_years
        .iter()
        .map(|y| format!("Code added in {y}"))
        .collect();
    let year_to_label_index: std::collections::HashMap<u32, usize> = sorted_blame_years
        .iter()
        .enumerate()
        .map(|(i, &year)| (year, i))
        .collect();

    let num_labels = labels.len();
    let num_snapshots = snapshots.len();
    let mut y = vec![vec![0i64; num_snapshots]; num_labels];

    for (commit_idx, snapshot) in snapshots.iter().enumerate() {
        let mut is_snapshot_bad = false;
        for (commit_key, line_count) in snapshot {
            let blame_year = commit_infos[*commit_key].year;
            let label_idx = year_to_label_index
                .get(&blame_year)
                .expect("Label index not found");
            y[*label_idx][commit_idx] += *line_count;
            if y[*label_idx][commit_idx] > 1_000_000_000 {
                println!(
                    "Warning: commit {} has {} lines in year {} (line count: {})",
                    commit_idx, y[*label_idx][commit_idx], blame_year, line_count
                );
                is_snapshot_bad = true;
            }
        }
        if is_snapshot_bad {
            println!("Snapshot {} is bad", commit_idx);
            println!("{:?}", snapshot);
        }
    }
    CohortData { y, ts, labels }
}
