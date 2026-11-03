//! Validation of tool arguments before they reach the catalog.
//!
//! Every free-text input has a ceiling, so one request cannot make the store do unbounded work
//! or echo unbounded text.

use catalog::{SearchQuery, query_tokens};

use crate::api::{MAX_LIMIT, MIN_LIMIT, SearchArgs};
use crate::error::ToolError;

/// Results returned when the caller gives no `limit`.
const DEFAULT_LIMIT: u32 = 5;
/// Longest search query, in characters.
pub const MAX_QUERY_CHARS: usize = 200;
/// Most words in a search query. Each word is compared against the whole vocabulary.
pub const MAX_QUERY_WORDS: usize = 20;
/// Most `fits` requirements in one search.
pub const MAX_FITS: usize = 10;
/// Longest brand, `fits` entry or part id, in characters.
pub const MAX_FIELD_CHARS: usize = 100;

/// Refuses `value` when it is longer than [`MAX_FIELD_CHARS`] characters.
pub fn bounded(field: &'static str, value: &str) -> Result<(), ToolError> {
    // A byte length above four times the limit is too long whatever the characters are, so the
    // count never walks an unbounded string.
    if value.len() > MAX_FIELD_CHARS * 4 || value.chars().count() > MAX_FIELD_CHARS {
        return Err(ToolError::TooLong {
            field,
            max: MAX_FIELD_CHARS,
        });
    }
    Ok(())
}

fn limit(requested: Option<u32>) -> Result<usize, ToolError> {
    let limit = requested.unwrap_or(DEFAULT_LIMIT);
    if !(MIN_LIMIT..=MAX_LIMIT).contains(&limit) {
        return Err(ToolError::Limit {
            min: MIN_LIMIT,
            max: MAX_LIMIT,
        });
    }
    Ok(limit as usize)
}

fn fits(entries: Vec<String>) -> Result<Vec<String>, ToolError> {
    if entries.len() > MAX_FITS {
        return Err(ToolError::TooManyFits { max: MAX_FITS });
    }
    for entry in &entries {
        bounded("fits entry", entry)?;
        if entry.trim().is_empty() {
            return Err(ToolError::BlankFit);
        }
    }
    Ok(entries)
}

/// The catalog query for `args`, or the reason the arguments are refused.
pub fn search_query(args: SearchArgs) -> Result<SearchQuery, ToolError> {
    let text = args.query.unwrap_or_default();
    if text.len() > MAX_QUERY_CHARS * 4 || text.chars().count() > MAX_QUERY_CHARS {
        return Err(ToolError::TooLong {
            field: "query",
            max: MAX_QUERY_CHARS,
        });
    }
    if query_tokens(&text).len() > MAX_QUERY_WORDS {
        return Err(ToolError::TooManyWords {
            max: MAX_QUERY_WORDS,
        });
    }
    if let Some(brand) = &args.brand {
        bounded("brand", brand)?;
    }
    Ok(SearchQuery {
        text,
        brand: args.brand,
        category: args.category,
        fits: fits(args.fits.unwrap_or_default())?,
        in_stock_only: args.in_stock_only.unwrap_or(false),
        max_price_usd: args.max_price_usd,
        limit: limit(args.limit)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args() -> SearchArgs {
        SearchArgs {
            query: None,
            brand: None,
            category: None,
            fits: None,
            in_stock_only: None,
            max_price_usd: None,
            limit: None,
        }
    }

    #[test]
    fn an_empty_search_gets_the_default_limit() {
        assert_eq!(search_query(args()).unwrap().limit, 5);
    }

    #[test]
    fn a_limit_outside_the_range_is_refused() {
        for bad in [0, 26, u32::MAX] {
            let refused = search_query(SearchArgs {
                limit: Some(bad),
                ..args()
            });
            assert_eq!(
                refused.unwrap_err().to_string(),
                "limit must be between 1 and 25."
            );
        }
    }

    #[test]
    fn a_query_with_too_many_words_is_refused_with_advice() {
        let query = vec!["chain"; MAX_QUERY_WORDS + 1].join(" ");
        let refused = search_query(SearchArgs {
            query: Some(query),
            ..args()
        });
        assert_eq!(
            refused.unwrap_err().to_string(),
            "The query has more than 20 words. Keep the few that name the part."
        );
    }

    #[test]
    fn blank_and_oversize_fits_entries_are_refused() {
        let blank = search_query(SearchArgs {
            fits: Some(vec!["road".into(), "  ".into()]),
            ..args()
        });
        assert_eq!(blank.unwrap_err(), ToolError::BlankFit);
        let long = search_query(SearchArgs {
            fits: Some(vec!["x".repeat(MAX_FIELD_CHARS + 1)]),
            ..args()
        });
        assert!(matches!(
            long.unwrap_err(),
            ToolError::TooLong {
                field: "fits entry",
                ..
            }
        ));
    }

    #[test]
    fn bounded_counts_characters_not_bytes() {
        assert!(bounded("brand", &"é".repeat(MAX_FIELD_CHARS)).is_ok());
        assert!(bounded("brand", &"é".repeat(MAX_FIELD_CHARS + 1)).is_err());
    }
}
