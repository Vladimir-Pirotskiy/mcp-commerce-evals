//! Availability checks and cart building, independent of the MCP layer.

use std::collections::BTreeMap;

use catalog::{Catalog, Part, StockStatus};

use crate::api::{Availability, CartLine, CartOutput, LineAvailability, LineRequest};
use crate::error::{ToolError, echo};
use crate::input;

/// Most lines one call may carry.
pub const MAX_LINES: usize = 20;
/// Most units of one part in one line.
pub const MAX_QUANTITY: u32 = 99;

/// What a buyer is told about a part number the catalog does not have.
const UNKNOWN_PART_MESSAGE: &str = "No part has this number. Use search_products to find it.";

fn quantity_of(line: &LineRequest) -> u32 {
    line.quantity.unwrap_or(1).clamp(1, MAX_QUANTITY)
}

/// Rejects empty or oversized requests and out-of-range quantities.
pub fn validate(lines: &[LineRequest]) -> Result<(), ToolError> {
    if lines.is_empty() || lines.len() > MAX_LINES {
        return Err(ToolError::ItemCount { max: MAX_LINES });
    }
    for line in lines {
        input::bounded("id", &line.id)?;
    }
    match lines
        .iter()
        .find(|l| l.quantity.is_some_and(|q| !(1..=MAX_QUANTITY).contains(&q)))
    {
        Some(line) => Err(ToolError::Quantity {
            id: line.id.clone(),
            max: MAX_QUANTITY,
        }),
        None => Ok(()),
    }
}

fn describe(part: &Part, requested: u32) -> (Availability, String) {
    match part.stock {
        StockStatus::InStock | StockStatus::LowStock if part.quantity_available >= requested => (
            Availability::Available,
            format!("{requested} in stock, ships in one business day."),
        ),
        StockStatus::InStock | StockStatus::LowStock => (
            Availability::Partial,
            format!(
                "Only {} on the shelf; {requested} were requested.",
                part.quantity_available
            ),
        ),
        StockStatus::Backorder => (
            Availability::Backorder,
            format!(
                "On backorder; ships about {} days after the order.",
                part.restock_days.unwrap_or(0)
            ),
        ),
        StockStatus::OutOfStock => (
            Availability::Unavailable,
            "Out of stock with no restock date.".to_owned(),
        ),
    }
}

/// Availability of every requested part. Repeated parts are added up first, exactly as
/// [`build_cart`] does, so the two tools never disagree: the entry for a part that was sent on
/// several lines carries the summed `requested`. Entries follow the first appearance of each
/// part; every unknown number gets an entry of its own.
pub fn check(catalog: &Catalog, lines: &[LineRequest]) -> Vec<LineAvailability> {
    merge_lines(catalog, lines)
        .into_iter()
        .map(|merged| match merged {
            Merged::Known(part, requested) => {
                let (availability, message) = describe(part, requested);
                LineAvailability {
                    id: part.id.clone(),
                    requested,
                    availability,
                    quantity_on_shelf: part.quantity_available,
                    restock_days: part.restock_days,
                    unit_price_usd: Some(part.price_usd),
                    unit_price_cents: Some(part.price_cents()),
                    message,
                }
            }
            Merged::Unknown(line) => LineAvailability {
                id: line.id.clone(),
                requested: quantity_of(line),
                availability: Availability::UnknownPart,
                quantity_on_shelf: 0,
                restock_days: None,
                unit_price_usd: None,
                unit_price_cents: None,
                message: UNKNOWN_PART_MESSAGE.to_owned(),
            },
        })
        .collect()
}

/// Dollars for a whole number of cents. The catalog caps a price at `MAX_PRICE_CENTS`, so a cart
/// (20 lines of at most 99 units) stays under 2 billion cents; the arithmetic still saturates
/// rather than wraps, should a larger value ever arrive.
fn dollars(cents: u32) -> f64 {
    f64::from(cents) / 100.0
}

/// FNV-1a, so that the same cart contents give the same identifier on every machine.
fn fingerprint(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// A requested part after repeated lines were added up.
enum Merged<'a> {
    /// A part in the catalog and the total quantity requested over all its lines.
    Known(&'a Part, u32),
    /// A line naming no part.
    Unknown(&'a LineRequest),
}

/// Sums the requested quantity per part, so a part sent on several lines is judged once. Known
/// parts keep the position of their first line; each unknown line stays on its own. Shared by
/// `check_availability` and `create_cart` so that both judge the same quantities.
fn merge_lines<'a>(catalog: &'a Catalog, lines: &'a [LineRequest]) -> Vec<Merged<'a>> {
    let mut merged: Vec<Merged<'a>> = Vec::new();
    for line in lines {
        let Some(part) = catalog.get(&line.id) else {
            merged.push(Merged::Unknown(line));
            continue;
        };
        let existing = merged.iter_mut().find_map(|entry| match entry {
            Merged::Known(known, total) if known.id == part.id => Some(total),
            Merged::Known(..) | Merged::Unknown(_) => None,
        });
        match existing {
            Some(total) => *total += quantity_of(line),
            None => merged.push(Merged::Known(part, quantity_of(line))),
        }
    }
    merged
}

/// Builds a cart, merging repeated parts first and judging the merged quantity. Refuses unknown
/// parts, parts that are out of stock, merged quantities beyond the shelf or beyond
/// [`MAX_QUANTITY`]; backordered parts are allowed and flagged.
pub fn build_cart(
    catalog: &Catalog,
    lines: &[LineRequest],
    checkout_base: &str,
) -> Result<CartOutput, ToolError> {
    validate(lines)?;
    let mut problems = Vec::new();
    // Sorted by part number, so the same items give the same cart id in any order.
    let mut merged: BTreeMap<&str, (&Part, u32)> = BTreeMap::new();
    for entry in merge_lines(catalog, lines) {
        match entry {
            Merged::Known(part, total) => {
                merged.insert(part.id.as_str(), (part, total));
            }
            Merged::Unknown(line) => {
                problems.push(format!("{}: {UNKNOWN_PART_MESSAGE}", echo(&line.id)));
            }
        }
    }
    for (part, total) in merged.values() {
        if *total > MAX_QUANTITY {
            return Err(ToolError::MergedQuantity {
                id: part.id.clone(),
                total: *total,
                max: MAX_QUANTITY,
            });
        }
        let (availability, message) = describe(part, *total);
        if !matches!(
            availability,
            Availability::Available | Availability::Backorder
        ) {
            problems.push(format!("{}: {message}", part.id));
        }
    }
    if !problems.is_empty() {
        return Err(ToolError::CartRefused { problems });
    }
    let cart_lines: Vec<CartLine> = merged
        .values()
        .map(|(part, quantity)| {
            let (availability, _) = describe(part, *quantity);
            CartLine {
                id: part.id.clone(),
                title: part.title.clone(),
                quantity: *quantity,
                unit_price_usd: part.price_usd,
                unit_price_cents: part.price_cents(),
                line_total_usd: dollars(part.price_cents().saturating_mul(*quantity)),
                line_total_cents: part.price_cents().saturating_mul(*quantity),
                availability,
                restock_days: part.restock_days,
            }
        })
        .collect();
    let subtotal = cart_lines
        .iter()
        .fold(0_u32, |sum, l| sum.saturating_add(l.line_total_cents));
    let items = cart_lines
        .iter()
        .map(|l| format!("{}:{}", l.id, l.quantity))
        .collect::<Vec<_>>();
    let cart_id = format!("cart_{:016x}", fingerprint(&items.join(";")));
    Ok(CartOutput {
        checkout_url: format!("{checkout_base}/{cart_id}?items={}", items.join(",")),
        cart_id,
        currency: "USD".to_owned(),
        lines: cart_lines,
        subtotal_usd: dollars(subtotal),
        subtotal_cents: subtotal,
        note: "Give the buyer checkout_url. They review the cart and pay there; this store never \
               takes payment through a tool. Shipping and tax are added at checkout. This is a \
               demo store: the link does not lead to a real checkout."
            .to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use catalog::{StockStatus, demo_catalog};

    use super::*;

    fn line(id: &str, quantity: Option<u32>) -> LineRequest {
        LineRequest {
            id: id.to_owned(),
            quantity,
        }
    }

    fn id_with(catalog: &Catalog, stock: StockStatus) -> String {
        catalog
            .parts()
            .iter()
            .find(|p| p.stock == stock)
            .map(|p| p.id.clone())
            .unwrap()
    }

    #[test]
    fn availability_follows_the_stock_status() {
        let catalog = demo_catalog();
        let in_stock = catalog
            .parts()
            .iter()
            .find(|p| p.stock == StockStatus::InStock && p.quantity_available >= 5)
            .unwrap();
        let got = check(
            &catalog,
            &[
                line(&in_stock.id, Some(2)),
                line(
                    &id_with(&catalog, StockStatus::LowStock),
                    Some(MAX_QUANTITY),
                ),
                line(&id_with(&catalog, StockStatus::Backorder), None),
                line(&id_with(&catalog, StockStatus::OutOfStock), None),
                line("NOPE-1", None),
            ],
        );
        let kinds: Vec<_> = got.iter().map(|l| l.availability).collect();
        assert_eq!(
            kinds,
            [
                Availability::Available,
                Availability::Partial,
                Availability::Backorder,
                Availability::Unavailable,
                Availability::UnknownPart
            ]
        );
        assert!(got[2].restock_days.is_some());
        assert!(got[4].unit_price_usd.is_none());
    }

    #[test]
    fn the_dearest_possible_cart_is_exact_and_fits() {
        let parts: Vec<Part> = (0..MAX_LINES)
            .map(|i| {
                let mut part = demo_catalog().parts()[i].clone();
                part.id = format!("TST-AA-{i:04}");
                part.price_usd = f64::from(catalog::MAX_PRICE_CENTS) / 100.0;
                part.stock = StockStatus::InStock;
                part.quantity_available = MAX_QUANTITY;
                part.restock_days = None;
                part
            })
            .collect();
        let catalog = Catalog::new(parts).unwrap();
        let lines: Vec<LineRequest> = catalog
            .parts()
            .iter()
            .map(|p| line(&p.id, Some(MAX_QUANTITY)))
            .collect();

        let cart = build_cart(&catalog, &lines, "https://x.example").unwrap();

        let expected = u64::from(catalog::MAX_PRICE_CENTS) * u64::from(MAX_QUANTITY) * 20;
        assert_eq!(u64::from(cart.subtotal_cents), expected);
    }

    #[test]
    fn money_in_cents_is_the_rounded_dollar_amount_everywhere() {
        let catalog = demo_catalog();
        for part in catalog.parts() {
            assert_eq!(
                f64::from(part.price_cents()),
                (part.price_usd * 100.0).round()
            );
        }
        let part = catalog
            .parts()
            .iter()
            .find(|p| p.stock == StockStatus::InStock && p.quantity_available >= 3)
            .unwrap();
        let cart = build_cart(&catalog, &[line(&part.id, Some(3))], "https://x.example").unwrap();
        let cart_line = &cart.lines[0];
        assert_eq!(
            f64::from(cart_line.unit_price_cents),
            (cart_line.unit_price_usd * 100.0).round()
        );
        assert_eq!(
            f64::from(cart_line.line_total_cents),
            (cart_line.line_total_usd * 100.0).round()
        );
        assert_eq!(
            f64::from(cart.subtotal_cents),
            (cart.subtotal_usd * 100.0).round()
        );
        let checked = &check(&catalog, &[line(&part.id, None)])[0];
        assert_eq!(checked.unit_price_cents, Some(part.price_cents()));
    }

    #[test]
    fn check_availability_adds_repeated_lines_up_like_create_cart() {
        let catalog = demo_catalog();
        let part = catalog
            .parts()
            .iter()
            .find(|p| p.stock == StockStatus::InStock && p.quantity_available >= 4)
            .unwrap();
        // Each line fits on the shelf alone; together they do not.
        let each = part.quantity_available - 1;
        let lines = [
            line(&part.id, Some(each)),
            line("NOPE-1", None),
            line(&part.id.to_lowercase(), Some(each)),
        ];

        let checked = check(&catalog, &lines);

        assert_eq!(checked.len(), 2, "one entry per part, unknown lines apart");
        assert_eq!(checked[0].id, part.id);
        assert_eq!(checked[0].requested, 2 * each);
        assert_eq!(checked[0].availability, Availability::Partial);
        assert_eq!(checked[1].availability, Availability::UnknownPart);
        assert!(build_cart(&catalog, &lines, "https://x.example").is_err());
    }

    #[test]
    fn a_cart_merges_repeated_parts_and_sums_in_cents() {
        let catalog = demo_catalog();
        let part = catalog
            .parts()
            .iter()
            .find(|p| p.stock == StockStatus::InStock && p.quantity_available >= 6)
            .unwrap();
        let cart = build_cart(
            &catalog,
            &[
                line(&part.id, Some(2)),
                line(&part.id.to_lowercase(), Some(3)),
            ],
            "https://shop.example/checkout",
        )
        .unwrap();
        assert_eq!(cart.lines.len(), 1);
        assert_eq!(cart.lines[0].quantity, 5);
        assert_eq!(cart.subtotal_usd, (part.price_usd * 500.0).round() / 100.0);
        assert_eq!(cart.subtotal_cents, part.price_cents() * 5);
        assert_eq!(cart.lines[0].line_total_cents, cart.subtotal_cents);
        assert!(
            cart.checkout_url
                .starts_with("https://shop.example/checkout/cart_")
        );
        assert!(
            cart.checkout_url
                .ends_with(&format!("?items={}:5", part.id))
        );
    }

    #[test]
    fn the_same_items_give_the_same_cart_id_in_any_order() {
        let catalog = demo_catalog();
        let a = id_with(&catalog, StockStatus::InStock);
        let b = id_with(&catalog, StockStatus::Backorder);
        let one = build_cart(
            &catalog,
            &[line(&a, None), line(&b, None)],
            "https://x.example",
        )
        .unwrap();
        let two = build_cart(
            &catalog,
            &[line(&b, None), line(&a, None)],
            "https://x.example",
        )
        .unwrap();
        assert_eq!(one.cart_id, two.cart_id);
        let backordered = one.lines.iter().find(|l| l.id == b).unwrap();
        assert_eq!(backordered.availability, Availability::Backorder);
    }

    #[test]
    fn a_cart_is_refused_for_unknown_unavailable_or_oversized_lines() {
        let catalog = demo_catalog();
        let base = "https://x.example";
        let out = id_with(&catalog, StockStatus::OutOfStock);
        for items in [
            vec![line("NOPE-1", None)],
            vec![line(&out, None)],
            vec![line(&out, Some(0))],
        ] {
            assert!(build_cart(&catalog, &items, base).is_err());
        }
        assert!(build_cart(&catalog, &[], base).is_err());
        let many: Vec<_> = (0..=MAX_LINES).map(|_| line(&out, None)).collect();
        assert!(validate(&many).is_err());
        assert!(validate(&[line(&out, Some(MAX_QUANTITY + 1))]).is_err());
    }
}
