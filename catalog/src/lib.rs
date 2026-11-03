//! A seeded demo catalog of fictional bicycle parts, with search.
//!
//! Every brand, part and number in this crate is invented for demonstration. [`generate`]
//! builds the same catalog for the same seed on every machine; [`Catalog`] indexes it for
//! BM25 search with typo tolerance, synonyms and filters.

mod compat;
mod generate;
mod model;
mod rng;
mod search;
mod text;

pub use compat::{AXLES, BIKES, BRAKES, SPEEDS, WHEELS, all_tags};
pub use generate::{BRANDS, GenerateError, MAX_PARTS, generate};
pub use model::{Category, MAX_PRICE_CENTS, Part, StockStatus};
pub use search::{
    Catalog, CatalogError, DuplicatePartId, Hit, InvalidPrice, SearchQuery, SearchResults,
};
pub use text::{canonical_tag, query_tokens};

/// Seed of the catalog that ships with the repository and that `evals/queries.json` refers to.
pub const DEMO_SEED: u64 = 2026;

/// Size of the shipped demo catalog.
pub const DEMO_SIZE: usize = 500;

/// The demo catalog the store serves and the queries file was written against.
///
/// # Panics
///
/// Never in practice: tests pin that the demo size is reachable and its part numbers unique.
#[must_use]
pub fn demo_catalog() -> Catalog {
    Catalog::new(
        generate(DEMO_SEED, DEMO_SIZE).expect("the demo size is within the generator's reach"),
    )
    .expect("the generator emits unique part numbers")
}
