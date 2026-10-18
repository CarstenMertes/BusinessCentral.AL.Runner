//! Finding test codeunits and their `[Test]` procedures in AL source text.
//!
//! A deliberately shallow text scan: it reads no syntax tree, so it is cheap enough to run over a
//! whole bucket before anything is compiled (to choose what to compile for a test filter).

/// `(codeunit id, [Test] procedure names)` for every `Subtype = Test` codeunit in `src`
/// (comments are ignored).
pub fn test_codeunits(src: &str) -> Vec<(u32, Vec<String>)> {
    scan(&strip_comments(src)).into_iter().map(|c| (c.0, c.1)).collect()
}

/// `(codeunit id, TestType)` for every `Subtype = Test` codeunit in `src`. The type is the value of the codeunit's
/// `TestType = ...;` property as written (`UnitTest`, `IntegrationTest`, `Uncategorized`, `AITest`), or `None` when the
/// codeunit has no such property. Microsoft's recipe picks the company of a run from it (BCApps
/// `build/scripts/AppObjectValidation.psm1`, regex `TestType\s+=\s+(\w+);`).
pub fn test_codeunit_types(src: &str) -> Vec<(u32, Option<String>)> {
    scan(&strip_comments(src)).into_iter().map(|c| (c.0, c.2)).collect()
}

/// Blanks `//` line comments and `/* */` block comments and the text of string
/// literals, so a `//` inside quotes is not mistaken for a comment and words in a string are not code.
fn strip_comments(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            q @ (b'\'' | b'"') => {
                let start = i;
                i += 1;
                while i < b.len() && b[i] != q && b[i] != b'\n' {
                    i += 1;
                }
                i = (i + 1).min(b.len());
                // The text of a string is blanked (same length): words inside it (`codeunit 449`) are not code.
                out.extend(
                    src[start..i]
                        .chars()
                        .map(|c| if c == '\'' || c == '"' || c == '\n' { c } else { ' ' }),
                );
            }
            b'/' if b.get(i + 1) == Some(&b'/') => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if b.get(i + 1) == Some(&b'*') => {
                i += 2;
                while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                    i += 1;
                }
                i = (i + 2).min(b.len());
            }
            _ => {
                let start = i;
                while i < b.len() && b[i] != b'\'' && b[i] != b'"' && b[i] != b'/' {
                    i += 1;
                }
                if i == start {
                    i += 1;
                }
                out.push_str(&src[start..i]);
            }
        }
    }
    out
}

/// Returns `(codeunit id, [Test] procedure names)` for every test codeunit in `src`.
fn scan(src: &str) -> Vec<(u32, Vec<String>, Option<String>)> {
    let words: Vec<&str> = src
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '[' || c == ']'))
        .filter(|w| !w.is_empty())
        .collect();
    let mut out: Vec<(u32, Vec<String>, bool, Option<String>)> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        if w.eq_ignore_ascii_case("codeunit") {
            if let Some(id) = words.get(i + 1).and_then(|n| n.parse::<u32>().ok()) {
                // Only an object header is followed by an id; `Record Codeunit` etc. is not.
                out.push((id, Vec::new(), false, None));
                i += 2;
                continue;
            }
        }
        if let Some(cur) = out.last_mut() {
            if w.eq_ignore_ascii_case("subtype") {
                if words.get(i + 1).is_some_and(|v| v.eq_ignore_ascii_case("test")) {
                    cur.2 = true;
                }
            } else if w.eq_ignore_ascii_case("testtype") {
                if let Some(v) = words.get(i + 1) {
                    cur.3.get_or_insert_with(|| v.to_string());
                }
            } else if w.eq_ignore_ascii_case("[test]") {
                if let Some(pos) = words[i + 1..].iter().position(|w| w.eq_ignore_ascii_case("procedure")) {
                    if let Some(name) = words.get(i + 1 + pos + 1) {
                        cur.1.push(name.to_string());
                    }
                }
            }
        }
        i += 1;
    }
    out.into_iter().filter(|c| c.2).map(|c| (c.0, c.1, c.3)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quoted_name_that_starts_with_a_number_does_not_start_a_codeunit() {
        // Tests-Misc "6.0SP1 - VAT 1 to 1" (132517): `Codeunit::"6.0SP1 - VAT 1 to 1"` reads as the words `Codeunit 6`; 38 tests were lost.
        let src = "codeunit 132517 \"6.0SP1 - VAT 1 to 1\"\n{\n Subtype = Test;\n [Test]\n procedure A()\n begin X(Codeunit::\"6.0SP1 - VAT 1 to 1\"); end;\n [Test]\n procedure B()\n begin end;\n}\n";
        assert_eq!(
            test_codeunits(src),
            vec![(132517, vec!["A".to_string(), "B".to_string()])]
        );
    }

    #[test]
    fn a_string_that_says_codeunit_and_a_number_does_not_start_a_codeunit() {
        // Tests-Misc "Test Job Queue SNAP" (139020): a Label text names "COD449" and "codeunit 449"; the scan took that for a
        // second codeunit header and lost every [Test] after it, so `--parity` dropped 68 tests that Microsoft does not disable.
        let src = "codeunit 139020 \"Snap\"\n{\n Subtype = Test;\n var T: Label 'run codeunit 449 now';\n [Test]\n procedure A()\n begin end;\n [Test]\n procedure B()\n begin end;\n}\n";
        assert_eq!(
            test_codeunits(src),
            vec![(139020, vec!["A".to_string(), "B".to_string()])]
        );
    }

    #[test]
    fn the_test_type_property_is_read_per_codeunit() {
        let src = "codeunit 1 \"A\" { Subtype = Test; TestType = UnitTest; [Test] procedure X() begin end; }\n\
                   codeunit 2 \"B\" { Subtype = Test; [Test] procedure Y() begin end; }\n\
                   codeunit 3 \"C\" { Subtype = Test; TestType = IntegrationTest; RequiredTestIsolation = Disabled; [Test] procedure Z() begin end; }\n\
                   codeunit 4 \"Lib\" { TestType = UnitTest; }";
        assert_eq!(
            test_codeunit_types(src),
            vec![
                (1, Some("UnitTest".to_string())),
                (2, None),
                (3, Some("IntegrationTest".to_string()))
            ]
        );
    }

    #[test]
    fn two_codeunits_and_comments_are_still_found() {
        let src = "codeunit 1 \"A\" { Subtype = Test; [Test] procedure X() begin end; }\n// codeunit 9 \"no\"\ncodeunit 2 \"B\" { Subtype = Test; [Test] procedure Y() begin end; }";
        assert_eq!(
            test_codeunits(src),
            vec![(1, vec!["X".to_string()]), (2, vec!["Y".to_string()])]
        );
    }
}
