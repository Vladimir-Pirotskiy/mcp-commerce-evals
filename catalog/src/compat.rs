//! Compatibility tags and the rule that decides whether a part fits a bike.
//!
//! A tag belongs to a dimension (drivetrain speed, bike type, wheel size, brake type, axle
//! standard). A part that lists no tag in a dimension does not care about it; a part that lists
//! some tags in a dimension fits only a bike that has one of them.

use crate::text::{canonical_tag, synonyms};

/// Drivetrain speeds.
pub const SPEEDS: [&str; 5] = ["8-speed", "9-speed", "10-speed", "11-speed", "12-speed"];
/// Bike types.
pub const BIKES: [&str; 5] = ["road", "gravel", "mountain", "commuter", "e-bike"];
/// Wheel sizes.
pub const WHEELS: [&str; 5] = ["700c", "650b", "26in", "27.5in", "29in"];
/// Brake types.
pub const BRAKES: [&str; 2] = ["rim-brake", "disc-brake"];
/// Axle standards.
pub const AXLES: [&str; 4] = ["quick-release", "12x100", "12x142", "15x110"];

const DIMENSIONS: [&[&str]; 5] = [&SPEEDS, &BIKES, &WHEELS, &BRAKES, &AXLES];

/// Every known compatibility tag.
pub fn all_tags() -> impl Iterator<Item = &'static str> {
    DIMENSIONS.iter().flat_map(|d| d.iter().copied())
}

fn dimension_of(tag: &str) -> Option<&'static [&'static str]> {
    DIMENSIONS.iter().copied().find(|d| d.contains(&tag))
}

/// True when a part with compatibility tags `fits` works with every requirement in `required`.
///
/// Requirements must already be canonical (see [`canonical_tag`]). An unknown requirement only
/// matches a part that lists it literally.
pub fn is_compatible(fits: &[String], required: &[String]) -> bool {
    required.iter().all(|req| match dimension_of(req) {
        Some(dim) => {
            let constrained = fits.iter().any(|t| dim.contains(&t.as_str()));
            !constrained || fits.contains(req)
        }
        None => fits.contains(req),
    })
}

/// The canonical form of one requirement. A word that is not a tag but is a query synonym for
/// one (`mtb` for `mountain`) becomes that tag, like the same word typed in the search text.
fn canonical_requirement(raw: &str) -> String {
    let tag = canonical_tag(raw);
    if dimension_of(&tag).is_some() {
        return tag;
    }
    synonyms(&tag, false)
        .iter()
        .map(|word| canonical_tag(word))
        .find(|candidate| dimension_of(candidate).is_some())
        .unwrap_or(tag)
}

/// Canonicalizes free-form requirements such as `11 speed`, `Disc Brakes` or `mtb`.
pub fn canonical_tags(raw: &[String]) -> Vec<String> {
    raw.iter().map(|t| canonical_requirement(t)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(items: &[&str]) -> Vec<String> {
        items.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn unconstrained_dimension_always_fits() {
        assert!(is_compatible(&tags(&["road"]), &tags(&["11-speed"])));
        assert!(is_compatible(&[], &tags(&["11-speed", "disc-brake"])));
    }

    #[test]
    fn constrained_dimension_must_contain_the_tag() {
        let chain = tags(&["11-speed", "road", "gravel"]);
        assert!(is_compatible(&chain, &tags(&["11-speed", "road"])));
        assert!(!is_compatible(&chain, &tags(&["12-speed"])));
        assert!(!is_compatible(&chain, &tags(&["mountain"])));
    }

    #[test]
    fn unknown_requirement_needs_a_literal_match() {
        assert!(!is_compatible(&tags(&["road"]), &tags(&["tandem"])));
        assert!(is_compatible(&tags(&["tandem"]), &tags(&["tandem"])));
    }

    #[test]
    fn a_query_synonym_maps_to_the_catalog_tag() {
        let raw = tags(&["MTB", "city", "tandem"]);
        assert_eq!(
            canonical_tags(&raw),
            tags(&["mountain", "commuter", "tandem"])
        );
    }

    #[test]
    fn free_form_requirements_are_canonicalized() {
        let raw = tags(&["11 speed", "Disc Brakes", "29er"]);
        assert_eq!(
            canonical_tags(&raw),
            tags(&["11-speed", "disc-brake", "29in"])
        );
    }
}
