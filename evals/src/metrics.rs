//! Retrieval metrics: rank of the first right answer, hit@k, MRR and latency percentiles.

use serde::Serialize;

use crate::tool_level::QueryResult;

/// Lowercase letters and digits only, so `ALD-CH-1002`, `ald ch 1002` and `aldch1002` agree.
pub fn normalize(id: &str) -> String {
    id.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// 1-based position of the first returned id that is expected.
pub fn rank_of(returned: &[String], expected: &[String]) -> Option<usize> {
    let wanted: Vec<String> = expected.iter().map(|id| normalize(id)).collect();
    returned
        .iter()
        .position(|id| wanted.contains(&normalize(id)))
        .map(|i| i + 1)
}

/// Aggregate scores over a set of queries.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Summary {
    /// Number of queries.
    pub n: usize,
    /// Share of queries whose first result is a right answer.
    pub hit_at_1: f64,
    /// Share of queries with a right answer in the first five results.
    pub hit_at_5: f64,
    /// Mean reciprocal rank (0 for a query with no right answer in the results).
    pub mrr: f64,
    /// Queries whose tool call failed; they count as misses.
    pub errors: usize,
    /// Median latency of the search call, in milliseconds.
    pub latency_p50_ms: f64,
    /// 95th percentile latency, in milliseconds.
    pub latency_p95_ms: f64,
    /// Mean latency, in milliseconds.
    pub latency_mean_ms: f64,
}

fn share(count: usize, n: usize) -> f64 {
    if n == 0 { 0.0 } else { count as f64 / n as f64 }
}

/// Nearest-rank percentile of an ascending list.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (p * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// Scores a set of query results.
pub fn summarize(results: &[QueryResult]) -> Summary {
    let n = results.len();
    let at = |k: usize| {
        results
            .iter()
            .filter(|r| r.rank.is_some_and(|rank| rank <= k))
            .count()
    };
    let reciprocal: f64 = results
        .iter()
        .filter_map(|r| r.rank)
        .map(|rank| 1.0 / rank as f64)
        .sum();
    let mut latencies: Vec<f64> = results
        .iter()
        .filter(|r| r.error.is_none())
        .map(|r| r.latency_ms)
        .collect();
    latencies.sort_by(f64::total_cmp);
    let mean = if latencies.is_empty() {
        0.0
    } else {
        latencies.iter().sum::<f64>() / latencies.len() as f64
    };
    Summary {
        n,
        hit_at_1: share(at(1), n),
        hit_at_5: share(at(5), n),
        mrr: if n == 0 { 0.0 } else { reciprocal / n as f64 },
        errors: results.iter().filter(|r| r.error.is_some()).count(),
        latency_p50_ms: percentile(&latencies, 0.50),
        latency_p95_ms: percentile(&latencies, 0.95),
        latency_mean_ms: mean,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(rank: Option<usize>, latency_ms: f64, error: Option<&str>) -> QueryResult {
        QueryResult {
            id: "q".to_owned(),
            kind: "k".to_owned(),
            text: "t".to_owned(),
            expected: vec!["X".to_owned()],
            returned: Vec::new(),
            rank,
            latency_ms,
            error: error.map(str::to_owned),
        }
    }

    #[test]
    fn rank_ignores_case_and_punctuation_and_takes_the_best_expected_id() {
        let returned = ["B-2".to_owned(), "a-1".to_owned(), "c-3".to_owned()];
        assert_eq!(rank_of(&returned, &["A1".to_owned()]), Some(2));
        assert_eq!(
            rank_of(&returned, &["c 3".to_owned(), "b2".to_owned()]),
            Some(1)
        );
        assert_eq!(rank_of(&returned, &["z-9".to_owned()]), None);
        assert_eq!(rank_of(&[], &["a".to_owned()]), None);
    }

    #[test]
    fn hit_rates_and_mrr_match_a_hand_calculation() {
        let results = [
            result(Some(1), 10.0, None),
            result(Some(3), 20.0, None),
            result(None, 30.0, None),
            result(Some(6), 40.0, None),
        ];
        let summary = summarize(&results);
        assert_eq!(summary.n, 4);
        assert_eq!(summary.hit_at_1, 0.25);
        assert_eq!(summary.hit_at_5, 0.5);
        assert!((summary.mrr - (1.0 + 1.0 / 3.0 + 1.0 / 6.0) / 4.0).abs() < 1e-12);
    }

    #[test]
    fn percentiles_use_nearest_rank_and_errors_are_misses_without_latency() {
        let mut results: Vec<_> = (1..=20)
            .map(|i| result(Some(1), f64::from(i), None))
            .collect();
        results.push(result(None, 9999.0, Some("boom")));
        let summary = summarize(&results);
        assert_eq!(summary.errors, 1);
        assert_eq!(summary.latency_p50_ms, 10.0);
        assert_eq!(summary.latency_p95_ms, 19.0);
        assert_eq!(summary.latency_mean_ms, 10.5);
        assert_eq!(summarize(&[]).mrr, 0.0);
    }
}
