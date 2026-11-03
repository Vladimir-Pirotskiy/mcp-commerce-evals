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

/// Asks git where `name` lives in this checkout and tells cargo to rerun when it changes.
/// `--git-path` resolves a linked worktree, where `.git` is a file: HEAD and the index are in the
/// worktree's own git directory, refs and `packed-refs` in the common one.
fn rerun_if_git_path_changes(name: &str) {
    if let Some(path) = git(&["rev-parse", "--git-path", name]) {
        // A path that does not exist would make cargo rerun this script on every build.
        if std::path::Path::new(&path).exists() {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src");
    // A commit on the current branch moves the branch ref (or packed-refs) and the index, not
    // HEAD, so watching HEAD alone leaves the recorded commit stale.
    for name in ["HEAD", "index", "packed-refs"] {
        rerun_if_git_path_changes(name);
    }
    if let Some(reference) = git(&["symbolic-ref", "--quiet", "HEAD"]) {
        rerun_if_git_path_changes(&reference);
    }
    let Some(commit) = git(&["rev-parse", "HEAD"]).filter(|c| !c.is_empty()) else {
        return;
    };
    let dirty =
        git(&["status", "--porcelain", "--untracked-files=no"]).is_some_and(|s| !s.is_empty());
    let suffix = if dirty { "+dirty" } else { "" };
    println!("cargo:rustc-env={COMMIT_VAR}={commit}{suffix}");
}
