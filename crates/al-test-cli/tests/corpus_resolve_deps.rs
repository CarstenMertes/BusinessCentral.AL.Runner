//! The corpus as a workspace with its dependency apps (ADR-013): every reference that
//! still does not resolve is in `crates/al-sema/tests/expected-unresolved.txt`, the backlog
//! of the ADR. The list shrinks as dependencies, platform tables and the rest arrive; it
//! never grows without a decision.
//!
//! Needs the `corpus` submodule, including its `.alpackages`. Ignored in a debug build (about 45 s
//! there); CI runs it in the release step:
//! `cargo test --release -p al-test-cli --test corpus_resolve_deps -- --ignored`.

use al_test_cli::{deps, runtime};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

#[test]
#[ignore = "slow unoptimised: cargo test --release -p al-test-cli --test corpus_resolve_deps -- --ignored"]
fn unresolved_references_are_exactly_what_no_dependency_supplies() {
    let tests = root().join("corpus/tests");
    assert!(
        tests.join("al-language/app.json").exists(),
        "corpus submodule not checked out"
    );
    let fixture = tests.join("al-language-internals-fixture");
    let mut apps = vec![
        runtime::load_app(&fixture).unwrap(),
        runtime::load_app(&tests.join("al-language")).unwrap(),
    ];
    let mut kinds = vec![al_sema::AppKind::Workspace; apps.len()];
    let dependencies = deps::load(&tests.join("al-language"), &[fixture], &[]).unwrap();
    assert!(
        dependencies.apps.iter().any(|a| a.name == "Base Application"),
        "the corpus's .alpackages supplies Base Application: {:?}",
        dependencies.log
    );
    kinds.extend(dependencies.apps.iter().map(|_| al_sema::AppKind::Dependency));
    apps.extend(dependencies.apps);
    let ws = al_sema::Workspace::load_apps(apps, kinds, &["BC27PLUS".to_string()].into());
    let members = al_sema::members::build(&ws);
    let checked = al_sema::check::check(&ws, &members, &al_builtins::catalog::Catalog::load());
    // Only the corpus's own files: what Microsoft's code references is Microsoft's business.
    let own: std::collections::HashSet<&str> = ws
        .files
        .iter()
        .filter(|f| ws.kinds[f.app] == al_sema::AppKind::Workspace)
        .map(|f| f.path.as_str())
        .collect();
    let mut unresolved: BTreeMap<String, usize> = BTreeMap::new();
    let mut other = Vec::new();
    for d in ws
        .diagnostics
        .iter()
        .chain(&checked.diagnostics)
        .filter(|d| own.contains(d.file.as_str()) && !d.code.starts_with("AR1"))
    {
        if d.code == "AR0002" {
            *unresolved.entry(d.message.clone()).or_default() += 1;
        } else {
            other.push(format!("{} {}: {}", d.code, d.file, d.message));
        }
    }
    assert!(other.is_empty(), "{}", other.join("\n"));
    let actual: String = unresolved.iter().map(|(m, n)| format!("{n:4} {m}\n")).collect();
    let golden_path = root().join("crates/al-sema/tests/expected-unresolved.txt");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&golden_path, &actual).unwrap();
    }
    let golden = std::fs::read_to_string(&golden_path).unwrap_or_default();
    assert_eq!(
        actual, golden,
        "unresolved references changed; review and rerun with UPDATE_GOLDEN=1"
    );
}

/// `docs/corpus-coverage.md` and `.json` are what `al-test-cli --coverage-map` writes for the pinned
/// corpus (which builtin overloads some corpus test calls). They are committed, so a corpus bump or a
/// catalog change that moves the map shows in the diff. Regenerate with
/// `al-test-cli --root . --coverage-map` (or `UPDATE_GOLDEN=1` on this test).
#[test]
#[ignore = "slow unoptimised: cargo test --release -p al-test-cli --test corpus_resolve_deps -- --ignored"]
fn corpus_coverage_map_is_current() {
    let commit = String::from_utf8(
        std::process::Command::new("git")
            .arg("-C")
            .arg(root().join("corpus"))
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    let map = al_test_cli::coverage::corpus_coverage(&root(), &commit).unwrap();
    for (name, text) in [
        ("docs/corpus-coverage.md", al_test_cli::coverage::to_markdown(&map)),
        ("docs/corpus-coverage.json", al_test_cli::coverage::to_json(&map)),
    ] {
        let path = root().join(name);
        if std::env::var_os("UPDATE_GOLDEN").is_some() {
            std::fs::write(&path, &text).unwrap();
        }
        let committed = std::fs::read_to_string(&path).unwrap_or_default();
        assert!(
            committed == text,
            "{name} is stale; run `al-test-cli --root . --coverage-map` and commit it"
        );
    }
}
