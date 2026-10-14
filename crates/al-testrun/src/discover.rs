//! Finding test codeunits and their `[Test]` procedures in AL source text.
//!
//! A deliberately shallow text scan: it reads no syntax tree, so it is cheap enough to run over a
//! whole bucket before anything is compiled (to choose what to compile for a test filter).

/// `(codeunit id, [Test] procedure names)` for every `Subtype = Test` codeunit in `src`
/// (comments are ignored).
pub fn test_codeunits(src: &str) -> Vec<(u32, Vec<String>)> {
    scan(&strip_comments(src))
}

/// Blanks `//` line comments and `/* */` block comments, keeping string
/// literals intact so a `//` inside quotes is not mistaken for a comment.
fn strip_comments(src: &str) -> String {
    let b = src.as_bytes();
    let mut out = String::with_capacity(src.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'\'' => {
                let start = i;
                i += 1;
                while i < b.len() && b[i] != b'\'' && b[i] != b'\n' {
                    i += 1;
                }
                i = (i + 1).min(b.len());
                out.push_str(&src[start..i]);
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
                while i < b.len() && b[i] != b'\'' && b[i] != b'/' {
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
fn scan(src: &str) -> Vec<(u32, Vec<String>)> {
    let words: Vec<&str> = src
        .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '[' || c == ']'))
        .filter(|w| !w.is_empty())
        .collect();
    let mut out: Vec<(u32, Vec<String>, bool)> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        if w.eq_ignore_ascii_case("codeunit") {
            if let Some(id) = words.get(i + 1).and_then(|n| n.parse::<u32>().ok()) {
                // Only an object header is followed by an id; `Record Codeunit` etc. is not.
                out.push((id, Vec::new(), false));
                i += 2;
                continue;
            }
        }
        if let Some(cur) = out.last_mut() {
            if w.eq_ignore_ascii_case("subtype") {
                if words.get(i + 1).is_some_and(|v| v.eq_ignore_ascii_case("test")) {
                    cur.2 = true;
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
    out.into_iter().filter(|c| c.2).map(|c| (c.0, c.1)).collect()
}
