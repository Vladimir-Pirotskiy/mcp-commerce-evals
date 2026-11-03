//! Shape checks on the GitHub workflow files, which no other test reads. They are text checks
//! (the repo has no YAML parser and needs none for these).

use std::path::Path;

fn workflow(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.github/workflows");
    std::fs::read_to_string(path.join(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// The text of one top-level job: from its `  name:` line to the next job.
fn job(workflow: &str, name: &str) -> String {
    let header = format!("  {name}:");
    let mut lines = workflow.lines().skip_while(|line| *line != header);
    assert!(lines.next().is_some(), "no job {name}");
    lines
        .take_while(|line| {
            !(line.starts_with("  ") && !line.starts_with("   ") && line.ends_with(':'))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn ci_runs_weekly_so_an_upstream_advisory_shows_up_without_a_push() {
    let ci = workflow("ci.yml");
    assert!(ci.contains("\n  schedule:\n"), "no schedule trigger");
    assert!(
        ci.contains("\n    - cron: "),
        "the schedule has no cron entry"
    );
}

#[test]
fn advisories_are_their_own_job_that_cannot_block_a_pull_request() {
    let ci = workflow("ci.yml");
    assert!(
        !job(&ci, "deny").contains("advisories"),
        "the deny job (licenses, bans, sources) must not also run the advisory scan"
    );
    let advisories = job(&ci, "advisories");
    assert!(advisories.contains("check advisories"), "{advisories}");
    assert!(
        advisories.contains("continue-on-error: ${{ github.event_name != 'schedule' }}"),
        "only a scheduled run may fail on an advisory:\n{advisories}"
    );
}
