//! Parts a rider touches: saddles, pedals, bar tape, grips; plus cages and lights.

use super::draft::{Draft, spread};
use crate::rng::Rng;

pub(super) fn saddle(rng: &mut Rng) -> Draft {
    let width = *rng.pick(&[130, 143, 155, 160, 175]);
    let rail = *rng.pick(&["steel", "chromoly", "carbon"]);
    let shape = *rng.pick(&["flat", "curved", "cut-out"]);
    let (bike, usage) = match width {
        130 | 143 => ("road", "fast road riding"),
        155 => ("gravel", "long gravel days"),
        160 => ("mountain", "technical trails"),
        _ => ("commuter", "daily commuting"),
    };
    let price = spread(rng, 25, if rail == "carbon" { 140 } else { 60 });
    Draft::new(format!("{width}mm {shape} Saddle {rail} rails"), price)
        .attr("width_mm", width)
        .attr("shape", shape)
        .attr("rails", rail)
        .attr("clamp", "7mm round rails")
        .fits(&[bike])
        .say(format!(
            "A {shape} saddle, {width}mm wide, on {rail} rails; built for {usage}."
        ))
        .say(if shape == "cut-out" {
            "A relief channel takes pressure off soft tissue on long rides."
        } else {
            "Firm padding stays comfortable past the three-hour mark."
        })
}

pub(super) fn pedals(rng: &mut Rng) -> Draft {
    let kind = *rng.pick(&[
        "flat platform",
        "clipless road",
        "clipless mountain",
        "hybrid",
    ]);
    let body = *rng.pick(&["alloy", "composite", "magnesium"]);
    let bike = if kind.contains("road") {
        "road"
    } else if kind.contains("mountain") || kind == "flat platform" {
        "mountain"
    } else {
        "commuter"
    };
    let price = spread(rng, 20, 110);
    Draft::new(format!("{kind} Pedals {body} body"), price)
        .attr("pedal_type", kind)
        .attr("body", body)
        .attr("thread", "9/16 inch")
        .attr(
            "cleats",
            if kind.starts_with("clipless") {
                "included"
            } else {
                "none"
            },
        )
        .fits(&[bike])
        .say(format!("{kind} pedals, {body} body, 9/16 inch thread."))
        .say(if kind.starts_with("clipless") {
            "Adjustable release tension; cleats are in the box."
        } else {
            "Replaceable pins give grip in any shoe."
        })
}

pub(super) fn handlebar_tape(rng: &mut Rng) -> Draft {
    let material = *rng.pick(&["cork", "gel", "microfiber", "leather"]);
    let colour = *rng.pick(&["black", "white", "red", "blue", "tan"]);
    let price = spread(rng, 12, 30);
    Draft::new(format!("{colour} {material} Handlebar Tape"), price)
        .attr("material", material)
        .attr("colour", colour)
        .attr("length_mm", 2000)
        .fits(&["road", "gravel"])
        .say(format!(
            "{colour} {material} wrap for drop handlebars, 2 m long."
        ))
        .say("Comes with end plugs and finishing tape.")
}

pub(super) fn grips(rng: &mut Rng) -> Draft {
    let lock = rng.percent(60);
    let diameter = *rng.pick(&[30, 32, 34]);
    let colour = *rng.pick(&["black", "grey", "orange", "green"]);
    let price = spread(rng, 10, 26);
    Draft::new(
        format!(
            "{} Grips {diameter}mm {colour}",
            if lock { "Lock-on" } else { "Slip-on" }
        ),
        price,
    )
    .attr("diameter_mm", diameter)
    .attr("lock_on", if lock { "yes" } else { "no" })
    .attr("colour", colour)
    .fits(&["mountain", "commuter", "e-bike"])
    .say(format!("{diameter}mm {colour} grips for flat handlebars."))
    .say(if lock {
        "Clamps on both ends, so they stay put without glue."
    } else {
        "Slide on with hairspray; light and cheap."
    })
}

pub(super) fn bottle_cage(rng: &mut Rng) -> Draft {
    let material = *rng.pick(&["alloy", "nylon", "carbon", "stainless steel"]);
    let side = *rng.pick(&["universal", "side-entry"]);
    let price = spread(rng, 6, if material == "carbon" { 38 } else { 16 });
    Draft::new(format!("{material} Bottle Cage {side}"), price)
        .attr("material", material)
        .attr("entry", side)
        .attr("bolt_spacing_mm", 64)
        .say(format!(
            "Bottle cage in {material} with {side} access; fits standard 64mm bolt holes."
        ))
        .say("Holds a 500-750ml bottle firmly over rough roads.")
}

pub(super) fn light(rng: &mut Rng) -> Draft {
    let front = rng.percent(55);
    let lumens = if front {
        *rng.pick(&[300, 600, 1000, 1500])
    } else {
        *rng.pick(&[30, 50, 100])
    };
    let position = if front { "front" } else { "rear" };
    let usb = rng.percent(85);
    let price = spread(rng, 14, 70);
    let charge = if usb {
        "USB-C rechargeable"
    } else {
        "AAA battery"
    };
    Draft::new(format!("{lumens} lumen {position} Light"), price)
        .attr("position", position)
        .attr("lumens", lumens)
        .attr("power", charge)
        .attr("mount", "strap")
        .say(format!(
            "A {position} light with {lumens} lumens, {charge}."
        ))
        .say("A strap mount fits handlebars and seatposts; a flash mode helps daytime visibility.")
}
