//! Domain types of the demo catalog.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What kind of part a product is.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    /// Drivetrain chain.
    Chain,
    /// Rear cassette (cogset).
    Cassette,
    /// Disc or rim brake pads.
    BrakePads,
    /// Disc brake rotor.
    Rotor,
    /// Tire.
    Tire,
    /// Inner tube.
    Tube,
    /// Shift or brake cable.
    Cable,
    /// Saddle.
    Saddle,
    /// Pedals.
    Pedals,
    /// Handlebar tape for drop bars.
    HandlebarTape,
    /// Handlebar grips for flat bars.
    Grips,
    /// Chainring.
    Chainring,
    /// Rear derailleur.
    RearDerailleur,
    /// Bottle cage.
    BottleCage,
    /// Front or rear light.
    Light,
    /// Chain lubricant.
    ChainLube,
    /// Thru-axle or quick-release skewer.
    Axle,
    /// Rear hub.
    Hub,
}

/// The dearest price a part may have, in cents ($10,000). With 20 cart lines of 99 units each
/// the largest subtotal is under 2 billion cents, inside `u32`.
pub const MAX_PRICE_CENTS: u32 = 1_000_000;

impl Part {
    /// The price in whole cents, the exact form of [`Part::price_usd`]; use it for arithmetic.
    /// [`Catalog::new`](crate::Catalog::new) refuses a part whose price is not a whole number
    /// of cents in `1..=MAX_PRICE_CENTS`, so for a catalog part this is exact; out of range it
    /// saturates.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the value is clamped into 0..=MAX_PRICE_CENTS first"
    )]
    pub fn price_cents(&self) -> u32 {
        (self.price_usd * 100.0)
            .round()
            .clamp(0.0, f64::from(MAX_PRICE_CENTS)) as u32
    }

    /// True when the price is a whole number of cents from one to [`MAX_PRICE_CENTS`].
    pub(crate) fn has_valid_price(&self) -> bool {
        let cents = self.price_usd * 100.0;
        cents.is_finite()
            && (1.0..=f64::from(MAX_PRICE_CENTS)).contains(&cents.round())
            && (cents - cents.round()).abs() < 1e-6
    }
}

impl Category {
    /// Every category, in catalog order.
    pub const ALL: [Category; 18] = [
        Category::Chain,
        Category::Cassette,
        Category::BrakePads,
        Category::Rotor,
        Category::Tire,
        Category::Tube,
        Category::Cable,
        Category::Saddle,
        Category::Pedals,
        Category::HandlebarTape,
        Category::Grips,
        Category::Chainring,
        Category::RearDerailleur,
        Category::BottleCage,
        Category::Light,
        Category::ChainLube,
        Category::Axle,
        Category::Hub,
    ];

    /// Two-letter code used inside part numbers.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Category::Chain => "CH",
            Category::Cassette => "CS",
            Category::BrakePads => "BP",
            Category::Rotor => "RT",
            Category::Tire => "TR",
            Category::Tube => "TB",
            Category::Cable => "CB",
            Category::Saddle => "SD",
            Category::Pedals => "PD",
            Category::HandlebarTape => "HT",
            Category::Grips => "GR",
            Category::Chainring => "CR",
            Category::RearDerailleur => "RD",
            Category::BottleCage => "BC",
            Category::Light => "LT",
            Category::ChainLube => "LU",
            Category::Axle => "AX",
            Category::Hub => "HB",
        }
    }

    /// Plain-language name of the category, as a buyer would write it.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Category::Chain => "chain",
            Category::Cassette => "cassette",
            Category::BrakePads => "brake pads",
            Category::Rotor => "rotor",
            Category::Tire => "tire",
            Category::Tube => "inner tube",
            Category::Cable => "cable",
            Category::Saddle => "saddle",
            Category::Pedals => "pedals",
            Category::HandlebarTape => "handlebar tape",
            Category::Grips => "grips",
            Category::Chainring => "chainring",
            Category::RearDerailleur => "rear derailleur",
            Category::BottleCage => "bottle cage",
            Category::Light => "light",
            Category::ChainLube => "chain lube",
            Category::Axle => "axle",
            Category::Hub => "hub",
        }
    }
}

/// How soon a part can be bought.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum StockStatus {
    /// Plenty on the shelf; ships in one business day.
    InStock,
    /// Four or fewer units left.
    LowStock,
    /// None on the shelf; ships when the supplier delivers (see `restock_days`).
    Backorder,
    /// None on the shelf and no restock date.
    OutOfStock,
}

impl StockStatus {
    /// True when the part can ship from the shelf now (in stock or low stock).
    #[must_use]
    pub fn is_on_shelf(self) -> bool {
        matches!(self, StockStatus::InStock | StockStatus::LowStock)
    }
}

/// One product in the demo catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Part {
    /// Part number, for example `VEL-CH-1042`. Unique and stable for a given seed.
    pub id: String,
    /// Product title.
    pub title: String,
    /// Plain-text description.
    pub description: String,
    /// Fictional brand name.
    pub brand: String,
    /// Product category.
    pub category: Category,
    /// Price in US dollars.
    pub price_usd: f64,
    /// Stock status.
    pub stock: StockStatus,
    /// Units on the shelf (zero unless `stock` is in stock or low stock).
    pub quantity_available: u32,
    /// Days until a backordered part ships; absent for other statuses.
    pub restock_days: Option<u32>,
    /// Technical attributes such as `speed`, `wheel_size` or `brake_type`.
    pub attributes: BTreeMap<String, String>,
    /// Compatibility tags: bike types and standards the part fits. Empty means universal.
    pub fits: Vec<String>,
}
