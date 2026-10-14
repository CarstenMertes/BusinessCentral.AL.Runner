//! The install baseline runs only when a selected test can observe it (`observe.rs`). A pure test
//! in an app that depends on an app with install code starts without the baseline and gets the
//! outcome it gets with it; a test that reads what the install code wrote still sees it.

use al_sema::workspace::AppSource;
use al_testrun::Outcome;
use al_testrun::compile_cache::CompileCache;
use al_testrun::runtime::{CorpusRuntime, Manifest, RunOptions, SuiteRun};

const BASE: &str = r#"
table 50100 Setting { fields { field(1; Code; Code[20]) { } field(2; Value; Text[50]) { } } keys { key(PK; Code) { } } }
codeunit 50101 Installer
{
    Subtype = Install;
    trigger OnInstallAppPerCompany() var S: Record Setting; begin S.Code := 'SEEDED'; S.Value := 'yes'; S.Insert(); end;
}
codeunit 50102 Helper
{
    procedure Pure(A: Integer): Integer begin exit(A + 1); end;
    procedure Seeded(): Boolean var S: Record Setting; begin exit(S.Get('SEEDED')); end;
}
"#;

const MAIN: &str = r#"
codeunit 60101 PureTests
{
    Subtype = Test;
    var H: Codeunit Helper;
    [Test] procedure Adds() begin if H.Pure(1) <> 2 then Error('add'); end;
    [Test] procedure Fails() begin Error('on purpose'); end;
}
codeunit 60102 DataTests
{
    Subtype = Test;
    [Test] procedure SeesTheInstallRow() var S: Record Setting; begin if not S.Get('SEEDED') then Error('no row'); end;
}
codeunit 60103 HelperTests
{
    Subtype = Test;
    var H: Codeunit Helper;
    [Test] procedure SeesTheInstallRowThroughTheHelper() begin if not H.Seeded() then Error('no row'); end;
}
"#;

fn app(id: &str, deps: &[&str], src: &str) -> AppSource {
    AppSource {
        id: id.into(),
        name: id.into(),
        dependencies: deps.iter().map(|d| d.to_string()).collect(),
        files: vec![(format!("{id}.al"), src.into())],
    }
}

fn run(only: &[u32], cache: Option<&std::path::Path>) -> CorpusRuntime {
    let manifest = |p: &str| Manifest {
        publisher: p.into(),
        version: "1.0.0.0".into(),
        resources: vec![],
    };
    CorpusRuntime::try_run_suites_with(
        vec![app("main", &["base"], MAIN), app("base", &[], BASE)],
        vec![al_sema::AppKind::Workspace, al_sema::AppKind::Dependency],
        &[manifest("Me"), manifest("Microsoft")],
        &Default::default(),
        None,
        Default::default(),
        &[SuiteRun::everything()],
        &RunOptions {
            only: (!only.is_empty()).then(|| only.iter().map(|&id| (0, id)).collect()),
            compile_cache: cache.map(|dir| CompileCache {
                dir: dir.to_path_buf(),
                require: false,
            }),
            ..Default::default()
        },
    )
    .expect("the run starts")
}

fn outcome(rt: &CorpusRuntime, test: &str) -> Outcome {
    rt.results()
        .iter()
        .find(|r| r.test == test)
        .unwrap_or_else(|| panic!("{test} did not run"))
        .outcome
        .clone()
}

/// Both compile paths: the demand-driven one and the one with the compile cache.
fn both(only: &[u32], check: impl Fn(&CorpusRuntime)) {
    check(&run(only, None));
    let dir = tempfile::tempdir().unwrap();
    check(&run(only, Some(dir.path())));
    check(&run(only, Some(dir.path())));
}

#[test]
fn a_pure_test_starts_without_the_baseline() {
    both(&[60101], |rt| {
        assert!(rt.phases.baseline_skipped, "{:?}", rt.phases.baseline_why);
        assert_eq!(outcome(rt, "Adds"), Outcome::Pass);
        assert_eq!(outcome(rt, "Fails"), Outcome::Fail("on purpose".into()));
        assert!(
            rt.phases
                .baseline_line()
                .is_some_and(|l| l.starts_with("install baseline: not run, no selected test can observe it")),
            "{:?}",
            rt.phases.baseline_line()
        );
    });
}

#[test]
fn a_test_that_reads_a_table_gets_the_baseline() {
    both(&[60102], |rt| {
        assert!(!rt.phases.baseline_skipped);
        assert!(rt.phases.baseline_why.as_deref().is_some_and(|w| w.contains("record")));
        assert_eq!(outcome(rt, "SeesTheInstallRow"), Outcome::Pass);
    });
}

#[test]
fn a_test_that_reads_a_table_through_a_helper_gets_the_baseline() {
    both(&[60103], |rt| {
        assert!(!rt.phases.baseline_skipped);
        assert!(
            rt.phases
                .baseline_why
                .as_deref()
                .is_some_and(|w| w.contains("Helper.Seeded"))
        );
        assert_eq!(outcome(rt, "SeesTheInstallRowThroughTheHelper"), Outcome::Pass);
    });
}

#[test]
fn one_test_that_needs_the_baseline_keeps_it_for_all() {
    both(&[60101, 60102], |rt| {
        assert!(!rt.phases.baseline_skipped);
        assert_eq!(outcome(rt, "Adds"), Outcome::Pass);
        assert_eq!(outcome(rt, "SeesTheInstallRow"), Outcome::Pass);
    });
}

#[test]
fn an_unselected_run_keeps_the_baseline() {
    both(&[], |rt| {
        assert!(!rt.phases.baseline_skipped);
        assert_eq!(outcome(rt, "SeesTheInstallRow"), Outcome::Pass);
    });
}

#[test]
fn a_pure_selection_does_not_build_or_use_the_compile_cache() {
    let dir = tempfile::tempdir().unwrap();
    let rt = run(&[60101], Some(dir.path()));
    assert!(rt.phases.baseline_skipped && rt.phases.compile_cache_skipped);
    assert!(rt.phases.cache.is_none());
    assert_eq!(al_testrun::compile_cache::list(dir.path()).len(), 0);
    assert_eq!(outcome(&rt, "Adds"), Outcome::Pass);
}

#[test]
fn a_selection_that_needs_the_baseline_still_uses_the_compile_cache() {
    let dir = tempfile::tempdir().unwrap();
    let rt = run(&[60103], Some(dir.path()));
    // The helper's table access is found only after the first compile; the second goes through the cache.
    assert!(!rt.phases.baseline_skipped && !rt.phases.compile_cache_skipped);
    assert!(rt.phases.cache.as_ref().is_some_and(|c| c.built));
    assert_eq!(outcome(&rt, "SeesTheInstallRowThroughTheHelper"), Outcome::Pass);
}
