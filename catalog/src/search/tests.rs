//! Ranking tests on a small hand-made catalog, plus properties of the generated one.

use std::collections::BTreeMap;

use super::*;
use crate::model::{Category, StockStatus};
use crate::{DEMO_SEED, DEMO_SIZE, generate};

fn part(id: &str, title: &str, category: Category, price: f64, fits: &[&str]) -> Part {
    Part {
        id: id.to_owned(),
        title: title.to_owned(),
        description: String::new(),
        brand: "Aldermoor".to_owned(),
        category,
        price_usd: price,
        stock: StockStatus::InStock,
        quantity_available: 10,
        restock_days: None,
        attributes: BTreeMap::new(),
        fits: fits.iter().map(ToString::to_string).collect(),
    }
}

fn shelf() -> Catalog {
    let mut chain10 = part(
        "ALD-CH-1001",
        "Aldermoor 10-Speed Chain",
        Category::Chain,
        20.0,
        &["10-speed", "road"],
    );
    chain10.description = "A chain for 10-speed road drivetrains.".to_owned();
    let mut chain11 = part(
        "ALD-CH-1002",
        "Aldermoor 11-Speed Chain",
        Category::Chain,
        28.0,
        &["11-speed", "road"],
    );
    chain11.description = "A chain for 11-speed road drivetrains.".to_owned();
    let mut chain11_mtb = part(
        "ALD-CH-1003",
        "Aldermoor 11-Speed Chain",
        Category::Chain,
        24.0,
        &["11-speed", "mountain"],
    );
    chain11_mtb.description = "A chain for 11-speed mountain drivetrains.".to_owned();
    let mut cassette = part(
        "ALD-CS-2001",
        "Aldermoor 11-Speed 11-34T Cassette",
        Category::Cassette,
        70.0,
        &["11-speed", "gravel"],
    );
    cassette
        .attributes
        .insert("tooth_range".to_owned(), "11-34T".to_owned());
    let mut rotor = part(
        "ALD-RT-3001",
        "Aldermoor 160mm Disc Rotor",
        Category::Rotor,
        40.0,
        &["disc-brake"],
    );
    rotor.brand = "Brackenridge".to_owned();
    let mut tire = part(
        "ALD-TR-4001",
        "Aldermoor 700x28c Road Tire",
        Category::Tire,
        45.0,
        &["700c", "road"],
    );
    tire.stock = StockStatus::Backorder;
    tire.quantity_available = 0;
    tire.restock_days = Some(14);
    let lube = part(
        "ALD-LU-5001",
        "Aldermoor Wet Chain Lube",
        Category::ChainLube,
        9.0,
        &[],
    );
    Catalog::new(vec![
        chain10,
        chain11,
        chain11_mtb,
        cassette,
        rotor,
        tire,
        lube,
    ])
    .unwrap()
}

fn ids(results: &SearchResults<'_>) -> Vec<String> {
    results.hits.iter().map(|h| h.part.id.clone()).collect()
}

fn ask(catalog: &Catalog, text: &str) -> Vec<String> {
    ids(&catalog.search(&SearchQuery {
        text: text.to_owned(),
        limit: 10,
        ..Default::default()
    }))
}

#[test]
fn a_full_part_number_ranks_first_in_any_spelling() {
    let catalog = shelf();
    for text in ["ALD-CH-1002", "ald ch 1002", "aldch1002"] {
        assert_eq!(
            ask(&catalog, text).first().map(String::as_str),
            Some("ALD-CH-1002"),
            "{text}"
        );
    }
}

#[test]
fn speed_spellings_pick_the_right_chain() {
    let catalog = shelf();
    for text in [
        "11spd chain",
        "chain 11 speed",
        "11s chain",
        "11-speed chain",
    ] {
        let found = ask(&catalog, text);
        assert!(
            ["ALD-CH-1002", "ALD-CH-1003"].contains(&found[0].as_str()),
            "{text}: {found:?}"
        );
        assert_ne!(found[0], "ALD-CH-1001", "{text}");
    }
}

#[test]
fn compatibility_words_break_the_tie_between_equal_chains() {
    let found = ask(&shelf(), "a chain that fits my 11-speed road bike");
    assert_eq!(found[0], "ALD-CH-1002");
    let found = ask(&shelf(), "11-speed mountain chain");
    assert_eq!(found[0], "ALD-CH-1003");
}

#[test]
fn a_misspelled_word_still_finds_the_part() {
    let catalog = shelf();
    assert_eq!(ask(&catalog, "cassete")[0], "ALD-CS-2001");
    assert_eq!(ask(&catalog, "dics rotor")[0], "ALD-RT-3001");
    assert_eq!(ask(&catalog, "lubricent")[0], "ALD-LU-5001");
}

#[test]
fn a_synonym_finds_the_part() {
    let catalog = shelf();
    assert_eq!(ask(&catalog, "road tyre")[0], "ALD-TR-4001");
    assert_eq!(ask(&catalog, "cogset")[0], "ALD-CS-2001");
}

#[test]
fn a_catalog_word_one_typo_from_a_synonym_key_is_not_rewritten() {
    let mut tire = part("ALD-TR-4002", "Gravel Tire", Category::Tire, 40.0, &[]);
    tire.attributes
        .insert("colour".to_owned(), "black".to_owned());
    let pad = part(
        "ALD-BP-6001",
        "Disc Brake Pad",
        Category::BrakePads,
        15.0,
        &[],
    );
    let catalog = Catalog::new(vec![tire, pad]).unwrap();
    assert_eq!(ask(&catalog, "black"), ["ALD-TR-4002"]);
    assert_eq!(ask(&catalog, "block")[0], "ALD-BP-6001");
}

#[test]
fn a_tooth_range_is_found_by_its_numbers() {
    assert_eq!(ask(&shelf(), "11-34 cassette")[0], "ALD-CS-2001");
}

#[test]
fn nothing_matches_gibberish() {
    assert_eq!(ask(&shelf(), "zzzxqv"), Vec::<String>::new());
}

#[test]
fn the_fits_filter_drops_incompatible_parts_and_keeps_universal_ones() {
    let catalog = shelf();
    let query = SearchQuery {
        text: "chain".to_owned(),
        fits: vec!["11 speed".to_owned(), "Road".to_owned()],
        limit: 10,
        ..Default::default()
    };
    let found = ids(&catalog.search(&query));
    assert!(found.contains(&"ALD-CH-1002".to_owned()));
    assert!(
        found.contains(&"ALD-LU-5001".to_owned()),
        "lube has no tags, so it fits"
    );
    assert!(!found.contains(&"ALD-CH-1001".to_owned()));
    assert!(!found.contains(&"ALD-CH-1003".to_owned()));
}

#[test]
fn stock_price_and_brand_filters_apply() {
    let catalog = shelf();
    let all = SearchQuery {
        limit: 10,
        ..Default::default()
    };
    let in_stock = SearchQuery {
        in_stock_only: true,
        ..all.clone()
    };
    assert!(!ids(&catalog.search(&in_stock)).contains(&"ALD-TR-4001".to_owned()));
    let cheap = SearchQuery {
        max_price_usd: Some(25.0),
        ..all.clone()
    };
    assert_eq!(
        ids(&catalog.search(&cheap)),
        ["ALD-LU-5001", "ALD-CH-1001", "ALD-CH-1003"]
    );
    let brand = SearchQuery {
        brand: Some("brackenridge".to_owned()),
        ..all
    };
    assert_eq!(ids(&catalog.search(&brand)), ["ALD-RT-3001"]);
}

#[test]
fn without_text_shelf_stock_comes_first_then_price() {
    let catalog = shelf();
    let results = catalog.search(&SearchQuery {
        limit: 10,
        ..Default::default()
    });
    let found = ids(&results);
    assert_eq!(
        found.last().map(String::as_str),
        Some("ALD-TR-4001"),
        "backorder is last"
    );
    assert_eq!(found[0], "ALD-LU-5001", "cheapest on the shelf is first");
}

#[test]
fn total_counts_matches_before_the_limit() {
    let catalog = shelf();
    let results = catalog.search(&SearchQuery {
        text: "chain".to_owned(),
        limit: 1,
        ..Default::default()
    });
    assert_eq!(results.hits.len(), 1);
    assert!(results.total >= 4);
}

#[test]
fn get_ignores_case_and_punctuation() {
    let catalog = shelf();
    assert!(catalog.get("ald-ch-1002").is_some());
    assert!(catalog.get("ALD CH 1002").is_some());
    assert!(catalog.get("ALD-CH-9999").is_none());
}

#[test]
fn every_generated_part_is_found_first_by_its_own_number() {
    let catalog = Catalog::new(generate(DEMO_SEED, DEMO_SIZE).unwrap()).unwrap();
    for part in catalog.parts() {
        let query = SearchQuery {
            text: part.id.clone(),
            limit: 1,
            ..Default::default()
        };
        let results = catalog.search(&query);
        assert_eq!(results.hits[0].part.id, part.id);
    }
}

#[test]
fn every_generated_part_is_in_the_top_five_for_its_own_title() {
    let catalog = Catalog::new(generate(DEMO_SEED, DEMO_SIZE).unwrap()).unwrap();
    for part in catalog.parts() {
        let query = SearchQuery {
            text: part.title.clone(),
            limit: 5,
            ..Default::default()
        };
        let found = ids(&catalog.search(&query));
        assert!(found.contains(&part.id), "{} not in {found:?}", part.title);
    }
}

#[test]
fn part_numbers_that_compact_to_the_same_key_are_rejected() {
    let a = part("ALD-CH-1001", "Chain", Category::Chain, 20.0, &[]);
    let b = part("ald ch 1001", "Other chain", Category::Chain, 21.0, &[]);

    let Err(CatalogError::DuplicateId(error)) = Catalog::new(vec![a, b]) else {
        panic!("a duplicate key must be refused");
    };

    assert_eq!(
        (error.first.as_str(), error.second.as_str()),
        ("ALD-CH-1001", "ald ch 1001")
    );
}

#[test]
fn the_generated_catalog_has_unique_part_numbers() {
    assert!(Catalog::new(generate(DEMO_SEED, DEMO_SIZE).unwrap()).is_ok());
}

#[test]
fn matching_one_of_two_query_words_keeps_eighty_percent_of_the_score() {
    let zebra = part("ZZZ-1", "Zebra", Category::Chain, 5.0, &[]);
    let lion = part("ZZZ-2", "Zebra Lion", Category::Chain, 6.0, &[]);
    let catalog = Catalog::new(vec![zebra, lion]).unwrap();
    let score_of = |text: &str, id: &str| {
        let results = catalog.search(&SearchQuery {
            text: text.to_owned(),
            limit: 10,
            ..Default::default()
        });
        results.hits.iter().find(|h| h.part.id == id).unwrap().score
    };

    let alone = score_of("zebra", "ZZZ-1");
    let with_missing_word = score_of("zebra lion", "ZZZ-1");

    assert!(
        (with_missing_word - 0.8 * alone).abs() < 1e-9,
        "{with_missing_word} vs {alone}"
    );
}

#[test]
fn a_price_that_is_not_a_sellable_whole_number_of_cents_is_refused() {
    for bad in [10.005, 0.0, -5.0, f64::NAN, f64::INFINITY, 10_000.01, 4.0e9] {
        let p = part("ALD-CH-1001", "Chain", Category::Chain, bad, &[]);
        let Err(CatalogError::Price(error)) = Catalog::new(vec![p]) else {
            panic!("price {bad} must be refused");
        };
        assert_eq!(error.id, "ALD-CH-1001");
    }
    for good in [0.01, 19.99, 10_000.0] {
        let p = part("ALD-CH-1001", "Chain", Category::Chain, good, &[]);
        let catalog = Catalog::new(vec![p]).unwrap();
        assert_eq!(
            f64::from(catalog.parts()[0].price_cents()),
            (good * 100.0).round()
        );
    }
}
