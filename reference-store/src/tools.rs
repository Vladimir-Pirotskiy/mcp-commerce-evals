//! The MCP server: four tools over the demo catalog.

use std::sync::Arc;

use catalog::Catalog;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::{ServerHandler, tool, tool_handler, tool_router};

use crate::api::{
    AvailabilityOutput, CartOutput, GetProductArgs, LinesArgs, PartSummary, ProductDetail,
    SearchArgs, SearchOutput,
};
use crate::cart;
use crate::error::ToolError;
use crate::input;

/// The store's MCP handler. Cheap to clone: the catalog is shared.
#[derive(Clone)]
pub struct ReferenceStore {
    catalog: Arc<Catalog>,
    checkout_base: Arc<str>,
    tool_router: ToolRouter<Self>,
}

impl ReferenceStore {
    /// A handler over `catalog`; carts point at `checkout_base`.
    pub fn new(catalog: Arc<Catalog>, checkout_base: Arc<str>) -> Self {
        Self {
            catalog,
            checkout_base,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl ReferenceStore {
    #[tool(
        description = "Find bicycle parts. Use this first, whenever the buyer names a part, a \
                       part number, a need or a bike. Takes free text (typos, synonyms and \
                       part numbers work) plus optional filters: brand, category, fits (such \
                       as 11-speed, road, disc-brake, 29in), in_stock_only and max_price_usd. \
                       Returns up to `limit` ranked results with id, title, brand, price, \
                       stock status and what each part fits. Search results show stock status \
                       but not quantities; call check_availability before promising a \
                       quantity. Queries \
                       are limited to 200 characters and 20 words; `limit` is 1 to 25, default 5.",
        annotations(
            title = "Search products",
            read_only_hint = true,
            open_world_hint = false
        )
    )]
    fn search_products(
        &self,
        Parameters(args): Parameters<SearchArgs>,
    ) -> Result<Json<SearchOutput>, ToolError> {
        let query = input::search_query(args)?;
        let found = self.catalog.search(&query);
        let suggestion = found.hits.is_empty().then(|| {
            "No part matched. Try fewer or simpler words, remove a filter, or search by category \
             with an empty query."
                .to_owned()
        });
        Ok(Json(SearchOutput {
            total_matches: found.total,
            results: found
                .hits
                .iter()
                .map(|h| PartSummary::from(h.part))
                .collect(),
            suggestion,
        }))
    }

    #[tool(
        description = "Get everything the store knows about one part: description, technical \
                       attributes (speed, wheel size, brake type, axle standard and so on), \
                       compatibility list, price, stock status, units on the shelf and restock \
                       days. Use it to compare two or three candidates from search_products or \
                       to confirm that a part fits the buyer's bike before recommending it.",
        annotations(
            title = "Get product detail",
            read_only_hint = true,
            open_world_hint = false
        )
    )]
    fn get_product(
        &self,
        Parameters(args): Parameters<GetProductArgs>,
    ) -> Result<Json<ProductDetail>, ToolError> {
        input::bounded("id", &args.id)?;
        match self.catalog.get(&args.id) {
            Some(part) => Ok(Json(ProductDetail { part: part.clone() })),
            None => Err(ToolError::UnknownPart { id: args.id }),
        }
    }

    #[tool(
        description = "Check whether specific quantities can ship now. Send one to twenty \
                       {id, quantity} items. For each item the answer is available (all on the \
                       shelf), partial (fewer on the shelf than requested), backorder (ships \
                       after restock_days), unavailable (out of stock, no date) or unknown_part. \
                       Call this before create_cart and tell the buyer about any item that is \
                       not available. Changes nothing.",
        annotations(
            title = "Check availability",
            read_only_hint = true,
            open_world_hint = false
        )
    )]
    fn check_availability(
        &self,
        Parameters(args): Parameters<LinesArgs>,
    ) -> Result<Json<AvailabilityOutput>, ToolError> {
        cart::validate(&args.items)?;
        Ok(Json(AvailabilityOutput {
            items: cart::check(&self.catalog, &args.items),
        }))
    }

    #[tool(
        description = "Create a cart and get a checkout link for the buyer. Use it only after \
                       the buyer has chosen the parts and quantities. Takes one to twenty \
                       {id, quantity} items and returns checkout_url, lines and the subtotal. \
                       The buyer opens checkout_url, reviews the cart and pays there: you never \
                       pay and cannot. Refuses unknown parts, out-of-stock parts and quantities \
                       above the shelf (a part sent on several lines counts once, with the \
                       lines added up); backordered parts are accepted and flagged.",
        annotations(
            title = "Create cart",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn create_cart(
        &self,
        Parameters(args): Parameters<LinesArgs>,
    ) -> Result<Json<CartOutput>, ToolError> {
        cart::build_cart(&self.catalog, &args.items, &self.checkout_base).map(Json)
    }
}

#[tool_handler(
    router = self.tool_router,
    name = "reference-store",
    instructions = "Demo bicycle-parts store with invented brands and a synthetic catalog. \
        Typical flow: search_products to find candidates, get_product for detail when choosing \
        between them, check_availability for the quantity the buyer wants, then create_cart and \
        hand the checkout_url to the buyer. The buyer pays on that page; no tool takes payment. \
        Do not guess part numbers: copy them from search results."
)]
impl ServerHandler for ReferenceStore {}
