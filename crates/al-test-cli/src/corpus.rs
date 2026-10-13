//! Corpus discovery: which test codeunits exist, which area folder each lives
//! in, and which `[Test]` procedures it declares.
//!
//! This is a deliberately shallow text scan, good enough to attribute oracle
//! cases to areas before the real parser exists. It is replaced by `al-syntax`
//! once the parser covers the corpus.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestCodeunit {
    pub id: u32,
    pub area: String,
    pub file: PathBuf,
    pub tests: Vec<String>,
}

#[derive(Debug, Default)]
pub struct CorpusIndex {
    pub codeunits: BTreeMap<u32, TestCodeunit>,
    /// Every `.al` file under the app root, sorted.
    pub files: Vec<PathBuf>,
}

impl CorpusIndex {
    pub fn area_of(&self, codeunit: u32) -> Option<&str> {
        self.codeunits.get(&codeunit).map(|c| c.area.as_str())
    }
}

pub fn discover(app_root: &Path) -> std::io::Result<CorpusIndex> {
    let mut ix = CorpusIndex::default();
    walk(app_root, &mut ix.files)?;
    ix.files.sort();
    for file in &ix.files {
        let src = std::fs::read_to_string(file)?;
        let rel = file.strip_prefix(app_root).unwrap_or(file);
        let area = match rel.components().next() {
            Some(c) if rel.components().count() > 1 => c.as_os_str().to_string_lossy().into_owned(),
            _ => String::new(),
        };
        for cu in scan(&strip_comments(&src)) {
            ix.codeunits.insert(
                cu.0,
                TestCodeunit {
                    id: cu.0,
                    area: area.clone(),
                    file: file.clone(),
                    tests: cu.1,
                },
            );
        }
    }
    Ok(ix)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let p = entry?.path();
        if p.is_dir() {
            walk(&p, out)?;
        } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("al")) {
            out.push(p);
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(root: &Path, rel: &str, src: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, src).unwrap();
    }

    fn corpus() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "text/TestText.al",
            "codeunit 60100 \"Test Text\"\n{\n    Subtype = Test;\n\n    [Test]\n    procedure Text_A_Works()\n    begin\n    end;\n\n    [Test]\n    [HandlerFunctions('H')]\n    procedure Text_B_Works()\n    begin\n    end;\n\n    procedure Helper()\n    begin\n    end;\n}\n",
        );
        write(
            d.path(),
            "record/deep/TestRec.al",
            "// [Test] in a comment is not a test\ncodeunit 60200 TestRec\n{\n    Subtype=Test;\n    [Test]\n    procedure Rec_A()\n    begin\n    end;\n}\n",
        );
        write(
            d.path(),
            "_fixtures/Helper.al",
            "codeunit 60021 Assert\n{\n    procedure IsTrue()\n    begin\n    end;\n}\n",
        );
        d
    }

    #[test]
    fn finds_test_codeunits_and_their_area_folder() {
        let d = corpus();
        let ix = discover(d.path()).unwrap();
        assert_eq!(ix.codeunits.keys().copied().collect::<Vec<_>>(), vec![60100, 60200]);
        assert_eq!(ix.area_of(60100), Some("text"));
        assert_eq!(ix.area_of(60200), Some("record"));
    }

    #[test]
    fn non_test_codeunits_are_not_indexed() {
        let d = corpus();
        assert_eq!(discover(d.path()).unwrap().area_of(60021), None);
    }

    #[test]
    fn lists_test_procedures_including_ones_with_more_attributes() {
        let d = corpus();
        let ix = discover(d.path()).unwrap();
        assert_eq!(ix.codeunits[&60100].tests, vec!["Text_A_Works", "Text_B_Works"]);
        assert_eq!(ix.codeunits[&60200].tests, vec!["Rec_A"]);
    }

    #[test]
    fn lists_every_al_file() {
        let d = corpus();
        assert_eq!(discover(d.path()).unwrap().files.len(), 3);
    }
}
