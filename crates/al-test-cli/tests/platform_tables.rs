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
    outcomes_app("main", main, roots)
}

/// [`outcomes_with`] with the id of the app under test chosen (a platform table lists apps by Guid).
fn outcomes_app(app_id: &str, main: &str, roots: al_sema::check::Roots) -> Vec<(String, Outcome)> {
    use al_sema::workspace::{AppKind, AppSource};
    let app = |id: &str, name: &str, deps: &[&str], files: Vec<(String, String)>| AppSource {
        id: id.into(),
        name: name.into(),
        dependencies: deps.iter().map(|d| d.to_string()).collect(),
        files,
    };
    let (_, l) = al_sema::lower::compile_apps_with(
        vec![
            app(app_id, "main", &[], vec![("main.al".into(), main.into())]),
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
    // The two sources above carry no manifest, so no version; the app tables list an app only with one.
    let mut program = (*l.program).clone();
    for a in program.apps.iter_mut().filter(|a| a.version.is_empty()) {
        a.version = "1.0.0.0".into();
    }
    let engine = Engine::new(std::sync::Arc::new(program), blocked);
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
  [Test] procedure UsesATableWithNoProvider() var P: Record "License Information"; begin if P.IsEmpty() then; end; }"#,
    );
    let why = not_run(&o, "UsesATableWithNoProvider");
    assert!(
        why.contains("platform table 'License Information' (2000000040) has no provider"),
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
  [Test] procedure ReadsAnUnpinnedColumn() var F: Record Field; L: Text; begin L := F."Type Name"; end; }"#,
    );
    let why = not_run(&o, "ReadsAnUnpinnedColumn");
    assert!(why.contains("column 9 of platform table 'Field'"), "{why}");
    assert!(
        !matches!(o.iter().find(|(n, _)| n == "ReadsAPinnedColumn").map(|(_, o)| o), Some(Outcome::NotRun(w)) if w.contains("column")),
        "{o:?}"
    );
}

// Business Foundation's table extensions on "Source Code" declare fields with `ObsoleteState = Moved`; Base
// Application's Dimension Management scans the Field table for all tables. A moved field has no row in the Field
// table, a pending one has (types/PWMisc.al:FieldTable_MovedAndPendingField, BC 27.5 and 29.0).
#[test]
fn a_moved_field_has_no_row_in_the_field_table_and_a_scan_of_all_fields_runs() {
    let o = outcomes(
        r#"table 50001 "Moved Host" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { } } }
tableextension 50002 "Moved Ext" extends "Moved Host" { fields {
  field(50000; Gone; Boolean) { ObsoleteState = Moved; ObsoleteReason = 'x'; ObsoleteTag = '1'; MovedTo = '437dbf0e-84ff-417a-965d-ed2bb9650972'; }
  field(50001; Soon; Boolean) { ObsoleteState = Pending; ObsoleteReason = 'x'; ObsoleteTag = '1'; } } }
codeunit 50000 "T" { Subtype = Test;
  [Test] procedure ScansAllFields() var F: Record Field; begin F.SetRange(Class, F.Class::Normal); if F.FindSet() then repeat until F.Next() = 0; end;
  [Test] procedure MovedHasNoRow() var F: Record Field; begin F.SetRange(TableNo, 50001); F.SetRange("No.", 50000); if F.FindFirst() then Error('moved field listed'); end;
  [Test] procedure PendingHasARow() var F: Record Field; begin F.SetRange(TableNo, 50001); F.SetRange("No.", 50001); if not F.FindFirst() then Error('pending field missing'); end; }"#,
    );
    passes(&o, "ScansAllFields");
    passes(&o, "MovedHasNoRow");
    passes(&o, "PendingHasARow");
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

// Gen. Jnl.-Post Line PostBankAcc does `BankAccLedgEntry.CopyLinks(GenJnlLine)` for a journal line without links, in
// about 330 Microsoft tests: the copy of nothing is nothing, not the "There is no Record Link within the filter" error
// an unused FindSet would raise.
#[test]
fn copying_the_links_of_a_record_without_links_copies_nothing() {
    let o = outcomes(
        r#"table 50001 "Host" { fields { field(1; "No."; Integer) { } } keys { key(PK; "No.") { Clustered = true; } } }
codeunit 50000 "T" { Subtype = Test;
  [Test] procedure CopyNothing() var A: Record "Host"; B: Record "Host"; begin
    A."No." := 1; A.Insert(); B."No." := 2; B.Insert();
    B.CopyLinks(A);
    if B.HasLinks() then Error('links appeared');
    A.AddLink('https://example.com', 'D');
    B.CopyLinks(A);
    if not B.HasLinks() then Error('links not copied');
  end; }"#,
    );
    passes(&o, "CopyNothing");
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
  [Test] procedure NamesTheId() var Id: Integer; begin Id := Database::"License Information"; if Id <> 2000000040 then Error('id %1', Id); end;
  [Test] procedure ReadingTheRowsIsStillRefused() var P: Record "License Information"; begin if P.IsEmpty() then; end; }"#,
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
    L.SetRange(Name, 'L2'); L.FindFirst(); if not L.IsObsolete then Error('not obsolete');
    // A layout with no Caption or Summary reads its name for both (session/PWPlatformTables.al:126-139).
    if L.Caption <> 'L2' then Error('caption [%1]', L.Caption); if L.Description <> 'L2' then Error('description'); end;
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

/// An xmlport that leaves `Encoding` out reads UTF16 (xmlport/TestPlatformMetadataDefaultsContracts.al:93),
/// not UTF8; one that declares it is not pinned and its row is refused.
#[test]
fn an_xmlport_without_an_encoding_property_reads_utf16() {
    let o = outcomes(
        r#"table 50001 "Thing" { fields { field(1; "No."; Integer) { } } keys { key(PK; "No.") { Clustered = true; } } }
xmlport 50002 "X" { schema { textelement(Root) { tableelement(Row; Thing) { fieldelement(No; Row."No.") { } } } } }
codeunit 50000 "T" { Subtype = Test;
  [Test] procedure Default() var M: Record "XmlPort Metadata"; begin M.Get(50002); if M.Encoding <> M.Encoding::UTF16 then Error('encoding'); end; }"#,
    );
    passes(&o, "Default");
}

/// "Page Metadata"."CardPageID" is the id of the page named in CardPageId and 0 when the page states none
/// (handlers/TestPageCloseTriggersSelectionCardPageContracts.al:305-315).
#[test]
fn a_pages_card_page_id_is_the_stated_page_or_zero() {
    let o = outcomes(
        r#"table 50001 "Thing" { fields { field(1; "No."; Integer) { } } keys { key(PK; "No.") { Clustered = true; } } }
page 50002 "Card" { PageType = Card; SourceTable = Thing; CardPageId = "Card"; layout { area(Content) { field(No; Rec."No.") { } } } }
page 50003 "List" { PageType = List; SourceTable = Thing; CardPageId = "Card"; layout { area(Content) { repeater(R) { field(No; Rec."No.") { } } } } }
page 50004 "Plain List" { PageType = List; SourceTable = Thing; layout { area(Content) { repeater(R) { field(No; Rec."No.") { } } } } }
codeunit 50000 "T" { Subtype = Test;
  [Test] procedure CardPage() var M: Record "Page Metadata"; begin
    M.Get(50003); if M.CardPageID <> 50002 then Error('list %1', M.CardPageID);
    M.Get(50002); if M.CardPageID <> 50002 then Error('card %1', M.CardPageID);
    M.Get(50004); if M.CardPageID <> 0 then Error('plain %1', M.CardPageID); end; }"#,
    );
    passes(&o, "CardPage");
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

/// A table trigger event bound to a field is published as `<event> - <field>`
/// (xmlport/TestPlatformMetadataDefaultsContracts.al:113-124).
#[test]
fn a_validate_subscription_on_a_field_names_the_field_in_the_published_function() {
    let o = outcomes(
        r#"table 50001 "Thing" { fields { field(1; "No."; Integer) { } field(2; Txt; Text[10]) { } } keys { key(PK; "No.") { Clustered = true; } } }
codeunit 50002 "Sub" {
  [EventSubscriber(ObjectType::Table, Database::"Thing", 'OnAfterInsertEvent', '', false, false)] local procedure H1() begin end;
  [EventSubscriber(ObjectType::Table, Database::"Thing", 'OnBeforeValidateEvent', 'Txt', false, false)] local procedure H2() begin end; }
codeunit 50000 "T" { Subtype = Test;
  [Test] procedure Rows() var E: Record "Event Subscription"; begin
    E.Get(50002, 'H1'); if E."Published Function" <> 'OnAfterInsertEvent' then Error('plain %1', E."Published Function");
    E.Get(50002, 'H2'); if E."Published Function" <> 'OnBeforeValidateEvent - Txt' then Error('field %1', E."Published Function"); end; }"#,
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
  [Test] procedure ConditionalRelation() var F: Record "Field"; begin F.Get(50003, 6); if (F.RelationTableNo <> 0) or (F.RelationFieldNo <> 0) then Error('conditional relation reads 0/0'); end; }"#,
    );
    passes(&o, "FieldRelation");
    // record/TestPlatformMetadataDeclarationContracts.al:262-264.
    passes(&o, "ConditionalRelation");
    passes(&o, "Rows");
}

/// The code coverage tables hold nothing until a recording starts; after one they hold what ran
/// (codeunit/TestCodeCoverageTable.al, TestCodeCoveragePerTest.al). `Line No.` is a source line.
#[test]
fn code_coverage_tables_hold_what_was_recorded_and_delete_all_empties_them() {
    let o = outcomes(
        r#"codeunit 50000 "T" { Subtype = Test;
  procedure Initialize() var C: Record "Code Coverage"; R: Record "Code Coverage Tests Run"; L: Record "Code Coverage Test Lookup"; begin CodeCoverageLog(false, false); C.DeleteAll(); R.DeleteAll(); L.DeleteAll(); end;
  procedure Branch(V: Integer): Integer
  begin
    if V > 1 then
      exit(1);
    exit(0);
  end;
  [Test] procedure EmptyAtFirst() var C: Record "Code Coverage"; begin if not C.IsEmpty() then Error('rows'); end;
  [Test] procedure RowsAfterRecording()
  var C: Record "Code Coverage";
  begin Initialize();
    CodeCoverageLog(true, false); Branch(5); CodeCoverageLog(false, false);
    C.SetRange("Object Type", C."Object Type"::Codeunit); C.SetRange("Object ID", 50000);
    C.SetRange("Line Type", C."Line Type"::"Trigger/Function"); C.SetRange("Line No.", 3);
    if C.IsEmpty() then Error('declaration row');
    C.SetRange("Line Type", C."Line Type"::Code); C.SetRange("Line No.", 6);
    C.FindFirst(); if C."No. of Hits" = 0 then Error('hit');
    C.SetRange("Line No.", 7);
    C.FindFirst(); if C."No. of Hits" <> 0 then Error('not hit');
  end;
  [Test] procedure NothingRecordedWhileOff()
  var C: Record "Code Coverage";
  begin Initialize();
    Branch(5); C.SetRange("Object ID", 50000);
    if not C.IsEmpty() then Error('rows');
  end;
  [Test] procedure DeleteAllEmptiesIt() var C: Record "Code Coverage"; begin Initialize(); CodeCoverageLog(true, false); Branch(1); CodeCoverageLog(false, false); C.DeleteAll(); if not C.IsEmpty() then Error('rows'); end;
  [Test] procedure AFilteredDeleteAllLeavesTheOtherRows() var C: Record "Code Coverage"; begin Initialize(); CodeCoverageLog(true, false); Branch(1); CodeCoverageLog(false, false); C.SetRange("Object ID", 1); C.DeleteAll(); C.Reset(); if C.IsEmpty() then Error('gone'); end;
  [Test] procedure ARecordingAfterDeleteAllStartsFromNothing()
  var C: Record "Code Coverage";
  begin Initialize();
    CodeCoverageLog(true, false); Branch(1); CodeCoverageLog(false, false); C.DeleteAll();
    CodeCoverageLog(true, false); CodeCoverageLog(false, false);
    C.SetRange("Object ID", 50000); C.SetRange("Line No.", 6);
    if not C.IsEmpty() then Error('old rows came back');
  end; }"#,
    );
    passes(&o, "EmptyAtFirst");
    passes(&o, "RowsAfterRecording");
    passes(&o, "NothingRecordedWhileOff");
    passes(&o, "DeleteAllEmptiesIt");
    passes(&o, "AFilteredDeleteAllLeavesTheOtherRows");
    passes(&o, "ARecordingAfterDeleteAllStartsFromNothing");
}

/// codeunit/TestCodeCoverageLineTextContracts.al: the `begin` and `end` lines have `Empty` rows, every row's `Line` holds the
/// platform's text for an app without source, and the status is Covered, Not Covered or Non Applicable.
#[test]
fn code_coverage_rows_of_an_app_without_source_hold_the_platform_text_and_a_status() {
    let o = outcomes(
        r#"codeunit 50000 "T" { Subtype = Test;
  procedure Branch(V: Integer): Integer
  begin
    if V > 1 then
      exit(1);
    exit(0);
  end;
  [Test] procedure Rows()
  var C: Record "Code Coverage"; Rows: Text;
  begin
    CodeCoverageLog(false, false); C.DeleteAll();
    CodeCoverageLog(true, false); Branch(5); CodeCoverageLog(false, false);
    C.SetRange("Object ID", 50000); C.SetRange("Line No.", 2, 7);
    C.FindSet();
    repeat
      if not C.Line.StartsWith('The extension that declares this object has its manifest property ShowMyCode') then Error('line text %1', C.Line);
      Rows += Format(C."Line No.") + ' ' + Format(C."Line Type") + ' ' + Format(C."No. of Hits") + ' ' + Format(C."Code Coverage Status") + '|';
    until C.Next() = 0;
    if Rows <> '2 Trigger/Function 0 Non Applicable|3 Empty 0 Non Applicable|4 Code 1 Covered|5 Code 1 Covered|6 Code 0 Not Covered|7 Empty 0 Non Applicable|' then Error(Rows);
  end; }"#,
    );
    passes(&o, "Rows");
}

/// A `[Test]` procedure that starts while recording has a Tests Run row, and the methods that run
/// inside it a Test Lookup row each; a call outside any test leaves none
/// (codeunit/TestCodeCoveragePerTest.al).
#[test]
fn code_coverage_notes_the_tests_that_start_while_recording() {
    let o = outcomes(
        r#"codeunit 50000 "T" { Subtype = Test;
  procedure Initialize() var C: Record "Code Coverage"; R: Record "Code Coverage Tests Run"; L: Record "Code Coverage Test Lookup"; begin CodeCoverageLog(false, false); C.DeleteAll(); R.DeleteAll(); L.DeleteAll(); end;
  procedure Helper(): Integer begin exit(1); end;
  [Test] procedure Inner() begin Helper(); end;
  [Test] procedure ATestStartedWhileRecording()
  var R: Record "Code Coverage Tests Run"; L: Record "Code Coverage Test Lookup";
  begin Initialize();
    CodeCoverageLog(true, false); Inner(); CodeCoverageLog(false, false);
    R.SetRange("Object ID", 50000);
    if R.Count() <> 1 then Error('one test row');
    R.FindFirst(); if R."Method Name" <> 'Inner' then Error('name');
    L.SetRange("Object ID", 50000); L.SetRange("Test ID", R."Test ID");
    if L.Count() <> 1 then Error('one method inside the test');
    L.FindFirst(); if L."Method ID" = R."Method ID" then Error('the test itself is no method inside it');
  end;
  [Test] procedure ACallOutsideAnyTest()
  var R: Record "Code Coverage Tests Run"; L: Record "Code Coverage Test Lookup";
  begin Initialize();
    CodeCoverageLog(true, false); Helper(); CodeCoverageLog(false, false);
    R.SetRange("Object ID", 50000); L.SetRange("Object ID", 50000);
    if not R.IsEmpty() then Error('tests run');
    if not L.IsEmpty() then Error('lookup');
  end; }"#,
    );
    passes(&o, "ATestStartedWhileRecording");
    passes(&o, "ACallOutsideAnyTest");
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

/// Base Application's `profile "ORDER PROCESSOR"` names its role center by page id (`RoleCenter = 9006;`): the row
/// exists with that id (about 320 Microsoft tests start with `AllProfile.SetRange("Profile ID", 'ORDER PROCESSOR')`).
#[test]
fn a_profile_that_names_its_role_center_by_page_id_is_a_row() {
    let o = outcomes(
        r#"page 50001 "Center" { PageType = RoleCenter; }
profile "BY ID" { RoleCenter = 50001; }
codeunit 50000 "T" { Subtype = Test;
  [Test] procedure ById() var P: Record "All Profile"; begin P.SetRange("Profile ID", 'BY ID'); P.FindFirst(); if P."Role Center ID" <> 50001 then Error('role center %1', P."Role Center ID"); end; }"#,
    );
    passes(&o, "ById");
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
        r#"codeunit 50001 "Holder" { var L: Record "License Information" temporary; G: Record "License Information";
  procedure Log(N: Integer; T: Text) begin L."Line No." := N; L.Text := CopyStr(T, 1, 250); L.Insert(); end;
  procedure Count(): Integer begin exit(L.Count()); end; }
codeunit 50000 "T" { Subtype = Test;
  [Test] procedure TemporaryRoundTrips() var H: Codeunit Holder; begin H.Log(1, 'a'); H.Log(2, 'b'); if H.Count() <> 2 then Error('count %1', H.Count()); end;
  [Test] procedure DeclaredAndNeverRead() var G: Record "License Information"; begin end;
  [Test] procedure PersistentReadIsRefused() var G: Record "License Information"; begin if G.Get(1) then; end;
  [Test] procedure PersistentWriteIsRefused() var L: Record "License Information"; begin L."Line No." := 1; L.Insert(); end; }"#,
    );
    passes(&o, "TemporaryRoundTrips");
    passes(&o, "DeclaredAndNeverRead");
    for t in ["PersistentReadIsRefused", "PersistentWriteIsRefused"] {
        let why = not_run(&o, t);
        assert!(why.contains("has no provider"), "{t}: {why}");
    }
    let why = not_run(&o, "PersistentReadIsRefused");
    assert!(
        why.contains("platform table 'License Information' (2000000040)"),
        "{why}"
    );
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
  keys { key(PK; "No.") { Clustered = true; } key(S1; Amt) { MaintainSIFTIndex = false; } key(S2; Amt, "No.") { SumIndexFields = "No."; ObsoleteState = Pending; ObsoleteReason = 'x'; ObsoleteTag = '1'; } } }
page 50002 "Thing List" { PageType = List; SourceTable = Thing; }
codeunit 50000 "T" { Subtype = Test;
  [Test] procedure Keys() var K: Record "Key"; begin K.SetRange(TableNo, 50001); K.SetRange("No.", 1); K.FindFirst(); if K.MaintainSIFTIndex then Error('default sift'); if K.ObsoleteState <> K.ObsoleteState::No then Error('default state');
    K.SetRange("No.", 2); K.FindFirst(); if K.MaintainSIFTIndex then Error('sift'); K.SetRange("No.", 3); K.FindFirst(); if not K.MaintainSIFTIndex then Error('sum index fields'); if K.ObsoleteState <> K.ObsoleteState::Pending then Error('pending'); end;
  [Test] procedure Table() var M: Record "Table Metadata"; begin M.Get(50001); if M.DrillDownPageId <> 50002 then Error('drill down %1', M.DrillDownPageId); end; }"#,
    );
    passes(&o, "Keys");
    passes(&o, "Table");
}

#[test]
fn a_published_application_row_passes_the_tenant_filter_of_the_retention_policy_module() {
    // install/TestRetenPolAllowedTables_Tests.al: Base Application registers table 405 only when
    // System Application finds its Published Application row through
    // SetFilter("Tenant ID", '%1|%2', '', TenantId). The row must therefore satisfy that filter.
    let o = outcomes_app(
        "5d86850b-0d76-4eca-bd7b-951ad998e997",
        r#"codeunit 50000 "T" { Subtype = Test;
  [Test] procedure OwnRowPassesTheTenantFilter() var P: Record "Published Application"; begin
    P.SetRange(ID, '5d86850b-0d76-4eca-bd7b-951ad998e997');
    P.SetFilter("Tenant ID", '%1|%2', '', 'any-tenant');
    if not P.FindFirst() then Error('no row'); end; }"#,
        al_sema::check::Roots::default(),
    );
    passes(&o, "OwnRowPassesTheTenantFilter");
}

/// A relation whose condition is `Kind = filter(A | B)` lists one row with Condition Type FILTER and the member
/// ordinals joined by `|` (session/PWPlatformTables.al:109-123, BC 27.5).
#[test]
fn a_filter_condition_of_option_members_lists_the_ordinals() {
    let o = outcomes(
        r#"table 50001 "Target" { fields { field(1; Id; Code[10]) { } } keys { key(PK; Id) { Clustered = true; } } }
table 50002 "Source" { fields { field(1; Id; Integer) { } field(2; Kind; Option) { OptionMembers = A,B,C; }
    field(3; Ref; Code[10]) { TableRelation = if (Kind = filter(A | B)) Target.Id; } }
  keys { key(PK; Id) { Clustered = true; } } }
codeunit 50000 "T" { Subtype = Test;
  [Test] procedure Rows() var R: Record "Table Relations Metadata"; begin
    R.SetRange("Table ID", 50002); R.SetRange("Field No.", 3); R.FindFirst();
    if R."Condition Field No." <> 2 then Error('field %1', R."Condition Field No.");
    if Format(R."Condition Type") <> 'FILTER' then Error('type %1', Format(R."Condition Type"));
    if R."Condition Value" <> '0|1' then Error('value %1', R."Condition Value"); end; }"#,
    );
    passes(&o, "Rows");
}

/// types/ProbeTypeFormsAndFilters.al:FormatRecord_AssignmentCopiesBlobBytes (BC 27.5): a record assigned from one that
/// was never inserted shows its Blob as filled, and CalcFields then reads the Blob back empty (no stored row).
#[test]
fn calc_fields_of_a_blob_whose_row_is_not_stored_leaves_it_empty() {
    let o = outcomes(
        r#"table 50001 "Blobby" { fields { field(1; Id; Integer) { } field(2; Pic; Blob) { } } keys { key(PK; Id) { Clustered = true; } } }
codeunit 50000 "T" { Subtype = Test;
  [Test] procedure Copy() var R: Record "Blobby"; C: Record "Blobby"; Out: OutStream; Ins: InStream; T: Text; begin
    R.Id := 1; R.Pic.CreateOutStream(Out); Out.WriteText('abc'); C := R;
    if not C.Pic.HasValue() then Error('copy has no blob'); C.CalcFields(Pic); C.Pic.CreateInStream(Ins); Ins.ReadText(T);
    if T <> '' then Error('blob %1', T); end; }"#,
    );
    passes(&o, "Copy");
}

/// A filter on "Number of Calls" or "Active Manual Instances" is answered from the session's counts
/// (session/PWPlatformTables.al:EventSubscription_FilterOnNumberOfCalls, BC 27.5 observes it).
#[test]
fn event_subscriptions_can_be_filtered_on_the_session_counts() {
    let o = outcomes(
        r#"codeunit 50001 "Pub" { [IntegrationEvent(false, false)] procedure OnThing() begin end; procedure Raise() begin OnThing(); end; }
codeunit 50002 "Sub" { [EventSubscriber(ObjectType::Codeunit, Codeunit::"Pub", 'OnThing', '', false, false)] local procedure H1() begin end; }
codeunit 50000 "T" { Subtype = Test;
  [Test] procedure Counts() var E: Record "Event Subscription"; P: Codeunit "Pub"; begin
    E.SetFilter("Number of Calls", '>0'); if E.Count() <> 0 then Error('before %1', E.Count());
    P.Raise(); E.Reset(); E.SetFilter("Number of Calls", '>0'); E.SetRange("Subscriber Codeunit ID", 50002);
    if E.Count() <> 1 then Error('after %1', E.Count()); E.FindFirst(); if E."Number of Calls" <> 1 then Error('calls');
    E.Reset(); E.SetRange("Subscriber Codeunit ID", 50002); E.SetRange("Active Manual Instances", 0); if E.Count() <> 1 then Error('manual'); end; }"#,
    );
    passes(&o, "Counts");
}
