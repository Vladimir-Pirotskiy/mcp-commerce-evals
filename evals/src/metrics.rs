//! Retrieval metrics: rank of the first right answer, hit@k with Wilson intervals, MRR,
//! recall@k, nDCG@k and latency percentiles.

use std::collections::HashSet;

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
#[must_use]
pub fn rank_of(returned: &[Option<String>], expected: &[String]) -> Option<usize> {
    let wanted: Vec<String> = expected.iter().map(|id| normalize(id)).collect();
    returned
        .iter()
        .position(|id| {
            id.as_ref()
                .is_some_and(|id| wanted.contains(&normalize(id)))
        })
        .map(|i| i + 1)
}

/// Depth of the recall and nDCG scores; it matches the default `--top-k`, so a smaller `top_k`
/// caps them.
pub const RANK_DEPTH: usize = 10;
/// The hit@k depth that is reported next to hit@1.
const HIT_DEPTH: usize = 5;
/// Two-sided 95% normal quantile, used for the Wilson intervals.
const Z_95: f64 = 1.96;
/// Percentile of the median latency.
const P50: usize = 50;
/// Percentile of the tail latency.
const P95: usize = 95;

/// A confidence interval for a share.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Interval {
    /// Lower bound.
    pub low: f64,
    /// Upper bound.
    pub high: f64,
}

/// The 95% Wilson score interval for `successes` out of `n`; `0..0` when there are no trials.
/// It stays inside 0..1 and is honest at small `n`, unlike the normal approximation.
#[must_use]
pub fn wilson(successes: usize, n: usize) -> Interval {
    if n == 0 {
        return Interval {
            low: 0.0,
            high: 0.0,
        };
    }
    let trials = to_f64(n);
    let p = to_f64(successes) / trials;
    let z2 = Z_95 * Z_95;
    let denominator = 1.0 + z2 / trials;
    let centre = (p + z2 / (2.0 * trials)) / denominator;
    let spread =
        Z_95 * (p * (1.0 - p) / trials + z2 / (4.0 * trials * trials)).sqrt() / denominator;
    Interval {
        low: (centre - spread).max(0.0),
        high: (centre + spread).min(1.0),
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "counts of queries are far below 2^52, so the conversion is exact"
)]
fn to_f64(count: usize) -> f64 {
    count as f64
}

/// Aggregate scores over a set of queries.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Summary {
    /// Number of queries.
    pub n: usize,
    /// Share of queries whose first result is a right answer.
    pub hit_at_1: f64,
    /// 95% Wilson interval of `hit_at_1`.
    pub hit_at_1_ci: Interval,
    /// Share of queries with a right answer in the first five results.
    pub hit_at_5: f64,
    /// 95% Wilson interval of `hit_at_5`.
    pub hit_at_5_ci: Interval,
    /// Mean reciprocal rank (0 for a query with no right answer in the results).
    pub mrr: f64,
    /// Mean over queries of the share of a query's expected ids found in the first ten results.
    pub recall_at_10: f64,
    /// Mean nDCG over the first ten results, with binary relevance.
    pub ndcg_at_10: f64,
    /// Queries whose tool call failed; they count as misses.
    pub errors: usize,
    /// Number of timed search calls behind the latency figures.
    pub latency_samples: usize,
    /// Median latency of the search call, in milliseconds.
    pub latency_p50_ms: f64,
    /// 95th percentile latency, in milliseconds.
    pub latency_p95_ms: f64,
    /// Mean latency, in milliseconds.
    pub latency_mean_ms: f64,
}

/// `count` as a share of `n`, 0 when `n` is 0.
pub(crate) fn share(count: usize, n: usize) -> f64 {
    if n == 0 {
        0.0
    } else {
        to_f64(count) / to_f64(n)
    }
}

/// Nearest-rank percentile of an ascending list, in integer arithmetic: the value at 1-based
/// rank `ceil(percent * len / 100)`.
fn percentile(sorted: &[f64], percent: usize) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let rank = (percent * sorted.len()).div_ceil(100);
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// The distinct normalised ids a query accepts.
fn wanted_set(expected: &[String]) -> HashSet<String> {
    expected.iter().map(|id| normalize(id)).collect()
}

/// Share of the expected ids that appear in the first `k` returned ids.
#[must_use]
pub fn recall_at(returned: &[Option<String>], expected: &[String], k: usize) -> f64 {
    let wanted = wanted_set(expected);
    let found: HashSet<String> = returned
        .iter()
        .take(k)
        .flatten()
        .map(|id| normalize(id))
        .filter(|id| wanted.contains(id))
        .collect();
    to_f64(found.len()) / to_f64(wanted.len().max(1))
}

/// nDCG over the first `k` returned ids with binary relevance: each expected id counts once, at
/// its first position, and the ideal ranking puts all expected ids first.
pub fn ndcg_at(returned: &[Option<String>], expected: &[String], k: usize) -> f64 {
    let wanted = wanted_set(expected);
    let mut seen = HashSet::new();
    let dcg: f64 = returned
        .iter()
        .take(k)
        .enumerate()
        .filter(|(_, id)| {
            id.as_ref()
                .map(|id| normalize(id))
                .is_some_and(|id| wanted.contains(&id) && seen.insert(id))
        })
        .map(|(position, _)| discount(position))
        .sum();
    let ideal: f64 = (0..wanted.len().min(k)).map(discount).sum();
    if ideal == 0.0 { 0.0 } else { dcg / ideal }
}

/// The gain lost by a relevant result at 0-based `position`: `1 / log2(position + 2)`.
fn discount(position: usize) -> f64 {
    1.0 / (to_f64(position) + 2.0).log2()
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
    let (hits_1, hits_5) = (at(1), at(HIT_DEPTH));
    let reciprocal: f64 = results
        .iter()
        .filter_map(|r| r.rank)
        .map(|rank| 1.0 / to_f64(rank))
        .sum();
    let mean_of = |score: fn(&QueryResult) -> f64| {
        if n == 0 {
            0.0
        } else {
            results.iter().map(score).sum::<f64>() / to_f64(n)
        }
    };
    let mut latencies: Vec<f64> = results
        .iter()
        .flat_map(|r| r.latencies_ms.iter().copied())
        .collect();
    latencies.sort_by(f64::total_cmp);
    let mean = if latencies.is_empty() {
        0.0
    } else {
        latencies.iter().sum::<f64>() / to_f64(latencies.len())
    };
    Summary {
        n,
        hit_at_1: share(hits_1, n),
        hit_at_1_ci: wilson(hits_1, n),
        hit_at_5: share(hits_5, n),
        hit_at_5_ci: wilson(hits_5, n),
        mrr: if n == 0 { 0.0 } else { reciprocal / to_f64(n) },
        recall_at_10: mean_of(|r| recall_at(&r.returned, &r.expected, RANK_DEPTH)),
        ndcg_at_10: mean_of(|r| ndcg_at(&r.returned, &r.expected, RANK_DEPTH)),
        errors: results.iter().filter(|r| r.error.is_some()).count(),
        latency_samples: latencies.len(),
        latency_p50_ms: percentile(&latencies, P50),
        latency_p95_ms: percentile(&latencies, P95),
        latency_mean_ms: mean,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(rank: Option<usize>, latency_ms: f64, error: Option<&str>) -> QueryResult {
        let returned = rank.map_or_else(Vec::new, |rank| {
            let mut ids = vec![None; rank - 1];
            ids.push(Some("X".to_owned()));
            ids
        });
        QueryResult {
            id: "q".to_owned(),
            kind: "k".to_owned(),
            text: "t".to_owned(),
            expected: vec!["X".to_owned()],
            returned,
            rank,
            latencies_ms: if error.is_none() {
                vec![latency_ms]
            } else {
                Vec::new()
            },
            error: error.map(str::to_owned),
        }
    }

    #[test]
    fn rank_ignores_case_and_punctuation_and_takes_the_best_expected_id() {
        let returned = [Some("B-2".to_owned()), None, Some("a-1".to_owned())];
        assert_eq!(rank_of(&returned, &["A1".to_owned()]), Some(3));
        assert_eq!(
            rank_of(&returned, &["a 1".to_owned(), "b2".to_owned()]),
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

    fn ids(list: &[&str]) -> Vec<Option<String>> {
        list.iter().map(|id| Some((*id).to_owned())).collect()
    }

    fn wanted(list: &[&str]) -> Vec<String> {
        list.iter().map(|id| (*id).to_owned()).collect()
    }

    #[test]
    fn wilson_interval_matches_published_values() {
        // 9 of 10: the textbook Wilson 95% interval is 0.596 to 0.982.
        let ci = wilson(9, 10);
        assert!((ci.low - 0.5958).abs() < 1e-3, "{ci:?}");
        assert!((ci.high - 0.9821).abs() < 1e-3, "{ci:?}");
        let all = wilson(10, 10);
        assert_eq!(all.high, 1.0);
        assert!(all.low > 0.69 && all.low < 0.73, "{all:?}");
        assert_eq!(
            wilson(0, 0),
            Interval {
                low: 0.0,
                high: 0.0
            }
        );
    }

    #[test]
    fn recall_is_the_share_of_expected_ids_found_in_the_top_k() {
        let returned = ids(&["A", "x", "B", "y", "C"]);
        let expected = wanted(&["a", "b", "c", "d"]);
        assert_eq!(recall_at(&returned, &expected, 3), 0.5);
        assert_eq!(recall_at(&returned, &expected, 10), 0.75);
        assert_eq!(recall_at(&[], &expected, 10), 0.0);
    }

    #[test]
    fn ndcg_is_one_for_an_ideal_ranking_and_discounts_late_hits() {
        let expected = wanted(&["A", "B"]);
        assert!((ndcg_at(&ids(&["A", "B", "x"]), &expected, 10) - 1.0).abs() < 1e-12);
        // One hit at rank 2 of two expected: (1/log2(3)) / (1 + 1/log2(3)).
        let late = ndcg_at(&ids(&["x", "A"]), &expected, 10);
        let gain = 1.0 / 3.0_f64.log2();
        assert!((late - gain / (1.0 + gain)).abs() < 1e-12, "{late}");
        assert_eq!(ndcg_at(&ids(&["x", "y"]), &expected, 10), 0.0);
    }

    #[test]
    fn percentiles_use_integer_nearest_rank_at_the_boundaries() {
        let twenty: Vec<f64> = (1..=20).map(f64::from).collect();
        assert_eq!(percentile(&twenty, 95), 19.0);
        assert_eq!(percentile(&twenty, 50), 10.0);
        let hundred_and_one: Vec<f64> = (1..=101).map(f64::from).collect();
        assert_eq!(percentile(&hundred_and_one, 95), 96.0);
        assert_eq!(percentile(&[7.0], 95), 7.0);
        assert_eq!(percentile(&[], 95), 0.0);
    }
}
