//! Shape checks on the GitHub workflow files, which no other test reads. They are text checks
//! (the repo has no YAML parser and needs none for these).

use std::path::Path;

const WORKFLOWS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../.github/workflows");

fn workflow(name: &str) -> String {
    let path = Path::new(WORKFLOWS).join(name);
    assert!(path.exists(), "{name} is missing");
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{name}: {e}"))
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

#[test]
fn every_action_is_pinned_by_commit_sha_with_a_version_comment() {
    for name in ["ci.yml", "release.yml"] {
        let text = workflow(name);
        let uses: Vec<&str> = text
            .lines()
            .filter_map(|line| line.split_once("uses: ").map(|(_, rest)| rest))
            .collect();
        assert!(!uses.is_empty(), "{name} uses no actions");
        for entry in uses {
            let (action, comment) = entry.split_once(" # ").unwrap_or((entry, ""));
            let sha = action.rsplit_once('@').map_or("", |(_, sha)| sha);
            assert!(
                sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()),
                "{name}: {action} is not pinned by a full commit sha"
            );
            assert!(
                comment.starts_with('v'),
                "{name}: {action} has no version comment"
            );
        }
    }
}

#[test]
fn a_version_tag_builds_linux_binaries_and_publishes_a_release() {
    let release = workflow("release.yml");
    assert!(
        release.contains("\n  push:\n    tags:\n      - \"v*\"\n"),
        "{release}"
    );
    assert!(release.contains("gh release create"), "{release}");
    assert!(release.contains("--release"), "{release}");
    assert!(release.contains("--locked"), "{release}");
}

#[test]
fn only_the_release_job_may_write_to_the_repository() {
    let ci = workflow("ci.yml");
    assert!(!ci.contains("write"), "ci.yml asks for a write permission");
    let release = workflow("release.yml");
    assert!(
        release.contains("\npermissions:\n  contents: read\n"),
        "the workflow default must be read-only"
    );
    assert_eq!(release.matches("contents: write").count(), 1, "{release}");
    assert!(job(&release, "release").contains("permissions:\n      contents: write\n"));
}

#[test]
fn the_release_workflow_tests_first_takes_notes_from_the_changelog_and_smokes_the_tarball() {
    let release = workflow("release.yml");
    assert!(
        release.contains("persist-credentials: false"),
        "checkout must not leave the token in .git/config"
    );
    let test_at = release
        .find("cargo test --locked --workspace")
        .expect("the workflow must run the tests");
    let build_at = release
        .find("cargo build --locked --release")
        .expect("the workflow must build");
    assert!(test_at < build_at, "tests must run before the build");
    assert!(release.contains("--notes-file"), "{release}");
    assert!(!release.contains("--generate-notes"), "{release}");
    assert!(release.contains("scripts/release-notes.sh"), "{release}");
    let smoke_at = release
        .find("scripts/smoke-release.sh")
        .expect("the workflow must smoke the tarball");
    let create_at = release
        .find("gh release create")
        .expect("creates the release");
    assert!(
        smoke_at < create_at,
        "smoke must run before the release is created"
    );
}

fn script(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("scripts")
        .join(name)
}

#[test]
fn release_notes_are_the_changelog_section_of_the_version() {
    let changelog = Path::new(env!("CARGO_MANIFEST_DIR")).join("../CHANGELOG.md");
    let run = |version: &str| {
        std::process::Command::new("sh")
            .arg(script("release-notes.sh"))
            .args([version])
            .arg(&changelog)
            .output()
            .expect("run sh")
    };

    let ok = run("0.1.0");
    let notes = String::from_utf8_lossy(&ok.stdout);
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    assert!(notes.contains("First release."), "{notes}");
    assert!(notes.contains("### Added"), "{notes}");
    assert!(
        !notes.contains("## ["),
        "no other section's heading: {notes}"
    );

    let missing = run("9.9.9");
    assert!(!missing.status.success(), "an unknown version must fail");
}
