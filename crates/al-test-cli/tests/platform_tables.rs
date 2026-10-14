//! Platform tables (ADR-013 decision 7, ADR-019): the registry of native providers is bound to
//! the real declarations in the platform `System` app, and a provider never stands in for AL
//! that exists.
//!
//! Needs the `corpus` submodule, including its `.alpackages` (`System.app` and the Microsoft apps).

use al_builtins::platform::{REGISTRY, Provider};
use al_deps::package::read_package;
use al_syntax::{ast, scan::scan};
use al_test_cli::report::Outcome;
use al_test_cli::runtime::run_codeunit;
use al_vm::Engine;
use std::path::{Path, PathBuf};

fn packages() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/tests/al-language/.alpackages");
    assert!(
        dir.join("System.app").exists(),
        "corpus submodule not checked out: run `git submodule update --init`"
    );
    dir
}

fn system_app_files() -> Vec<(String, String)> {
    let bytes = std::fs::read(packages().join("System.app")).unwrap();
    read_package(&bytes).expect("System.app reads").1
}

fn symbols() -> std::collections::BTreeSet<String> {
    ["BC27PLUS".to_string()].into()
}

#[test]
fn every_registry_entry_is_a_bodiless_table_of_the_system_app_with_that_id_and_name() {
    let files = system_app_files();
    let syms = symbols();
    for (id, name, provider) in REGISTRY {
        let mut found = Vec::new();
        for (path, src) in &files {
            if !src.contains(&id.to_string()) {
                continue;
            }
            let parse = al_syntax::parse(src, &syms);
            let root = ast::SourceFile::cast(parse.syntax()).unwrap();
            for o in root.objects() {
                if o.kind() == "table" && o.id() == Some(*id) {
                    found.push((path.clone(), o));
                }
            }
        }
        assert_eq!(found.len(), 1, "{name} ({id}) must be declared once in System.app: {found:?}", found = found.iter().map(|f| &f.0).collect::<Vec<_>>());
        let (path, o) = &found[0];
        assert!(o.name().eq_ignore_ascii_case(name), "{path}: the registry names {id} `{name}`, the declaration `{}`", o.name());
        // A platform table has no AL body: the platform has the behaviour (ADR-013 decision 7).
        assert!(
            ast::routines_in(o.syntax()).is_empty(),
            "{path}: {name} has code, so it is AL that exists and may not be replaced natively ({provider:?})"
        );
    }
}

#[test]
#[ignore = "scans 9500 files: cargo test --release -p al-test-cli --test platform_tables -- --ignored"]
fn no_other_app_supplies_a_table_under_a_registered_id_or_name() {
    let syms = symbols();
    for entry in std::fs::read_dir(packages()).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "app") || path.file_name().is_some_and(|n| n == "System.app") {
            continue;
        }
        let Ok((_, files)) = read_package(&std::fs::read(&path).unwrap()) else { continue };
        for (file, src) in files {
            for o in scan(&src, &syms).objects.iter().filter(|o| o.kind == "table") {
                for (id, name, _) in REGISTRY {
                    assert!(
                        o.id != Some(*id) && !o.name.eq_ignore_ascii_case(name),
                        "{}: {file} declares table `{}` ({:?}), which the platform registry owns ({name}, {id})",
                        path.display(),
                        o.name,
                        o.id
                    );
                }
            }
        }
    }
}

/// Outcomes of the test codeunit in `main`, compiled over the real platform `System` app.
fn outcomes(main: &str) -> Vec<(String, Outcome)> {
    use al_sema::workspace::{AppKind, AppSource};
    let app = |id: &str, name: &str, deps: &[&str], files: Vec<(String, String)>| AppSource {
        id: id.into(),
        name: name.into(),
        dependencies: deps.iter().map(|d| d.to_string()).collect(),
        files,
    };
    let (_, l) = al_sema::lower::compile_apps(
        vec![
            app("main", "main", &[], vec![("main.al".into(), main.into())]),
            app(al_sema::workspace::PLATFORM_APP_ID, "System", &[], system_app_files()),
        ],
        vec![AppKind::Workspace, AppKind::Dependency],
        &symbols(),
    );
    let blocked = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let engine = Engine::new(l.program.clone(), blocked);
    let oi = l
        .program
        .objects
        .iter()
        .position(|o| o.name == "T")
        .expect("test codeunit T");
    run_codeunit(&engine, &l, oi)
}

fn not_run(o: &[(String, Outcome)], test: &str) -> String {
    match o.iter().find(|(n, _)| n == test).map(|(_, o)| o) {
        Some(Outcome::NotRun(why)) => why.clone(),
        other => panic!("{test} was expected to be refused, got {other:?}"),
    }
}

#[test]
fn a_platform_table_with_no_provider_is_refused_where_code_uses_it_and_names_table_and_id() {
    let o = outcomes(
        r#"codeunit 50000 "T" { Subtype = Test;
  [Test] procedure UsesATableWithNoProvider() var P: Record "Aggregate Permission Set"; begin if P.IsEmpty() then; end; }"#,
    );
    let why = not_run(&o, "UsesATableWithNoProvider");
    assert!(
        why.contains("platform table 'Aggregate Permission Set' (2000000167) has no provider"),
        "{why}"
    );
}

#[test]
fn a_virtual_platform_table_is_read_but_cannot_be_written() {
    let o = outcomes(
        r#"codeunit 50000 "T" { Subtype = Test;
  [Test] procedure ReadsTheIntegerTable() var I: Record Integer; begin I.SetRange(Number, 3, 5); if I.Count() <> 3 then Error('count %1', I.Count()); end;
  [Test] procedure WritesTheIntegerTable() var I: Record Integer; begin I.Number := 7; I.Insert(); end; }"#,
    );
    assert!(o.iter().any(|(n, o)| n == "ReadsTheIntegerTable" && *o == Outcome::Pass), "{o:?}");
    let why = not_run(&o, "WritesTheIntegerTable");
    assert!(why.contains("virtual table 'Integer'"), "{why}");
}

#[test]
fn a_column_nothing_pins_is_refused_where_code_reads_it() {
    let o = outcomes(
        r#"codeunit 50000 "T" { Subtype = Test;
  [Test] procedure ReadsAPinnedColumn() var F: Record Field; begin F.SetRange(TableNo, 50000); if F.IsEmpty() then; end;
  [Test] procedure ReadsAnUnpinnedColumn() var F: Record Field; L: Integer; begin L := F.Len; end; }"#,
    );
    let why = not_run(&o, "ReadsAnUnpinnedColumn");
    assert!(why.contains("column 6 of platform table 'Field'"), "{why}");
    assert!(!matches!(o.iter().find(|(n, _)| n == "ReadsAPinnedColumn").map(|(_, o)| o), Some(Outcome::NotRun(w)) if w.contains("column")), "{o:?}");
}

#[test]
fn registry_providers_are_virtual_or_stored() {
    for (_, name, p) in REGISTRY {
        assert!(matches!(p, Provider::Virtual(_) | Provider::Stored), "{name}");
    }
}
