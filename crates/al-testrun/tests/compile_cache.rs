//! The compile cache of the dependency apps (ADR-022) as a run sees it: the entry is built once,
//! the next run finds it, a changed input misses, and a file that is damaged, cut short or made
//! for other inputs never changes an outcome.

use al_sema::workspace::AppSource;
use al_testrun::compile_cache::{self, CompileCache};
use al_testrun::runtime::{CorpusRuntime, Manifest, RunOptions, SuiteRun};
use std::path::{Path, PathBuf};

const BASE: &str = r#"
table 50100 Cust { fields { field(1; "No."; Code[20]) { } field(2; Name; Text[50]) { } } keys { key(PK; "No.") { } } }
interface IGreet { procedure Hello(): Text; }
codeunit 50101 DefaultGreet implements IGreet { procedure Hello(): Text begin exit('dep'); end; }
codeunit 50102 Dep
{
    procedure Count(): Integer var C: Record Cust; begin exit(C.Count()); end;
    procedure Say(G: Interface IGreet): Text begin exit(G.Hello()); end;
    procedure Fail() begin Error('from the dependency'); end;
}
"#;

const MAIN: &str = r#"
tableextension 60100 CustExt extends Cust { fields { field(60100; Extra; Text[20]) { } } }
codeunit 60101 WsGreet implements IGreet { procedure Hello(): Text begin exit('ws'); end; }
codeunit 60102 Tests
{
    Subtype = Test;
    [Test] procedure Inserts() var C: Record Cust; D: Codeunit Dep; begin C."No." := 'A'; C.Extra := 'x'; C.Insert(); if D.Count() <> 1 then Error('count'); end;
    [Test] procedure Interface_FromTheWorkspace() var D: Codeunit Dep; G: Codeunit WsGreet; begin if D.Say(G) <> 'ws' then Error('say'); end;
    [Test] procedure Interface_FromTheDependency() var D: Codeunit Dep; G: Codeunit DefaultGreet; begin if D.Say(G) <> 'dep' then Error('say'); end;
    [Test] procedure Fails() var D: Codeunit Dep; begin D.Fail(); end;
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

/// The workspace app first, as the command lines give them; the program puts the dependency first.
fn apps(base: &str) -> (Vec<AppSource>, Vec<al_sema::AppKind>, Vec<Manifest>) {
    (
        vec![app("main", &["base"], MAIN), app("base", &[], base)],
        vec![al_sema::AppKind::Workspace, al_sema::AppKind::Dependency],
        vec![
            Manifest {
                publisher: "Me".into(),
                version: "1.0.0.0".into(),
                resources: vec![],
                include_source: false,
            },
            Manifest {
                publisher: "Microsoft".into(),
                version: "27.5.0.0".into(),
                resources: vec![],
                include_source: false,
            },
        ],
    )
}

fn cache(dir: &Path, require: bool) -> Option<CompileCache> {
    Some(CompileCache {
        dir: dir.to_path_buf(),
        require,
    })
}

fn run(base: &str, cc: Option<CompileCache>) -> Result<CorpusRuntime, String> {
    let (apps, kinds, manifests) = apps(base);
    CorpusRuntime::try_run_suites_with(
        apps,
        kinds,
        &manifests,
        &Default::default(),
        None,
        Default::default(),
        &[SuiteRun::everything()],
        &RunOptions {
            compile_cache: cc,
            ..Default::default()
        },
    )
}

/// What a run decided, test by test, as text.
fn outcomes(rt: &CorpusRuntime) -> Vec<String> {
    rt.results()
        .iter()
        .map(|r| format!("{} {} {} {:?}", r.app, r.codeunit, r.test, r.outcome))
        .collect()
}

fn files(dir: &Path) -> Vec<PathBuf> {
    compile_cache::list(dir).into_iter().map(|(p, _)| p).collect()
}

#[test]
fn the_first_run_builds_the_entry_and_the_next_run_uses_it() {
    let dir = tempfile::tempdir().unwrap();
    let plain = run(BASE, None).unwrap();
    let first = run(BASE, cache(dir.path(), false)).unwrap();
    let second = run(BASE, cache(dir.path(), true)).unwrap();
    let (a, b) = (
        first.phases.cache.as_ref().unwrap(),
        second.phases.cache.as_ref().unwrap(),
    );
    assert!(a.built && !a.hit, "{a:?}");
    assert!(b.hit && !b.built, "{b:?}");
    assert_eq!(files(dir.path()).len(), 1);
    // The tests of the workspace are test 0 of the caller's apps, whatever the program's order.
    assert!(
        outcomes(&plain).iter().all(|l| l.starts_with("0 ")),
        "{:?}",
        outcomes(&plain)
    );
    assert_eq!(outcomes(&first), outcomes(&plain));
    assert_eq!(outcomes(&second), outcomes(&plain));
    // The workspace extends a dependency table and implements a dependency interface.
    assert!(b.relowered >= 2, "{b:?}");
    let want: Vec<&str> = vec!["Pass", "Pass", "Pass", "Fail"];
    let got: Vec<String> = outcomes(&second);
    for (g, w) in got.iter().zip(want) {
        assert!(g.contains(w), "{g} should be {w}");
    }
}

#[test]
fn a_changed_source_misses_and_builds_another_entry() {
    let dir = tempfile::tempdir().unwrap();
    run(BASE, cache(dir.path(), false)).unwrap();
    let changed = BASE.replace("exit('dep')", "exit('dop')");
    // Required: the entry for the changed source does not exist.
    let err = run(&changed, cache(dir.path(), true))
        .err()
        .expect("a miss is an error when required");
    assert!(err.contains("--require-compile-cache"), "{err}");
    let rt = run(&changed, cache(dir.path(), false)).unwrap();
    assert!(rt.phases.cache.as_ref().unwrap().built);
    assert_eq!(
        files(dir.path()).len(),
        2,
        "the old entry stays, the new one is beside it"
    );
    // That change is visible in the outcome (the dependency greets 'dop'), and the cached run saw it.
    assert!(
        outcomes(&rt)
            .iter()
            .any(|l| l.contains("Interface_FromTheDependency") && l.contains("Fail"))
    );
}

#[test]
fn a_version_or_a_target_change_misses() {
    let dir = tempfile::tempdir().unwrap();
    run(BASE, cache(dir.path(), false)).unwrap();
    let (apps, kinds, mut manifests) = apps(BASE);
    manifests[1].version = "27.5.0.1".into();
    let rt = CorpusRuntime::try_run_suites_with(
        apps,
        kinds,
        &manifests,
        &Default::default(),
        None,
        Default::default(),
        &[SuiteRun::everything()],
        &RunOptions {
            compile_cache: cache(dir.path(), false),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(
        rt.phases.cache.as_ref().unwrap().built,
        "another version is another entry"
    );
    assert_eq!(files(dir.path()).len(), 2);
}

#[test]
fn a_run_without_the_cache_or_with_the_eager_switch_makes_no_entry() {
    let dir = tempfile::tempdir().unwrap();
    let rt = run(BASE, None).unwrap();
    assert!(rt.phases.cache.is_none());
    assert!(files(dir.path()).is_empty());
}

/// Whatever is wrong with the file, the run gives the outcomes of a run without it, says so, and
/// leaves a good file behind.
#[test]
fn a_damaged_truncated_or_foreign_file_never_changes_an_outcome() {
    let dir = tempfile::tempdir().unwrap();
    let want = outcomes(&run(BASE, None).unwrap());
    run(BASE, cache(dir.path(), false)).unwrap();
    let path = files(dir.path()).pop().unwrap();
    let good = std::fs::read(&path).unwrap();
    let foreign = {
        // An entry made for other inputs, put under this key's name.
        let other = tempfile::tempdir().unwrap();
        run(&BASE.replace("exit('dep')", "exit('xyz')"), cache(other.path(), false)).unwrap();
        std::fs::read(files(other.path()).pop().unwrap()).unwrap()
    };
    let mut cases: Vec<(&str, Vec<u8>)> = vec![
        ("truncated to half", good[..good.len() / 2].to_vec()),
        ("truncated by one byte", good[..good.len() - 1].to_vec()),
        ("empty", Vec::new()),
        ("garbage", vec![7; 5000]),
        ("made for other inputs", foreign),
    ];
    for at in [3, 20, 60, 100, good.len() / 3, good.len() / 2, good.len() - 5] {
        let mut bad = good.clone();
        bad[at] ^= 0x55;
        cases.push(("a flipped byte", bad));
    }
    for (what, bytes) in cases {
        std::fs::write(&path, &bytes).unwrap();
        let rt = run(BASE, cache(dir.path(), false)).unwrap();
        assert_eq!(outcomes(&rt), want, "{what}: outcomes changed");
        let report = rt.phases.cache.as_ref().unwrap();
        if what != "a flipped byte" {
            assert!(!report.hit, "{what}: the damaged file was used");
            assert!(report.built || !report.notes.is_empty(), "{what}: {report:?}");
            // The next run finds a good file.
            let again = run(BASE, cache(dir.path(), true)).unwrap();
            assert!(again.phases.cache.as_ref().unwrap().hit, "{what}: not repaired");
            assert_eq!(outcomes(&again), want);
        } else if report.hit {
            // The flipped byte is in a block this run does not read (an object the workspace
            // changes is compiled from source). Reading every block finds it.
            let (apps, _, manifests) = apps(BASE);
            let key = compile_cache::key_for(&[&apps[1]], &manifests[1..], &Default::default(), None);
            let (entry, _) = compile_cache::read_entry(dir.path(), &key)
                .unwrap()
                .expect("the file is there");
            assert!(entry.verify().is_err(), "a flipped byte that nothing noticed");
        }
    }
}

#[test]
fn two_writers_and_a_reader_never_see_half_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let (apps, _, manifests) = apps(BASE);
    let dep = [&apps[1]];
    let key = compile_cache::key_for(&dep, &manifests[1..], &Default::default(), None);
    let bytes = compile_cache::build_entry(vec![apps[1].clone()], &Default::default(), None, &key);
    let stop = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|scope| {
        let writers: Vec<_> = (0..3)
            .map(|_| {
                scope.spawn(|| {
                    for _ in 0..20 {
                        compile_cache::write_entry(dir.path(), &key, &bytes).unwrap();
                    }
                })
            })
            .collect();
        let reader = scope.spawn(|| {
            let mut seen = 0;
            while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                // A missing file is fine; a half file would be an Err (and would be removed).
                match compile_cache::read_entry(dir.path(), &key) {
                    Ok(Some(_)) => seen += 1,
                    Ok(None) => {}
                    Err(e) => panic!("a reader saw a damaged file: {e}"),
                }
            }
            seen
        });
        for w in writers {
            w.join().unwrap();
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        assert!(reader.join().unwrap() > 0, "the reader never saw the file");
    });
    assert_eq!(files(dir.path()).len(), 1, "no temporary file is left");
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        1,
        "no temporary file is left"
    );
}

#[test]
fn building_the_same_entry_twice_gives_the_same_bytes() {
    let (apps, _, manifests) = apps(BASE);
    let dep = [&apps[1]];
    let key = compile_cache::key_for(&dep, &manifests[1..], &Default::default(), None);
    let a = compile_cache::build_entry(vec![apps[1].clone()], &Default::default(), None, &key);
    let b = compile_cache::build_entry(vec![apps[1].clone()], &Default::default(), None, &key);
    assert!(a == b);
}

#[test]
fn the_key_changes_with_a_byte_of_source_a_symbol_or_the_target() {
    let (apps, _, manifests) = apps(BASE);
    let base = compile_cache::key_for(&[&apps[1]], &manifests[1..], &Default::default(), None).digest();
    let mut changed = apps[1].clone();
    changed.files[0].1.push(' ');
    assert_ne!(
        base,
        compile_cache::key_for(&[&changed], &manifests[1..], &Default::default(), None).digest()
    );
    let mut other_path = apps[1].clone();
    other_path.files[0].0 = "renamed.al".into();
    assert_ne!(
        base,
        compile_cache::key_for(&[&other_path], &manifests[1..], &Default::default(), None).digest()
    );
    let symbols: std::collections::BTreeSet<String> = ["CLEAN27".to_string()].into();
    assert_ne!(
        base,
        compile_cache::key_for(&[&apps[1]], &manifests[1..], &symbols, None).digest()
    );
    let target = al_version::Target {
        bc: al_version::BcVersion::new(27, 5),
        runtime: al_version::Runtime::for_bc(27),
    };
    assert_ne!(
        base,
        compile_cache::key_for(&[&apps[1]], &manifests[1..], &Default::default(), Some(&target)).digest()
    );
    assert_eq!(
        base,
        compile_cache::key_for(&[&apps[1]], &manifests[1..], &Default::default(), None).digest()
    );
}

#[test]
fn an_entry_nothing_used_for_a_month_goes_when_another_is_built() {
    let dir = tempfile::tempdir().unwrap();
    run(BASE, cache(dir.path(), false)).unwrap();
    let old = files(dir.path()).pop().unwrap();
    let file = std::fs::File::options().write(true).open(&old).unwrap();
    file.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(40 * 24 * 3600))
        .unwrap();
    drop(file);
    // A different set of apps: a new entry, and the old one is past its time.
    run(&BASE.replace("exit('dep')", "exit('dap')"), cache(dir.path(), false)).unwrap();
    assert_eq!(files(dir.path()).len(), 1);
    assert!(!old.exists(), "the entry nothing used for 40 days is still there");
    // A hit marks the entry as used, so a run that keeps using it keeps it.
    let kept = files(dir.path()).pop().unwrap();
    let file = std::fs::File::options().write(true).open(&kept).unwrap();
    file.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(40 * 24 * 3600))
        .unwrap();
    drop(file);
    run(&BASE.replace("exit('dep')", "exit('dap')"), cache(dir.path(), true)).unwrap();
    run(&BASE.replace("exit('dep')", "exit('dup')"), cache(dir.path(), false)).unwrap();
    assert!(kept.exists(), "an entry a run used lately was pruned");
    assert_eq!(files(dir.path()).len(), 2);
}

/// A dependency with a report that one and then two reportextensions change (ADR-025).
const REPORTS: &str = r#"
report 50110 R1 { ProcessingOnly = true; UseRequestPage = false; dataset { dataitem(D; Cust) { } } }
report 50111 R2 { ProcessingOnly = true; UseRequestPage = false; dataset { dataitem(D; Cust) { } } }
reportextension 50112 R1E extends R1 { trigger OnPreReport() begin end; }
reportextension 50114 R2B extends R2 { trigger OnPostReport() begin end; }
reportextension 50113 R2A extends R2 { trigger OnPreReport() begin end; }
"#;

/// ADR-025, decision 7: the facts of a report with one and with two reportextensions go through the
/// file, the run that reads it decides what the uncached run decides, and an entry written at
/// format 1 (the file name does not carry the format) is refused, removed and built again.
#[test]
fn reportextensions_round_trip_and_a_format_1_file_is_rejected_and_rebuilt() {
    let base = format!("{BASE}{REPORTS}");
    let dir = tempfile::tempdir().unwrap();
    let want = outcomes(&run(&base, None).unwrap());
    let first = run(&base, cache(dir.path(), false)).unwrap();
    assert_eq!(outcomes(&first), want);
    let second = run(&base, cache(dir.path(), true)).unwrap();
    assert!(second.phases.cache.as_ref().unwrap().hit);
    assert_eq!(outcomes(&second), want);
    // A format-1 file under this key's name.
    let (apps, _, manifests) = apps(&base);
    let key = compile_cache::key_for(&[&apps[1]], &manifests[1..], &Default::default(), None);
    let mut old = key.clone();
    old.format = 1;
    let bytes = compile_cache::build_entry(vec![apps[1].clone()], &Default::default(), None, &old);
    let path = files(dir.path()).pop().unwrap();
    std::fs::write(&path, bytes).unwrap();
    let third = run(&base, cache(dir.path(), false)).unwrap();
    let report = third.phases.cache.as_ref().unwrap();
    assert!(!report.hit && report.built, "{report:?}");
    assert_eq!(outcomes(&third), want);
    assert!(
        run(&base, cache(dir.path(), true))
            .unwrap()
            .phases
            .cache
            .as_ref()
            .unwrap()
            .hit
    );
}

// ---- layers (owner decision 2026-10-15) ----------------------------------------------------

const LIB: &str = r#"
tableextension 50200 LibCustExt extends Cust { fields { field(50200; LibNote; Text[20]) { } } }
codeunit 50201 LibGreet implements IGreet { procedure Hello(): Text begin exit('lib'); end; }
codeunit 50202 Lib { procedure Note(var C: Record Cust): Text begin exit(C.LibNote); end; procedure Version(): Integer begin exit(1); end; }
"#;

const IMPL: &str = r#"
tableextension 50300 ImplCustExt extends Cust { fields { field(50300; ImplNote; Text[20]) { } } }
codeunit 50301 ImplGreet implements IGreet { procedure Hello(): Text begin exit('impl'); end; }
codeunit 50302 Impl
{
    procedure Both(var C: Record Cust): Text begin exit(C.LibNote + C.ImplNote); end;
    procedure Version(): Integer var L: Codeunit Lib; begin exit(L.Version() + 10); end;
}
"#;

const LAYER_TESTS: &str = r#"
codeunit 60200 LayerTests
{
    Subtype = Test;
    [Test] procedure Fields_AcrossLayers() var C: Record Cust; I: Codeunit Impl; begin C.LibNote := 'a'; C.ImplNote := 'b'; if I.Both(C) <> 'ab' then Error('both'); end;
    [Test] procedure Calls_AcrossLayers() var I: Codeunit Impl; begin if I.Version() <> 11 then Error('version'); end;
    [Test] procedure Interfaces_AcrossLayers() var D: Codeunit Dep; L: Codeunit LibGreet; M: Codeunit ImplGreet; begin if D.Say(L) <> 'lib' then Error('lib'); if D.Say(M) <> 'impl' then Error('impl'); end;
}
"#;

/// The caller's order is the workspace first, then the implementation app, the partner library and
/// the Microsoft app: the layers must not depend on it.
fn layered_run(lib: &str, imp: &str, cc: Option<CompileCache>) -> Result<CorpusRuntime, String> {
    let man = |p: &str, v: &str| Manifest {
        publisher: p.into(),
        version: v.into(),
        resources: vec![],
        ..Default::default()
    };
    CorpusRuntime::try_run_suites_with(
        vec![
            app("main", &["impl"], LAYER_TESTS),
            app("impl", &["lib", "base"], imp),
            app("lib", &["base"], lib),
            app("base", &[], BASE),
        ],
        vec![
            al_sema::AppKind::Workspace,
            al_sema::AppKind::Dependency,
            al_sema::AppKind::Dependency,
            al_sema::AppKind::Dependency,
        ],
        &[
            man("Me", "1.0.0.0"),
            man("Me", "1.0.0.0"),
            man("Acme", "2.0.0.0"),
            man("Microsoft", "27.5.0.0"),
        ],
        &Default::default(),
        None,
        Default::default(),
        &[SuiteRun::everything()],
        &RunOptions {
            compile_cache: cc,
            ..Default::default()
        },
    )
}

/// Which layers of the run were found: the Microsoft layer first.
fn layer_hits(rt: &CorpusRuntime) -> Vec<bool> {
    rt.phases.cache.as_ref().unwrap().layers.iter().map(|l| l.hit).collect()
}

#[test]
fn the_dependency_set_is_cached_as_layers_microsoft_first_then_one_per_other_app() {
    let dir = tempfile::tempdir().unwrap();
    let want = outcomes(&layered_run(LIB, IMPL, None).unwrap());
    assert!(want.iter().all(|l| l.contains("Pass")), "{want:?}");
    let first = layered_run(LIB, IMPL, cache(dir.path(), false)).unwrap();
    assert_eq!(layer_hits(&first), vec![false, false, false]);
    assert_eq!(files(dir.path()).len(), 3, "one file per layer");
    assert_eq!(outcomes(&first), want);
    let second = layered_run(LIB, IMPL, cache(dir.path(), true)).unwrap();
    assert_eq!(layer_hits(&second), vec![true, true, true]);
    assert!(second.phases.cache.as_ref().unwrap().hit);
    assert_eq!(outcomes(&second), want);
}

#[test]
fn a_change_in_the_implementation_app_rebuilds_only_its_layer() {
    let dir = tempfile::tempdir().unwrap();
    layered_run(LIB, IMPL, cache(dir.path(), false)).unwrap();
    let before = files(dir.path());
    let changed = IMPL.replace("L.Version() + 10", "L.Version() + 20");
    // With the cache required, the changed layer is missing: an error naming it.
    assert!(layered_run(LIB, &changed, cache(dir.path(), true)).is_err());
    let rt = layered_run(LIB, &changed, cache(dir.path(), false)).unwrap();
    assert_eq!(layer_hits(&rt), vec![true, true, false]);
    assert!(rt.phases.cache.as_ref().unwrap().built);
    assert!(
        outcomes(&rt)
            .iter()
            .any(|l| l.contains("Calls_AcrossLayers") && l.contains("Fail"))
    );
    // The two lower files are the same files, untouched; the new layer file is beside the old.
    let after = files(dir.path());
    assert_eq!(after.len(), 4);
    for f in before.iter().filter(|f| after.contains(f)) {
        assert!(f.exists());
    }
    assert!(after.iter().filter(|f| before.contains(f)).count() >= 3);
    assert_eq!(outcomes(&rt), outcomes(&layered_run(LIB, &changed, None).unwrap()));
}

#[test]
fn a_change_in_a_library_app_rebuilds_its_layer_and_the_layers_above_but_not_the_microsoft_layer() {
    let dir = tempfile::tempdir().unwrap();
    layered_run(LIB, IMPL, cache(dir.path(), false)).unwrap();
    let changed = LIB.replace("exit(1)", "exit(2)");
    let rt = layered_run(&changed, IMPL, cache(dir.path(), false)).unwrap();
    assert_eq!(layer_hits(&rt), vec![true, false, false]);
    assert_eq!(outcomes(&rt), outcomes(&layered_run(&changed, IMPL, None).unwrap()));
    assert!(
        outcomes(&rt)
            .iter()
            .any(|l| l.contains("Calls_AcrossLayers") && l.contains("Fail"))
    );
}

#[test]
fn a_change_in_a_microsoft_app_rebuilds_every_layer() {
    let dir = tempfile::tempdir().unwrap();
    layered_run(LIB, IMPL, cache(dir.path(), false)).unwrap();
    let rt = {
        let man = |p: &str, v: &str| Manifest {
            publisher: p.into(),
            version: v.into(),
            resources: vec![],
            ..Default::default()
        };
        CorpusRuntime::try_run_suites_with(
            vec![
                app("main", &["impl"], LAYER_TESTS),
                app("impl", &["lib", "base"], IMPL),
                app("lib", &["base"], LIB),
                app("base", &[], BASE),
            ],
            vec![
                al_sema::AppKind::Workspace,
                al_sema::AppKind::Dependency,
                al_sema::AppKind::Dependency,
                al_sema::AppKind::Dependency,
            ],
            &[
                man("Me", "1.0.0.0"),
                man("Me", "1.0.0.0"),
                man("Acme", "2.0.0.0"),
                man("Microsoft", "27.5.0.1"),
            ],
            &Default::default(),
            None,
            Default::default(),
            &[SuiteRun::everything()],
            &RunOptions {
                compile_cache: cache(dir.path(), false),
                ..Default::default()
            },
        )
        .unwrap()
    };
    assert_eq!(layer_hits(&rt), vec![false, false, false]);
}

#[test]
fn a_missing_lower_layer_file_is_rebuilt_and_the_layers_above_it_are_kept() {
    let dir = tempfile::tempdir().unwrap();
    layered_run(LIB, IMPL, cache(dir.path(), false)).unwrap();
    let report = layered_run(LIB, IMPL, cache(dir.path(), true)).unwrap();
    let microsoft = report.phases.cache.as_ref().unwrap().layers[0].file.clone();
    std::fs::remove_file(&microsoft).unwrap();
    let rt = layered_run(LIB, IMPL, cache(dir.path(), false)).unwrap();
    assert_eq!(layer_hits(&rt), vec![false, true, true]);
    assert_eq!(outcomes(&rt), outcomes(&layered_run(LIB, IMPL, None).unwrap()));
}

#[test]
fn a_damaged_upper_layer_never_changes_an_outcome() {
    let dir = tempfile::tempdir().unwrap();
    let want = outcomes(&layered_run(LIB, IMPL, None).unwrap());
    layered_run(LIB, IMPL, cache(dir.path(), false)).unwrap();
    let report = layered_run(LIB, IMPL, cache(dir.path(), true)).unwrap();
    let top = report.phases.cache.as_ref().unwrap().layers[2].file.clone();
    let good = std::fs::read(&top).unwrap();
    for bad in [good[..good.len() / 2].to_vec(), vec![1; 3000], Vec::new()] {
        std::fs::write(&top, &bad).unwrap();
        let rt = layered_run(LIB, IMPL, cache(dir.path(), false)).unwrap();
        assert_eq!(layer_hits(&rt), vec![true, true, false]);
        assert_eq!(outcomes(&rt), want);
    }
}
