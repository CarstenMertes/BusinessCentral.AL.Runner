//! The tests Microsoft turns off in its own CI: BCApps' `src/DisabledTests/<App>/*.json`.
//!
//! Each file is a JSON array of `{ "codeunitId": N, "codeunitName": "...", "method": "Name" }`; a method of `"*"` disables
//! the whole codeunit. A few files hold one such object instead of an array, and one spells the keys with capitals. There
//! is no reason in the files, only (sometimes) a `bug` number, so the runner reports a test as "disabled by Microsoft"
//! and nothing more. The files are read by the host; this module only parses their text and answers questions.

use std::collections::HashSet;

/// The set of tests Microsoft disables.
#[derive(Debug, Clone, Default)]
pub struct DisabledTests {
    whole: HashSet<u32>,
    methods: HashSet<(u32, String)>,
    entries: usize,
    files: usize,
}

impl DisabledTests {
    /// Adds the entries of one file's text. `Err`: the text is not JSON of the expected shape.
    pub fn add_file(&mut self, text: &str) -> Result<usize, String> {
        let text = text.trim_start_matches('\u{feff}');
        let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
        let list: Vec<&serde_json::Value> = match &v {
            serde_json::Value::Array(a) => a.iter().collect(),
            serde_json::Value::Object(_) => vec![&v],
            _ => return Err("neither an array nor an object".into()),
        };
        let mut n = 0;
        for e in list {
            let serde_json::Value::Object(o) = e else {
                return Err("an entry is not an object".into());
            };
            let get = |key: &str| o.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v);
            let id = match get("codeunitId") {
                Some(serde_json::Value::Number(n)) => n.as_u64().and_then(|n| u32::try_from(n).ok()),
                Some(serde_json::Value::String(s)) => s.trim().parse().ok(),
                _ => None,
            }
            .ok_or("an entry has no codeunitId")?;
            let method = match get("method") {
                Some(serde_json::Value::String(s)) => s.trim().to_string(),
                _ => return Err("an entry has no method".into()),
            };
            if method == "*" {
                self.whole.insert(id);
            } else {
                self.methods.insert((id, method.to_lowercase()));
            }
            n += 1;
        }
        self.entries += n;
        self.files += 1;
        Ok(n)
    }

    /// Whether Microsoft disables this test.
    pub fn is_disabled(&self, codeunit: u32, method: &str) -> bool {
        self.whole.contains(&codeunit) || self.methods.contains(&(codeunit, method.to_lowercase()))
    }

    /// The entries read so far.
    pub fn entries(&self) -> usize {
        self.entries
    }

    /// The files read so far.
    pub fn files(&self) -> usize {
        self.files
    }

    pub fn is_empty(&self) -> bool {
        self.entries == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn methods_whole_codeunits_and_odd_files_are_read() {
        let mut d = DisabledTests::default();
        assert_eq!(
            d.add_file(
                "\u{feff}[{\"codeunitId\":134141,\"codeunitName\":\"X\",\"method\":\"DoIt\"},\
                 {\"codeunitId\":5,\"codeunitName\":\"Y\",\"method\":\"*\",\"bug\":12}]"
            ),
            Ok(2)
        );
        assert_eq!(
            d.add_file("{\"CodeunitId\":7,\"CodeunitName\":\"Z\",\"Method\":\"Cap\"}"),
            Ok(1)
        );
        assert!(d.is_disabled(134141, "doit") && d.is_disabled(134141, "DOIT"));
        assert!(!d.is_disabled(134141, "Other"));
        assert!(d.is_disabled(5, "Anything"));
        assert!(d.is_disabled(7, "cap"));
        assert_eq!((d.entries(), d.files()), (3, 2));
        assert!(d.add_file("[1]").is_err() && d.add_file("{\"method\":\"a\"}").is_err() && d.add_file("x").is_err());
    }
}
