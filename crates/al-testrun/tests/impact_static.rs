//! Static selection for an impact run (ADR-043 section 2.3, step 5): which tests can reach an impact point of an
//! extension, from the compiled program, and that it covers what a traced run of the same tests recorded.

use al_sema::workspace::AppSource;
use al_testrun::impact_static::{Graph, Verdict, baseline_roots, key_node, select};
use al_testrun::impact_trace::{Matcher, impact_points};
use al_testrun::runtime::{CorpusRuntime, Manifest, RunOptions, SuiteRun};
use std::collections::BTreeMap;

const BASE: &str = r#"
table 50980 "SX Row"
{
    fields { field(1; Id; Integer) { } field(2; Kind; Enum "SX Kind") { } }
    keys { key(PK; Id) { } }
}
table 50981 "SX Other"
{
    fields { field(1; Id; Integer) { } }
    keys { key(PK; Id) { } }
}
enum 50982 "SX Kind"
{
    Extensible = true;
    value(0; None) { }
}
codeunit 50983 "SX Posting"
{
    procedure Post()
    begin
        OnBeforePost();
    end;

    [IntegrationEvent(false, false)]
    local procedure OnBeforePost()
    begin
    end;
}
codeunit 50984 "SX Run Posting"
{
    trigger OnRun()
    var
        P: Codeunit "SX Posting";
    begin
        P.Post();
    end;
}
page 50985 "SX Row Card"
{
    PageType = Card;
    SourceTable = "SX Row";
    layout { area(Content) { field(Id; Rec.Id) { } } }
}
"#;

const TESTS: &str = r#"
codeunit 50986 "SX Tests"
{
    Subtype = Test;

    [Test]
    procedure CallsPost()
    var
        P: Codeunit "SX Posting";
    begin
        P.Post();
    end;

    [Test]
    procedure RunsLiteral()
    begin
        Codeunit.Run(Codeunit::"SX Run Posting");
    end;

    [Test]
    procedure RunsVariable()
    var
        Id: Integer;
    begin
        Id := 50984;
        Codeunit.Run(Id);
    end;

    [Test]
    procedure UsesOther()
    var
        O: Record "SX Other";
    begin
        O.Id := 1;
        O.Insert();
    end;

    [Test]
    procedure UsesRow()
    var
        R: Record "SX Row";
    begin
        if R.Get(1) then;
    end;

    [Test]
    procedure OpensRowRef()
    var
        Ref: RecordRef;
    begin
        Ref.Open(Database::"SX Row");
    end;

    [Test]
    procedure OpensAnyRef()
    var
        Ref: RecordRef;
        N: Integer;
    begin
        N := 50981;
        Ref.Open(N);
    end;

    [Test]
    procedure OpensCard()
    var
        Card: TestPage "SX Row Card";
    begin
        Card.OpenView();
        Card.Close();
    end;

    [Test]
    procedure CastsAKindConstant()
    begin
        if Format(Enum::"SX Kind"::None) = '' then;
    end;

    [Test]
    procedure ReadsKinds()
    var
        K: Enum "SX Kind";
    begin
        if K.Ordinals().Count() = 0 then;
    end;
}
codeunit 50987 "SX Other Tests"
{
    Subtype = Test;

    [Test]
    procedure Nothing()
    begin
    end;
}
"#;

const EXT: &str = r#"
codeunit 50990 "SX Sub"
{
    [EventSubscriber(ObjectType::Codeunit, Codeunit::"SX Posting", 'OnBeforePost', '', false, false)]
    local procedure Check()
    begin
    end;
}
tableextension 50991 "SX Row Ext" extends "SX Row"
{
    fields { field(50991; Extra; Integer) { } }
}
enumextension 50992 "SX Kind Ext" extends "SX Kind"
{
    value(50992; Partner) { }
}
codeunit 50993 "SX Install"
{
    Subtype = Install;
    trigger OnInstallAppPerCompany()
    var
        O: Record "SX Other";
    begin
        O.Id := 9;
        O.Insert();
    end;
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

fn compile(base: &str, with_ext: bool) -> al_sema::lower::Lowered {
    let mut apps = vec![app("base", &[], base), app("tests", &["base"], TESTS)];
    if with_ext {
        apps.push(app("ext", &["base"], EXT));
    }
    let (ws, lowered) = al_sema::lower::compile(apps, &Default::default());
    assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
    lowered
}

/// The test routines of the "tests" app, as (object, routine), with their names.
fn tests_of(lowered: &al_sema::lower::Lowered) -> Vec<((usize, usize), String)> {
    let program = &lowered.program;
    let app = program.apps.iter().position(|a| a.id == "tests").unwrap() as u32;
    let mut out = Vec::new();
    for (oi, o) in program.objects.iter().enumerate().filter(|(_, o)| o.app == app) {
        for (ri, r) in o.routines.iter().enumerate() {
            if r.attributes.iter().any(|a| a.name.eq_ignore_ascii_case("Test")) {
                out.push(((oi, ri), r.name.clone()));
            }
        }
    }
    out
}

/// Per impact point (by the start of its description): test name -> verdict.
fn verdicts(base: &str) -> (Vec<Option<String>>, BTreeMap<String, BTreeMap<String, Verdict>>) {
    let lowered = compile(base, true);
    let program = &lowered.program;
    let ext = program.apps.iter().position(|a| a.id == "ext").unwrap() as u32;
    let base_app = program.apps.iter().position(|a| a.id == "base").unwrap() as u32;
    let points = impact_points(&lowered, |oi| program.objects[oi].app == ext);
    let tests = tests_of(&lowered);
    let units: Vec<(usize, usize)> = tests.iter().map(|(t, _)| *t).collect();
    let graph = Graph::build(&lowered);
    let roots = baseline_roots(program, |oi| program.objects[oi].app == base_app);
    let sel = select(&lowered, &graph, &points, &units, &roots);
    let mut out: BTreeMap<String, BTreeMap<String, Verdict>> = BTreeMap::new();
    for (t, name) in &tests {
        for (pi, v) in sel.verdicts.get(t).into_iter().flatten() {
            out.entry(points[*pi].what.clone())
                .or_default()
                .insert(name.clone(), v.clone());
        }
    }
    (sel.everything, out)
}

fn of<'a>(all: &'a BTreeMap<String, BTreeMap<String, Verdict>>, start: &str) -> Vec<(&'a str, &'a Verdict)> {
    all.iter()
        .find(|(k, _)| k.starts_with(start))
        .map(|(_, m)| m.iter().map(|(k, v)| (k.as_str(), v)).collect())
        .unwrap_or_default()
}

fn cannot(what: &str) -> Verdict {
    Verdict::CannotRuleOut(what.into())
}

#[test]
fn a_subscriber_is_reached_by_calls_and_by_codeunit_run_on_a_constant_and_not_ruled_out_by_an_id_in_a_variable() {
    let (everything, all) = verdicts(BASE);
    assert!(everything.iter().all(Option::is_none), "{everything:?}");
    assert_eq!(
        of(&all, "subscriber \"SX Sub\".Check"),
        [
            ("CallsPost", &Verdict::Reaches),
            ("RunsLiteral", &Verdict::Reaches),
            ("RunsVariable", &cannot("Codeunit.Run with an id from a variable")),
        ],
        "{all:#?}"
    );
}

#[test]
fn a_table_extension_is_reached_through_records_record_refs_and_pages_of_its_table() {
    let (_, all) = verdicts(BASE);
    assert_eq!(
        of(&all, "tableextension \"SX Row Ext\""),
        [
            (
                "OpensAnyRef",
                &cannot("RecordRef.Open with a table number from a variable")
            ),
            ("OpensCard", &Verdict::Reaches),
            ("OpensRowRef", &Verdict::Reaches),
            ("UsesRow", &Verdict::Reaches),
        ],
        "{all:#?}"
    );
}

#[test]
fn an_enum_extension_is_reached_through_variables_of_its_enum_and_fields_of_that_type() {
    let (_, all) = verdicts(BASE);
    assert_eq!(
        of(&all, "enumextension \"SX Kind Ext\""),
        [
            ("CastsAKindConstant", &Verdict::Reaches),
            (
                "OpensAnyRef",
                &cannot("RecordRef.Open with a table number from a variable")
            ),
            ("OpensCard", &Verdict::Reaches),
            ("OpensRowRef", &Verdict::Reaches),
            ("ReadsKinds", &Verdict::Reaches),
            ("UsesRow", &Verdict::Reaches),
        ],
        "{all:#?}"
    );
}

#[test]
fn install_code_selects_the_tests_that_use_a_table_it_writes() {
    let (_, all) = verdicts(BASE);
    assert_eq!(
        of(&all, "codeunit \"SX Install\""),
        [
            (
                "OpensAnyRef",
                &cannot("RecordRef.Open with a table number from a variable")
            ),
            ("UsesOther", &Verdict::Reaches),
        ],
        "{all:#?}"
    );
    // The index selects the same way: tests that used a table the install code can write.
    let lowered = compile(BASE, true);
    let program = &lowered.program;
    let ext = program.apps.iter().position(|a| a.id == "ext").unwrap() as u32;
    let points = impact_points(&lowered, |oi| program.objects[oi].app == ext);
    let install = points.iter().find(|p| p.what.contains("SX Install")).unwrap();
    assert_eq!(install.matcher, Matcher::AnyKey(vec!["table 50981".into()]));
    let kind = points.iter().find(|p| p.what.contains("SX Kind Ext")).unwrap();
    assert_eq!(kind.matcher, Matcher::Key("enum 50982".into()));
}

#[test]
fn a_subscriber_the_install_baseline_can_reach_takes_every_test() {
    let base = format!(
        "{BASE}\ncodeunit 50989 \"SX Base Install\" {{ Subtype = Install; trigger OnInstallAppPerCompany() var P: Codeunit \"SX Posting\"; begin P.Post(); end; }}"
    );
    let (everything, _) = verdicts(&base);
    let why: Vec<&String> = everything.iter().flatten().collect();
    assert_eq!(why.len(), 1, "{everything:?}");
    assert!(why[0].starts_with("subscriber \"SX Sub\".Check"), "{why:?}");
    assert!(why[0].contains("install code"), "{why:?}");
}

/// The run of the tests with the impact trace on, without the extension (what `impact-index build` records).
fn traced() -> CorpusRuntime {
    let manifest = |p: &str| Manifest {
        publisher: p.into(),
        version: "1.0.0.0".into(),
        resources: vec![],
        include_source: false,
        layout_files: Vec::new(),
        symbols: vec![],
    };
    CorpusRuntime::try_run_suites_with(
        vec![app("tests", &["base"], TESTS), app("base", &[], BASE)],
        vec![al_sema::AppKind::Workspace, al_sema::AppKind::Dependency],
        &[manifest("Microsoft"), manifest("Microsoft")],
        &Default::default(),
        None,
        Default::default(),
        &[SuiteRun::everything()],
        &RunOptions {
            impact_trace: true,
            ..Default::default()
        },
    )
    .expect("the run starts")
}

#[test]
fn a_traced_run_notes_reads_of_an_enums_values() {
    let rt = traced();
    let r = rt.results().iter().find(|r| r.test == "ReadsKinds").unwrap();
    let keys: Vec<&String> = r.impact.iter().map(|&k| &rt.impact_keys()[k as usize]).collect();
    assert!(keys.iter().any(|k| *k == "enum 50982"), "{keys:?}");
}

#[test]
fn static_reach_covers_every_key_a_traced_run_recorded() {
    let rt = traced();
    let lowered = compile(BASE, false);
    let graph = Graph::build(&lowered);
    let tests = tests_of(&lowered);
    let mut checked = 0;
    for r in rt.results() {
        let (t, _) = tests.iter().find(|(_, n)| *n == r.test).unwrap();
        let roots: Vec<usize> = al_testrun::impact_static::test_roots(&lowered.program, t.0, t.1)
            .into_iter()
            .map(|(o, r)| graph.routine_node(o, r))
            .collect();
        let reach = graph.reached_from(&roots);
        for &k in &r.impact {
            let key = &rt.impact_keys()[k as usize];
            let node = key_node(&graph, &lowered.program, key).unwrap_or_else(|| panic!("{key}: no node"));
            assert!(reach.possible[node], "{}: {key} is not in its static reach", r.test);
            checked += 1;
        }
    }
    assert!(checked > 10, "{checked}");
}
