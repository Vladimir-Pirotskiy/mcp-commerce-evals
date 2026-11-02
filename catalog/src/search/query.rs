//! Search request and result types.

use crate::compat::{canonical_tags, is_compatible};
use crate::model::{Category, Part};

/// A search request: free text plus optional filters.
#[derive(Debug, Clone, Default)]
pub struct SearchQuery {
    /// Free text; empty means "everything that passes the filters".
    pub text: String,
    /// Keep only this brand (case-insensitive).
    pub brand: Option<String>,
    /// Keep only this category.
    pub category: Option<Category>,
    /// Keep only parts that fit every one of these requirements, such as `11-speed` or `road`.
    pub fits: Vec<String>,
    /// Keep only parts that can ship from the shelf now.
    pub in_stock_only: bool,
    /// Keep only parts priced at or below this many dollars.
    pub max_price_usd: Option<f64>,
    /// Return at most this many hits.
    pub limit: usize,
}

impl SearchQuery {
    /// Applies every filter (not the text) to a part. `fits` must already be canonical.
    pub(crate) fn admits(&self, part: &Part, fits: &[String]) -> bool {
        self.brand
            .as_ref()
            .is_none_or(|b| part.brand.eq_ignore_ascii_case(b))
            && self.category.is_none_or(|c| part.category == c)
            && (!self.in_stock_only || part.stock.is_on_shelf())
            && self.max_price_usd.is_none_or(|max| part.price_usd <= max)
            && is_compatible(&part.fits, fits)
    }

    pub(crate) fn canonical_fits(&self) -> Vec<String> {
        canonical_tags(&self.fits)
    }
}

/// One ranked result.
#[derive(Debug, Clone)]
pub struct Hit<'a> {
    /// The matching part.
    pub part: &'a Part,
    /// Relevance score; higher is better. Only comparable within one search.
    pub score: f64,
}

/// The outcome of a search.
#[derive(Debug, Clone)]
pub struct SearchResults<'a> {
    /// How many parts matched before the `limit` was applied.
    pub total: usize,
    /// The best matches, best first.
    pub hits: Vec<Hit<'a>>,
}
