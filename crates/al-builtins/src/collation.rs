//! The order the database sorts text in (the collation of a BC SQL database), for the part of it
//! the corpus pins.
//!
//! Pinned by query/TestQueryRuntimeContracts.al:Query_OrderByText_UsesTheDatabaseCollation_NotOrdinalOrder
//! and :Query_GroupByText_IsCaseSensitive: `_x`, `1`, `a`, `a`, `A`, `ä`, `a b`, `ab`, `b`, `B`, `Z`.
//! That order reads as three passes over the whole text: first the base letters (punctuation
//! before digits before letters, case and accents ignored), then the accents (none before an
//! umlaut), then the case (lower before upper). Characters beyond ASCII and the umlauts ä, ö, ü
//! are not pinned: [`supported`] says no and callers refuse.

use std::cmp::Ordering;

/// Whether every character of `s` has a pinned place in the order.
pub fn supported(s: &str) -> bool {
    s.chars()
        .all(|c| (' '..='~').contains(&c) || matches!(c, 'ä' | 'ö' | 'ü' | 'Ä' | 'Ö' | 'Ü'))
}

/// (primary, accent, upper-case) of one character.
fn weights(c: char) -> (u32, u8, u8) {
    let (base, accent) = match c {
        'ä' | 'Ä' => ('a', 1),
        'ö' | 'Ö' => ('o', 1),
        'ü' | 'Ü' => ('u', 1),
        c => (c, 0),
    };
    let upper = u8::from(c.is_uppercase());
    let lower = base.to_ascii_lowercase();
    let primary = if lower == ' ' {
        0x100
    } else if lower.is_ascii_digit() {
        0x300 + u32::from(lower)
    } else if lower.is_ascii_lowercase() {
        0x400 + u32::from(lower)
    } else {
        // Punctuation and symbols sort before the digits; their order among themselves is not pinned.
        0x200 + u32::from(lower)
    };
    (primary, accent, upper)
}

/// Compares two texts that are [`supported`].
pub fn compare(a: &str, b: &str) -> Ordering {
    let wa: Vec<_> = a.chars().map(weights).collect();
    let wb: Vec<_> = b.chars().map(weights).collect();
    wa.iter()
        .map(|w| w.0)
        .cmp(wb.iter().map(|w| w.0))
        .then_with(|| wa.iter().map(|w| w.1).cmp(wb.iter().map(|w| w.1)))
        .then_with(|| wa.iter().map(|w| w.2).cmp(wb.iter().map(|w| w.2)))
}

#[cfg(test)]
mod tests {
    use super::*;

    // query/TestQueryRuntimeContracts.al:Query_OrderByText_UsesTheDatabaseCollation_NotOrdinalOrder
    #[test]
    fn the_pinned_order() {
        let pinned = ["_x", "1", "a", "a", "A", "ä", "a b", "ab", "b", "B", "Z"];
        let mut shuffled = ["Z", "b", "ab", "A", "a b", "ä", "B", "a", "1", "_x", "a"];
        shuffled.sort_by(|x, y| compare(x, y));
        assert_eq!(shuffled, pinned);
    }

    #[test]
    fn only_ascii_and_umlauts_are_pinned() {
        assert!(supported("Ärger ab_1"));
        assert!(!supported("é"));
        assert!(!supported("ß"));
    }
}
