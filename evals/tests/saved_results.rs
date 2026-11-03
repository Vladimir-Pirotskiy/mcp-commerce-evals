//! The committed `results/*.json` files must still parse and render with the current report
//! code, and the tables in the README must be what that code renders from them.

use std::path::{Path, PathBuf};

use mcp_commerce_evals::report::render_saved;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn results_files() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(root().join("results"))
        .expect("results dir")
        .map(|entry| entry.expect("entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    files.sort();
    files
}

fn rendered(path: &Path) -> String {
    let json = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    render_saved(&json).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The Markdown tables in `text`: runs of consecutive lines that start with `|`.
fn tables(text: &str) -> Vec<Vec<&str>> {
    let mut tables: Vec<Vec<&str>> = Vec::new();
    let mut previous_was_row = false;
    for line in text.lines() {
        let is_row = line.starts_with('|');
        if is_row {
            if previous_was_row {
                tables.last_mut().expect("a table").push(line);
            } else {
                tables.push(vec![line]);
            }
        }
        previous_was_row = is_row;
    }
    tables
}

#[test]
fn every_committed_results_file_parses_and_renders() {
    let files = results_files();
    assert!(
        files.len() >= 3,
        "expected the committed results: {files:?}"
    );
    for file in files {
        let text = rendered(&file);
        assert!(text.contains("| **all** |"), "{file:?}: {text}");
    }
}

#[test]
fn a_file_from_before_trials_and_models_were_recorded_still_renders() {
    let json = r#"{
      "tool_level": { "queries": [] },
      "assistant_level": { "model": "m", "queries": [{
        "id": "q", "kind": "k", "text": "t", "expected": ["X"], "final_answer": "ANSWER: X",
        "committed": "X", "correct": true, "mentions_expected": true, "answered": true,
        "stop_reason": "end_turn", "tool_calls": 1, "tools_used": ["search_products"],
        "turns": 2, "input_tokens": 1, "output_tokens": 1, "error": null
      }] }
    }"#;
    let text = render_saved(json).expect("an old file parses");
    assert!(text.contains("Model: m"), "{text}");
}

/// Every table row that the current code renders from a committed file is in the README, so the
/// README cannot drift from the files (or from the report code) unnoticed.
#[test]
fn the_readme_tables_are_what_the_current_code_renders_from_the_results() {
    let readme = std::fs::read_to_string(root().join("README.md")).expect("README");
    for file in results_files() {
        let text = rendered(&file);
        let name = file
            .file_name()
            .expect("name")
            .to_string_lossy()
            .into_owned();
        let wanted = if name.starts_with("tool-level") {
            "| kind | queries | hit@1"
        } else {
            "| kind | queries | committed answer"
        };
        let table = tables(&text)
            .into_iter()
            .find(|table| table[0].starts_with(wanted))
            .unwrap_or_else(|| panic!("{name}: no table starting {wanted:?}"));
        for row in table {
            assert!(
                readme.lines().any(|line| line == row),
                "{name}: this row is not in README.md (run `mcp-commerce-evals render`):\n{row}"
            );
        }
    }
}

#[test]
fn the_render_subcommand_prints_the_report_without_any_network_call() {
    let file = root().join("results/tool-level-2026-11-03.json");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mcp-commerce-evals"))
        .arg("render")
        .arg(&file)
        .output()
        .expect("run the binary");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("| **all** | 40 |"), "{stdout}");
}
