//! Tires, inner tubes, rear hubs and axles.

use super::draft::{Draft, spread};
use crate::rng::Rng;

pub(super) fn tire(rng: &mut Rng) -> Draft {
    // (wheel tag, size text, widths, bike type)
    let (wheel, size, widths, bike): (&str, &str, &[&str], &str) = match rng.below(6) {
        0 => ("700c", "700", &["25c", "28c", "30c", "32c"], "road"),
        1 => ("700c", "700", &["38c", "40c", "42c"], "gravel"),
        2 => ("650b", "650b", &["47c", "50c"], "gravel"),
        3 => ("29in", "29", &["2.25", "2.4", "2.6"], "mountain"),
        4 => ("27.5in", "27.5", &["2.25", "2.4"], "mountain"),
        _ => ("700c", "700", &["35c", "38c"], "commuter"),
    };
    let width = *rng.pick(widths);
    let casing = *rng.pick(&["tubeless-ready", "clincher", "folding clincher"]);
    let protection = bike == "commuter" || rng.percent(30);
    let price = spread(rng, 24, 60);
    let name = format!("{size}x{width} {bike} Tire {casing}");
    let mut draft = Draft::new(name, price)
        .attr("wheel_size", wheel)
        .attr("width", width)
        .attr("casing", casing)
        .attr("puncture_protection", if protection { "yes" } else { "no" })
        .fit(wheel)
        .fits(&[bike])
        .say(format!(
            "A {casing} {bike} tire, {width} wide, for {wheel} wheels."
        ));
    draft = draft.say(if protection {
        "A puncture protection belt under the tread cuts down on flats."
    } else {
        "A supple casing for low rolling resistance and grip."
    });
    draft
}

pub(super) fn tube(rng: &mut Rng) -> Draft {
    let (wheel, widths) = *rng.pick(&[
        ("700c", "23-32c"),
        ("700c", "35-45c"),
        ("650b", "47-54c"),
        ("29in", "2.1-2.5in"),
        ("27.5in", "2.1-2.5in"),
        ("26in", "1.9-2.3in"),
    ]);
    let valve = *rng.pick(&["presta", "schrader"]);
    let length = *rng.pick(&[32, 40, 48, 60, 80]);
    let material = *rng.pick(&["butyl", "butyl", "latex", "TPU"]);
    let price = spread(rng, 4, if material == "butyl" { 6 } else { 14 });
    Draft::new(
        format!("{wheel} {valve} Inner Tube {widths} {length}mm valve"),
        price,
    )
    .attr("wheel_size", wheel)
    .attr("tire_width", widths)
    .attr("valve", valve)
    .attr("valve_length_mm", length)
    .attr("material", material)
    .fit(wheel)
    .say(format!(
        "{material} inner tube with a {valve} valve ({length}mm) for {wheel} wheels ({widths})."
    ))
    .say(if material == "butyl" {
        "Holds air well and is easy to patch."
    } else {
        "Lighter than butyl, with lower rolling resistance."
    })
}

pub(super) fn axle(rng: &mut Rng) -> Draft {
    let axles: [(&str, &[&str], bool); 4] = [
        ("12x100", &["road", "gravel"], false),
        ("12x142", &["road", "gravel", "mountain"], true),
        ("15x110", &["mountain", "e-bike"], false),
        ("quick-release", &["road", "commuter"], true),
    ];
    let (standard, bikes, rear) = *rng.pick(&axles);
    let position = if rear { "rear" } else { "front" };
    let kind = if standard == "quick-release" {
        "Skewer"
    } else {
        "Thru Axle"
    };
    let price = spread(rng, 14, 30);
    Draft::new(format!("{standard} {position} {kind}"), price)
        .attr("standard", standard)
        .attr("position", position)
        .attr("material", *rng.pick(&["alloy", "chromoly"]))
        .fit(standard)
        .fits(bikes)
        .say(format!(
            "A {position} {} that fits {standard} dropouts.",
            kind.to_lowercase()
        ))
        .say("Fits through the hub and threads into the frame; includes the lever.")
}

pub(super) fn hub(rng: &mut Rng) -> Draft {
    let standard = *rng.pick(&["quick-release", "12x142", "12x142", "15x110"]);
    let disc = standard != "quick-release" || rng.percent(40);
    let mount = if disc {
        *rng.pick(&["6-bolt", "centerlock"])
    } else {
        "rim brake"
    };
    let freehub = *rng.pick(&["standard", "wide"]);
    let spokes = *rng.pick(&[24, 28, 32]);
    let speeds: &[&str] = if freehub == "wide" {
        &["11-speed", "12-speed"]
    } else {
        &["8-speed", "9-speed", "10-speed", "11-speed"]
    };
    let price = spread(rng, 55, 140);
    let brake_tag = if disc { "disc-brake" } else { "rim-brake" };
    Draft::new(format!("Rear Hub {standard} {freehub} freehub {spokes}H"), price)
        .attr("axle_standard", standard)
        .attr("freehub", freehub)
        .attr("brake_mount", mount)
        .attr("spoke_holes", spokes)
        .fit(standard)
        .fit(brake_tag)
        .fits(speeds)
        .say(format!("A {spokes}-hole rear hub on a {standard} axle with a {freehub} freehub and {mount} brake mount."))
        .say("Sealed cartridge bearings and a quick-engaging freehub.")
}
