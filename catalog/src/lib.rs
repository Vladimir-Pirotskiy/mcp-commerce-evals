//! A seeded demo catalog of fictional bicycle parts, with search.
//!
//! Every brand, part and number in this crate is invented for demonstration. [`generate`]
//! builds the same catalog for the same seed on every machine.

mod generate;
mod model;
mod rng;

pub use generate::{BRANDS, generate};
pub use model::{Category, Part, StockStatus};

/// Seed of the catalog that ships with the repository and that `evals/queries.json` refers to.
pub const DEMO_SEED: u64 = 2026;

/// Size of the shipped demo catalog.
pub const DEMO_SIZE: usize = 500;
