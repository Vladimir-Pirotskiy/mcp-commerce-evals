//! Retrieval metrics: rank of the first right answer, hit@k with Wilson intervals, MRR,
//! capped recall@k, nDCG@k and latency percentiles.

use std::borrow::Borrow;
use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::tool_level::QueryResult;

/// Lowercase letters and digits only, so `ALD-CH-1002`, `ald ch 1002` and `aldch1002` agree.
/// An id with none (all punctuation or non-ASCII) normalizes to the empty string; callers treat
/// that as matching nothing, never as an id.
pub fn normalize(id: &str) -> String {
    id.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The normalized forms of `ids`, leaving out any that normalize to nothing, so an empty key can
/// never be "expected" and match another empty key.
pub(crate) fn normalized_ids(ids: &[String]) -> Vec<String> {
    ids.iter()
        .map(|id| normalize(id))
        .filter(|id| !id.is_empty())
        .collect()
}

/// 1-based position of the first returned id that is expected.
#[must_use]
pub fn rank_of(returned: &[Option<String>], expected: &[String]) -> Option<usize> {
    let wanted = normalized_ids(expected);
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

/// Fixed seed of the bootstrap intervals, recorded in every report's `meta`, so the same results
/// always give the same intervals.
pub const BOOTSTRAP_SEED: u64 = 0x2026_1103_0cad_5eed;
/// How many resamples each bootstrap interval is built from.
pub const BOOTSTRAP_RESAMPLES: usize = 2000;

/// `SplitMix64`, a tiny seeded generator: enough for resampling, no crate needed. Not for secrets.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number in `0..bound` (`bound` above zero); the modulo bias is below 2^-40 for the sizes
    /// used here.
    fn below(&mut self, bound: usize) -> usize {
        let bound = u64::try_from(bound).unwrap_or(u64::MAX).max(1);
        usize::try_from(self.next_u64() % bound).unwrap_or(0)
    }
}

/// Fewest queries a bootstrap interval is shown for. Below it the resamples can take only a few
/// distinct values and the percentile interval is far narrower than the real uncertainty.
pub const MIN_BOOTSTRAP_N: usize = 10;

/// The 95% percentile-bootstrap interval of the mean of `values`: resample them with replacement
/// [`BOOTSTRAP_RESAMPLES`] times from [`BOOTSTRAP_SEED`], and take the 2.5th and 97.5th
/// percentile of the resampled means.
///
/// `None` (shown as `n/a`) when there are fewer than [`MIN_BOOTSTRAP_N`] values, or when they are
/// all equal: a percentile bootstrap of such a sample is a zero-width interval that claims a
/// certainty the data cannot give.
#[must_use]
pub fn bootstrap_mean_ci(values: &[f64]) -> Option<Interval> {
    let clusters: Vec<&[f64]> = values.chunks(1).collect();
    cluster_bootstrap_mean_ci(&clusters)
}

/// Like [`bootstrap_mean_ci`], but resamples whole `clusters` (the repeated runs of one query)
/// rather than single values, so runs of a query stay together and its trials are not counted as
/// independent. The mean is over all values of the resampled clusters. `None` under the same
/// rules, counted in clusters.
#[must_use]
pub fn cluster_bootstrap_mean_ci(clusters: &[&[f64]]) -> Option<Interval> {
    let sums: Vec<(f64, f64)> = clusters
        .iter()
        .filter(|c| !c.is_empty())
        .map(|c| (c.iter().sum::<f64>(), to_f64(c.len())))
        .collect();
    let cluster_means: Vec<f64> = sums.iter().map(|(sum, size)| sum / size).collect();
    let (lowest, highest) = cluster_means
        .iter()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), m| {
            (lo.min(*m), hi.max(*m))
        });
    if sums.len() < MIN_BOOTSTRAP_N || highest - lowest < 1e-12 {
        return None;
    }
    let mut rng = SplitMix64(BOOTSTRAP_SEED);
    let mut means: Vec<f64> = (0..BOOTSTRAP_RESAMPLES)
        .map(|_| {
            let (total, count) = (0..sums.len())
                .map(|_| sums[rng.below(sums.len())])
                .fold((0.0, 0.0), |(t, c), (sum, size)| (t + sum, c + size));
            total / count
        })
        .collect();
    means.sort_by(f64::total_cmp);
    Some(Interval {
        low: means[BOOTSTRAP_RESAMPLES * 25 / 1000],
        high: means[BOOTSTRAP_RESAMPLES * 975 / 1000 - 1],
    })
}

/// Unbiased pass@k (Chen et al., 2021): the chance that at least one of `k` runs drawn without
/// replacement from `n` runs, `correct` of which passed, passes. `1 - C(n-c, k) / C(n, k)`, as a
/// product so large counts do not overflow. 0 when `k` is 0 or above `n`.
#[must_use]
pub fn pass_at_k(n: usize, correct: usize, k: usize) -> f64 {
    if k == 0 || k > n {
        return 0.0;
    }
    let wrong = n - correct.min(n);
    if wrong < k {
        return 1.0;
    }
    let all_fail: f64 = (wrong + 1..=n)
        .map(|i| 1.0 - to_f64(k) / to_f64(i))
        .product();
    1.0 - all_fail
}

/// A confidence interval for a share.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
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
pub(crate) fn to_f64(count: usize) -> f64 {
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
    /// Mean reciprocal rank (0 for a query with no right answer in the first [`RANK_DEPTH`]
    /// results).
    pub mrr: f64,
    /// 95% bootstrap interval of `mrr` (seeded, see [`BOOTSTRAP_SEED`]); null (`n/a`) under
    /// [`MIN_BOOTSTRAP_N`] queries or when every query scored the same.
    pub mrr_ci: Option<Interval>,
    /// Mean over queries of capped recall@10: expected ids found in the first ten results over
    /// `min(|expected|, 10)`.
    pub recall_at_10: f64,
    /// 95% bootstrap interval of `recall_at_10`.
    pub recall_at_10_ci: Option<Interval>,
    /// Mean nDCG over the first ten results, with binary relevance.
    pub ndcg_at_10: f64,
    /// 95% bootstrap interval of `ndcg_at_10`.
    pub ndcg_at_10_ci: Option<Interval>,
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
    normalized_ids(expected).into_iter().collect()
}

/// Capped recall@k: the expected ids found in the first `k` returned ids, divided by
/// `min(|expected|, k)`. Plain recall divides by `|expected|`, which makes 1.0 unreachable for a
/// query with more than `k` right answers; the cap makes it mean "the top `k` is full of right
/// answers", the same ceiling nDCG@k uses.
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
    to_f64(found.len()) / to_f64(wanted.len().min(k).max(1))
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

/// Scores a set of query results, owned or borrowed.
pub fn summarize<R: Borrow<QueryResult>>(results: &[R]) -> Summary {
    let results: Vec<&QueryResult> = results.iter().map(Borrow::borrow).collect();
    let n = results.len();
    let at = |k: usize| {
        results
            .iter()
            .filter(|r| r.rank.is_some_and(|rank| rank <= k))
            .count()
    };
    let (hits_1, hits_5) = (at(1), at(HIT_DEPTH));
    let per_query =
        |score: fn(&QueryResult) -> f64| -> Vec<f64> { results.iter().map(|r| score(r)).collect() };
    // Capped at the depth recall and nDCG use, so a right answer beyond it scores 0 everywhere.
    let reciprocal = per_query(|r| {
        r.rank
            .filter(|rank| *rank <= RANK_DEPTH)
            .map_or(0.0, |rank| 1.0 / to_f64(rank))
    });
    let recall = per_query(|r| recall_at(&r.returned, &r.expected, RANK_DEPTH));
    let ndcg = per_query(|r| ndcg_at(&r.returned, &r.expected, RANK_DEPTH));
    let mean_of = |values: &[f64]| {
        if values.is_empty() {
            0.0
        } else {
            values.iter().sum::<f64>() / to_f64(values.len())
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
        mrr: mean_of(&reciprocal),
        mrr_ci: bootstrap_mean_ci(&reciprocal),
        recall_at_10: mean_of(&recall),
        recall_at_10_ci: bootstrap_mean_ci(&recall),
        ndcg_at_10: mean_of(&ndcg),
        ndcg_at_10_ci: bootstrap_mean_ci(&ndcg),
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
            nondeterministic: false,
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
    fn mrr_counts_only_ranks_within_the_scored_depth() {
        // With --top-k 100 a right answer can sit at rank 11; docs/evals.md says MRR is 0 then.
        let results = [result(Some(10), 1.0, None), result(Some(11), 1.0, None)];
        let summary = summarize(&results);
        assert!((summary.mrr - 0.05).abs() < 1e-12, "{}", summary.mrr);
    }

    #[test]
    fn an_id_with_no_ascii_letters_or_digits_never_matches_anything() {
        let foreign = [Some("日本語".to_owned())];
        assert_eq!(rank_of(&foreign, &["東京".to_owned()]), None);
        assert_eq!(rank_of(&foreign, &["日本語".to_owned()]), None);
        assert_eq!(recall_at(&foreign, &["東京".to_owned()], 10), 0.0);
        assert_eq!(ndcg_at(&foreign, &["東京".to_owned()], 10), 0.0);
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
        assert_eq!(summarize::<QueryResult>(&[]).mrr, 0.0);
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
        assert_eq!(recall_at(&returned, &expected, 3), 2.0 / 3.0);
        assert_eq!(recall_at(&returned, &expected, 10), 0.75);
        assert_eq!(recall_at(&[], &expected, 10), 0.0);
    }

    #[test]
    fn recall_is_capped_by_k_so_a_long_expected_list_can_still_reach_one() {
        let expected: Vec<String> = (0..17).map(|i| format!("P-{i}")).collect();
        let returned: Vec<Option<String>> = (0..10).map(|i| Some(format!("p{i}"))).collect();
        assert_eq!(recall_at(&returned, &expected, 10), 1.0);
        assert_eq!(recall_at(&returned[..5], &expected, 10), 0.5);
        assert_eq!(recall_at(&returned, &expected, 5), 1.0);
    }

    #[test]
    fn splitmix64_matches_the_reference_sequence() {
        let mut rng = SplitMix64(0);
        assert_eq!(rng.next_u64(), 0xe220_a839_7b1d_cdaf);
        assert_eq!(rng.next_u64(), 0x6e78_9e6a_a1b9_65f4);
        assert_eq!(rng.next_u64(), 0x06c4_5d18_8009_454f);
    }

    #[test]
    fn the_bootstrap_interval_is_deterministic_and_brackets_the_mean() {
        let values: Vec<f64> = (0..40)
            .map(|i| if i % 4 == 0 { 0.0 } else { 1.0 })
            .collect();
        let first = bootstrap_mean_ci(&values).unwrap();
        assert_eq!(
            Some(first),
            bootstrap_mean_ci(&values),
            "same data, same interval"
        );
        assert!(first.low < 0.75 && 0.75 < first.high, "{first:?}");
        assert!(first.low > 0.55 && first.high < 0.95, "{first:?}");
        // Pinned to the values this seed gives (resampled means of 0/1 scores are multiples of
        // 1/40), so a change of seed, resample count or method is noticed.
        assert!((first.low - 0.6).abs() < 1e-12, "{first:?}");
        assert!((first.high - 0.875).abs() < 1e-12, "{first:?}");
    }

    #[test]
    fn the_bootstrap_interval_of_graded_scores_is_pinned_to_the_seed() {
        // Graded scores make the percentiles fall between multiples of 1/40, so almost any other
        // seed moves them.
        let values: Vec<f64> = (0..40).map(|i| f64::from((i * 7) % 11) / 10.0).collect();
        let interval = bootstrap_mean_ci(&values).unwrap();
        assert!((interval.low - 0.405).abs() < 1e-12, "{interval:?}");
        assert!((interval.high - 0.5975).abs() < 1e-12, "{interval:?}");
    }

    #[test]
    fn a_bootstrap_interval_is_not_shown_for_a_tiny_or_constant_sample() {
        // A constant sample has a zero-width percentile interval: it would print "1.00-1.00".
        assert_eq!(bootstrap_mean_ci(&[1.0; 5]), None);
        assert_eq!(bootstrap_mean_ci(&[0.3]), None);
        assert_eq!(bootstrap_mean_ci(&[]), None);
        assert_eq!(bootstrap_mean_ci(&[0.5; 40]), None, "degenerate at any n");
        let nine: Vec<f64> = (0..9).map(|i| f64::from(i % 2)).collect();
        assert_eq!(bootstrap_mean_ci(&nine), None, "below the minimum n");
        let ten: Vec<f64> = (0..10).map(|i| f64::from(i % 2)).collect();
        assert!(bootstrap_mean_ci(&ten).is_some());
    }

    #[test]
    fn resampling_whole_queries_widens_the_interval_for_perfectly_correlated_trials() {
        // 20 queries, 3 trials each: half always right, half always wrong.
        let query = |score: f64| vec![score; 3];
        let runs: Vec<Vec<f64>> = (0..20).map(|i| query(f64::from(i % 2))).collect();
        let clusters: Vec<&[f64]> = runs.iter().map(Vec::as_slice).collect();
        let flat: Vec<f64> = runs.iter().flatten().copied().collect();

        let clustered = cluster_bootstrap_mean_ci(&clusters).unwrap();
        let naive = bootstrap_mean_ci(&flat).unwrap();
        let wilson = wilson(30, 60);

        let width = |i: Interval| i.high - i.low;
        assert!(
            width(clustered) > width(naive) * 1.4,
            "{clustered:?} vs {naive:?}"
        );
        assert!(
            width(clustered) > width(wilson) * 1.4,
            "{clustered:?} vs {wilson:?}"
        );
    }

    #[test]
    fn pass_at_k_is_the_unbiased_estimator() {
        // 5 runs, 2 passed: pass@1 = 2/5, pass@2 = 1 - C(3,2)/C(5,2) = 0.7, pass@4 = 1.
        assert!((pass_at_k(5, 2, 1) - 0.4).abs() < 1e-12);
        assert!((pass_at_k(5, 2, 2) - 0.7).abs() < 1e-12);
        assert_eq!(pass_at_k(5, 2, 4), 1.0);
        assert_eq!(pass_at_k(5, 0, 5), 0.0);
        assert_eq!(pass_at_k(5, 5, 1), 1.0);
        assert_eq!(pass_at_k(3, 1, 4), 0.0, "cannot draw more runs than exist");
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
