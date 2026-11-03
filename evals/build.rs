//! Records the git commit the binary was built from, so a report can say what produced it.

use std::process::Command;

/// Name of the compile-time variable that carries the commit.
const COMMIT_VAR: &str = "MCP_EVALS_GIT_COMMIT";

/// Runs `git` with `args` and returns its trimmed stdout when it succeeds.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src");
    println!("cargo:rerun-if-changed=../.git/HEAD");
    let Some(commit) = git(&["rev-parse", "HEAD"]).filter(|c| !c.is_empty()) else {
        return;
    };
    let dirty =
        git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    let suffix = if dirty { "+dirty" } else { "" };
    println!("cargo:rustc-env={COMMIT_VAR}={commit}{suffix}");
}
