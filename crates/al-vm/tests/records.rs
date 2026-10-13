//! Records end to end: AL source → compile → run against the in-memory store.

use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_sema::workspace::AppSource;
use al_vm::{Engine, Session};

const TABLE: &str = r#"
table 50900 "ALT T"
{
    fields
    {
        field(1; "No."; Code[20]) { }
        field(2; Amount; Decimal) { InitValue = 5; trigger OnValidate() begin Log += 'V' + Format(xRec.Amount) + '>' + Format(Amount); end; }
        field(3; Grp; Integer) { }
        field(4; Log; Text[250]) { }
    }
    keys { key(PK; "No.") { } key(ByGrp; Grp, "No.") { } }

    trigger OnInsert() begin Log += 'I'; end;
    trigger OnModify() begin Log += 'M'; end;
    trigger OnDelete() begin if "No." = 'X' then Error('cannot delete X'); end;

    procedure Bump(N: Decimal) begin Amount += N; end;
}
"#;

fn run(body: &str, routine: &str) -> Result<Value, Signal> {
    let src = format!("{TABLE}\ncodeunit 50100 C {{ {body} }}");
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
        .position(|r| r.name == routine)
        .unwrap();
    if let Some(Some(r)) = blocked.get(oi).and_then(|o| o.get(ri)) {
        panic!("{routine} is blocked: {r}");
    }
    let engine = Engine::new(l.program, blocked);
    let mut s = Session::new(engine);
    let inst = s.instance(oi);
    s.call(oi, ri, inst, vec![]).map(|(v, _)| v)
}

fn text(r: Result<Value, Signal>) -> String {
    match r {
        Ok(Value::Text(s)) | Ok(Value::Code(s)) => s,
        other => panic!("{other:?}"),
    }
}

#[test]
fn insert_get_count_deleteall() {
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; begin
            R.\"No.\" := 'a'; R.Insert();
            R.\"No.\" := 'b'; R.Insert();
            Clear(R);
            R.Get('A');
            exit(Format(R.Count()) + R.\"No.\" + Format(R.IsEmpty()));
        end;",
        "F",
    );
    assert_eq!(text(r), "2ANo");
}

#[test]
fn init_applies_initvalue_and_keeps_the_primary_key() {
    // Doc (Record.Init): primary key fields are not initialized.
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; begin R.\"No.\" := 'K'; R.Amount := 9; R.Init(); exit(R.\"No.\" + Format(R.Amount)); end;",
        "F",
    );
    assert_eq!(text(r), "K5");
}

#[test]
fn duplicate_insert_raises_or_returns_false() {
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; begin R.\"No.\" := 'a'; R.Insert(); asserterror R.Insert(); exit(GetLastErrorText()); end;",
        "F",
    );
    assert!(text(r).contains("already exists"));
    let r = run(
        "procedure F(): Boolean var R: Record \"ALT T\"; begin R.\"No.\" := 'a'; R.Insert(); exit(R.Insert()); end;",
        "F",
    );
    assert!(matches!(r, Ok(Value::Boolean(false))));
}

#[test]
fn get_missing_raises_or_returns_false() {
    let r = run(
        "procedure F(): Boolean var R: Record \"ALT T\"; begin exit(R.Get('nope')); end;",
        "F",
    );
    assert!(matches!(r, Ok(Value::Boolean(false))));
    let r = run("procedure F() var R: Record \"ALT T\"; begin R.Get('nope'); end;", "F");
    assert!(matches!(r, Err(Signal::Error(e)) if e.message.contains("does not exist")));
}

#[test]
fn modify_and_delete() {
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; begin
            R.\"No.\" := 'a'; R.Insert();
            R.Amount := 7; R.Modify();
            Clear(R); R.Get('A');
            R.Delete();
            exit(Format(R.Get('A')) + Format(R.Amount));
        end;",
        "F",
    );
    // Get fails (returns No) and leaves the buffer as it was (Amount 7).
    assert_eq!(text(r), "No7");
}

#[test]
fn find_and_next_follow_the_current_key() {
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; S: Text; begin
            R.\"No.\" := 'C'; R.Grp := 1; R.Insert();
            R.\"No.\" := 'A'; R.Grp := 2; R.Insert();
            R.\"No.\" := 'B'; R.Grp := 1; R.Insert();
            if R.FindSet() then repeat S += R.\"No.\"; until R.Next() = 0;
            S += '|';
            R.SetCurrentKey(Grp);
            if R.FindSet() then repeat S += R.\"No.\"; until R.Next() = 0;
            S += '|';
            if R.FindLast() then S += R.\"No.\";
            exit(S);
        end;",
        "F",
    );
    assert_eq!(text(r), "ABC|BCA|A");
}

#[test]
fn setrange_filters_and_reset() {
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; begin
            R.\"No.\" := 'A'; R.Grp := 1; R.Insert();
            R.\"No.\" := 'B'; R.Grp := 2; R.Insert();
            R.\"No.\" := 'C'; R.Grp := 3; R.Insert();
            R.SetRange(Grp, 2, 3);
            exit(Format(R.Count()) + Format(R.FindFirst()) + R.\"No.\");
        end;",
        "F",
    );
    assert_eq!(text(r), "2YesB");
    let r = run(
        "procedure F(): Integer var R: Record \"ALT T\"; begin R.\"No.\" := 'A'; R.Insert(); R.SetRange(Grp, 9); R.Reset(); exit(R.Count()); end;",
        "F",
    );
    assert!(matches!(r, Ok(Value::Integer(1))));
}

#[test]
fn deleteall_respects_filters() {
    let r = run(
        "procedure F(): Integer var R: Record \"ALT T\"; begin
            R.\"No.\" := 'A'; R.Grp := 1; R.Insert();
            R.\"No.\" := 'B'; R.Grp := 2; R.Insert();
            R.SetRange(Grp, 2); R.DeleteAll(); R.Reset();
            exit(R.Count());
        end;",
        "F",
    );
    assert!(matches!(r, Ok(Value::Integer(1))));
}

#[test]
fn triggers_run_only_when_asked() {
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; begin
            R.\"No.\" := 'A'; R.Insert(true);
            R.Modify(true);
            R.\"No.\" := 'B'; R.Insert(false);
            exit(R.Log);
        end;",
        "F",
    );
    // OnInsert then OnModify ran on A's buffer; B's insert ran no trigger.
    assert_eq!(text(r), "IM");
}

#[test]
fn trigger_errors_propagate() {
    let r = run(
        "procedure F() var R: Record \"ALT T\"; begin R.\"No.\" := 'X'; R.Insert(); R.Delete(true); end;",
        "F",
    );
    assert!(matches!(r, Err(Signal::Error(e)) if e.message == "cannot delete X"));
}

#[test]
fn validate_runs_onvalidate_with_xrec() {
    // xRec.Amount is 5: InitValue applies at variable declaration
    // (record/TestInitValueContracts.al:204).
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; begin R.Validate(Amount, 8); exit(R.Log); end;",
        "F",
    );
    assert_eq!(text(r), "V5>8");
}

#[test]
fn assignment_copies_fields_but_not_filters() {
    let r = run(
        "procedure F(): Text var A: Record \"ALT T\"; B: Record \"ALT T\"; begin
            A.\"No.\" := 'A'; A.Insert();
            A.\"No.\" := 'B'; A.Insert();
            A.SetRange(\"No.\", 'A');
            B := A;
            B.\"No.\" := 'Z';
            exit(A.\"No.\" + B.\"No.\" + Format(B.Count()));
        end;",
        "F",
    );
    assert_eq!(text(r), "BZ2");
}

#[test]
fn record_parameters_by_value_and_by_var() {
    let r = run(
        "local procedure ByVal(R: Record \"ALT T\") begin R.\"No.\" := 'V'; end;
         local procedure ByVar(var R: Record \"ALT T\") begin R.\"No.\" := 'R'; end;
         procedure F(): Text var R: Record \"ALT T\"; S: Text; begin R.\"No.\" := 'A'; ByVal(R); S := R.\"No.\"; ByVar(R); exit(S + R.\"No.\"); end;",
        "F",
    );
    assert_eq!(text(r), "AR");
}

#[test]
fn table_procedures_run_on_the_receiver() {
    let r = run(
        "procedure F(): Decimal var R: Record \"ALT T\"; begin R.Bump(2); R.Bump(3); exit(R.Amount); end;",
        "F",
    );
    assert!(matches!(r, Ok(Value::Decimal(d)) if d == 10.into()), "{r:?}");
}

#[test]
fn asserterror_rolls_back_to_the_last_commit() {
    // record/TestRecordRenameRollback.al: an error caught by asserterror rolls back to the last commit point.
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; begin
            R.\"No.\" := 'A'; R.Insert(); Commit();
            asserterror begin R.\"No.\" := 'B'; R.Insert(); Error('x'); end;
            exit(Format(R.Get('A')) + Format(R.Get('B')));
        end;",
        "F",
    );
    assert_eq!(text(r), "YesNo");
}

#[test]
fn temporary_records_have_their_own_table() {
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; T: Record \"ALT T\" temporary; begin
            T.\"No.\" := 'A'; T.Insert();
            exit(Format(T.Count()) + Format(R.Count()));
        end;",
        "F",
    );
    assert_eq!(text(r), "10");
}

// record/TestTempTableContracts.al:TempVar_PassedByValue_CallerNotAffected -- a temporary record
// passed by value arrives with an independent, empty table; by var it is the caller's table.
#[test]
fn temporary_records_passed_by_value_get_an_independent_empty_table() {
    let r = run(
        "procedure F(): Text var Caller: Record \"ALT T\" temporary; begin
            Caller.\"No.\" := 'A'; Caller.Insert();
            exit(Format(ByValue(Caller)) + Format(Caller.Count()) + Format(ByRef(Caller)) + Format(Caller.Count()));
        end;
        local procedure ByValue(T: Record \"ALT T\" temporary): Integer begin T.\"No.\" := 'B'; T.Insert(); exit(T.Count()); end;
        local procedure ByRef(var T: Record \"ALT T\" temporary): Integer begin T.\"No.\" := 'C'; T.Insert(); exit(T.Count()); end;",
        "F",
    );
    assert_eq!(text(r), "1122");
}

// record/TestRefusedWriteRowVersion.al -- a refused write hands the record no rowversion.
#[test]
fn refused_writes_leave_the_record_without_a_rowversion() {
    let r = run(
        "procedure F(): Text var A: Record \"ALT T\"; B: Record \"ALT T\"; C: Record \"ALT T\"; begin
            A.\"No.\" := 'a'; A.Insert();
            B.\"No.\" := 'a';
            C.\"No.\" := 'zz';
            exit(Format(B.Insert()) + Format(B.SystemRowVersion) + Format(C.Modify()) + Format(C.SystemRowVersion)
                + Format(A.SystemRowVersion <> 0));
        end;",
        "F",
    );
    assert_eq!(text(r), "No0No0Yes");
}

fn run_as_test(body: &str, routine: &str) -> Result<Value, Signal> {
    let src = format!("{TABLE}\ncodeunit 50100 C {{ {body} }}");
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
        .position(|r| r.name == routine)
        .unwrap();
    let engine = Engine::new(l.program, blocked);
    let mut s = Session::new(engine);
    s.begin_test();
    let inst = s.instance(oi);
    s.call(oi, ri, inst, vec![]).map(|(v, _)| v)
}

// record/TestAssertErrorRollback.al (Initialize commits; a plain Error() after later writes rolls
// them back to that commit) against codeunit/TestScopeIsolationContracts.al:Test04... and
// session/TestTransactionContracts.al:Error_After_Insert_Before_Commit_RecordPersists (the same
// shape, no commit in the test: the row survives). The corpus does not say what separates them,
// so within a test, a plain Error() caught after pending writes and no Commit() in the test is
// Unsupported. Errors of the write itself, writes inside the block, committed writes and code
// outside a test are unaffected.
#[test]
fn asserterror_after_uncommitted_writes_without_a_commit_in_the_test_is_unsupported() {
    let after_insert =
        "procedure F() var R: Record \"ALT T\"; begin R.\"No.\" := 'a'; R.Insert(); asserterror Error('x'); end;";
    let r = run_as_test(after_insert, "F");
    assert!(
        matches!(&r, Err(Signal::Unsupported(m)) if m.contains("asserterror")),
        "{r:?}"
    );
    // The same code outside a test run keeps rolling back to the last commit.
    assert!(run(after_insert, "F").is_ok());
    // A Commit in the test moves the boundary: 60943's shape.
    let r = run_as_test(
        "procedure F(): Text var R: Record \"ALT T\"; begin Commit(); R.\"No.\" := 'a'; R.Insert(); asserterror Error('x'); exit(Format(R.Count())); end;",
        "F",
    );
    assert_eq!(text(r), "0");
    // Writes inside the asserterror block are rolled back (record/TestRecordRenameRollback.al).
    let r = run_as_test(
        "procedure F(): Text var R: Record \"ALT T\"; begin asserterror begin R.\"No.\" := 'a'; R.Insert(); Error('x'); end; exit(Format(R.Count())); end;",
        "F",
    );
    assert_eq!(text(r), "0");
    // An error of the write itself (not a plain Error()) is not covered.
    let r = run_as_test(
        "procedure F(): Text var R: Record \"ALT T\"; begin R.\"No.\" := 'a'; R.Insert(); asserterror R.Insert(); exit(GetLastErrorText()); end;",
        "F",
    );
    assert!(text(r).contains("already exists"));
}

#[test]
fn find_without_argument_is_find_equal_by_primary_key() {
    // Doc (Record.Find): Which defaults to '='.
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; begin
            R.\"No.\" := 'A'; R.Grp := 1; R.Insert();
            R.Grp := 7;
            exit(Format(R.Find()) + Format(R.Grp));
        end;",
        "F",
    );
    assert_eq!(text(r), "Yes1");
}

#[test]
fn find_equal_with_filters_is_not_guessed() {
    let r = run(
        "procedure F(): Boolean var R: Record \"ALT T\"; begin R.\"No.\" := 'A'; R.Insert(); R.SetRange(Grp, 1); exit(R.Find()); end;",
        "F",
    );
    assert!(matches!(r, Err(Signal::Unsupported(_))));
}

#[test]
fn date_and_time_init_values() {
    let src = "table 50910 D { fields { field(1; K; Integer) { } field(2; Dt; Date) { InitValue = 20240115D; } field(3; Tm; Time) { InitValue = 120000T; } } }\n";
    let body =
        "procedure F(): Text var R: Record D; begin exit(Format(R.Dt, 0, 9) + ' ' + Format(R.Tm = 120000T)); end;";
    let (ws, l) = al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: vec![("f.al".into(), format!("{src}codeunit 50100 C {{ {body} }}"))],
        }],
        &Default::default(),
    );
    assert!(ws.diagnostics.is_empty());
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    let blocked = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let mut s = Session::new(Engine::new(l.program, blocked));
    let inst = s.instance(oi);
    assert_eq!(text(s.call(oi, 0, inst, vec![]).map(|(v, _)| v)), "2024-01-15 Yes");
}

#[test]
fn rename_of_a_table_other_fields_relate_to_is_unsupported() {
    let src = "table 50911 P { fields { field(1; K; Code[10]) { } } }\ntable 50912 Ch { fields { field(1; Id; Integer) { } field(2; Parent; Code[10]) { TableRelation = P.K; } } }\n";
    let body = "procedure F() var R: Record P; begin R.K := 'A'; R.Insert(); R.Rename('B'); end;";
    let (_, l) = al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: vec![("f.al".into(), format!("{src}codeunit 50100 C {{ {body} }}"))],
        }],
        &Default::default(),
    );
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    let blocked = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let mut s = Session::new(Engine::new(l.program, blocked));
    let inst = s.instance(oi);
    assert!(matches!(s.call(oi, 0, inst, vec![]), Err(Signal::Unsupported(r)) if r.contains("relat")));
}

#[test]
fn setfilter_with_expressions_and_placeholders() {
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; begin
            R.\"No.\" := 'AP1'; R.Grp := 5; R.Insert();
            R.\"No.\" := 'AP2'; R.Grp := 15; R.Insert();
            R.\"No.\" := 'B'; R.Grp := 25; R.Insert();
            R.SetFilter(\"No.\", 'ap*');
            R.SetFilter(Grp, '>%1', 10);
            exit(Format(R.Count()) + Format(R.FindFirst()) + R.\"No.\");
        end;",
        "F",
    );
    assert_eq!(text(r), "1YesAP2");
}

#[test]
fn manual_subscribers_that_nothing_binds_do_not_block_writes() {
    // corpus _fixtures/events/ALTManualTableEventSubscriber.al: a Manual subscriber only fires while bound.
    let src = format!("{TABLE}\ncodeunit 50101 Sub {{ EventSubscriberInstance = Manual;
        [EventSubscriber(ObjectType::Table, Database::\"ALT T\", 'OnAfterInsertEvent', '', false, false)]
        local procedure OnAfterInsert(var Rec: Record \"ALT T\") begin Error('must not fire'); end; }}
        codeunit 50100 C {{ procedure F(): Integer var R: Record \"ALT T\"; begin R.\"No.\" := 'A'; R.Insert(); exit(R.Count()); end; }}");
    let (_, l) = al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: vec![("f.al".into(), src)],
        }],
        &Default::default(),
    );
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    let blocked = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let mut s = Session::new(Engine::new(l.program, blocked));
    let inst = s.instance(oi);
    assert!(matches!(s.call(oi, 0, inst, vec![]), Ok((Value::Integer(1), _))));
}

#[test]
fn extension_field_triggers_do_not_block_other_writes() {
    let src = format!("{TABLE}\ntableextension 50102 X extends \"ALT T\" {{ fields {{ field(50000; Ext; Integer) {{ trigger OnValidate() begin Log := 'ext'; end; }} }} }}
        codeunit 50100 C {{ procedure F(): Text var R: Record \"ALT T\"; begin R.\"No.\" := 'A'; R.Insert(); R.Validate(Ext, 1); R.DeleteAll(); exit(R.Log + Format(R.Count())); end; }}");
    let (_, l) = al_sema::lower::compile(
        vec![AppSource {
            id: "a".into(),
            name: "a".into(),
            dependencies: vec![],
            files: vec![("f.al".into(), src)],
        }],
        &Default::default(),
    );
    let oi = l.program.objects.iter().position(|o| o.name == "C").unwrap();
    let blocked = l
        .unsupported
        .iter()
        .map(|o| o.iter().map(|r| r.first().cloned()).collect())
        .collect();
    let mut s = Session::new(Engine::new(l.program, blocked));
    let inst = s.instance(oi);
    assert_eq!(text(s.call(oi, 0, inst, vec![]).map(|(v, _)| v)), "ext0");
}

#[test]
fn writes_assign_increasing_row_versions() {
    let r = run(
        "procedure F(): Text var R: Record \"ALT T\"; V1: BigInteger; V2: BigInteger; begin
            R.\"No.\" := 'A'; R.Insert(); R.Get('A'); V1 := R.SystemRowVersion;
            R.Grp := 1; R.Modify(); R.Get('A'); V2 := R.SystemRowVersion;
            exit(Format(V1 > 0) + Format(V2 > V1));
        end;",
        "F",
    );
    assert_eq!(text(r), "YesYes");
}

// record/TestRenamePropagationConditional.al: a rename must re-key related rows, and a conditional
// relation relates to every table of its arms (the else table too). Re-keying is not
// implemented, so renaming any table some relation points to is refused.
#[test]
fn renaming_a_table_any_arm_of_a_relation_points_to_is_refused() {
    let src = r#"
table 50901 PA { fields { field(1; Code; Code[10]) { } } keys { key(PK; Code) { } } }
table 50902 PB { fields { field(1; Code; Code[10]) { } } keys { key(PK; Code) { } } }
table 50903 PC { fields { field(1; Code; Code[10]) { } } keys { key(PK; Code) { } } }
table 50904 Child
{
    fields
    {
        field(1; Id; Integer) { }
        field(2; Kind; Option) { OptionMembers = A,B; }
        field(3; Ref; Code[10]) { TableRelation = if (Kind = const(A)) PA.Code else PB.Code; }
    }
    keys { key(PK; Id) { } }
}
codeunit 50100 C
{
    procedure Rename(Which: Integer)
    var A: Record PA; B: Record PB; C: Record PC;
    begin
        case Which of
            1: begin A.Code := 'X'; A.Insert(); A.Rename('Y'); end;
            2: begin B.Code := 'X'; B.Insert(); B.Rename('Y'); end;
            3: begin C.Code := 'X'; C.Insert(); C.Rename('Y'); end;
        end;
    end;
    procedure R1() begin Rename(1); end;
    procedure R2() begin Rename(2); end;
    procedure R3() begin Rename(3); end;
}"#;
    let call = |r: &str| {
        let (ws, l) = al_sema::lower::compile(
            vec![AppSource {
                id: "a".into(),
                name: "a".into(),
                dependencies: vec![],
                files: vec![("f.al".into(), src.into())],
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
        let ri = l.program.objects[oi].routines.iter().position(|x| x.name == r).unwrap();
        let mut s = Session::new(Engine::new(l.program, blocked));
        let inst = s.instance(oi);
        s.call(oi, ri, inst, vec![]).map(|(v, _)| v)
    };
    assert!(matches!(call("R1"), Err(Signal::Unsupported(m)) if m.contains("relate")));
    assert!(matches!(call("R2"), Err(Signal::Unsupported(m)) if m.contains("relate")));
    assert!(call("R3").is_ok(), "{:?}", call("R3"));
}

// record/TestRecordStaleBufferWrite.al: a Modify through a buffer another variable has since
// changed is accepted (last writer wins) -- unless a Commit sits between that other write and
// this one, which refuses it with DB:RecordChanged. A buffer that never read the row (no
// rowversion) is never stale.
#[test]
fn modify_through_a_stale_buffer_is_refused_only_after_a_commit() {
    let flow = |commit: bool| {
        run(
            &format!(
                "procedure F(): Text var Rec: Record \"ALT T\"; Stale: Record \"ALT T\"; begin
                    Rec.\"No.\" := 'a'; Rec.Insert(); Commit();
                    Stale.Get('a'); Rec.Get('a'); Rec.Grp := 42; Rec.Modify();
                    {c}
                    Stale.Grp := 7;
                    if {commit} then begin
                        asserterror Stale.Modify();
                        exit(GetLastErrorCode() + '|' + GetLastErrorText());
                    end;
                    Stale.Modify();
                    Rec.Get('a');
                    exit(Format(Rec.Grp));
                end;",
                c = if commit { "Commit();" } else { "" },
                commit = if commit { "true" } else { "false" }
            ),
            "F",
        )
    };
    let refused = text(flow(true));
    assert!(refused.starts_with("DB:RecordChanged|"), "{refused}");
    assert!(
        refused.contains("not up-to-date") && refused.contains("No.='A'"),
        "{refused}"
    );
    assert_eq!(text(flow(false)), "7");
    // A buffer that never read the row has no rowversion to be stale against.
    let r = run(
        "procedure F(): Integer var Rec: Record \"ALT T\"; Fresh: Record \"ALT T\"; begin
            Rec.\"No.\" := 'a'; Rec.Insert(); Commit();
            Rec.Grp := 1; Rec.Modify(); Commit();
            Fresh.\"No.\" := 'a'; Fresh.Grp := 9; Fresh.Modify();
            Rec.Get('a'); exit(Rec.Grp);
        end;",
        "F",
    );
    assert!(matches!(r, Ok(Value::Integer(9))), "{r:?}");
}
