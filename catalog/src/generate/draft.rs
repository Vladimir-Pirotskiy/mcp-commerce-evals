//! Builder for one generated part, before brand, id and stock are assigned.

use std::collections::BTreeMap;

use crate::rng::Rng;

/// A part under construction.
pub(super) struct Draft {
    pub name: String,
    pub price_dollars: u32,
    pub attributes: BTreeMap<String, String>,
    pub fits: Vec<String>,
    pub sentences: Vec<String>,
}

impl Draft {
    pub(super) fn new(name: impl Into<String>, price_dollars: u32) -> Self {
        Self {
            name: name.into(),
            price_dollars,
            attributes: BTreeMap::new(),
            fits: Vec::new(),
            sentences: Vec::new(),
        }
    }

    pub(super) fn attr(mut self, key: &str, value: impl ToString) -> Self {
        self.attributes.insert(key.to_owned(), value.to_string());
        self
    }

    pub(super) fn fit(mut self, tag: impl ToString) -> Self {
        self.fits.push(tag.to_string());
        self
    }

    pub(super) fn fits(mut self, tags: &[&str]) -> Self {
        self.fits.extend(tags.iter().map(ToString::to_string));
        self
    }

    /// Adds a sentence, capitalizing its first letter.
    pub(super) fn say(mut self, sentence: impl Into<String>) -> Self {
        let sentence = sentence.into();
        let mut chars = sentence.chars();
        let capitalized = chars
            .next()
            .map(|first| first.to_uppercase().chain(chars).collect())
            .unwrap_or_default();
        self.sentences.push(capitalized);
        self
    }
}

/// Speeds a drivetrain part can have.
pub(super) const SPEEDS: [u32; 5] = [8, 9, 10, 11, 12];

/// The `N-speed` tag.
pub(super) fn speed_tag(speed: u32) -> String {
    format!("{speed}-speed")
}

/// Bike types that commonly run a drivetrain with this many speeds.
pub(super) fn bikes_for_speed(speed: u32) -> &'static [&'static str] {
    match speed {
        8 => &["commuter", "mountain"],
        9 => &["commuter", "mountain", "road"],
        10 => &["road", "gravel", "mountain"],
        11 => &["road", "gravel", "mountain"],
        _ => &["road", "gravel", "mountain", "e-bike"],
    }
}

/// A base price plus a random extra of up to `extra` dollars.
pub(super) fn spread(rng: &mut Rng, base: u32, extra: u32) -> u32 {
    base + rng.range(0, extra)
}
