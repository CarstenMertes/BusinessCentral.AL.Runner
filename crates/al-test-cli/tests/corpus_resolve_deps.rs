//! The corpus as a workspace with its dependency apps (ADR-013): every reference that
//! still does not resolve is in `crates/al-sema/tests/expected-unresolved.txt`, the backlog
//! of the ADR. The list shrinks as dependencies, platform tables and the rest arrive; it
//! never grows without a decision.
//!
//! Needs the `corpus` submodule, including its `.alpackages`.

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
