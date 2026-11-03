//! The commit recorded at build time is the commit checked out now.

use std::process::Command;

#[test]
fn the_recorded_commit_is_the_one_checked_out() {
    let Some(head) = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    else {
        eprintln!("skipped: git is not available or this is not a checkout");
        return;
    };
    let Some(recorded) = option_env!("MCP_EVALS_GIT_COMMIT") else {
        panic!("git reports HEAD {head} but the build recorded no commit");
    };

    assert_eq!(recorded.trim_end_matches("+dirty"), head);
}
