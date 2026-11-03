//! Brake pads, rotors and cables.

use super::draft::{Draft, spread};
use crate::rng::Rng;

pub(super) fn brake_pads(rng: &mut Rng) -> Draft {
    let disc = rng.percent(75);
    let compound = *rng.pick(&["resin", "sintered", "semi-metallic"]);
    let price = spread(rng, 12, 28);
    let (kind, fit_tag, caliper) = if disc {
        let caliper = *rng.pick(&[
            "Type-A two-piston",
            "Type-B flat-mount",
            "Type-C four-piston",
        ]);
        ("Disc", "disc-brake", caliper)
    } else {
        let holder = *rng.pick(&["cartridge-holder", "threaded-post"]);
        ("Rim", "rim-brake", holder)
    };
    let bike = *rng.pick(&["road", "mountain", "gravel", "commuter"]);
    let behaviour = match compound {
        "resin" => "Quiet, with a soft initial bite; wears faster in the wet.",
        "sintered" => "Long wear and steady power in wet weather and on long descents.",
        _ => "A balance of bite and wear for mixed riding.",
    };
    Draft::new(format!("{kind} Brake Pads {compound} {caliper}"), price)
        .attr("brake_type", fit_tag)
        .attr("compound", compound)
        .attr("caliper_fit", caliper)
        .attr("quantity", "1 pair")
        .fit(fit_tag)
        .fits(&[bike])
        .say(format!(
            "One pair of {compound} {} brake pads for {caliper} calipers.",
            kind.to_lowercase()
        ))
        .say(behaviour)
}

pub(super) fn rotor(rng: &mut Rng) -> Draft {
    let diameter = *rng.pick(&[140, 160, 180, 203]);
    let mount = *rng.pick(&["6-bolt", "centerlock"]);
    let usage: &[&str] = match diameter {
        140 | 160 => &["road", "gravel"],
        180 => &["gravel", "mountain", "e-bike"],
        _ => &["mountain", "e-bike"],
    };
    let price = spread(rng, 18 + diameter / 10, 22);
    Draft::new(format!("{diameter}mm {mount} Disc Rotor"), price)
        .attr("diameter_mm", diameter)
        .attr("mount", mount)
        .attr("thickness_mm", *rng.pick(&["1.8", "2.0"]))
        .attr("material", "stainless steel")
        .fit("disc-brake")
        .fits(usage)
        .say(format!(
            "A {diameter}mm stainless steel rotor with a {mount} mount."
        ))
        .say(if diameter >= 180 {
            "The larger diameter adds stopping power and heat capacity on long descents."
        } else {
            "A compact rotor that keeps weight down."
        })
}

pub(super) fn cable(rng: &mut Rng) -> Draft {
    let shift = rng.percent(50);
    let head = *rng.pick(&["road", "mountain"]);
    let length = *rng.pick(&[1800, 2000, 2100, 2750]);
    let coating = *rng.pick(&["polished stainless", "PTFE-coated", "galvanized"]);
    let kind = if shift { "Shift" } else { "Brake" };
    let price = spread(rng, 6, 14);
    Draft::new(format!("{kind} Cable Set {length}mm {head}"), price)
        .attr("cable_type", &kind.to_lowercase())
        .attr("length_mm", length)
        .attr("coating", coating)
        .attr("lever_end", head)
        .fits(&[head])
        .say(format!(
            "A {coating} {} cable and housing set, {length}mm long, with a {head}-style lever end.",
            kind.to_lowercase()
        ))
        .say("Low-friction liner keeps the lever light.")
}
