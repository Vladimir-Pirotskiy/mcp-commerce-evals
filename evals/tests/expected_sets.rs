//! Re-derives every expected id set in `queries.json` from the seeded catalog's attributes, with
//! one plain predicate per query, and asserts the file says the same. The sets are therefore
//! checkable without trusting the search engine: nothing here calls it.

use std::collections::BTreeSet;
use std::path::Path;

use catalog::{Catalog, Category, Part, demo_catalog};
use mcp_commerce_evals::queries;

const QUERIES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/queries.json");

/// A rule that selects the parts a buyer's sentence describes.
type Rule = fn(&Part) -> bool;

fn attr<'a>(part: &'a Part, key: &str) -> Option<&'a str> {
    part.attributes.get(key).map(String::as_str)
}

fn is(part: &Part, category: Category, attributes: &[(&str, &str)]) -> bool {
    part.category == category
        && attributes
            .iter()
            .all(|(key, value)| attr(part, key) == Some(value))
}

fn fits(part: &Part, tag: &str) -> bool {
    part.fits.iter().any(|t| t == tag)
}

/// The store's "in stock only" filter: can ship from the shelf now.
fn on_shelf(part: &Part) -> bool {
    part.stock.is_on_shelf()
}

/// One rule per query id. The identity rules (q01 to q06) spell the part number the buyer meant,
/// including q06, whose text has a typo on purpose.
const RULES: &[(&str, Rule)] = &[
    ("q01", |p| p.id == "FER-CH-4296"),
    ("q02", |p| p.id == "BRK-CS-3163"),
    ("q03", |p| p.id == "DUN-RT-8582"),
    ("q04", |p| p.id == "QUI-PD-5690"),
    ("q05", |p| p.id == "ALD-GR-7369"),
    ("q06", |p| p.id == "STO-HT-1064"),
    ("q07", |p| {
        p.brand == "Pellmoor"
            && is(
                p,
                Category::Saddle,
                &[
                    ("width_mm", "175"),
                    ("shape", "flat"),
                    ("rails", "chromoly"),
                ],
            )
    }),
    ("q08", |p| {
        is(
            p,
            Category::HandlebarTape,
            &[("colour", "white"), ("material", "cork")],
        )
    }),
    ("q09", |p| {
        is(
            p,
            Category::Grips,
            &[
                ("diameter_mm", "34"),
                ("colour", "grey"),
                ("lock_on", "yes"),
            ],
        )
    }),
    ("q10", |p| {
        is(
            p,
            Category::ChainLube,
            &[("type", "wet"), ("volume_ml", "250")],
        )
    }),
    ("q11", |p| {
        is(
            p,
            Category::Pedals,
            &[("pedal_type", "clipless road"), ("body", "magnesium")],
        )
    }),
    ("q12", |p| {
        is(
            p,
            Category::Rotor,
            &[
                ("mount", "centerlock"),
                ("diameter_mm", "160"),
                ("thickness_mm", "1.8"),
            ],
        )
    }),
    ("q13", |p| {
        is(
            p,
            Category::Tire,
            &[
                ("casing", "tubeless-ready"),
                ("wheel_size", "650b"),
                ("width", "50c"),
            ],
        )
    }),
    ("q14", |p| {
        is(
            p,
            Category::RearDerailleur,
            &[("cage", "long"), ("clutch", "yes"), ("speed", "12-speed")],
        )
    }),
    ("q15", |p| {
        is(p, Category::Chain, &[("speed", "11-speed")]) && fits(p, "road")
    }),
    ("q16", |p| {
        is(p, Category::Cassette, &[("speed", "10-speed")]) && fits(p, "mountain")
    }),
    ("q17", |p| {
        is(p, Category::BrakePads, &[("brake_type", "rim-brake")]) && fits(p, "road")
    }),
    ("q18", |p| {
        is(p, Category::Tire, &[("wheel_size", "29in")]) && fits(p, "mountain")
    }),
    ("q19", |p| {
        is(
            p,
            Category::Axle,
            &[("standard", "15x110"), ("position", "front")],
        )
    }),
    ("q20", |p| {
        p.category == Category::Rotor && fits(p, "e-bike")
    }),
    ("q21", |p| {
        is(p, Category::Chainring, &[("speed", "12-speed")]) && fits(p, "road")
    }),
    ("q22", |p| {
        is(p, Category::Hub, &[("axle_standard", "12x142")])
            && fits(p, "11-speed")
            && fits(p, "disc-brake")
    }),
    ("q23", |p| {
        is(p, Category::Cassette, &[("tooth_range", "11-34T")])
    }),
    ("q24", |p| is(p, Category::ChainLube, &[("type", "wet")])),
    ("q25", |p| {
        is(
            p,
            Category::BrakePads,
            &[("brake_type", "disc-brake"), ("compound", "resin")],
        )
    }),
    ("q26", |p| {
        is(p, Category::RearDerailleur, &[("speed", "11-speed")])
    }),
    ("q27", |p| {
        is(p, Category::BottleCage, &[("material", "carbon")])
    }),
    ("q28", |p| {
        is(p, Category::Pedals, &[("pedal_type", "clipless mountain")])
    }),
    ("q29", |p| {
        is(
            p,
            Category::Tire,
            &[("wheel_size", "29in"), ("width", "2.4")],
        )
    }),
    ("q30", |p| {
        is(
            p,
            Category::Cassette,
            &[("speed", "10-speed"), ("tooth_range", "10-51T")],
        )
    }),
    ("q31", |p| {
        is(
            p,
            Category::Light,
            &[("position", "front"), ("lumens", "1000")],
        )
    }),
    ("q32", |p| is(p, Category::ChainLube, &[("type", "dry")])),
    ("q33", |p| {
        is(p, Category::Axle, &[("standard", "quick-release")]) && fits(p, "road")
    }),
    ("q34", |p| {
        is(p, Category::RearDerailleur, &[("speed", "11-speed")])
    }),
    ("q35", |p| {
        is(p, Category::Chain, &[("speed", "11-speed")]) && on_shelf(p) && p.price_usd < 30.0
    }),
    ("q36", |p| {
        is(p, Category::BrakePads, &[("brake_type", "disc-brake")])
            && on_shelf(p)
            && p.price_usd < 20.0
    }),
    ("q37", |p| {
        is(p, Category::Saddle, &[("rails", "carbon")]) && p.price_usd < 60.0
    }),
    ("q38", |p| {
        is(p, Category::Tire, &[("wheel_size", "29in")]) && on_shelf(p) && p.price_usd < 30.0
    }),
    ("q39", |p| {
        is(p, Category::Light, &[("position", "rear")]) && p.brand == "Ferrowick" && on_shelf(p)
    }),
    ("q40", |p| {
        is(
            p,
            Category::Tube,
            &[("valve", "presta"), ("wheel_size", "700c")],
        ) && on_shelf(p)
            && p.price_usd < 8.0
    }),
];

fn derived(catalog: &Catalog, rule: Rule) -> BTreeSet<String> {
    catalog
        .parts()
        .iter()
        .filter(|part| rule(part))
        .map(|part| part.id.clone())
        .collect()
}

#[test]
fn every_expected_set_is_rederived_from_catalog_attributes() {
    let queries = queries::load(Path::new(QUERIES)).unwrap();
    let catalog = demo_catalog();
    let mut mismatches = Vec::new();
    for query in &queries {
        let (_, rule) = RULES
            .iter()
            .find(|(id, _)| *id == query.id)
            .unwrap_or_else(|| panic!("no rule for {}", query.id));
        let from_file: BTreeSet<String> = query.expected.iter().cloned().collect();
        let from_catalog = derived(&catalog, *rule);
        if from_file != from_catalog {
            mismatches.push(format!(
                "{} {:?}: file {from_file:?}, catalog {from_catalog:?}",
                query.id, query.text
            ));
        }
    }
    assert!(mismatches.is_empty(), "{}", mismatches.join("\n"));
}

#[test]
fn every_query_has_exactly_one_rule() {
    let queries = queries::load(Path::new(QUERIES)).unwrap();
    assert_eq!(RULES.len(), queries.len());
    for query in &queries {
        assert_eq!(
            RULES.iter().filter(|(id, _)| *id == query.id).count(),
            1,
            "{}",
            query.id
        );
    }
}
