//! Companies: `ChangeCompany`, the Company table, `RecordRef.Open` with a company name. Each test
//! names the corpus test that pins it.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

const TABLES: &str = r#"
table 50910 "CC Row"
{
    fields { field(1; Id; Integer) { } field(2; V; Integer) { } }
    keys { key(PK; Id) { } }
}
table 50911 "CC Tenant"
{
    DataPerCompany = false;
    fields { field(1; Id; Integer) { } }
    keys { key(PK; Id) { } }
}
table 50912 "CC Rel"
{
    fields { field(1; Id; Integer) { } field(2; RowId; Integer) { TableRelation = "CC Row".Id; } }
    keys { key(PK; Id) { } }
}
table 50913 "CC Parent"
{
    fields
    {
        field(1; Id; Integer) { }
        field(2; Kids; Integer) { FieldClass = FlowField; CalcFormula = count("CC Child" where(ParentId = field(Id))); }
    }
    keys { key(PK; Id) { } }
}
table 50914 "CC Child"
{
    fields { field(1; Id; Integer) { } field(2; ParentId; Integer) { } }
    keys { key(PK; Id) { } }
}
table 2000000006 Company
{
    DataPerCompany = false;
    fields
    {
        field(1; Name; Text[30]) { }
        field(2; "Evaluation Company"; Boolean) { }
        field(3; "Display Name"; Text[250]) { }
        field(8000; Id; Guid) { }
        field(8005; "Business Profile Id"; Text[250]) { }
    }
    keys { key(PK; Name) { } }
}
"#;

const NO_COMPANY_TABLE: &str = r#"
table 50910 "CC Row"
{
    fields { field(1; Id; Integer) { } field(2; V; Integer) { } }
    keys { key(PK; Id) { } }
}
"#;

fn run_with(tables: &str, vars: &str, body: &str) -> Result<Value, Signal> {
    let src = format!("{tables}\ncodeunit 50100 C {{ procedure F(): Text var {vars} begin {body} end; }}");
    let (ws, l) = al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: vec![("f.al".into(), src)],
        }],
        &Default::default(),
    );
    assert!(ws.diagnostics.is_empty(), "{:?}", ws.diagnostics);
    let blocked: Vec<Vec<Option<String>>> = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    let ri = l.program.objects[oi]
        .routines
        .iter()
        .position(|r| r.name == "F")
        .unwrap();
    if let Some(Some(r)) = blocked.get(oi).and_then(|o| o.get(ri)) {
        return Err(Signal::Unsupported(r.clone()));
    }
    let engine = Engine::new(l.program, blocked);
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    s.call(oi, ri, inst, vec![]).map(|(v, _)| v)
}

fn text(vars: &str, body: &str) -> String {
    match run_with(TABLES, vars, body) {
        Ok(Value::Text(s)) | Ok(Value::Code(s)) => s,
        other => panic!("{other:?}"),
    }
}

fn refusal(vars: &str, body: &str) -> String {
    match run_with(TABLES, vars, body) {
        Err(Signal::Unsupported(r)) => r,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

const NEW_CO: &str = "Co: Record Company;";
const MAKE_CO: &str = "Co.Name := 'OTHER'; Co.Insert(); Commit();";

#[test]
fn change_company_to_the_empty_name_is_the_current_company() {
    // record/TestRecordGet.al:145-153 (`ChangeCompany('')` is true: empty string = current company).
    let r = run_with(
        NO_COMPANY_TABLE,
        "R: Record \"CC Row\"; ok: Boolean;",
        "ok := R.ChangeCompany(''); exit(Format(ok) + R.CurrentCompany());",
    );
    assert!(
        matches!(r, Ok(Value::Text(ref s)) if s.starts_with("Yes") && s.len() > 3),
        "{r:?}"
    );
}

#[test]
fn change_company_to_an_unknown_name_is_false() {
    // record/TestSecurityFilteringContracts.al:149-160 (a name nobody created answers false). The
    // test's app does not use the Company table, so the session company is the only company.
    let r = run_with(
        NO_COMPANY_TABLE,
        "R: Record \"CC Row\"; ok: Boolean;",
        "ok := R.ChangeCompany('NonExistentCompany12345XYZ'); exit(Format(ok));",
    );
    assert!(matches!(r, Ok(Value::Text(ref s)) if s == "No"), "{r:?}");
    // With the Company table in the program: the same answer while no row has that name.
    assert_eq!(
        text(
            "R: Record \"CC Row\"; ok: Boolean;",
            "ok := R.ChangeCompany('NOPE'); exit(Format(ok));"
        ),
        "No"
    );
}

#[test]
fn the_answer_follows_the_company_table() {
    // record/TestRecordChangeCompanyRows.al:128-145 (ChangeCompany_ToCompanyThatNoLongerExists_AnswersFalse).
    let out = text(
        &format!("{NEW_CO} R: Record \"CC Row\"; L: Record \"CC Row\";"),
        &format!(
            "{MAKE_CO} Co.Get('OTHER');
             exit(Format(R.ChangeCompany('OTHER')) + '|' + R.CurrentCompany() + '|');
             Co.Delete(); Commit();
             exit(Format(L.ChangeCompany('OTHER')));"
        ),
    );
    // the first exit ends the routine; the deleted-company arm is the next test
    assert_eq!(out, "Yes|OTHER|");
    let out = text(
        &format!("{NEW_CO} R: Record \"CC Row\"; L: Record \"CC Row\"; a: Text;"),
        &format!(
            "{MAKE_CO} a := Format(R.ChangeCompany('OTHER'));
             Co.Get('OTHER'); Co.Delete(); Commit();
             exit(a + Format(L.ChangeCompany('OTHER')));"
        ),
    );
    assert_eq!(out, "YesNo");
}

/// AL that appends `Format(e) + ';'` to `s` for each expression, between the statements.
fn log(e: &str) -> String {
    format!("s += Format({e}) + ';';")
}

#[test]
fn the_rows_of_two_companies_are_apart_and_a_table_not_per_company_is_shared() {
    // record/TestRecordChangeCompanyRows.al:75-132 (ChangeCompany_ToInsertedCompany_ReadsAndWritesThatCompanysOwnRows).
    let out = text(
        &format!(
            "{NEW_CO} Home: Record \"CC Row\"; Other: Record \"CC Row\"; Back: Record \"CC Row\";
             T: Record \"CC Tenant\"; TOther: Record \"CC Tenant\"; s: Text;"
        ),
        &format!(
            "Home.Id := 1; Home.V := 11; Home.Insert(); {MAKE_CO}
             {} {} {}
             Other.Id := 1; Other.V := 22; Other.Insert();
             {}
             Other.Get(1); {}
             Home.Get(1); {} {}
             Other.V := 33; Other.Modify(); Home.Get(1); {}
             Other.DeleteAll(); {} {}
             {} Back.Get(1); {} {}
             T.Id := 7; T.Insert();
             {} {} {}
             exit(s);",
            log("Other.ChangeCompany('OTHER')"),
            log("Other.CurrentCompany()"),
            log("Other.Count()"),
            log("Other.Count()"),
            log("Other.V"),
            log("Home.V"),
            log("Home.Count()"),
            log("Home.V"),
            log("Other.Count()"),
            log("Home.Count()"),
            log("Back.ChangeCompany(CompanyName())"),
            log("Back.Count()"),
            log("Back.V"),
            log("TOther.ChangeCompany('OTHER')"),
            log("TOther.Count()"),
            log("TOther.Get(7)"),
        ),
    );
    assert_eq!(out, "Yes;OTHER;0;1;22;11;1;11;0;1;Yes;1;11;Yes;1;Yes;");
}

#[test]
fn a_table_relation_is_validated_against_the_rows_of_the_records_own_company() {
    // record/TestRecordChangeCompanyRows.al:121-131: the session company holds Id 1, the other
    // company does not, so Validate fails there until the other company holds the row.
    let out = text(
        &format!("{NEW_CO} Home: Record \"CC Row\"; Other: Record \"CC Row\"; Rel: Record \"CC Rel\"; s: Text;"),
        &format!(
            "Home.Id := 1; Home.Insert(); {MAKE_CO}
             Rel.ChangeCompany('OTHER'); Rel.Id := 1;
             asserterror Rel.Validate(RowId, 1);
             {}
             Other.ChangeCompany('OTHER'); Other.Id := 1; Other.Insert();
             Rel.Validate(RowId, 1); {}
             exit(s);",
            log("StrPos(GetLastErrorText(), 'cannot be found in the related table') > 0"),
            log("Rel.RowId"),
        ),
    );
    assert_eq!(out, "Yes;1;");
}

#[test]
fn an_error_rolls_back_the_uncommitted_writes_in_the_other_company_only() {
    // record/TestRecordChangeCompanyRows.al:187-232 (ChangeCompany_ErrorRollsBackAnUncommittedWriteInTheOtherCompany).
    let out = text(
        &format!("{NEW_CO} Other: Record \"CC Row\"; s: Text;"),
        &format!(
            "{MAKE_CO} Other.ChangeCompany('OTHER');
             Other.Id := 4; Other.Insert(); {}
             asserterror Error('roll back the first write'); {}
             Other.Id := 5; Other.Insert(); Commit();
             Other.Id := 6; Other.Insert(); {}
             asserterror Error('roll back'); {} {} {}
             exit(s);",
            log("Other.Count()"),
            log("Other.Count()"),
            log("Other.Count()"),
            log("Other.Count()"),
            log("Other.Get(5)"),
            log("Other.Get(6)"),
        ),
    );
    assert_eq!(out, "1;0;2;1;Yes;No;");
}

#[test]
fn a_flowfield_counts_the_rows_of_the_company_its_record_is_on() {
    // record/TestRecordChangeCompanyRows.al:234-285 (ChangeCompany_FlowFieldOnARecordInTheOtherCompany_CountsThatCompanysRows).
    let out = text(
        &format!(
            "{NEW_CO} HP: Record \"CC Parent\"; HC: Record \"CC Child\"; OP: Record \"CC Parent\"; OC: Record \"CC Child\"; s: Text;"
        ),
        &format!(
            "HP.Id := 1; HP.Insert(); HC.Id := 1; HC.ParentId := 1; HC.Insert(); {MAKE_CO}
             OP.ChangeCompany('OTHER'); OC.ChangeCompany('OTHER');
             OP.Id := 1; OP.Insert();
             OC.Id := 1; OC.ParentId := 1; OC.Insert(); OC.Id := 2; OC.Insert(); OC.Id := 3; OC.Insert();
             OP.Get(1); OP.CalcFields(Kids); {}
             HP.Get(1); HP.CalcFields(Kids); {}
             exit(s);",
            log("OP.Kids"),
            log("HP.Kids"),
        ),
    );
    assert_eq!(out, "3;1;");
}

#[test]
fn recordref_open_and_change_company_use_the_other_companys_rows() {
    // record/TestRecordChangeCompanyRows.al:287-330 (RecordRef_ChangeCompany_ReadsAndWritesTheOtherCompanysRows).
    let out = text(
        &format!("{NEW_CO} Home: Record \"CC Row\"; RR: RecordRef; Opened: RecordRef; s: Text;"),
        &format!(
            "Home.Id := 1; Home.V := 11; Home.Insert(); {MAKE_CO}
             RR.Open(50910); {} {}
             RR.Init(); RR.Field(1).Value := 1; RR.Field(2).Value := 77; RR.Insert(); {}
             RR.Close();
             Home.Get(1); {} {}
             Opened.Open(50910, false, 'OTHER'); {} {}
             Opened.FindFirst(); {}
             exit(s);",
            log("RR.ChangeCompany('OTHER')"),
            log("RR.Count()"),
            log("RR.Count()"),
            log("Home.V"),
            log("Home.Count()"),
            log("Opened.CurrentCompany()"),
            log("Opened.Count()"),
            log("Opened.Field(2).Value"),
        ),
    );
    assert_eq!(out, "Yes;0;1;11;1;OTHER;1;77;");
}

#[test]
fn recordref_open_with_the_session_company_name_opens_the_session_company() {
    // Base App's Change Log Management.LogModification opens `xRecRef.Open(No, false, RecRef.CurrentCompany())`;
    // the onprem tests of the User table pass through it (access-control/TestUserAuthEmailTrigger.al:187).
    let out = text(
        "Home: Record \"CC Row\"; Opened: RecordRef; s: Text;",
        &format!(
            "Home.Id := 1; Home.Insert();
             Opened.Open(50910, false, Home.CurrentCompany()); {}
             Opened.Close(); Opened.Open(50910, false, ''); {}
             exit(s);",
            log("Opened.Count()"),
            log("Opened.Count()"),
        ),
    );
    assert_eq!(out, "1;1;");
}

#[test]
fn deleting_a_company_drops_its_rows() {
    // Not pinned: a deleted company holds no rows when a company of that name is made again
    // (BC drops a company's data with it; docs/probes-needed.md). The corpus deletes the company
    // it made at the end of every ChangeCompany test (record/TestRecordChangeCompanyRows.al).
    let out = text(
        &format!("{NEW_CO} Other: Record \"CC Row\"; s: Text;"),
        &format!(
            "{MAKE_CO} Other.ChangeCompany('OTHER'); Other.Id := 1; Other.Insert(); Commit();
             Co.Get('OTHER'); Co.Delete(); Commit();
             {MAKE_CO} Other.ChangeCompany('OTHER'); {}
             exit(s);",
            log("Other.Count()"),
        ),
    );
    assert_eq!(out, "0;");
}

// ---- record/TestChangeCompanyFiltersCopyDeleteContracts.al (BC 27.5) -------------------------------------------

#[test]
fn a_missing_company_is_false_when_the_result_is_used_and_an_error_when_it_is_not() {
    // :215-231. The session company is the first row of the Company table here, so the session name exists.
    let out = text(
        "R: Record \"CC Row\"; T: Record \"CC Row\" temporary; ok: Boolean; s: Text;",
        "ok := R.ChangeCompany('NoSuchCompany'); s += Format(ok) + Format(R.CurrentCompany() = CompanyName());
         asserterror R.ChangeCompany('NoSuchCompany');
         s += '|' + GetLastErrorText() + '|' + GetLastErrorCode();
         ok := T.ChangeCompany('NoSuchCompany'); s += '|' + Format(ok); exit(s);",
    );
    assert_eq!(
        out,
        "NoYes|The company \"NoSuchCompany\" does not exist.|DB:RecordNotFound|No"
    );
}

#[test]
fn the_company_name_is_case_sensitive() {
    // :234-246.
    let out = text(
        "R: Record \"CC Row\"; ok: Boolean; s: Text;",
        "ok := R.ChangeCompany(''); s += Format(ok) + Format(R.CurrentCompany() = CompanyName());
         ok := R.ChangeCompany(LowerCase(CompanyName())); s += '|' + Format(ok) + Format(R.CurrentCompany() = CompanyName()); exit(s);",
    );
    assert_eq!(out, "YesYes|NoYes");
}

#[test]
fn record_ref_open_on_a_missing_company_raises_the_same_error() {
    // :249-257.
    let out = text(
        "Ref: RecordRef;",
        "asserterror Ref.Open(50910, false, 'NoSuchCompany'); exit(GetLastErrorText() + '|' + GetLastErrorCode());",
    );
    assert_eq!(out, "The company \"NoSuchCompany\" does not exist.|DB:RecordNotFound");
}

#[test]
fn change_company_keeps_the_filters_the_marks_and_works_on_a_temporary_record() {
    // :25-62 (the company arm): filters, the current key and the marks stay; a temporary record keeps its rows.
    let out = text(
        &format!("{NEW_CO} R: Record \"CC Row\"; T: Record \"CC Row\" temporary; s: Text;"),
        &format!(
            "R.Id := 1; R.Insert(); R.Mark(true); R.SetFilter(Id, '>0');
             {MAKE_CO}
             {} s += R.GetFilter(Id) + '|';
             T.Id := 5; T.Insert(); {} {}
             exit(s);",
            log("R.ChangeCompany('OTHER')"),
            log("T.ChangeCompany('OTHER')"),
            log("T.Count()"),
        ),
    );
    assert_eq!(out, "Yes;>0|Yes;1;");
}

// ---- session/TestCrmTableNoConnectionContracts.al:111-118 (BC 27.5) --------------------------------------------

#[test]
fn record_ref_open_of_a_crm_table_without_a_connection_raises_the_connection_error() {
    let tables = format!(
        "{TABLES}\ntable 50920 \"CC Crm\" {{ TableType = CRM; fields {{ field(1; Id; Guid) {{ }} }} keys {{ key(PK; Id) {{ }} }} }}\n"
    );
    let out = match run_with(
        &tables,
        "Ref: RecordRef;",
        "asserterror Ref.Open(50920); exit(GetLastErrorText() + '|' + GetLastErrorCode());",
    ) {
        Ok(Value::Text(s)) => s,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        out,
        "Table connection  for table type CRM must be registered using RegisterTableConnection or cmdlet New-NAVTableConnection before it can be used.|Argument"
    );
}
