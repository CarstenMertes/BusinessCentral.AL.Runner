//! A dependency app that is loaded from its index and reads its files on demand
//! (`Workspace::load_apps_lazy`) gives the workspace and the program an eager load gives.

use al_sema::lower::compile_loaded;
use al_sema::workspace::{AppKind, AppSource, FileIndex, LazyDep, SourceLoader, Workspace};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const BASE_A: &str = r#"
namespace Base.Core;
using System.Utilities;
table 50100 Cust { fields { field(1; "No."; Code[20]) { } field(2; Name; Text[50]) { } } keys { key(PK; "No.") { } } }
tableextension 50101 CustExt extends Cust { fields { field(10; Extra; Integer) { } } }
page 50102 CustCard { PageType = Card; SourceTable = Cust; layout { area(Content) { field(N; Rec."No.") { } } } }
interface IGreet { procedure Hello(): Text; }
codeunit 50103 Greeter implements IGreet { procedure Hello(): Text begin exit('dep'); end; }
"#;

const BASE_B: &str = r#"
dotnet { assembly(mscorlib) { type(System.Text.StringBuilder; Builder) { } } }
codeunit 50110 Subs
{
    Subtype = Install;
    EventSubscriberInstance = Manual;
    [EventSubscriber(ObjectType::Table, Database::Cust, 'OnAfterInsertEvent', '', false, false)]
    local procedure OnIns(var Rec: Record Cust) begin end;
    trigger OnInstallAppPerCompany() begin end;
}
report 50111 Rep { dataset { dataitem(C; Cust) { column(No; "No.") { } } } rendering { layout(L1) { Type = RDLC; LayoutFile = 'a.rdlc'; } } }
"#;

const MAIN: &str = r#"
codeunit 60100 Tests
{
    Subtype = Test;
    [Test] procedure Uses() var C: Record Cust; G: Codeunit Greeter; begin C."No." := 'A'; C.Extra := 1; C.Insert(); if G.Hello() <> 'dep' then Error('x'); end;
}
"#;

#[derive(Debug)]
struct Loader {
    texts: Vec<String>,
    reads: Arc<AtomicUsize>,
}

impl SourceLoader for Loader {
    fn text(&self, i: usize) -> String {
        self.reads.fetch_add(1, Ordering::SeqCst);
        self.texts[i].clone()
    }
}

fn app(id: &str, deps: &[&str], files: &[(&str, &str)]) -> AppSource {
    AppSource {
        id: id.into(),
        name: id.into(),
        dependencies: deps.iter().map(|d| d.to_string()).collect(),
        files: files.iter().map(|(p, t)| (p.to_string(), t.to_string())).collect(),
    }
}

fn eager_apps() -> Vec<AppSource> {
    vec![
        app("base", &[], &[("a.al", BASE_A), ("b.al", BASE_B)]),
        app("main", &["base"], &[("m.al", MAIN)]),
    ]
}

fn kinds() -> Vec<AppKind> {
    vec![AppKind::Dependency, AppKind::Workspace]
}

/// The same apps with the dependency's texts left out and an index to go with them.
fn lazy_apps(reads: &Arc<AtomicUsize>) -> (Vec<AppSource>, Vec<Option<LazyDep>>) {
    let eager = Workspace::load_apps(eager_apps(), kinds(), &BTreeSet::new());
    let mut index = eager.export_index();
    // Through bytes, as a cache keeps it.
    let bytes = postcard::to_stdvec(&index[0]).unwrap();
    index[0] = postcard::from_bytes::<Vec<FileIndex>>(&bytes).unwrap();
    let mut apps = eager_apps();
    let loader = Loader {
        texts: apps[0].files.iter().map(|f| f.1.clone()).collect(),
        reads: reads.clone(),
    };
    for f in &mut apps[0].files {
        f.1.clear();
    }
    let lazy = vec![
        Some(LazyDep {
            index: index.swap_remove(0),
            loader: Arc::new(loader),
        }),
        None,
    ];
    (apps, lazy)
}

#[test]
fn the_lazy_load_has_the_objects_and_extensions_of_the_eager_load() {
    let reads = Arc::new(AtomicUsize::new(0));
    let eager = Workspace::load_apps(eager_apps(), kinds(), &BTreeSet::new());
    let (apps, lazy) = lazy_apps(&reads);
    let ws = Workspace::load_apps_lazy(apps, kinds(), &BTreeSet::new(), lazy);
    // The workspace names a dependency table, so resolving that name reads its file; no more.
    assert!(reads.load(Ordering::SeqCst) <= 2);
    assert_eq!(format!("{:?}", eager.objects), format!("{:?}", ws.objects));
    assert_eq!(format!("{:?}", eager.dotnet), format!("{:?}", ws.dotnet));
    for fi in 0..eager.files.len() {
        assert_eq!(eager.files[fi].path, ws.files[fi].path);
        assert_eq!(eager.files[fi].namespace, ws.files[fi].namespace);
        assert_eq!(eager.files[fi].usings, ws.files[fi].usings);
    }
    for i in 0..eager.objects.len() {
        assert_eq!(eager.extensions_of(i), ws.extensions_of(i));
        assert_eq!(eager.extension_target(i), ws.extension_target(i));
        assert_eq!(eager.subscribers_of(i), ws.subscribers_of(i));
    }
    assert_eq!(eager.export_index(), ws.export_index());
}

#[test]
fn the_lazy_load_compiles_to_the_program_of_the_eager_load() {
    let catalog = al_builtins::catalog::Catalog::load();
    let eager = Workspace::load_apps(eager_apps(), kinds(), &BTreeSet::new());
    let (_, a) = compile_loaded(eager, Default::default(), &catalog, Default::default());
    let reads = Arc::new(AtomicUsize::new(0));
    let (apps, lazy) = lazy_apps(&reads);
    let ws = Workspace::load_apps_lazy(apps, kinds(), &BTreeSet::new(), lazy);
    let (_, b) = compile_loaded(ws, Default::default(), &catalog, Default::default());
    assert!(*a.program == *b.program, "the programs differ");
    assert_eq!(a.unsupported, b.unsupported);
    assert!(reads.load(Ordering::SeqCst) > 0, "the files the tests reach are read");
    assert!(
        reads.load(Ordering::SeqCst) <= 2,
        "a file is read once ({})",
        reads.load(Ordering::SeqCst)
    );
}

#[test]
fn a_workspace_that_reaches_nothing_in_the_dependency_reads_none_of_its_files() {
    let catalog = al_builtins::catalog::Catalog::load();
    let reads = Arc::new(AtomicUsize::new(0));
    let (mut apps, lazy) = lazy_apps(&reads);
    apps[1].files[0].1 = "codeunit 60100 T { procedure P() begin end; }".into();
    let ws = Workspace::load_apps_lazy(apps, kinds(), &BTreeSet::new(), lazy);
    let _ = compile_loaded(ws, Default::default(), &catalog, Default::default());
    assert_eq!(reads.load(Ordering::SeqCst), 0);
}
