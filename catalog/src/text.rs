//! Tokenizing, normalizing and fuzzy matching of search text.

/// Words that carry no signal in a buyer query.
const STOPWORDS: &[&str] = &[
    "a", "an", "the", "for", "my", "me", "with", "of", "on", "in", "to", "and", "or", "fit",
    "fits", "fitting", "that", "this", "is", "are", "i", "need", "want", "looking", "bike", "buy",
    "please", "under", "over", "good",
];

/// Query-side synonyms: a (normalized) buyer word and the catalog words it may stand for.
/// Shortest buyer word that may match a synonym key with a typo; shorter words are too ambiguous.
const MIN_TYPO_SYNONYM_CHARS: usize = 5;

const SYNONYMS: &[(&str, &[&str])] = &[
    ("tyre", &["tire"]),
    ("cogset", &["cassette"]),
    ("sprocket", &["cassette"]),
    ("freewheel", &["cassette"]),
    ("block", &["pad"]),
    ("disk", &["disc", "rotor"]),
    ("chainwheel", &["chainring"]),
    ("mech", &["derailleur"]),
    ("seat", &["saddle"]),
    ("bar", &["handlebar"]),
    ("oil", &["lube"]),
    ("lubricant", &["lube"]),
    ("headlight", &["light", "front"]),
    ("taillight", &["light", "rear"]),
    ("lamp", &["light"]),
    ("platform", &["flat"]),
    ("skewer", &["axle", "quick", "release"]),
    ("racing", &["road"]),
    ("mtb", &["mountain"]),
    ("city", &["commuter"]),
    ("hybrid", &["commuter"]),
    ("through", &["thru"]),
    ("clip", &["clipless"]),
];

/// Splits text into lowercase alphanumeric words; a dot between digits stays (`27.5`).
fn raw_tokens(input: &str) -> Vec<String> {
    let chars: Vec<char> = input.chars().collect();
    let mut out = Vec::new();
    let mut cur = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let digit_before = i > 0 && chars[i - 1].is_ascii_digit();
        let digit_after = chars.get(i + 1).is_some_and(char::is_ascii_digit);
        if c.is_alphanumeric() || (c == '.' && digit_before && digit_after) {
            cur.extend(c.to_lowercase());
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn is_number(token: &str) -> bool {
    !token.is_empty() && token.chars().all(|c| c.is_ascii_digit())
}

/// `11spd`, `11s`, `11speed` and `11sp` all become `11speed`.
fn speed_form(token: &str) -> Option<String> {
    ["speeds", "speed", "spd", "sp", "s"]
        .iter()
        .find_map(|suffix| {
            let num = token.strip_suffix(suffix)?;
            (is_number(num) && num.len() <= 2).then(|| format!("{num}speed"))
        })
}

/// `29er` and `29inch` become `29in`.
fn wheel_form(token: &str) -> Option<String> {
    ["inches", "inch", "in", "er"].iter().find_map(|suffix| {
        let num = token.strip_suffix(suffix)?;
        matches!(num, "26" | "27.5" | "29").then(|| format!("{num}in"))
    })
}

/// `34t` and `34T` become `34`: tooth counts are written both ways.
fn tooth_form(token: &str) -> Option<String> {
    let num = token.strip_suffix('t')?;
    (is_number(num) && num.len() <= 2).then(|| num.to_owned())
}

/// Drops a plural from plain words of four or more letters: `es` after `x`, `z`, `ch`, `sh` and
/// `ss` (`boxes` is `box`), otherwise a bare `s`.
fn stem(token: &str) -> &str {
    let es_plural = token.len() > 4
        && token.chars().all(|c| c.is_ascii_alphabetic())
        && ["xes", "zes", "ches", "shes", "sses"]
            .iter()
            .any(|ending| token.ends_with(ending));
    if es_plural {
        return &token[..token.len() - 2];
    }
    let plural = token.len() > 3
        && token.chars().all(|c| c.is_ascii_alphabetic())
        && token.ends_with('s')
        && !token.ends_with("ss")
        && !token.ends_with("us")
        && !token.ends_with("is");
    if plural {
        &token[..token.len() - 1]
    } else {
        token
    }
}

fn normalize(tokens: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(tokens.len());
    let mut iter = tokens.into_iter().peekable();
    while let Some(token) = iter.next() {
        let next = iter.peek().map(String::as_str);
        if is_number(&token) && matches!(next, Some("speed" | "speeds" | "spd" | "sp")) {
            iter.next();
            out.push(format!("{token}speed"));
        } else if matches!(token.as_str(), "26" | "27.5" | "29")
            && matches!(next, Some("in" | "inch" | "inches"))
        {
            iter.next();
            out.push(format!("{token}in"));
        } else if token == "e" && next == Some("bike") {
            iter.next();
            out.push("ebike".to_owned());
        } else if let Some(form) = speed_form(&token)
            .or_else(|| wheel_form(&token))
            .or_else(|| tooth_form(&token))
        {
            out.push(form);
        } else {
            out.push(stem(&token).to_owned());
        }
    }
    out
}

/// Normalized index and query tokens of `input`.
pub fn tokens(input: &str) -> Vec<String> {
    normalize(raw_tokens(input))
}

/// Tokens of a buyer query with stopwords removed.
#[must_use]
pub fn query_tokens(input: &str) -> Vec<String> {
    let mut out = tokens(input);
    out.retain(|t| !STOPWORDS.contains(&t.as_str()));
    out
}

/// Catalog words a buyer word may stand for. With `allow_typo`, a word of five or more letters
/// also matches a synonym key one typo away; pass `false` for a word the catalog already uses,
/// or "black" would reach the key "block" and pull in brake pads.
pub fn synonyms(token: &str, allow_typo: bool) -> &'static [&'static str] {
    let typo_ok = allow_typo && token.chars().count() >= MIN_TYPO_SYNONYM_CHARS;
    SYNONYMS
        .iter()
        .find(|(word, _)| *word == token || (typo_ok && distance_within(word, token, 1).is_some()))
        .map_or(&[], |(_, expansions)| *expansions)
}

/// The canonical spelling of a compatibility tag: `11 speed` is `11-speed`, `Disc Brakes` is
/// `disc-brake`.
#[must_use]
pub fn canonical_tag(raw: &str) -> String {
    tokens(raw)
        .iter()
        .map(|t| match t.strip_suffix("speed") {
            Some(num) if is_number(num) => format!("{num}-speed"),
            _ if t == "ebike" => "e-bike".to_owned(),
            _ => t.clone(),
        })
        .collect::<Vec<_>>()
        .join("-")
}

/// Strips everything but letters and digits, for comparing part numbers.
pub fn compact(input: &str) -> String {
    input
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Longest word, in characters, that [`distance_within`] compares; a longer, different word is
/// never a typo of anything in the vocabulary. The cap keeps the rows on the stack.
const MAX_WORD_CHARS: usize = 64;

/// A cell outside the band or past the budget; half of `usize::MAX` so adding one cannot overflow.
const BEYOND_BAND: usize = usize::MAX / 2;

type Row = [usize; MAX_WORD_CHARS + 1];

fn chars_of(word: &str) -> Option<([char; MAX_WORD_CHARS], usize)> {
    let mut out = ['\0'; MAX_WORD_CHARS];
    let mut len = 0;
    for c in word.chars() {
        *out.get_mut(len)? = c;
        len += 1;
    }
    Some((out, len))
}

/// Edit distance (insert, delete, substitute, swap neighbours) if it is at most `max`.
///
/// Only the diagonal band of width `2 * max + 1` is computed and the scan stops as soon as a
/// whole row exceeds `max`, so a call costs `O(max * len)` and never allocates. It runs once per
/// vocabulary term for every query word that is not itself a term.
pub fn distance_within(a: &str, b: &str, max: usize) -> Option<usize> {
    if a == b {
        return Some(0);
    }
    let ((a, a_len), (b, b_len)) = (chars_of(a)?, chars_of(b)?);
    if a_len.abs_diff(b_len) > max {
        return None;
    }
    let (a, b) = (&a[..a_len], &b[..b_len]);
    let mut older: Row = [BEYOND_BAND; MAX_WORD_CHARS + 1];
    let mut previous: Row = [BEYOND_BAND; MAX_WORD_CHARS + 1];
    for (j, cell) in previous.iter_mut().enumerate().take(b_len.min(max) + 1) {
        *cell = j;
    }
    for i in 1..=a_len {
        let mut current: Row = [BEYOND_BAND; MAX_WORD_CHARS + 1];
        let low = i.saturating_sub(max);
        let high = (i + max).min(b_len);
        let mut row_best = BEYOND_BAND;
        if low == 0 {
            current[0] = i;
            row_best = i;
        }
        for j in low.max(1)..=high {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (previous[j] + 1)
                .min(current[j - 1] + 1)
                .min(previous[j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(older[j - 2] + 1);
            }
            current[j] = best;
            row_best = row_best.min(best);
        }
        if row_best > max {
            return None;
        }
        older = previous;
        previous = current;
    }
    let d = previous[b_len];
    (d <= max).then_some(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_spellings_collapse() {
        for q in ["11-speed", "11 speed", "11spd", "11s", "11 Speed"] {
            assert_eq!(tokens(q), vec!["11speed"], "{q}");
        }
    }

    #[test]
    fn wheel_spellings_collapse_and_decimals_survive() {
        assert_eq!(tokens("29er"), vec!["29in"]);
        assert_eq!(tokens("27.5 inch"), vec!["27.5in"]);
        assert_eq!(tokens("27.5in"), vec!["27.5in"]);
    }

    #[test]
    fn tooth_counts_lose_their_suffix() {
        assert_eq!(tokens("11-34T"), vec!["11", "34"]);
        assert_eq!(tokens("38t chainring"), vec!["38", "chainring"]);
    }

    #[test]
    fn e_bike_is_one_token() {
        assert_eq!(tokens("e-bike"), vec!["ebike"]);
        assert_eq!(tokens("E bike tires"), vec!["ebike", "tire"]);
    }

    #[test]
    fn plurals_are_stemmed_but_short_words_are_not() {
        assert_eq!(
            tokens("brake pads tires gas"),
            vec!["brake", "pad", "tire", "gas"]
        );
    }

    #[test]
    fn query_tokens_drop_filler_words() {
        assert_eq!(
            query_tokens("a chain that fits my 11-speed road bike"),
            vec!["chain", "11speed", "road"]
        );
    }

    #[test]
    fn es_plurals_lose_the_whole_ending() {
        assert_eq!(
            tokens("boxes brushes watches buzzes glasses"),
            ["box", "brush", "watch", "buzz", "glass"]
        );
        assert_eq!(tokens("tires cases axes"), ["tire", "case", "axe"]);
    }

    #[test]
    fn canonical_tags_match_the_catalog_spelling() {
        assert_eq!(canonical_tag("Disc Brakes"), "disc-brake");
        assert_eq!(canonical_tag("12 speed"), "12-speed");
        assert_eq!(canonical_tag("e-bike"), "e-bike");
        assert_eq!(canonical_tag("700C"), "700c");
    }

    #[test]
    fn distance_counts_swaps_as_one_edit() {
        assert_eq!(distance_within("disc", "dics", 1), Some(1));
        assert_eq!(distance_within("cassette", "casette", 1), Some(1));
        assert_eq!(distance_within("chain", "chair", 1), Some(1));
        assert_eq!(distance_within("chain", "brake", 2), None);
    }

    #[test]
    fn distance_stops_at_the_budget_and_handles_edges() {
        assert_eq!(distance_within("", "ab", 2), Some(2));
        assert_eq!(distance_within("", "abc", 2), None);
        assert_eq!(distance_within("rotor", "rotor", 0), Some(0));
        assert_eq!(distance_within("cassette", "casette", 2), Some(1));
        assert_eq!(distance_within("derailleur", "derailler", 2), Some(1));
        assert_eq!(distance_within("kitten", "sitting", 2), None);
        assert_eq!(distance_within("kitten", "sitting", 3), Some(3));
        let long = "a".repeat(100);
        assert_eq!(distance_within(&long, &long, 1), Some(0));
        assert_eq!(distance_within(&long, &format!("{long}b"), 1), None);
    }

    #[test]
    fn a_typo_of_a_synonym_key_matches_only_when_allowed() {
        assert_eq!(synonyms("blok", true), &[] as &[&str]);
        assert_eq!(synonyms("tyres", true), &["tire"]);
        assert_eq!(synonyms("tyres", false), &[] as &[&str]);
        assert_eq!(synonyms("tyre", false), &["tire"]);
    }

    #[test]
    fn compact_ignores_punctuation_and_case() {
        assert_eq!(compact("VEL-ch 1042"), "velch1042");
    }
}
