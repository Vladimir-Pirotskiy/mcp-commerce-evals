//! A BM25 index over weighted fields, with synonym and typo expansion of query terms.

use std::collections::HashMap;

use crate::model::Part;
use crate::text::{distance_within, query_tokens, synonyms, tokens};

/// BM25 term-frequency saturation: how quickly repeating a word stops adding score. 1.2 is the
/// usual default; the fields are short, so a higher value would only reward keyword stuffing.
const K1: f64 = 1.2;
/// BM25 length normalisation, from 0 (ignore length) to 1 (fully normalise). 0.75 is the usual
/// default and keeps a long description from drowning a short, precise title.
const B: f64 = 0.75;
/// Fraction of its score a document keeps however few of the query's words it matches; the rest
/// grows with coverage, so matching one of two words keeps `0.6 + 0.4 / 2 = 0.8` of the score.
/// Low enough that covering every word wins, high enough that one rare word still finds a part.
const COVERAGE_FLOOR: f64 = 0.6;
/// Weight of a synonym expansion relative to the word the buyer typed.
const SYNONYM_WEIGHT: f64 = 0.8;
/// Weight of a typo correction relative to an exact word.
const FUZZY_WEIGHT: f64 = 0.7;

/// How many times a token counts when it appears in each field.
const W_ID: f64 = 3.0;
const W_TITLE: f64 = 3.0;
const W_BRAND: f64 = 2.0;
const W_CATEGORY: f64 = 2.0;
const W_ATTRIBUTE: f64 = 1.5;
/// An attribute's name (`speed`, `compound`) counts a third of its value: the name alone says
/// little about what a part is, but it keeps a query such as "tooth range" useful.
const W_ATTRIBUTE_KEY: f64 = W_ATTRIBUTE / 3.0;
const W_FITS: f64 = 1.5;
const W_DESCRIPTION: f64 = 1.0;

/// A count as a float. Counts here are documents and words, far below 2^52, so it is exact.
#[expect(
    clippy::cast_precision_loss,
    reason = "counts of documents and words are far below 2^52"
)]
fn count(n: usize) -> f64 {
    n as f64
}

/// One query word and the index terms it may stand for.
struct Slot {
    alternatives: Vec<(String, f64)>,
}

/// Inverted index with weighted term frequencies.
pub(crate) struct Index {
    postings: HashMap<String, Vec<(usize, f64)>>,
    doc_len: Vec<f64>,
    avg_len: f64,
}

impl Index {
    pub(crate) fn build(parts: &[Part]) -> Self {
        let mut postings: HashMap<String, HashMap<usize, f64>> = HashMap::new();
        let mut doc_len = vec![0.0; parts.len()];
        for (doc, part) in parts.iter().enumerate() {
            let mut add = |text: &str, weight: f64| {
                for token in tokens(text) {
                    *postings.entry(token).or_default().entry(doc).or_insert(0.0) += weight;
                    doc_len[doc] += weight;
                }
            };
            add(&part.id, W_ID);
            add(&part.title, W_TITLE);
            add(&part.brand, W_BRAND);
            add(part.category.label(), W_CATEGORY);
            for (key, value) in &part.attributes {
                add(key, W_ATTRIBUTE_KEY);
                add(value, W_ATTRIBUTE);
            }
            for tag in &part.fits {
                add(tag, W_FITS);
            }
            add(&part.description, W_DESCRIPTION);
        }
        let avg_len = doc_len.iter().sum::<f64>() / count(parts.len().max(1));
        let postings = postings
            .into_iter()
            .map(|(term, docs)| {
                let mut docs: Vec<_> = docs.into_iter().collect();
                docs.sort_by_key(|(doc, _)| *doc);
                (term, docs)
            })
            .collect();
        Self {
            postings,
            doc_len,
            avg_len,
        }
    }

    /// Scores the documents allowed by `allowed` against `text`; documents with no matching
    /// word are left out.
    pub(crate) fn score(&self, text: &str, allowed: &[bool]) -> Vec<(usize, f64)> {
        let slots = self.slots(text);
        if slots.is_empty() {
            return Vec::new();
        }
        let n = count(self.doc_len.len());
        let mut totals: HashMap<usize, (f64, usize)> = HashMap::new();
        for slot in &slots {
            let mut best: HashMap<usize, f64> = HashMap::new();
            for (term, weight) in &slot.alternatives {
                let Some(docs) = self.postings.get(term) else {
                    continue;
                };
                let df = count(docs.len());
                let idf = (1.0 + (n - df + 0.5) / (df + 0.5)).ln();
                for &(doc, tf) in docs.iter().filter(|(doc, _)| allowed[*doc]) {
                    let norm = K1 * (1.0 - B + B * self.doc_len[doc] / self.avg_len);
                    let score = weight * idf * tf * (K1 + 1.0) / (tf + norm);
                    let entry = best.entry(doc).or_insert(0.0);
                    *entry = entry.max(score);
                }
            }
            for (doc, score) in best {
                let entry = totals.entry(doc).or_insert((0.0, 0));
                entry.0 += score;
                entry.1 += 1;
            }
        }
        let slot_count = count(slots.len());
        totals
            .into_iter()
            .map(|(doc, (sum, matched))| {
                // Reward documents that cover more of the query's words.
                let coverage = count(matched) / slot_count;
                (
                    doc,
                    sum * (COVERAGE_FLOOR + (1.0 - COVERAGE_FLOOR) * coverage),
                )
            })
            .collect()
    }

    fn slots(&self, text: &str) -> Vec<Slot> {
        query_tokens(text)
            .into_iter()
            .map(|token| {
                let mut alternatives: Vec<(String, f64)> = Vec::new();
                let mut push = |term: &str, weight: f64| {
                    if !alternatives.iter().any(|(t, _)| t == term) {
                        alternatives.push((term.to_owned(), weight));
                    }
                };
                let known = self.postings.contains_key(&token);
                if known {
                    push(&token, 1.0);
                } else if let Some(fixed) = self.correction(&token) {
                    push(&fixed, FUZZY_WEIGHT);
                }
                for synonym in synonyms(&token, !known) {
                    push(synonym, SYNONYM_WEIGHT);
                }
                Slot { alternatives }
            })
            .collect()
    }

    /// The closest indexed word to a word that is not indexed, if one is close enough.
    fn correction(&self, token: &str) -> Option<String> {
        let len = token.chars().count();
        if len < 4 || token.chars().any(|c| c.is_ascii_digit()) {
            return None;
        }
        let max = if len >= 8 { 2 } else { 1 };
        self.postings
            .iter()
            .filter(|(term, _)| !term.chars().any(|c| c.is_ascii_digit()))
            .filter_map(|(term, docs)| {
                let d = distance_within(token, term, max)?;
                Some((d, std::cmp::Reverse(docs.len()), term))
            })
            .min()
            .map(|(_, _, term)| term.clone())
    }
}
