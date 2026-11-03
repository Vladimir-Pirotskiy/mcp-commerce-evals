//! The queries file: what buyers ask and which parts count as a right answer.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

use crate::error::{Error, Result};

/// One buyer query with its expected answers.
#[derive(Debug, Clone, Deserialize)]
pub struct Query {
    /// Unique identifier, for example `q07`.
    pub id: String,
    /// Group used for per-kind results, for example `typo` or `compatibility`.
    pub kind: String,
    /// What the buyer says. The assistant-level run sends this text as the user message.
    pub text: String,
    /// Part ids that count as a right answer; the best rank among them is scored.
    pub expected: Vec<String>,
    /// Arguments for the search tool in the tool-level run. Defaults to `{"query": text}`;
    /// give it when the buyer's constraints are structured filters.
    #[serde(default)]
    pub search: Option<Value>,
}

/// Where the shipped queries file is, relative to the repository root.
const SHIPPED_QUERIES: &str = "evals/queries.json";
/// Where the shipped queries file was when this binary was built from source.
const BUILD_TIME_QUERIES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/queries.json");

/// The queries file to use when none is given: the shipped one under the current directory,
/// else the copy in the source tree this binary was built from.
#[must_use]
pub fn default_path() -> PathBuf {
    prefer_existing(Path::new(SHIPPED_QUERIES), Path::new(BUILD_TIME_QUERIES))
}

fn prefer_existing(first: &Path, fallback: &Path) -> PathBuf {
    if first.exists() { first } else { fallback }.to_path_buf()
}

/// Reads a queries file as text.
///
/// # Errors
///
/// Fails when the file cannot be read.
pub fn read_file(path: &Path) -> Result<String> {
    std::fs::read_to_string(path)
        .map_err(|e| Error::Input(format!("cannot read {}: {e}", path.display())))
}

/// Parses and validates a queries file (a JSON array of queries).
///
/// # Errors
///
/// Fails when the text is not valid JSON of the right shape, or a query is invalid.
pub fn parse(raw: &str) -> Result<Vec<Query>> {
    let queries: Vec<Query> = serde_json::from_str(raw)?;
    validate(&queries)?;
    Ok(queries)
}

/// Reads, parses and validates a queries file.
///
/// # Errors
///
/// Fails when the file cannot be read or holds an invalid query set.
pub fn load(path: &Path) -> Result<Vec<Query>> {
    parse(&read_file(path)?)
}

fn validate(queries: &[Query]) -> Result<()> {
    if queries.is_empty() {
        return Err(Error::Input("the queries file has no queries".to_owned()));
    }
    let mut seen = HashSet::new();
    for q in queries {
        if !seen.insert(q.id.as_str()) {
            return Err(Error::Input(format!("duplicate query id {}", q.id)));
        }
        if q.expected.is_empty() || q.text.trim().is_empty() {
            return Err(Error::Input(format!(
                "query {} needs text and expected ids",
                q.id
            )));
        }
        if q.search.as_ref().is_some_and(|s| !s.is_object()) {
            return Err(Error::Input(format!(
                "query {}: search must be an object",
                q.id
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(json: &str) -> Result<Vec<Query>> {
        let queries: Vec<Query> = serde_json::from_str(json)?;
        validate(&queries).map(|()| queries)
    }

    #[test]
    fn the_default_path_falls_back_to_the_build_time_copy() {
        let missing = Path::new("/no/such/queries.json");
        let present = Path::new(BUILD_TIME_QUERIES);
        assert_eq!(prefer_existing(present, missing), present);
        assert_eq!(prefer_existing(missing, present), present);
        assert!(default_path().exists());
    }

    #[test]
    fn a_minimal_file_loads() {
        let queries = parse(r#"[{"id":"a","kind":"k","text":"chain","expected":["X-1"]}]"#);
        assert_eq!(queries.unwrap().len(), 1);
    }

    #[test]
    fn duplicate_ids_empty_expectations_and_non_object_search_are_rejected() {
        let dup = r#"[{"id":"a","kind":"k","text":"t","expected":["X"]},
                      {"id":"a","kind":"k","text":"t","expected":["X"]}]"#;
        assert!(parse(dup).is_err());
        assert!(parse(r#"[{"id":"a","kind":"k","text":"t","expected":[]}]"#).is_err());
        assert!(
            parse(r#"[{"id":"a","kind":"k","text":"t","expected":["X"],"search":"q"}]"#).is_err()
        );
        assert!(parse("[]").is_err());
    }
}
