//! Text helpers the semantic model shares. Each states exactly what it does: the copies this replaces were
//! identical, and the variants that differ (single quotes too, trimming, dropping empty parts) stay where they are
//! until their callers are checked one by one (docs/reviews/2026-10-rework-plan.md, `helpers-dedupe`).

/// `s` trimmed, without one pair of surrounding double quotes. A doubled quote inside is kept as written.
pub(crate) fn unquote_double(s: &str) -> String {
    let t = s.trim();
    t.strip_prefix('"')
        .and_then(|r| r.strip_suffix('"'))
        .unwrap_or(t)
        .to_string()
}

/// `text` split at the top-level `sep`: outside double quotes and parentheses. The parts are not trimmed and empty
/// parts stay (`a,,b` gives three).
pub(crate) fn split_top_double(text: &str, sep: char) -> Vec<String> {
    let (mut out, mut cur, mut quoted, mut depth) = (Vec::new(), String::new(), false, 0);
    for c in text.chars() {
        match c {
            '"' => quoted = !quoted,
            '(' if !quoted => depth += 1,
            ')' if !quoted => depth -= 1,
            c if c == sep && !quoted && depth == 0 => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    out.push(cur);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unquote_double_trims_and_strips_one_pair() {
        assert_eq!(unquote_double("  \"No.\" "), "No.");
        assert_eq!(unquote_double("Plain"), "Plain");
        assert_eq!(unquote_double("'x'"), "'x'");
        assert_eq!(unquote_double("\"a\"\"b\""), "a\"\"b");
    }

    #[test]
    fn split_top_double_keeps_empty_parts_and_ignores_quoted_and_nested_separators() {
        assert_eq!(split_top_double("a,,b", ','), ["a", "", "b"]);
        assert_eq!(split_top_double("f(a,b),\"x,y\",z", ','), ["f(a,b)", "\"x,y\"", "z"]);
        assert_eq!(split_top_double("", ','), [""]);
        assert_eq!(split_top_double(" a , b ", ','), [" a ", " b "]);
    }
}
