//! Platform tables (ADR-013 decision 7, ADR-019): the registry of native providers is bound to
//! the real declarations in the platform `System` app, and a provider never stands in for AL
//! that exists.
//!
//! Needs the `corpus` submodule, including its `.alpackages` (`System.app` and the Microsoft apps).

use al_builtins::platform::{Provider, REGISTRY};
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
        assert_eq!(
            found.len(),
            1,
            "{name} ({id}) must be declared once in System.app: {found:?}",
            found = found.iter().map(|f| &f.0).collect::<Vec<_>>()
        );
        let (path, o) = &found[0];
        assert!(
            o.name().eq_ignore_ascii_case(name),
            "{path}: the registry names {id} `{name}`, the declaration `{}`",
            o.name()
        );
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
        let Ok((_, files)) = read_package(&std::fs::read(&path).unwrap()) else {
            continue;
        };
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
    outcomes_with(main, al_sema::check::Roots::default())
}

/// [`outcomes`] with the demand roots chosen: `eager` compiles every table of the System app.
fn outcomes_with(main: &str, roots: al_sema::check::Roots) -> Vec<(String, Outcome)> {
    use al_sema::workspace::{AppKind, AppSource};
    let app = |id: &str, name: &str, deps: &[&str], files: Vec<(String, String)>| AppSource {
        id: id.into(),
        name: name.into(),
        dependencies: deps.iter().map(|d| d.to_string()).collect(),
        files,
    };
    let (_, l) = al_sema::lower::compile_apps_with(
        vec![
            app("main", "main", &[], vec![("main.al".into(), main.into())]),
            app(al_sema::workspace::PLATFORM_APP_ID, "System", &[], system_app_files()),
        ],
        vec![AppKind::Workspace, AppKind::Dependency],
        &symbols(),
        None,
        roots,
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
  [Test] procedure UsesATableWithNoProvider() var P: Record "Feature Key"; begin if P.IsEmpty() then; end; }"#,
    );
    let why = not_run(&o, "UsesATableWithNoProvider");
    assert!(
        why.contains("platform table 'Feature Key' (2000000211) has no provider"),
        "{why}"
    );
}

// A name that ends in a dot (`Metadata Permission Set Rel.`, 23 of Microsoft's permission tests) is a
// platform table with no provider, not an object outside the workspace.
#[test]
fn a_platform_table_whose_name_ends_in_a_dot_names_the_missing_provider() {
    let o = outcomes(
        r#"codeunit 50000 "T" { Subtype = Test;
  [Test] procedure UsesIt() var P: Record "Metadata Permission Set Rel."; begin if P.IsEmpty() then; end; }"#,
    );
    let why = not_run(&o, "UsesIt");
    assert!(
        why.contains("platform table 'Metadata Permission Set Rel.' (") && why.contains("has no provider"),
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
    assert!(
        o.iter()
            .any(|(n, o)| n == "ReadsTheIntegerTable" && *o == Outcome::Pass),
        "{o:?}"
    );
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
    assert!(
        !matches!(o.iter().find(|(n, _)| n == "ReadsAPinnedColumn").map(|(_, o)| o), Some(Outcome::NotRun(w)) if w.contains("column")),
        "{o:?}"
    );
}

#[test]
fn registry_providers_are_virtual_or_stored() {
    for (_, name, p) in REGISTRY {
        assert!(matches!(p, Provider::Virtual(_) | Provider::Stored), "{name}");
    }
}

fn passes(o: &[(String, Outcome)], test: &str) {
    match o.iter().find(|(n, _)| n == test).map(|(_, o)| o) {
        Some(Outcome::Pass) => {}
        other => panic!("{test} was expected to pass, got {other:?}"),
    }
}

#[test]
fn the_session_starts_with_its_company_user_and_super_row() {
    let o = outcomes(
        r#"codeunit 50000 "T" { Subtype = Test;
  [Test] procedure CompanyRow() var C: Record Company; begin if not C.Get(CompanyName()) then Error('no company row'); end;
  [Test] procedure UserRow() var U: Record User; begin if not U.Get(UserSecurityId()) then Error('no user row'); if U."User Name" <> UserId() then Error('name %1', U."User Name"); end;
  [Test] procedure SuperRow() var A: Record "Access Control"; begin A.SetRange("User Security ID", UserSecurityId()); A.SetRange("Role ID", 'SUPER'); if A.IsEmpty() then Error('no SUPER'); end; }"#,
    );
    for t in ["CompanyRow", "UserRow", "SuperRow"] {
        passes(&o, t);
    }
}

#[test]
fn record_links_are_rows_of_the_record_link_table() {
    let o = outcomes(
        r#"table 50001 "Host" { fields { field(1; "No."; Integer) { } } keys { key(PK; "No.") { Clustered = true; } } }
codeunit 50000 "T" { Subtype = Test;
  [Test] procedure AddHasDelete() var H: Record "Host"; L: Record "Record Link"; Id: Integer; begin
    H."No." := 1; H.Insert();
    if H.HasLinks() then Error('links before');
    Id := H.AddLink('https://example.com', 'D');
    if not H.HasLinks() then Error('no links after AddLink');
    L.Get(Id);
    if L.URL1 <> 'https://example.com' then Error('url');
    H.DeleteLinks();
    if H.HasLinks() then Error('links after DeleteLinks');
  end; }"#,
    );
    passes(&o, "AddHasDelete");
}

#[test]
fn date_and_time_zone_and_windows_language_answer_from_their_pins() {
    let o = outcomes(
        r#"codeunit 50000 "T" { Subtype = Test;
  [Test] procedure MondayWeek() var D: Record Date; begin D.SetRange("Period Type", D."Period Type"::Week); D.SetRange("Period Start", DMY2Date(19, 1, 2026)); if not D.FindFirst() then Error('no week'); if D."Period No." <> 4 then Error('week %1', D."Period No."); end;
  [Test] procedure ZonesNumbered() var Z: Record "Time Zone"; N: Integer; begin if Z.Count() < 2 then Error('few'); Z.FindSet(); repeat N += 1; if Z."No." <> N then Error('gap'); until Z.Next() = 0; end;
  [Test] procedure English() var W: Record "Windows Language"; begin if not W.Get(1033) then Error('no 1033'); if W."Language Tag" <> 'en-US' then Error('tag'); if W.Get(999999) then Error('999999'); end; }"#,
    );
    for t in ["MondayWeek", "ZonesNumbered", "English"] {
        passes(&o, t);
    }
}

#[test]
fn a_permission_set_is_listed_with_its_caption_and_assignable_default() {
    let o = outcomes(
        r#"table 50001 "Thing" { fields { field(1; "No."; Integer) { } } keys { key(PK; "No.") { Clustered = true; } } }
permissionset 50002 "My Set" { Caption = 'My Set Caption'; Assignable = true; Permissions = tabledata "Thing" = RIMD; }
permissionset 50003 "No Assign" { Permissions = tabledata "Thing" = r; }
codeunit 50000 "T" { Subtype = Test;
  [Test] procedure Listed() var M: Record "Metadata Permission Set"; begin M.SetRange("Role ID", 'MY SET'); M.FindFirst(); if M.Name <> 'My Set Caption' then Error('name'); if not M.Assignable then Error('assignable'); end;
  [Test] procedure UndeclaredIsNotAssignable() var M: Record "Metadata Permission Set"; begin M.SetRange("Role ID", 'NO ASSIGN'); M.FindFirst(); if M.Assignable then Error('assignable'); if M.Name <> 'NO ASSIGN' then Error('name %1', M.Name); end;
  [Test] procedure Grants() var P: Record "Metadata Permission"; begin P.SetRange("Role ID", 'MY SET'); if P.Count() <> 1 then Error('count'); P.FindFirst(); if P."Insert Permission" <> P."Insert Permission"::Yes then Error('insert'); if P."Object Type" <> P."Object Type"::"Table Data" then Error('type'); end; }"#,
    );
    for t in ["Listed", "UndeclaredIsNotAssignable", "Grants"] {
        passes(&o, t);
    }
}

#[test]
fn this_ready_isolation_member_and_filter_group_setter_lower() {
    let o = outcomes(
        r#"table 50001 "Thing" { fields { field(1; "No."; Integer) { } } keys { key(PK; "No.") { Clustered = true; } } }
codeunit 50000 "T" { Subtype = Test;
  local procedure Twice(N: Integer): Integer begin exit(N * 2); end;
  [Test] procedure ThisCall() begin if this.Twice(2) <> 4 then Error('this'); end;
  [Test] procedure Setters() var R: Record "Thing"; begin R.ReadIsolation := R.ReadIsolation::ReadUnCommitted; R.FilterGroup := 2; if R.FilterGroup <> 2 then Error('group'); end; }"#,
    );
    passes(&o, "ThisCall");
    passes(&o, "Setters");
}

/// Compiling every table of the System app (eager) must not make the Key table panic on a platform
/// table that has no provider: it has no usable members, so it has no fields and no keys, and its
/// Key rows are refused like those of any table nothing compiled. The tables with a real
/// declaration still answer (corpus record/TestKeyVirtualTable.al:67-80).
#[test]
fn the_key_table_survives_an_eager_compile_of_platform_tables_without_a_provider() {
    let o = outcomes_with(
        r#"table 50001 "Probe" { fields { field(1; A; Integer) { } field(2; B; Integer) { } }
  keys { key(PK; A) { Clustered = true; } key(S; B, A) { } } }
codeunit 50000 "T" { Subtype = Test;
  [Test] procedure CountsTheDeclaredKeysAndTheSystemIdKey() var K: Record "Key"; begin
    K.SetRange(TableNo, Database::"Probe");
    if K.Count() <> 3 then Error('count %1', K.Count()); end; }"#,
        al_sema::check::Roots {
            eager: true,
            ..Default::default()
        },
    );
    passes(&o, "CountsTheDeclaredKeysAndTheSystemIdKey");
}

/// Change Log Management assigns `xRecRef.ReadIsolation` on a RecordRef parameter (Base
/// Application, ChangeLogManagement.Codeunit.al). Assigning it compiles and runs on a parameter
/// and on a local RecordRef, as the corpus pins for a plain one (record/TestRecordLock.al:86,
/// session/TestBCLockingContracts.al:14, both `*_IsCallable`).
#[test]
fn read_isolation_is_assignable_on_a_recordref_parameter_and_a_local_one() {
    let o = outcomes(
        r#"table 50002 "Probe" { fields { field(1; A; Integer) { } } keys { key(PK; A) { } } }
codeunit 50001 "H" {
  procedure OnParam(var RecRef: RecordRef; xRecRef: RecordRef) begin xRecRef.ReadIsolation := xRecRef.ReadIsolation::ReadCommitted; end;
  procedure OnLocal() var R: RecordRef; begin R.Open(50002); R.ReadIsolation := R.ReadIsolation::ReadCommitted; end; }
codeunit 50000 "T" { Subtype = Test;
  [Test] procedure Param() var H: Codeunit H; R: RecordRef; begin R.Open(50002); H.OnParam(R, R); end;
  [Test] procedure Local() var H: Codeunit H; begin H.OnLocal(); end; }"#,
    );
    passes(&o, "Param");
    passes(&o, "Local");
}

/// `Database::"X"` is the table's id, a constant of its declaration. A platform table nothing
/// provides rows for is refused where code reads or writes it as a record, not where code only
/// names its id (Base Application's Change Log Management lists such tables by id).
#[test]
fn the_id_of_a_platform_table_with_no_provider_can_be_named() {
    let o = outcomes(
        r#"codeunit 50000 "T" { Subtype = Test;
  [Test] procedure NamesTheId() var Id: Integer; begin Id := Database::"Feature Key"; if Id <> 2000000211 then Error('id %1', Id); end;
  [Test] procedure ReadingTheRowsIsStillRefused() var P: Record "Feature Key"; begin if P.IsEmpty() then; end; }"#,
    );
    passes(&o, "NamesTheId");
    let why = not_run(&o, "ReadingTheRowsIsStillRefused");
    assert!(why.contains("has no provider"), "{why}");
}

/// Report Layout List, Report Metadata and Report Data Items (handlers/TestSourceReportLayoutFlags.al,
/// TestPrecompiledReportLayouts.al, TestReportMetadataColumns.al).
#[test]
fn a_report_is_listed_with_its_layouts_metadata_and_data_items() {
    let o = outcomes(
        r#"table 50001 "Thing" { fields { field(1; "No."; Integer) { } field(2; Kind; Option) { OptionMembers = A,B; } field(3; Name; Text[30]) { } }
  keys { key(PK; "No.") { Clustered = true; } key(ByName; Name, Kind) { } } }
report 50002 "R" { UseRequestPage = false; ProcessingOnly = false;
  dataset { dataitem(Thing; Thing) { DataItemTableView = sorting(Name, Kind) order(descending); RequestFilterFields = Name, "No.";
    dataitem(Child; Thing) { DataItemLink = "No." = field("No."); } } }
  rendering { layout(L1) { Type = Excel; LayoutFile = 'a.xlsx'; ExcelLayoutMultipleDataSheets = true; }
    layout(L2) { Type = Word; LayoutFile = 'b.docx'; ObsoleteState = Pending; ObsoleteReason = 'x'; ObsoleteTag = '1'; } } }
codeunit 50000 "T" { Subtype = Test;
  [Test] procedure Layouts() var L: Record "Report Layout List"; begin L.SetRange("Report ID", 50002); if L.Count() <> 2 then Error('count %1', L.Count());
    L.SetRange(Name, 'L1'); L.FindFirst(); if L."Layout Format" <> L."Layout Format"::Excel then Error('format'); if L.IsObsolete then Error('obsolete');
    if L.ExcelLayoutMultipleDataSheets <> L.ExcelLayoutMultipleDataSheets::"Multiple data sheets" then Error('sheets');
    L.SetRange(Name, 'L2'); L.FindFirst(); if not L.IsObsolete then Error('not obsolete'); end;
  [Test] procedure Metadata() var M: Record "Report Metadata"; begin M.Get(50002); if M.ProcessingOnly then Error('processing'); if M.UseRequestPage then Error('request page'); if M.FirstDataItemTableID <> Database::Thing then Error('table'); end;
  [Test] procedure DataItems() var D: Record "Report Data Items"; begin D.SetRange("Report ID", 50002); if D.Count() <> 2 then Error('count'); D.FindFirst();
    if D.Name <> 'Thing' then Error('name'); if D."Indentation Level" <> 0 then Error('indent'); if D."Request Filter Fields" <> '3,1' then Error('rff %1', D."Request Filter Fields");
    if D."Sorting Fields" <> '3,2' then Error('sorting %1', D."Sorting Fields"); if D."Data Item Table View" <> 'SORTING(Field3,Field2) ORDER(1)' then Error('view %1', D."Data Item Table View");
    D.Next(); if D.Name <> 'Child' then Error('child'); if D."Indentation Level" <> 1 then Error('child indent'); end; }"#,
    );
    for t in ["Layouts", "Metadata", "DataItems"] {
        passes(&o, t);
    }
}

/// XmlPort Metadata and Query Metadata (xmlport/TestXmlPortMetadataVirtualTable.al,
/// query/TestQueryMetadataVirtualTable.al).
#[test]
fn xmlports_and_queries_are_listed_from_their_declarations() {
    let o = outcomes(
        r#"table 50001 "Thing" { fields { field(1; "No."; Integer) { } } keys { key(PK; "No.") { Clustered = true; } } }
xmlport 50002 "X" { Caption = 'X Caption'; Direction = Export; Format = VariableText; UseRequestPage = false; TransactionType = Browse; TextEncoding = UTF16;
  schema { textelement(Root) { tableelement(Row; Thing) { fieldelement(No; Row."No.") { } } } } }
query 50003 "Q" { QueryType = API; APIPublisher = 'pub'; APIGroup = 'grp'; APIVersion = 'v2.0'; EntityName = 'q'; EntitySetName = 'qs';
  elements { dataitem(Thing; Thing) { column(No; "No.") { } } } }
query 50004 "Plain" { elements { dataitem(Thing; Thing) { column(No; "No.") { } } } }
codeunit 50000 "T" { Subtype = Test;
  [Test] procedure XmlPort() var M: Record "XmlPort Metadata"; begin M.Get(50002); if M.Name <> 'X' then Error('name'); if M.Caption <> 'X Caption' then Error('caption'); if M.UseRequestPage then Error('request page');
    if M.Direction <> M.Direction::Export then Error('direction'); if M.Format <> M.Format::VariableText then Error('format'); if M.TransactionType <> M.TransactionType::Browse then Error('tx'); if M.TextEncoding <> M.TextEncoding::UTF16 then Error('enc'); if M.Get(50009) then Error('invented'); end;
  [Test] procedure Query() var M: Record "Query Metadata"; P: Record "Query Metadata"; begin M.Get(50003); if M.APIPublisher <> 'pub' then Error('publisher'); if M.APIVersion <> 'v2.0' then Error('version'); if M.EntitySetName <> 'qs' then Error('set');
    P.Get(50004); if P.APIPublisher <> '' then Error('plain publisher'); if P.Caption <> 'Plain' then Error('caption %1', P.Caption); end; }"#,
    );
    passes(&o, "XmlPort");
    passes(&o, "Query");
}

/// Page Action (record/TestPageActionVirtualTable.al): the area is a container row, a group and its
/// actions nest under it.
#[test]
fn a_pages_actions_are_rows_nested_under_their_area_and_group() {
    let o = outcomes(
        r#"table 50001 "Thing" { fields { field(1; "No."; Integer) { } } keys { key(PK; "No.") { Clustered = true; } } }
page 50002 "P" { PageType = Card; SourceTable = Thing; layout { area(Content) { field(No; Rec."No.") { } } }
  actions { area(Processing) { group(G) { action(A) { RunObject = page P; } action(B) { } } } } }
codeunit 50000 "T" { Subtype = Test;
  [Test] procedure Nesting() var G: Record "Page Action"; A: Record "Page Action"; C: Record "Page Action"; begin
    G.SetRange("Page ID", 50002); G.SetRange(Name, 'G'); G.FindFirst();
    C.SetRange("Page ID", 50002); C.SetRange("Action ID", G."Parent Action ID"); C.FindFirst();
    if C.Indentation <> 0 then Error('container'); if G.Indentation <> 1 then Error('group');
    if C."Action Type" = G."Action Type" then Error('types');
    A.SetRange("Page ID", 50002); A.SetRange(Name, 'A'); A.FindFirst();
    if A.Indentation <> 2 then Error('action'); if A."Parent Action ID" <> G."Action ID" then Error('parent'); if A.RunObjectID <> 50002 then Error('run object');
    A.SetRange(Name, 'B'); A.FindFirst(); if A.RunObjectID <> 0 then Error('plain run object'); end; }"#,
    );
    passes(&o, "Nesting");
}

/// Event Subscription (record/TestEventSubscriptionVirtualTable.al): rows of compiled subscribers.
#[test]
fn event_subscribers_are_rows_keyed_by_codeunit_and_function() {
    let o = outcomes(
        r#"codeunit 50001 "Pub" { [IntegrationEvent(false, false)] procedure OnThing() begin end; [BusinessEvent(false)] procedure OnBusiness() begin end; }
codeunit 50002 "Sub" {
  [EventSubscriber(ObjectType::Codeunit, Codeunit::"Pub", 'OnThing', '', false, false)] local procedure H1() begin end;
  [EventSubscriber(ObjectType::Codeunit, Codeunit::"Pub", 'OnBusiness', '', false, false)] local procedure H2() begin end; }
codeunit 50000 "T" { Subtype = Test;
  [Test] procedure Rows() var E: Record "Event Subscription"; begin
    E.SetRange("Subscriber Codeunit ID", 50002); if E.Count() <> 2 then Error('count %1', E.Count());
    E.Get(50002, 'H1'); if E."Published Function" <> 'OnThing' then Error('function'); if E."Event Type" <> E."Event Type"::Integration then Error('type');
    if E."Publisher Object Type" <> E."Publisher Object Type"::Codeunit then Error('object type'); if E."Publisher Object ID" <> 50001 then Error('object id'); if not E.Active then Error('active');
    E.Get(50002, 'H2'); if E."Event Type" <> E."Event Type"::Business then Error('business');
    if E.Get(50002, 'Nope') then Error('invented'); E.SetRange("Subscriber Codeunit ID", 50001); if not E.IsEmpty() then Error('publisher'); end; }"#,
    );
    passes(&o, "Rows");
}

/// Table Relations Metadata (record/TestTableRelationsMetadataVirtualTable.al), including a
/// condition that is a `filter(...)`, which the runtime's own relation check does not understand.
#[test]
fn table_relations_are_rows_per_arm_and_condition() {
    let o = outcomes(
        r#"table 50001 "Parent" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { Clustered = true; } } }
table 50002 "Parent B" { fields { field(1; Code; Code[20]) { } } keys { key(PK; Code) { Clustered = true; } } }
table 50003 "Child" { fields { field(1; "Entry No."; Integer) { } field(2; Kind; Option) { OptionMembers = A,B; } field(3; Doc; Option) { OptionMembers = X,Y; }
    field(4; Plain; Code[20]) { TableRelation = Parent.Code; } field(5; Soft; Code[20]) { }
    field(6; Cond; Code[20]) { TableRelation = if (Kind = const(A)) Parent.Code else "Parent B".Code; }
    field(7; Filtered; Code[20]) { TableRelation = if (Kind = const(A), Doc = filter(<> X)) Parent where(Code = filter(<> '')); } }
  keys { key(PK; "Entry No.") { Clustered = true; } } }
codeunit 50000 "T" { Subtype = Test;
  [Test] procedure Rows() var R: Record "Table Relations Metadata"; begin
    R.SetRange("Table ID", 50003); R.SetRange("Field No.", 4); if R.Count() <> 1 then Error('plain'); R.FindFirst(); if R."Related Table ID" <> 50001 then Error('table'); if R."Related Field No." <> 1 then Error('field');
    R.SetRange("Field No.", 5); if not R.IsEmpty() then Error('soft');
    R.SetRange("Field No.", 6); if R.Count() <> 2 then Error('cond'); R.FindSet(); if R."Relation No." <> 1 then Error('first'); if R."Condition Field No." <> 2 then Error('cond field'); R.Next(); if R."Related Table ID" <> 50002 then Error('else');
    R.SetRange("Field No.", 7); if R.Count() <> 2 then Error('two conditions %1', R.Count()); R.FindFirst(); if R."Condition Field No." <> 2 then Error('first condition'); R.Next(); if R."Condition Field No." <> 3 then Error('second condition'); end;
  [Test] procedure FieldRelation() var F: Record "Field"; begin F.Get(50003, 7); if F.RelationTableNo <> 50001 then Error('filtered relation %1', F.RelationTableNo); if F.RelationFieldNo <> 1 then Error('related field'); end;
  [Test] procedure ConditionalRelation() var F: Record "Field"; begin F.Get(50003, 6); end; }"#,
    );
    passes(&o, "FieldRelation");
    let why = not_run(&o, "ConditionalRelation");
    assert!(why.contains("conditional TableRelation"), "{why}");
    passes(&o, "Rows");
}

/// The code coverage tables hold nothing until a recording starts; after one they are refused until
/// a DeleteAll empties them (codeunit/TestCodeCoverageTable.al).
#[test]
fn code_coverage_tables_are_empty_before_recording_and_refused_after_it() {
    let o = outcomes(
        r#"codeunit 50000 "T" { Subtype = Test;
  [Test] procedure EmptyAtFirst() var C: Record "Code Coverage"; begin if not C.IsEmpty() then Error('rows'); end;
  [Test] procedure RefusedAfterRecording() var C: Record "Code Coverage"; begin CodeCoverageLog(true, false); CodeCoverageLog(false, false); if C.IsEmpty() then; end;
  [Test] procedure DeleteAllEmptiesIt() var C: Record "Code Coverage"; begin CodeCoverageLog(true, false); CodeCoverageLog(false, false); C.DeleteAll(); if not C.IsEmpty() then Error('rows'); end;
  [Test] procedure ADeleteAllWithAFilterDoesNot() var C: Record "Code Coverage"; begin CodeCoverageLog(true, false); CodeCoverageLog(false, false); C.SetRange("Object ID", 1); C.DeleteAll(); C.Reset(); if C.IsEmpty() then; end; }"#,
    );
    passes(&o, "EmptyAtFirst");
    passes(&o, "DeleteAllEmptiesIt");
    for t in ["RefusedAfterRecording", "ADeleteAllWithAFilterDoesNot"] {
        let why = not_run(&o, t);
        assert!(why.contains("code coverage recording"), "{why}");
    }
}

/// All Profile (profile/TestAllProfileTable.al): stored and seeded from the profile objects. The delete
/// rule for an installed app's row needs an app with a GUID id: the corpus pins it.
#[test]
fn a_profile_object_is_a_row_and_a_tenant_row_can_be_added_and_removed() {
    let o = outcomes(
        r#"page 50001 "Center" { PageType = RoleCenter; }
profile "My Profile" { Caption = 'My Caption'; RoleCenter = Center; Enabled = true; }
codeunit 50000 "T" { Subtype = Test;
  [Test] procedure Present() var P: Record "All Profile"; begin P.SetRange("Profile ID", 'My Profile'); P.FindFirst(); if P.Caption <> 'My Caption' then Error('caption'); if P."Role Center ID" <> 50001 then Error('role center'); end;
  [Test] procedure TenantRow() var P: Record "All Profile"; begin P.Scope := P.Scope::Tenant; P."Profile ID" := 'Mine'; P.Caption := 'Mine'; P.Enabled := false; P.Insert(); P.Get(P.Scope::Tenant, P."App ID", 'Mine'); P.Delete(); if P.Get(P.Scope::Tenant, P."App ID", 'Mine') then Error('still there'); end; }"#,
    );
    passes(&o, "Present");
    passes(&o, "TenantRow");
}

/// A platform table nothing provides rows for is still a table: a temporary record of it holds what
/// AL inserts, a variable that is declared and never read does not stop the code that has it, and
/// only a read or write of its persistent rows is refused, naming table and id. The corpus pins that
/// `temporary` borrows the shape of a platform table and the provider never sees the record
/// (record/TestTemporaryVirtualTableRecord.al, for Field, Date and Aggregate Permission Set);
/// WMS Management keeps its error log in `Record "License Information" temporary`.
#[test]
fn a_platform_table_without_provider_works_as_a_temporary_record_and_is_refused_otherwise() {
    let o = outcomes(
        r#"codeunit 50001 "Holder" { var L: Record "License Information" temporary; G: Record "Feature Key";
  procedure Log(N: Integer; T: Text) begin L."Line No." := N; L.Text := CopyStr(T, 1, 250); L.Insert(); end;
  procedure Count(): Integer begin exit(L.Count()); end; }
codeunit 50000 "T" { Subtype = Test;
  [Test] procedure TemporaryRoundTrips() var H: Codeunit Holder; begin H.Log(1, 'a'); H.Log(2, 'b'); if H.Count() <> 2 then Error('count %1', H.Count()); end;
  [Test] procedure DeclaredAndNeverRead() var G: Record "Feature Key"; begin end;
  [Test] procedure PersistentReadIsRefused() var G: Record "Feature Key"; begin if G.Get('X') then; end;
  [Test] procedure PersistentWriteIsRefused() var L: Record "License Information"; begin L."Line No." := 1; L.Insert(); end; }"#,
    );
    passes(&o, "TemporaryRoundTrips");
    passes(&o, "DeclaredAndNeverRead");
    for t in ["PersistentReadIsRefused", "PersistentWriteIsRefused"] {
        let why = not_run(&o, t);
        assert!(why.contains("has no provider"), "{t}: {why}");
    }
    let why = not_run(&o, "PersistentReadIsRefused");
    assert!(why.contains("platform table 'Feature Key' (2000000211)"), "{why}");
}

/// Columns that the declaration describes, filled from it: the caption, card page and editability of
/// a page, and the table name, caption, class and enabled flag of a field. Not pinned by a corpus
/// test: Page Management reads CardPageID off the lookup page and Microsoft's tests read the others
/// (docs/probes-needed.md, "Probe batch 5").
#[test]
fn declared_caption_card_page_class_and_enabled_are_read_from_the_declaration() {
    let o = outcomes(
        r#"table 50001 "Thing" { fields { field(1; "No."; Integer) { Caption = 'Number'; } field(2; Total; Decimal) { FieldClass = FlowField; CalcFormula = count(Thing); Caption = 'The total'; }
    field(3; Filt; Integer) { FieldClass = FlowFilter; } field(4; Off; Integer) { Enabled = false; } field(5; Plain; Text[10]) { } }
  keys { key(PK; "No.") { Clustered = true; } } }
page 50002 "Thing Card" { PageType = Card; SourceTable = Thing; Caption = 'A card'; }
page 50003 "Thing List" { PageType = List; SourceTable = Thing; CardPageID = "Thing Card"; }
codeunit 50000 "T" { Subtype = Test;
  [Test] procedure Pages() var M: Record "Page Metadata"; begin M.Get(50003); if M.CardPageID <> 50002 then Error('card %1', M.CardPageID); if M.Caption <> 'Thing List' then Error('caption %1', M.Caption);
    M.Get(50002); if M.CardPageID <> 0 then Error('no card'); if M.Caption <> 'A card' then Error('card caption %1', M.Caption); end;
  [Test] procedure Fields() var F: Record "Field"; begin
    F.Get(50001, 1); if F.TableName <> 'Thing' then Error('table name'); if F."Field Caption" <> 'Number' then Error('caption %1', F."Field Caption"); if F.Class <> F.Class::Normal then Error('class'); if not F.Enabled then Error('enabled');
    F.Get(50001, 2); if F.Class <> F.Class::FlowField then Error('flowfield'); if F."Field Caption" <> 'The total' then Error('total caption');
    F.Get(50001, 3); if F.Class <> F.Class::FlowFilter then Error('flowfilter'); if F."Field Caption" <> 'Filt' then Error('name as caption');
    F.Get(50001, 4); if F.Enabled then Error('disabled'); end; }"#,
    );
    passes(&o, "Pages");
    passes(&o, "Fields");
}

/// A key states `MaintainSIFTIndex` and `ObsoleteState`, a table its `DrillDownPageId`; the tables
/// report them (probe batch 5: no corpus test reads them).
#[test]
fn a_keys_sift_flag_and_obsolete_state_and_a_tables_drill_down_page_are_read_from_the_declaration() {
    let o = outcomes(
        r#"table 50001 "Thing" { DrillDownPageId = "Thing List"; fields { field(1; "No."; Integer) { } field(2; Amt; Decimal) { } }
  keys { key(PK; "No.") { Clustered = true; } key(S1; Amt) { MaintainSIFTIndex = false; } key(S2; Amt, "No.") { ObsoleteState = Pending; ObsoleteReason = 'x'; ObsoleteTag = '1'; } } }
page 50002 "Thing List" { PageType = List; SourceTable = Thing; }
codeunit 50000 "T" { Subtype = Test;
  [Test] procedure Keys() var K: Record "Key"; begin K.SetRange(TableNo, 50001); K.SetRange("No.", 1); K.FindFirst(); if not K.MaintainSIFTIndex then Error('default sift'); if K.ObsoleteState <> K.ObsoleteState::No then Error('default state');
    K.SetRange("No.", 2); K.FindFirst(); if K.MaintainSIFTIndex then Error('sift'); K.SetRange("No.", 3); K.FindFirst(); if K.ObsoleteState <> K.ObsoleteState::Pending then Error('pending'); end;
  [Test] procedure Table() var M: Record "Table Metadata"; begin M.Get(50001); if M.DrillDownPageId <> 50002 then Error('drill down %1', M.DrillDownPageId); end; }"#,
    );
    passes(&o, "Keys");
    passes(&o, "Table");
}
