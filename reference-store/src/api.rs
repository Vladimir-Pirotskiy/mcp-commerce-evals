//! Input and output types of the MCP tools. The doc comments become the JSON Schema
//! descriptions that an assistant reads when it decides how to fill in a call.
//!
//! Every type drops the formats schemars invents for Rust numbers (`uint32`, `double`): JSON
//! Schema does not define them and a client that validates the schema warns on each one. Unsigned
//! fields keep `minimum: 0`.

use catalog::{Category, Part, StockStatus};
use schemars::JsonSchema;
use schemars::transform::RestrictFormats;
use serde::{Deserialize, Serialize};

/// Arguments of `search_products`.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(transform = RestrictFormats::default())]
pub struct SearchArgs {
    /// What the buyer is looking for, in their own words: a part number such as
    /// `ALD-CH-1002`, a product type ("11-speed chain"), a need ("brake pads for disc brakes")
    /// or a bike ("tire for 29er mountain bike"). Typos and synonyms are tolerated. Leave empty
    /// to list parts by the filters alone.
    pub query: Option<String>,
    /// Only parts from this brand, for example `Aldermoor`.
    pub brand: Option<String>,
    /// Only parts in this category.
    pub category: Option<Category>,
    /// Requirements the part must fit, such as `11-speed`, `road`, `disc-brake`, `29in`,
    /// `12x142`. A part that says nothing about a requirement is treated as universal for it.
    pub fits: Option<Vec<String>>,
    /// When true, hide parts that cannot ship from the shelf now (backorder and out of stock).
    pub in_stock_only: Option<bool>,
    /// Only parts priced at or below this many US dollars.
    pub max_price_usd: Option<f64>,
    /// How many results to return, 1 to 25. Defaults to 5.
    pub limit: Option<u32>,
}

/// One search result: enough to choose between parts without opening each one.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(transform = RestrictFormats::default())]
pub struct PartSummary {
    /// Part number; pass it to `get_product`, `check_availability` or `create_cart`.
    pub id: String,
    /// Product title.
    pub title: String,
    /// Brand name.
    pub brand: String,
    /// Product category.
    pub category: Category,
    /// Price in US dollars.
    pub price_usd: f64,
    /// Stock status at search time.
    pub stock_status: StockStatus,
    /// What the part fits; empty means universal.
    pub fits: Vec<String>,
}

impl From<&Part> for PartSummary {
    fn from(part: &Part) -> Self {
        Self {
            id: part.id.clone(),
            title: part.title.clone(),
            brand: part.brand.clone(),
            category: part.category,
            price_usd: part.price_usd,
            stock_status: part.stock,
            fits: part.fits.clone(),
        }
    }
}

/// Result of `search_products`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(transform = RestrictFormats::default())]
pub struct SearchOutput {
    /// How many parts matched in total, before the limit.
    pub total_matches: usize,
    /// The best matches, best first.
    pub results: Vec<PartSummary>,
    /// Advice for the next call when nothing matched; absent otherwise.
    pub suggestion: Option<String>,
}

/// Arguments of `get_product`.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(transform = RestrictFormats::default())]
pub struct GetProductArgs {
    /// Part number as returned by `search_products`. Case and dashes do not matter.
    pub id: String,
}

/// Full detail of one part.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(transform = RestrictFormats::default())]
pub struct ProductDetail {
    /// Everything the store knows about the part.
    #[serde(flatten)]
    pub part: Part,
}

/// A part number and how many the buyer wants.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(transform = RestrictFormats::default())]
pub struct LineRequest {
    /// Part number as returned by `search_products`.
    pub id: String,
    /// How many units, 1 to 99. Defaults to 1.
    pub quantity: Option<u32>,
}

/// Arguments of `check_availability` and `create_cart`.
#[derive(Debug, Deserialize, JsonSchema)]
#[schemars(transform = RestrictFormats::default())]
pub struct LinesArgs {
    /// One to twenty parts with quantities.
    pub items: Vec<LineRequest>,
}

/// Whether a requested quantity can be had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[schemars(transform = RestrictFormats::default())]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    /// The whole quantity ships from the shelf now.
    Available,
    /// Only part of the quantity is on the shelf; see `quantity_on_shelf`.
    Partial,
    /// None on the shelf; ships when the supplier delivers, see `restock_days`.
    Backorder,
    /// None on the shelf and no restock date.
    Unavailable,
    /// No part has this number.
    UnknownPart,
}

/// Availability of one requested line.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(transform = RestrictFormats::default())]
pub struct LineAvailability {
    /// The part number as the buyer gave it.
    pub id: String,
    /// Units requested.
    pub requested: u32,
    /// Whether the requested quantity can be had.
    pub availability: Availability,
    /// Units on the shelf now (zero when unknown, backordered or out of stock).
    pub quantity_on_shelf: u32,
    /// Days until a backordered part ships.
    pub restock_days: Option<u32>,
    /// Unit price in US dollars; absent for an unknown part.
    pub unit_price_usd: Option<f64>,
    /// One sentence a buyer can be told.
    pub message: String,
}

/// Result of `check_availability`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(transform = RestrictFormats::default())]
pub struct AvailabilityOutput {
    /// One entry per requested line, in request order.
    pub items: Vec<LineAvailability>,
}

/// One line of a created cart.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(transform = RestrictFormats::default())]
pub struct CartLine {
    /// Part number.
    pub id: String,
    /// Product title.
    pub title: String,
    /// Units in the cart.
    pub quantity: u32,
    /// Unit price in US dollars.
    pub unit_price_usd: f64,
    /// Unit price times quantity.
    pub line_total_usd: f64,
    /// `available` or `backorder`.
    pub availability: Availability,
    /// Days until a backordered line ships.
    pub restock_days: Option<u32>,
}

/// Result of `create_cart`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(transform = RestrictFormats::default())]
pub struct CartOutput {
    /// Identifier of the cart; the same items always give the same identifier.
    pub cart_id: String,
    /// Where the buyer opens the cart and pays. Give this link to the buyer; never try to pay.
    pub checkout_url: String,
    /// Currency of every amount.
    pub currency: String,
    /// The cart lines.
    pub lines: Vec<CartLine>,
    /// Sum of the line totals; shipping and tax are added at checkout.
    pub subtotal_usd: f64,
    /// What to tell the buyer.
    pub note: String,
}
