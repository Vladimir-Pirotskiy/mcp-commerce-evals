//! Chains, cassettes, chainrings, rear derailleurs and lube.

use super::draft::{Draft, SPEEDS, bikes_for_speed, speed_tag, spread};
use crate::rng::Rng;

const CHAIN_BLURBS: [&str; 4] = [
    "Shifts cleanly under load and stays quiet on long rides.",
    "Includes a reusable quick link for roadside repairs.",
    "Treated to resist rust through a wet winter.",
    "A solid everyday choice for training and commuting.",
];

pub(super) fn chain(rng: &mut Rng) -> Draft {
    let speed = *rng.pick(&SPEEDS);
    let finish = *rng.pick(&[
        "nickel-plated",
        "zinc-coated",
        "hardened steel",
        "hollow-pin",
    ]);
    let links = *rng.pick(&[112, 116, 118, 126]);
    let price = spread(rng, 8 + 4 * (speed - 7), 14);
    Draft::new(format!("{speed}-Speed Chain"), price)
        .attr("speed", &speed_tag(speed))
        .attr("links", links)
        .attr("finish", finish)
        .attr("quick_link", "included")
        .fit(&speed_tag(speed))
        .fits(bikes_for_speed(speed))
        .say(format!(
            "A {finish} chain with {links} links for {speed}-speed drivetrains."
        ))
        .say(*rng.pick(&CHAIN_BLURBS))
}

const CASSETTE_RANGES: [(&str, &str); 9] = [
    ("11-25", "road"),
    ("11-28", "road"),
    ("11-30", "road"),
    ("11-32", "gravel"),
    ("11-34", "gravel"),
    ("11-36", "mountain"),
    ("10-45", "mountain"),
    ("10-51", "mountain"),
    ("12-32", "commuter"),
];

pub(super) fn cassette(rng: &mut Rng) -> Draft {
    let speed = *rng.pick(&[9, 10, 11, 12]);
    let (range, usage) = *rng.pick(&CASSETTE_RANGES);
    let freehub = if range == "10-51" || range == "10-45" {
        "wide"
    } else {
        "standard"
    };
    let price = spread(rng, 30 + 10 * (speed - 8), 60);
    Draft::new(format!("{speed}-Speed {range}T {usage} Cassette"), price)
        .attr("speed", &speed_tag(speed))
        .attr("tooth_range", &format!("{range}T"))
        .attr("freehub", freehub)
        .attr("weight_g", rng.range(190, 480))
        .fit(&speed_tag(speed))
        .fits(&[usage])
        .say(format!(
            "{speed}-speed {usage} cassette with a {range}T range on a {freehub} freehub."
        ))
        .say(*rng.pick(&[
            "Steel cogs with a machined carrier keep the shift ramps crisp.",
            "Wide gaps between sprockets make hill climbs easier.",
            "Tight gearing keeps the cadence steady in a group ride.",
        ]))
}

pub(super) fn chainring(rng: &mut Rng) -> Draft {
    let speed = *rng.pick(&[9, 10, 11, 12]);
    let teeth = *rng.pick(&[30, 32, 34, 36, 38, 42, 46, 50, 52]);
    let bcd = *rng.pick(&[94, 104, 110, 130]);
    let single = teeth <= 38;
    let kind = if single { "Narrow-Wide" } else { "Road" };
    let usage = if single { "mountain" } else { "road" };
    let price = spread(rng, 28, 55);
    Draft::new(format!("{teeth}T {kind} Chainring {bcd}mm BCD"), price)
        .attr("teeth", teeth)
        .attr("bcd", &format!("{bcd}mm"))
        .attr("speed", &speed_tag(speed))
        .fit(&speed_tag(speed))
        .fits(&[usage])
        .say(format!(
            "A {teeth}-tooth {kind} chainring on a {bcd}mm bolt circle."
        ))
        .say(if single {
            "Alternating tooth shapes hold the chain on rough ground."
        } else {
            "Ramped teeth give quick shifts between rings."
        })
}

pub(super) fn rear_derailleur(rng: &mut Rng) -> Draft {
    let speed = *rng.pick(&[9, 10, 11, 12]);
    let cage = *rng.pick(&["short", "medium", "long"]);
    let clutch = rng.percent(50);
    let max_cog = match cage {
        "short" => 28,
        "medium" => 34,
        _ => 51,
    };
    let price = spread(rng, 35 + 12 * (speed - 8), 90);
    let clutch_text = if clutch { "clutch" } else { "no-clutch" };
    Draft::new(format!("{speed}-Speed {cage}-cage Rear Derailleur"), price)
        .attr("speed", &speed_tag(speed))
        .attr("cage", cage)
        .attr("max_cog", &format!("{max_cog}T"))
        .attr("clutch", if clutch { "yes" } else { "no" })
        .fit(&speed_tag(speed))
        .fits(bikes_for_speed(speed))
        .say(format!(
            "A {cage}-cage rear derailleur ({clutch_text}) that takes up to a {max_cog}T cog."
        ))
        .say(*rng.pick(&[
            "A stiff parallelogram keeps shifts precise.",
            "A rotating clutch reduces chain slap on rough ground.",
            "Sealed pulley bearings spin freely.",
        ]))
}

pub(super) fn chain_lube(rng: &mut Rng) -> Draft {
    let kind = *rng.pick(&["wet", "dry", "wax"]);
    let ml = *rng.pick(&[60, 120, 250]);
    let price = spread(rng, 7, 12);
    let use_for = match kind {
        "wet" => "rain, mud and winter riding",
        "dry" => "dusty roads and summer rides",
        _ => "a clean drivetrain with little dirt pickup",
    };
    Draft::new(format!("{kind} Chain Lube {ml}ml"), price)
        .attr("type", kind)
        .attr("volume_ml", ml)
        .say(format!("A {kind} lubricant for {use_for}."))
        .say("Drip bottle with a fine nozzle; works on any chain.")
}
