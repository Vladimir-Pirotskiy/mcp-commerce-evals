//! A BM25 index over weighted fields, with synonym and typo expansion of query terms.

use std::collections::HashMap;

use crate::model::Part;
use crate::text::{distance_within, query_tokens, synonyms, tokens};

const K1: f64 = 1.2;
const B: f64 = 0.75;
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
const W_FITS: f64 = 1.5;
const W_DESCRIPTION: f64 = 1.0;

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
                add(key, W_ATTRIBUTE / 3.0);
                add(value, W_ATTRIBUTE);
            }
            for tag in &part.fits {
                add(tag, W_FITS);
            }
            add(&part.description, W_DESCRIPTION);
        }
        let avg_len = doc_len.iter().sum::<f64>() / parts.len().max(1) as f64;
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
        let n = self.doc_len.len() as f64;
        let mut totals: HashMap<usize, (f64, usize)> = HashMap::new();
        for slot in &slots {
            let mut best: HashMap<usize, f64> = HashMap::new();
            for (term, weight) in &slot.alternatives {
                let Some(docs) = self.postings.get(term) else {
                    continue;
                };
                let idf = (1.0 + (n - docs.len() as f64 + 0.5) / (docs.len() as f64 + 0.5)).ln();
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
        let slot_count = slots.len() as f64;
        totals
            .into_iter()
            .map(|(doc, (sum, matched))| {
                // Reward documents that cover more of the query's words.
                (doc, sum * (0.6 + 0.4 * matched as f64 / slot_count))
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
