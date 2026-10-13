//! The corpus as a workspace: the test app plus the internals fixture it depends on.

use al_sema::{AppSource, Workspace};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn al_files(dir: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p: PathBuf = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "al") {
                let rel = p.strip_prefix(dir).unwrap().to_string_lossy().into_owned();
                out.push((rel, std::fs::read_to_string(&p).unwrap()));
            }
        }
    }
    out.sort();
    out
}

fn app(dir: &Path) -> AppSource {
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("app.json")).unwrap()).unwrap();
    AppSource {
        id: manifest["id"].as_str().unwrap().into(),
        name: manifest["name"].as_str().unwrap().into(),
        dependencies: manifest["dependencies"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["id"].as_str().unwrap().to_string())
            .collect(),
        files: al_files(dir),
    }
}

pub fn corpus() -> Workspace {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/tests");
    assert!(
        root.join("al-language/app.json").exists(),
        "corpus submodule not checked out"
    );
    Workspace::load(
        vec![
            app(&root.join("al-language-internals-fixture")),
            app(&root.join("al-language")),
        ],
        &["BC27PLUS".to_string()].into(),
    )
}

#[test]
fn unresolved_references_are_exactly_the_known_external_set() {
    let ws = corpus();
    let mut unresolved: BTreeMap<String, usize> = BTreeMap::new();
    let mut other = Vec::new();
    for d in &ws.diagnostics {
        if d.code == "AR0002" {
            *unresolved.entry(d.message.clone()).or_default() += 1;
        } else {
            other.push(format!("{} {}: {}", d.code, d.file, d.message));
        }
    }
    assert!(other.is_empty(), "{}", other.join("\n"));
    let actual: String = unresolved.iter().map(|(m, n)| format!("{n:4} {m}\n")).collect();
    let golden_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/expected-unresolved.txt");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&golden_path, &actual).unwrap();
    }
    let golden = std::fs::read_to_string(&golden_path).unwrap_or_default();
    assert_eq!(
        actual, golden,
        "unresolved references changed; review and rerun with UPDATE_GOLDEN=1"
    );
}
