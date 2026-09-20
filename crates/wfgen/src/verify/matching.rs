use crate::oracle::OracleAlert;

use super::types::ActualAlert;

/// Result of greedy matching within a single key group.
pub(super) struct MatchResult {
    pub(super) matched: usize,
    /// (expected_idx, actual_idx) pairs with score mismatch.
    pub(super) mismatches: Vec<(usize, usize)>,
    /// Indices into the expected slice that were not paired.
    pub(super) missing_indices: Vec<usize>,
    /// Indices into the actual slice that were not paired.
    pub(super) unexpected_indices: Vec<usize>,
}

/// Greedily pair expected and actual alerts within a group by nearest time.
///
/// **性能**：ISO 时间戳的解析（`parse_time_approx`）必须**按边预计算**、不能放进内层循环。
/// 历史上它在内层逐对解析，于是成本 ≈ Σ(组大小²) × 一次 chrono 解析：
/// 热实体的语料（同一 entity_id 落几千条）实测把它推到秒级（q21：单组 8050² ≈ 6500 万次
/// 日期解析 → `wfgen verify` 5.4s）。现改为 O(n) 次解析 + 纯 f64 比较，配对结果逐条不变。
///
/// 剩下的扫描仍是 Σ(|期望组| × |实际组|)；单热组达数万条时仍会显形（每对约 1–2ns），
/// 届时应改成「按时间排序 + 双向游标/DSU 找最近未用」——当前语料规模下不值那份复杂度。
pub(super) fn greedy_match(
    expected: &[&OracleAlert],
    actual: &[&ActualAlert],
    score_tolerance: f64,
    time_tolerance_secs: f64,
) -> MatchResult {
    let mut used_actual = vec![false; actual.len()];
    let mut matched = 0usize;
    let mut mismatches = Vec::new();
    let mut paired_expected = vec![false; expected.len()];

    // 时间戳只解析一次（每条边各一遍），内层循环只做 f64 比较。
    let exp_times: Vec<f64> = expected
        .iter()
        .map(|e| parse_time_approx(&e.emit_time))
        .collect();
    let act_times: Vec<f64> = actual
        .iter()
        .map(|a| parse_time_approx(&a.fired_at))
        .collect();

    // For each expected alert, find the nearest unused actual by time
    for (ei, exp) in expected.iter().enumerate() {
        let exp_time = exp_times[ei];
        let mut best_idx: Option<usize> = None;
        let mut best_dist = f64::MAX;

        for (j, act_time) in act_times.iter().enumerate() {
            if used_actual[j] {
                continue;
            }
            let dist = (exp_time - act_time).abs();
            if dist < best_dist {
                best_dist = dist;
                best_idx = Some(j);
            }
        }

        if let Some(j) = best_idx {
            used_actual[j] = true;
            paired_expected[ei] = true;
            let score_diff = (exp.score - actual[j].score).abs();
            let time_diff = best_dist; // abs time diff in seconds
            if score_diff <= score_tolerance && time_diff <= time_tolerance_secs {
                matched += 1;
            } else {
                mismatches.push((ei, j));
            }
        }
    }

    let missing_indices: Vec<usize> = paired_expected
        .iter()
        .enumerate()
        .filter(|(_, paired)| !**paired)
        .map(|(i, _)| i)
        .collect();

    let unexpected_indices: Vec<usize> = used_actual
        .iter()
        .enumerate()
        .filter(|(_, used)| !**used)
        .map(|(i, _)| i)
        .collect();

    MatchResult {
        matched,
        mismatches,
        missing_indices,
        unexpected_indices,
    }
}

/// Parse an ISO 8601 timestamp to seconds-since-epoch (approximate, for ordering).
pub(super) fn parse_time_approx(s: &str) -> f64 {
    s.parse::<chrono::DateTime<chrono::Utc>>()
        .map(|dt| dt.timestamp() as f64 + dt.timestamp_subsec_millis() as f64 / 1000.0)
        .unwrap_or(0.0)
}
