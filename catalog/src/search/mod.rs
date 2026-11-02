//! Catalog search: BM25 text relevance, typo tolerance, synonyms and filters.

mod index;
mod query;

use std::cmp::Ordering;
use std::collections::HashMap;

pub use query::{Hit, SearchQuery, SearchResults};

use crate::model::Part;
use crate::text::compact;
use index::Index;

/// Added to a part whose number the buyer typed in full, so it always ranks first.
const EXACT_ID_BONUS: f64 = 1000.0;

/// An indexed, searchable set of parts.
pub struct Catalog {
    parts: Vec<Part>,
    by_id: HashMap<String, usize>,
    index: Index,
}

impl Catalog {
    /// Indexes `parts`.
    pub fn new(parts: Vec<Part>) -> Self {
        let by_id = parts
            .iter()
            .enumerate()
            .map(|(i, p)| (compact(&p.id), i))
            .collect();
        let index = Index::build(&parts);
        Self {
            parts,
            by_id,
            index,
        }
    }

    /// Every part, in catalog order.
    pub fn parts(&self) -> &[Part] {
        &self.parts
    }

    /// Looks a part up by its number, ignoring case and punctuation (`vel ch 1042`).
    pub fn get(&self, id: &str) -> Option<&Part> {
        self.by_id.get(&compact(id)).map(|&i| &self.parts[i])
    }

    /// Runs a search: filters first, then ranks by relevance; with no text, ranks shelf stock
    /// first, then price.
    pub fn search(&self, query: &SearchQuery) -> SearchResults<'_> {
        let fits = query.canonical_fits();
        let allowed: Vec<bool> = self.parts.iter().map(|p| query.admits(p, &fits)).collect();
        let mut scored: Vec<(usize, f64)> = if query.text.trim().is_empty() {
            allowed
                .iter()
                .enumerate()
                .filter(|(_, ok)| **ok)
                .map(|(i, _)| (i, 0.0))
                .collect()
        } else {
            self.index.score(&query.text, &allowed)
        };
        if let Some(&i) = self
            .by_id
            .get(&compact(&query.text))
            .filter(|&&i| allowed[i])
        {
            match scored.iter_mut().find(|(doc, _)| *doc == i) {
                Some(entry) => entry.1 += EXACT_ID_BONUS,
                None => scored.push((i, EXACT_ID_BONUS)),
            }
        }
        scored.sort_by(|a, b| self.order(a, b));
        let total = scored.len();
        let hits = scored
            .into_iter()
            .take(query.limit)
            .map(|(i, score)| Hit {
                part: &self.parts[i],
                score,
            })
            .collect();
        SearchResults { total, hits }
    }

    fn order(&self, a: &(usize, f64), b: &(usize, f64)) -> Ordering {
        let (pa, pb) = (&self.parts[a.0], &self.parts[b.0]);
        b.1.total_cmp(&a.1)
            .then(pb.stock.is_on_shelf().cmp(&pa.stock.is_on_shelf()))
            .then(pa.price_usd.total_cmp(&pb.price_usd))
            .then_with(|| pa.id.cmp(&pb.id))
    }
}

#[cfg(test)]
mod tests;
