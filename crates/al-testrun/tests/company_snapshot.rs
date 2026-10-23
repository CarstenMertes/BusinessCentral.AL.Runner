//! The company built by a company data codeunit (`--company-data`, Microsoft's Legacy lane) once for every test app of
//! the same dependency apps (`runtime/company.rs`): the second test app reads the company the first one kept, every app
//! starts from its own copy, the outcomes are those of a run that builds it again (`no_company_reuse`), and an app under
//! test that takes part in building it gets a company built with it.

use al_sema::workspace::AppSource;
use al_testrun::Outcome;
use al_testrun::company_data::CompanyData;
use al_testrun::compile_cache::CompileCache;
use al_testrun::runtime::{CorpusRuntime, Manifest, RunOptions, SuiteRun};
use std::path::Path;

const BASE: &str = r#"
table 50100 "Demo Log" { fields { field(1; No; Integer) { } field(2; What; Text[50]) { } } keys { key(PK; No) { } } }
codeunit 50130 "Build Company"
{
    trigger OnRun() var L: Record "Demo Log"; begin L.No := 1; L.What := 'demo'; L.Insert(); OnBuilt(); end;
    [IntegrationEvent(false, false)] procedure OnBuilt() begin end;
}
"#;

/// A test app: its codeunit 60100 lists the log, writes a row of its own (so that a copy shared with the next app
/// would show it), and holds `extra`.
fn test_app(tag: &str, extra: &str) -> String {
    format!(
        r#"
codeunit 60100 "Company Tests"
{{
    Subtype = Test;
    [Test] procedure Log() var L: Record "Demo Log"; T: Text; begin
        L.FindSet(); repeat T += L.What + ';'; until L.Next() = 0;
        if T <> 'demo;' then Error(T);
        L.No := 99; L.What := '{tag} wrote'; L.Insert(); end;
}}
{extra}
"#
    )
}

fn app(id: &str, deps: &[&str], src: String) -> AppSource {
    AppSource {
        id: id.into(),
        name: id.into(),
        dependencies: deps.iter().map(|d| d.to_string()).collect(),
        files: vec![(format!("{id}.al"), src)],
    }
}

fn manifest(p: &str) -> Manifest {
    Manifest {
        publisher: p.into(),
        version: "1.0.0.0".into(),
        ..Default::default()
    }
}

fn run(main_id: &str, main: String, cache: &Path, no_reuse: bool) -> CorpusRuntime {
    CorpusRuntime::try_run_suites_with(
        vec![app(main_id, &["base"], main), app("base", &[], BASE.to_string())],
        vec![al_sema::AppKind::Workspace, al_sema::AppKind::Dependency],
        &[manifest("Me"), manifest("Microsoft")],
        &Default::default(),
        None,
        Default::default(),
        &[SuiteRun::everything()],
        &RunOptions {
            compile_cache: Some(CompileCache {
                dir: cache.to_path_buf(),
                require: false,
                rebuild: false,
                country: String::new(),
            }),
            company_data: Some(CompanyData {
                codeunit: 50130,
                name: "Build Company".into(),
                resources: None,
                identity: "test".into(),
                environment: None,
                // Microsoft's Legacy lane installs its test apps before the company is built, as it does the base apps.
                installed_first: Some(vec!["base".into(), main_id.into()]),
            }),
            no_company_reuse: no_reuse,
            ..Default::default()
        },
    )
    .expect("the run starts")
}

fn outcomes(rt: &CorpusRuntime) -> Vec<(String, Outcome)> {
    let mut v: Vec<_> = rt
        .results()
        .iter()
        .map(|r| (r.test.clone(), r.outcome.clone()))
        .collect();
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v
}

fn company(rt: &CorpusRuntime) -> String {
    rt.phases.company.clone().unwrap_or_default()
}

#[test]
fn test_apps_of_the_same_dependency_apps_start_from_one_company_each_from_its_own_copy() {
    let dir = tempfile::tempdir().unwrap();
    // A test app with tables, a permission set extension and a manual subscriber takes no part in building the company.
    let a = test_app(
        "a",
        r#"table 60101 "A Table" { fields { field(1; K; Code[10]) { } } }
           permissionset 60105 "Base Set" { Permissions = tabledata "Demo Log" = R; }
           permissionsetextension 60106 "A Set Ext" extends "Base Set" { Permissions = tabledata "A Table" = RIMD; }
           codeunit 60102 "A Manual" { EventSubscriberInstance = Manual;
             [EventSubscriber(ObjectType::Codeunit, Codeunit::"Build Company", 'OnBuilt', '', false, false)]
             local procedure OnBuilt() var L: Record "Demo Log"; begin L.No := 2; L.What := 'manual'; L.Insert(); end; }"#,
    );
    let b = test_app("b", "");
    let first = run("a", a.clone(), dir.path(), false);
    assert_eq!(outcomes(&first), vec![("Log".to_string(), Outcome::Pass)]);
    assert!(
        company(&first).starts_with("built for the dependency apps alone, and kept"),
        "{}",
        company(&first)
    );
    let second = run("b", b.clone(), dir.path(), false);
    assert!(
        company(&second).starts_with("read from the company"),
        "{}",
        company(&second)
    );
    // The row the first app wrote is not in the second app's company.
    assert_eq!(outcomes(&second), vec![("Log".to_string(), Outcome::Pass)]);
    let again = run("a", a.clone(), dir.path(), false);
    assert!(
        company(&again).starts_with("read from the company"),
        "{}",
        company(&again)
    );
    assert_eq!(outcomes(&again), outcomes(&first));

    // The differential: with the reuse off the company is built again for every app, with the same outcomes.
    let off = tempfile::tempdir().unwrap();
    for (id, src, with) in [("a", &a, &first), ("b", &b, &second)] {
        let fresh = run(id, src.clone(), off.path(), true);
        assert!(company(&fresh).contains("reuse is off"), "{}", company(&fresh));
        assert_eq!(outcomes(&fresh), outcomes(with), "{id}");
    }
}

#[test]
fn a_test_app_that_takes_part_in_building_the_company_gets_one_built_with_it() {
    let dir = tempfile::tempdir().unwrap();
    // Without a bind, the subscriber runs while the company is built, as it would in BC with the app installed.
    let subscriber = test_app(
        "s",
        r#"codeunit 60102 "Auto" {
             [EventSubscriber(ObjectType::Codeunit, Codeunit::"Build Company", 'OnBuilt', '', false, false)]
             local procedure OnBuilt() var L: Record "Demo Log"; begin L.No := 2; L.What := 'auto'; L.Insert(); end; }"#,
    );
    let rt = run("s", subscriber, dir.path(), false);
    assert!(
        company(&rt).starts_with("built with the apps under test"),
        "{}",
        company(&rt)
    );
    assert!(company(&rt).contains("subscribes to events"), "{}", company(&rt));
    let [(_, Outcome::Fail(why))] = &outcomes(&rt)[..] else {
        panic!("{:?}", outcomes(&rt))
    };
    assert!(why.contains("demo;auto;"), "{why}");
    // An extension object and install code take part too.
    for (extra, why) in [
        (
            r#"tableextension 60103 "Log Ext" extends "Demo Log" { fields { field(60100; Extra; Integer) { } } }"#,
            "extends an object",
        ),
        (
            r#"codeunit 60104 "Inst" { Subtype = Install; trigger OnInstallAppPerCompany() begin end; }"#,
            "is install code",
        ),
    ] {
        let rt = run("x", test_app("x", extra), dir.path(), false);
        assert!(company(&rt).contains(why), "{}", company(&rt));
    }
}
