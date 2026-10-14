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
        for cu in al_testrun::discover::test_codeunits(&src) {
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
