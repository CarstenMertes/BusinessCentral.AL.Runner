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
    let members = al_sema::members::build(&ws);
    let checked = al_sema::check::check(&ws, &members, &al_builtins::catalog::Catalog::load());
    let mut unresolved: BTreeMap<String, usize> = BTreeMap::new();
    let mut other = Vec::new();
    for d in ws
        .diagnostics
        .iter()
        .chain(&checked.diagnostics)
        .filter(|d| !d.code.starts_with("AR1"))
    {
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

#[test]
fn every_corpus_table_field_has_a_known_type() {
    let ws = corpus();
    let members = al_sema::members::build(&ws);
    let mut unknown = Vec::new();
    let mut fields = 0;
    for (i, m) in members.iter().enumerate() {
        for f in m.fields.iter().filter(|f| f.declared_in == Some(i)) {
            fields += 1;
            if f.ty == al_sema::types::Ty::Unknown {
                unknown.push(format!("{} :: {}", ws.objects[i].name, f.name));
            }
        }
    }
    assert!(fields > 300, "only {fields} fields found");
    assert!(
        unknown.is_empty(),
        "{} field(s) with unknown type:\n{}",
        unknown.len(),
        unknown.join("\n")
    );
}

/// Milestone 1: every name, member and call in every corpus body resolves,
/// except references into apps outside the workspace (golden list above).
#[test]
fn corpus_bodies_resolve_with_no_model_gaps() {
    let ws = corpus();
    let members = al_sema::members::build(&ws);
    let r = al_sema::check::check(&ws, &members, &al_builtins::catalog::Catalog::load());
    let gaps: Vec<String> = r
        .diagnostics
        .iter()
        .filter(|d| d.code.starts_with("AR1"))
        .map(|d| format!("{} {}: {}", d.code, d.file, d.message))
        .collect();
    assert!(
        gaps.is_empty(),
        "{} model gap(s):\n{}",
        gaps.len(),
        gaps.iter().take(40).cloned().collect::<Vec<_>>().join("\n")
    );
}

/// Prints the resolver's diagnostics over the corpus (run with --nocapture).
#[test]
#[ignore = "exploration aid, not a gate"]
fn explore_body_resolution() {
    let ws = corpus();
    let members = al_sema::members::build(&ws);
    let r = al_sema::check::check(&ws, &members, &al_builtins::catalog::Catalog::load());
    let mut by_msg: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_code: BTreeMap<&str, usize> = BTreeMap::new();
    for d in &r.diagnostics {
        *by_code.entry(d.code).or_default() += 1;
        *by_msg.entry(format!("{} {}", d.code, d.message)).or_default() += 1;
    }
    println!("{by_code:?}");
    let mut v: Vec<_> = by_msg.into_iter().collect();
    v.sort_by_key(|e| std::cmp::Reverse(e.1));
    for (m, n) in v.iter().take(80) {
        println!("{n:5} {m}");
    }
    if let Ok(pat) = std::env::var("SHOW") {
        for d in r.diagnostics.iter().filter(|d| d.message.contains(&pat)).take(6) {
            let f = ws.files.iter().find(|f| f.path == d.file).unwrap();
            let src =
                al_syntax::SyntaxNode::new_root(ws.green(ws.files.iter().position(|g| g.path == f.path).unwrap()))
                    .text()
                    .to_string();
            let line = src[..d.offset].matches('\n').count() + 1;
            let text = src.lines().nth(line - 1).unwrap_or("").trim();
            println!("  {}:{line}: {text}", d.file);
        }
    }
}
