//! Pass/fail gates on the headline scores and the exit codes a CI job can act on.

use std::fmt;

use crate::error::{Error, Result};
use crate::metrics::Summary;

/// Exit code when every query ran but a score gate was not met.
pub const EXIT_GATE_FAILED: u8 = 3;
/// Exit code when at least one query errored and no score gate failed.
pub const EXIT_QUERY_ERRORS: u8 = 2;

/// Minimum scores over all queries; a gate left as `None` is not checked.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Gates {
    /// Least acceptable hit@5.
    pub min_hit5: Option<f64>,
    /// Least acceptable mean reciprocal rank.
    pub min_mrr: Option<f64>,
}

/// A gate whose score fell short.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GateFailure {
    /// Flag that set the gate, for example `--min-hit5`.
    pub flag: &'static str,
    /// The least acceptable score.
    pub required: f64,
    /// The score that was measured.
    pub actual: f64,
}

impl fmt::Display for GateFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "gate failed: {} {:.4} is below the required {:.4}",
            self.flag.trim_start_matches("--min-"),
            self.actual,
            self.required
        )
    }
}

impl Gates {
    /// The gates that `overall` does not meet.
    #[must_use]
    pub fn failures(&self, overall: &Summary) -> Vec<GateFailure> {
        let checks = [
            ("--min-hit5", self.min_hit5, overall.hit_at_5),
            ("--min-mrr", self.min_mrr, overall.mrr),
        ];
        checks
            .into_iter()
            .filter_map(|(flag, required, actual)| {
                required
                    .filter(|required| actual < *required)
                    .map(|required| GateFailure {
                        flag,
                        required,
                        actual,
                    })
            })
            .collect()
    }
}

/// The exit code for a finished run: 0, or [`EXIT_QUERY_ERRORS`], or [`EXIT_GATE_FAILED`] when a
/// gate failed (which wins, because it is the more specific verdict).
#[must_use]
pub fn exit_status(query_errors: usize, failures: &[GateFailure]) -> u8 {
    if !failures.is_empty() {
        EXIT_GATE_FAILED
    } else if query_errors > 0 {
        EXIT_QUERY_ERRORS
    } else {
        0
    }
}

/// Parses a score threshold, which must lie between 0 and 1.
///
/// # Errors
///
/// Fails when the text is not a number between 0 and 1.
pub fn parse_threshold(text: &str) -> Result<f64> {
    let value: f64 = text
        .parse()
        .map_err(|_| Error::Input(format!("{text:?} is not a number")))?;
    if (0.0..=1.0).contains(&value) {
        Ok(value)
    } else {
        Err(Error::Input(format!("{text} is outside 0..=1")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::summarize;

    fn overall(hit_at_5: f64, mrr: f64) -> Summary {
        Summary {
            hit_at_5,
            mrr,
            ..summarize::<crate::tool_level::QueryResult>(&[])
        }
    }

    #[test]
    fn unset_gates_never_fail() {
        let failures = Gates::default().failures(&overall(0.0, 0.0));
        assert_eq!(failures, Vec::new());
    }

    #[test]
    fn names_each_gate_that_is_not_met() {
        let gates = Gates {
            min_hit5: Some(0.9),
            min_mrr: Some(0.8),
        };
        let failures = gates.failures(&overall(0.85, 0.9));
        assert_eq!(failures.len(), 1);
        assert!(failures[0].to_string().contains("hit5"), "{}", failures[0]);
        let none = gates.failures(&overall(0.9, 0.8));
        assert_eq!(none, Vec::new());
    }

    #[test]
    fn a_failed_gate_outranks_query_errors_in_the_exit_status() {
        let failure = GateFailure {
            flag: "--min-mrr",
            required: 1.0,
            actual: 0.5,
        };
        assert_eq!(exit_status(0, &[]), 0);
        assert_eq!(exit_status(2, &[]), EXIT_QUERY_ERRORS);
        assert_eq!(exit_status(2, &[failure]), EXIT_GATE_FAILED);
    }

    #[test]
    fn thresholds_must_be_numbers_between_zero_and_one() {
        assert_eq!(parse_threshold("0.9").unwrap(), 0.9);
        assert!(parse_threshold("1.5").is_err());
        assert!(parse_threshold("high").is_err());
    }
}
