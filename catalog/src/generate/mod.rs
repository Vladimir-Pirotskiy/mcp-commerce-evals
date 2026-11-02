//! Deterministic generator of the demo catalog.

mod braking;
mod contact;
mod draft;
mod drivetrain;
mod wheels;

use std::collections::HashSet;

use crate::model::{Category, Part, StockStatus};
use crate::rng::Rng;
use draft::Draft;

/// Fictional brands: (name, part-number prefix).
pub const BRANDS: [(&str, &str); 14] = [
    ("Aldermoor", "ALD"),
    ("Brackenridge", "BRK"),
    ("Corvane", "COR"),
    ("Dunmore", "DUN"),
    ("Ferrowick", "FER"),
    ("Glenhollow", "GLE"),
    ("Ironvale", "IRO"),
    ("Nettlefold", "NET"),
    ("Oakhaven", "OAK"),
    ("Pellmoor", "PEL"),
    ("Quillon", "QUI"),
    ("Rookwood", "ROO"),
    ("Stonebrook", "STO"),
    ("Tamsin", "TAM"),
];

const LINES: [&str; 8] = [
    "Sport", "Trail", "Tour", "Race", "Comp", "Pro", "Elite", "Ride",
];

/// The catalog the generator produces for a seed and a size.
pub fn generate(seed: u64, count: usize) -> Vec<Part> {
    let mut rng = Rng::new(seed);
    let mut seen_titles = HashSet::new();
    let mut seen_ids = HashSet::new();
    let mut parts = Vec::with_capacity(count);
    let mut attempts = 0;
    while parts.len() < count && attempts < count * 50 {
        attempts += 1;
        let category = Category::ALL[parts.len() % Category::ALL.len()];
        let brand_index = (category as usize + rng.below(5)) % BRANDS.len();
        let (brand, prefix) = BRANDS[brand_index];
        let draft = draft_for(category, &mut rng);
        let line = *rng.pick(&LINES);
        let title = format!("{brand} {line} {}", draft.name);
        if !seen_titles.insert(title.clone()) {
            continue;
        }
        let id = loop {
            let id = format!("{prefix}-{}-{}", category.code(), rng.range(1000, 9999));
            if seen_ids.insert(id.clone()) {
                break id;
            }
        };
        parts.push(finish(&mut rng, id, title, brand, category, draft));
    }
    parts
}

fn draft_for(category: Category, rng: &mut Rng) -> Draft {
    match category {
        Category::Chain => drivetrain::chain(rng),
        Category::Cassette => drivetrain::cassette(rng),
        Category::Chainring => drivetrain::chainring(rng),
        Category::RearDerailleur => drivetrain::rear_derailleur(rng),
        Category::ChainLube => drivetrain::chain_lube(rng),
        Category::BrakePads => braking::brake_pads(rng),
        Category::Rotor => braking::rotor(rng),
        Category::Cable => braking::cable(rng),
        Category::Tire => wheels::tire(rng),
        Category::Tube => wheels::tube(rng),
        Category::Axle => wheels::axle(rng),
        Category::Hub => wheels::hub(rng),
        Category::Saddle => contact::saddle(rng),
        Category::Pedals => contact::pedals(rng),
        Category::HandlebarTape => contact::handlebar_tape(rng),
        Category::Grips => contact::grips(rng),
        Category::BottleCage => contact::bottle_cage(rng),
        Category::Light => contact::light(rng),
    }
}

fn finish(
    rng: &mut Rng,
    id: String,
    title: String,
    brand: &str,
    category: Category,
    draft: Draft,
) -> Part {
    let cents = *rng.pick(&[0, 49, 95, 99]);
    let (stock, quantity_available, restock_days) = match rng.below(100) {
        0..=61 => (StockStatus::InStock, rng.range(5, 60), None),
        62..=75 => (StockStatus::LowStock, rng.range(1, 4), None),
        76..=89 => (StockStatus::Backorder, 0, Some(rng.range(7, 35))),
        _ => (StockStatus::OutOfStock, 0, None),
    };
    Part {
        id,
        title,
        description: draft.sentences.join(" "),
        brand: brand.to_owned(),
        category,
        price_usd: f64::from(draft.price_dollars) + f64::from(cents) / 100.0,
        stock,
        quantity_available,
        restock_days,
        attributes: draft.attributes,
        fits: draft.fits,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_catalog() {
        assert_eq!(generate(42, 120), generate(42, 120));
        assert_ne!(generate(42, 120), generate(43, 120));
    }

    #[test]
    fn ids_and_titles_are_unique() {
        let parts = generate(2026, 500);
        assert_eq!(parts.len(), 500);
        let ids: HashSet<_> = parts.iter().map(|p| &p.id).collect();
        let titles: HashSet<_> = parts.iter().map(|p| &p.title).collect();
        assert_eq!(ids.len(), 500);
        assert_eq!(titles.len(), 500);
    }

    #[test]
    fn every_category_is_present_with_sane_fields() {
        let parts = generate(2026, 500);
        for category in Category::ALL {
            assert!(parts.iter().any(|p| p.category == category), "{category:?}");
        }
        for p in &parts {
            assert!(p.price_usd > 0.0 && !p.description.is_empty(), "{}", p.id);
            assert!(p.id.contains(p.category.code()), "{}", p.id);
            assert_eq!(p.stock.is_on_shelf(), p.quantity_available > 0, "{}", p.id);
            assert_eq!(
                p.stock == StockStatus::Backorder,
                p.restock_days.is_some(),
                "{}",
                p.id
            );
        }
    }

    #[test]
    fn all_stock_statuses_occur() {
        let parts = generate(2026, 500);
        for status in [
            StockStatus::InStock,
            StockStatus::LowStock,
            StockStatus::Backorder,
            StockStatus::OutOfStock,
        ] {
            assert!(parts.iter().any(|p| p.stock == status), "{status:?}");
        }
    }
}
