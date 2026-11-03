//! The score floors in `.github/workflows/ci.yml` must stay at or below the committed tool-level
//! results, and no further below than one query, so a results change cannot silently break CI
//! and a floor cannot drift into meaninglessness.

use std::path::Path;

use serde_json::Value;

const RESULTS: &str = "results/tool-level-2026-11-03.json";

fn repo_file(relative: &str) -> String {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    std::fs::read_to_string(root.join(relative)).unwrap_or_else(|e| panic!("{relative}: {e}"))
}

/// The number after `flag` in the workflow file.
fn floor(workflow: &str, flag: &str) -> f64 {
    let after = workflow
        .split(flag)
        .nth(1)
        .unwrap_or_else(|| panic!("{flag} is not in ci.yml"));
    after
        .split_whitespace()
        .next()
        .and_then(|number| number.parse().ok())
        .unwrap_or_else(|| panic!("{flag} is not followed by a number"))
}

#[test]
fn ci_floors_sit_at_or_below_the_committed_results_by_at_most_one_query() {
    let workflow = repo_file(".github/workflows/ci.yml");
    let results: Value = serde_json::from_str(&repo_file(RESULTS)).expect("results JSON");
    let overall = &results["tool_level"]["overall"];
    let queries = overall["n"].as_f64().expect("n");
    let one_query = 1.0 / queries;
    for (flag, field) in [("--min-hit5", "hit_at_5"), ("--min-mrr", "mrr")] {
        let committed = overall[field].as_f64().expect(field);
        let floor = floor(&workflow, flag);
        assert!(
            floor <= committed,
            "{flag} {floor} is above the committed {field} {committed}: CI would fail"
        );
        assert!(
            committed - floor <= one_query,
            "{flag} {floor} is more than one query ({one_query}) below {field} {committed}"
        );
    }
}
